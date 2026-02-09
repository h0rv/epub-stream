//! Streaming ZIP reader for EPUB files
//!
//! Memory-efficient ZIP reader that streams files without loading entire archive.
//! Uses fixed-size central directory cache (max 256 entries, ~4KB).
//! Supports DEFLATE decompression using miniz_oxide.

extern crate alloc;

use alloc::string::{String, ToString};
use heapless::Vec as HeaplessVec;
use log;
use miniz_oxide::{DataFormat, MZFlush, MZStatus};
use std::io::{Read, Seek, SeekFrom, Write};

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
    cd_size: u32,
    num_entries: u16,
    uses_zip64: bool,
}

/// Central directory entry metadata
#[derive(Debug, Clone)]
pub struct CdEntry {
    /// Compression method (0=stored, 8=deflated)
    pub method: u16,
    /// Compressed size in bytes
    pub compressed_size: u32,
    /// Uncompressed size in bytes
    pub uncompressed_size: u32,
    /// Offset to local file header
    pub local_header_offset: u32,
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
            filename: String::new(),
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
        if eocd.uses_zip64 {
            return Err(ZipError::UnsupportedZip64);
        }
        let strict = limits.is_some_and(|l| l.strict);
        if strict && eocd.num_entries as usize > MAX_CD_ENTRIES {
            return Err(ZipError::CentralDirFull);
        }

        let mut entries: HeaplessVec<CdEntry, MAX_CD_ENTRIES> = HeaplessVec::new();

        // Parse central directory entries
        file.seek(SeekFrom::Start(eocd.cd_offset))
            .map_err(|_| ZipError::IoError)?;
        let cd_end = eocd.cd_offset + eocd.cd_size as u64;

        for _ in 0..eocd.num_entries.min(MAX_CD_ENTRIES as u16) {
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

        if eocd.num_entries as usize > MAX_CD_ENTRIES {
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
            num_entries: eocd.num_entries as usize,
            limits,
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
                let cd_size = Self::read_u32_le(&buffer, i + 12);
                let cd_offset = Self::read_u32_le(&buffer, i + 16) as u64;
                let comment_len = Self::read_u16_le(&buffer, i + 20) as u64;
                let eocd_pos = scan_base + i as u64;
                let eocd_end = eocd_pos + EOCD_MIN_SIZE as u64 + comment_len;
                if eocd_end != file_size {
                    continue;
                }

                let cd_end = cd_offset
                    .checked_add(cd_size as u64)
                    .ok_or(ZipError::InvalidFormat)?;
                if cd_end > eocd_pos || cd_end > file_size {
                    return Err(ZipError::InvalidFormat);
                }

                let uses_zip64_sentinel =
                    num_entries == u16::MAX || cd_size == u32::MAX || cd_offset == u32::MAX as u64;
                let uses_zip64_locator = if eocd_pos >= 20 {
                    file.seek(SeekFrom::Start(eocd_pos - 20))
                        .map_err(|_| ZipError::IoError)?;
                    let mut locator_sig = [0u8; 4];
                    file.read_exact(&mut locator_sig)
                        .map_err(|_| ZipError::IoError)?;
                    u32::from_le_bytes(locator_sig) == SIG_ZIP64_EOCD_LOCATOR
                } else {
                    false
                };

                return Ok(EocdInfo {
                    cd_offset,
                    cd_size,
                    num_entries,
                    uses_zip64: uses_zip64_sentinel || uses_zip64_locator,
                });
            }
        }

