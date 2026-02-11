//! High-level EPUB API for common workflows.
//!
//! This module provides a convenience wrapper around the lower-level parsers.
//! It is intended for the common "open EPUB -> inspect metadata -> read chapters"
//! flow.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::str;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::Path;

use crate::error::{
    EpubError, ErrorLimitContext, ErrorPhase, LimitKind, PhaseError, PhaseErrorContext, ZipError,
};
use crate::metadata::{extract_metadata, EpubMetadata};
use crate::navigation::{parse_nav_xhtml, parse_ncx, NavPoint, Navigation};
use crate::render_prep::{
    parse_font_faces_from_css, parse_stylesheet_links, ChapterStylesheets, EmbeddedFontFace,
    FontLimits, RenderPrep, RenderPrepOptions, StyleLimits, StyledChapter, StyledEventOrRun,
    StylesheetSource,
};
use crate::spine::Spine;

use crate::tokenizer::{tokenize_html, Token};
use crate::zip::{CdEntry, StreamingZip, ZipLimits};

/// Validation strictness for high-level open/parse flows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ValidationMode {
    /// Best-effort behavior for partial/quirky EPUBs.
    #[default]
    Lenient,
    /// Fail early for structural inconsistencies.
    Strict,
}

/// High-level configuration for opening EPUB books.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpubBookOptions {
    /// Optional ZIP safety limits used while reading archive entries.
    ///
    /// When `None`, no explicit file-size caps are enforced by this crate.
    pub zip_limits: Option<ZipLimits>,
    /// Validation strictness for high-level parse/open behavior.
    pub validation_mode: ValidationMode,
    /// Optional cap for navigation payload bytes.
    pub max_nav_bytes: Option<usize>,
}

impl Default for EpubBookOptions {
    fn default() -> Self {
        Self {
            zip_limits: None,
            validation_mode: ValidationMode::Lenient,
            max_nav_bytes: None,
        }
    }
}

/// Compatibility open configuration for embedded-facing APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct OpenConfig {
    /// Baseline high-level open options.
    pub options: EpubBookOptions,
    /// When enabled, navigation parsing is deferred until `ensure_navigation`.
    pub lazy_navigation: bool,
}

impl From<EpubBookOptions> for OpenConfig {
    fn from(options: EpubBookOptions) -> Self {
        Self {
            options,
            lazy_navigation: false,
        }
    }
}

/// Streaming chapter-event options for bounded extraction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChapterEventsOptions {
    /// Render-prep options used to produce event/run stream.
    pub render: RenderPrepOptions,
    /// Hard cap on emitted items.
    pub max_items: usize,
}

impl Default for ChapterEventsOptions {
    fn default() -> Self {
        Self {
            render: RenderPrepOptions::default(),
            max_items: 131_072,
        }
    }
}

/// Options for streaming chapter event processing without full materialization.
///
/// This provides true streaming from ZIP with configurable chunk sizes and limits.
#[derive(Clone, Debug)]
pub struct StreamingChapterOptions {
    /// Render-prep options for styling.
    pub render_prep: RenderPrepOptions,
    /// Hard cap on emitted items.
    pub max_items: usize,
    /// Maximum chapter entry size in bytes.
    pub max_entry_bytes: usize,
    /// Chunk size limits for incremental processing.
    pub chunk_limits: Option<crate::streaming::ChunkLimits>,
    /// Whether to extract stylesheets (requires additional reads).
    pub load_stylesheets: bool,
}

impl Default for StreamingChapterOptions {
    fn default() -> Self {
        Self {
            render_prep: RenderPrepOptions::default(),
            max_items: 131_072,
            max_entry_bytes: 4 * 1024 * 1024, // 4MB default
            chunk_limits: None,               // Use defaults
            load_stylesheets: false,          // Skip stylesheets for speed
        }
    }
}

impl StreamingChapterOptions {
    /// Create embedded-friendly options with small chunks.
    pub fn embedded() -> Self {
        Self {
            render_prep: RenderPrepOptions::default(),
            max_items: 10_000,
            max_entry_bytes: 512 * 1024, // 512KB max
            chunk_limits: Some(crate::streaming::ChunkLimits::embedded()),
            load_stylesheets: false,
        }
    }

    /// Set explicit chunk limits.
    pub fn with_chunk_limits(mut self, limits: crate::streaming::ChunkLimits) -> Self {
        self.chunk_limits = Some(limits);
        self
    }

    /// Enable/disable stylesheet loading.
    pub fn with_stylesheets(mut self, load: bool) -> Self {
        self.load_stylesheets = load;
        self
    }
}

/// Result from streaming chapter event processing.
#[derive(Clone, Debug)]
pub struct ChapterStreamResult {
    /// Number of items emitted.
    pub items_emitted: usize,
    /// Total bytes read from chapter.
    pub bytes_read: usize,
    /// Whether streaming is complete.
    pub complete: bool,
}

/// Builder for ergonomic high-level EPUB opening/parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct EpubBookBuilder {
    options: EpubBookOptions,
}

impl EpubBookBuilder {
    /// Create a new builder with no explicit limits.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set explicit ZIP limits.
    pub fn with_zip_limits(mut self, limits: ZipLimits) -> Self {
        self.options.zip_limits = Some(limits);
        self
    }

    /// Enable strict validation mode.
    pub fn strict(mut self) -> Self {
        self.options.validation_mode = ValidationMode::Strict;
        self
    }

    /// Set explicit validation mode.
    pub fn validation_mode(mut self, mode: ValidationMode) -> Self {
        self.options.validation_mode = mode;
        self
    }

    /// Set an explicit navigation payload byte cap.
    pub fn with_max_nav_bytes(mut self, max_nav_bytes: usize) -> Self {
        self.options.max_nav_bytes = Some(max_nav_bytes);
        self
    }

    /// Open an EPUB from a file path.
    pub fn open<P: AsRef<Path>>(self, path: P) -> Result<EpubBook<File>, EpubError> {
        EpubBook::open_with_options(path, self.options)
    }

    /// Open an EPUB from an arbitrary reader.
    pub fn from_reader<R: Read + Seek>(self, reader: R) -> Result<EpubBook<R>, EpubError> {
        EpubBook::from_reader_with_options(reader, self.options)
    }

    /// Parse summary metadata from a file path.
    pub fn parse_file<P: AsRef<Path>>(self, path: P) -> Result<EpubSummary, EpubError> {
        parse_epub_file_with_options(path, self.options)
    }

    /// Parse summary metadata from an arbitrary reader.
    pub fn parse_reader<R: Read + Seek>(self, reader: R) -> Result<EpubSummary, EpubError> {
        parse_epub_reader_with_options(reader, self.options)
    }
}

/// Parsed top-level EPUB data for lightweight usage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubSummary {
    metadata: EpubMetadata,
    spine: Spine,
    navigation: Option<Navigation>,
}

impl EpubSummary {
    /// EPUB package metadata.
    pub fn metadata(&self) -> &EpubMetadata {
        &self.metadata
    }

    /// Reading order from `<spine>`.
    pub fn spine(&self) -> &Spine {
        &self.spine
    }

    /// Parsed navigation document, when one is available.
    pub fn navigation(&self) -> Option<&Navigation> {
        self.navigation.as_ref()
    }
}

/// Parse an EPUB from any `Read + Seek` source.
pub fn parse_epub_reader<R: Read + Seek>(reader: R) -> Result<EpubSummary, EpubError> {
    parse_epub_reader_with_options(reader, EpubBookOptions::default())
}

/// Parse an EPUB from any `Read + Seek` source with explicit options.
pub fn parse_epub_reader_with_options<R: Read + Seek>(
    reader: R,
    options: EpubBookOptions,
) -> Result<EpubSummary, EpubError> {
    let mut zip =
        StreamingZip::new_with_limits(reader, options.zip_limits).map_err(EpubError::Zip)?;
    load_summary_from_zip(&mut zip, options)
}

/// Parse an EPUB from a file path.
pub fn parse_epub_file<P: AsRef<Path>>(path: P) -> Result<EpubSummary, EpubError> {
    parse_epub_file_with_options(path, EpubBookOptions::default())
}

/// Parse an EPUB from a file path with explicit options.
pub fn parse_epub_file_with_options<P: AsRef<Path>>(
    path: P,
    options: EpubBookOptions,
) -> Result<EpubSummary, EpubError> {
    let file = File::open(path).map_err(|e| EpubError::Io(e.to_string()))?;
    parse_epub_reader_with_options(file, options)
}

/// High-level EPUB handle backed by an open ZIP reader.
pub struct EpubBook<R: Read + Seek> {
    zip: StreamingZip<R>,
    opf_path: String,
    metadata: EpubMetadata,
    spine: Spine,
    validation_mode: ValidationMode,
    max_nav_bytes: Option<usize>,
    navigation_loaded: bool,
    navigation: Option<Navigation>,
    embedded_fonts_cache: Option<Vec<EmbeddedFontFace>>,
}

