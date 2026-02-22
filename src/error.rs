//! Unified error types for epub-stream
//!
//! Provides a top-level `EpubError` that wraps module-specific errors,
//! plus `From` impls so `?` works across module boundaries.

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use core::fmt;

/// Stable processing phases for typed EPUB failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorPhase {
    /// EPUB open/bootstrap work (container + OPF discovery).
    Open,
    /// Generic parsing/tokenization work.
    Parse,
    /// Style/CSS preparation work.
    Style,
    /// Layout/pagination work.
    Layout,
    /// Backend rendering work.
    Render,
}

impl fmt::Display for ErrorPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Parse => write!(f, "parse"),
            Self::Style => write!(f, "style"),
            Self::Layout => write!(f, "layout"),
            Self::Render => write!(f, "render"),
        }
    }
}

/// Typed actual-vs-limit payload for hard-cap failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLimitContext {
    /// Stable limit field name (e.g. `max_css_bytes`).
    pub kind: Box<str>,
    /// Observed value.
    pub actual: usize,
    /// Configured cap.
    pub limit: usize,
}

impl ErrorLimitContext {
    /// Build a new limit context.
    pub fn new(kind: impl Into<String>, actual: usize, limit: usize) -> Self {
        Self {
            kind: kind.into().into_boxed_str(),
            actual,
            limit,
        }
    }
}

/// Rich optional context for typed phase errors.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseErrorContext {
    /// Optional archive path context.
    pub path: Option<Box<str>>,
    /// Optional resource href context.
    pub href: Option<Box<str>>,
    /// Optional chapter index.
    pub chapter_index: Option<usize>,
    /// Optional source context (e.g. inline style location).
    pub source: Option<Box<str>>,
    /// Optional selector context.
    pub selector: Option<Box<str>>,
    /// Optional selector index.
    pub selector_index: Option<usize>,
    /// Optional declaration context.
    pub declaration: Option<Box<str>>,
    /// Optional declaration index.
    pub declaration_index: Option<usize>,
    /// Optional tokenizer/read offset in bytes.
    pub token_offset: Option<usize>,
    /// Optional actual-vs-limit payload.
    pub limit: Option<Box<ErrorLimitContext>>,
}

/// Typed error with explicit processing phase and context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseError {
    /// Stable processing phase.
    pub phase: ErrorPhase,
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Human-readable message.
    pub message: Box<str>,
    /// Optional rich context.
    pub context: Option<Box<PhaseErrorContext>>,
}

impl PhaseError {
    /// Create a typed phase error.
    pub fn new(phase: ErrorPhase, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            code,
            message: message.into().into_boxed_str(),
            context: None,
        }
    }
}

/// Top-level error type for epub-stream operations
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EpubError {
    /// Typed phase-aware error with structured context.
    Phase(PhaseError),
    /// ZIP archive error
    Zip(ZipError),
    /// XML/XHTML parsing error
    Parse(String),
    /// Invalid EPUB structure (missing required files, broken references, etc.)
    InvalidEpub(String),
    /// Navigation parsing error
    Navigation(String),
    /// CSS parsing error
    Css(String),
    /// I/O error (description only, since `std::io::Error` is not `Clone`)
    Io(String),
    /// Chapter index requested is out of bounds
    ChapterOutOfBounds {
        /// Requested chapter index.
        index: usize,
        /// Total number of chapters available.
        chapter_count: usize,
    },
    /// Spine references a manifest item that does not exist
    ManifestItemMissing {
        /// Missing manifest `id` referenced by spine `idref`.
        idref: String,
    },
    /// Chapter content could not be decoded as UTF-8
    ChapterNotUtf8 {
        /// Chapter href/path in the EPUB archive.
        href: String,
    },
    /// Hard limit exceeded (typed error instead of OOM)
    LimitExceeded {
        /// Kind of limit that was exceeded.
        kind: LimitKind,
        /// Actual value observed.
        actual: usize,
        /// Configured limit.
        limit: usize,
        /// Optional path context.
        path: Option<String>,
    },
    /// Provided buffer is too small for the operation
    BufferTooSmall {
        /// Required size.
        required: usize,
        /// Provided size.
        provided: usize,
        /// Context about which buffer.
        context: String,
    },
}

/// Kinds of limits that can be exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitKind {
    /// File size limit.
    FileSize,
    /// Memory budget limit.
    MemoryBudget,
    /// Token/event count limit.
    EventCount,
    /// Nesting depth limit.
    NestingDepth,
    /// CSS stylesheet size limit.
    CssSize,
    /// Font count/size limit.
    FontLimit,
}

