//! Streaming ZIP reader for EPUB files
//!
//! Memory-efficient ZIP reader that streams files without loading entire archive.
//! Uses fixed-size central directory cache (max 256 entries, ~4KB).
//! Supports DEFLATE decompression using miniz_oxide.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::boxed::Box;
use heapless::Vec as HeaplessVec;
use log;
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use std::io::{Read, Seek, SeekFrom, Write};

#[cfg(target_os = "espidf")]
const DEFAULT_ZIP_SCRATCH_BYTES: usize = 2 * 1024;
#[cfg(not(target_os = "espidf"))]
const DEFAULT_ZIP_SCRATCH_BYTES: usize = 8 * 1024;

/// Maximum number of central directory entries to cache
const MAX_CD_ENTRIES: usize = 256;

/// Maximum filename length in ZIP entries
const MAX_FILENAME_LEN: usize = 256;

/// Runtime-configurable ZIP safety limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZipLimits {
    /// Maximum compressed or uncompressed file size allowed for reads.
    pub max_file_read_size: usize,
    /// Maximum allowed size for the required `mimetype` entry.
    pub max_mimetype_size: usize,
    /// Whether ZIP parsing should fail on strict structural issues.
    pub strict: bool,
    /// Maximum bytes scanned from file tail while searching for EOCD.
    pub max_eocd_scan: usize,
}

impl ZipLimits {
    /// Create explicit ZIP limits.
    pub fn new(max_file_read_size: usize, max_mimetype_size: usize) -> Self {
        Self {
            max_file_read_size,
            max_mimetype_size,
            strict: false,
            max_eocd_scan: MAX_EOCD_SCAN,
        }
    }

    /// Enable or disable strict ZIP parsing behavior.
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Set a cap for EOCD tail scan bytes.
    pub fn with_max_eocd_scan(mut self, max_eocd_scan: usize) -> Self {
        self.max_eocd_scan = max_eocd_scan.max(EOCD_MIN_SIZE);
        self
    }
}

/// Local file header signature (little-endian)
const SIG_LOCAL_FILE_HEADER: u32 = 0x04034b50;

/// Central directory entry signature (little-endian)
const SIG_CD_ENTRY: u32 = 0x02014b50;

/// End of central directory signature (little-endian)
const SIG_EOCD: u32 = 0x06054b50;
/// ZIP64 end of central directory record signature (little-endian)
const SIG_ZIP64_EOCD: u32 = 0x06064b50;
/// ZIP64 end of central directory locator signature (little-endian)
const SIG_ZIP64_EOCD_LOCATOR: u32 = 0x07064b50;
/// Minimum EOCD record size in bytes
const EOCD_MIN_SIZE: usize = 22;
/// Maximum EOCD search window (EOCD + max comment length)
const MAX_EOCD_SCAN: usize = EOCD_MIN_SIZE + u16::MAX as usize;

/// Compression methods
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;

// Re-export the crate's public ZIP error alias for module consumers.
pub use crate::error::ZipError;

#[derive(Clone, Copy, Debug)]
struct EocdInfo {
    cd_offset: u64,
    cd_size: u64,
    num_entries: u64,
}

#[derive(Clone, Copy, Debug)]
struct Zip64EocdInfo {
    disk_number: u32,
    disk_with_cd_start: u32,
    num_entries: u64,
    cd_size: u64,
    cd_offset: u64,
}

/// Central directory entry metadata
#[derive(Debug, Clone)]
pub struct CdEntry {
    /// Compression method (0=stored, 8=deflated)
    pub method: u16,
    /// Compressed size in bytes
    pub compressed_size: u64,
    /// Uncompressed size in bytes
    pub uncompressed_size: u64,
    /// Offset to local file header
    pub local_header_offset: u64,
    /// CRC32 checksum
    pub crc32: u32,
    /// Filename (max 255 chars)
    pub filename: String,
}

impl CdEntry {
    /// Create new empty entry
    fn new() -> Self {
        Self {
            method: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            local_header_offset: 0,
            crc32: 0,
            filename: String::with_capacity(0),
        }
    }
}

/// Streaming ZIP file reader
pub struct StreamingZip<F: Read + Seek> {
    /// File handle
    file: F,
    /// Central directory entries (fixed size)
    entries: HeaplessVec<CdEntry, MAX_CD_ENTRIES>,
    /// Number of entries in central directory
    num_entries: usize,
    /// Optional configurable resource/safety limits.
    limits: Option<ZipLimits>,
    /// Reusable DEFLATE state to avoid large hot-path allocations on embedded.
    inflate_state: Box<miniz_oxide::inflate::stream::InflateState>,
}

impl<F: Read + Seek> StreamingZip<F> {
    /// Open a ZIP file and parse the central directory
    pub fn new(file: F) -> Result<Self, ZipError> {
        Self::new_with_limits(file, None)
    }

    /// Open a ZIP file with explicit runtime limits.
    pub fn new_with_limits(mut file: F, limits: Option<ZipLimits>) -> Result<Self, ZipError> {
        // Find and parse EOCD
        let max_eocd_scan = limits
            .map(|l| l.max_eocd_scan.min(MAX_EOCD_SCAN))
            .unwrap_or(MAX_EOCD_SCAN);
        let eocd = Self::find_eocd(&mut file, max_eocd_scan)?;
        let strict = limits.is_some_and(|l| l.strict);
        if strict && eocd.num_entries > MAX_CD_ENTRIES as u64 {
            return Err(ZipError::CentralDirFull);
        }

        let mut entries: HeaplessVec<CdEntry, MAX_CD_ENTRIES> = HeaplessVec::new();

        // Parse central directory entries
        file.seek(SeekFrom::Start(eocd.cd_offset))
            .map_err(|_| ZipError::IoError)?;
        let cd_end = eocd
            .cd_offset
            .checked_add(eocd.cd_size)
            .ok_or(ZipError::InvalidFormat)?;

        let entries_to_scan = core::cmp::min(eocd.num_entries, MAX_CD_ENTRIES as u64);
        for _ in 0..entries_to_scan {
            let pos = file.stream_position().map_err(|_| ZipError::IoError)?;
            if pos >= cd_end {
                if strict {
                    return Err(ZipError::InvalidFormat);
                }
                break;
            }
            if let Some(entry) = Self::read_cd_entry(&mut file)? {
                entries.push(entry).map_err(|_| ZipError::CentralDirFull)?;
            } else if strict {
                return Err(ZipError::InvalidFormat);
            } else {
                break;
            }
        }

        if eocd.num_entries > MAX_CD_ENTRIES as u64 {
            log::warn!(
                "[ZIP] Archive has {} entries but only {} were loaded (max: {})",
                eocd.num_entries,
                entries.len(),
                MAX_CD_ENTRIES
            );
        }

        log::debug!(
            "[ZIP] Parsed {} central directory entries (offset {})",
            entries.len(),
            eocd.cd_offset
        );

        Ok(Self {
            file,
            entries,
            num_entries: core::cmp::min(eocd.num_entries, usize::MAX as u64) as usize,
            limits,
            inflate_state: Box::new(miniz_oxide::inflate::stream::InflateState::new(
                DataFormat::Raw,
            )),
        })
    }