/// Lightweight chapter descriptor in spine order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterRef {
    /// Spine position index.
    pub index: usize,
    /// Spine `idref`.
    pub idref: String,
    /// Manifest href relative to OPF.
    pub href: String,
    /// Manifest media type.
    pub media_type: String,
}

/// Stable reading position with anchor + fallback offset information.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadingPosition {
    /// 0-based chapter index in spine order.
    pub chapter_index: usize,
    /// Optional chapter href hint for robust restore across index shifts.
    pub chapter_href: Option<String>,
    /// Optional anchor payload (fragment id or CFI-like token).
    pub anchor: Option<String>,
    /// Fallback character offset in the chapter when anchor cannot be resolved.
    pub fallback_offset: usize,
}

/// Semantic navigation primitive for seeking/resolve operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Locator {
    /// Resolve by chapter index.
    Chapter(usize),
    /// Resolve by chapter href (optionally with `#fragment`).
    Href(String),
    /// Resolve a fragment in the current chapter context.
    Fragment(String),
    /// Resolve by TOC id (mapped from nav href fragment or label).
    TocId(String),
    /// Resolve from a persisted reading position.
    Position(ReadingPosition),
}

/// Fully resolved location information returned from locator APIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedLocation {
    /// Resolved chapter descriptor.
    pub chapter: ChapterRef,
    /// Optional resolved fragment (without leading '#').
    pub fragment: Option<String>,
    /// Canonical position payload for persistence.
    pub position: ReadingPosition,
}

/// Lightweight mutable reading session detached from ZIP/file state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadingSession {
    chapters: Vec<ChapterRef>,
    navigation: Option<Navigation>,
    current: ReadingPosition,
}

impl ReadingSession {
    /// Create a reading session from chapter descriptors and optional navigation.
    pub fn new(chapters: Vec<ChapterRef>, navigation: Option<Navigation>) -> Self {
        let first_href = chapters.first().map(|c| c.href.clone());
        Self {
            chapters,
            navigation,
            current: ReadingPosition {
                chapter_index: 0,
                chapter_href: first_href,
                anchor: None,
                fallback_offset: 0,
            },
        }
    }

    /// Return current stable reading position.
    pub fn current_position(&self) -> ReadingPosition {
        self.current.clone()
    }

    /// Seek to an explicit reading position.
    pub fn seek_position(&mut self, pos: &ReadingPosition) -> Result<(), EpubError> {
        if pos.chapter_index >= self.chapters.len() {
            return Err(EpubError::ChapterOutOfBounds {
                index: pos.chapter_index,
                chapter_count: self.chapters.len(),
            });
        }
        self.current = pos.clone();
        if self.current.chapter_href.is_none() {
            self.current.chapter_href = Some(self.chapters[pos.chapter_index].href.clone());
        }
        Ok(())
    }

    /// Chapter-local progress ratio in `[0.0, 1.0]`.
    pub fn chapter_progress(&self) -> f32 {
        if self.chapters.is_empty() {
            return 0.0;
        }
        if self.current.fallback_offset == 0 {
            0.0
        } else {
            1.0
        }
    }

    /// Whole-book progress ratio in `[0.0, 1.0]`.
    pub fn book_progress(&self) -> f32 {
        if self.chapters.is_empty() {
            return 0.0;
        }
        let chapter_ratio = self.chapter_progress();
        ((self.current.chapter_index as f32) + chapter_ratio) / (self.chapters.len() as f32)
    }

    /// Resolve a semantic locator to a concrete chapter/fragment location.
    pub fn resolve_locator(&mut self, loc: Locator) -> Result<ResolvedLocation, EpubError> {
        match loc {
            Locator::Chapter(index) => {
                let chapter =
                    self.chapters
                        .get(index)
                        .cloned()
                        .ok_or(EpubError::ChapterOutOfBounds {
                            index,
                            chapter_count: self.chapters.len(),
                        })?;
                self.current.chapter_index = index;
                self.current.chapter_href = Some(chapter.href.clone());
                self.current.anchor = None;
                Ok(ResolvedLocation {
                    chapter,
                    fragment: None,
                    position: self.current.clone(),
                })
            }
            Locator::Href(href) => {
                let (base, fragment) = split_href_fragment(&href);
                let (index, chapter) = self
                    .chapters
                    .iter()
                    .enumerate()
                    .find(|(_, chapter)| chapter.href == base)
                    .map(|(idx, chapter)| (idx, chapter.clone()))
                    .ok_or_else(|| {
                        EpubError::InvalidEpub(format!("unknown chapter href: {}", href))
                    })?;
                self.current.chapter_index = index;
                self.current.chapter_href = Some(chapter.href.clone());
                self.current.anchor = fragment.clone();
                Ok(ResolvedLocation {
                    chapter,
                    fragment,
                    position: self.current.clone(),
                })
            }
            Locator::Fragment(fragment) => {
                let idx = self
                    .current
                    .chapter_index
                    .min(self.chapters.len().saturating_sub(1));
                let chapter =
                    self.chapters
                        .get(idx)
                        .cloned()
                        .ok_or(EpubError::ChapterOutOfBounds {
                            index: idx,
                            chapter_count: self.chapters.len(),
                        })?;
                self.current.chapter_index = idx;
                self.current.chapter_href = Some(chapter.href.clone());
                self.current.anchor = Some(fragment.clone());
                Ok(ResolvedLocation {
                    chapter,
                    fragment: Some(fragment),
                    position: self.current.clone(),
                })
            }
            Locator::TocId(id) => {
                let nav = self.navigation.as_ref().ok_or_else(|| {
                    EpubError::Navigation("no navigation document available".to_string())
                })?;
                let href = find_toc_href(nav, &id).ok_or_else(|| {
                    EpubError::Navigation(format!("toc id/label not found: {}", id))
                })?;
                self.resolve_locator(Locator::Href(href))
            }
            Locator::Position(pos) => {
                self.seek_position(&pos)?;
                self.resolve_locator(Locator::Chapter(pos.chapter_index))
            }
        }
    }
}

/// Resumable pagination session that tracks parse/layout state across page turns.
///
/// This session maintains cursor state so that page N+1 can continue from where
/// page N left off without re-parsing the chapter from the start.
#[derive(Clone, Debug)]
pub struct PaginationSession {
    /// Current chapter index.
    chapter_index: usize,
    /// Current byte offset in chapter source.
    byte_offset: usize,
    /// Current event/token index.
    event_index: usize,
    /// Element stack at current position.
    element_stack: Vec<String>,
    /// Current page number.
    page_number: usize,
    /// Whether chapter has been fully processed.
    chapter_complete: bool,
}

impl PaginationSession {
    /// Create a new pagination session starting at chapter 0.
    pub fn new() -> Self {
        Self {
            chapter_index: 0,
            byte_offset: 0,
            event_index: 0,
            element_stack: Vec::with_capacity(32),
            page_number: 0,
            chapter_complete: false,
        }
    }

    /// Create session for a specific chapter and offset.
    pub fn at_position(chapter: usize, byte_offset: usize, event_index: usize) -> Self {
        Self {
            chapter_index: chapter,
            byte_offset,
            event_index,
            element_stack: Vec::with_capacity(32),
            page_number: 0,
            chapter_complete: false,
        }
    }

    /// Get current chapter index.
    pub fn chapter_index(&self) -> usize {
        self.chapter_index
    }

    /// Get current byte offset.
    pub fn byte_offset(&self) -> usize {
        self.byte_offset
    }

    /// Get current event index.
    pub fn event_index(&self) -> usize {
        self.event_index
    }

    /// Get current page number.
    pub fn page_number(&self) -> usize {
        self.page_number
    }

    /// Check if current chapter is fully processed.
    pub fn is_chapter_complete(&self) -> bool {
        self.chapter_complete
    }

    /// Advance to next page, preserving cursor state.
    pub fn next_page(&mut self) {
        self.page_number += 1;
    }

    /// Advance cursor position.
    pub fn advance(&mut self, bytes: usize, events: usize) {
        self.byte_offset += bytes;
        self.event_index += events;
    }

    /// Push element onto stack.
    pub fn push_element(&mut self, tag: &str) {
        self.element_stack.push(tag.to_string());
    }

    /// Pop element from stack.
    pub fn pop_element(&mut self) -> Option<String> {
        self.element_stack.pop()
    }

    /// Move to next chapter.
    pub fn next_chapter(&mut self) {
        self.chapter_index += 1;
        self.byte_offset = 0;
        self.event_index = 0;
        self.element_stack.clear();
        self.chapter_complete = false;
    }

    /// Mark chapter as complete.
    pub fn mark_chapter_complete(&mut self) {
        self.chapter_complete = true;
    }

