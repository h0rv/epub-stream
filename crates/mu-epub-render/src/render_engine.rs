use mu_epub::{EpubBook, RenderPrep, RenderPrepError, RenderPrepOptions, StyledEventOrRun};
use std::collections::VecDeque;
use std::fmt;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::render_ir::{OverlayContent, OverlaySize, PaginationProfileId, RenderPage};
use crate::render_layout::{
    LayoutConfig, LayoutEngine, LayoutSession as CoreLayoutSession, TextMeasurer,
};

/// Cancellation hook for long-running layout operations.
pub trait CancelToken {
    fn is_cancelled(&self) -> bool;
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

/// Per-run configuration used by `RenderEngine::begin`.
#[derive(Clone)]
pub struct RenderConfig<'a> {
    page_range: Option<PageRange>,
    cache: Option<&'a dyn RenderCacheStore>,
    cancel: Option<&'a dyn CancelToken>,
    embedded_fonts: bool,
    text_measurer: Option<Arc<dyn TextMeasurer>>,
}

impl<'a> Default for RenderConfig<'a> {
    fn default() -> Self {
        Self {
            page_range: None,
            cache: None,
            cancel: None,
            embedded_fonts: true,
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
    /// Create a render engine.
    pub fn new(opts: RenderEngineOptions) -> Self {
        Self {
            layout: LayoutEngine::new(opts.layout),
            opts,
            diagnostic_sink: None,
        }
    }

    /// Register or replace the diagnostics sink.
    pub fn set_diagnostic_sink<F>(&mut self, sink: F)
    where
        F: FnMut(RenderDiagnostic) + Send + 'static,
    {
        self.diagnostic_sink = Some(Arc::new(Mutex::new(Box::new(sink))));
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
        let payload = format!("{:?}|{:?}", self.opts.prep, self.opts.layout);
        PaginationProfileId::from_bytes(payload.as_bytes())
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
        LayoutSession {
            engine: self,
            chapter_index,
            profile,
            cfg: config,
            inner: if cached_hit {
                None
            } else {
                Some(self.layout.start_session_with_text_measurer(text_measurer))
            },
            pending_pages: pending,
            rendered_pages: Vec::with_capacity(0),
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
        self.prepare_chapter_with_config(book, chapter_index, config, |page| {
            if pages.len() < page_limit {
                pages.push(page);
            } else {
                dropped_pages = dropped_pages.saturating_add(1);
            }
        })?;
        let chapter_total = pages.len().max(1);
        for page in pages.iter_mut() {
            Self::annotate_page_for_chapter(page, chapter_index);
            if page.metrics.chapter_page_count.is_none() {
                Self::annotate_page_metrics(page, chapter_total);
            }
        }
        if dropped_pages > 0 {
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
    ) -> Result<(), RenderEngineError>
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
    ) -> Result<(), RenderEngineError>
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
    ) -> Result<(), RenderEngineError>
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
    ) -> Result<(), RenderEngineError>
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
    ) -> Result<(), RenderEngineError>
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
    ) -> Result<(), RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        C: CancelToken + ?Sized,
        F: FnMut(RenderPage),
    {
        let embedded_fonts = config.embedded_fonts;
        let defer_emit_until_finish = config.page_range.is_some();
        let started = Instant::now();
        if cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        let mut session = self.begin(chapter_index, config);
        session.set_hyphenation_language(book.language());
        if session.is_complete() {
            session.drain_pages(&mut on_page);
            return Ok(());
        }
        let mut prep = RenderPrep::new(self.opts.prep).with_serif_default();
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
                session.drain_pages(&mut on_page);
            }
        })?;
        if saw_cancelled || cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        session.finish()?;
        session.drain_pages(&mut on_page);
        let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.emit_diagnostic(RenderDiagnostic::ReflowTimeMs(elapsed));
        Ok(())
    }

    fn prepare_chapter_bytes_with_cancel_and_config<R, C, F>(
        &self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        html: &[u8],
        cancel: &C,
        config: RenderConfig<'_>,
        mut on_page: F,
    ) -> Result<(), RenderEngineError>
    where
        R: std::io::Read + std::io::Seek,
        C: CancelToken + ?Sized,
        F: FnMut(RenderPage),
    {
        let embedded_fonts = config.embedded_fonts;
        let defer_emit_until_finish = config.page_range.is_some();
        let started = Instant::now();
        if cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        let mut session = self.begin(chapter_index, config);
        session.set_hyphenation_language(book.language());
        if session.is_complete() {
            session.drain_pages(&mut on_page);
            return Ok(());
        }
        let mut prep = RenderPrep::new(self.opts.prep).with_serif_default();
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
                session.drain_pages(&mut on_page);
            }
        })?;
        if saw_cancelled || cancel.is_cancelled() {
            self.emit_diagnostic(RenderDiagnostic::Cancelled);
            return Err(RenderEngineError::Cancelled);
        }
        session.finish()?;
        session.drain_pages(&mut on_page);
        let elapsed = started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.emit_diagnostic(RenderDiagnostic::ReflowTimeMs(elapsed));
        Ok(())
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
            return Ok(Vec::with_capacity(0));
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
                Ok(()) => {
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
    ) -> Result<(), RenderEngineError>
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
            page.sync_commands();
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
    use mu_epub::{BlockRole, ComputedTextStyle, StyledEvent, StyledRun};

    fn body_run(text: &str) -> StyledEventOrRun {
        StyledEventOrRun::Run(StyledRun {
            text: text.to_string(),
            style: ComputedTextStyle {
                family_stack: vec!["serif".to_string()],
                weight: 400,
                italic: false,
                size_px: 16.0,
                line_height: 1.4,
                letter_spacing: 0.0,
                block_role: BlockRole::Body,
            },
            font_id: 0,
            resolved_family: "serif".to_string(),
        })
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
}
