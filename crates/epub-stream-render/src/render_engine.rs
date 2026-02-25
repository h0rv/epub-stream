use epub_stream::navigation::NavPoint;
use epub_stream::{
    BlockRole, ChapterRef, EpubBook, FontPolicy, Locator, Navigation, RenderPrep, RenderPrepError,
    RenderPrepOptions, StyledEventOrRun,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::render_ir::{
    DrawCommand, ImageObjectCommand, JustifyMode, OverlayContent, OverlayItem, OverlayRect,
    OverlaySize, OverlaySlot, PageAnnotation, PageChromeCommand, PageChromeKind, PageMetrics,
    PaginationProfileId, RectCommand, RenderPage, ResolvedTextStyle, RuleCommand, TextCommand,
};
use crate::render_layout::{
    LayoutConfig, LayoutEngine, LayoutSession as CoreLayoutSession, TextMeasurer,
};

/// Cancellation hook for long-running layout operations.
pub trait CancelToken {
    fn is_cancelled(&self) -> bool;
}

/// Summary emitted after chapter layout completes.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChapterLayoutSummary {
    /// Total pages produced for this chapter.
    pub page_count: usize,
}

/// Never-cancel token for default call paths.
#[derive(Clone, Copy, Debug, Default)]
pub struct NeverCancel;

impl CancelToken for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Runtime diagnostics from render preparation/layout.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderDiagnostic {
    ReflowTimeMs(u32),
    Cancelled,
    MemoryLimitExceeded {
        kind: &'static str,
        actual: usize,
        limit: usize,
    },
    CacheHit {
        chapter_index: usize,
        page_count: usize,
    },
    CacheMiss {
        chapter_index: usize,
    },
}

type DiagnosticCallback = Arc<Mutex<Box<dyn FnMut(RenderDiagnostic) + Send + 'static>>>;
type DiagnosticSink = Option<DiagnosticCallback>;

/// Render-engine options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderEngineOptions {
    /// Prep options passed to `RenderPrep`.
    pub prep: RenderPrepOptions,
    /// Layout options used to produce pages.
    pub layout: LayoutConfig,
}

impl RenderEngineOptions {
    /// Build options for a target display size.
    pub fn for_display(width: i32, height: i32) -> Self {
        Self {
            prep: RenderPrepOptions::default(),
            layout: LayoutConfig::for_display(width, height),
        }
    }
}

/// Alias used for chapter page slicing.
pub type PageRange = core::ops::Range<usize>;

/// Compact chapter-level page span used by `RenderBookPageMap`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderBookPageMapEntry {
    /// Chapter index in spine order (0-based).
    pub chapter_index: usize,
    /// Chapter href in OPF-relative form.
    pub chapter_href: String,
    /// First global rendered page index for this chapter.
    pub first_page_index: usize,
    /// Rendered page count for this chapter.
    pub page_count: usize,
}

impl RenderBookPageMapEntry {
    fn contains_global_page(&self, global_page_index: usize) -> bool {
        if self.page_count == 0 {
            return false;
        }
        let start = self.first_page_index;
        let end = start.saturating_add(self.page_count);
        global_page_index >= start && global_page_index < end
    }
}

/// Deterministic locator resolution kind for rendered page targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderLocatorTargetKind {
    /// Locator mapped directly to the chapter start page.
    ChapterStart,
    /// Locator fragment mapped to a chapter start fallback.
    FragmentFallbackChapterStart,
    /// Locator fragment matched to an explicit anchor page.
    ///
    /// Reserved for future anchor-index integration.
    FragmentAnchor,
}

/// Resolved rendered page target for locator/href operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderLocatorPageTarget {
    /// Resolved global page index.
    pub page_index: usize,
    /// Resolved chapter index.
    pub chapter_index: usize,
    /// Resolved chapter href.
    pub chapter_href: String,
    /// Optional fragment payload (without leading '#').
    pub fragment: Option<String>,
    /// Resolution strategy used for this target.
    pub kind: RenderLocatorTargetKind,
}

/// Persisted rendered reading position token.
///
/// The token stores chapter identity hints plus normalized chapter/global progress
/// so callers can remap positions after reflow/profile changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderReadingPositionToken {
    /// Chapter index hint from the source pagination profile.
    pub chapter_index: usize,
    /// Optional chapter href hint for robust remap across index shifts.
    pub chapter_href: Option<String>,
    /// Page offset within the chapter in the source pagination profile.
    pub chapter_page_index: usize,
    /// Total pages in the source chapter.
    pub chapter_page_count: usize,
    /// Chapter progress ratio in `[0.0, 1.0]`.
    pub chapter_progress: f32,
    /// Global page index in the source rendered sequence.
    pub global_page_index: usize,
    /// Total global pages in the source rendered sequence.
    pub global_page_count: usize,
}

impl RenderReadingPositionToken {
    fn normalized_chapter_progress(&self) -> f32 {
        if self.chapter_page_count > 1 {
            return page_progress_from_count(self.chapter_page_index, self.chapter_page_count);
        }
        normalize_progress(self.chapter_progress)
    }

    fn normalized_global_progress(&self) -> f32 {
        page_progress_from_count(self.global_page_index, self.global_page_count)
    }
}

/// Compact chapter-level rendered page index for locator and remap operations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderBookPageMap {
    entries: Vec<RenderBookPageMapEntry>,
    total_pages: usize,
}

impl RenderBookPageMap {
    /// Build a compact page map from spine chapters and rendered page counts.
    ///
    /// `chapter_page_counts` is interpreted in spine-index space.
    /// Missing entries are treated as `0` pages.
    pub fn from_chapter_page_counts(
        chapters: &[ChapterRef],
        chapter_page_counts: &[usize],
    ) -> Self {
        let mut entries = Vec::with_capacity(chapters.len());
        let mut first_page_index = 0usize;

        for (order_index, chapter) in chapters.iter().enumerate() {
            let page_count = chapter_page_counts
                .get(chapter.index)
                .copied()
                .or_else(|| chapter_page_counts.get(order_index).copied())
                .unwrap_or(0);
            entries.push(RenderBookPageMapEntry {
                chapter_index: chapter.index,
                chapter_href: chapter.href.clone(),
                first_page_index,
                page_count,
            });
            first_page_index = first_page_index.saturating_add(page_count);
        }

        Self {
            entries,
            total_pages: first_page_index,
        }
    }

    /// Chapter-level entries in spine order.
    pub fn entries(&self) -> &[RenderBookPageMapEntry] {
        &self.entries
    }

    /// Total rendered page count represented by this map.
    pub fn total_pages(&self) -> usize {
        self.total_pages
    }

    /// Resolve the first rendered page for `chapter_index`.
    ///
    /// Returns `None` if the chapter has zero rendered pages or is absent.
    pub fn chapter_start_page_index(&self, chapter_index: usize) -> Option<usize> {
        self.chapter_entry_with_pages(chapter_index)
            .map(|entry| entry.first_page_index)
    }

    /// Resolve the rendered global page range for `chapter_index`.
    ///
    /// Returns `None` if the chapter has zero rendered pages or is absent.
    pub fn chapter_page_range(&self, chapter_index: usize) -> Option<PageRange> {
        self.chapter_entry_with_pages(chapter_index).map(|entry| {
            entry.first_page_index..entry.first_page_index.saturating_add(entry.page_count)
        })
    }

    /// Resolve a chapter/fragment href into a rendered page target.
    ///
    /// Fragment mapping is best-effort. Until anchor-level mappings are available,
    /// fragment hrefs deterministically fall back to chapter start.
    pub fn resolve_href(&self, href: &str) -> Option<RenderLocatorPageTarget> {
        self.resolve_href_with_fragment_progress(href, None)
    }

    /// Resolve a chapter/fragment href with optional normalized fragment progress.
    ///
    /// When `fragment_progress` is provided for hrefs containing a fragment,
    /// the target page is resolved to the nearest page in that chapter and the
    /// target kind is marked as `FragmentAnchor`.
    pub fn resolve_href_with_fragment_progress(
        &self,
        href: &str,
        fragment_progress: Option<f32>,
    ) -> Option<RenderLocatorPageTarget> {
        let (base_href, fragment) = split_href_fragment(href);
        if base_href.is_empty() {
            return None;
        }

        let chapter_index = self.find_chapter_index_for_href(base_href)?;
        let chapter = self.chapter_entry_with_pages(chapter_index)?;
        let mut page_index = chapter.first_page_index;
        let kind = match fragment {
            Some(_) => {
                if let Some(progress) = fragment_progress {
                    let local_index = progress_to_page_index(progress, chapter.page_count.max(1));
                    page_index = chapter.first_page_index.saturating_add(local_index);
                    RenderLocatorTargetKind::FragmentAnchor
                } else {
                    RenderLocatorTargetKind::FragmentFallbackChapterStart
                }
            }
            None => RenderLocatorTargetKind::ChapterStart,
        };

        Some(RenderLocatorPageTarget {
            page_index,
            chapter_index: chapter.chapter_index,
            chapter_href: chapter.chapter_href.clone(),
            fragment: fragment.map(ToOwned::to_owned),
            kind,
        })
    }

    /// Alias for resolving TOC href targets.
    pub fn resolve_toc_href(&self, href: &str) -> Option<RenderLocatorPageTarget> {
        self.resolve_href(href)
    }

    /// Resolve a `epub_stream::Locator` into a rendered page target.
    ///
    /// Pass `navigation` when resolving `Locator::TocId`.
    pub fn resolve_locator(
        &self,
        locator: &Locator,
        navigation: Option<&Navigation>,
    ) -> Option<RenderLocatorPageTarget> {
        match locator {
            Locator::Chapter(chapter_index) => self.resolve_chapter_start(*chapter_index, None),
            Locator::Href(href) => self.resolve_href(href),
            Locator::Fragment(_) => None,
            Locator::TocId(id) => {
                let navigation = navigation?;
                let href = find_toc_href(navigation, id)?;
                self.resolve_href(&href)
            }
            Locator::Position(pos) => self.resolve_position_locator(pos),
        }
    }