    /// Reset for new chapter.
    pub fn reset_chapter(&mut self, chapter_index: usize) {
        self.chapter_index = chapter_index;
        self.byte_offset = 0;
        self.event_index = 0;
        self.element_stack.clear();
        self.chapter_complete = false;
    }

    /// Create a reading position from current state.
    pub fn to_position(&self) -> ReadingPosition {
        ReadingPosition {
            chapter_index: self.chapter_index,
            chapter_href: None,
            anchor: None,
            fallback_offset: self.byte_offset,
        }
    }
}

impl Default for PaginationSession {
    fn default() -> Self {
        Self::new()
    }
}

fn split_href_fragment(href: &str) -> (String, Option<String>) {
    if let Some((base, fragment)) = href.split_once('#') {
        return (base.to_string(), Some(fragment.to_string()));
    }
    (href.to_string(), None)
}

fn find_toc_href(nav: &Navigation, id: &str) -> Option<String> {
    fn visit(points: &[NavPoint], id: &str) -> Option<String> {
        for point in points {
            let (_, fragment) = split_href_fragment(&point.href);
            if point.label == id || fragment.as_deref() == Some(id) {
                return Some(point.href.clone());
            }
            if let Some(hit) = visit(&point.children, id) {
                return Some(hit);
            }
        }
        None
    }
    visit(&nav.toc, id)
}

impl EpubBook<File> {
    /// Open an EPUB from disk and parse core structures.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, EpubError> {
        Self::open_with_options(path, EpubBookOptions::default())
    }

    /// Open an EPUB from disk with explicit options.
    pub fn open_with_options<P: AsRef<Path>>(
        path: P,
        options: EpubBookOptions,
    ) -> Result<Self, EpubError> {
        let file = File::open(path).map_err(|e| EpubError::Io(e.to_string()))?;
        Self::from_reader_with_options(file, options)
    }

    /// Open an EPUB from disk with compatibility open configuration.
    pub fn open_with_config<P: AsRef<Path>>(
        path: P,
        config: OpenConfig,
    ) -> Result<Self, EpubError> {
        let file = File::open(path).map_err(|e| EpubError::Io(e.to_string()))?;
        Self::from_reader_with_config(file, config)
    }
}

impl<R: Read + Seek> EpubBook<R> {
    /// Open an EPUB from any `Read + Seek` source and parse core structures.
    ///
    /// # Allocation behavior
    /// - Bounded by `ZipLimits` in options
    /// - Allocates central directory cache (~4KB fixed)
    /// - Worst-case: ~10KB for metadata + navigation
    pub fn from_reader(reader: R) -> Result<Self, EpubError> {
        Self::from_reader_with_options(reader, EpubBookOptions::default())
    }

    /// Open an EPUB from any `Read + Seek` source and parse core structures.
    ///
    /// # Allocation behavior
    /// - Bounded by `ZipLimits` in options
    /// - Caller buffer required: No
    /// - Worst-case memory: Configurable via `options.zip_limits`
    pub fn from_reader_with_options(
        reader: R,
        options: EpubBookOptions,
    ) -> Result<Self, EpubError> {
        Self::from_reader_with_config(reader, OpenConfig::from(options))
    }

    /// Open an EPUB from any `Read + Seek` source with compatibility open configuration.
    ///
    /// # Allocation behavior
    /// - Bounded by `ZipLimits` in config.options
    /// - Supports lazy navigation loading to defer allocation
    /// - Caller buffer required: No
    pub fn from_reader_with_config(reader: R, config: OpenConfig) -> Result<Self, EpubError> {
        let options = config.options;
        let mut zip =
            StreamingZip::new_with_limits(reader, options.zip_limits).map_err(EpubError::Zip)?;
        zip.validate_mimetype().map_err(EpubError::Zip)?;

        let container = read_entry(&mut zip, "META-INF/container.xml")?;
        let opf_path = crate::metadata::parse_container_xml(&container)?;
        let opf = read_entry(&mut zip, &opf_path)?;
        let metadata = extract_metadata(&container, &opf)?;
        let spine = crate::spine::parse_spine(&opf)?;
        validate_open_invariants(&metadata, &spine, options.validation_mode)?;
        let (navigation, navigation_loaded) = if config.lazy_navigation {
            (None, false)
        } else {
            (
                parse_navigation(
                    &mut zip,
                    &metadata,
                    &spine,
                    &opf_path,
                    options.validation_mode,
                    options.max_nav_bytes,
                )?,
                true,
            )
        };

        Ok(Self {
            zip,
            opf_path,
            metadata,
            spine,
            validation_mode: options.validation_mode,
            max_nav_bytes: options.max_nav_bytes,
            navigation_loaded,
            navigation,
            embedded_fonts_cache: None,
        })
    }

    /// EPUB package metadata.
    pub fn metadata(&self) -> &EpubMetadata {
        &self.metadata
    }

    /// Convenience: metadata title.
    pub fn title(&self) -> &str {
        self.metadata.title.as_str()
    }

    /// Convenience: metadata author.
    pub fn author(&self) -> &str {
        self.metadata.author.as_str()
    }

    /// Convenience: metadata language.
    pub fn language(&self) -> &str {
        self.metadata.language.as_str()
    }

    /// Reading order from `<spine>`.
    pub fn spine(&self) -> &Spine {
        &self.spine
    }

    /// Parsed navigation document, when one is available.
    pub fn navigation(&self) -> Option<&Navigation> {
        self.navigation.as_ref()
    }

    /// Lazily parse and cache navigation data when not loaded yet.
    pub fn ensure_navigation(&mut self) -> Result<Option<&Navigation>, EpubError> {
        if !self.navigation_loaded {
            self.navigation = parse_navigation(
                &mut self.zip,
                &self.metadata,
                &self.spine,
                &self.opf_path,
                self.validation_mode,
                self.max_nav_bytes,
            )?;
            self.navigation_loaded = true;
        }
        Ok(self.navigation.as_ref())
    }

    /// Convenience: top-level TOC entries from parsed navigation.
    pub fn toc(&self) -> Option<&[NavPoint]> {
        self.navigation.as_ref().map(|n| n.toc.as_slice())
    }

    /// Number of entries in the spine reading order.
    pub fn chapter_count(&self) -> usize {
        self.spine.len()
    }

    /// Create a detached reading session for locator/progress operations.
    pub fn reading_session(&self) -> ReadingSession {
        ReadingSession::new(self.chapters().collect(), self.navigation.clone())
    }