    /// Find EOCD and extract central directory info
    fn find_eocd(file: &mut F, max_eocd_scan: usize) -> Result<EocdInfo, ZipError> {
        // Get file size
        let file_size = file.seek(SeekFrom::End(0)).map_err(|_| ZipError::IoError)?;

        if file_size < EOCD_MIN_SIZE as u64 {
            return Err(ZipError::InvalidFormat);
        }

        // Scan last (EOCD + max comment) bytes for EOCD signature.
        let scan_range = file_size.min(max_eocd_scan as u64) as usize;
        let mut buffer = alloc::vec![0u8; scan_range];

        file.seek(SeekFrom::Start(file_size - scan_range as u64))
            .map_err(|_| ZipError::IoError)?;
        let bytes_read = file.read(&mut buffer).map_err(|_| ZipError::IoError)?;
        let scan_base = file_size - bytes_read as u64;

        // Scan backwards for EOCD signature
        for i in (0..=bytes_read.saturating_sub(EOCD_MIN_SIZE)).rev() {
            if Self::read_u32_le(&buffer, i) == SIG_EOCD {
                // Found EOCD, extract info
                let num_entries = Self::read_u16_le(&buffer, i + 8);
                let cd_size_32 = Self::read_u32_le(&buffer, i + 12);
                let cd_offset_32 = Self::read_u32_le(&buffer, i + 16) as u64;
                let comment_len = Self::read_u16_le(&buffer, i + 20) as u64;
                let eocd_pos = scan_base + i as u64;
                let eocd_end = eocd_pos + EOCD_MIN_SIZE as u64 + comment_len;
                if eocd_end != file_size {
                    continue;
                }

                let uses_zip64_sentinel = num_entries == u16::MAX
                    || cd_size_32 == u32::MAX
                    || cd_offset_32 == u32::MAX as u64;

                let mut zip64_locator: Option<(u32, u64, u32)> = None;
                if eocd_pos >= 20 {
                    file.seek(SeekFrom::Start(eocd_pos - 20))
                        .map_err(|_| ZipError::IoError)?;
                    let mut locator = [0u8; 20];
                    file.read_exact(&mut locator)
                        .map_err(|_| ZipError::IoError)?;
                    if u32::from_le_bytes([locator[0], locator[1], locator[2], locator[3]])
                        == SIG_ZIP64_EOCD_LOCATOR
                    {
                        let zip64_disk =
                            u32::from_le_bytes([locator[4], locator[5], locator[6], locator[7]]);
                        let zip64_eocd_offset = u64::from_le_bytes([
                            locator[8],
                            locator[9],
                            locator[10],
                            locator[11],
                            locator[12],
                            locator[13],
                            locator[14],
                            locator[15],
                        ]);
                        let total_disks = u32::from_le_bytes([
                            locator[16],
                            locator[17],
                            locator[18],
                            locator[19],
                        ]);
                        zip64_locator = Some((zip64_disk, zip64_eocd_offset, total_disks));
                    }
                }

                if uses_zip64_sentinel || zip64_locator.is_some() {
                    let (zip64_disk, zip64_eocd_offset, total_disks) =
                        zip64_locator.ok_or(ZipError::InvalidFormat)?;
                    if zip64_disk != 0 || total_disks != 1 {
                        return Err(ZipError::UnsupportedZip64);
                    }
                    let zip64 = Self::read_zip64_eocd(file, zip64_eocd_offset)?;
                    if zip64.disk_number != 0 || zip64.disk_with_cd_start != 0 {
                        return Err(ZipError::UnsupportedZip64);
                    }
                    let cd_end = zip64
                        .cd_offset
                        .checked_add(zip64.cd_size)
                        .ok_or(ZipError::InvalidFormat)?;
                    if cd_end > eocd_pos || cd_end > file_size {
                        return Err(ZipError::InvalidFormat);
                    }
                    return Ok(EocdInfo {
                        cd_offset: zip64.cd_offset,
                        cd_size: zip64.cd_size,
                        num_entries: zip64.num_entries,
                    });
                }

                let cd_end = cd_offset_32
                    .checked_add(cd_size_32 as u64)
                    .ok_or(ZipError::InvalidFormat)?;
                if cd_end > eocd_pos || cd_end > file_size {
                    return Err(ZipError::InvalidFormat);
                }

                return Ok(EocdInfo {
                    cd_offset: cd_offset_32,
                    cd_size: cd_size_32 as u64,
                    num_entries: num_entries as u64,
                });
            }
        }

        Err(ZipError::InvalidFormat)
    }