    /// Build a persisted reading-position token from a rendered global page index.
    ///
    /// Out-of-range indices are clamped to the nearest valid page.
    pub fn reading_position_token_for_page_index(
        &self,
        global_page_index: usize,
    ) -> Option<RenderReadingPositionToken> {
        if self.total_pages == 0 {
            return None;
        }

        let clamped = global_page_index.min(self.total_pages.saturating_sub(1));
        let chapter = self.entry_for_global_page(clamped)?;
        let chapter_offset = clamped.saturating_sub(chapter.first_page_index);

        Some(RenderReadingPositionToken {
            chapter_index: chapter.chapter_index,
            chapter_href: Some(chapter.chapter_href.clone()),
            chapter_page_index: chapter_offset,
            chapter_page_count: chapter.page_count.max(1),
            chapter_progress: page_progress_from_count(chapter_offset, chapter.page_count.max(1)),
            global_page_index: clamped,
            global_page_count: self.total_pages.max(1),
        })
    }

    /// Remap a persisted reading-position token into this page map.
    ///
    /// Remap keeps chapter identity when the chapter is still present and has
    /// rendered pages; otherwise it falls back to global progress remap.
    pub fn remap_reading_position_token(
        &self,
        token: &RenderReadingPositionToken,
    ) -> Option<usize> {
        if self.total_pages == 0 {
            return None;
        }

        if let Some(chapter) = self.chapter_entry_for_token(token) {
            let local_index =
                progress_to_page_index(token.normalized_chapter_progress(), chapter.page_count);
            return Some(chapter.first_page_index.saturating_add(local_index));
        }

        Some(progress_to_page_index(
            token.normalized_global_progress(),
            self.total_pages,
        ))
    }

    fn resolve_position_locator(
        &self,
        position: &epub_stream::ReadingPosition,
    ) -> Option<RenderLocatorPageTarget> {
        if let Some(href) = position.chapter_href.as_deref() {
            if let Some(anchor) = position
                .anchor
                .as_deref()
                .filter(|anchor| !anchor.is_empty())
            {
                let mut href_with_fragment = String::with_capacity(
                    href.len().saturating_add(anchor.len()).saturating_add(1),
                );
                href_with_fragment.push_str(href);
                href_with_fragment.push('#');
                href_with_fragment.push_str(anchor);
                if let Some(target) = self.resolve_href(&href_with_fragment) {
                    return Some(target);
                }
            }
            if let Some(target) = self.resolve_href(href) {
                return Some(target);
            }
        }
        self.resolve_chapter_start(position.chapter_index, position.anchor.as_deref())
    }

    fn resolve_chapter_start(
        &self,
        chapter_index: usize,
        anchor: Option<&str>,
    ) -> Option<RenderLocatorPageTarget> {
        let chapter = self.chapter_entry_with_pages(chapter_index)?;
        let has_anchor = anchor.is_some_and(|value| !value.is_empty());
        let kind = if has_anchor {
            RenderLocatorTargetKind::FragmentFallbackChapterStart
        } else {
            RenderLocatorTargetKind::ChapterStart
        };

        Some(RenderLocatorPageTarget {
            page_index: chapter.first_page_index,
            chapter_index: chapter.chapter_index,
            chapter_href: chapter.chapter_href.clone(),
            fragment: anchor.map(ToOwned::to_owned),
            kind,
        })
    }

    fn chapter_entry_for_token(
        &self,
        token: &RenderReadingPositionToken,
    ) -> Option<&RenderBookPageMapEntry> {
        if let Some(href) = token.chapter_href.as_deref() {
            let (base_href, _) = split_href_fragment(href);
            if let Some(chapter_index) = self.find_chapter_index_for_href(base_href) {
                if let Some(chapter) = self.chapter_entry_with_pages(chapter_index) {
                    return Some(chapter);
                }
            }
        }
        self.chapter_entry_with_pages(token.chapter_index)
    }

    fn chapter_entry_with_pages(&self, chapter_index: usize) -> Option<&RenderBookPageMapEntry> {
        self.entries
            .iter()
            .find(|entry| entry.chapter_index == chapter_index && entry.page_count > 0)
    }

    fn entry_for_global_page(&self, global_page_index: usize) -> Option<&RenderBookPageMapEntry> {
        self.entries
            .iter()
            .find(|entry| entry.contains_global_page(global_page_index))
    }

    fn find_chapter_index_for_href(&self, base_href: &str) -> Option<usize> {
        if base_href.is_empty() {
            return None;
        }

        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.chapter_href == base_href)
        {
            return Some(entry.chapter_index);
        }

        let normalized_target = normalize_rel_path(base_href);
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| normalize_rel_path(&entry.chapter_href) == normalized_target)
        {
            return Some(entry.chapter_index);
        }

        let target_basename = basename_of(&normalized_target);
        if target_basename.is_empty() {
            return None;
        }
        let mut candidate: Option<usize> = None;
        for entry in &self.entries {
            let normalized_entry = normalize_rel_path(&entry.chapter_href);
            let entry_basename = basename_of(&normalized_entry);
            if entry_basename != target_basename {
                continue;
            }
            if candidate.is_some() {
                return None;
            }
            candidate = Some(entry.chapter_index);
        }
        candidate
    }
}

/// Storage hooks for render-page caches.
pub trait RenderCacheStore {
    /// Load cached pages for `chapter_index` and pagination profile, if available.
    fn load_chapter_pages(
        &self,
        _profile: PaginationProfileId,
        _chapter_index: usize,
    ) -> Option<Vec<RenderPage>> {
        None
    }

    /// Persist rendered chapter pages for the pagination profile.
    fn store_chapter_pages(
        &self,
        _profile: PaginationProfileId,
        _chapter_index: usize,
        _pages: &[RenderPage],
    ) {
    }
}

const CACHE_SCHEMA_VERSION: u8 = 1;
const DEFAULT_MAX_CACHE_FILE_BYTES: usize = 4 * 1024 * 1024;
static CACHE_WRITE_NONCE: AtomicUsize = AtomicUsize::new(0);

/// File-backed render-page cache store.
///
/// Cache paths are deterministic by pagination profile and chapter index:
/// `<root>/<profile-hex>/chapter-<index>.json`.
///
/// The store uses a JSON envelope with a schema version and enforces
/// `max_file_bytes` on both reads and writes. When I/O, decode, or size checks
/// fail, operations return `None`/no-op instead of bubbling errors.
#[derive(Clone, Debug)]
pub struct FileRenderCacheStore {
    root: PathBuf,
    max_file_bytes: usize,
}

impl FileRenderCacheStore {
    /// Create a new cache store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_file_bytes: DEFAULT_MAX_CACHE_FILE_BYTES,
        }
    }

    /// Set the maximum allowed cache file size in bytes.
    ///
    /// Values of `0` are treated as `1` to keep the cap explicit.
    pub fn with_max_file_bytes(mut self, max_file_bytes: usize) -> Self {
        self.max_file_bytes = max_file_bytes.max(1);
        self
    }

    /// Root directory for cache files.
    pub fn cache_root(&self) -> &Path {
        &self.root
    }

    /// Maximum allowed cache file size in bytes.
    pub fn max_file_bytes(&self) -> usize {
        self.max_file_bytes
    }

    /// Deterministic cache path for profile/chapter payload.
    pub fn chapter_cache_path(
        &self,
        profile: PaginationProfileId,
        chapter_index: usize,
    ) -> PathBuf {
        let profile_dir = profile_hex(profile);
        self.root
            .join(profile_dir)
            .join(format!("chapter-{}.json", chapter_index)) // allow: file I/O path, not hot
    }
}

impl RenderCacheStore for FileRenderCacheStore {
    fn load_chapter_pages(
        &self,
        profile: PaginationProfileId,
        chapter_index: usize,
    ) -> Option<Vec<RenderPage>> {
        let path = self.chapter_cache_path(profile, chapter_index);
        let max_file_bytes = self.max_file_bytes as u64;
        if fs::metadata(&path).ok()?.len() > max_file_bytes {
            return None;
        }

        let file = File::open(path).ok()?;
        let mut reader = file.take(max_file_bytes.saturating_add(1));
        let mut payload = Vec::with_capacity(8);
        if reader.read_to_end(&mut payload).is_err() {
            return None;
        }
        if payload.len() > self.max_file_bytes {
            return None;
        }
        let envelope: PersistedCacheEnvelope = serde_json::from_slice(&payload).ok()?;
        envelope.into_render_pages()
    }

    fn store_chapter_pages(
        &self,
        profile: PaginationProfileId,
        chapter_index: usize,
        pages: &[RenderPage],
    ) {
        let final_path = self.chapter_cache_path(profile, chapter_index);
        let Some(parent) = final_path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }

        let nonce = CACHE_WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            // allow: file I/O path, not hot
            "chapter-{}.json.tmp-{}-{}",
            chapter_index,
            std::process::id(),
            nonce
        ));

        let envelope = PersistedCacheEnvelope::from_pages(pages);
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => file,
            Err(_) => return,
        };
        let writer = BufWriter::new(file);
        let mut writer = CappedWriter::new(writer, self.max_file_bytes);
        if serde_json::to_writer(&mut writer, &envelope).is_err() {
            remove_file_quiet(&temp_path);
            return;
        }
        if writer.flush().is_err() {
            remove_file_quiet(&temp_path);
            return;
        }
        let mut writer = writer.into_inner();
        if writer.flush().is_err() {
            remove_file_quiet(&temp_path);
            return;
        }
        let file = match writer.into_inner() {
            Ok(file) => file,
            Err(_) => {
                remove_file_quiet(&temp_path);
                return;
            }
        };
        if file.sync_all().is_err() {
            remove_file_quiet(&temp_path);
            return;
        }
        drop(file);
        if fs::rename(&temp_path, &final_path).is_err() {
            remove_file_quiet(&temp_path);
            return;
        }
        sync_directory(parent);
    }
}

/// Resolve a page index in `new_pages` by chapter progress carried by
/// `old_pages[old_page_index]`.
pub fn remap_page_index_by_chapter_progress(
    old_pages: &[RenderPage],
    old_page_index: usize,
    new_pages: &[RenderPage],
) -> Option<usize> {
    let target = chapter_progress_for_index(old_pages, old_page_index);
    resolve_page_index_for_chapter_progress(target, new_pages)
}