    /// Enumerate chapters in spine order.
    pub fn chapters(&self) -> impl Iterator<Item = ChapterRef> + '_ {
        self.spine
            .items()
            .iter()
            .enumerate()
            .filter_map(|(index, spine_item)| {
                self.metadata
                    .get_item(&spine_item.idref)
                    .map(|manifest_item| ChapterRef {
                        index,
                        idref: spine_item.idref.clone(),
                        href: manifest_item.href.clone(),
                        media_type: manifest_item.media_type.clone(),
                    })
            })
    }

    /// Get a chapter descriptor by spine index.
    pub fn chapter(&self, index: usize) -> Result<ChapterRef, EpubError> {
        let spine_item = self
            .spine
            .get_item(index)
            .ok_or(EpubError::ChapterOutOfBounds {
                index,
                chapter_count: self.spine.len(),
            })?;

        let manifest_item = self.metadata.get_item(&spine_item.idref).ok_or_else(|| {
            EpubError::ManifestItemMissing {
                idref: spine_item.idref.clone(),
            }
        })?;

        Ok(ChapterRef {
            index,
            idref: spine_item.idref.clone(),
            href: manifest_item.href.clone(),
            media_type: manifest_item.media_type.clone(),
        })
    }

    /// Get a chapter descriptor by spine `idref`.
    pub fn chapter_by_id(&self, idref: &str) -> Result<ChapterRef, EpubError> {
        let index = self
            .spine
            .items()
            .iter()
            .position(|item| item.idref == idref)
            .ok_or_else(|| EpubError::ManifestItemMissing {
                idref: idref.to_string(),
            })?;
        self.chapter(index)
    }

    /// Read a resource by OPF-relative href into a new `Vec<u8>`.
    ///
    /// Fragment suffixes (e.g. `chapter.xhtml#p3`) are ignored.
    ///
    /// # Allocation behavior
    /// - **Allocates**: Returns new `Vec<u8>`
    /// - **Non-embedded-fast-path**: Use `read_resource_into` for embedded
    /// - Caller buffer required: No
    /// - Worst-case memory: Unbounded (depends on file size)
    ///
    /// For bounded allocation, use `read_resource_into_with_limit`.
    pub fn read_resource(&mut self, href: &str) -> Result<Vec<u8>, EpubError> {
        let mut out = Vec::with_capacity(0);
        self.read_resource_into(href, &mut out)?;
        Ok(out)
    }

    /// Stream a resource by OPF-relative href into a writer.
    ///
    /// Fragment suffixes (e.g. `chapter.xhtml#p3`) are ignored.
    ///
    /// # Allocation behavior
    /// - **Zero hidden allocations**: Uses bounded internal buffers
    /// - Caller buffer required: Yes (writer handles output)
    /// - **Preferred for embedded**: Streaming API
    pub fn read_resource_into<W: Write>(
        &mut self,
        href: &str,
        writer: &mut W,
    ) -> Result<usize, EpubError> {
        self.read_resource_into_with_hard_cap(href, writer, usize::MAX)
    }

    /// Stream a resource by OPF-relative href into a writer with an explicit cap.
    ///
    /// Fragment suffixes (e.g. `chapter.xhtml#p3`) are ignored.
    pub fn read_resource_into_with_limit<W: Write>(
        &mut self,
        href: &str,
        writer: &mut W,
        max_bytes: usize,
    ) -> Result<usize, EpubError> {
        self.read_resource_into_with_hard_cap(href, writer, max_bytes)
    }

    /// Stream a resource by OPF-relative href with a hard cap.
    ///
    /// Fragment suffixes (e.g. `chapter.xhtml#p3`) are ignored.
    pub fn read_resource_into_with_hard_cap<W: Write>(
        &mut self,
        href: &str,
        writer: &mut W,
        hard_cap_bytes: usize,
    ) -> Result<usize, EpubError> {
        let zip_path = resolve_opf_relative_path(&self.opf_path, href);
        read_entry_into_with_limit(&mut self.zip, &zip_path, writer, hard_cap_bytes)
    }

    /// Read spine item content bytes by index.
    pub fn read_spine_item_bytes(&mut self, index: usize) -> Result<Vec<u8>, EpubError> {
        let href = self.chapter(index)?.href;

        self.read_resource(&href)
    }

    /// Read a spine chapter as UTF-8 HTML/XHTML text by index.
    ///
    /// # Allocation behavior
    /// - **Allocates**: Returns new `String`
    /// - **Non-embedded-fast-path**: Use `chapter_html_into` for embedded
    /// - Caller buffer required: No
    /// - Worst-case memory: Depends on chapter size
    ///
    /// For bounded allocation, use `chapter_html_into_with_limit`.
    pub fn chapter_html(&mut self, index: usize) -> Result<String, EpubError> {
        let mut out = String::with_capacity(0);
        self.chapter_html_into(index, &mut out)?;
        Ok(out)
    }

    /// Read a spine chapter as UTF-8 HTML/XHTML text into caller-provided output.
    ///
    /// # Allocation behavior
    /// - **Zero hidden allocations**: Reuses caller's String buffer
    /// - Caller buffer required: Yes
    /// - **Preferred for embedded**: Buffer reuse API
    pub fn chapter_html_into(&mut self, index: usize, out: &mut String) -> Result<(), EpubError> {
        self.chapter_html_into_with_limit(index, usize::MAX, out)
    }

    /// Read a spine chapter as UTF-8 HTML/XHTML text with a hard byte cap into caller output.
    pub fn chapter_html_into_with_limit(
        &mut self,
        index: usize,
        max_bytes: usize,
        out: &mut String,
    ) -> Result<(), EpubError> {
        out.clear();
        let chapter = self.chapter(index)?;
        let mut bytes = Vec::with_capacity(0);
        self.read_resource_into_with_hard_cap(&chapter.href, &mut bytes, max_bytes)?;
        let mut html = String::from_utf8(bytes)
            .map_err(|_| EpubError::ChapterNotUtf8 { href: chapter.href })?;
        core::mem::swap(out, &mut html);
        Ok(())
    }

    /// Resolve chapter stylesheet sources in cascade order.
    pub fn chapter_stylesheets(&mut self, index: usize) -> Result<ChapterStylesheets, EpubError> {
        self.chapter_stylesheets_with_options(index, StyleLimits::default())
    }

    /// Resolve chapter stylesheet sources in cascade order with explicit limits.
    pub fn chapter_stylesheets_with_options(
        &mut self,
        index: usize,
        limits: StyleLimits,
    ) -> Result<ChapterStylesheets, EpubError> {
        let chapter = self.chapter(index)?;
        let html = self.chapter_html(index)?;
        let links = parse_stylesheet_links(&chapter.href, &html);
        let mut sources = Vec::with_capacity(0);

        for href in links {
            let bytes = self.read_resource(&href)?;
            if bytes.len() > limits.max_css_bytes {
                return Err(EpubError::Parse(format!(
                    "Stylesheet exceeds max_css_bytes ({} > {}) at '{}'",
                    bytes.len(),
                    limits.max_css_bytes,
                    href
                )));
            }
            let css = String::from_utf8(bytes)
                .map_err(|_| EpubError::Parse(format!("Stylesheet is not UTF-8: {}", href)))?;
            sources.push(StylesheetSource { href, css });
        }

        Ok(ChapterStylesheets { sources })
    }

    /// Backward-compatible alias for chapter stylesheet discovery with explicit limits.
    pub fn styles_for_chapter(
        &mut self,
        index: usize,
        limits: StyleLimits,
    ) -> Result<ChapterStylesheets, EpubError> {
        self.chapter_stylesheets_with_options(index, limits)
    }

    /// Resolve chapter stylesheet sources with caller-provided buffers.
    ///
    /// This variant reuses the provided buffer for each stylesheet read, avoiding
    /// repeated allocations in the hot path.
    ///
    /// # Allocation behavior
    /// - **Zero per-stylesheet allocations**: Reuses caller-provided buffer
    /// - Caller buffer required: Yes (scratch_buf for I/O)
    /// - **Preferred for embedded**: Avoids allocation per stylesheet
    pub fn chapter_stylesheets_with_scratch(
        &mut self,
        index: usize,
        limits: StyleLimits,
        scratch_buf: &mut Vec<u8>,
    ) -> Result<ChapterStylesheets, EpubError> {
        let chapter = self.chapter(index)?;
        let html = self.chapter_html(index)?;
        let links = parse_stylesheet_links(&chapter.href, &html);
        let mut sources = Vec::with_capacity(links.len());

        for href in links {
            scratch_buf.clear();
            self.read_resource_into(&href, scratch_buf)
                .map_err(|_| EpubError::Zip(ZipError::FileNotFound))?;

            if scratch_buf.len() > limits.max_css_bytes {
                return Err(EpubError::LimitExceeded {
                    kind: LimitKind::CssSize,
                    actual: scratch_buf.len(),
                    limit: limits.max_css_bytes,
                    path: Some(href.clone()),
                });
            }

            let css = String::from_utf8(scratch_buf.clone())
                .map_err(|_| EpubError::ChapterNotUtf8 { href: href.clone() })?;

            sources.push(StylesheetSource { href, css });
        }

        Ok(ChapterStylesheets { sources })
    }

    /// Enumerate embedded font-face metadata from EPUB CSS resources.
    pub fn embedded_fonts(&mut self) -> Result<Vec<EmbeddedFontFace>, EpubError> {
        self.embedded_fonts_with_limits(FontLimits::default())
    }

    /// Enumerate embedded font-face metadata with explicit limits.
    pub fn embedded_fonts_with_options(
        &mut self,
        limits: FontLimits,
    ) -> Result<Vec<EmbeddedFontFace>, EpubError> {
        self.embedded_fonts_with_limits(limits)
    }

    /// Enumerate embedded font-face metadata with explicit limits.
    ///
    /// This path lazily scans CSS once and reuses cached face metadata on subsequent calls.
    pub fn embedded_fonts_with_limits(
        &mut self,
        limits: FontLimits,
    ) -> Result<Vec<EmbeddedFontFace>, EpubError> {
        let faces = self.ensure_embedded_fonts_loaded()?;
        if faces.len() > limits.max_faces {
            return Err(EpubError::LimitExceeded {
                kind: LimitKind::FontLimit,
                actual: faces.len(),
                limit: limits.max_faces,
                path: None,
            });
        }
        Ok(faces.clone())
    }

    /// Enumerate embedded font-face metadata with caller-provided buffer.
    ///
    /// This variant scans CSS files without caching, using a caller-provided buffer
    /// to avoid allocations. Suitable for one-time font enumeration in constrained
    /// environments where caching is not desired.
    ///
    /// # Allocation behavior
    /// - **Zero per-CSS allocations**: Reuses caller-provided buffer
    /// - Caller buffer required: Yes (scratch_buf for I/O)
    /// - No caching: Always reads from archive
    pub fn embedded_fonts_with_scratch(
        &mut self,
        limits: FontLimits,
        scratch_buf: &mut Vec<u8>,
    ) -> Result<Vec<EmbeddedFontFace>, EpubError> {
        let css_hrefs: Vec<String> = self
            .metadata
            .manifest
            .iter()
            .filter(|item| item.media_type == "text/css")
            .map(|item| item.href.clone())
            .collect();

        let mut out = Vec::with_capacity(limits.max_faces.min(16));

        for href in css_hrefs {
            if out.len() >= limits.max_faces {
                break;
            }

            scratch_buf.clear();
            match self.read_resource_into(&href, scratch_buf) {
                Ok(_) => {}
                Err(_) => continue, // Skip missing CSS files
            }

            if scratch_buf.len() > limits.max_bytes_per_font {
                continue; // Skip oversized CSS
            }

            let css = match String::from_utf8(scratch_buf.clone()) {
                Ok(s) => s,
                Err(_) => continue, // Skip non-UTF8 CSS
            };

            let faces = parse_font_faces_from_css(&href, &css);
            for face in faces {
                if out.len() >= limits.max_faces {
                    break;
                }
                out.push(face);
            }
        }

        Ok(out)
    }

    /// Style chapter content into an event/run stream with default options.
    ///
    /// # Allocation behavior
    /// - **Allocates**: Returns `StyledChapter` with internal Vec
    /// - **Non-embedded-fast-path**: Use `chapter_events` for streaming
    /// - Caller buffer required: No
    /// - Worst-case memory: Depends on `MemoryBudget` in options
    pub fn chapter_styled_runs(&mut self, index: usize) -> Result<StyledChapter, EpubError> {
        self.chapter_styled_runs_with_options(index, RenderPrepOptions::default())
    }

    /// Style chapter content into an event/run stream with explicit options.
    ///
    /// # Allocation behavior
    /// - **Bounded by limits**: Respects `MemoryBudget` in options
    /// - Caller buffer required: No
    /// - Worst-case memory: Configurable via `options.memory`
    pub fn chapter_styled_runs_with_options(
        &mut self,
        index: usize,
        options: RenderPrepOptions,
    ) -> Result<StyledChapter, EpubError> {
        let mut prep = RenderPrep::new(options).with_serif_default();
        let prepared = prep.prepare_chapter(self, index).map_err(EpubError::from)?;
        let mut items = Vec::with_capacity(0);
        for item in prepared.iter() {
            items.push(item.clone());
        }
        Ok(StyledChapter::from_items(items))
    }

    /// Stream chapter style events/runs via callback with bounded item emission.
    ///
    /// # Allocation behavior
    /// - **Zero hidden allocations**: Uses bounded internal buffers
    /// - Caller buffer required: No (callback receives items)
    /// - **Preferred for embedded**: Streaming API with item caps
    /// - Worst-case memory: Bounded by `opts.render.memory`
    pub fn chapter_events<F>(
        &mut self,
        index: usize,
        opts: ChapterEventsOptions,
        mut on_item: F,
    ) -> Result<usize, EpubError>
    where
        F: FnMut(StyledEventOrRun) -> Result<(), EpubError>,
    {
        let mut prep = RenderPrep::new(opts.render).with_serif_default();
        let mut emitted = 0usize;
        let mut callback_error: Option<EpubError> = None;
        let mut hit_cap = false;

        prep.prepare_chapter_with(self, index, |item| {
            if callback_error.is_some() || hit_cap {
                return;
            }
            if emitted >= opts.max_items {
                hit_cap = true;
                return;
            }
            if let Err(err) = on_item(item) {
                callback_error = Some(err);
                return;
            }
            emitted += 1;
        })
        .map_err(EpubError::from)?;

        if let Some(err) = callback_error {
            return Err(err);
        }
        if hit_cap {
            // TODO: RenderPrep callbacks cannot currently short-circuit parsing.
            // This cap bounds emitted output, but upstream tokenization keeps scanning.
            return Err(EpubError::Parse(format!(
                "Chapter event count exceeded max_items ({})",
                opts.max_items
            )));
        }
        Ok(emitted)
    }

    /// Stream chapter events with caller-provided scratch buffers.
    ///
    /// This is the most memory-efficient API for processing chapter content. It uses
    /// caller-provided buffers for all internal operations, avoiding any hidden allocations
    /// in the hot path.
    ///
    /// # Allocation behavior
    /// - **Zero hidden allocations**: Uses only caller-provided buffers
    /// - Caller buffer required: Yes (chapter_buf, token_buf, scratch)
    /// - **Preferred for embedded**: True zero-allocation streaming path
    /// - Worst-case memory: Bounded by provided buffer sizes
    ///
    /// # Errors
    /// Returns `EpubError::BufferTooSmall` if provided buffers are insufficient.
    /// Returns `EpubError::LimitExceeded` if hard caps are reached.
    pub fn chapter_events_with_scratch<F>(
        &mut self,
        index: usize,
        opts: ChapterEventsOptions,
        chapter_buf: &mut Vec<u8>,
        scratch: &mut crate::streaming::ScratchBuffers,
        mut on_item: F,
    ) -> Result<ChapterStreamResult, EpubError>
    where
        F: FnMut(StyledEventOrRun) -> Result<(), EpubError>,
    {
        use crate::zip::CdEntry;

        // Clear buffers for reuse
        chapter_buf.clear();
        scratch.clear();

        let chapter = self.chapter(index)?;
        let href = chapter.href;
        let zip_path = resolve_opf_relative_path(&self.opf_path, &href);

        // Get ZIP entry
        let entry = self
            .zip
            .get_entry(&zip_path)
            .ok_or(EpubError::Zip(ZipError::FileNotFound))?
            .clone();

        // Check hard caps before reading
        let uncompressed = usize::try_from(entry.uncompressed_size)
            .map_err(|_| EpubError::Zip(ZipError::FileTooLarge))?;

        // Check ZIP limits
        if let Some(limits) = self.zip.limits() {
            if uncompressed > limits.max_file_read_size {
                return Err(EpubError::LimitExceeded {
                    kind: LimitKind::FileSize,
                    actual: uncompressed,
                    limit: limits.max_file_read_size,
                    path: Some(zip_path),
                });
            }
        }

        // Check memory budget
        if uncompressed > opts.render.memory.max_entry_bytes {
            return Err(EpubError::LimitExceeded {
                kind: LimitKind::MemoryBudget,
                actual: uncompressed,
                limit: opts.render.memory.max_entry_bytes,
                path: Some(zip_path),
            });
        }

        // Check if chapter fits in provided buffer
        if uncompressed > chapter_buf.capacity() {
            return Err(EpubError::BufferTooSmall {
                required: uncompressed,
                provided: chapter_buf.capacity(),
                context: "chapter_buf".to_string(),
            });
        }

        // Read chapter into caller-provided buffer using scratch for I/O
        let use_entry = CdEntry {
            filename: String::with_capacity(0),
            method: entry.method,
            compressed_size: entry.compressed_size,
            uncompressed_size: entry.uncompressed_size,
            local_header_offset: entry.local_header_offset,
            crc32: entry.crc32,
        };

        // `ZipArchive::read_file_with_scratch` consumes `&mut [u8]` and therefore
        // uses slice length (not Vec capacity) as the writable output window.
        // Ensure the Vec length matches the entry size before decompression while
        // staying within the pre-validated capacity budget.
        if chapter_buf.len() < uncompressed {
            chapter_buf.resize(uncompressed, 0);
        }
        if scratch.read_buf.is_empty() {
            // `read_file_with_scratch` requires a non-empty input slice and uses
            // slice length as the compressed read chunk size.
            let read_chunk = scratch.read_buf.capacity().max(1024);
            scratch.read_buf.resize(read_chunk, 0);
        }
        let bytes_read = self
            .zip
            .read_file_with_scratch(
                &use_entry,
                chapter_buf.as_mut_slice(),
                &mut scratch.read_buf,
            )
            .map_err(EpubError::Zip)?;
        chapter_buf.truncate(bytes_read);

        let mut emitted = 0usize;
        let mut callback_err: Option<EpubError> = None;
        let mut prep = RenderPrep::new(opts.render).with_serif_default();
        prep.prepare_chapter_bytes_with(self, index, chapter_buf, |item| {
            if callback_err.is_some() || emitted >= opts.max_items {
                return;
            }
            if let Err(e) = on_item(item) {
                callback_err = Some(e);
                return;
            }
            emitted += 1;
        })
        .map_err(EpubError::from)?;

        if let Some(err) = callback_err {
            return Err(err);
        }
        if emitted >= opts.max_items {
            return Err(EpubError::LimitExceeded {
                kind: LimitKind::EventCount,
                actual: emitted,
                limit: opts.max_items,
                path: Some(zip_path),
            });
        }

        Ok(ChapterStreamResult {
            items_emitted: emitted,
            bytes_read: chapter_buf.len(),
            complete: true,
        })
    }

    /// Read a chapter and return plain text extracted from token stream.
    ///
    /// # Allocation behavior
    /// - **Allocates**: Returns new `String`
    /// - **Non-embedded-fast-path**: Use `chapter_text_into` for embedded
    /// - Caller buffer required: No
    /// - Worst-case memory: Depends on chapter text size
    ///
    /// For lower memory usage, prefer `chapter_text_into`/`chapter_text_with_limit`.
    pub fn chapter_text(&mut self, index: usize) -> Result<String, EpubError> {
        let mut out = String::with_capacity(0);
        self.chapter_text_into(index, &mut out)?;
        Ok(out)
    }

    /// Extract plain text for a chapter into a caller-provided string buffer.
    ///
    /// This avoids allocating an intermediate `Vec<Token>` and is intended as
    /// the default API for constrained environments.
    ///
    /// # Allocation behavior
    /// - **Zero hidden allocations**: Reuses caller's String buffer
    /// - Caller buffer required: Yes
    /// - **Preferred for embedded**: Primary text extraction API
    pub fn chapter_text_into(&mut self, index: usize, out: &mut String) -> Result<(), EpubError> {
        self.chapter_text_into_with_limit(index, usize::MAX, out)
    }

    /// Extract plain text for a chapter and cap output to `max_bytes`.
    ///
    /// Output is truncated on a UTF-8 boundary when the limit is reached.
    pub fn chapter_text_with_limit(
        &mut self,
        index: usize,
        max_bytes: usize,
    ) -> Result<String, EpubError> {
        let mut out = String::with_capacity(0);
        self.chapter_text_into_with_limit(index, max_bytes, &mut out)?;
        Ok(out)
    }

    /// Extract plain text into caller-provided storage, with a hard byte cap.
    ///
    /// Existing content of `out` is cleared before writing.
    pub fn chapter_text_into_with_limit(
        &mut self,
        index: usize,
        max_bytes: usize,
        out: &mut String,
    ) -> Result<(), EpubError> {
        out.clear();
        if max_bytes == 0 {
            return Ok(());
        }

        let chapter = self.chapter(index)?;
        let bytes = self.read_resource(&chapter.href)?;
        extract_plain_text_limited(&bytes, max_bytes, out)
    }

    /// Tokenize spine item content by index.
    ///
    /// # Allocation behavior
    /// - **Allocates**: Returns `Vec<Token>` (unbounded growth possible)
    /// - **Non-embedded-fast-path**: Use `chapter_text_into` for bounded paths
    /// - Caller buffer required: No
    /// - Worst-case memory: Unbounded (depends on chapter complexity)
    ///
    /// Prefer `chapter_text_into` for low-memory extraction paths.
    /// For bounded tokenization, use `tokenize_html_limited` from the tokenizer module.
    pub fn tokenize_spine_item(&mut self, index: usize) -> Result<Vec<Token>, EpubError> {
        let chapter = self.chapter(index)?;
        let bytes = self.read_resource(&chapter.href)?;
        let html =
            str::from_utf8(&bytes).map_err(|_| EpubError::ChapterNotUtf8 { href: chapter.href })?;
        tokenize_html(html).map_err(EpubError::from)
    }

    /// Backward-compatible alias for `read_spine_item_bytes`.
    pub fn read_spine_chapter(&mut self, index: usize) -> Result<Vec<u8>, EpubError> {
        self.read_spine_item_bytes(index)
    }

    /// Backward-compatible alias for `tokenize_spine_item`.
    pub fn tokenize_spine_chapter(&mut self, index: usize) -> Result<Vec<Token>, EpubError> {
        self.tokenize_spine_item(index)
    }

    fn ensure_embedded_fonts_loaded(&mut self) -> Result<&Vec<EmbeddedFontFace>, EpubError> {
        if self.embedded_fonts_cache.is_none() {
            let css_hrefs: Vec<String> = self
                .metadata
                .manifest
                .iter()
                .filter(|item| item.media_type == "text/css")
                .map(|item| item.href.clone())
                .collect();
            let mut out = Vec::with_capacity(0);
            for href in css_hrefs {
                let bytes = self.read_resource(&href)?;
                let css = String::from_utf8(bytes)
                    .map_err(|_| EpubError::Parse(format!("Stylesheet is not UTF-8: {}", href)))?;
                out.extend(parse_font_faces_from_css(&href, &css));
            }
            self.embedded_fonts_cache = Some(out);
        }
        self.embedded_fonts_cache
            .as_ref()
            .ok_or_else(|| EpubError::Parse("Embedded font cache initialization failed".into()))
    }
}