    fn read_zip64_eocd(file: &mut F, offset: u64) -> Result<Zip64EocdInfo, ZipError> {
        file.seek(SeekFrom::Start(offset))
            .map_err(|_| ZipError::IoError)?;
        let mut fixed = [0u8; 56];
        file.read_exact(&mut fixed).map_err(|_| ZipError::IoError)?;

        let sig = u32::from_le_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]);
        if sig != SIG_ZIP64_EOCD {
            return Err(ZipError::InvalidFormat);
        }

        let record_size = u64::from_le_bytes([
            fixed[4], fixed[5], fixed[6], fixed[7], fixed[8], fixed[9], fixed[10], fixed[11],
        ]);
        if record_size < 44 {
            return Err(ZipError::InvalidFormat);
        }

        let disk_number = u32::from_le_bytes([fixed[16], fixed[17], fixed[18], fixed[19]]);
        let disk_with_cd_start = u32::from_le_bytes([fixed[20], fixed[21], fixed[22], fixed[23]]);
        let num_entries = u64::from_le_bytes([
            fixed[32], fixed[33], fixed[34], fixed[35], fixed[36], fixed[37], fixed[38], fixed[39],
        ]);
        let cd_size = u64::from_le_bytes([
            fixed[40], fixed[41], fixed[42], fixed[43], fixed[44], fixed[45], fixed[46], fixed[47],
        ]);
        let cd_offset = u64::from_le_bytes([
            fixed[48], fixed[49], fixed[50], fixed[51], fixed[52], fixed[53], fixed[54], fixed[55],
        ]);

        Ok(Zip64EocdInfo {
            disk_number,
            disk_with_cd_start,
            num_entries,
            cd_size,
            cd_offset,
        })
    }

    /// Read a central directory entry from file
    fn read_cd_entry(file: &mut F) -> Result<Option<CdEntry>, ZipError> {
        let mut sig_buf = [0u8; 4];
        if file.read_exact(&mut sig_buf).is_err() {
            return Ok(None);
        }
        let sig = u32::from_le_bytes(sig_buf);

        if sig != SIG_CD_ENTRY {
            return Ok(None); // End of central directory
        }

        // Read fixed portion of central directory entry (42 bytes = offsets 4-45)
        // This includes everything up to and including the local header offset
        let mut buf = [0u8; 42];
        file.read_exact(&mut buf).map_err(|_| ZipError::IoError)?;

        let mut entry = CdEntry::new();

        // Parse central directory entry fields
        // buf contains bytes 4-49 of the CD entry (after the 4-byte signature)
        // buf[N] corresponds to CD entry offset (N + 4)
        entry.method = u16::from_le_bytes([buf[6], buf[7]]); // CD offset 10
        entry.crc32 = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]); // CD offset 16
        let compressed_size_32 = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]); // CD offset 20
        let uncompressed_size_32 = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]); // CD offset 24
        let name_len = u16::from_le_bytes([buf[24], buf[25]]) as usize; // CD offset 28
        let extra_len = u16::from_le_bytes([buf[26], buf[27]]) as usize; // CD offset 30
        let comment_len = u16::from_le_bytes([buf[28], buf[29]]) as usize; // CD offset 32
        let local_header_offset_32 = u32::from_le_bytes([buf[38], buf[39], buf[40], buf[41]]); // CD offset 42
        entry.compressed_size = compressed_size_32 as u64;
        entry.uncompressed_size = uncompressed_size_32 as u64;
        entry.local_header_offset = local_header_offset_32 as u64;

        // Read filename
        if name_len > 0 && name_len <= MAX_FILENAME_LEN {
            let mut name_buf = alloc::vec![0u8; name_len];
            file.read_exact(&mut name_buf)
                .map_err(|_| ZipError::IoError)?;
            entry.filename = String::from_utf8_lossy(&name_buf).to_string();
        } else if name_len > MAX_FILENAME_LEN {
            // Skip over filename bytes we can't store
            file.seek(SeekFrom::Current(name_len as i64))
                .map_err(|_| ZipError::IoError)?;
        }

        let needs_zip64_uncompressed = uncompressed_size_32 == u32::MAX;
        let needs_zip64_compressed = compressed_size_32 == u32::MAX;
        let needs_zip64_offset = local_header_offset_32 == u32::MAX;
        let mut got_zip64_uncompressed = false;
        let mut got_zip64_compressed = false;
        let mut got_zip64_offset = false;

        // Parse ZIP extra fields, specifically ZIP64 extended information (0x0001).
        let mut extra_remaining = extra_len;
        while extra_remaining >= 4 {
            let mut hdr = [0u8; 4];
            file.read_exact(&mut hdr).map_err(|_| ZipError::IoError)?;
            let header_id = u16::from_le_bytes([hdr[0], hdr[1]]);
            let field_size = u16::from_le_bytes([hdr[2], hdr[3]]) as usize;
            extra_remaining -= 4;

            if field_size > extra_remaining {
                return Err(ZipError::InvalidFormat);
            }

            if header_id == 0x0001 {
                let mut field_remaining = field_size;
                if needs_zip64_uncompressed {
                    if field_remaining < 8 {
                        return Err(ZipError::InvalidFormat);
                    }
                    let mut val = [0u8; 8];
                    file.read_exact(&mut val).map_err(|_| ZipError::IoError)?;
                    entry.uncompressed_size = u64::from_le_bytes(val);
                    got_zip64_uncompressed = true;
                    field_remaining -= 8;
                }
                if needs_zip64_compressed {
                    if field_remaining < 8 {
                        return Err(ZipError::InvalidFormat);
                    }
                    let mut val = [0u8; 8];
                    file.read_exact(&mut val).map_err(|_| ZipError::IoError)?;
                    entry.compressed_size = u64::from_le_bytes(val);
                    got_zip64_compressed = true;
                    field_remaining -= 8;
                }
                if needs_zip64_offset {
                    if field_remaining < 8 {
                        return Err(ZipError::InvalidFormat);
                    }
                    let mut val = [0u8; 8];
                    file.read_exact(&mut val).map_err(|_| ZipError::IoError)?;
                    entry.local_header_offset = u64::from_le_bytes(val);
                    got_zip64_offset = true;
                    field_remaining -= 8;
                }
                if field_remaining > 0 {
                    file.seek(SeekFrom::Current(field_remaining as i64))
                        .map_err(|_| ZipError::IoError)?;
                }
            } else if field_size > 0 {
                file.seek(SeekFrom::Current(field_size as i64))
                    .map_err(|_| ZipError::IoError)?;
            }
            extra_remaining -= field_size;
        }
        if extra_remaining > 0 {
            file.seek(SeekFrom::Current(extra_remaining as i64))
                .map_err(|_| ZipError::IoError)?;
        }

        if (needs_zip64_uncompressed && !got_zip64_uncompressed)
            || (needs_zip64_compressed && !got_zip64_compressed)
            || (needs_zip64_offset && !got_zip64_offset)
        {
            return Err(ZipError::InvalidFormat);
        }

        if comment_len > 0 {
            file.seek(SeekFrom::Current(comment_len as i64))
                .map_err(|_| ZipError::IoError)?;
        }

        Ok(Some(entry))
    }

    /// Get entry by filename (case-insensitive)
    pub fn get_entry(&self, name: &str) -> Option<&CdEntry> {
        self.entries.iter().find(|e| {
            e.filename == name
                || e.filename.eq_ignore_ascii_case(name)
                || (name.starts_with('/') && e.filename.eq_ignore_ascii_case(&name[1..]))
                || (e.filename.starts_with('/') && e.filename[1..].eq_ignore_ascii_case(name))
        })
    }

    /// Debug: Log all entries in the ZIP (for troubleshooting)
    #[allow(dead_code)]
    fn debug_list_entries(&self) {
        log::info!(
            "[ZIP] Central directory contains {} entries:",
            self.entries.len()
        );
        for (i, entry) in self.entries.iter().enumerate() {
            log::info!(
                "[ZIP]  [{}] '{}' (method={}, compressed={}, uncompressed={})",
                i,
                entry.filename,
                entry.method,
                entry.compressed_size,
                entry.uncompressed_size
            );
        }
    }

    /// Read and decompress a file into the provided buffer
    /// Returns number of bytes written to buffer
    pub fn read_file(&mut self, entry: &CdEntry, buf: &mut [u8]) -> Result<usize, ZipError> {
        let mut input_buf = alloc::vec![0u8; DEFAULT_ZIP_SCRATCH_BYTES];
        self.read_file_with_scratch(entry, buf, &mut input_buf)
    }

    /// Read and decompress a file into the provided buffer using caller-provided scratch input.
    ///
    /// This is intended for embedded callers that want deterministic allocation behavior.
    /// `input_buf` must be non-empty.
    pub fn read_file_with_scratch(
        &mut self,
        entry: &CdEntry,
        buf: &mut [u8],
        input_buf: &mut [u8],
    ) -> Result<usize, ZipError> {
        if input_buf.is_empty() {
            return Err(ZipError::BufferTooSmall);
        }
        if let Some(limits) = self.limits {
            if entry.uncompressed_size > limits.max_file_read_size as u64 {
                return Err(ZipError::FileTooLarge);
            }
            if entry.compressed_size > limits.max_file_read_size as u64 {
                return Err(ZipError::FileTooLarge);
            }
        }
        let uncompressed_size =
            usize::try_from(entry.uncompressed_size).map_err(|_| ZipError::FileTooLarge)?;
        if uncompressed_size > buf.len() {
            return Err(ZipError::BufferTooSmall);
        }

        // Calculate data offset by reading local file header
        let data_offset = self.calc_data_offset(entry)?;

        // Seek to data
        self.file
            .seek(SeekFrom::Start(data_offset))
            .map_err(|_| ZipError::IoError)?;

        match entry.method {
            METHOD_STORED => {
                // Read stored data directly
                let size =
                    usize::try_from(entry.compressed_size).map_err(|_| ZipError::FileTooLarge)?;
                if size > buf.len() {
                    return Err(ZipError::BufferTooSmall);
                }
                self.file
                    .read_exact(&mut buf[..size])
                    .map_err(|_| ZipError::IoError)?;
                // Verify CRC32
                if entry.crc32 != 0 {
                    let calc_crc = crc32fast::hash(&buf[..size]);
                    if calc_crc != entry.crc32 {
                        return Err(ZipError::CrcMismatch);
                    }
                }
                Ok(size)
            }
            METHOD_DEFLATED => {
                self.inflate_state.reset(DataFormat::Raw);
                let mut compressed_remaining =
                    usize::try_from(entry.compressed_size).map_err(|_| ZipError::FileTooLarge)?;
                let mut pending = &[][..];
                let mut written = 0usize;

                loop {
                    if pending.is_empty() && compressed_remaining > 0 {
                        let take = core::cmp::min(compressed_remaining, input_buf.len());
                        self.file
                            .read_exact(&mut input_buf[..take])
                            .map_err(|_| ZipError::IoError)?;
                        pending = &input_buf[..take];
                        compressed_remaining -= take;
                    }

                    if written >= buf.len() && (compressed_remaining > 0 || !pending.is_empty()) {
                        return Err(ZipError::BufferTooSmall);
                    }

                    let result = miniz_oxide::inflate::stream::inflate(
                        &mut self.inflate_state,
                        pending,
                        &mut buf[written..],
                        MZFlush::None,
                    );
                    let consumed = result.bytes_consumed;
                    let produced = result.bytes_written;
                    pending = &pending[consumed..];
                    written += produced;

                    match result.status {
                        Ok(MZStatus::StreamEnd) => {
                            if compressed_remaining != 0 || !pending.is_empty() {
                                return Err(ZipError::DecompressError);
                            }
                            break;
                        }
                        Ok(MZStatus::Ok) => {
                            if consumed == 0 && produced == 0 {
                                return Err(ZipError::DecompressError);
                            }
                        }
                        Ok(MZStatus::NeedDict) => return Err(ZipError::DecompressError),
                        Err(_) => return Err(ZipError::DecompressError),
                    }
                }

                // Verify CRC32 if available
                if entry.crc32 != 0 {
                    let calc_crc = crc32fast::hash(&buf[..written]);
                    if calc_crc != entry.crc32 {
                        return Err(ZipError::CrcMismatch);
                    }
                }
                Ok(written)
            }
            _ => Err(ZipError::UnsupportedCompression),
        }
    }

    /// Stream a file's decompressed bytes into an arbitrary writer.
    ///
    /// For stored and DEFLATE entries this path is chunked and avoids full-entry output buffers.
    pub fn read_file_to_writer<W: Write>(
        &mut self,
        entry: &CdEntry,
        writer: &mut W,
    ) -> Result<usize, ZipError> {
        let mut input_buf = alloc::vec![0u8; DEFAULT_ZIP_SCRATCH_BYTES];
        let mut output_buf = alloc::vec![0u8; DEFAULT_ZIP_SCRATCH_BYTES];
        self.read_file_to_writer_with_scratch(entry, writer, &mut input_buf, &mut output_buf)
    }

    /// Stream a file's decompressed bytes into an arbitrary writer using caller-provided scratch buffers.
    ///
    /// This API is intended for embedded use cases where callers want strict control over
    /// allocation and stack usage. `input_buf` and `output_buf` must both be non-empty.
    ///
    /// For `METHOD_STORED`, only `input_buf` is used for chunked copying.
    /// For `METHOD_DEFLATED`, both buffers are used.
    pub fn read_file_to_writer_with_scratch<W: Write>(
        &mut self,
        entry: &CdEntry,
        writer: &mut W,
        input_buf: &mut [u8],
        output_buf: &mut [u8],
    ) -> Result<usize, ZipError> {
        if input_buf.is_empty() || output_buf.is_empty() {
            return Err(ZipError::BufferTooSmall);
        }
        if let Some(limits) = self.limits {
            if entry.uncompressed_size > limits.max_file_read_size as u64 {
                return Err(ZipError::FileTooLarge);
            }
            if entry.compressed_size > limits.max_file_read_size as u64 {
                return Err(ZipError::FileTooLarge);
            }
        }

        let data_offset = self.calc_data_offset(entry)?;
        self.file
            .seek(SeekFrom::Start(data_offset))
            .map_err(|_| ZipError::IoError)?;

        match entry.method {
            METHOD_STORED => {
                let mut remaining =
                    usize::try_from(entry.compressed_size).map_err(|_| ZipError::FileTooLarge)?;
                let mut hasher = crc32fast::Hasher::new();
                let mut written = 0usize;

                while remaining > 0 {
                    let take = core::cmp::min(remaining, input_buf.len());
                    self.file
                        .read_exact(&mut input_buf[..take])
                        .map_err(|_| ZipError::IoError)?;
                    writer
                        .write_all(&input_buf[..take])
                        .map_err(|_| ZipError::IoError)?;
                    hasher.update(&input_buf[..take]);
                    written += take;
                    remaining -= take;
                }

                if entry.crc32 != 0 && hasher.finalize() != entry.crc32 {
                    return Err(ZipError::CrcMismatch);
                }
                Ok(written)
            }
            METHOD_DEFLATED => {
                self.inflate_state.reset(DataFormat::Raw);
                let mut compressed_remaining =
                    usize::try_from(entry.compressed_size).map_err(|_| ZipError::FileTooLarge)?;
                let mut pending = &[][..];
                let mut written = 0usize;
                let mut hasher = crc32fast::Hasher::new();

                loop {
                    if pending.is_empty() && compressed_remaining > 0 {
                        let take = core::cmp::min(compressed_remaining, input_buf.len());
                        self.file
                            .read_exact(&mut input_buf[..take])
                            .map_err(|_| ZipError::IoError)?;
                        pending = &input_buf[..take];
                        compressed_remaining -= take;
                    }

                    let result = miniz_oxide::inflate::stream::inflate(
                        &mut self.inflate_state,
                        pending,
                        output_buf,
                        MZFlush::None,
                    );
                    let consumed = result.bytes_consumed;
                    let produced = result.bytes_written;
                    pending = &pending[consumed..];

                    if produced > 0 {
                        writer
                            .write_all(&output_buf[..produced])
                            .map_err(|_| ZipError::IoError)?;
                        hasher.update(&output_buf[..produced]);
                        written += produced;
                    }

                    match result.status {
                        Ok(MZStatus::StreamEnd) => {
                            if compressed_remaining != 0 || !pending.is_empty() {
                                return Err(ZipError::DecompressError);
                            }
                            break;
                        }
                        Ok(MZStatus::Ok) => {
                            if consumed == 0 && produced == 0 {
                                return Err(ZipError::DecompressError);
                            }
                        }
                        Ok(MZStatus::NeedDict) => return Err(ZipError::DecompressError),
                        Err(_) => return Err(ZipError::DecompressError),
                    }
                }

                if entry.crc32 != 0 && hasher.finalize() != entry.crc32 {
                    return Err(ZipError::CrcMismatch);
                }
                Ok(written)
            }
            _ => Err(ZipError::UnsupportedCompression),
        }
    }

    /// Read a file by its local header offset (avoids borrow issues)
    /// This is useful when you need to read a file after getting its metadata
    pub fn read_file_at_offset(
        &mut self,
        local_header_offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, ZipError> {
        // Find entry by offset
        let entry = self
            .entries
            .iter()
            .find(|e| e.local_header_offset == local_header_offset)
            .ok_or(ZipError::FileNotFound)?;

        // Create a temporary entry clone to avoid borrow issues
        let entry_clone = CdEntry {
            method: entry.method,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            local_header_offset: entry.local_header_offset,
            crc32: entry.crc32,
            filename: entry.filename.clone(),
        };

        self.read_file(&entry_clone, buf)
    }

    /// Calculate the offset to the actual file data (past local header)
    fn calc_data_offset(&mut self, entry: &CdEntry) -> Result<u64, ZipError> {
        let offset = entry.local_header_offset;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| ZipError::IoError)?;

        // Read local file header (30 bytes fixed + variable filename/extra)
        let mut header = [0u8; 30];
        self.file
            .read_exact(&mut header)
            .map_err(|_| ZipError::IoError)?;

        // Verify signature
        let sig = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        if sig != SIG_LOCAL_FILE_HEADER {
            return Err(ZipError::InvalidFormat);
        }

        // Get filename and extra field lengths
        let name_len = u16::from_le_bytes([header[26], header[27]]) as u64;
        let extra_len = u16::from_le_bytes([header[28], header[29]]) as u64;

        // Data starts after local header + filename + extra field
        let data_offset = offset + 30 + name_len + extra_len;

        Ok(data_offset)
    }

    /// Read u16 from buffer at offset (little-endian)
    fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes([buf[offset], buf[offset + 1]])
    }

    /// Read u32 from buffer at offset (little-endian)
    fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    /// Validate that the archive contains a valid EPUB mimetype file
    ///
    /// Checks that a file named "mimetype" exists and its content is exactly
    /// `application/epub+zip`, as required by the EPUB specification.
    pub fn validate_mimetype(&mut self) -> Result<(), ZipError> {
        let entry = self
            .get_entry("mimetype")
            .ok_or_else(|| {
                ZipError::InvalidMimetype("mimetype file not found in archive".to_string())
            })?
            .clone();

        if let Some(limits) = self.limits {
            if entry.uncompressed_size > limits.max_mimetype_size as u64 {
                return Err(ZipError::InvalidMimetype(
                    "mimetype file too large".to_string(),
                ));
            }
        }

        let size = usize::try_from(entry.uncompressed_size)
            .map_err(|_| ZipError::InvalidMimetype("mimetype file too large".to_string()))?;
        let mut buf = alloc::vec![0u8; size];
        let bytes_read = self.read_file(&entry, &mut buf)?;

        let content = core::str::from_utf8(&buf[..bytes_read]).map_err(|_| {
            ZipError::InvalidMimetype("mimetype file is not valid UTF-8".to_string())
        })?;

        if content != "application/epub+zip" {
            return Err(ZipError::InvalidMimetype(format!(
                "expected 'application/epub+zip', got '{}'",
                content
            )));
        }

        Ok(())
    }

    /// Check if this archive is a valid EPUB file
    ///
    /// Convenience wrapper around `validate_mimetype()` that returns a boolean.
    pub fn is_valid_epub(&mut self) -> bool {
        self.validate_mimetype().is_ok()
    }

    /// Get number of entries in central directory
    pub fn num_entries(&self) -> usize {
        self.num_entries.min(self.entries.len())
    }

    /// Iterate over all entries
    pub fn entries(&self) -> impl Iterator<Item = &CdEntry> {
        self.entries.iter()
    }

    /// Get entry by index
    pub fn get_entry_by_index(&self, index: usize) -> Option<&CdEntry> {
        self.entries.get(index)
    }

    /// Get the active limits used by this ZIP reader.
    pub fn limits(&self) -> Option<ZipLimits> {
        self.limits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Simple test to verify the module compiles
    #[test]
    fn test_zip_error_debug() {
        let err = ZipError::FileNotFound;
        assert_eq!(format!("{:?}", err), "FileNotFound");
    }

    #[test]
    fn test_zip_error_invalid_mimetype_debug() {
        let err = ZipError::InvalidMimetype("wrong content".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("InvalidMimetype"));
        assert!(debug.contains("wrong content"));
    }

    #[test]
    fn test_zip_error_invalid_mimetype_equality() {
        let err1 = ZipError::InvalidMimetype("missing".to_string());
        let err2 = ZipError::InvalidMimetype("missing".to_string());
        let err3 = ZipError::InvalidMimetype("different".to_string());
        assert_eq!(err1, err2);
        assert_ne!(err1, err3);
    }

    #[test]
    fn test_zip_error_variants_are_distinct() {
        let errors: Vec<ZipError> = vec![
            ZipError::FileNotFound,
            ZipError::InvalidFormat,
            ZipError::UnsupportedCompression,
            ZipError::DecompressError,
            ZipError::CrcMismatch,
            ZipError::IoError,
            ZipError::CentralDirFull,
            ZipError::BufferTooSmall,
            ZipError::FileTooLarge,
            ZipError::InvalidMimetype("test".to_string()),
            ZipError::UnsupportedZip64,
        ];

        // Each variant should be different from every other
        for (i, a) in errors.iter().enumerate() {
            for (j, b) in errors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "variants at index {} and {} should differ", i, j);
                }
            }
        }
    }

    #[test]
    fn test_zip_error_clone() {
        let err = ZipError::InvalidMimetype("test message".to_string());
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn test_cd_entry_new() {
        let entry = CdEntry::new();
        assert_eq!(entry.method, 0);
        assert_eq!(entry.compressed_size, 0);
        assert_eq!(entry.uncompressed_size, 0);
        assert_eq!(entry.local_header_offset, 0);
        assert_eq!(entry.crc32, 0);
        assert!(entry.filename.is_empty());
    }

    /// Helper to build a minimal valid ZIP archive with a single stored file.
    ///
    /// The archive contains one file with the given name and content,
    /// stored without compression (method 0).
    fn build_single_file_zip(filename: &str, content: &[u8]) -> Vec<u8> {
        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let content_len = content.len() as u32;
        let crc = crc32fast::hash(content);

        let mut zip = Vec::with_capacity(0);

        // -- Local file header --
        let local_offset = zip.len() as u32;
        zip.extend_from_slice(&SIG_LOCAL_FILE_HEADER.to_le_bytes()); // signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&METHOD_STORED.to_le_bytes()); // compression
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC32
        zip.extend_from_slice(&content_len.to_le_bytes()); // compressed size
        zip.extend_from_slice(&content_len.to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&name_len.to_le_bytes()); // filename length
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        zip.extend_from_slice(name_bytes); // filename
        zip.extend_from_slice(content); // file data

        // -- Central directory entry --
        let cd_offset = zip.len() as u32;
        zip.extend_from_slice(&SIG_CD_ENTRY.to_le_bytes()); // signature
        zip.extend_from_slice(&20u16.to_le_bytes()); // version made by
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&METHOD_STORED.to_le_bytes()); // compression
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC32
        zip.extend_from_slice(&content_len.to_le_bytes()); // compressed size
        zip.extend_from_slice(&content_len.to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&name_len.to_le_bytes()); // filename length
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment length
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        zip.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        zip.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        zip.extend_from_slice(&local_offset.to_le_bytes()); // local header offset
        zip.extend_from_slice(name_bytes); // filename

        let cd_size = (zip.len() as u32) - cd_offset;

        // -- End of central directory --
        zip.extend_from_slice(&SIG_EOCD.to_le_bytes()); // signature
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
        zip.extend_from_slice(&1u16.to_le_bytes()); // entries on this disk
        zip.extend_from_slice(&1u16.to_le_bytes()); // total entries
        zip.extend_from_slice(&cd_size.to_le_bytes()); // CD size
        zip.extend_from_slice(&cd_offset.to_le_bytes()); // CD offset
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment length

        zip
    }

    fn build_single_file_zip64(filename: &str, content: &[u8]) -> Vec<u8> {
        let name_bytes = filename.as_bytes();
        let name_len = name_bytes.len() as u16;
        let content_len = content.len() as u64;
        let crc = crc32fast::hash(content);

        let mut zip = Vec::with_capacity(0);

        // -- Local file header --
        let local_offset = zip.len() as u64;
        zip.extend_from_slice(&SIG_LOCAL_FILE_HEADER.to_le_bytes()); // signature
        zip.extend_from_slice(&45u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&METHOD_STORED.to_le_bytes()); // compression
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC32
        zip.extend_from_slice(&(content_len as u32).to_le_bytes()); // compressed size
        zip.extend_from_slice(&(content_len as u32).to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&name_len.to_le_bytes()); // filename length
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        zip.extend_from_slice(name_bytes); // filename
        zip.extend_from_slice(content); // file data

        // -- Central directory entry with ZIP64 extra field --
        let cd_offset = zip.len() as u64;
        let zip64_extra_len = 24u16; // uncompressed + compressed + local header offset
        zip.extend_from_slice(&SIG_CD_ENTRY.to_le_bytes()); // signature
        zip.extend_from_slice(&45u16.to_le_bytes()); // version made by
        zip.extend_from_slice(&45u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&METHOD_STORED.to_le_bytes()); // compression
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes()); // CRC32
        zip.extend_from_slice(&u32::MAX.to_le_bytes()); // compressed size sentinel
        zip.extend_from_slice(&u32::MAX.to_le_bytes()); // uncompressed size sentinel
        zip.extend_from_slice(&name_len.to_le_bytes()); // filename length
        zip.extend_from_slice(&(zip64_extra_len + 4).to_le_bytes()); // extra field length
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment length
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        zip.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        zip.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        zip.extend_from_slice(&u32::MAX.to_le_bytes()); // local header offset sentinel
        zip.extend_from_slice(name_bytes); // filename
        zip.extend_from_slice(&0x0001u16.to_le_bytes()); // ZIP64 extra header id
        zip.extend_from_slice(&zip64_extra_len.to_le_bytes()); // ZIP64 extra length
        zip.extend_from_slice(&content_len.to_le_bytes()); // uncompressed size
        zip.extend_from_slice(&content_len.to_le_bytes()); // compressed size
        zip.extend_from_slice(&local_offset.to_le_bytes()); // local header offset

        let cd_size = (zip.len() as u64) - cd_offset;

        // -- ZIP64 EOCD record --
        let zip64_eocd_offset = zip.len() as u64;
        zip.extend_from_slice(&SIG_ZIP64_EOCD.to_le_bytes()); // signature
        zip.extend_from_slice(&44u64.to_le_bytes()); // size of ZIP64 EOCD record
        zip.extend_from_slice(&45u16.to_le_bytes()); // version made by
        zip.extend_from_slice(&45u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u32.to_le_bytes()); // disk number
        zip.extend_from_slice(&0u32.to_le_bytes()); // disk where CD starts
        zip.extend_from_slice(&1u64.to_le_bytes()); // entries on this disk
        zip.extend_from_slice(&1u64.to_le_bytes()); // total entries
        zip.extend_from_slice(&cd_size.to_le_bytes()); // central directory size
        zip.extend_from_slice(&cd_offset.to_le_bytes()); // central directory offset

        // -- ZIP64 EOCD locator --
        zip.extend_from_slice(&SIG_ZIP64_EOCD_LOCATOR.to_le_bytes()); // signature
        zip.extend_from_slice(&0u32.to_le_bytes()); // disk with ZIP64 EOCD
        zip.extend_from_slice(&zip64_eocd_offset.to_le_bytes()); // ZIP64 EOCD offset
        zip.extend_from_slice(&1u32.to_le_bytes()); // total disks

        // -- Legacy EOCD with ZIP64 sentinels --
        zip.extend_from_slice(&SIG_EOCD.to_le_bytes()); // signature
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk number
        zip.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
        zip.extend_from_slice(&u16::MAX.to_le_bytes()); // entries on this disk sentinel
        zip.extend_from_slice(&u16::MAX.to_le_bytes()); // total entries sentinel
        zip.extend_from_slice(&u32::MAX.to_le_bytes()); // CD size sentinel
        zip.extend_from_slice(&u32::MAX.to_le_bytes()); // CD offset sentinel
        zip.extend_from_slice(&0u16.to_le_bytes()); // comment length

        zip
    }

    fn add_zip_comment(mut zip: Vec<u8>, comment_len: usize) -> Vec<u8> {
        let eocd_pos = zip.len() - EOCD_MIN_SIZE;
        let comment_len = comment_len as u16;
        zip[eocd_pos + 20..eocd_pos + 22].copy_from_slice(&comment_len.to_le_bytes());
        zip.extend_from_slice(&vec![b'A'; comment_len as usize]);
        zip
    }

    #[test]
    fn test_validate_mimetype_success() {
        let zip_data = build_single_file_zip("mimetype", b"application/epub+zip");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        assert!(zip.validate_mimetype().is_ok());
    }

    #[test]
    fn test_eocd_found_with_long_comment() {
        let zip_data = add_zip_comment(
            build_single_file_zip("mimetype", b"application/epub+zip"),
            2_000,
        );
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).expect("EOCD should be discoverable");
        assert!(zip.validate_mimetype().is_ok());
    }

    #[test]
    fn test_eocd_scan_limit_rejects_long_tail() {
        let zip_data = add_zip_comment(
            build_single_file_zip("mimetype", b"application/epub+zip"),
            2_000,
        );
        let cursor = std::io::Cursor::new(zip_data);
        let limits = ZipLimits::new(1024 * 1024, 1024).with_max_eocd_scan(128);
        let result = StreamingZip::new_with_limits(cursor, Some(limits));
        assert!(matches!(result, Err(ZipError::InvalidFormat)));
    }

    #[test]
    fn test_zip64_sentinel_without_locator_is_invalid() {
        let mut zip_data = build_single_file_zip("mimetype", b"application/epub+zip");
        let eocd_pos = zip_data.len() - EOCD_MIN_SIZE;
        zip_data[eocd_pos + 8..eocd_pos + 10].copy_from_slice(&u16::MAX.to_le_bytes());
        let cursor = std::io::Cursor::new(zip_data);
        let result = StreamingZip::new(cursor);
        assert!(matches!(result, Err(ZipError::InvalidFormat)));
    }

    #[test]
    fn test_zip64_single_file_archive_is_readable() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip64("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).expect("ZIP64 archive should parse");
        let entry = zip.get_entry("mimetype").expect("mimetype entry").clone();
        assert_eq!(entry.uncompressed_size, content.len() as u64);
        assert_eq!(entry.compressed_size, content.len() as u64);

        let mut buf = [0u8; 64];
        let n = zip
            .read_file(&entry, &mut buf)
            .expect("ZIP64 entry should read");
        assert_eq!(&buf[..n], content);
    }

    #[test]
    fn test_strict_rejects_too_many_cd_entries() {
        let mut zip_data = build_single_file_zip("mimetype", b"application/epub+zip");
        let eocd_pos = zip_data.len() - EOCD_MIN_SIZE;
        let count = (MAX_CD_ENTRIES as u16) + 1;
        zip_data[eocd_pos + 8..eocd_pos + 10].copy_from_slice(&count.to_le_bytes());
        zip_data[eocd_pos + 10..eocd_pos + 12].copy_from_slice(&count.to_le_bytes());
        let cursor = std::io::Cursor::new(zip_data);
        let limits = ZipLimits::new(1024 * 1024, 1024).with_strict(true);
        let result = StreamingZip::new_with_limits(cursor, Some(limits));
        assert!(matches!(result, Err(ZipError::CentralDirFull)));
    }

    #[test]
    fn test_validate_mimetype_wrong_content() {
        let zip_data = build_single_file_zip("mimetype", b"text/plain");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let result = zip.validate_mimetype();
        assert!(result.is_err());
        match result.unwrap_err() {
            ZipError::InvalidMimetype(msg) => {
                assert!(msg.contains("text/plain"));
            }
            other => panic!("Expected InvalidMimetype, got {:?}", other),
        }
    }

    #[test]
    fn test_validate_mimetype_missing_file() {
        let zip_data = build_single_file_zip("not_mimetype.txt", b"hello");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let result = zip.validate_mimetype();
        assert!(result.is_err());
        match result.unwrap_err() {
            ZipError::InvalidMimetype(msg) => {
                assert!(msg.contains("not found"));
            }
            other => panic!("Expected InvalidMimetype, got {:?}", other),
        }
    }

    #[test]
    fn test_is_valid_epub_true() {
        let zip_data = build_single_file_zip("mimetype", b"application/epub+zip");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        assert!(zip.is_valid_epub());
    }

    #[test]
    fn test_is_valid_epub_false_wrong_content() {
        let zip_data = build_single_file_zip("mimetype", b"application/zip");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        assert!(!zip.is_valid_epub());
    }

    #[test]
    fn test_is_valid_epub_false_missing() {
        let zip_data = build_single_file_zip("other.txt", b"some content");
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        assert!(!zip.is_valid_epub());
    }

    #[test]
    fn test_streaming_zip_read_file() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();

        assert_eq!(zip.num_entries(), 1);

        let entry = zip.get_entry("mimetype").unwrap().clone();
        assert_eq!(entry.filename, "mimetype");
        assert_eq!(entry.uncompressed_size, content.len() as u64);
        assert_eq!(entry.method, METHOD_STORED);

        let mut buf = [0u8; 64];
        let n = zip.read_file(&entry, &mut buf).unwrap();
        assert_eq!(&buf[..n], content);
    }

    #[test]
    fn test_read_file_to_writer_with_scratch_streams_stored_entry() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let entry = zip.get_entry("mimetype").unwrap().clone();

        let mut out = Vec::with_capacity(0);
        let mut input = [0u8; 16];
        let mut output = [0u8; 16];
        let n = zip
            .read_file_to_writer_with_scratch(&entry, &mut out, &mut input, &mut output)
            .expect("streaming with scratch should succeed");
        assert_eq!(n, content.len());
        assert_eq!(out, content);
    }

    #[test]
    fn test_read_file_to_writer_with_scratch_rejects_empty_buffers() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let entry = zip.get_entry("mimetype").unwrap().clone();

        let mut out = Vec::with_capacity(0);
        let mut input = [];
        let mut output = [0u8; 16];
        let err = zip
            .read_file_to_writer_with_scratch(&entry, &mut out, &mut input, &mut output)
            .expect_err("empty input buffer must fail");
        assert!(matches!(err, ZipError::BufferTooSmall));
    }

    #[test]
    fn test_read_file_with_scratch_streams_into_output_buffer() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let entry = zip.get_entry("mimetype").unwrap().clone();

        let mut out = [0u8; 64];
        let mut input = [0u8; 8];
        let n = zip
            .read_file_with_scratch(&entry, &mut out, &mut input)
            .expect("read_file_with_scratch should succeed");
        assert_eq!(&out[..n], content);
    }

    #[test]
    fn test_read_file_with_scratch_rejects_empty_input_buffer() {
        let content = b"application/epub+zip";
        let zip_data = build_single_file_zip("mimetype", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let entry = zip.get_entry("mimetype").unwrap().clone();

        let mut out = [0u8; 64];
        let mut input = [];
        let err = zip
            .read_file_with_scratch(&entry, &mut out, &mut input)
            .expect_err("empty input buffer must fail");
        assert!(matches!(err, ZipError::BufferTooSmall));
    }

    #[test]
    fn test_zip_limits_enforced_when_configured() {
        let content = b"1234567890";
        let zip_data = build_single_file_zip("data.txt", content);
        let cursor = std::io::Cursor::new(zip_data);
        let limits = ZipLimits::new(8, 8);
        let mut zip = StreamingZip::new_with_limits(cursor, Some(limits)).unwrap();
        let entry = zip.get_entry("data.txt").unwrap().clone();
        let mut buf = [0u8; 32];
        let result = zip.read_file(&entry, &mut buf);
        assert!(matches!(result, Err(ZipError::FileTooLarge)));
    }

    #[test]
    fn test_zip_limits_not_enforced_by_default() {
        let content = b"1234567890";
        let zip_data = build_single_file_zip("data.txt", content);
        let cursor = std::io::Cursor::new(zip_data);
        let mut zip = StreamingZip::new(cursor).unwrap();
        let entry = zip.get_entry("data.txt").unwrap().clone();
        let mut buf = [0u8; 32];
        let n = zip.read_file(&entry, &mut buf).unwrap();
        assert_eq!(&buf[..n], content);
    }
}