/// Resolve a chapter progress value (`[0, 1]`) into a valid page index.
///
/// Returns `None` when `pages` is empty.
pub fn resolve_page_index_for_chapter_progress(
    chapter_progress: f32,
    pages: &[RenderPage],
) -> Option<usize> {
    if pages.is_empty() {
        return None;
    }
    let target = normalize_progress(chapter_progress);
    let mut best_idx = 0usize;
    let mut best_distance = f32::INFINITY;
    let mut prev_progress = 0.0f32;

    for (idx, page) in pages.iter().enumerate() {
        let mut page_progress = page_progress_for_index(page, idx, pages.len());
        if idx > 0 && page_progress < prev_progress {
            page_progress = prev_progress;
        }
        prev_progress = page_progress;

        let distance = (page_progress - target).abs();
        if distance < best_distance {
            best_distance = distance;
            best_idx = idx;
            continue;
        }
        if (distance - best_distance).abs() <= f32::EPSILON && idx < best_idx {
            best_idx = idx;
            continue;
        }
        if page_progress > target && distance > best_distance {
            break;
        }
    }

    Some(best_idx)
}

fn normalize_progress(progress: f32) -> f32 {
    if progress.is_finite() {
        return progress.clamp(0.0, 1.0);
    }
    0.0
}

fn page_progress_from_count(page_index: usize, page_count: usize) -> f32 {
    if page_count <= 1 {
        return 1.0;
    }
    let clamped = page_index.min(page_count.saturating_sub(1));
    (clamped as f32 / (page_count - 1) as f32).clamp(0.0, 1.0)
}

fn progress_to_page_index(progress: f32, page_count: usize) -> usize {
    if page_count <= 1 {
        return 0;
    }
    let max_index = page_count.saturating_sub(1);
    let scaled = normalize_progress(progress) * max_index as f32;
    let rounded = scaled.round();
    if !rounded.is_finite() || rounded <= 0.0 {
        return 0;
    }
    let index = rounded as usize;
    index.min(max_index)
}

fn split_href_fragment(href: &str) -> (&str, Option<&str>) {
    let (base, fragment) = match href.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (href, None),
    };
    let fragment = fragment.filter(|value| !value.is_empty());
    (base, fragment)
}

/// Estimate normalized chapter progress for an anchor fragment in XHTML bytes.
///
/// This is a lightweight best-effort helper intended for locator remap flows.
/// It searches for `id="<fragment>"`, `id='<fragment>'`, `name="<fragment>"`,
/// and `name='<fragment>'` byte patterns and maps the match offset into `[0, 1]`.
pub fn estimate_fragment_progress_in_html(chapter_html: &[u8], fragment: &str) -> Option<f32> {
    if chapter_html.is_empty() {
        return None;
    }
    let fragment = fragment.trim();
    if fragment.is_empty() {
        return None;
    }
    let needle = fragment.as_bytes();

    let mut patterns: [Vec<u8>; 4] = [
        Vec::with_capacity(needle.len() + 5),
        Vec::with_capacity(needle.len() + 5),
        Vec::with_capacity(needle.len() + 7),
        Vec::with_capacity(needle.len() + 7),
    ];
    patterns[0].extend_from_slice(b"id=\"");
    patterns[0].extend_from_slice(needle);
    patterns[0].push(b'"');

    patterns[1].extend_from_slice(b"id='");
    patterns[1].extend_from_slice(needle);
    patterns[1].push(b'\'');

    patterns[2].extend_from_slice(b"name=\"");
    patterns[2].extend_from_slice(needle);
    patterns[2].push(b'"');

    patterns[3].extend_from_slice(b"name='");
    patterns[3].extend_from_slice(needle);
    patterns[3].push(b'\'');

    let mut best: Option<usize> = None;
    for pattern in &patterns {
        if let Some(pos) = find_bytes(chapter_html, pattern) {
            best = Some(best.map_or(pos, |current| current.min(pos)));
        }
    }
    let position = best?;
    if chapter_html.len() <= 1 {
        return Some(0.0);
    }
    Some((position as f32 / (chapter_html.len() - 1) as f32).clamp(0.0, 1.0))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn normalize_rel_path(path: &str) -> String {
    let base = path
        .split_once('?')
        .map_or(path, |(without_query, _)| without_query);
    let slash_normalized = base.replace('\\', "/");
    let mut out_parts: Vec<&str> = Vec::with_capacity(8);

    for part in slash_normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            let _ = out_parts.pop();
            continue;
        }
        out_parts.push(part);
    }

    out_parts.join("/")
}

fn basename_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn find_toc_href(navigation: &Navigation, id: &str) -> Option<String> {
    fn visit(points: &[NavPoint], id: &str) -> Option<String> {
        for point in points {
            let (_, fragment) = split_href_fragment(&point.href);
            if point.label == id || fragment == Some(id) {
                return Some(point.href.clone());
            }
            if let Some(hit) = visit(&point.children, id) {
                return Some(hit);
            }
        }
        None
    }
    visit(&navigation.toc, id)
}

fn chapter_progress_for_index(pages: &[RenderPage], page_index: usize) -> f32 {
    if pages.is_empty() {
        return 0.0;
    }
    let clamped_index = page_index.min(pages.len().saturating_sub(1));
    page_progress_for_index(&pages[clamped_index], clamped_index, pages.len())
}

fn page_progress_for_index(page: &RenderPage, fallback_index: usize, fallback_total: usize) -> f32 {
    if let Some(chapter_page_count) = page.metrics.chapter_page_count {
        if chapter_page_count <= 1 {
            return 1.0;
        }
        let chapter_page_index = page
            .metrics
            .chapter_page_index
            .min(chapter_page_count.saturating_sub(1));
        return (chapter_page_index as f32 / (chapter_page_count - 1) as f32).clamp(0.0, 1.0);
    }
    if page.metrics.progress_chapter.is_finite() {
        return page.metrics.progress_chapter.clamp(0.0, 1.0);
    }
    if fallback_total <= 1 {
        return 1.0;
    }
    (fallback_index as f32 / (fallback_total - 1) as f32).clamp(0.0, 1.0)
}