impl EpubBook<File> {
    /// Create a high-level builder for opening/parsing EPUBs.
    pub fn builder() -> EpubBookBuilder {
        EpubBookBuilder::new()
    }
}

fn load_summary_from_zip<R: Read + Seek>(
    zip: &mut StreamingZip<R>,
    options: EpubBookOptions,
) -> Result<EpubSummary, EpubError> {
    zip.validate_mimetype().map_err(EpubError::Zip)?;
    let container = read_entry(zip, "META-INF/container.xml")?;
    let opf_path = crate::metadata::parse_container_xml(&container)?;
    let opf = read_entry(zip, &opf_path)?;
    let metadata = extract_metadata(&container, &opf)?;
    let spine = crate::spine::parse_spine(&opf)?;
    validate_open_invariants(&metadata, &spine, options.validation_mode)?;
    let navigation = parse_navigation(
        zip,
        &metadata,
        &spine,
        &opf_path,
        options.validation_mode,
        options.max_nav_bytes,
    )?;

    Ok(EpubSummary {
        metadata,
        spine,
        navigation,
    })
}

fn parse_navigation<R: Read + Seek>(
    zip: &mut StreamingZip<R>,
    metadata: &EpubMetadata,
    spine: &Spine,
    opf_path: &str,
    validation_mode: ValidationMode,
    max_nav_bytes: Option<usize>,
) -> Result<Option<Navigation>, EpubError> {
    let nav_item = spine
        .toc_id()
        .and_then(|toc_id| metadata.get_item(toc_id))
        .or_else(|| {
            metadata.manifest.iter().find(|item| {
                item.properties
                    .as_deref()
                    .is_some_and(|p| p.split_whitespace().any(|prop| prop == "nav"))
            })
        })
        .or_else(|| {
            metadata.manifest.iter().find(|item| {
                item.media_type == "application/x-dtbncx+xml"
                    || item.href.to_ascii_lowercase().ends_with(".ncx")
            })
        });

    let Some(nav_item) = nav_item else {
        return Ok(None);
    };

    let nav_path = resolve_opf_relative_path(opf_path, &nav_item.href);
    let nav_bytes = match read_entry(zip, &nav_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            if matches!(validation_mode, ValidationMode::Strict) {
                return Err(err);
            }
            log::warn!("Failed to read navigation document '{}': {}", nav_path, err);
            return Ok(None);
        }
    };

    if let Some(limit) = max_nav_bytes {
        if nav_bytes.len() > limit {
            return Err(EpubError::Phase(PhaseError {
                phase: ErrorPhase::Open,
                code: "NAV_BYTES_LIMIT",
                message: format!(
                    "Navigation bytes exceed configured max_nav_bytes ({} > {})",
                    nav_bytes.len(),
                    limit
                )
                .into_boxed_str(),
                context: Some(Box::new(PhaseErrorContext {
                    source: None,
                    path: Some(nav_path.clone().into_boxed_str()),
                    href: Some(nav_item.href.clone().into_boxed_str()),
                    chapter_index: None,
                    selector: None,
                    selector_index: None,
                    declaration: None,
                    declaration_index: None,
                    token_offset: None,
                    limit: Some(Box::new(ErrorLimitContext::new(
                        "max_nav_bytes",
                        nav_bytes.len(),
                        limit,
                    ))),
                })),
            }));
        }
    }

    let parsed = if nav_item.media_type == "application/x-dtbncx+xml"
        || nav_item.href.to_ascii_lowercase().ends_with(".ncx")
    {
        parse_ncx(&nav_bytes)
    } else {
        parse_nav_xhtml(&nav_bytes)
    };

    match parsed {
        Ok(nav) => Ok(Some(nav)),
        Err(err) => {
            if matches!(validation_mode, ValidationMode::Strict) {
                Err(EpubError::Navigation(err.to_string()))
            } else {
                log::warn!(
                    "Failed to parse navigation document '{}': {}",
                    nav_path,
                    err
                );
                Ok(None)
            }
        }
    }
}

