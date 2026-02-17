//! mu-epub -- Memory-efficient EPUB parser for embedded systems
//!
//! Streaming EPUB parser designed for constrained devices. Provides SAX-style
//! XML parsing, XHTML tokenization, and optional text layout/pagination.
//!
//! # Features
//!
//! - `std` (default) -- enables streaming ZIP reader and file I/O
//! - `layout` -- text layout engine for pagination
//!
//! # Allocation Behavior
//!
//! This crate uses an alloc-bounded design with explicit limits on all
//! expensive entrypoints. The primary APIs require caller-provided buffers
//! or scratch space (`*_with_scratch`, `*_into`). Convenience APIs that
//! allocate (`read_resource() -> Vec<u8>`) are available only with the `std`
//! feature and are clearly marked as non-embedded-fast-path.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::disallowed_methods,
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::todo,
        clippy::unimplemented
    )
)]
#![deny(
    clippy::large_enum_variant,
    clippy::large_stack_arrays,
    clippy::redundant_clone
)]
#![warn(
    clippy::box_collection,
    clippy::needless_collect,
    clippy::map_clone,
    clippy::implicit_clone,
    clippy::inefficient_to_string
)]
extern crate alloc;

pub mod css;
pub mod error;
pub mod metadata;
pub mod navigation;
pub mod spine;
pub mod streaming;
pub mod tokenizer;

#[cfg(feature = "layout")]
pub mod layout;

#[cfg(feature = "std")]
pub mod book;

#[cfg(feature = "std")]
pub mod validate;

#[cfg(feature = "std")]
pub mod render_prep;

#[cfg(feature = "async")]
pub mod async_api;

#[cfg(feature = "std")]
pub mod zip;

// Re-export key types for convenience
#[cfg(feature = "async")]
pub use async_api::{open_epub_file_async, open_epub_file_async_with_options};
#[cfg(feature = "std")]
pub use book::{
    parse_epub_file, parse_epub_file_with_options, parse_epub_reader,
    parse_epub_reader_with_options, ChapterRef, ChapterStreamResult, CoverImageOptions,
    CoverImageRef, CoverImageSource, EpubBook, EpubBookBuilder, EpubBookOptions, EpubSummary,
    ImageReadOptions, Locator, PaginationSession, ReadingPosition, ReadingSession,
    ResolvedLocation, ValidationMode,
};
pub use css::{CssStyle, Stylesheet};
pub use error::{
    EpubError, ErrorLimitContext, ErrorPhase, LimitKind, PhaseError, PhaseErrorContext, ZipError,
    ZipErrorKind,
};
pub use metadata::EpubMetadata;
pub use navigation::Navigation;
#[cfg(feature = "std")]
pub use render_prep::{
    BlockRole, ChapterStylesheets, ComputedTextStyle, EmbeddedFontFace, EmbeddedFontStyle,
    FontFallbackPolicy, FontLimits, FontPolicy, FontResolutionTrace, FontResolver, LayoutHints,
    MemoryBudget, PreparedChapter, RenderPrep, RenderPrepError, RenderPrepOptions, RenderPrepTrace,
    ResolvedFontFace, StyleConfig, StyleLimits, StyledChapter, StyledEvent, StyledEventOrRun,
    StyledImage, StyledRun, Styler, StylesheetSource,
};
pub use spine::Spine;
pub use streaming::{
    ChunkAllocator, ChunkLimits, PaginationContext, ScratchBuffers, StreamingChapterProcessor,
    StreamingStats,
};
pub use tokenizer::{
    tokenize_html_into, tokenize_html_limited, tokenize_html_with_scratch, Token, TokenizeError,
    TokenizeLimits, TokenizeScratch,
};
#[cfg(feature = "std")]
pub use validate::{
    validate_epub_file, validate_epub_file_with_options, validate_epub_reader,
    validate_epub_reader_with_options, ValidationDiagnostic, ValidationOptions, ValidationReport,
    ValidationSeverity,
};
#[cfg(feature = "std")]
pub use zip::ZipLimits;