fn profile_hex(profile: PaginationProfileId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in profile.0 {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn remove_file_quiet(path: &Path) {
    let _ = fs::remove_file(path);
}

fn sync_directory(path: &Path) {
    if let Ok(dir) = File::open(path) {
        let _ = dir.sync_all();
    }
}

struct CappedWriter<W> {
    inner: W,
    max_bytes: usize,
    written: usize,
}

impl<W> CappedWriter<W> {
    fn new(inner: W, max_bytes: usize) -> Self {
        Self {
            inner,
            max_bytes: max_bytes.max(1),
            written: 0,
        }
    }

    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CappedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let remaining = self.max_bytes.saturating_sub(self.written);
        if buf.len() > remaining {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cache file exceeds max_file_bytes",
            ));
        }
        self.inner.write_all(buf)?;
        self.written = self.written.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedCacheEnvelope {
    version: u8,
    pages: Vec<PersistedRenderPage>,
}

impl PersistedCacheEnvelope {
    fn from_pages(pages: &[RenderPage]) -> Self {
        Self {
            version: CACHE_SCHEMA_VERSION,
            pages: pages.iter().map(PersistedRenderPage::from).collect(),
        }
    }

    fn into_render_pages(self) -> Option<Vec<RenderPage>> {
        if self.version != CACHE_SCHEMA_VERSION {
            return None;
        }
        Some(self.pages.into_iter().map(RenderPage::from).collect())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedRenderPage {
    page_number: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    commands: Vec<PersistedDrawCommand>,
    content_commands: Vec<PersistedDrawCommand>,
    chrome_commands: Vec<PersistedDrawCommand>,
    overlay_commands: Vec<PersistedDrawCommand>,
    overlay_items: Vec<PersistedOverlayItem>,
    annotations: Vec<PersistedPageAnnotation>,
    metrics: PersistedPageMetrics,
}

impl From<&RenderPage> for PersistedRenderPage {
    fn from(page: &RenderPage) -> Self {
        Self {
            page_number: page.page_number,
            commands: page
                .commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            content_commands: page
                .content_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            chrome_commands: page
                .chrome_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            overlay_commands: page
                .overlay_commands
                .iter()
                .map(PersistedDrawCommand::from)
                .collect(),
            overlay_items: page
                .overlay_items
                .iter()
                .map(PersistedOverlayItem::from)
                .collect(),
            annotations: page
                .annotations
                .iter()
                .map(PersistedPageAnnotation::from)
                .collect(),
            metrics: page.metrics.into(),
        }
    }
}

impl From<PersistedRenderPage> for RenderPage {
    fn from(value: PersistedRenderPage) -> Self {
        Self {
            page_number: value.page_number,
            commands: value.commands.into_iter().map(DrawCommand::from).collect(),
            content_commands: value
                .content_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            chrome_commands: value
                .chrome_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            overlay_commands: value
                .overlay_commands
                .into_iter()
                .map(DrawCommand::from)
                .collect(),
            overlay_items: value
                .overlay_items
                .into_iter()
                .map(OverlayItem::from)
                .collect(),
            annotations: value
                .annotations
                .into_iter()
                .map(PageAnnotation::from)
                .collect(),
            metrics: value.metrics.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedOverlayItem {
    slot: PersistedOverlaySlot,
    z: i32,
    content: PersistedOverlayContent,
}

impl From<&OverlayItem> for PersistedOverlayItem {
    fn from(value: &OverlayItem) -> Self {
        Self {
            slot: (&value.slot).into(),
            z: value.z,
            content: (&value.content).into(),
        }
    }
}

impl From<PersistedOverlayItem> for OverlayItem {
    fn from(value: PersistedOverlayItem) -> Self {
        Self {
            slot: value.slot.into(),
            z: value.z,
            content: value.content.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum PersistedOverlaySlot {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
    Custom(PersistedOverlayRect),
}

impl From<&OverlaySlot> for PersistedOverlaySlot {
    fn from(value: &OverlaySlot) -> Self {
        match value {
            OverlaySlot::TopLeft => Self::TopLeft,
            OverlaySlot::TopCenter => Self::TopCenter,
            OverlaySlot::TopRight => Self::TopRight,
            OverlaySlot::BottomLeft => Self::BottomLeft,
            OverlaySlot::BottomCenter => Self::BottomCenter,
            OverlaySlot::BottomRight => Self::BottomRight,
            OverlaySlot::Custom(rect) => Self::Custom((*rect).into()),
        }
    }
}

impl From<PersistedOverlaySlot> for OverlaySlot {
    fn from(value: PersistedOverlaySlot) -> Self {
        match value {
            PersistedOverlaySlot::TopLeft => Self::TopLeft,
            PersistedOverlaySlot::TopCenter => Self::TopCenter,
            PersistedOverlaySlot::TopRight => Self::TopRight,
            PersistedOverlaySlot::BottomLeft => Self::BottomLeft,
            PersistedOverlaySlot::BottomCenter => Self::BottomCenter,
            PersistedOverlaySlot::BottomRight => Self::BottomRight,
            PersistedOverlaySlot::Custom(rect) => Self::Custom(rect.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedOverlayRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl From<OverlayRect> for PersistedOverlayRect {
    fn from(value: OverlayRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<PersistedOverlayRect> for OverlayRect {
    fn from(value: PersistedOverlayRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum PersistedOverlayContent {
    Text(String),
    Command(PersistedDrawCommand),
}

impl From<&OverlayContent> for PersistedOverlayContent {
    fn from(value: &OverlayContent) -> Self {
        match value {
            OverlayContent::Text(text) => Self::Text(text.clone()),
            OverlayContent::Command(cmd) => Self::Command(cmd.into()),
        }
    }
}

impl From<PersistedOverlayContent> for OverlayContent {
    fn from(value: PersistedOverlayContent) -> Self {
        match value {
            PersistedOverlayContent::Text(text) => Self::Text(text),
            PersistedOverlayContent::Command(cmd) => Self::Command(cmd.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedPageAnnotation {
    kind: String,
    value: Option<String>,
}

impl From<&PageAnnotation> for PersistedPageAnnotation {
    fn from(value: &PageAnnotation) -> Self {
        Self {
            kind: String::from(&value.kind),
            value: value.value.clone(),
        }
    }
}

impl From<PersistedPageAnnotation> for PageAnnotation {
    fn from(value: PersistedPageAnnotation) -> Self {
        Self {
            kind: value.kind.into(),
            value: value.value,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedPageMetrics {
    chapter_index: usize,
    chapter_page_index: usize,
    chapter_page_count: Option<usize>,
    global_page_index: Option<usize>,
    global_page_count_estimate: Option<usize>,
    progress_chapter: f32,
    progress_book: Option<f32>,
}

impl From<PageMetrics> for PersistedPageMetrics {
    fn from(value: PageMetrics) -> Self {
        Self {
            chapter_index: value.chapter_index,
            chapter_page_index: value.chapter_page_index,
            chapter_page_count: value.chapter_page_count,
            global_page_index: value.global_page_index,
            global_page_count_estimate: value.global_page_count_estimate,
            progress_chapter: normalize_progress(value.progress_chapter),
            progress_book: value.progress_book.map(normalize_progress),
        }
    }
}

impl From<PersistedPageMetrics> for PageMetrics {
    fn from(value: PersistedPageMetrics) -> Self {
        Self {
            chapter_index: value.chapter_index,
            chapter_page_index: value.chapter_page_index,
            chapter_page_count: value.chapter_page_count,
            global_page_index: value.global_page_index,
            global_page_count_estimate: value.global_page_count_estimate,
            progress_chapter: normalize_progress(value.progress_chapter),
            progress_book: value.progress_book.map(normalize_progress),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum PersistedDrawCommand {
    Text(PersistedTextCommand),
    Rule(PersistedRuleCommand),
    ImageObject(PersistedImageObjectCommand),
    Rect(PersistedRectCommand),
    PageChrome(PersistedPageChromeCommand),
}

impl From<&DrawCommand> for PersistedDrawCommand {
    fn from(value: &DrawCommand) -> Self {
        match value {
            DrawCommand::Text(cmd) => Self::Text(cmd.into()),
            DrawCommand::Rule(cmd) => Self::Rule((*cmd).into()),
            DrawCommand::ImageObject(cmd) => Self::ImageObject(cmd.into()),
            DrawCommand::Rect(cmd) => Self::Rect((*cmd).into()),
            DrawCommand::PageChrome(cmd) => Self::PageChrome(cmd.into()),
        }
    }
}

impl From<PersistedDrawCommand> for DrawCommand {
    fn from(value: PersistedDrawCommand) -> Self {
        match value {
            PersistedDrawCommand::Text(cmd) => Self::Text(cmd.into()),
            PersistedDrawCommand::Rule(cmd) => Self::Rule(cmd.into()),
            PersistedDrawCommand::ImageObject(cmd) => Self::ImageObject(cmd.into()),
            PersistedDrawCommand::Rect(cmd) => Self::Rect(cmd.into()),
            PersistedDrawCommand::PageChrome(cmd) => Self::PageChrome(cmd.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedTextCommand {
    x: i32,
    baseline_y: i32,
    text: String,
    font_id: Option<u32>,
    style: PersistedResolvedTextStyle,
}

impl From<&TextCommand> for PersistedTextCommand {
    fn from(value: &TextCommand) -> Self {
        Self {
            x: value.x,
            baseline_y: value.baseline_y,
            text: value.text.clone(),
            font_id: value.font_id,
            style: (&value.style).into(),
        }
    }
}

impl From<PersistedTextCommand> for TextCommand {
    fn from(value: PersistedTextCommand) -> Self {
        Self {
            x: value.x,
            baseline_y: value.baseline_y,
            text: value.text,
            font_id: value.font_id,
            style: value.style.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedResolvedTextStyle {
    font_id: Option<u32>,
    family: String,
    weight: u16,
    italic: bool,
    size_px: f32,
    line_height: f32,
    letter_spacing: f32,
    role: PersistedBlockRole,
    justify_mode: PersistedJustifyMode,
}

impl From<&ResolvedTextStyle> for PersistedResolvedTextStyle {
    fn from(value: &ResolvedTextStyle) -> Self {
        Self {
            font_id: value.font_id,
            family: value.family.to_string(),
            weight: value.weight,
            italic: value.italic,
            size_px: value.size_px,
            line_height: value.line_height,
            letter_spacing: value.letter_spacing,
            role: value.role.into(),
            justify_mode: value.justify_mode.into(),
        }
    }
}

impl From<PersistedResolvedTextStyle> for ResolvedTextStyle {
    fn from(value: PersistedResolvedTextStyle) -> Self {
        Self {
            font_id: value.font_id,
            family: value.family.into(),
            weight: value.weight,
            italic: value.italic,
            size_px: value.size_px,
            line_height: value.line_height,
            letter_spacing: value.letter_spacing,
            role: value.role.into(),
            justify_mode: value.justify_mode.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PersistedBlockRole {
    Body,
    Paragraph,
    Heading(u8),
    ListItem,
    FigureCaption,
    Preformatted,
}

impl From<BlockRole> for PersistedBlockRole {
    fn from(value: BlockRole) -> Self {
        match value {
            BlockRole::Body => Self::Body,
            BlockRole::Paragraph => Self::Paragraph,
            BlockRole::Heading(level) => Self::Heading(level),
            BlockRole::ListItem => Self::ListItem,
            BlockRole::FigureCaption => Self::FigureCaption,
            BlockRole::Preformatted => Self::Preformatted,
        }
    }
}

impl From<PersistedBlockRole> for BlockRole {
    fn from(value: PersistedBlockRole) -> Self {
        match value {
            PersistedBlockRole::Body => Self::Body,
            PersistedBlockRole::Paragraph => Self::Paragraph,
            PersistedBlockRole::Heading(level) => Self::Heading(level),
            PersistedBlockRole::ListItem => Self::ListItem,
            PersistedBlockRole::FigureCaption => Self::FigureCaption,
            PersistedBlockRole::Preformatted => Self::Preformatted,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PersistedJustifyMode {
    None,
    InterWord { extra_px_total: i32 },
    AlignRight { offset_px: i32 },
    AlignCenter { offset_px: i32 },
}

impl From<JustifyMode> for PersistedJustifyMode {
    fn from(value: JustifyMode) -> Self {
        match value {
            JustifyMode::None => Self::None,
            JustifyMode::InterWord { extra_px_total } => Self::InterWord { extra_px_total },
            JustifyMode::AlignRight { offset_px } => Self::AlignRight { offset_px },
            JustifyMode::AlignCenter { offset_px } => Self::AlignCenter { offset_px },
        }
    }
}

impl From<PersistedJustifyMode> for JustifyMode {
    fn from(value: PersistedJustifyMode) -> Self {
        match value {
            PersistedJustifyMode::None => Self::None,
            PersistedJustifyMode::InterWord { extra_px_total } => {
                Self::InterWord { extra_px_total }
            }
            PersistedJustifyMode::AlignRight { offset_px } => Self::AlignRight { offset_px },
            PersistedJustifyMode::AlignCenter { offset_px } => Self::AlignCenter { offset_px },
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedRuleCommand {
    x: i32,
    y: i32,
    length: u32,
    thickness: u32,
    horizontal: bool,
}

impl From<RuleCommand> for PersistedRuleCommand {
    fn from(value: RuleCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            length: value.length,
            thickness: value.thickness,
            horizontal: value.horizontal,
        }
    }
}

impl From<PersistedRuleCommand> for RuleCommand {
    fn from(value: PersistedRuleCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            length: value.length,
            thickness: value.thickness,
            horizontal: value.horizontal,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedImageObjectCommand {
    src: String,
    alt: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl From<&ImageObjectCommand> for PersistedImageObjectCommand {
    fn from(value: &ImageObjectCommand) -> Self {
        Self {
            src: value.src.clone(),
            alt: value.alt.clone(),
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<PersistedImageObjectCommand> for ImageObjectCommand {
    fn from(value: PersistedImageObjectCommand) -> Self {
        Self {
            src: value.src,
            alt: value.alt,
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct PersistedRectCommand {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    fill: bool,
}

impl From<RectCommand> for PersistedRectCommand {
    fn from(value: RectCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            fill: value.fill,
        }
    }
}

impl From<PersistedRectCommand> for RectCommand {
    fn from(value: PersistedRectCommand) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
            fill: value.fill,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PersistedPageChromeCommand {
    kind: PersistedPageChromeKind,
    text: Option<String>,
    current: Option<usize>,
    total: Option<usize>,
}

impl From<&PageChromeCommand> for PersistedPageChromeCommand {
    fn from(value: &PageChromeCommand) -> Self {
        Self {
            kind: value.kind.into(),
            text: value.text.clone(),
            current: value.current,
            total: value.total,
        }
    }
}

impl From<PersistedPageChromeCommand> for PageChromeCommand {
    fn from(value: PersistedPageChromeCommand) -> Self {
        Self {
            kind: value.kind.into(),
            text: value.text,
            current: value.current,
            total: value.total,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum PersistedPageChromeKind {
    Header,
    Footer,
    Progress,
}

impl From<PageChromeKind> for PersistedPageChromeKind {
    fn from(value: PageChromeKind) -> Self {
        match value {
            PageChromeKind::Header => Self::Header,
            PageChromeKind::Footer => Self::Footer,
            PageChromeKind::Progress => Self::Progress,
        }
    }
}

impl From<PersistedPageChromeKind> for PageChromeKind {
    fn from(value: PersistedPageChromeKind) -> Self {
        match value {
            PersistedPageChromeKind::Header => Self::Header,
            PersistedPageChromeKind::Footer => Self::Footer,
            PersistedPageChromeKind::Progress => Self::Progress,
        }
    }
}

/// Per-run configuration used by `RenderEngine::begin`.
#[derive(Clone)]
pub struct RenderConfig<'a> {
    page_range: Option<PageRange>,
    cache: Option<&'a dyn RenderCacheStore>,
    cancel: Option<&'a dyn CancelToken>,
    embedded_fonts: bool,
    forced_font_family: Option<String>,
    text_measurer: Option<Arc<dyn TextMeasurer>>,
}

impl<'a> Default for RenderConfig<'a> {
    fn default() -> Self {
        Self {
            page_range: None,
            cache: None,
            cancel: None,
            embedded_fonts: true,
            forced_font_family: None,
            text_measurer: None,
        }
    }
}

impl<'a> RenderConfig<'a> {
    /// Limit emitted pages to the given chapter range `[start, end)`.
    pub fn with_page_range(mut self, range: PageRange) -> Self {
        self.page_range = Some(range);
        self
    }

    /// Use cache hooks for loading/storing chapter pages.
    pub fn with_cache(mut self, cache: &'a dyn RenderCacheStore) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Attach an optional cancellation token for session operations.
    pub fn with_cancel(mut self, cancel: &'a dyn CancelToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Enable or disable embedded-font registration for this render run.
    ///
    /// Disable this in constrained environments to skip EPUB font-face loading
    /// and rely on fallback font policy.
    pub fn with_embedded_fonts(mut self, enabled: bool) -> Self {
        self.embedded_fonts = enabled;
        self
    }

    /// Force a single fallback family for all text shaping/layout.
    ///
    /// This disables embedded font matching to keep measurement/rendering consistent
    /// with the requested family.
    pub fn with_forced_font_family(mut self, family: impl Into<String>) -> Self {
        let family = family.into();
        let trimmed = family.trim();
        if trimmed.is_empty() {
            self.forced_font_family = None;
            return self;
        }
        self.forced_font_family = Some(trimmed.to_string()); // allow: config-time setter
        self.embedded_fonts = false;
        self
    }

    /// Attach a glyph-width measurer used by line layout.
    pub fn with_text_measurer(mut self, measurer: Arc<dyn TextMeasurer>) -> Self {
        self.text_measurer = Some(measurer);
        self
    }
}

/// Render engine for chapter -> page conversion.
#[derive(Clone)]
pub struct RenderEngine {
    opts: RenderEngineOptions,
    layout: LayoutEngine,
    pagination_profile: PaginationProfileId,
    diagnostic_sink: DiagnosticSink,
}

impl fmt::Debug for RenderEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderEngine")
            .field("opts", &self.opts)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl RenderEngine {
    fn compute_pagination_profile_id(opts: RenderEngineOptions) -> PaginationProfileId {
        let payload = format!("{:?}|{:?}", opts.prep, opts.layout); // allow: once per config, hashing input
        PaginationProfileId::from_bytes(payload.as_bytes())
    }

    /// Create a render engine.
    pub fn new(opts: RenderEngineOptions) -> Self {
        let pagination_profile = Self::compute_pagination_profile_id(opts);
        Self {
            layout: LayoutEngine::new(opts.layout),
            opts,
            pagination_profile,
            diagnostic_sink: None,
        }
    }

    /// Register or replace the diagnostics sink.
    pub fn set_diagnostic_sink<F>(&mut self, sink: F)
    where
        F: FnMut(RenderDiagnostic) + Send + 'static,
    {
        self.diagnostic_sink = Some(Arc::new(Mutex::new(Box::new(sink)))); // allow: once, diagnostic setup
    }

    fn emit_diagnostic(&self, diagnostic: RenderDiagnostic) {
        let Some(sink) = &self.diagnostic_sink else {
            return;
        };
        if let Ok(mut sink) = sink.lock() {
            sink(diagnostic);
        }
    }

    /// Stable fingerprint for all layout-affecting settings.
    pub fn pagination_profile_id(&self) -> PaginationProfileId {
        self.pagination_profile
    }

    /// Begin a chapter layout session for embedded/incremental integrations.
    pub fn begin<'a>(
        &'a self,
        chapter_index: usize,
        config: RenderConfig<'a>,
    ) -> LayoutSession<'a> {
        let profile = self.pagination_profile_id();
        let text_measurer = config.text_measurer.clone();
        let mut pending = VecDeque::new();
        let mut cached_hit = false;
        if let Some(cache) = config.cache {
            if let Some(pages) = cache.load_chapter_pages(profile, chapter_index) {
                cached_hit = true;
                self.emit_diagnostic(RenderDiagnostic::CacheHit {
                    chapter_index,
                    page_count: pages.len(),
                });
                let range = normalize_page_range(config.page_range.clone());
                let total_pages = pages.len();
                for (idx, mut page) in pages.into_iter().enumerate() {
                    Self::annotate_page_for_chapter(&mut page, chapter_index);
                    Self::annotate_page_metrics(&mut page, total_pages);
                    if page_in_range(idx, &range) {
                        pending.push_back(page);
                    }
                }
            }
        }
        if config.cache.is_some() && !cached_hit {
            self.emit_diagnostic(RenderDiagnostic::CacheMiss { chapter_index });
        }
        let inner = if cached_hit {
            None
        } else {
            let mut session = self.layout.start_session_with_text_measurer(text_measurer);
            if let Some(family) = config.forced_font_family.as_deref() {
                session.set_override_family(Arc::from(family));
            }
            Some(session)
        };
        LayoutSession {
            engine: self,
            chapter_index,
            profile,
            cfg: config,
            inner,
            pending_pages: pending,
            rendered_pages: Vec::with_capacity(8),
            page_index: 0,
            completed: cached_hit,
        }
    }

    fn annotate_page_for_chapter(page: &mut RenderPage, chapter_index: usize) {
        page.metrics.chapter_index = chapter_index;
        page.metrics.chapter_page_index = page.page_number.saturating_sub(1);
    }

    fn annotate_page_metrics(page: &mut RenderPage, chapter_page_count: usize) {
        let chapter_page_count = chapter_page_count.max(1);
        page.metrics.chapter_page_count = Some(chapter_page_count);
        page.metrics.global_page_index = Some(page.metrics.chapter_page_index);
        page.metrics.global_page_count_estimate = Some(chapter_page_count);
        page.metrics.progress_chapter = if chapter_page_count <= 1 {
            1.0
        } else {
            page.metrics.chapter_page_index as f32 / (chapter_page_count - 1) as f32
        }
        .clamp(0.0, 1.0);
        page.metrics.progress_book = Some(page.metrics.progress_chapter);
    }

    /// Prepare and layout a chapter into render pages.
    pub fn prepare_chapter<R: std::io::Read + std::io::Seek>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
    ) -> Result<Vec<RenderPage>, RenderEngineError> {
        self.prepare_chapter_with_config_collect(book, chapter_index, RenderConfig::default())
    }

    /// Prepare and layout a chapter into render pages with explicit run config.
    pub fn prepare_chapter_with_config_collect<R: std::io::Read + std::io::Seek>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        config: RenderConfig<'_>,
    ) -> Result<Vec<RenderPage>, RenderEngineError> {
        let page_limit = self.opts.prep.memory.max_pages_in_memory;
        let mut pages = Vec::with_capacity(page_limit.min(8));
        let mut dropped_pages = 0usize;
        let summary = self.prepare_chapter_with_config(book, chapter_index, config, |page| {
            if pages.len() < page_limit {
                pages.push(page);
            } else {
                dropped_pages = dropped_pages.saturating_add(1);
            }
        })?;
        // Post-hoc: set total count now that layout is complete.
        for page in pages.iter_mut() {
            if page.metrics.chapter_page_count.is_none() {
                Self::annotate_page_metrics(page, summary.page_count);
            }
        }
        if dropped_pages > 0 {
            self.emit_diagnostic(RenderDiagnostic::MemoryLimitExceeded {
                kind: "max_pages_in_memory",
                actual: pages.len().saturating_add(dropped_pages),
                limit: page_limit,
            });
            return Err(RenderEngineError::LimitExceeded {
                kind: "max_pages_in_memory",
                actual: pages.len().saturating_add(dropped_pages),
                limit: page_limit,
            });
        }
        Ok(pages)
    }

    /// Prepare and layout a chapter and stream each page.
    pub fn prepare_chapter_with<R, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        F: FnMut(RenderPage),
    {
        self.prepare_chapter_with_config(book, chapter_index, RenderConfig::default(), on_page)
    }

    /// Prepare and layout a chapter with explicit config and stream each page.
    pub fn prepare_chapter_with_config<R, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        config: RenderConfig<'_>,
        mut on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        F: FnMut(RenderPage),
    {
        let cancel = config.cancel.unwrap_or(&NeverCancel);
        self.prepare_chapter_with_cancel_and_config(book, chapter_index, cancel, config, |page| {
            on_page(page)
        })
    }

    /// Prepare and layout caller-provided chapter bytes and stream each page.
    ///
    /// This path avoids internal chapter-byte allocation and is intended for
    /// embedded call sites that keep a reusable chapter buffer.
    pub fn prepare_chapter_bytes_with<R, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        html: &[u8],
        on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        F: FnMut(RenderPage),
    {
        self.prepare_chapter_bytes_with_config(
            book,
            chapter_index,
            html,
            RenderConfig::default(),
            on_page,
        )
    }

    /// Prepare and layout caller-provided chapter bytes with explicit config.
    pub fn prepare_chapter_bytes_with_config<R, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        html: &[u8],
        config: RenderConfig<'_>,
        on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        F: FnMut(RenderPage),
    {
        let cancel = config.cancel.unwrap_or(&NeverCancel);
        self.prepare_chapter_bytes_with_cancel_and_config(
            book,
            chapter_index,
            html,
            cancel,
            config,
            on_page,
        )
    }

    /// Prepare and layout a chapter while honoring cancellation.
    pub fn prepare_chapter_with_cancel<R, C, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        cancel: &C,
        on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        C: CancelToken,
        F: FnMut(RenderPage),
    {
        let config = RenderConfig::default().with_cancel(cancel);
        self.prepare_chapter_with_cancel_and_config(book, chapter_index, cancel, config, on_page)
    }

    fn prepare_chapter_with_cancel_and_config<R, C, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        cancel: &C,
        config: RenderConfig<'_>,
        mut on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        C: CancelToken + ?Sized,
        F: FnMut(RenderPage),
    {
        let embedded_fonts = config.embedded_fonts;
        let forced_font_family = config.forced_font_family.clone();
        let defer_emit_until_finish = config.page_range.is_some();
        let started = Instant::now();
        if cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        let mut session = self.begin(chapter_index, config);
        session.set_hyphenation_language(book.language());
        let mut page_count = 0usize;
        if session.is_complete() {
            session.drain_pages(|mut page| {
                Self::annotate_page_for_chapter(&mut page, chapter_index);
                page_count += 1;
                on_page(page);
            });
            return Ok(ChapterLayoutSummary { page_count });
        }
        let mut prep = if let Some(family) = forced_font_family.as_deref() {
            RenderPrep::new(self.opts.prep).with_font_policy(forced_font_policy(family))
        } else {
            RenderPrep::new(self.opts.prep).with_serif_default()
        };
        if embedded_fonts {
            prep = prep.with_embedded_fonts_from_book(book)?;
        }
        let mut saw_cancelled = false;
        prep.prepare_chapter_with(book, chapter_index, |item| {
            if saw_cancelled || cancel.is_cancelled() {
                saw_cancelled = true;
                return;
            }
            if session.push(item).is_err() {
                saw_cancelled = true;
                return;
            }
            if !defer_emit_until_finish {
                session.drain_pages(|mut page| {
                    Self::annotate_page_for_chapter(&mut page, chapter_index);
                    page_count += 1;
                    on_page(page);
                });
            }
        })?;
        if saw_cancelled || cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        session.finish()?;
        session.drain_pages(|mut page| {
            Self::annotate_page_for_chapter(&mut page, chapter_index);
            page_count += 1;
            on_page(page);
        });
        let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.emit_diagnostic(RenderDiagnostic::ReflowTimeMs(elapsed));
        Ok(ChapterLayoutSummary { page_count })
    }

    fn prepare_chapter_bytes_with_cancel_and_config<R, C, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        html: &[u8],
        cancel: &C,
        config: RenderConfig<'_>,
        mut on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        C: CancelToken + ?Sized,
        F: FnMut(RenderPage),
    {
        let embedded_fonts = config.embedded_fonts;
        let forced_font_family = config.forced_font_family.clone();
        let defer_emit_until_finish = config.page_range.is_some();
        let started = Instant::now();
        if cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        let mut session = self.begin(chapter_index, config);
        session.set_hyphenation_language(book.language());
        let mut page_count = 0usize;
        if session.is_complete() {
            session.drain_pages(|mut page| {
                Self::annotate_page_for_chapter(&mut page, chapter_index);
                page_count += 1;
                on_page(page);
            });
            return Ok(ChapterLayoutSummary { page_count });
        }
        let mut prep = if let Some(family) = forced_font_family.as_deref() {
            RenderPrep::new(self.opts.prep).with_font_policy(forced_font_policy(family))
        } else {
            RenderPrep::new(self.opts.prep).with_serif_default()
        };
        if embedded_fonts {
            prep = prep.with_embedded_fonts_from_book(book)?;
        }
        let mut saw_cancelled = false;
        prep.prepare_chapter_bytes_with(book, chapter_index, html, |item| {
            if saw_cancelled || cancel.is_cancelled() {
                saw_cancelled = true;
                return;
            }
            if session.push(item).is_err() {
                saw_cancelled = true;
                return;
            }
            if !defer_emit_until_finish {
                session.drain_pages(|mut page| {
                    Self::annotate_page_for_chapter(&mut page, chapter_index);
                    page_count += 1;
                    on_page(page);
                });
            }
        })?;
        if saw_cancelled || cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        session.finish()?;
        session.drain_pages(|mut page| {
            Self::annotate_page_for_chapter(&mut page, chapter_index);
            page_count += 1;
            on_page(page);
        });
        let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.emit_diagnostic(RenderDiagnostic::ReflowTimeMs(elapsed));
        Ok(ChapterLayoutSummary { page_count })
    }

    /// Prepare and layout a chapter, returning pages within `[start, end)`.
    ///
    /// Range indices are zero-based over the emitted chapter page sequence.
    /// Returned `RenderPage::page_number` values remain 1-based chapter page numbers.
    pub fn prepare_chapter_page_range<R: std::io::Read + std::io::Seek>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        start: usize,
        end: usize,
    ) -> Result<Vec<RenderPage>, RenderEngineError> {
        self.page_range(book, chapter_index, start..end)
    }

    /// Alias for chapter page range rendering.
    pub fn page_range<R: std::io::Read + std::io::Seek>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        range: PageRange,
    ) -> Result<Vec<RenderPage>, RenderEngineError> {
        if range.start >= range.end {
            return Ok(Vec::with_capacity(8));
        }
        self.prepare_chapter_with_config_collect(
            book,
            chapter_index,
            RenderConfig::default().with_page_range(range),
        )
    }

    /// Prepare and layout a chapter and return pages as an iterator.
    ///
    /// This iterator is eager: pages are prepared first, then iterated.
    pub fn prepare_chapter_iter<R: std::io::Read + std::io::Seek>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
    ) -> Result<RenderPageIter, RenderEngineError> {
        let pages = self.prepare_chapter(book, chapter_index)?;
        Ok(RenderPageIter {
            inner: pages.into_iter(),
        })
    }

    /// Prepare and layout a chapter as a streaming iterator.
    ///
    /// Unlike `prepare_chapter_iter`, this method streams pages incrementally from a
    /// worker thread using a bounded channel (`capacity=1`) for backpressure.
    /// It requires ownership of the book so the worker can read resources directly.
    pub fn prepare_chapter_iter_streaming<R>(
        &self,
        mut book: EpubBook<R>,
        chapter_index: usize,
    ) -> RenderPageStreamIter
    where
        R: std::io::Read + std::io::Seek + Send + 'static,
    {
        let (tx, rx) = sync_channel(1);
        let engine = self.clone();

        std::thread::spawn(move || {
            let mut receiver_closed = false;
            let result = engine.prepare_chapter_with(&mut book, chapter_index, |page| {
                if receiver_closed {
                    return;
                }
                if tx.send(StreamMessage::Page(page)).is_err() {
                    receiver_closed = true;
                }
            });

            if receiver_closed {
                return;
            }
            match result {
                Ok(_summary) => {
                    let _ = tx.send(StreamMessage::Done);
                }
                Err(err) => {
                    let _ = tx.send(StreamMessage::Error(err));
                }
            }
        });

        RenderPageStreamIter {
            rx,
            finished: false,
        }
    }

    /// Prepare with an overlay composer that maps page metrics into overlay items.
    pub fn prepare_chapter_with_overlay_composer<R, O, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        viewport: OverlaySize,
        composer: &O,
        mut on_page: F,
    ) -> Result<ChapterLayoutSummary, RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        O: crate::render_ir::OverlayComposer,
        F: FnMut(RenderPage),
    {
        self.prepare_chapter_with(book, chapter_index, |mut page| {
            let overlays = composer.compose(&page.metrics, viewport);
            for item in overlays {
                page.overlay_items.push(item.clone());
                if let OverlayContent::Command(cmd) = item.content {
                    page.push_overlay_command(cmd);
                }
            }
            on_page(page);
        })
    }
}

/// Incremental wrapper session returned by `RenderEngine::begin`.
pub struct LayoutSession<'a> {
    engine: &'a RenderEngine,
    chapter_index: usize,
    profile: PaginationProfileId,
    cfg: RenderConfig<'a>,
    inner: Option<CoreLayoutSession>,
    pending_pages: VecDeque<RenderPage>,
    rendered_pages: Vec<RenderPage>,
    page_index: usize,
    completed: bool,
}

impl LayoutSession<'_> {
    /// Set the hyphenation language hint (e.g. "en", "en-US").
    pub fn set_hyphenation_language(&mut self, language_tag: &str) {
        if let Some(inner) = self.inner.as_mut() {
            inner.set_hyphenation_language(language_tag);
        }
    }

    /// Push one styled item through layout and enqueue closed pages.
    pub fn push(&mut self, item: StyledEventOrRun) -> Result<(), RenderEngineError> {
        if self.completed {
            return Ok(());
        }
        if self.cfg.cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            self.engine.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        if let Some(inner) = self.inner.as_mut() {
            let chapter = self.chapter_index;
            let range = normalize_page_range(self.cfg.page_range.clone());
            let rendered = &mut self.rendered_pages;
            let pending = &mut self.pending_pages;
            let page_index = &mut self.page_index;
            let capture_for_cache = self.cfg.cache.is_some();
            inner.push_item_with_pages(item, &mut |mut page| {
                RenderEngine::annotate_page_for_chapter(&mut page, chapter);
                if capture_for_cache {
                    rendered.push(page.clone());
                }
                if page_in_range(*page_index, &range) {
                    pending.push_back(page);
                }
                *page_index += 1;
            });
        }
        Ok(())
    }

    /// Drain currently available pages in FIFO order.
    pub fn drain_pages<F>(&mut self, mut on_page: F)
    where
        F: FnMut(RenderPage),
    {
        while let Some(page) = self.pending_pages.pop_front() {
            on_page(page);
        }
    }

    /// Finish layout and enqueue any remaining pages.
    pub fn finish(&mut self) -> Result<(), RenderEngineError> {
        if self.completed {
            return Ok(());
        }
        if self.cfg.cancel.is_some_and(|cancel| cancel.is_cancelled()) {
            self.engine.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        if let Some(inner) = self.inner.as_mut() {
            let chapter = self.chapter_index;
            let range = normalize_page_range(self.cfg.page_range.clone());
            let rendered = &mut self.rendered_pages;
            let pending = &mut self.pending_pages;
            let page_index = &mut self.page_index;
            let capture_for_cache = self.cfg.cache.is_some();
            inner.finish(&mut |mut page| {
                RenderEngine::annotate_page_for_chapter(&mut page, chapter);
                if capture_for_cache {
                    rendered.push(page.clone());
                }
                if page_in_range(*page_index, &range) {
                    pending.push_back(page);
                }
                *page_index += 1;
            });
        }
        let chapter_total = self.page_index.max(1);
        for page in self.pending_pages.iter_mut() {
            RenderEngine::annotate_page_metrics(page, chapter_total);
        }
        for page in self.rendered_pages.iter_mut() {
            RenderEngine::annotate_page_metrics(page, chapter_total);
        }
        if let Some(cache) = self.cfg.cache {
            if !self.rendered_pages.is_empty() {
                cache.store_chapter_pages(self.profile, self.chapter_index, &self.rendered_pages);
            }
        }
        self.completed = true;
        Ok(())
    }

    fn is_complete(&self) -> bool {
        self.completed
    }
}

fn normalize_page_range(range: Option<PageRange>) -> Option<PageRange> {
    match range {
        Some(r) if r.start < r.end => Some(r),
        Some(_) => Some(0..0),
        None => None,
    }
}

fn page_in_range(idx: usize, range: &Option<PageRange>) -> bool {
    range.as_ref().map(|r| r.contains(&idx)).unwrap_or(true)
}

fn forced_font_policy(family: &str) -> FontPolicy {
    let mut policy = FontPolicy::serif_default();
    let normalized = family
        .split(',')
        .next()
        .map(str::trim)
        .map(|part| part.trim_matches('"').trim_matches('\''))
        .filter(|part| !part.is_empty())
        .unwrap_or("serif");
    policy.preferred_families = vec![normalized.to_string()]; // allow: config-time builder
    policy.default_family = normalized.to_string(); // allow: config-time builder
    policy.allow_embedded_fonts = false;
    policy
}

/// Stable page iterator wrapper returned by `RenderEngine::prepare_chapter_iter`.
#[derive(Debug)]
pub struct RenderPageIter {
    inner: std::vec::IntoIter<RenderPage>,
}

impl Iterator for RenderPageIter {
    type Item = RenderPage;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for RenderPageIter {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl std::iter::FusedIterator for RenderPageIter {}

enum StreamMessage {
    Page(RenderPage),
    Error(RenderEngineError),
    Done,
}

/// Streaming page iterator produced by `RenderEngine::prepare_chapter_iter_streaming`.
#[derive(Debug)]
pub struct RenderPageStreamIter {
    rx: Receiver<StreamMessage>,
    finished: bool,
}

impl Iterator for RenderPageStreamIter {
    type Item = Result<RenderPage, RenderEngineError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.rx.recv() {
            Ok(StreamMessage::Page(page)) => Some(Ok(page)),
            Ok(StreamMessage::Error(err)) => {
                self.finished = true;
                Some(Err(err))
            }
            Ok(StreamMessage::Done) | Err(_) => {
                self.finished = true;
                None
            }
        }
    }
}

/// Render engine error.
#[derive(Debug)]
pub enum RenderEngineError {
    /// Render prep failed.
    Prep(RenderPrepError),
    /// Layout run was cancelled.
    Cancelled,
    /// Render page collection exceeded configured memory limits.
    LimitExceeded {
        kind: &'static str,
        actual: usize,
        limit: usize,
    },
}

impl core::fmt::Display for RenderEngineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Prep(err) => write!(f, "render prep failed: {}", err),
            Self::Cancelled => write!(f, "render cancelled"),
            Self::LimitExceeded {
                kind,
                actual,
                limit,
            } => write!(
                f,
                "render memory limit exceeded: {} (actual={} limit={})",
                kind, actual, limit
            ),
        }
    }
}

impl std::error::Error for RenderEngineError {}

impl From<RenderPrepError> for RenderEngineError {
    fn from(value: RenderPrepError) -> Self {
        Self::Prep(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_ir::{
        DrawCommand, ImageObjectCommand, JustifyMode, OverlayItem, OverlayRect, OverlaySlot,
        PageAnnotation, PageAnnotationKind, PageChromeCommand, PageChromeKind, RectCommand,
        ResolvedTextStyle, RuleCommand, TextCommand,
    };
    use epub_stream::{BlockRole, ChapterRef, ComputedTextStyle, StyledEvent, StyledRun};
    use std::fs;
    use std::path::PathBuf;

    fn body_run(text: &str) -> StyledEventOrRun {
        StyledEventOrRun::Run(StyledRun {
            text: text.to_string(),
            style: ComputedTextStyle {
                family_stack: vec!["serif".to_string()].into(),
                weight: 400,
                italic: false,
                size_px: 16.0,
                line_height: 1.4,
                letter_spacing: 0.0,
                block_role: BlockRole::Body,
            },
            font_id: 0,
        })
    }

    fn cache_fixture_page(page_number: usize, chapter_page_count: usize) -> RenderPage {
        let mut page = RenderPage::new(page_number);
        page.push_content_text_command(TextCommand {
            x: 12,
            baseline_y: 24,
            text: format!("page-{page_number}"),
            font_id: Some(3),
            style: ResolvedTextStyle {
                font_id: Some(3),
                family: "serif".into(),
                weight: 400,
                italic: false,
                size_px: 16.0,
                line_height: 1.4,
                letter_spacing: 0.0,
                role: BlockRole::Body,
                justify_mode: JustifyMode::InterWord { extra_px_total: 6 },
            },
        });
        page.push_content_rule_command(RuleCommand {
            x: 10,
            y: 28,
            length: 100,
            thickness: 1,
            horizontal: true,
        });
        page.push_content_image_object_command(ImageObjectCommand {
            src: "images/pic.png".to_string(),
            alt: "diagram".to_string(),
            x: 10,
            y: 40,
            width: 64,
            height: 48,
        });
        page.push_chrome_command(DrawCommand::PageChrome(PageChromeCommand {
            kind: PageChromeKind::Footer,
            text: Some(format!("{page_number}/{chapter_page_count}")),
            current: Some(page_number),
            total: Some(chapter_page_count),
        }));
        page.push_overlay_command(DrawCommand::Rect(RectCommand {
            x: 6,
            y: 6,
            width: 18,
            height: 8,
            fill: true,
        }));
        page.overlay_items.push(OverlayItem {
            slot: OverlaySlot::TopRight,
            z: 1,
            content: OverlayContent::Text("bookmark".to_string()),
        });
        page.overlay_items.push(OverlayItem {
            slot: OverlaySlot::Custom(OverlayRect {
                x: 4,
                y: 5,
                width: 10,
                height: 11,
            }),
            z: 2,
            content: OverlayContent::Command(DrawCommand::Rule(RuleCommand {
                x: 2,
                y: 3,
                length: 12,
                thickness: 1,
                horizontal: false,
            })),
        });
        page.annotations.push(PageAnnotation {
            kind: PageAnnotationKind::Note,
            value: Some(format!("a{page_number}")),
        });
        page.metrics.chapter_index = 4;
        page.metrics.chapter_page_index = page_number.saturating_sub(1);
        page.metrics.chapter_page_count = Some(chapter_page_count);
        page.metrics.global_page_index = Some(page_number.saturating_sub(1));
        page.metrics.global_page_count_estimate = Some(chapter_page_count);
        page.metrics.progress_chapter = if chapter_page_count <= 1 {
            1.0
        } else {
            page.metrics.chapter_page_index as f32 / (chapter_page_count - 1) as f32
        };
        page.metrics.progress_book = Some(page.metrics.progress_chapter);
        page.sync_commands();
        page
    }

    fn progress_pages(count: usize) -> Vec<RenderPage> {
        let mut pages = Vec::with_capacity(count);
        for idx in 0..count {
            let mut page = RenderPage::new(idx + 1);
            page.metrics.chapter_page_index = idx;
            page.metrics.chapter_page_count = Some(count);
            page.metrics.progress_chapter = if count <= 1 {
                1.0
            } else {
                idx as f32 / (count - 1) as f32
            };
            page.metrics.progress_book = Some(page.metrics.progress_chapter);
            pages.push(page);
        }
        pages
    }

    fn temp_cache_root(label: &str) -> PathBuf {
        let nonce = CACHE_WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "epub-stream-render-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn sample_chapters() -> Vec<ChapterRef> {
        vec![
            ChapterRef {
                index: 0,
                idref: "c0".to_string(),
                href: "text/ch0.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
            },
            ChapterRef {
                index: 1,
                idref: "c1".to_string(),
                href: "text/ch1.xhtml".to_string(),
                media_type: "application/xhtml+xml".to_string(),
            },
        ]
    }

    #[test]
    fn begin_push_and_drain_pages_streams_incrementally() {
        let mut opts = RenderEngineOptions::for_display(300, 120);
        opts.layout.margin_top = 8;
        opts.layout.margin_bottom = 8;
        let engine = RenderEngine::new(opts);

        let mut items = Vec::new();
        for _ in 0..40 {
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
            items.push(body_run("one two three four five six seven eight nine ten"));
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
        }

        let mut session = engine.begin(3, RenderConfig::default());
        let mut streamed = Vec::new();
        for item in &items {
            session.push(item.clone()).expect("push should pass");
            session.drain_pages(|page| streamed.push(page));
        }
        session.finish().expect("finish should pass");
        session.drain_pages(|page| streamed.push(page));

        let mut expected = engine.layout.layout_items(items);
        for page in &mut expected {
            page.metrics.chapter_index = 3;
        }
        assert_eq!(streamed, expected);
        assert!(streamed.iter().all(|page| page.metrics.chapter_index == 3));
    }

    #[test]
    fn pagination_profile_id_matches_options_payload_and_clone() {
        let mut opts = RenderEngineOptions::for_display(300, 120);
        opts.layout.margin_top = 8;
        opts.layout.margin_bottom = 8;
        let expected_payload = format!("{:?}|{:?}", opts.prep, opts.layout);
        let expected = PaginationProfileId::from_bytes(expected_payload.as_bytes());

        let engine = RenderEngine::new(opts);
        assert_eq!(engine.pagination_profile_id(), expected);
        assert_eq!(engine.clone().pagination_profile_id(), expected);
    }

    #[test]
    fn forced_font_family_config_disables_embedded_fonts() {
        let cfg = RenderConfig::default().with_forced_font_family("  monospace ");
        assert_eq!(cfg.forced_font_family.as_deref(), Some("monospace"));
        assert!(!cfg.embedded_fonts);
    }

    #[test]
    fn empty_forced_font_family_is_ignored() {
        let cfg = RenderConfig::default().with_forced_font_family("   ");
        assert!(cfg.forced_font_family.is_none());
        assert!(cfg.embedded_fonts);
    }

    #[test]
    fn forced_font_policy_uses_first_family_entry() {
        let policy = forced_font_policy("Alegreya, serif");
        assert_eq!(policy.default_family, "Alegreya");
        assert_eq!(policy.preferred_families, vec!["Alegreya".to_string()]);
        assert!(!policy.allow_embedded_fonts);
    }

    #[test]
    fn cache_roundtrip_load_store() {
        let root = temp_cache_root("cache-roundtrip");
        let store = FileRenderCacheStore::new(&root).with_max_file_bytes(256 * 1024);
        let profile = PaginationProfileId::from_bytes(b"profile-a");
        let chapter_index = 9;
        let pages = vec![cache_fixture_page(1, 2), cache_fixture_page(2, 2)];

        store.store_chapter_pages(profile, chapter_index, &pages);
        let cache_path = store.chapter_cache_path(profile, chapter_index);
        assert!(cache_path.exists());
        let cache_json = fs::read_to_string(&cache_path).expect("cache file should be readable");
        let cache_payload: serde_json::Value =
            serde_json::from_str(&cache_json).expect("cache JSON should parse");
        assert_eq!(
            cache_payload["pages"][0]["content_commands"][0]["Text"]["style"]["family"].as_str(),
            Some("serif")
        );

        let loaded = store.load_chapter_pages(profile, chapter_index);
        assert_eq!(loaded, Some(pages.clone()));

        let tiny_cap = FileRenderCacheStore::new(&root).with_max_file_bytes(48);
        tiny_cap.store_chapter_pages(profile, chapter_index + 1, &pages);
        assert!(tiny_cap
            .load_chapter_pages(profile, chapter_index + 1)
            .is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_omits_legacy_commands_when_unsynced() {
        let root = temp_cache_root("cache-unsynced");
        let store = FileRenderCacheStore::new(&root).with_max_file_bytes(256 * 1024);
        let profile = PaginationProfileId::from_bytes(b"profile-unsynced");
        let chapter_index = 0;

        let mut page = RenderPage::new(1);
        page.push_content_rule_command(RuleCommand {
            x: 0,
            y: 0,
            length: 8,
            thickness: 1,
            horizontal: true,
        });
        assert!(page.commands.is_empty());

        store.store_chapter_pages(profile, chapter_index, &[page.clone()]);
        let cache_path = store.chapter_cache_path(profile, chapter_index);
        let cache_json = fs::read_to_string(&cache_path).expect("cache file should be readable");
        let cache_payload: serde_json::Value =
            serde_json::from_str(&cache_json).expect("cache JSON should parse");
        assert!(cache_payload["pages"][0].get("commands").is_none());

        let loaded = store
            .load_chapter_pages(profile, chapter_index)
            .expect("cache load should succeed");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].commands.len(), 0);
        assert_eq!(loaded[0].merged_commands_len(), 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_deserialization_accepts_legacy_commands_field() {
        let payload = serde_json::json!({
            "version": CACHE_SCHEMA_VERSION,
            "pages": [{
                "page_number": 1,
                "commands": [{
                    "Rule": {
                        "x": 0,
                        "y": 0,
                        "length": 9,
                        "thickness": 1,
                        "horizontal": true
                    }
                }],
                "content_commands": [],
                "chrome_commands": [],
                "overlay_commands": [],
                "overlay_items": [],
                "annotations": [],
                "metrics": {
                    "chapter_index": 0,
                    "chapter_page_index": 0,
                    "chapter_page_count": 1,
                    "global_page_index": 0,
                    "global_page_count_estimate": 1,
                    "progress_chapter": 0.0,
                    "progress_book": 0.0
                }
            }]
        });
        let envelope: PersistedCacheEnvelope =
            serde_json::from_value(payload).expect("legacy payload should parse");
        let pages = envelope
            .into_render_pages()
            .expect("schema version should match");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].commands.len(), 1);
        assert_eq!(pages[0].merged_commands_len(), 1);
    }

    #[test]
    fn persisted_page_annotation_kind_preserves_string_compatibility() {
        let persisted_note = PersistedPageAnnotation {
            kind: "note".to_string(),
            value: Some("n1".to_string()),
        };
        let note: PageAnnotation = persisted_note.clone().into();
        assert_eq!(note.kind, PageAnnotationKind::Note);
        assert_eq!(PersistedPageAnnotation::from(&note).kind, "note");

        let persisted_inline = PersistedPageAnnotation {
            kind: "inline_image_src".to_string(),
            value: Some("images/pic.jpg".to_string()),
        };
        let inline: PageAnnotation = persisted_inline.clone().into();
        assert_eq!(inline.kind, PageAnnotationKind::InlineImageSrc);
        assert_eq!(
            PersistedPageAnnotation::from(&inline).kind,
            "inline_image_src"
        );

        let legacy_unknown_json = r#"{"kind":"legacy_annotation","value":"payload"}"#;
        let persisted_unknown: PersistedPageAnnotation =
            serde_json::from_str(legacy_unknown_json).expect("legacy JSON should deserialize");
        let unknown: PageAnnotation = persisted_unknown.into();
        assert_eq!(
            unknown.kind,
            PageAnnotationKind::Unknown("legacy_annotation".to_string())
        );
        assert_eq!(
            PersistedPageAnnotation::from(&unknown).kind,
            "legacy_annotation"
        );
    }

    #[test]
    fn remap_helpers_monotonicity_and_bounds() {
        let old_pages = progress_pages(7);
        let new_pages = progress_pages(11);

        let mut prev = 0usize;
        for old_idx in 0..old_pages.len() {
            let mapped = remap_page_index_by_chapter_progress(&old_pages, old_idx, &new_pages)
                .expect("new pages should resolve");
            assert!(mapped < new_pages.len());
            assert!(mapped >= prev);
            prev = mapped;
        }

        assert_eq!(
            remap_page_index_by_chapter_progress(&old_pages, usize::MAX, &new_pages),
            Some(new_pages.len() - 1)
        );

        let mut prev_resolved = 0usize;
        for step in 0..=50 {
            let progress = step as f32 / 50.0;
            let mapped = resolve_page_index_for_chapter_progress(progress, &new_pages)
                .expect("new pages should resolve");
            assert!(mapped < new_pages.len());
            assert!(mapped >= prev_resolved);
            prev_resolved = mapped;
        }

        assert_eq!(
            resolve_page_index_for_chapter_progress(-10.0, &new_pages),
            Some(0)
        );
        assert_eq!(
            resolve_page_index_for_chapter_progress(10.0, &new_pages),
            Some(new_pages.len() - 1)
        );
        assert_eq!(
            resolve_page_index_for_chapter_progress(f32::NAN, &new_pages),
            Some(0)
        );
        assert_eq!(resolve_page_index_for_chapter_progress(0.5, &[]), None);
    }

    #[test]
    fn resolve_href_with_fragment_progress_maps_anchor_inside_chapter() {
        let map = RenderBookPageMap::from_chapter_page_counts(&sample_chapters(), &[3, 5]);
        let target = map
            .resolve_href_with_fragment_progress("text/ch1.xhtml#intro", Some(0.5))
            .expect("target should resolve");
        assert_eq!(target.chapter_index, 1);
        assert_eq!(target.page_index, 5);
        assert_eq!(target.kind, RenderLocatorTargetKind::FragmentAnchor);
        assert_eq!(target.fragment.as_deref(), Some("intro"));

        let fallback = map
            .resolve_href("text/ch1.xhtml#intro")
            .expect("fallback should resolve");
        assert_eq!(fallback.page_index, 3);
        assert_eq!(
            fallback.kind,
            RenderLocatorTargetKind::FragmentFallbackChapterStart
        );
    }

    #[test]
    fn estimate_fragment_progress_in_html_matches_id_and_name_patterns() {
        let html = br#"
            <html><body>
                <p>intro text</p>
                <h2 id="middle">Middle</h2>
                <a name='end-anchor'>End</a>
            </body></html>
        "#;
        let middle = estimate_fragment_progress_in_html(html, "middle")
            .expect("middle anchor should resolve");
        let end = estimate_fragment_progress_in_html(html, "end-anchor")
            .expect("end anchor should resolve");
        assert!(middle > 0.0);
        assert!(end > middle);
        assert!(estimate_fragment_progress_in_html(html, "missing").is_none());
        assert!(estimate_fragment_progress_in_html(&[], "middle").is_none());
        assert!(estimate_fragment_progress_in_html(html, "").is_none());
    }
}