fn validate_open_invariants(
    metadata: &EpubMetadata,
    spine: &Spine,
    validation_mode: ValidationMode,
) -> Result<(), EpubError> {
    if matches!(validation_mode, ValidationMode::Lenient) {
        return Ok(());
    }

    for item in spine.items() {
        if metadata.get_item(&item.idref).is_none() {
            return Err(EpubError::ManifestItemMissing {
                idref: item.idref.clone(),
            });
        }
    }

    Ok(())
}

fn read_entry<R: Read + Seek>(zip: &mut StreamingZip<R>, path: &str) -> Result<Vec<u8>, EpubError> {
    let mut buf = Vec::with_capacity(0);
    read_entry_into(zip, path, &mut buf)?;
    Ok(buf)
}

fn read_entry_into<R: Read + Seek, W: Write>(
    zip: &mut StreamingZip<R>,
    path: &str,
    writer: &mut W,
) -> Result<usize, EpubError> {
    read_entry_into_with_limit(zip, path, writer, usize::MAX)
}

fn read_entry_into_with_limit<R: Read + Seek, W: Write>(
    zip: &mut StreamingZip<R>,
    path: &str,
    writer: &mut W,
    max_bytes: usize,
) -> Result<usize, EpubError> {
    let (method, compressed_size, uncompressed_size, local_header_offset, crc32) = {
        let entry = zip
            .get_entry(path)
            .ok_or(EpubError::Zip(ZipError::FileNotFound))?;
        (
            entry.method,
            entry.compressed_size,
            entry.uncompressed_size,
            entry.local_header_offset,
            entry.crc32,
        )
    };

    if uncompressed_size > max_bytes as u64 || compressed_size > max_bytes as u64 {
        return Err(EpubError::Zip(ZipError::FileTooLarge));
    }
    let entry = CdEntry {
        method,
        compressed_size,
        uncompressed_size,
        local_header_offset,
        crc32,
        filename: String::with_capacity(0),
    };
    zip.read_file_to_writer(&entry, writer)
        .map_err(EpubError::Zip)
}