impl fmt::Display for EpubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EpubError::Phase(err) => write!(
                f,
                "{} error [{}]: {}",
                err.phase.to_string().to_ascii_uppercase(),
                err.code,
                err.message
            ),
            EpubError::Zip(kind) => write!(f, "ZIP error: {}", kind),
            EpubError::Parse(msg) => write!(f, "Parse error: {}", msg),
            EpubError::InvalidEpub(msg) => write!(f, "Invalid EPUB: {}", msg),
            EpubError::Navigation(msg) => write!(f, "Navigation error: {}", msg),
            EpubError::Css(msg) => write!(f, "CSS error: {}", msg),
            EpubError::Io(msg) => write!(f, "I/O error: {}", msg),
            EpubError::ChapterOutOfBounds {
                index,
                chapter_count,
            } => write!(
                f,
                "Chapter index {} out of bounds (chapter count: {})",
                index, chapter_count
            ),
            EpubError::ManifestItemMissing { idref } => {
                write!(f, "Spine item '{}' does not exist in manifest", idref)
            }
            EpubError::ChapterNotUtf8 { href } => {
                write!(f, "Chapter content is not valid UTF-8: {}", href)
            }
            EpubError::LimitExceeded {
                kind,
                actual,
                limit,
                path,
            } => {
                write!(
                    f,
                    "{} limit exceeded: {} > {}{}",
                    kind,
                    actual,
                    limit,
                    path.as_ref()
                        .map(|p| format!(" at {}", p))
                        .unwrap_or_default()
                )
            }
            EpubError::BufferTooSmall {
                required,
                provided,
                context,
            } => {
                write!(
                    f,
                    "Buffer too small for {}: required {} bytes, provided {}",
                    context, required, provided
                )
            }
        }
    }
}

impl fmt::Display for LimitKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimitKind::FileSize => write!(f, "File size"),
            LimitKind::MemoryBudget => write!(f, "Memory budget"),
            LimitKind::EventCount => write!(f, "Event count"),
            LimitKind::NestingDepth => write!(f, "Nesting depth"),
            LimitKind::CssSize => write!(f, "CSS size"),
            LimitKind::FontLimit => write!(f, "Font limit"),
        }
    }
}

/// ZIP-specific error variants
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZipErrorKind {
    /// File not found in archive
    FileNotFound,
    /// Invalid ZIP format
    InvalidFormat,
    /// Unsupported compression method
    UnsupportedCompression,
    /// Decompression failed
    DecompressError,
    /// CRC32 mismatch
    CrcMismatch,
    /// I/O error during ZIP operations
    IoError,
    /// Central directory full (exceeded max entries)
    CentralDirFull,
    /// Buffer too small for decompressed content
    BufferTooSmall,
    /// File exceeds maximum allowed size
    FileTooLarge,
    /// Invalid or missing mimetype file
    InvalidMimetype(String),
    /// ZIP64 structures are present but unsupported
    UnsupportedZip64,
}

/// Public ZIP error type alias used across the crate API.
pub type ZipError = ZipErrorKind;

impl fmt::Display for ZipErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipErrorKind::FileNotFound => write!(f, "file not found in archive"),
            ZipErrorKind::InvalidFormat => write!(f, "invalid ZIP format"),
            ZipErrorKind::UnsupportedCompression => write!(f, "unsupported compression method"),
            ZipErrorKind::DecompressError => write!(f, "decompression failed"),
            ZipErrorKind::CrcMismatch => write!(f, "CRC32 checksum mismatch"),
            ZipErrorKind::IoError => write!(f, "I/O error"),
            ZipErrorKind::CentralDirFull => write!(f, "central directory full"),
            ZipErrorKind::BufferTooSmall => write!(f, "buffer too small"),
            ZipErrorKind::FileTooLarge => write!(f, "file too large"),
            ZipErrorKind::InvalidMimetype(msg) => write!(f, "invalid mimetype: {}", msg),
            ZipErrorKind::UnsupportedZip64 => write!(f, "ZIP64 is not supported"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EpubError {}

#[cfg(feature = "std")]
impl std::error::Error for ZipErrorKind {}

impl From<crate::tokenizer::TokenizeError> for EpubError {
    fn from(err: crate::tokenizer::TokenizeError) -> Self {
        EpubError::Parse(err.to_string())
    }
}

impl From<PhaseError> for EpubError {
    fn from(err: PhaseError) -> Self {
        Self::Phase(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_epub_error_display() {
        let err = EpubError::Parse("bad xml".into());
        assert_eq!(format!("{}", err), "Parse error: bad xml");
    }

    #[test]
    fn test_phase_error_display() {
        let err = EpubError::Phase(PhaseError::new(
            ErrorPhase::Style,
            "STYLE_LIMIT",
            "limit exceeded",
        ));
        assert_eq!(
            format!("{}", err),
            "STYLE error [STYLE_LIMIT]: limit exceeded"
        );
    }

    #[test]
    fn test_zip_error_kind_debug() {
        let kind = ZipErrorKind::FileNotFound;
        assert_eq!(format!("{:?}", kind), "FileNotFound");
    }

    #[test]
    fn test_invalid_mimetype_error() {
        let err = EpubError::Zip(ZipErrorKind::InvalidMimetype("wrong content type".into()));
        let display = format!("{}", err);
        assert!(display.contains("ZIP error"));
    }
}