        Err(ZipError::InvalidFormat)
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
        entry.compressed_size = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]); // CD offset 20
        entry.uncompressed_size = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]); // CD offset 24
        let name_len = u16::from_le_bytes([buf[24], buf[25]]) as usize; // CD offset 28
        let extra_len = u16::from_le_bytes([buf[26], buf[27]]) as usize; // CD offset 30
        let comment_len = u16::from_le_bytes([buf[28], buf[29]]) as usize; // CD offset 32
        entry.local_header_offset = u32::from_le_bytes([buf[38], buf[39], buf[40], buf[41]]); // CD offset 42

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

        // Skip extra field and comment
        let skip_bytes = extra_len + comment_len;
        if skip_bytes > 0 {
            file.seek(SeekFrom::Current(skip_bytes as i64))
                .map_err(|_| ZipError::IoError)?;
        }

        Ok(Some(entry))
    }

    /// Get entry by filename (case-insensitive)
    pub fn get_entry(&self, name: &str) -> Option<&CdEntry> {
        let name_lower = name.to_lowercase();
        self.entries
            .iter()
            .find(|e| e.filename.to_lowercase() == name_lower)
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
        let mut input_buf = alloc::vec![0u8; 8 * 1024];
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
            if entry.uncompressed_size as usize > limits.max_file_read_size {
                return Err(ZipError::FileTooLarge);
            }
            if entry.compressed_size as usize > limits.max_file_read_size {
                return Err(ZipError::FileTooLarge);
            }
        }
        if entry.uncompressed_size as usize > buf.len() {
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
                let size = entry.compressed_size as usize;
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
                let mut state = alloc::boxed::Box::new(
                    miniz_oxide::inflate::stream::InflateState::new(DataFormat::Raw),
                );
                let mut compressed_remaining = entry.compressed_size as usize;
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

                    let flush = if compressed_remaining == 0 {
                        MZFlush::Finish
                    } else {
                        MZFlush::None
                    };
                    let result = miniz_oxide::inflate::stream::inflate(
                        &mut state,
                        pending,
                        &mut buf[written..],
                        flush,
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
        let mut input_buf = alloc::vec![0u8; 8 * 1024];
        let mut output_buf = alloc::vec![0u8; 8 * 1024];
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
            if entry.uncompressed_size as usize > limits.max_file_read_size {
                return Err(ZipError::FileTooLarge);
            }
            if entry.compressed_size as usize > limits.max_file_read_size {
                return Err(ZipError::FileTooLarge);
            }
        }

        let data_offset = self.calc_data_offset(entry)?;
        self.file
            .seek(SeekFrom::Start(data_offset))
            .map_err(|_| ZipError::IoError)?;

        match entry.method {
            METHOD_STORED => {
                let mut remaining = entry.compressed_size as usize;
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
                let mut state = alloc::boxed::Box::new(
                    miniz_oxide::inflate::stream::InflateState::new(DataFormat::Raw),
                );
                let mut compressed_remaining = entry.compressed_size as usize;
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

                    let flush = if compressed_remaining == 0 {
                        MZFlush::Finish
                    } else {
                        MZFlush::None
                    };
                    let result = miniz_oxide::inflate::stream::inflate(
                        &mut state, pending, output_buf, flush,
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
        local_header_offset: u32,
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
        let offset = entry.local_header_offset as u64;
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
            if entry.uncompressed_size as usize > limits.max_mimetype_size {
                return Err(ZipError::InvalidMimetype(
                    "mimetype file too large".to_string(),
                ));
            }
        }

        let size = entry.uncompressed_size as usize;
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

        let mut zip = Vec::new();

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
    fn test_zip64_sentinel_rejected() {
        let mut zip_data = build_single_file_zip("mimetype", b"application/epub+zip");
        let eocd_pos = zip_data.len() - EOCD_MIN_SIZE;
        zip_data[eocd_pos + 8..eocd_pos + 10].copy_from_slice(&u16::MAX.to_le_bytes());
        let cursor = std::io::Cursor::new(zip_data);
        let result = StreamingZip::new(cursor);
        assert!(matches!(result, Err(ZipError::UnsupportedZip64)));
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
        assert_eq!(entry.uncompressed_size, content.len() as u32);
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

        let mut out = Vec::new();
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

        let mut out = Vec::new();
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