fn resolve_opf_relative_path(opf_path: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    if href.is_empty() {
        return normalize_path(opf_path);
    }
    if href.starts_with('/') {
        return normalize_path(href.trim_start_matches('/'));
    }
    if href.contains("://") {
        return href.to_string();
    }

    let base_dir = opf_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    if base_dir.is_empty() {
        normalize_path(href)
    } else {
        normalize_path(&format!("{}/{}", base_dir, href))
    }
}

fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(0);
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn should_skip_text_tag(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "head" | "nav" | "header" | "footer" | "aside" | "noscript"
    )
}

fn normalize_plain_text_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_was_space = true;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(ch);
            prev_was_space = false;
        }
    }
    if result.ends_with(' ') {
        result.pop();
    }
    result
}

fn push_limited(out: &mut String, value: &str, max_bytes: usize) -> bool {
    if out.len() >= max_bytes || value.is_empty() {
        return out.len() >= max_bytes;
    }
    let remaining = max_bytes - out.len();
    if value.len() <= remaining {
        out.push_str(value);
        return false;
    }
    let mut end = remaining;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if end > 0 {
        out.push_str(&value[..end]);
    }
    true
}

fn push_newline_limited(out: &mut String, max_bytes: usize) -> bool {
    if out.is_empty() || out.ends_with('\n') {
        return false;
    }
    push_limited(out, "\n", max_bytes)
}

fn push_text_limited(out: &mut String, text: &str, max_bytes: usize) -> bool {
    if text.is_empty() {
        return false;
    }
    if !out.is_empty() && !out.ends_with('\n') && push_limited(out, " ", max_bytes) {
        return true;
    }
    push_limited(out, text, max_bytes)
}

fn extract_plain_text_limited(
    html: &[u8],
    max_bytes: usize,
    out: &mut String,
) -> Result<(), EpubError> {
    let mut reader = Reader::from_reader(html);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    let mut buf = Vec::with_capacity(0);
    let mut skip_depth = 0usize;
    let mut done = false;

    while !done {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?
                    .to_string();
                if should_skip_text_tag(&name) {
                    skip_depth += 1;
                } else if skip_depth == 0
                    && matches!(name.as_str(), "p" | "div" | "li")
                    && push_newline_limited(out, max_bytes)
                {
                    done = true;
                }
            }
            Ok(Event::Empty(e)) => {
                if skip_depth > 0 {
                    buf.clear();
                    continue;
                }
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?
                    .to_string();
                if matches!(name.as_str(), "br" | "p" | "div" | "li")
                    && push_newline_limited(out, max_bytes)
                {
                    done = true;
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?
                    .to_string();
                if should_skip_text_tag(&name) {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if skip_depth == 0
                    && matches!(name.as_str(), "p" | "div" | "li")
                    && push_newline_limited(out, max_bytes)
                {
                    done = true;
                }
            }
            Ok(Event::Text(e)) => {
                if skip_depth > 0 {
                    buf.clear();
                    continue;
                }
                let text = e
                    .decode()
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?
                    .to_string();
                let normalized = normalize_plain_text_whitespace(&text);
                if push_text_limited(out, &normalized, max_bytes) {
                    done = true;
                }
            }
            Ok(Event::CData(e)) => {
                if skip_depth > 0 {
                    buf.clear();
                    continue;
                }
                let text = reader
                    .decoder()
                    .decode(&e)
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?
                    .to_string();
                let normalized = normalize_plain_text_whitespace(&text);
                if push_text_limited(out, &normalized, max_bytes) {
                    done = true;
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if skip_depth > 0 {
                    buf.clear();
                    continue;
                }
                let entity_name = e
                    .decode()
                    .map_err(|err| EpubError::Parse(format!("Decode error: {:?}", err)))?;
                let entity = format!("&{};", entity_name);
                let resolved = quick_xml::escape::unescape(&entity)
                    .map_err(|err| EpubError::Parse(format!("Unescape error: {:?}", err)))?
                    .to_string();
                let normalized = normalize_plain_text_whitespace(&resolved);
                if push_text_limited(out, &normalized, max_bytes) {
                    done = true;
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => return Err(EpubError::Parse(format!("XML error: {:?}", err))),
        }
        buf.clear();
    }

    if out.ends_with('\n') {
        out.pop();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_prep::{RenderPrep, RenderPrepOptions, RenderPrepTrace, StyledEventOrRun};

    #[test]
    fn test_resolve_opf_relative_path() {
        assert_eq!(
            resolve_opf_relative_path("EPUB/package.opf", "text/ch1.xhtml"),
            "EPUB/text/ch1.xhtml"
        );
        assert_eq!(
            resolve_opf_relative_path("OEBPS/content.opf", "../toc.ncx"),
            "toc.ncx"
        );
        assert_eq!(
            resolve_opf_relative_path("package.opf", "chapter.xhtml#p1"),
            "chapter.xhtml"
        );
        assert_eq!(
            resolve_opf_relative_path("EPUB/package.opf", "/META-INF/container.xml"),
            "META-INF/container.xml"
        );
    }

    #[test]
    fn test_read_resource_into_streams_to_writer() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");

        let mut out = Vec::with_capacity(0);
        let n = book
            .read_resource_into("xhtml/nav.xhtml", &mut out)
            .expect("resource should stream");
        assert_eq!(n, out.len());
        assert!(!out.is_empty());
    }

    #[test]
    fn test_read_resource_into_with_hard_cap_errors_when_exceeded() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");

        let mut out = Vec::with_capacity(0);
        let err = book
            .read_resource_into_with_hard_cap("xhtml/nav.xhtml", &mut out, 8)
            .expect_err("hard cap should fail");
        assert!(matches!(err, EpubError::Zip(ZipError::FileTooLarge)));
    }

    #[test]
    fn test_read_resource_into_with_limit_succeeds_when_under_cap() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");

        let mut out = Vec::with_capacity(0);
        let n = book
            .read_resource_into_with_limit("xhtml/nav.xhtml", &mut out, 1024 * 1024)
            .expect("limit should allow nav payload");
        assert_eq!(n, out.len());
        assert!(!out.is_empty());
    }

    #[test]
    fn test_open_enforces_max_nav_bytes_limit() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let err = match EpubBook::from_reader_with_options(
            file,
            EpubBookOptions {
                max_nav_bytes: Some(8),
                ..EpubBookOptions::default()
            },
        ) {
            Ok(_) => panic!("open should fail when navigation exceeds cap"),
            Err(err) => err,
        };
        match err {
            EpubError::Phase(phase) => {
                assert_eq!(phase.code, "NAV_BYTES_LIMIT");
                let ctx = phase.context.expect("phase context should be present");
                let limit = ctx.limit.expect("limit context should be present");
                assert_eq!(limit.kind.as_ref(), "max_nav_bytes");
                assert_eq!(limit.limit, 8);
            }
            other => panic!("expected phase error, got {:?}", other),
        }
    }

    #[test]
    fn test_lazy_navigation_loaded_by_ensure_navigation() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader_with_config(
            file,
            OpenConfig {
                options: EpubBookOptions::default(),
                lazy_navigation: true,
            },
        )
        .expect("book should open");
        assert!(book.navigation().is_none());
        let nav = book
            .ensure_navigation()
            .expect("ensure navigation should parse");
        assert!(nav.is_some());
    }

    #[test]
    fn test_chapter_text_into_matches_chapter_text() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let baseline = book.chapter_text(0).expect("chapter text should extract");
        let mut out = String::with_capacity(0);
        book.chapter_text_into(0, &mut out)
            .expect("chapter text into should extract");
        assert_eq!(baseline, out);
    }

    #[test]
    fn test_chapter_html_into_matches_chapter_html() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");

        let baseline = book.chapter_html(0).expect("chapter html should extract");
        let mut out = String::with_capacity(0);
        book.chapter_html_into(0, &mut out)
            .expect("chapter html into should extract");
        assert_eq!(baseline, out);
    }

    #[test]
    fn test_chapter_html_into_with_limit_enforces_cap() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");

        let mut out = String::with_capacity(0);
        let err = book
            .chapter_html_into_with_limit(0, 8, &mut out)
            .expect_err("hard cap should fail");
        assert!(matches!(err, EpubError::Zip(ZipError::FileTooLarge)));
    }

    #[test]
    fn test_chapter_text_with_limit_truncates_safely() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let full = book.chapter_text(0).expect("full text should extract");
        let limited = book
            .chapter_text_with_limit(0, 64)
            .expect("limited text should extract");
        assert!(limited.len() <= 64);
        assert!(full.starts_with(&limited));
    }

    #[test]
    fn test_chapter_text_with_zero_limit_is_empty() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let limited = book
            .chapter_text_with_limit(0, 0)
            .expect("limited text should extract");
        assert!(limited.is_empty());
    }

    #[test]
    fn test_chapter_text_into_with_limit_clears_existing_buffer() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let mut out = String::from("stale content");
        book.chapter_text_into_with_limit(0, 32, &mut out)
            .expect("limited text should extract");
        assert!(!out.starts_with("stale content"));
        assert!(out.len() <= 32);
    }

    #[test]
    fn test_extract_plain_text_limited_preserves_utf8_boundaries() {
        let html = "<p>hello 😀 world</p>";
        let mut out = String::with_capacity(0);
        extract_plain_text_limited(html.as_bytes(), 8, &mut out).expect("extract should succeed");
        assert!(out.len() <= 8);
        assert!(core::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn test_chapter_stylesheets_api_works() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let styles = book
            .chapter_stylesheets(0)
            .expect("chapter_stylesheets should succeed");
        assert!(styles.sources.iter().all(|s| !s.href.is_empty()));
    }

    #[test]
    fn test_styles_for_chapter_alias_matches_with_options() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let limits = StyleLimits::default();
        let a = book
            .chapter_stylesheets_with_options(0, limits)
            .expect("chapter_stylesheets_with_options should succeed");
        let b = book
            .styles_for_chapter(0, limits)
            .expect("styles_for_chapter should succeed");
        assert_eq!(a, b);
    }

    #[test]
    fn test_embedded_fonts_api_works() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let fonts = book
            .embedded_fonts()
            .expect("embedded_fonts should succeed");
        assert!(fonts.len() <= crate::render_prep::FontLimits::default().max_faces);
    }

    #[test]
    fn test_embedded_fonts_with_limits_alias_matches_with_options() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let limits = FontLimits::default();
        let a = book
            .embedded_fonts_with_options(limits)
            .expect("embedded_fonts_with_options should succeed");
        let b = book
            .embedded_fonts_with_limits(limits)
            .expect("embedded_fonts_with_limits should succeed");
        assert_eq!(a, b);
    }

    #[test]
    fn test_render_prep_golden_path_prepare_chapter() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let mut prep = RenderPrep::new(RenderPrepOptions::default())
            .with_serif_default()
            .with_embedded_fonts_from_book(&mut book)
            .expect("font registration should succeed");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);
        let chapter = prep
            .prepare_chapter(&mut book, index)
            .expect("prepare_chapter should succeed");
        assert!(chapter.iter().count() > 0);
    }

    #[test]
    fn test_chapter_styled_runs_api_returns_items() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);
        let styled = book
            .chapter_styled_runs(index)
            .expect("chapter_styled_runs should succeed");
        assert!(styled.iter().count() > 0);
    }

    #[test]
    fn test_chapter_events_streaming_emits_items() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);

        let mut seen = 0usize;
        let emitted = book
            .chapter_events(index, ChapterEventsOptions::default(), |_| {
                seen += 1;
                Ok(())
            })
            .expect("chapter_events should succeed");
        assert_eq!(emitted, seen);
        assert!(emitted > 0);
    }

    #[test]
    fn test_chapter_events_respects_max_items_cap() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);

        let err = book
            .chapter_events(
                index,
                ChapterEventsOptions {
                    max_items: 1,
                    ..ChapterEventsOptions::default()
                },
                |_| Ok(()),
            )
            .expect_err("max_items cap should fail");
        assert!(matches!(err, EpubError::Parse(_)));
    }

    #[test]
    fn test_render_prep_prepare_chapter_into_streams_items() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);
        let mut prep = RenderPrep::new(RenderPrepOptions::default())
            .with_serif_default()
            .with_embedded_fonts_from_book(&mut book)
            .expect("font registration should succeed");
        let mut out = Vec::with_capacity(0);
        prep.prepare_chapter_into(&mut book, index, &mut out)
            .expect("prepare_chapter_into should succeed");
        assert!(!out.is_empty());
    }

    #[test]
    fn test_render_prep_runs_persist_resolved_font_id() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);
        let mut prep = RenderPrep::new(RenderPrepOptions::default())
            .with_serif_default()
            .with_embedded_fonts_from_book(&mut book)
            .expect("font registration should succeed");

        let mut saw_run = false;
        prep.prepare_chapter_with_trace_context(&mut book, index, |item, trace| {
            if let StyledEventOrRun::Run(run) = item {
                saw_run = true;
                let font_trace = trace.font_trace().expect("run should include font trace");
                assert_eq!(run.font_id, font_trace.face.font_id);
                assert_eq!(run.resolved_family, font_trace.face.family);
            }
        })
        .expect("prepare_chapter_with_trace_context should succeed");
        assert!(saw_run);
    }

    #[test]
    fn test_render_prep_trace_context_contains_font_and_style_for_runs() {
        let file = std::fs::File::open(
            "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub",
        )
        .expect("fixture should open");
        let mut book = EpubBook::from_reader(file).expect("book should open");
        let index = (0..book.chapter_count())
            .find(|idx| {
                book.chapter_text_with_limit(*idx, 256)
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
            })
            .unwrap_or(0);
        let mut prep = RenderPrep::new(RenderPrepOptions::default())
            .with_serif_default()
            .with_embedded_fonts_from_book(&mut book)
            .expect("font registration should succeed");

        let mut saw_run = false;
        prep.prepare_chapter_with_trace_context(&mut book, index, |item, trace| match item {
            StyledEventOrRun::Run(run) => {
                saw_run = true;
                match trace {
                    RenderPrepTrace::Run { style, font } => {
                        assert_eq!(style.as_ref(), &run.style);
                        assert_eq!(font.face.font_id, run.font_id);
                        assert_eq!(font.face.family, run.resolved_family);
                    }
                    RenderPrepTrace::Event => panic!("run item should produce run trace context"),
                }
            }
            StyledEventOrRun::Event(_) => {
                assert!(matches!(trace, RenderPrepTrace::Event));
            }
        })
        .expect("prepare_chapter_with_trace_context should succeed");
        assert!(saw_run);
    }

    #[test]
    fn test_reading_session_resolve_locator_and_progress() {
        let chapters = vec![
            ChapterRef {
                index: 0,
                idref: "c1".to_string(),
                href: "text/ch1.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
            },
            ChapterRef {
                index: 1,
                idref: "c2".to_string(),
                href: "text/ch2.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
            },
        ];
        let nav = Navigation {
            toc: vec![NavPoint {
                label: "intro".to_string(),
                href: "text/ch2.xhtml#start".to_string(),
                children: Vec::with_capacity(0),
            }],
            page_list: Vec::with_capacity(0),
            landmarks: Vec::with_capacity(0),
        };
        let mut session = ReadingSession::new(chapters, Some(nav));
        let resolved = session
            .resolve_locator(Locator::TocId("intro".to_string()))
            .expect("toc id should resolve");
        assert_eq!(resolved.chapter.index, 1);
        assert_eq!(resolved.fragment.as_deref(), Some("start"));
        assert!(session.book_progress() > 0.0);
    }

    #[test]
    fn test_reading_session_seek_position_out_of_bounds() {
        let chapters = vec![ChapterRef {
            index: 0,
            idref: "c1".to_string(),
            href: "text/ch1.xhtml".to_string(),
            media_type: "application/xhtml+xml".to_string(),
        }];
        let mut session = ReadingSession::new(chapters, None);
        let err = session
            .seek_position(&ReadingPosition {
                chapter_index: 2,
                chapter_href: None,
                anchor: None,
                fallback_offset: 0,
            })
            .expect_err("seek should fail");
        assert!(matches!(err, EpubError::ChapterOutOfBounds { .. }));
    }
}
