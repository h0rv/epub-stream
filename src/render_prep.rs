//! Stream-first style/font preparation APIs for rendering pipelines.

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::min;
use core::fmt;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use smallvec::SmallVec;
use std::io::{BufRead, BufReader};

use crate::book::EpubBook;
use crate::css::{
    parse_inline_style, parse_stylesheet, CssStyle, FontSize, FontStyle, FontWeight, LineHeight,
    Stylesheet,
};
use crate::error::{
    EpubError, ErrorLimitContext, ErrorPhase, PhaseError, PhaseErrorContext, ZipError,
};

const IMAGE_DIMENSION_PROBE_MAX_BYTES: usize = 64 * 1024;
const IMAGE_DIMENSION_PROBE_CHUNK_BYTES: usize = 2 * 1024;

/// Limits for stylesheet parsing and application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleLimits {
    /// Maximum number of stylesheet rules to process.
    pub max_selectors: usize,
    /// Maximum bytes read for any individual stylesheet.
    pub max_css_bytes: usize,
    /// Maximum supported list nesting depth (reserved for downstream layout usage).
    pub max_nesting: usize,
}

impl Default for StyleLimits {
    fn default() -> Self {
        Self {
            max_selectors: 4096,
            max_css_bytes: 512 * 1024,
            max_nesting: 32,
        }
    }
}

/// Limits for embedded font enumeration and registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontLimits {
    /// Maximum number of font faces accepted.
    ///
    /// Note: the embedded-graphics TTF backend structurally limits selectable
    /// faces to `epub_stream_embedded_graphics::TTF_MAX_SELECTABLE_FACES` (32)
    /// due to 5-bit face ID encoding. Values above this cap are clamped with a
    /// warning at backend construction time.
    pub max_faces: usize,
    /// Maximum bytes for any one font file.
    pub max_bytes_per_font: usize,
    /// Maximum total bytes across all registered font files.
    pub max_total_font_bytes: usize,
}

impl Default for FontLimits {
    fn default() -> Self {
        Self {
            max_faces: 64,
            max_bytes_per_font: 8 * 1024 * 1024,
            max_total_font_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Safe layout hint clamps for text style normalization.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutHints {
    /// Default base font size in pixels.
    pub base_font_size_px: f32,
    /// Lower clamp for effective font size.
    pub min_font_size_px: f32,
    /// Upper clamp for effective font size.
    pub max_font_size_px: f32,
    /// Lower clamp for effective line-height multiplier.
    pub min_line_height: f32,
    /// Upper clamp for effective line-height multiplier.
    pub max_line_height: f32,
    /// Global text scale multiplier applied after CSS size resolution.
    ///
    /// This lets reader UIs scale books even when EPUB CSS uses fixed px sizes.
    pub text_scale: f32,
}

impl Default for LayoutHints {
    fn default() -> Self {
        Self {
            base_font_size_px: 16.0,
            min_font_size_px: 10.0,
            max_font_size_px: 42.0,
            min_line_height: 1.1,
            max_line_height: 2.2,
            text_scale: 1.0,
        }
    }
}

/// Style engine options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleConfig {
    /// Hard parsing limits.
    pub limits: StyleLimits,
    /// Normalization and clamp hints.
    pub hints: LayoutHints,
}

/// Render-prep orchestration options.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderPrepOptions {
    /// Stylesheet parsing and resolution options.
    pub style: StyleConfig,
    /// Font registration limits.
    pub fonts: FontLimits,
    /// Final style normalization hints.
    pub layout_hints: LayoutHints,
    /// Hard memory/resource budgets.
    pub memory: MemoryBudget,
}

/// Hard memory/resource budgets for open/parse/style/layout/render paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Max bytes allowed for a single heavy entry read (e.g. chapter XHTML).
    pub max_entry_bytes: usize,
    /// Max bytes allowed for a stylesheet payload.
    pub max_css_bytes: usize,
    /// Max bytes allowed for a navigation document payload.
    pub max_nav_bytes: usize,
    /// Max bytes allowed for a single inline `style="..."` attribute payload.
    pub max_inline_style_bytes: usize,
    /// Max page objects allowed in memory for eager consumers.
    pub max_pages_in_memory: usize,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self {
            max_entry_bytes: 4 * 1024 * 1024,
            max_css_bytes: 512 * 1024,
            max_nav_bytes: 512 * 1024,
            max_inline_style_bytes: 16 * 1024,
            max_pages_in_memory: 128,
        }
    }
}

/// Structured error for style/font preparation operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderPrepError {
    /// Processing phase where this error originated.
    pub phase: ErrorPhase,
    /// Stable machine-readable code.
    pub code: &'static str,
    /// Human-readable message.
    pub message: Box<str>,
    /// Optional archive path context.
    pub path: Option<Box<str>>,
    /// Optional chapter index context.
    pub chapter_index: Option<usize>,
    /// Optional typed actual-vs-limit context.
    pub limit: Option<Box<ErrorLimitContext>>,
    /// Optional additional context.
    pub context: Option<Box<RenderPrepErrorContext>>,
}

/// Extended optional context for render-prep errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenderPrepErrorContext {
    /// Optional source context (stylesheet href, inline style location, tokenizer phase).
    pub source: Option<Box<str>>,
    /// Optional selector context.
    pub selector: Option<Box<str>>,
    /// Optional selector index for structured consumers.
    pub selector_index: Option<usize>,
    /// Optional declaration context.
    pub declaration: Option<Box<str>>,
    /// Optional declaration index for structured consumers.
    pub declaration_index: Option<usize>,
    /// Optional tokenizer/read offset in bytes.
    pub token_offset: Option<usize>,
}

impl RenderPrepError {
    fn new_with_phase(phase: ErrorPhase, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            phase,
            code,
            message: message.into().into_boxed_str(),
            path: None,
            chapter_index: None,
            limit: None,
            context: None,
        }
    }

    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self::new_with_phase(ErrorPhase::Style, code, message)
    }

    fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into().into_boxed_str());
        self
    }

    fn with_phase(mut self, phase: ErrorPhase) -> Self {
        self.phase = phase;
        self
    }

    fn with_source(mut self, source: impl Into<String>) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.source = Some(source.into().into_boxed_str());
        self
    }

    fn with_chapter_index(mut self, chapter_index: usize) -> Self {
        self.chapter_index = Some(chapter_index);
        self
    }

    fn with_limit(mut self, kind: &'static str, actual: usize, limit: usize) -> Self {
        self.limit = Some(Box::new(ErrorLimitContext::new(kind, actual, limit)));
        self
    }

    fn with_selector(mut self, selector: impl Into<String>) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.selector = Some(selector.into().into_boxed_str());
        self
    }

    fn with_selector_index(mut self, selector_index: usize) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.selector_index = Some(selector_index);
        self
    }

    fn with_declaration(mut self, declaration: impl Into<String>) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.declaration = Some(declaration.into().into_boxed_str());
        self
    }

    fn with_declaration_index(mut self, declaration_index: usize) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.declaration_index = Some(declaration_index);
        self
    }

    fn with_token_offset(mut self, token_offset: usize) -> Self {
        let ctx = self
            .context
            .get_or_insert_with(|| Box::new(RenderPrepErrorContext::default()));
        ctx.token_offset = Some(token_offset);
        self
    }
}

impl fmt::Display for RenderPrepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.phase, self.code, self.message)?;
        if let Some(path) = self.path.as_deref() {
            write!(f, " [path={}]", path)?;
        }
        if let Some(chapter_index) = self.chapter_index {
            write!(f, " [chapter_index={}]", chapter_index)?;
        }
        if let Some(limit) = self.limit.as_deref() {
            write!(
                f,
                " [limit_kind={} actual={} limit={}]",
                limit.kind, limit.actual, limit.limit
            )?;
        }
        if let Some(ctx) = &self.context {
            if let Some(source) = ctx.source.as_deref() {
                write!(f, " [source={}]", source)?;
            }
            if let Some(selector) = ctx.selector.as_deref() {
                write!(f, " [selector={}]", selector)?;
            }
            if let Some(selector_index) = ctx.selector_index {
                write!(f, " [selector_index={}]", selector_index)?;
            }
            if let Some(declaration) = ctx.declaration.as_deref() {
                write!(f, " [declaration={}]", declaration)?;
            }
            if let Some(declaration_index) = ctx.declaration_index {
                write!(f, " [declaration_index={}]", declaration_index)?;
            }
            if let Some(token_offset) = ctx.token_offset {
                write!(f, " [token_offset={}]", token_offset)?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for RenderPrepError {}

impl From<RenderPrepError> for PhaseError {
    fn from(err: RenderPrepError) -> Self {
        let mut ctx = PhaseErrorContext {
            path: err.path.clone(),
            href: err.path.clone(),
            chapter_index: err.chapter_index,
            source: None,
            selector: None,
            selector_index: None,
            declaration: None,
            declaration_index: None,
            token_offset: None,
            limit: err.limit.clone(),
        };

        if let Some(extra) = &err.context {
            ctx.source = extra.source.clone();
            ctx.selector = extra.selector.clone();
            ctx.selector_index = extra.selector_index;
            ctx.declaration = extra.declaration.clone();
            ctx.declaration_index = extra.declaration_index;
            ctx.token_offset = extra.token_offset;
        }

        PhaseError {
            phase: err.phase,
            code: err.code,
            message: err.message,
            context: Some(Box::new(ctx)),
        }
    }
}

impl From<RenderPrepError> for EpubError {
    fn from(err: RenderPrepError) -> Self {
        EpubError::Phase(err.into())
    }
}

/// Source stylesheet payload in chapter cascade order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StylesheetSource {
    /// Archive path or inline marker for this stylesheet.
    pub href: String,
    /// Raw CSS bytes decoded as UTF-8.
    pub css: String,
}

/// Collection of resolved stylesheet sources.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChapterStylesheets {
    /// Sources in cascade order.
    pub sources: Vec<StylesheetSource>,
}

impl ChapterStylesheets {
    /// Iterate all stylesheet sources.
    pub fn iter(&self) -> impl Iterator<Item = &StylesheetSource> {
        self.sources.iter()
    }
}

/// Font style descriptor for `@font-face` metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedFontStyle {
    /// Upright style.
    Normal,
    /// Italic style.
    Italic,
    /// Oblique style.
    Oblique,
}

/// Embedded font face metadata extracted from EPUB CSS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedFontFace {
    /// Requested font family from `@font-face`.
    pub family: String,
    /// Numeric weight (e.g. 400, 700).
    pub weight: u16,
    /// Style variant.
    pub style: EmbeddedFontStyle,
    /// Optional stretch descriptor.
    pub stretch: Option<String>,
    /// OPF-relative href to font resource.
    pub href: String,
    /// Optional format hint from `format(...)`.
    pub format: Option<String>,
}

/// Semantic block role for computed styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole {
    /// Body text.
    Body,
    /// Paragraph block.
    Paragraph,
    /// Heading block by level.
    Heading(u8),
    /// List item block.
    ListItem,
    /// Figure caption block.
    FigureCaption,
    /// Preformatted/code block where whitespace/newlines are significant.
    Preformatted,
}

/// Cascaded and normalized text style for rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputedTextStyle {
    /// Ordered family preference stack.
    pub family_stack: SmallVec<[String; 2]>,
    /// Numeric weight.
    pub weight: u16,
    /// Italic toggle.
    pub italic: bool,
    /// Effective font size in pixels.
    pub size_px: f32,
    /// Effective line-height multiplier.
    pub line_height: f32,
    /// Effective letter spacing in pixels.
    pub letter_spacing: f32,
    /// Semantic block role.
    pub block_role: BlockRole,
}

/// Styled text run.
#[derive(Clone, Debug, PartialEq)]
pub struct StyledRun {
    /// Run text payload.
    pub text: String,
    /// Computed style for this run.
    pub style: ComputedTextStyle,
    /// Stable resolved font identity (0 means policy fallback).
    pub font_id: u32,
}

/// Styled inline image payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledImage {
    /// OPF-relative image href.
    pub src: String,
    /// Optional caption/alt text.
    pub alt: String,
    /// Optional intrinsic width hint in CSS px.
    pub width_px: Option<u16>,
    /// Optional intrinsic height hint in CSS px.
    pub height_px: Option<u16>,
    /// Whether image appears inside a `<figure>` container.
    pub in_figure: bool,
}

/// Structured block/layout events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyledEvent {
    /// Paragraph starts.
    ParagraphStart,
    /// Paragraph ends.
    ParagraphEnd,
    /// Heading starts.
    HeadingStart(u8),
    /// Heading ends.
    HeadingEnd(u8),
    /// List item starts.
    ListItemStart,
    /// List item ends.
    ListItemEnd,
    /// Explicit line break.
    LineBreak,
}

/// Stream item for styled output.
#[derive(Clone, Debug, PartialEq)]
pub enum StyledEventOrRun {
    /// Structural event.
    Event(StyledEvent),
    /// Styled text run.
    Run(StyledRun),
    /// Styled inline image.
    Image(StyledImage),
}

/// Styled chapter output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyledChapter {
    items: Vec<StyledEventOrRun>,
}

impl StyledChapter {
    /// Iterate full event/run stream.
    pub fn iter(&self) -> impl Iterator<Item = &StyledEventOrRun> {
        self.items.iter()
    }

    /// Iterate only text runs.
    pub fn runs(&self) -> impl Iterator<Item = &StyledRun> {
        self.items.iter().filter_map(|item| match item {
            StyledEventOrRun::Run(run) => Some(run),
            _ => None,
        })
    }

    /// Build from a pre-collected item vector.
    pub fn from_items(items: Vec<StyledEventOrRun>) -> Self {
        Self { items }
    }
}

/// Lightweight style system with CSS cascade resolution.
#[derive(Clone, Debug)]
pub struct Styler {
    config: StyleConfig,
    memory: MemoryBudget,
    parsed: Vec<Stylesheet>,
}

impl Styler {
    /// Create a styler with explicit config.
    pub fn new(config: StyleConfig) -> Self {
        Self {
            config,
            memory: MemoryBudget::default(),
            parsed: Vec::with_capacity(8),
        }
    }

    /// Override hard memory budget used in style paths.
    pub fn with_memory_budget(mut self, memory: MemoryBudget) -> Self {
        self.memory = memory;
        self
    }

    /// Parse and load stylesheets in cascade order.
    pub fn load_stylesheets(
        &mut self,
        sources: &ChapterStylesheets,
    ) -> Result<(), RenderPrepError> {
        self.clear_stylesheets();
        for source in &sources.sources {
            self.push_stylesheet_source(&source.href, &source.css)?;
        }
        Ok(())
    }

    fn clear_stylesheets(&mut self) {
        self.parsed.clear();
    }

    fn push_stylesheet_source(&mut self, href: &str, css: &str) -> Result<(), RenderPrepError> {
        let css_limit = min(self.config.limits.max_css_bytes, self.memory.max_css_bytes);
        if css.len() > css_limit {
            let err = RenderPrepError::new(
                "STYLE_CSS_TOO_LARGE",
                format!(
                    "Stylesheet exceeds max_css_bytes ({} > {})",
                    css.len(),
                    css_limit
                ),
            )
            .with_phase(ErrorPhase::Style)
            .with_limit("max_css_bytes", css.len(), css_limit)
            .with_path(href.to_string())
            .with_source(href.to_string());
            return Err(err);
        }
        let parsed = parse_stylesheet(css).map_err(|e| {
            RenderPrepError::new_with_phase(
                ErrorPhase::Style,
                "STYLE_PARSE_ERROR",
                format!("Failed to parse stylesheet: {}", e),
            )
            .with_path(href.to_string())
            .with_source(href.to_string())
        })?;
        if parsed.len() > self.config.limits.max_selectors {
            let err = RenderPrepError::new(
                "STYLE_SELECTOR_LIMIT",
                format!(
                    "Stylesheet exceeds max_selectors ({} > {})",
                    parsed.len(),
                    self.config.limits.max_selectors
                ),
            )
            .with_phase(ErrorPhase::Style)
            .with_limit(
                "max_selectors",
                parsed.len(),
                self.config.limits.max_selectors,
            )
            .with_selector(format!("selector_count={}", parsed.len()))
            .with_selector_index(self.config.limits.max_selectors)
            .with_path(href.to_string())
            .with_source(href.to_string());
            return Err(err);
        }
        self.parsed.push(parsed);
        Ok(())
    }

    /// Style a chapter and return a stream of events and runs.
    pub fn style_chapter(&self, html: &str) -> Result<StyledChapter, RenderPrepError> {
        let mut items = Vec::with_capacity(8);
        self.style_chapter_with(html, |item| items.push(item))?;
        Ok(StyledChapter { items })
    }

    /// Style a chapter and append results into an output buffer.
    pub fn style_chapter_into(
        &self,
        html: &str,
        out: &mut Vec<StyledEventOrRun>,
    ) -> Result<(), RenderPrepError> {
        self.style_chapter_with(html, |item| out.push(item))
    }

    /// Style a chapter and stream each item to a callback.
    pub fn style_chapter_with<F>(&self, html: &str, mut on_item: F) -> Result<(), RenderPrepError>
    where
        F: FnMut(StyledEventOrRun),
    {
        self.style_chapter_bytes_with(html.as_bytes(), &mut on_item)
    }

    /// Style a chapter from XHTML bytes and stream each item to a callback.
    pub fn style_chapter_bytes_with<F>(
        &self,
        html_bytes: &[u8],
        mut on_item: F,
    ) -> Result<(), RenderPrepError>
    where
        F: FnMut(StyledEventOrRun),
    {
        self.style_chapter_reader_with(html_bytes, &mut on_item)
    }

    /// Style a chapter from a streaming reader and emit each item.
    pub fn style_chapter_reader_with<Rd, F>(
        &self,
        html_reader: Rd,
        mut on_item: F,
    ) -> Result<(), RenderPrepError>
    where
        Rd: BufRead,
        F: FnMut(StyledEventOrRun),
    {
        let mut reader = Reader::from_reader(html_reader);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::with_capacity(8);
        let mut stack: Vec<ElementCtx> = Vec::with_capacity(8);
        let mut skip_depth = 0usize;
        let mut nesting_overflow = 0usize;
        let max_nesting = self.config.limits.max_nesting;
        let mut table_row_cells: Vec<usize> = Vec::with_capacity(8);
        let mut entity_buf = String::with_capacity(16);
        let mut pending_run: Option<StyledRun> = None;
        let mut resolved_style_cache: Option<(CssStyle, BlockRole, bool, bool)> = None;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    let tag = decode_tag_name(&reader, e.name().as_ref())?;
                    if should_skip_tag(tag.as_ref()) {
                        skip_depth += 1;
                        buf.clear();
                        continue;
                    }
                    if skip_depth > 0 {
                        buf.clear();
                        continue;
                    }
                    let ctx = element_ctx_from_start(
                        &reader,
                        &e,
                        tag,
                        self.memory.max_inline_style_bytes,
                    )?;
                    if matches!(ctx.tag.as_ref(), "img" | "image") {
                        let in_figure = stack.iter().any(|parent| parent.tag == "figure");
                        emit_image_event(&ctx, in_figure, &mut |item| {
                            emit_styled_item(&mut pending_run, item, &mut on_item);
                        });
                        buf.clear();
                        continue;
                    }
                    if ctx.tag == "tr" {
                        table_row_cells.push(0);
                    } else if matches!(ctx.tag.as_ref(), "td" | "th") {
                        if let Some(cell_count) = table_row_cells.last_mut() {
                            if *cell_count > 0 {
                                if resolved_style_cache.is_none() {
                                    resolved_style_cache = Some(self.resolve_context_style(&stack));
                                }
                                let Some((resolved, role, bold_tag, italic_tag)) =
                                    resolved_style_cache.as_ref()
                                else {
                                    buf.clear();
                                    continue;
                                };
                                let style =
                                    self.compute_style(resolved, *role, *bold_tag, *italic_tag);
                                emit_styled_run_text(
                                    &mut pending_run,
                                    Cow::Borrowed(" | "),
                                    style,
                                    0,
                                    &mut on_item,
                                );
                            }
                            *cell_count = cell_count.saturating_add(1);
                        }
                    }
                    emit_start_event(ctx.tag.as_ref(), &mut |item| {
                        emit_styled_item(&mut pending_run, item, &mut on_item);
                    });
                    if stack.len() >= max_nesting {
                        nesting_overflow += 1;
                        log::warn!(
                            "Element nesting depth {} exceeds max_nesting ({}); flattening style",
                            stack.len() + nesting_overflow,
                            max_nesting
                        );
                    } else {
                        stack.push(ctx);
                    }
                    resolved_style_cache = None;
                }
                Ok(Event::Empty(e)) => {
                    let tag = decode_tag_name(&reader, e.name().as_ref())?;
                    if skip_depth > 0 || should_skip_tag(tag.as_ref()) {
                        buf.clear();
                        continue;
                    }
                    let ctx = element_ctx_from_start(
                        &reader,
                        &e,
                        tag,
                        self.memory.max_inline_style_bytes,
                    )?;
                    if matches!(ctx.tag.as_ref(), "img" | "image") {
                        let in_figure = stack.iter().any(|parent| parent.tag == "figure");
                        emit_image_event(&ctx, in_figure, &mut |item| {
                            emit_styled_item(&mut pending_run, item, &mut on_item);
                        });
                        buf.clear();
                        continue;
                    }
                    if matches!(ctx.tag.as_ref(), "td" | "th") {
                        if let Some(cell_count) = table_row_cells.last_mut() {
                            if *cell_count > 0 {
                                if resolved_style_cache.is_none() {
                                    resolved_style_cache = Some(self.resolve_context_style(&stack));
                                }
                                let Some((resolved, role, bold_tag, italic_tag)) =
                                    resolved_style_cache.as_ref()
                                else {
                                    buf.clear();
                                    continue;
                                };
                                let style =
                                    self.compute_style(resolved, *role, *bold_tag, *italic_tag);
                                emit_styled_run_text(
                                    &mut pending_run,
                                    Cow::Borrowed(" | "),
                                    style,
                                    0,
                                    &mut on_item,
                                );
                            }
                            *cell_count = cell_count.saturating_add(1);
                        }
                    }
                    emit_start_event(ctx.tag.as_ref(), &mut |item| {
                        emit_styled_item(&mut pending_run, item, &mut on_item);
                    });
                    if ctx.tag == "br" {
                        emit_styled_item(
                            &mut pending_run,
                            StyledEventOrRun::Event(StyledEvent::LineBreak),
                            &mut on_item,
                        );
                    }
                    emit_end_event(ctx.tag.as_ref(), &mut |item| {
                        emit_styled_item(&mut pending_run, item, &mut on_item);
                    });
                }
                Ok(Event::End(e)) => {
                    let tag = decode_tag_name(&reader, e.name().as_ref())?;
                    if should_skip_tag(tag.as_ref()) {
                        skip_depth = skip_depth.saturating_sub(1);
                        buf.clear();
                        continue;
                    }
                    if skip_depth > 0 {
                        buf.clear();
                        continue;
                    }
                    emit_end_event(tag.as_ref(), &mut |item| {
                        emit_styled_item(&mut pending_run, item, &mut on_item);
                    });
                    if tag == "tr" {
                        table_row_cells.pop();
                    }
                    if nesting_overflow > 0 {
                        nesting_overflow -= 1;
                    } else if let Some(last) = stack.last() {
                        if last.tag == tag {
                            stack.pop();
                        }
                    }
                    resolved_style_cache = None;
                }
                Ok(Event::Text(e)) => {
                    if skip_depth > 0 {
                        buf.clear();
                        continue;
                    }
                    let text = e.decode().map_err(|err| {
                        RenderPrepError::new(
                            "STYLE_TOKENIZE_ERROR",
                            format!("Decode error: {:?}", err),
                        )
                        .with_phase(ErrorPhase::Style)
                        .with_source("text node decode")
                        .with_token_offset(reader_token_offset(&reader))
                    })?;
                    let preserve_ws = is_preformatted_context(&stack);
                    let normalized = normalize_plain_text_whitespace(text.as_ref(), preserve_ws);
                    if normalized.is_empty() {
                        buf.clear();
                        continue;
                    }
                    if resolved_style_cache.is_none() {
                        resolved_style_cache = Some(self.resolve_context_style(&stack));
                    }
                    let Some((resolved, role, bold_tag, italic_tag)) =
                        resolved_style_cache.as_ref()
                    else {
                        buf.clear();
                        continue;
                    };
                    let style = self.compute_style(resolved, *role, *bold_tag, *italic_tag);
                    emit_styled_run_text(&mut pending_run, normalized, style, 0, &mut on_item);
                }
                Ok(Event::CData(e)) => {
                    if skip_depth > 0 {
                        buf.clear();
                        continue;
                    }
                    let text = reader.decoder().decode(&e).map_err(|err| {
                        RenderPrepError::new(
                            "STYLE_TOKENIZE_ERROR",
                            format!("Decode error: {:?}", err),
                        )
                        .with_phase(ErrorPhase::Style)
                        .with_source("cdata decode")
                        .with_token_offset(reader_token_offset(&reader))
                    })?;
                    let preserve_ws = is_preformatted_context(&stack);
                    let normalized = normalize_plain_text_whitespace(text.as_ref(), preserve_ws);
                    if normalized.is_empty() {
                        buf.clear();
                        continue;
                    }
                    if resolved_style_cache.is_none() {
                        resolved_style_cache = Some(self.resolve_context_style(&stack));
                    }
                    let Some((resolved, role, bold_tag, italic_tag)) =
                        resolved_style_cache.as_ref()
                    else {
                        buf.clear();
                        continue;
                    };
                    let style = self.compute_style(resolved, *role, *bold_tag, *italic_tag);
                    emit_styled_run_text(&mut pending_run, normalized, style, 0, &mut on_item);
                }
                Ok(Event::GeneralRef(e)) => {
                    if skip_depth > 0 {
                        buf.clear();
                        continue;
                    }
                    let entity_name = e.decode().map_err(|err| {
                        RenderPrepError::new(
                            "STYLE_TOKENIZE_ERROR",
                            format!("Decode error: {:?}", err),
                        )
                        .with_phase(ErrorPhase::Style)
                        .with_source("entity decode")
                        .with_token_offset(reader_token_offset(&reader))
                    })?;
                    entity_buf.clear();
                    entity_buf.push('&');
                    entity_buf.push_str(entity_name.as_ref());
                    entity_buf.push(';');
                    let resolved_entity =
                        quick_xml::escape::unescape(&entity_buf).map_err(|err| {
                            RenderPrepError::new(
                                "STYLE_TOKENIZE_ERROR",
                                format!("Unescape error: {:?}", err),
                            )
                            .with_phase(ErrorPhase::Style)
                            .with_source("entity unescape")
                            .with_token_offset(reader_token_offset(&reader))
                        })?;
                    let preserve_ws = is_preformatted_context(&stack);
                    let normalized =
                        normalize_plain_text_whitespace(resolved_entity.as_ref(), preserve_ws);
                    if normalized.is_empty() {
                        buf.clear();
                        continue;
                    }
                    if resolved_style_cache.is_none() {
                        resolved_style_cache = Some(self.resolve_context_style(&stack));
                    }
                    let Some((resolved, role, bold_tag, italic_tag)) =
                        resolved_style_cache.as_ref()
                    else {
                        buf.clear();
                        continue;
                    };
                    let style = self.compute_style(resolved, *role, *bold_tag, *italic_tag);
                    emit_styled_run_text(&mut pending_run, normalized, style, 0, &mut on_item);
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(err) => {
                    return Err(RenderPrepError::new(
                        "STYLE_TOKENIZE_ERROR",
                        format!("XML error: {:?}", err),
                    )
                    .with_phase(ErrorPhase::Style)
                    .with_source("xml tokenizer")
                    .with_token_offset(reader_token_offset(&reader)));
                }
            }
            buf.clear();
        }
        flush_pending_run(&mut pending_run, &mut on_item);

        Ok(())
    }

    fn resolve_tag_style(&self, tag: &str, classes: &[String]) -> CssStyle {
        let mut style = CssStyle::new();
        if classes.is_empty() {
            for ss in &self.parsed {
                style.merge(&ss.resolve(tag, &[]));
            }
            return style;
        }
        let mut class_refs = Vec::with_capacity(classes.len());
        class_refs.extend(classes.iter().map(String::as_str));
        for ss in &self.parsed {
            style.merge(&ss.resolve(tag, &class_refs));
        }
        style
    }

    fn compute_style(
        &self,
        resolved: &CssStyle,
        role: BlockRole,
        bold_tag: bool,
        italic_tag: bool,
    ) -> ComputedTextStyle {
        let mut size_px = match resolved.font_size {
            Some(FontSize::Px(px)) => px,
            Some(FontSize::Em(em)) => self.config.hints.base_font_size_px * em,
            None => {
                if matches!(role, BlockRole::Heading(1 | 2)) {
                    self.config.hints.base_font_size_px * 1.25
                } else if matches!(role, BlockRole::FigureCaption) {
                    self.config.hints.base_font_size_px * 0.90
                } else {
                    self.config.hints.base_font_size_px
                }
            }
        };
        size_px *= self.config.hints.text_scale.clamp(0.5, 3.0);
        size_px = size_px.clamp(
            self.config.hints.min_font_size_px,
            self.config.hints.max_font_size_px,
        );

        let mut line_height = match resolved.line_height {
            Some(LineHeight::Px(px)) => (px / size_px).max(1.0),
            Some(LineHeight::Multiplier(m)) => m,
            None => {
                if matches!(role, BlockRole::FigureCaption) {
                    1.3
                } else if matches!(role, BlockRole::Preformatted) {
                    1.25
                } else {
                    1.4
                }
            }
        };
        line_height = line_height.clamp(
            self.config.hints.min_line_height,
            self.config.hints.max_line_height,
        );

        let weight = match resolved.font_weight.unwrap_or(FontWeight::Normal) {
            FontWeight::Bold => 700,
            FontWeight::Normal => 400,
        };
        let italic = matches!(
            resolved.font_style.unwrap_or(FontStyle::Normal),
            FontStyle::Italic
        );
        let final_weight = if bold_tag { 700 } else { weight };
        let final_italic = italic || italic_tag || matches!(role, BlockRole::FigureCaption);

        let family_stack = resolved
            .font_family
            .as_deref()
            .map(split_family_stack)
            .unwrap_or_else(|| {
                let mut stack = SmallVec::new();
                stack.push("serif".to_string());
                stack
            });
        let letter_spacing = resolved.letter_spacing.unwrap_or(0.0);

        ComputedTextStyle {
            family_stack,
            weight: final_weight,
            italic: final_italic,
            size_px,
            line_height,
            letter_spacing,
            block_role: role,
        }
    }

    fn resolve_context_style(&self, stack: &[ElementCtx]) -> (CssStyle, BlockRole, bool, bool) {
        let mut merged = CssStyle::new();
        let mut role = BlockRole::Body;
        let mut bold_tag = false;
        let mut italic_tag = false;

        for ctx in stack {
            merged.merge(&self.resolve_tag_style(&ctx.tag, &ctx.classes));
            if let Some(inline) = &ctx.inline_style {
                merged.merge(inline);
            }
            if matches!(ctx.tag.as_ref(), "strong" | "b") {
                bold_tag = true;
            }
            if matches!(ctx.tag.as_ref(), "em" | "i") {
                italic_tag = true;
            }
            role = role_from_tag(&ctx.tag).unwrap_or(role);
        }

        (merged, role, bold_tag, italic_tag)
    }
}

/// Fallback policy for font matching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontPolicy {
    /// Preferred family order used when style stack has no embedded match.
    pub preferred_families: Vec<String>,
    /// Final fallback family.
    pub default_family: String,
    /// Whether embedded fonts are allowed for matching.
    pub allow_embedded_fonts: bool,
    /// Whether synthetic bold is allowed.
    pub synthetic_bold: bool,
    /// Whether synthetic italic is allowed.
    pub synthetic_italic: bool,
}

impl FontPolicy {
    /// Serif-first policy.
    pub fn serif_default() -> Self {
        Self {
            preferred_families: vec!["serif".to_string()],
            default_family: "serif".to_string(),
            allow_embedded_fonts: true,
            synthetic_bold: false,
            synthetic_italic: false,
        }
    }
}

/// First-class public fallback policy alias.
pub type FontFallbackPolicy = FontPolicy;

impl Default for FontPolicy {
    fn default() -> Self {
        Self::serif_default()
    }
}

/// Resolved font face for a style request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedFontFace {
    /// Stable resolver identity for the chosen face (0 means policy fallback face).
    pub font_id: u32,
    /// Chosen family.
    pub family: String,
    /// Selected face metadata when matched in EPUB.
    pub embedded: Option<EmbeddedFontFace>,
}

/// Trace output for fallback reasoning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontResolutionTrace {
    /// Final selected face.
    pub face: ResolvedFontFace,
    /// Resolution reasoning chain.
    pub reason_chain: Vec<String>,
}

/// Font resolution engine.
#[derive(Clone, Debug)]
pub struct FontResolver {
    policy: FontPolicy,
    limits: FontLimits,
    faces: Vec<EmbeddedFontFace>,
}

impl FontResolver {
    /// Create a resolver with explicit policy and limits.
    pub fn new(policy: FontPolicy) -> Self {
        Self {
            policy,
            limits: FontLimits::default(),
            faces: Vec::with_capacity(8),
        }
    }

    /// Override registration limits.
    pub fn with_limits(mut self, limits: FontLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Register EPUB fonts and validate byte limits via callback.
    pub fn register_epub_fonts<I, F>(
        &mut self,
        fonts: I,
        mut loader: F,
    ) -> Result<(), RenderPrepError>
    where
        I: IntoIterator<Item = EmbeddedFontFace>,
        F: FnMut(&str) -> Result<Vec<u8>, EpubError>,
    {
        self.faces.clear();
        let mut total = 0usize;
        let mut dedupe_keys: Vec<(String, u16, EmbeddedFontStyle, String)> = Vec::with_capacity(8);

        for face in fonts {
            let normalized_family = normalize_family(&face.family);
            let dedupe_key = (
                normalized_family,
                face.weight,
                face.style,
                face.href.to_ascii_lowercase(),
            );
            if dedupe_keys.contains(&dedupe_key) {
                continue;
            }
            if self.faces.len() >= self.limits.max_faces {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_FACE_LIMIT",
                    "Too many embedded font faces",
                )
                .with_limit(
                    "max_faces",
                    self.faces.len() + 1,
                    self.limits.max_faces,
                ));
            }
            let bytes = loader(&face.href).map_err(|e| {
                RenderPrepError::new_with_phase(ErrorPhase::Style, "FONT_LOAD_ERROR", e.to_string())
                    .with_path(face.href.clone())
            })?;
            if bytes.len() > self.limits.max_bytes_per_font {
                let err = RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_BYTES_PER_FACE_LIMIT",
                    format!(
                        "Font exceeds max_bytes_per_font ({} > {})",
                        bytes.len(),
                        self.limits.max_bytes_per_font
                    ),
                )
                .with_path(face.href.clone())
                .with_limit(
                    "max_bytes_per_font",
                    bytes.len(),
                    self.limits.max_bytes_per_font,
                );
                return Err(err);
            }
            total += bytes.len();
            if total > self.limits.max_total_font_bytes {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_TOTAL_BYTES_LIMIT",
                    format!(
                        "Total font bytes exceed max_total_font_bytes ({} > {})",
                        total, self.limits.max_total_font_bytes
                    ),
                )
                .with_limit(
                    "max_total_font_bytes",
                    total,
                    self.limits.max_total_font_bytes,
                ));
            }
            dedupe_keys.push(dedupe_key);
            self.faces.push(face);
        }

        Ok(())
    }

    /// Register EPUB fonts and validate byte limits via byte-counting callback.
    ///
    /// The callback receives a hard cap and should stream bytes without
    /// materializing full font payloads in memory.
    pub fn register_epub_fonts_with_size_loader<I, F>(
        &mut self,
        fonts: I,
        mut loader: F,
    ) -> Result<(), RenderPrepError>
    where
        I: IntoIterator<Item = EmbeddedFontFace>,
        F: FnMut(&str, usize) -> Result<usize, EpubError>,
    {
        self.faces.clear();
        let mut total = 0usize;
        let mut dedupe_keys: Vec<(String, u16, EmbeddedFontStyle, String)> = Vec::with_capacity(8);
        let hard_cap = self.limits.max_bytes_per_font.saturating_add(1);

        for face in fonts {
            let normalized_family = normalize_family(&face.family);
            let dedupe_key = (
                normalized_family,
                face.weight,
                face.style,
                face.href.to_ascii_lowercase(),
            );
            if dedupe_keys.contains(&dedupe_key) {
                continue;
            }
            if self.faces.len() >= self.limits.max_faces {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_FACE_LIMIT",
                    "Too many embedded font faces",
                )
                .with_limit(
                    "max_faces",
                    self.faces.len() + 1,
                    self.limits.max_faces,
                ));
            }
            let bytes_len = match loader(&face.href, hard_cap) {
                Ok(size) => size,
                Err(EpubError::Zip(ZipError::FileTooLarge))
                | Err(EpubError::LimitExceeded { .. }) => {
                    return Err(RenderPrepError::new_with_phase(
                        ErrorPhase::Style,
                        "FONT_BYTES_PER_FACE_LIMIT",
                        format!(
                            "Font exceeds max_bytes_per_font (>{})",
                            self.limits.max_bytes_per_font
                        ),
                    )
                    .with_path(face.href.clone())
                    .with_limit(
                        "max_bytes_per_font",
                        self.limits.max_bytes_per_font.saturating_add(1),
                        self.limits.max_bytes_per_font,
                    ));
                }
                Err(err) => {
                    return Err(RenderPrepError::new_with_phase(
                        ErrorPhase::Style,
                        "FONT_LOAD_ERROR",
                        err.to_string(),
                    )
                    .with_path(face.href.clone()));
                }
            };
            if bytes_len > self.limits.max_bytes_per_font {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_BYTES_PER_FACE_LIMIT",
                    format!(
                        "Font exceeds max_bytes_per_font ({} > {})",
                        bytes_len, self.limits.max_bytes_per_font
                    ),
                )
                .with_path(face.href.clone())
                .with_limit(
                    "max_bytes_per_font",
                    bytes_len,
                    self.limits.max_bytes_per_font,
                ));
            }
            total += bytes_len;
            if total > self.limits.max_total_font_bytes {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "FONT_TOTAL_BYTES_LIMIT",
                    format!(
                        "Total font bytes exceed max_total_font_bytes ({} > {})",
                        total, self.limits.max_total_font_bytes
                    ),
                )
                .with_limit(
                    "max_total_font_bytes",
                    total,
                    self.limits.max_total_font_bytes,
                ));
            }
            dedupe_keys.push(dedupe_key);
            self.faces.push(face);
        }

        Ok(())
    }

    fn best_candidate_for_family(
        &self,
        requested_family: &str,
        style: &ComputedTextStyle,
    ) -> Option<(usize, u32)> {
        let mut best_candidate: Option<(usize, u32)> = None;
        for (idx, face) in self.faces.iter().enumerate() {
            if !family_names_match(requested_family, &face.family) {
                continue;
            }
            let score = {
                let weight_delta = (face.weight as i32 - style.weight as i32).unsigned_abs();
                let style_penalty = if style.italic {
                    if matches!(
                        face.style,
                        EmbeddedFontStyle::Italic | EmbeddedFontStyle::Oblique
                    ) {
                        0
                    } else {
                        1000
                    }
                } else if matches!(face.style, EmbeddedFontStyle::Normal) {
                    0
                } else {
                    1000
                };
                weight_delta + style_penalty
            };
            let should_replace = match best_candidate {
                Some((_, best_score)) => score < best_score,
                None => true,
            };
            if should_replace {
                best_candidate = Some((idx, score));
            }
        }
        best_candidate
    }

    fn resolve_matching_face_index(&self, style: &ComputedTextStyle) -> Option<usize> {
        if !self.policy.allow_embedded_fonts {
            return None;
        }
        for family in &style.family_stack {
            if let Some((chosen_idx, _)) = self.best_candidate_for_family(family, style) {
                return Some(chosen_idx);
            }
        }
        None
    }

    /// Resolve a style request to a stable face id (0 means policy fallback).
    pub fn resolve_font_id(&self, style: &ComputedTextStyle) -> u32 {
        self.resolve_matching_face_index(style)
            .map_or(0, |idx| idx as u32 + 1)
    }

    /// Resolve a style request to a concrete face.
    pub fn resolve(&self, style: &ComputedTextStyle) -> ResolvedFontFace {
        if let Some(chosen_idx) = self.resolve_matching_face_index(style) {
            let chosen = &self.faces[chosen_idx];
            ResolvedFontFace {
                font_id: chosen_idx as u32 + 1,
                family: chosen.family.clone(),
                embedded: Some(chosen.clone()),
            }
        } else {
            ResolvedFontFace {
                font_id: 0,
                family: self.policy.default_family.clone(),
                embedded: None,
            }
        }
    }

    /// Resolve with full fallback reasoning.
    pub fn resolve_with_trace(&self, style: &ComputedTextStyle) -> FontResolutionTrace {
        self.resolve_with_trace_for_text(style, None)
    }

    /// Resolve with full fallback reasoning and optional text context.
    pub fn resolve_with_trace_for_text(
        &self,
        style: &ComputedTextStyle,
        text: Option<&str>,
    ) -> FontResolutionTrace {
        let mut reasons = Vec::with_capacity(8);
        for family in &style.family_stack {
            if !self.policy.allow_embedded_fonts {
                reasons.push("embedded fonts disabled by policy".to_string());
                break;
            }
            if let Some((chosen_idx, _)) = self.best_candidate_for_family(family, style) {
                let chosen = &self.faces[chosen_idx];
                reasons.push(format!(
                    "matched embedded family '{}' via nearest weight/style",
                    family
                ));
                return FontResolutionTrace {
                    face: ResolvedFontFace {
                        font_id: chosen_idx as u32 + 1,
                        family: chosen.family.clone(),
                        embedded: Some(chosen.clone()),
                    },
                    reason_chain: reasons,
                };
            }
            reasons.push(format!("family '{}' unavailable in embedded set", family));
        }

        for family in &self.policy.preferred_families {
            reasons.push(format!("preferred fallback family candidate '{}'", family));
        }
        reasons.push(format!(
            "fallback to policy default '{}'",
            self.policy.default_family
        ));
        if text.is_some_and(has_non_ascii) {
            reasons
                .push("missing glyph risk: non-ASCII text with no embedded face match".to_string());
        }
        FontResolutionTrace {
            face: ResolvedFontFace {
                font_id: 0,
                family: self.policy.default_family.clone(),
                embedded: None,
            },
            reason_chain: reasons,
        }
    }
}

/// Render-prep orchestrator.
#[derive(Clone, Debug)]
pub struct RenderPrep {
    opts: RenderPrepOptions,
    styler: Styler,
    font_resolver: FontResolver,
    image_dimension_cache: Vec<(String, Option<(u16, u16)>)>,
    image_probe_scratch: Vec<u8>,
}

/// Structured trace context for a streamed chapter item.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderPrepTrace {
    /// Non-text structural event.
    Event,
    /// Text run with style context and font-resolution trace.
    Run {
        /// Style used for this run during resolution.
        style: Box<ComputedTextStyle>,
        /// Font resolution details for this run.
        font: Box<FontResolutionTrace>,
    },
}

impl RenderPrepTrace {
    /// Return font-resolution trace when this item is a text run.
    pub fn font_trace(&self) -> Option<&FontResolutionTrace> {
        match self {
            Self::Run { font, .. } => Some(font.as_ref()),
            Self::Event => None,
        }
    }

    /// Return style context when this item is a text run.
    pub fn style_context(&self) -> Option<&ComputedTextStyle> {
        match self {
            Self::Run { style, .. } => Some(style.as_ref()),
            Self::Event => None,
        }
    }
}

impl RenderPrep {
    /// Create a render-prep engine.
    pub fn new(opts: RenderPrepOptions) -> Self {
        let styler = Styler::new(opts.style).with_memory_budget(opts.memory);
        let font_resolver = FontResolver::new(FontPolicy::default()).with_limits(opts.fonts);
        Self {
            opts,
            styler,
            font_resolver,
            image_dimension_cache: Vec::with_capacity(64),
            image_probe_scratch: Vec::with_capacity(IMAGE_DIMENSION_PROBE_CHUNK_BYTES),
        }
    }

    /// Use serif default fallback policy.
    pub fn with_serif_default(mut self) -> Self {
        self.font_resolver =
            FontResolver::new(FontPolicy::serif_default()).with_limits(self.opts.fonts);
        self
    }

    /// Override fallback font policy used during style-to-face resolution.
    pub fn with_font_policy(mut self, policy: FontPolicy) -> Self {
        self.font_resolver = FontResolver::new(policy).with_limits(self.opts.fonts);
        self
    }

    /// Register all embedded fonts from a book.
    pub fn with_embedded_fonts_from_book<R: std::io::Read + std::io::Seek>(
        mut self,
        book: &mut EpubBook<R>,
    ) -> Result<Self, RenderPrepError> {
        let fonts = book
            .embedded_fonts_with_options(self.opts.fonts)
            .map_err(|e| {
                RenderPrepError::new_with_phase(
                    ErrorPhase::Parse,
                    "BOOK_EMBEDDED_FONTS",
                    e.to_string(),
                )
            })?;
        self.font_resolver.register_epub_fonts_with_size_loader(
            fonts,
            |href, hard_cap_bytes| {
                let mut sink = std::io::sink();
                book.read_resource_into_with_hard_cap(href, &mut sink, hard_cap_bytes)
            },
        )?;
        Ok(self)
    }

    fn map_chapter_read_error(
        &self,
        chapter_href: &str,
        chapter_index: usize,
        err: EpubError,
    ) -> RenderPrepError {
        match err {
            EpubError::Zip(ZipError::FileTooLarge) | EpubError::LimitExceeded { .. } => {
                RenderPrepError::new_with_phase(
                    ErrorPhase::Parse,
                    "ENTRY_BYTES_LIMIT",
                    format!(
                        "Chapter entry exceeds max_entry_bytes (> {})",
                        self.opts.memory.max_entry_bytes
                    ),
                )
                .with_path(chapter_href.to_string())
                .with_chapter_index(chapter_index)
                .with_limit(
                    "max_entry_bytes",
                    self.opts.memory.max_entry_bytes.saturating_add(1),
                    self.opts.memory.max_entry_bytes,
                )
            }
            other => RenderPrepError::new_with_phase(
                ErrorPhase::Parse,
                "BOOK_CHAPTER_HTML",
                other.to_string(),
            )
            .with_path(chapter_href.to_string())
            .with_chapter_index(chapter_index),
        }
    }

    fn open_chapter_reader_with_budget<'a, R: std::io::Read + std::io::Seek>(
        &self,
        book: &'a mut EpubBook<R>,
        chapter_href: &str,
        chapter_index: usize,
    ) -> Result<BufReader<crate::zip::ZipEntryReader<'a, R>>, RenderPrepError> {
        book.open_resource_reader_with_hard_cap(chapter_href, self.opts.memory.max_entry_bytes)
            .map(BufReader::new)
            .map_err(|err| self.map_chapter_read_error(chapter_href, chapter_index, err))
    }

    fn scan_chapter_assets_from_reader<Rd: BufRead>(
        &self,
        chapter_href: &str,
        reader: Rd,
    ) -> Result<(Vec<String>, Vec<String>), RenderPrepError> {
        let mut xml = Reader::from_reader(reader);
        xml.config_mut().trim_text(true);
        let mut buf = Vec::with_capacity(8);
        let mut stylesheet_links = Vec::with_capacity(8);
        let mut image_sources = Vec::with_capacity(8);

        loop {
            match xml.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    let name = e.name();
                    let local = local_name_bytes(name.as_ref());
                    if local.eq_ignore_ascii_case(b"link") {
                        let mut href_raw: Option<Vec<u8>> = None;
                        let mut rel_is_stylesheet = false;
                        for attr in e.attributes().flatten() {
                            if attr_key_matches(attr.key.as_ref(), b"href") {
                                href_raw = Some(attr.value.as_ref().to_vec());
                            } else if attr_key_matches(attr.key.as_ref(), b"rel") {
                                rel_is_stylesheet = value_has_token_ignore_ascii_case(
                                    attr.value.as_ref(),
                                    b"stylesheet",
                                );
                            }
                        }
                        if rel_is_stylesheet {
                            if let Some(href_raw) = href_raw {
                                if let Ok(href) = xml.decoder().decode(href_raw.as_slice()) {
                                    if !href.is_empty() {
                                        stylesheet_links
                                            .push(resolve_relative(chapter_href, href.as_ref()));
                                    }
                                }
                            }
                        }
                    } else if local.eq_ignore_ascii_case(b"img")
                        || local.eq_ignore_ascii_case(b"image")
                    {
                        if let Some(src) = image_src_from_start(&xml, true, &e) {
                            image_sources.push(resolve_relative(chapter_href, &src));
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(err) => {
                    return Err(RenderPrepError::new(
                        "STYLE_TOKENIZE_ERROR",
                        format!("XML error: {:?}", err),
                    )
                    .with_phase(ErrorPhase::Style)
                    .with_source("xml tokenizer")
                    .with_token_offset(reader_token_offset(&xml)));
                }
            }
            buf.clear();
        }

        stylesheet_links.sort_unstable();
        stylesheet_links.dedup();
        image_sources.sort_unstable();
        image_sources.dedup();
        Ok((stylesheet_links, image_sources))
    }

    fn apply_chapter_stylesheets_with_budget<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        chapter_href: &str,
        html: &[u8],
    ) -> Result<(), RenderPrepError> {
        let mut scratch = Vec::with_capacity(8);
        self.apply_chapter_stylesheets_with_budget_scratch(
            book,
            chapter_index,
            chapter_href,
            html,
            &mut scratch,
        )
    }

    fn apply_chapter_stylesheets_with_budget_scratch<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        chapter_href: &str,
        html: &[u8],
        scratch_buf: &mut Vec<u8>,
    ) -> Result<(), RenderPrepError> {
        let links = parse_stylesheet_links_bytes(chapter_href, html);
        self.apply_stylesheet_links_with_budget_scratch(book, chapter_index, &links, scratch_buf)
    }

    fn apply_stylesheet_links_with_budget_scratch<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        chapter_index: usize,
        links: &[String],
        scratch_buf: &mut Vec<u8>,
    ) -> Result<(), RenderPrepError> {
        self.styler.clear_stylesheets();
        let css_limit = min(
            self.opts.style.limits.max_css_bytes,
            self.opts.memory.max_css_bytes,
        );
        scratch_buf.clear();
        for href in links.iter() {
            scratch_buf.clear();
            book.read_resource_into_with_hard_cap(href, scratch_buf, css_limit)
                .map_err(|e| {
                    RenderPrepError::new_with_phase(
                        ErrorPhase::Parse,
                        "BOOK_CHAPTER_STYLESHEET_READ",
                        e.to_string(),
                    )
                    .with_path(href.clone())
                    .with_chapter_index(chapter_index)
                })?;
            if scratch_buf.len() > css_limit {
                return Err(RenderPrepError::new_with_phase(
                    ErrorPhase::Parse,
                    "STYLE_CSS_TOO_LARGE",
                    format!(
                        "Stylesheet exceeds max_css_bytes ({} > {})",
                        scratch_buf.len(),
                        css_limit
                    ),
                )
                .with_path(href.clone())
                .with_chapter_index(chapter_index)
                .with_limit("max_css_bytes", scratch_buf.len(), css_limit));
            }
            let css = core::str::from_utf8(scratch_buf).map_err(|_| {
                RenderPrepError::new_with_phase(
                    ErrorPhase::Parse,
                    "STYLE_CSS_NOT_UTF8",
                    format!("Stylesheet is not UTF-8: {}", href),
                )
                .with_path(href.clone())
                .with_chapter_index(chapter_index)
            })?;
            self.styler
                .push_stylesheet_source(href, css)
                .map_err(|e| e.with_chapter_index(chapter_index))?;
        }
        Ok(())
    }

    fn collect_intrinsic_image_dimensions<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        chapter_href: &str,
        html: &[u8],
    ) -> Vec<(String, (u16, u16))> {
        let sources = collect_image_sources_from_html(chapter_href, html);
        let mut out = Vec::with_capacity(sources.len());
        self.collect_intrinsic_image_dimensions_from_sources(book, &sources, &mut out);
        out
    }

    fn collect_intrinsic_image_dimensions_from_sources<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        sources: &[String],
        out: &mut Vec<(String, (u16, u16))>,
    ) {
        for src in sources.iter() {
            if let Some((w, h)) = self.resolve_intrinsic_image_dimensions(book, src) {
                image_dims_upsert_sorted(out, resource_path_without_fragment(src), (w, h));
            }
        }
    }

    fn resolve_intrinsic_image_dimensions<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        src: &str,
    ) -> Option<(u16, u16)> {
        let key = resource_path_without_fragment(src);
        if let Some(cached) = cache_lookup_sorted(&self.image_dimension_cache, key) {
            return Some(cached);
        }

        // Probe only a bounded header prefix through a streaming reader; no full
        // image materialization for intrinsic dimension inference.
        let cap = self
            .opts
            .memory
            .max_entry_bytes
            .clamp(1, IMAGE_DIMENSION_PROBE_MAX_BYTES);
        self.image_probe_scratch.clear();
        let dimensions = match book.open_resource_reader(key) {
            Ok(mut reader) => {
                let mut chunk = [0u8; IMAGE_DIMENSION_PROBE_CHUNK_BYTES];
                let mut found = None;
                while self.image_probe_scratch.len() < cap {
                    let remaining = cap.saturating_sub(self.image_probe_scratch.len());
                    let take = remaining.min(chunk.len());
                    if take == 0 {
                        break;
                    }
                    match std::io::Read::read(&mut reader, &mut chunk[..take]) {
                        Ok(0) => break,
                        Ok(n) => {
                            self.image_probe_scratch.extend_from_slice(&chunk[..n]);
                            if let Some(dimensions) =
                                infer_image_dimensions_from_bytes(&self.image_probe_scratch)
                            {
                                found = Some(dimensions);
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                found
            }
            Err(_) => None,
        };
        cache_upsert_sorted(&mut self.image_dimension_cache, key, dimensions);
        dimensions
    }

    /// Register fonts from any external source with a byte loader callback.
    pub fn with_registered_fonts<I, F>(
        mut self,
        fonts: I,
        mut loader: F,
    ) -> Result<Self, RenderPrepError>
    where
        I: IntoIterator<Item = EmbeddedFontFace>,
        F: FnMut(&str) -> Result<Vec<u8>, EpubError>,
    {
        self.font_resolver
            .register_epub_fonts(fonts, |href| loader(href))?;
        Ok(self)
    }

    /// Prepare a chapter into styled runs/events.
    pub fn prepare_chapter<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
    ) -> Result<PreparedChapter, RenderPrepError> {
        let mut items = Vec::with_capacity(8);
        self.prepare_chapter_with(book, index, |item| items.push(item))?;
        Ok(PreparedChapter {
            styled: StyledChapter::from_items(items),
        })
    }

    /// Prepare a chapter and append results into an output buffer.
    pub fn prepare_chapter_into<R: std::io::Read + std::io::Seek>(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        out: &mut Vec<StyledEventOrRun>,
    ) -> Result<(), RenderPrepError> {
        self.prepare_chapter_with(book, index, |item| out.push(item))
    }

    /// Prepare a chapter and stream each styled item via callback.
    pub fn prepare_chapter_with<R: std::io::Read + std::io::Seek, F: FnMut(StyledEventOrRun)>(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        mut on_item: F,
    ) -> Result<(), RenderPrepError> {
        let chapter = book.chapter(index).map_err(|e| {
            RenderPrepError::new_with_phase(ErrorPhase::Parse, "BOOK_CHAPTER_REF", e.to_string())
                .with_chapter_index(index)
        })?;
        let chapter_href = chapter.href;
        let (stylesheet_links, image_sources) = {
            let reader = self.open_chapter_reader_with_budget(book, &chapter_href, index)?;
            self.scan_chapter_assets_from_reader(&chapter_href, reader)?
        };
        let mut stylesheet_scratch = Vec::with_capacity(8);
        self.apply_stylesheet_links_with_budget_scratch(
            book,
            index,
            &stylesheet_links,
            &mut stylesheet_scratch,
        )?;
        let mut image_dimensions = Vec::with_capacity(image_sources.len());
        self.collect_intrinsic_image_dimensions_from_sources(
            book,
            &image_sources,
            &mut image_dimensions,
        );
        let font_resolver = &self.font_resolver;
        let chapter_href_ref = chapter_href.as_str();
        let reader = self.open_chapter_reader_with_budget(book, &chapter_href, index)?;
        self.styler.style_chapter_reader_with(reader, |item| {
            let item =
                resolve_item_assets_for_chapter(chapter_href_ref, Some(&image_dimensions), item);
            let item = resolve_item_with_font(font_resolver, item);
            on_item(item);
        })
    }

    /// Prepare a chapter from caller-provided XHTML bytes and stream each styled item.
    ///
    /// This avoids re-reading chapter bytes from the ZIP archive and is intended for
    /// embedded call sites that already own a reusable chapter buffer.
    #[inline(never)]
    pub fn prepare_chapter_bytes_with<
        R: std::io::Read + std::io::Seek,
        F: FnMut(StyledEventOrRun),
    >(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        html: &[u8],
        mut on_item: F,
    ) -> Result<(), RenderPrepError> {
        let chapter = book.chapter(index).map_err(|e| {
            RenderPrepError::new_with_phase(ErrorPhase::Parse, "BOOK_CHAPTER_REF", e.to_string())
                .with_chapter_index(index)
        })?;
        let chapter_href = chapter.href;
        if html.len() > self.opts.memory.max_entry_bytes {
            return Err(RenderPrepError::new_with_phase(
                ErrorPhase::Parse,
                "ENTRY_BYTES_LIMIT",
                format!(
                    "Chapter entry exceeds max_entry_bytes ({} > {})",
                    html.len(),
                    self.opts.memory.max_entry_bytes
                ),
            )
            .with_path(chapter_href.clone())
            .with_chapter_index(index)
            .with_limit(
                "max_entry_bytes",
                html.len(),
                self.opts.memory.max_entry_bytes,
            ));
        }
        self.apply_chapter_stylesheets_with_budget(book, index, &chapter_href, html)?;
        let image_dimensions =
            self.collect_intrinsic_image_dimensions(book, chapter_href.as_str(), html);
        let font_resolver = &self.font_resolver;
        let chapter_href_ref = chapter_href.as_str();
        self.styler.style_chapter_bytes_with(html, |item| {
            let item =
                resolve_item_assets_for_chapter(chapter_href_ref, Some(&image_dimensions), item);
            let item = resolve_item_with_font(font_resolver, item);
            on_item(item);
        })
    }

    /// Prepare chapter bytes with caller-provided stylesheet scratch.
    ///
    /// This avoids transient stylesheet `Vec<u8>` allocations by reusing `stylesheet_scratch`.
    #[inline(never)]
    pub fn prepare_chapter_bytes_with_scratch<
        R: std::io::Read + std::io::Seek,
        F: FnMut(StyledEventOrRun),
    >(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        html: &[u8],
        stylesheet_scratch: &mut Vec<u8>,
        mut on_item: F,
    ) -> Result<(), RenderPrepError> {
        let chapter = book.chapter(index).map_err(|e| {
            RenderPrepError::new_with_phase(ErrorPhase::Parse, "BOOK_CHAPTER_REF", e.to_string())
                .with_chapter_index(index)
        })?;
        let chapter_href = chapter.href;
        if html.len() > self.opts.memory.max_entry_bytes {
            return Err(RenderPrepError::new_with_phase(
                ErrorPhase::Parse,
                "ENTRY_BYTES_LIMIT",
                format!(
                    "Chapter entry exceeds max_entry_bytes ({} > {})",
                    html.len(),
                    self.opts.memory.max_entry_bytes
                ),
            )
            .with_path(chapter_href.clone())
            .with_chapter_index(index)
            .with_limit(
                "max_entry_bytes",
                html.len(),
                self.opts.memory.max_entry_bytes,
            ));
        }
        self.apply_chapter_stylesheets_with_budget_scratch(
            book,
            index,
            &chapter_href,
            html,
            stylesheet_scratch,
        )?;
        let font_resolver = &self.font_resolver;
        let chapter_href_ref = chapter_href.as_str();
        self.styler.style_chapter_bytes_with(html, |item| {
            let item = resolve_item_assets_for_chapter(chapter_href_ref, None, item);
            let item = resolve_item_with_font(font_resolver, item);
            on_item(item);
        })
    }

    /// Prepare a chapter and stream each styled item with structured trace context.
    pub fn prepare_chapter_with_trace_context<
        R: std::io::Read + std::io::Seek,
        F: FnMut(StyledEventOrRun, RenderPrepTrace),
    >(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        mut on_item: F,
    ) -> Result<(), RenderPrepError> {
        let chapter = book.chapter(index).map_err(|e| {
            RenderPrepError::new_with_phase(ErrorPhase::Parse, "BOOK_CHAPTER_REF", e.to_string())
                .with_chapter_index(index)
        })?;
        let chapter_href = chapter.href;
        let (stylesheet_links, image_sources) = {
            let reader = self.open_chapter_reader_with_budget(book, &chapter_href, index)?;
            self.scan_chapter_assets_from_reader(&chapter_href, reader)?
        };
        let mut stylesheet_scratch = Vec::with_capacity(8);
        self.apply_stylesheet_links_with_budget_scratch(
            book,
            index,
            &stylesheet_links,
            &mut stylesheet_scratch,
        )?;
        let mut image_dimensions = Vec::with_capacity(image_sources.len());
        self.collect_intrinsic_image_dimensions_from_sources(
            book,
            &image_sources,
            &mut image_dimensions,
        );
        let font_resolver = &self.font_resolver;
        let chapter_href_ref = chapter_href.as_str();
        let reader = self.open_chapter_reader_with_budget(book, &chapter_href, index)?;
        self.styler.style_chapter_reader_with(reader, |item| {
            let item =
                resolve_item_assets_for_chapter(chapter_href_ref, Some(&image_dimensions), item);
            let (item, trace) = resolve_item_with_font_trace(font_resolver, item);
            on_item(item, trace);
        })
    }

    /// Prepare a chapter and stream each styled item with optional font-resolution trace.
    #[deprecated(
        since = "0.2.0",
        note = "Use prepare_chapter_with_trace_context for stable structured trace output."
    )]
    pub fn prepare_chapter_with_trace<
        R: std::io::Read + std::io::Seek,
        F: FnMut(StyledEventOrRun, Option<FontResolutionTrace>),
    >(
        &mut self,
        book: &mut EpubBook<R>,
        index: usize,
        mut on_item: F,
    ) -> Result<(), RenderPrepError> {
        self.prepare_chapter_with_trace_context(book, index, |item, trace| {
            on_item(item, trace.font_trace().cloned());
        })
    }
}

/// Prepared chapter stream returned by render-prep.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedChapter {
    styled: StyledChapter,
}

impl PreparedChapter {
    /// Iterate full styled stream.
    pub fn iter(&self) -> impl Iterator<Item = &StyledEventOrRun> {
        self.styled.iter()
    }

    /// Iterate styled runs.
    pub fn runs(&self) -> impl Iterator<Item = &StyledRun> {
        self.styled.runs()
    }
}

#[derive(Clone, Debug, Default)]
struct ElementCtx {
    tag: Cow<'static, str>,
    classes: SmallVec<[String; 4]>,
    inline_style: Option<CssStyle>,
    img_src: Option<String>,
    img_alt: Option<String>,
    img_width_px: Option<u16>,
    img_height_px: Option<u16>,
}

fn reader_token_offset<Rd: BufRead>(reader: &Reader<Rd>) -> usize {
    usize::try_from(reader.buffer_position()).unwrap_or(usize::MAX)
}

fn first_non_empty_declaration_index(style_attr: &str) -> Option<usize> {
    style_attr
        .split(';')
        .enumerate()
        .find(|(_, decl)| !decl.trim().is_empty())
        .map(|(idx, _)| idx)
}

fn local_name_bytes(raw: &[u8]) -> &[u8] {
    raw.rsplit(|b| *b == b':').next().unwrap_or(raw)
}

fn attr_key_matches(raw: &[u8], expected: &[u8]) -> bool {
    local_name_bytes(raw).eq_ignore_ascii_case(expected)
}

fn decode_tag_name<Rd: BufRead>(
    reader: &Reader<Rd>,
    raw: &[u8],
) -> Result<Cow<'static, str>, RenderPrepError> {
    let local_raw = local_name_bytes(raw);
    if let Some(common) = canonical_tag_name_bytes(local_raw) {
        return Ok(Cow::Borrowed(common));
    }
    let decoded = reader.decoder().decode(raw).map_err(|err| {
        RenderPrepError::new_with_phase(
            ErrorPhase::Style,
            "STYLE_TOKENIZE_ERROR",
            format!("Decode error: {:?}", err),
        )
        .with_source("tag name decode")
        .with_token_offset(reader_token_offset(reader))
    })?;
    let local_name = decoded.rsplit(':').next().unwrap_or(decoded.as_ref());
    Ok(Cow::Owned(local_name.to_ascii_lowercase()))
}

fn canonical_tag_name_bytes(raw: &[u8]) -> Option<&'static str> {
    if raw.eq_ignore_ascii_case(b"p") {
        return Some("p");
    }
    if raw.eq_ignore_ascii_case(b"div") {
        return Some("div");
    }
    if raw.eq_ignore_ascii_case(b"figure") {
        return Some("figure");
    }
    if raw.eq_ignore_ascii_case(b"figcaption") {
        return Some("figcaption");
    }
    if raw.eq_ignore_ascii_case(b"table") {
        return Some("table");
    }
    if raw.eq_ignore_ascii_case(b"tr") {
        return Some("tr");
    }
    if raw.eq_ignore_ascii_case(b"td") {
        return Some("td");
    }
    if raw.eq_ignore_ascii_case(b"th") {
        return Some("th");
    }
    if raw.eq_ignore_ascii_case(b"br") {
        return Some("br");
    }
    if raw.eq_ignore_ascii_case(b"pre") {
        return Some("pre");
    }
    if raw.eq_ignore_ascii_case(b"textarea") {
        return Some("textarea");
    }
    if raw.eq_ignore_ascii_case(b"li") {
        return Some("li");
    }
    if raw.eq_ignore_ascii_case(b"h1") {
        return Some("h1");
    }
    if raw.eq_ignore_ascii_case(b"h2") {
        return Some("h2");
    }
    if raw.eq_ignore_ascii_case(b"h3") {
        return Some("h3");
    }
    if raw.eq_ignore_ascii_case(b"h4") {
        return Some("h4");
    }
    if raw.eq_ignore_ascii_case(b"h5") {
        return Some("h5");
    }
    if raw.eq_ignore_ascii_case(b"h6") {
        return Some("h6");
    }
    if raw.eq_ignore_ascii_case(b"img") {
        return Some("img");
    }
    if raw.eq_ignore_ascii_case(b"image") {
        return Some("image");
    }
    if raw.eq_ignore_ascii_case(b"script") {
        return Some("script");
    }
    if raw.eq_ignore_ascii_case(b"style") {
        return Some("style");
    }
    if raw.eq_ignore_ascii_case(b"head") {
        return Some("head");
    }
    if raw.eq_ignore_ascii_case(b"noscript") {
        return Some("noscript");
    }
    if raw.eq_ignore_ascii_case(b"strong") {
        return Some("strong");
    }
    if raw.eq_ignore_ascii_case(b"b") {
        return Some("b");
    }
    if raw.eq_ignore_ascii_case(b"em") {
        return Some("em");
    }
    if raw.eq_ignore_ascii_case(b"i") {
        return Some("i");
    }
    if raw.eq_ignore_ascii_case(b"code") {
        return Some("code");
    }
    if raw.eq_ignore_ascii_case(b"kbd") {
        return Some("kbd");
    }
    if raw.eq_ignore_ascii_case(b"samp") {
        return Some("samp");
    }
    if raw.eq_ignore_ascii_case(b"svg") {
        return Some("svg");
    }
    if raw.eq_ignore_ascii_case(b"link") {
        return Some("link");
    }
    None
}

fn element_ctx_from_start(
    reader: &Reader<impl BufRead>,
    e: &quick_xml::events::BytesStart<'_>,
    tag: Cow<'static, str>,
    max_inline_style_bytes: usize,
) -> Result<ElementCtx, RenderPrepError> {
    let mut classes = SmallVec::<[String; 4]>::with_capacity(4);
    let mut inline_style = None;
    let mut img_src: Option<String> = None;
    let mut img_alt: Option<String> = None;
    let mut img_width_px: Option<u16> = None;
    let mut img_height_px: Option<u16> = None;
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        if attr_key_matches(key, b"class") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            classes.clear();
            classes.extend(
                value
                    .split_whitespace()
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(ToString::to_string),
            );
            continue;
        }
        if attr_key_matches(key, b"style") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            let value = value.into_owned();
            if value.len() > max_inline_style_bytes {
                let mut prep_err = RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "STYLE_INLINE_BYTES_LIMIT",
                    format!(
                        "Inline style exceeds max_inline_style_bytes ({} > {})",
                        value.len(),
                        max_inline_style_bytes
                    ),
                )
                .with_source(format!("inline style on <{}>", tag))
                .with_declaration(value.clone())
                .with_token_offset(reader_token_offset(reader))
                .with_limit(
                    "max_inline_style_bytes",
                    value.len(),
                    max_inline_style_bytes,
                );
                if let Some(declaration_index) = first_non_empty_declaration_index(&value) {
                    prep_err = prep_err.with_declaration_index(declaration_index);
                }
                return Err(prep_err);
            }
            let parsed = parse_inline_style(&value).map_err(|err| {
                let mut prep_err = RenderPrepError::new_with_phase(
                    ErrorPhase::Style,
                    "STYLE_INLINE_PARSE_ERROR",
                    err.to_string(),
                )
                .with_source(format!("inline style on <{}>", tag))
                .with_declaration(value.clone())
                .with_token_offset(reader_token_offset(reader));
                if let Some(declaration_index) = first_non_empty_declaration_index(&value) {
                    prep_err = prep_err.with_declaration_index(declaration_index);
                }
                prep_err
            })?;
            inline_style = Some(parsed);
            continue;
        }
        if attr_key_matches(key, b"src")
            || (matches!(tag.as_ref(), "img" | "image") && attr_key_matches(key, b"href"))
        {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            if !value.is_empty() {
                img_src = Some(value.into_owned());
            }
            continue;
        }
        if attr_key_matches(key, b"alt") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            img_alt = Some(value.into_owned());
            continue;
        }
        if attr_key_matches(key, b"title") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            if img_alt.is_none() && !value.is_empty() {
                img_alt = Some(value.into_owned());
            }
            continue;
        }
        if attr_key_matches(key, b"width") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            img_width_px = parse_dimension_hint_px(value.as_ref());
            continue;
        }
        if attr_key_matches(key, b"height") {
            let Ok(value) = reader.decoder().decode(attr.value.as_ref()) else {
                continue;
            };
            img_height_px = parse_dimension_hint_px(value.as_ref());
        }
    }
    Ok(ElementCtx {
        tag,
        classes,
        inline_style,
        img_src,
        img_alt,
        img_width_px,
        img_height_px,
    })
}

fn parse_dimension_hint_px(raw: &str) -> Option<u16> {
    let trimmed = raw.trim().trim_end_matches("px").trim();
    let parsed = trimmed.parse::<u32>().ok()?;
    if parsed == 0 || parsed > u16::MAX as u32 {
        return None;
    }
    Some(parsed as u16)
}

fn emit_image_event<F: FnMut(StyledEventOrRun)>(
    ctx: &ElementCtx,
    in_figure: bool,
    on_item: &mut F,
) {
    if !matches!(ctx.tag.as_ref(), "img" | "image") {
        return;
    }
    let Some(src) = ctx.img_src.clone() else {
        return;
    };
    on_item(StyledEventOrRun::Image(StyledImage {
        src,
        alt: ctx.img_alt.clone().unwrap_or_default(),
        width_px: ctx.img_width_px,
        height_px: ctx.img_height_px,
        in_figure,
    }));
}

fn emit_start_event<F: FnMut(StyledEventOrRun)>(tag: &str, on_item: &mut F) {
    match tag {
        "p" | "div" | "figure" | "figcaption" | "table" | "tr" | "pre" | "textarea" => {
            on_item(StyledEventOrRun::Event(StyledEvent::ParagraphStart))
        }
        "li" => on_item(StyledEventOrRun::Event(StyledEvent::ListItemStart)),
        "h1" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(1))),
        "h2" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(2))),
        "h3" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(3))),
        "h4" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(4))),
        "h5" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(5))),
        "h6" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingStart(6))),
        _ => {}
    }
}

fn emit_end_event<F: FnMut(StyledEventOrRun)>(tag: &str, on_item: &mut F) {
    match tag {
        "p" | "div" | "figure" | "figcaption" | "table" | "tr" | "pre" | "textarea" => {
            on_item(StyledEventOrRun::Event(StyledEvent::ParagraphEnd))
        }
        "li" => on_item(StyledEventOrRun::Event(StyledEvent::ListItemEnd)),
        "h1" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(1))),
        "h2" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(2))),
        "h3" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(3))),
        "h4" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(4))),
        "h5" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(5))),
        "h6" => on_item(StyledEventOrRun::Event(StyledEvent::HeadingEnd(6))),
        _ => {}
    }
}

fn role_from_tag(tag: &str) -> Option<BlockRole> {
    match tag {
        "p" | "div" => Some(BlockRole::Paragraph),
        "li" => Some(BlockRole::ListItem),
        "figcaption" => Some(BlockRole::FigureCaption),
        "pre" | "textarea" | "code" | "kbd" | "samp" => Some(BlockRole::Preformatted),
        "h1" => Some(BlockRole::Heading(1)),
        "h2" => Some(BlockRole::Heading(2)),
        "h3" => Some(BlockRole::Heading(3)),
        "h4" => Some(BlockRole::Heading(4)),
        "h5" => Some(BlockRole::Heading(5)),
        "h6" => Some(BlockRole::Heading(6)),
        _ => None,
    }
}

fn should_skip_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "head" | "noscript")
}

fn is_preformatted_context(stack: &[ElementCtx]) -> bool {
    stack.iter().any(|ctx| {
        matches!(
            ctx.tag.as_ref(),
            "pre" | "code" | "kbd" | "samp" | "textarea"
        )
    })
}

fn normalize_plain_text_whitespace(text: &str, preserve: bool) -> Cow<'_, str> {
    if preserve {
        return Cow::Borrowed(text);
    }
    if text.chars().all(char::is_whitespace) {
        return Cow::Borrowed("");
    }
    let trimmed = text.trim_matches(char::is_whitespace);
    if trimmed.is_empty() {
        return Cow::Borrowed("");
    }
    // Fast path: already normalized ASCII single-space text.
    let mut prev_space = false;
    let mut needs_rewrite = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if prev_space || ch != ' ' {
                needs_rewrite = true;
                break;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
    }
    if !needs_rewrite {
        return Cow::Borrowed(trimmed);
    }
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
                prev_space = true;
            }
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    Cow::Owned(result)
}

fn normalize_family(family: &str) -> String {
    normalized_family_slice(family).to_ascii_lowercase()
}

fn normalized_family_slice(family: &str) -> &str {
    family.trim().trim_matches('"').trim_matches('\'')
}

fn family_names_match(requested: &str, candidate: &str) -> bool {
    normalized_family_slice(requested).eq_ignore_ascii_case(normalized_family_slice(candidate))
}

fn has_non_ascii(text: &str) -> bool {
    !text.is_ascii()
}

fn flush_pending_run<F: FnMut(StyledEventOrRun)>(
    pending_run: &mut Option<StyledRun>,
    on_item: &mut F,
) {
    if let Some(run) = pending_run.take() {
        on_item(StyledEventOrRun::Run(run));
    }
}

fn emit_styled_run_text<F: FnMut(StyledEventOrRun)>(
    pending_run: &mut Option<StyledRun>,
    text: Cow<'_, str>,
    style: ComputedTextStyle,
    font_id: u32,
    on_item: &mut F,
) {
    if let Some(prev) = pending_run.as_mut() {
        let next_text = text.as_ref();
        let keep_separate = prev.text == " | " || next_text == " | ";
        if prev.style == style && !keep_separate {
            if !next_text.is_empty() {
                if !prev.text.is_empty() && !matches!(style.block_role, BlockRole::Preformatted) {
                    prev.text.push(' ');
                }
                prev.text.push_str(next_text);
            }
            return;
        }
    }
    flush_pending_run(pending_run, on_item);
    *pending_run = Some(StyledRun {
        text: text.into_owned(),
        style,
        font_id,
    });
}

fn emit_styled_item<F: FnMut(StyledEventOrRun)>(
    pending_run: &mut Option<StyledRun>,
    item: StyledEventOrRun,
    on_item: &mut F,
) {
    match item {
        StyledEventOrRun::Run(run) => emit_styled_run_text(
            pending_run,
            Cow::Owned(run.text),
            run.style,
            run.font_id,
            on_item,
        ),
        other => {
            flush_pending_run(pending_run, on_item);
            on_item(other);
        }
    }
}

fn resolve_item_with_font(
    font_resolver: &FontResolver,
    item: StyledEventOrRun,
) -> StyledEventOrRun {
    match item {
        StyledEventOrRun::Run(mut run) => {
            run.font_id = font_resolver.resolve_font_id(&run.style);
            StyledEventOrRun::Run(run)
        }
        StyledEventOrRun::Event(event) => StyledEventOrRun::Event(event),
        StyledEventOrRun::Image(image) => StyledEventOrRun::Image(image),
    }
}

fn resolve_item_with_font_trace(
    font_resolver: &FontResolver,
    item: StyledEventOrRun,
) -> (StyledEventOrRun, RenderPrepTrace) {
    match item {
        StyledEventOrRun::Run(mut run) => {
            let trace = font_resolver.resolve_with_trace_for_text(&run.style, Some(&run.text));
            run.font_id = trace.face.font_id;
            let style = run.style.clone();
            (
                StyledEventOrRun::Run(run),
                RenderPrepTrace::Run {
                    style: Box::new(style),
                    font: Box::new(trace),
                },
            )
        }
        StyledEventOrRun::Event(event) => (StyledEventOrRun::Event(event), RenderPrepTrace::Event),
        StyledEventOrRun::Image(image) => (StyledEventOrRun::Image(image), RenderPrepTrace::Event),
    }
}

fn resolve_item_assets_for_chapter(
    chapter_href: &str,
    image_dimensions: Option<&[(String, (u16, u16))]>,
    mut item: StyledEventOrRun,
) -> StyledEventOrRun {
    if let StyledEventOrRun::Image(image) = &mut item {
        image.src = resolve_relative(chapter_href, &image.src);
        if let Some(dimensions) = image_dimensions {
            let key = resource_path_without_fragment(&image.src);
            if let Some((intrinsic_w, intrinsic_h)) = image_dims_lookup_sorted(dimensions, key) {
                match (image.width_px, image.height_px) {
                    (None, None) => {
                        image.width_px = Some(intrinsic_w);
                        image.height_px = Some(intrinsic_h);
                    }
                    (Some(width), None) if intrinsic_w > 0 => {
                        let ratio = intrinsic_h as f32 / intrinsic_w as f32;
                        let resolved = ((width as f32) * ratio).round();
                        image.height_px = bounded_nonzero_u16_f32(resolved);
                    }
                    (None, Some(height)) if intrinsic_h > 0 => {
                        let ratio = intrinsic_w as f32 / intrinsic_h as f32;
                        let resolved = ((height as f32) * ratio).round();
                        image.width_px = bounded_nonzero_u16_f32(resolved);
                    }
                    _ => {}
                }
            }
        }
    }
    item
}

fn split_family_stack(value: &str) -> SmallVec<[String; 2]> {
    value
        .split(',')
        .map(|part| part.trim().trim_matches('"').trim_matches('\''))
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

pub(crate) fn resolve_relative(base_path: &str, rel: &str) -> String {
    if rel.contains("://") {
        return rel.to_string();
    }
    if rel.starts_with('/') {
        return normalize_path(rel.trim_start_matches('/'));
    }
    let base_dir = base_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    if base_dir.is_empty() {
        normalize_path(rel)
    } else {
        normalize_path(&format!("{}/{}", base_dir, rel))
    }
}

fn resource_path_without_fragment(path: &str) -> &str {
    path.split('#').next().unwrap_or(path)
}

fn image_dims_lookup_sorted(dims: &[(String, (u16, u16))], key: &str) -> Option<(u16, u16)> {
    dims.binary_search_by(|(k, _)| k.as_str().cmp(key))
        .ok()
        .map(|idx| dims[idx].1)
}

fn image_dims_upsert_sorted(dims: &mut Vec<(String, (u16, u16))>, key: &str, value: (u16, u16)) {
    match dims.binary_search_by(|(k, _)| k.as_str().cmp(key)) {
        Ok(idx) => dims[idx].1 = value,
        Err(idx) => dims.insert(idx, (key.to_string(), value)),
    }
}

fn cache_lookup_sorted(cache: &[(String, Option<(u16, u16)>)], key: &str) -> Option<(u16, u16)> {
    cache
        .binary_search_by(|(k, _)| k.as_str().cmp(key))
        .ok()
        .and_then(|idx| cache[idx].1)
}

fn cache_upsert_sorted(
    cache: &mut Vec<(String, Option<(u16, u16)>)>,
    key: &str,
    value: Option<(u16, u16)>,
) {
    match cache.binary_search_by(|(k, _)| k.as_str().cmp(key)) {
        Ok(idx) => cache[idx].1 = value,
        Err(idx) => cache.insert(idx, (key.to_string(), value)),
    }
}

fn bounded_nonzero_u16(value: u32) -> Option<u16> {
    if value == 0 || value > u16::MAX as u32 {
        None
    } else {
        Some(value as u16)
    }
}

fn bounded_nonzero_u16_f32(value: f32) -> Option<u16> {
    if !value.is_finite() {
        return None;
    }
    let rounded = value.round();
    if rounded <= 0.0 || rounded > u16::MAX as f32 {
        None
    } else {
        Some(rounded as u16)
    }
}

fn collect_image_sources_from_html(chapter_href: &str, html: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(html);
    let mut buf = Vec::with_capacity(8);
    let mut out = Vec::with_capacity(8);

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.name();
                let tag_name = local_name_bytes(name.as_ref());
                if !tag_name.eq_ignore_ascii_case(b"img")
                    && !tag_name.eq_ignore_ascii_case(b"image")
                {
                    buf.clear();
                    continue;
                }
                if let Some(src) = image_src_from_start(&reader, true, &e) {
                    out.push(resolve_relative(chapter_href, &src));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }

    out.sort_unstable();
    out.dedup();
    out
}

fn image_src_from_start(
    reader: &Reader<impl BufRead>,
    image_tag: bool,
    start: &quick_xml::events::BytesStart<'_>,
) -> Option<String> {
    for attr in start.attributes().flatten() {
        let key = attr.key.as_ref();
        if attr_key_matches(key, b"src") || (image_tag && attr_key_matches(key, b"href")) {
            let value = match reader.decoder().decode(attr.value.as_ref()) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if !value.is_empty() {
                return Some(value.into_owned());
            }
        }
    }
    None
}

fn infer_image_dimensions_from_bytes(bytes: &[u8]) -> Option<(u16, u16)> {
    infer_png_dimensions(bytes)
        .or_else(|| infer_jpeg_dimensions(bytes))
        .or_else(|| infer_gif_dimensions(bytes))
        .or_else(|| infer_webp_dimensions(bytes))
        .or_else(|| infer_svg_dimensions(bytes))
}

fn infer_png_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    Some((bounded_nonzero_u16(width)?, bounded_nonzero_u16(height)?))
}

fn infer_gif_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 10 {
        return None;
    }
    if &bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a" {
        return None;
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]);
    let height = u16::from_le_bytes([bytes[8], bytes[9]]);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

fn infer_jpeg_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 1 < bytes.len() {
        while i < bytes.len() && bytes[i] != 0xFF {
            i += 1;
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let marker = bytes[i];
        i += 1;

        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let seg_len = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if seg_len < 2 {
            break;
        }
        let payload_start = i + 2;
        let payload_end = i.saturating_add(seg_len);
        if payload_end > bytes.len() {
            break;
        }
        if is_jpeg_sof_marker(marker) && seg_len >= 7 {
            if payload_start + 4 >= bytes.len() {
                break;
            }
            let height =
                u16::from_be_bytes([bytes[payload_start + 1], bytes[payload_start + 2]]) as u32;
            let width =
                u16::from_be_bytes([bytes[payload_start + 3], bytes[payload_start + 4]]) as u32;
            return Some((bounded_nonzero_u16(width)?, bounded_nonzero_u16(height)?));
        }
        i = payload_end;
    }
    None
}

fn is_jpeg_sof_marker(marker: u8) -> bool {
    matches!(
        marker,
        0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
    )
}

fn infer_webp_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.len() < 16 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let chunk_tag = &bytes[offset..offset + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]) as usize;
        let payload_start = offset + 8;
        let payload_end = payload_start.saturating_add(chunk_len);
        if payload_end > bytes.len() {
            break;
        }

        match chunk_tag {
            b"VP8X" if chunk_len >= 10 => {
                let w_minus_1 = (bytes[payload_start + 4] as u32)
                    | ((bytes[payload_start + 5] as u32) << 8)
                    | ((bytes[payload_start + 6] as u32) << 16);
                let h_minus_1 = (bytes[payload_start + 7] as u32)
                    | ((bytes[payload_start + 8] as u32) << 8)
                    | ((bytes[payload_start + 9] as u32) << 16);
                return Some((
                    bounded_nonzero_u16(w_minus_1 + 1)?,
                    bounded_nonzero_u16(h_minus_1 + 1)?,
                ));
            }
            b"VP8L" if chunk_len >= 5 && bytes[payload_start] == 0x2F => {
                let bits = u32::from_le_bytes([
                    bytes[payload_start + 1],
                    bytes[payload_start + 2],
                    bytes[payload_start + 3],
                    bytes[payload_start + 4],
                ]);
                let width = (bits & 0x3FFF) + 1;
                let height = ((bits >> 14) & 0x3FFF) + 1;
                return Some((bounded_nonzero_u16(width)?, bounded_nonzero_u16(height)?));
            }
            b"VP8 " if chunk_len >= 10 => {
                if bytes[payload_start + 3..payload_start + 6] != [0x9D, 0x01, 0x2A] {
                    return None;
                }
                let width =
                    u16::from_le_bytes([bytes[payload_start + 6], bytes[payload_start + 7]])
                        & 0x3FFF;
                let height =
                    u16::from_le_bytes([bytes[payload_start + 8], bytes[payload_start + 9]])
                        & 0x3FFF;
                if width == 0 || height == 0 {
                    return None;
                }
                return Some((width, height));
            }
            _ => {}
        }

        offset = payload_end + (chunk_len & 1);
    }
    None
}

fn infer_svg_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    let mut reader = Reader::from_reader(bytes);
    let mut buf = Vec::with_capacity(8);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if !local_name_bytes(e.name().as_ref()).eq_ignore_ascii_case(b"svg") {
                    buf.clear();
                    continue;
                }
                let mut width = None;
                let mut height = None;
                let mut view_box = None;
                for attr in e.attributes().flatten() {
                    let value = match reader.decoder().decode(&attr.value) {
                        Ok(v) => v.to_string(),
                        Err(_) => continue,
                    };
                    if attr_key_matches(attr.key.as_ref(), b"width") {
                        width = parse_svg_length_px(&value);
                    } else if attr_key_matches(attr.key.as_ref(), b"height") {
                        height = parse_svg_length_px(&value);
                    } else if attr_key_matches(attr.key.as_ref(), b"viewBox")
                        || attr_key_matches(attr.key.as_ref(), b"viewbox")
                    {
                        view_box = parse_svg_view_box(&value);
                    }
                }
                if let (Some(w), Some(h)) = (width, height) {
                    return Some((bounded_nonzero_u16_f32(w)?, bounded_nonzero_u16_f32(h)?));
                }
                if let Some((w, h)) = view_box {
                    return Some((bounded_nonzero_u16_f32(w)?, bounded_nonzero_u16_f32(h)?));
                }
                return None;
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }
    None
}

fn parse_svg_length_px(raw: &str) -> Option<f32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.ends_with('%') {
        return None;
    }
    let mut boundary = 0usize;
    for (idx, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || matches!(ch, '+' | '-' | '.' | 'e' | 'E') {
            boundary = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if boundary == 0 {
        return None;
    }
    let value = trimmed[..boundary].trim().parse::<f32>().ok()?;
    let unit = trimmed[boundary..].trim().to_ascii_lowercase();
    let factor = match unit.as_str() {
        "" | "px" => 1.0,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "q" => 96.0 / 101.6,
        _ => return None,
    };
    Some(value * factor)
}

fn parse_svg_view_box(raw: &str) -> Option<(f32, f32)> {
    let mut nums = raw
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|part| !part.trim().is_empty())
        .filter_map(|part| part.trim().parse::<f32>().ok());
    let _min_x = nums.next()?;
    let _min_y = nums.next()?;
    let width = nums.next()?;
    let height = nums.next()?;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((width, height))
}

fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(8);
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

pub(crate) fn parse_stylesheet_links(chapter_href: &str, html: &str) -> Vec<String> {
    parse_stylesheet_links_bytes(chapter_href, html.as_bytes())
}

fn value_has_token_ignore_ascii_case(value: &[u8], expected_token: &[u8]) -> bool {
    value
        .split(|b| b.is_ascii_whitespace())
        .any(|token| !token.is_empty() && token.eq_ignore_ascii_case(expected_token))
}

pub(crate) fn parse_stylesheet_links_bytes(chapter_href: &str, html_bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::with_capacity(8);
    let mut reader = Reader::from_reader(html_bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(8);

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if !local_name_bytes(e.name().as_ref()).eq_ignore_ascii_case(b"link") {
                    buf.clear();
                    continue;
                }
                let mut href_raw: Option<Vec<u8>> = None;
                let mut rel_is_stylesheet = false;
                for attr in e.attributes().flatten() {
                    if attr_key_matches(attr.key.as_ref(), b"href") {
                        href_raw = Some(attr.value.as_ref().to_vec());
                    } else if attr_key_matches(attr.key.as_ref(), b"rel") {
                        rel_is_stylesheet =
                            value_has_token_ignore_ascii_case(attr.value.as_ref(), b"stylesheet");
                    }
                }
                if !rel_is_stylesheet {
                    buf.clear();
                    continue;
                }
                let Some(href_raw) = href_raw else {
                    buf.clear();
                    continue;
                };
                let href = match reader.decoder().decode(href_raw.as_slice()) {
                    Ok(v) => v,
                    Err(_) => {
                        buf.clear();
                        continue;
                    }
                };
                if !href.is_empty() {
                    out.push(resolve_relative(chapter_href, href.as_ref()));
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }

    out
}

fn font_src_rank(path: &str) -> u8 {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".ttf") || lower.ends_with(".otf") {
        3
    } else if lower.ends_with(".woff2") {
        2
    } else if lower.ends_with(".woff") {
        1
    } else {
        0
    }
}

fn extract_font_face_src(css_href: &str, src_value: &str) -> Option<String> {
    let lower = src_value.to_ascii_lowercase();
    let mut search_from = 0usize;
    let mut best: Option<(u8, String)> = None;

    while let Some(idx) = lower[search_from..].find("url(") {
        let start = search_from + idx + 4;
        let tail = &src_value[start..];
        let Some(end) = tail.find(')') else {
            break;
        };
        let raw = tail[..end].trim().trim_matches('"').trim_matches('\'');
        if !raw.is_empty() && !raw.starts_with("data:") {
            let resolved = resolve_relative(css_href, raw);
            let rank = font_src_rank(&resolved);
            match &best {
                Some((best_rank, _)) if *best_rank >= rank => {}
                _ => best = Some((rank, resolved)),
            }
        }
        search_from = start + end + 1;
    }

    best.map(|(_, path)| path)
}

pub(crate) fn parse_font_faces_from_css(css_href: &str, css: &str) -> Vec<EmbeddedFontFace> {
    let mut out = Vec::with_capacity(8);
    let lower = css.to_ascii_lowercase();
    let mut search_from = 0usize;

    while let Some(idx) = lower[search_from..].find("@font-face") {
        let start = search_from + idx;
        let block_start = match css[start..].find('{') {
            Some(i) => start + i + 1,
            None => break,
        };
        let block_end = match css[block_start..].find('}') {
            Some(i) => block_start + i,
            None => break,
        };
        let block = &css[block_start..block_end];

        let mut family = None;
        let mut weight = 400u16;
        let mut style = EmbeddedFontStyle::Normal;
        let mut stretch = None;
        let mut href = None;
        let mut format_hint = None;

        for decl in block.split(';') {
            let decl = decl.trim();
            if decl.is_empty() {
                continue;
            }
            let Some(colon) = decl.find(':') else {
                continue;
            };
            let key = decl[..colon].trim().to_ascii_lowercase();
            let value = decl[colon + 1..].trim();
            match key.as_str() {
                "font-family" => {
                    let val = value.trim_matches('"').trim_matches('\'').trim();
                    if !val.is_empty() {
                        family = Some(val.to_string());
                    }
                }
                "font-weight" => {
                    let lower = value.to_ascii_lowercase();
                    weight = if lower == "bold" {
                        700
                    } else if lower == "normal" {
                        400
                    } else {
                        lower.parse::<u16>().unwrap_or(400)
                    };
                }
                "font-style" => {
                    let lower = value.to_ascii_lowercase();
                    style = if lower == "italic" {
                        EmbeddedFontStyle::Italic
                    } else if lower == "oblique" {
                        EmbeddedFontStyle::Oblique
                    } else {
                        EmbeddedFontStyle::Normal
                    };
                }
                "font-stretch" => {
                    if !value.is_empty() {
                        stretch = Some(value.to_string());
                    }
                }
                "src" => {
                    href = extract_font_face_src(css_href, value);
                    if let Some(fmt_idx) = value.to_ascii_lowercase().find("format(") {
                        let fmt_tail = &value[fmt_idx + 7..];
                        if let Some(end_paren) = fmt_tail.find(')') {
                            let raw = fmt_tail[..end_paren]
                                .trim()
                                .trim_matches('"')
                                .trim_matches('\'');
                            if !raw.is_empty() {
                                format_hint = Some(raw.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if let (Some(family), Some(href)) = (family, href) {
            out.push(EmbeddedFontFace {
                family,
                weight,
                style,
                stretch,
                href,
                format: format_hint,
            });
        }

        search_from = block_end + 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_tag_retains_semantic_elements() {
        assert!(!should_skip_tag("nav"));
        assert!(!should_skip_tag("header"));
        assert!(!should_skip_tag("footer"));
        assert!(!should_skip_tag("aside"));
        assert!(should_skip_tag("script"));
    }

    #[test]
    fn normalize_whitespace_preserves_preformatted_context() {
        let s = "a\n  b\t c";
        assert_eq!(normalize_plain_text_whitespace(s, true), s);
        assert_eq!(normalize_plain_text_whitespace(s, false), "a b c");
    }

    #[test]
    fn normalize_whitespace_borrows_trimmed_ascii_text() {
        let normalized = normalize_plain_text_whitespace("  alpha beta  ", false);
        assert!(matches!(normalized, Cow::Borrowed(_)));
        assert_eq!(normalized.as_ref(), "alpha beta");
    }

    #[test]
    fn normalize_whitespace_rewrites_non_space_whitespace() {
        let normalized = normalize_plain_text_whitespace("alpha\tbeta", false);
        assert!(matches!(normalized, Cow::Owned(_)));
        assert_eq!(normalized.as_ref(), "alpha beta");
    }

    #[test]
    fn parse_stylesheet_links_resolves_relative_paths() {
        let html = r#"<html><head>
<link rel="stylesheet" href="../styles/base.css"/>
<link rel="alternate stylesheet" href="theme.css"/>
</head></html>"#;
        let links = parse_stylesheet_links("text/ch1.xhtml", html);
        assert_eq!(links, vec!["styles/base.css", "text/theme.css"]);
    }

    #[test]
    fn parse_font_faces_prefers_ttf_otf_sources() {
        let css = r#"
@font-face {
  font-family: "Test";
  src: local("Test"), url("../fonts/test.woff2") format("woff2"), url("../fonts/test.ttf") format("truetype");
}
"#;
        let faces = parse_font_faces_from_css("styles/main.css", css);
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].href, "fonts/test.ttf");
    }

    #[test]
    fn parse_font_faces_extracts_basic_metadata() {
        let css = r#"
@font-face {
  font-family: 'Literata';
  font-style: italic;
  font-weight: 700;
  src: url('../fonts/Literata-Italic.woff2') format('woff2');
}
"#;
        let faces = parse_font_faces_from_css("styles/main.css", css);
        assert_eq!(faces.len(), 1);
        let face = &faces[0];
        assert_eq!(face.family, "Literata");
        assert_eq!(face.weight, 700);
        assert_eq!(face.style, EmbeddedFontStyle::Italic);
        assert_eq!(face.href, "fonts/Literata-Italic.woff2");
        assert_eq!(face.format.as_deref(), Some("woff2"));
    }

    #[test]
    fn styler_emits_runs_for_text() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<h1>Title</h1><p>Hello world</p>")
            .expect("style should succeed");
        assert!(chapter.runs().count() >= 2);
    }

    #[test]
    fn styler_run_font_id_defaults_to_zero() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p>Hello world</p>")
            .expect("style should succeed");
        let run = chapter.runs().next().expect("expected run");
        assert_eq!(run.font_id, 0);
    }

    #[test]
    fn styler_preserves_preformatted_run_whitespace_and_role() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<pre>alpha  beta\n  gamma</pre>")
            .expect("style should succeed");

        let run = chapter.runs().next().expect("expected preformatted run");
        assert_eq!(run.text, "alpha  beta\n  gamma");
        assert!(matches!(run.style.block_role, BlockRole::Preformatted));

        let starts = chapter
            .iter()
            .filter(|item| matches!(item, StyledEventOrRun::Event(StyledEvent::ParagraphStart)))
            .count();
        let ends = chapter
            .iter()
            .filter(|item| matches!(item, StyledEventOrRun::Event(StyledEvent::ParagraphEnd)))
            .count();
        assert!(starts >= 1);
        assert_eq!(starts, ends);
    }

    #[test]
    fn styler_style_chapter_with_streams_items() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let mut seen = 0usize;
        styler
            .style_chapter_with("<p>Hello</p>", |_item| {
                seen += 1;
            })
            .expect("style_chapter_with should succeed");
        assert!(seen > 0);
    }

    #[test]
    fn infer_image_dimensions_parses_common_formats() {
        let mut png = Vec::from(&b"\x89PNG\r\n\x1a\n"[..]);
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&960u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        assert_eq!(infer_image_dimensions_from_bytes(&png), Some((640, 960)));

        let gif = [
            b'G', b'I', b'F', b'8', b'9', b'a', 0x20, 0x03, 0x58, 0x02, 0, 0,
        ];
        assert_eq!(infer_image_dimensions_from_bytes(&gif), Some((800, 600)));

        let jpeg = [
            0xFF, 0xD8, // SOI
            0xFF, 0xE0, 0x00, 0x10, // APP0 len=16
            b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0, // APP0 payload
            0xFF, 0xC0, 0x00, 0x11, // SOF0 len=17
            0x08, // precision
            0x02, 0x58, // height 600
            0x03, 0x20, // width 800
            0x03, // components
            0x01, 0x11, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00, 0xFF, 0xD9,
        ];
        assert_eq!(infer_image_dimensions_from_bytes(&jpeg), Some((800, 600)));

        let mut webp = Vec::from(&b"RIFF"[..]);
        webp.extend_from_slice(&0u32.to_le_bytes());
        webp.extend_from_slice(b"WEBPVP8X");
        webp.extend_from_slice(&10u32.to_le_bytes());
        webp.extend_from_slice(&[0, 0, 0, 0]);
        let w_minus_1 = 799u32;
        webp.extend_from_slice(&[
            (w_minus_1 & 0xFF) as u8,
            ((w_minus_1 >> 8) & 0xFF) as u8,
            ((w_minus_1 >> 16) & 0xFF) as u8,
        ]);
        let h_minus_1 = 599u32;
        webp.extend_from_slice(&[
            (h_minus_1 & 0xFF) as u8,
            ((h_minus_1 >> 8) & 0xFF) as u8,
            ((h_minus_1 >> 16) & 0xFF) as u8,
        ]);
        assert_eq!(infer_image_dimensions_from_bytes(&webp), Some((800, 600)));

        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 480"></svg>"#;
        assert_eq!(infer_image_dimensions_from_bytes(svg), Some((320, 480)));
    }

    #[test]
    fn resolve_item_assets_uses_intrinsic_dimensions_when_missing() {
        let map = vec![("images/cover.jpg".to_string(), (600u16, 900u16))];
        let item = StyledEventOrRun::Image(StyledImage {
            src: "../images/cover.jpg".to_string(),
            alt: String::new(),
            width_px: None,
            height_px: Some(300),
            in_figure: false,
        });
        let resolved = resolve_item_assets_for_chapter("text/ch01.xhtml", Some(&map), item);
        let StyledEventOrRun::Image(image) = resolved else {
            panic!("expected image");
        };
        assert_eq!(image.src, "images/cover.jpg");
        assert_eq!(image.width_px, Some(200));
        assert_eq!(image.height_px, Some(300));
    }

    #[test]
    fn styler_emits_inline_image_event_with_dimension_hints() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p>Intro</p><img src=\"images/cover.jpg\" alt=\"Cover\" width=\"320\" height=\"480\"/>")
            .expect("style should succeed");

        let image = chapter
            .iter()
            .find_map(|item| match item {
                StyledEventOrRun::Image(img) => Some(img),
                _ => None,
            })
            .expect("expected image event");
        assert_eq!(image.src, "images/cover.jpg");
        assert_eq!(image.alt, "Cover");
        assert_eq!(image.width_px, Some(320));
        assert_eq!(image.height_px, Some(480));
    }

    #[test]
    fn styler_parses_px_dimension_hints_and_ignores_missing_src() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<img alt=\"No source\" width=\"80px\" height=\"60px\"/><img src=\"images/inline.png\" width=\"80px\" height=\"60px\"/>")
            .expect("style should succeed");

        let images: Vec<&StyledImage> = chapter
            .iter()
            .filter_map(|item| match item {
                StyledEventOrRun::Image(img) => Some(img),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].src, "images/inline.png");
        assert_eq!(images[0].width_px, Some(80));
        assert_eq!(images[0].height_px, Some(60));
    }

    #[test]
    fn styler_marks_images_inside_figure_and_figcaption_role() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter(
                "<figure><img src=\"images/inline.png\"/><figcaption>Figure caption</figcaption></figure>",
            )
            .expect("style should succeed");

        let image = chapter
            .iter()
            .find_map(|item| match item {
                StyledEventOrRun::Image(img) => Some(img),
                _ => None,
            })
            .expect("expected image");
        assert!(image.in_figure);

        let caption_run = chapter
            .runs()
            .find(|run| run.text.contains("Figure caption"))
            .expect("caption text run expected");
        assert!(matches!(
            caption_run.style.block_role,
            BlockRole::FigureCaption
        ));
    }

    #[test]
    fn styler_emits_svg_image_event_from_xlink_href() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter(
                "<p>Intro</p><svg><image xlink:href=\"images/cover.svg\" title=\"Vector cover\" width=\"240\" height=\"320\"></image></svg><p>Outro</p>",
            )
            .expect("style should succeed");

        let image = chapter
            .iter()
            .find_map(|item| match item {
                StyledEventOrRun::Image(img) => Some(img),
                _ => None,
            })
            .expect("expected svg image event");
        assert_eq!(image.src, "images/cover.svg");
        assert_eq!(image.alt, "Vector cover");
        assert_eq!(image.width_px, Some(240));
        assert_eq!(image.height_px, Some(320));
        assert!(chapter.runs().any(|run| run.text == "Outro"));
    }

    #[test]
    fn styler_linearizes_basic_table_rows_and_cells() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let chapter = styler
            .style_chapter(
                "<table><tr><th>Col A</th><th>Col B</th></tr><tr><td>1</td><td>2</td></tr></table>",
            )
            .expect("style should succeed");

        let runs: Vec<&str> = chapter.runs().map(|run| run.text.as_str()).collect();
        assert!(runs.windows(3).any(|w| w == ["Col A", " | ", "Col B"]));
        assert!(runs.windows(3).any(|w| w == ["1", " | ", "2"]));
        let starts = chapter
            .iter()
            .filter(|item| matches!(item, StyledEventOrRun::Event(StyledEvent::ParagraphStart)))
            .count();
        let ends = chapter
            .iter()
            .filter(|item| matches!(item, StyledEventOrRun::Event(StyledEvent::ParagraphEnd)))
            .count();
        assert!(starts >= 2);
        assert_eq!(starts, ends);
    }

    #[test]
    fn styler_applies_class_and_inline_style() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets {
                sources: vec![StylesheetSource {
                    href: "main.css".to_string(),
                    css: ".intro { font-size: 20px; font-style: normal; }".to_string(),
                }],
            })
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p class=\"intro\" style=\"font-style: italic\">Hello</p>")
            .expect("style should succeed");
        let first = chapter.runs().next().expect("expected run");
        assert_eq!(first.style.size_px, 20.0);
        assert!(first.style.italic);
    }

    #[test]
    fn styler_propagates_stylesheet_letter_spacing_px() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets {
                sources: vec![StylesheetSource {
                    href: "main.css".to_string(),
                    css: "p { letter-spacing: 1.5px; }".to_string(),
                }],
            })
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p>Hello</p>")
            .expect("style should succeed");
        let first = chapter.runs().next().expect("expected run");
        assert_eq!(first.style.letter_spacing, 1.5);
    }

    #[test]
    fn styler_inline_letter_spacing_normal_overrides_parent() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets {
                sources: vec![StylesheetSource {
                    href: "main.css".to_string(),
                    css: "p { letter-spacing: 2px; }".to_string(),
                }],
            })
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p style=\"letter-spacing: normal\">Hello</p>")
            .expect("style should succeed");
        let first = chapter.runs().next().expect("expected run");
        assert_eq!(first.style.letter_spacing, 0.0);
    }

    #[test]
    fn styler_respects_stylesheet_precedence_order() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets {
                sources: vec![
                    StylesheetSource {
                        href: "a.css".to_string(),
                        css: "p { font-size: 12px; }".to_string(),
                    },
                    StylesheetSource {
                        href: "b.css".to_string(),
                        css: "p { font-size: 18px; }".to_string(),
                    },
                ],
            })
            .expect("load should succeed");
        let chapter = styler
            .style_chapter("<p>Hello</p>")
            .expect("style should succeed");
        let first = chapter.runs().next().expect("expected run");
        assert_eq!(first.style.size_px, 18.0);
    }

    #[test]
    fn styler_enforces_css_byte_limit() {
        let mut styler = Styler::new(StyleConfig {
            limits: StyleLimits {
                max_css_bytes: 4,
                ..StyleLimits::default()
            },
            hints: LayoutHints::default(),
        });
        let styles = ChapterStylesheets {
            sources: vec![StylesheetSource {
                href: "a.css".to_string(),
                css: "p { font-weight: bold; }".to_string(),
            }],
        };
        let err = styler.load_stylesheets(&styles).expect_err("should reject");
        assert_eq!(err.code, "STYLE_CSS_TOO_LARGE");
        assert_eq!(err.phase, ErrorPhase::Style);
        let limit = err.limit.expect("expected limit context");
        assert_eq!(limit.kind.as_ref(), "max_css_bytes");
        assert!(limit.actual > limit.limit);
    }

    #[test]
    fn styler_enforces_selector_limit() {
        let mut styler = Styler::new(StyleConfig {
            limits: StyleLimits {
                max_selectors: 1,
                ..StyleLimits::default()
            },
            hints: LayoutHints::default(),
        });
        let styles = ChapterStylesheets {
            sources: vec![StylesheetSource {
                href: "a.css".to_string(),
                css: "p { font-weight: bold; } h1 { font-style: italic; }".to_string(),
            }],
        };
        let err = styler.load_stylesheets(&styles).expect_err("should reject");
        assert_eq!(err.code, "STYLE_SELECTOR_LIMIT");
        assert_eq!(err.phase, ErrorPhase::Style);
        let limit = err.limit.expect("expected limit context");
        assert_eq!(limit.kind.as_ref(), "max_selectors");
        assert_eq!(limit.actual, 2);
        assert_eq!(limit.limit, 1);
        let ctx = err.context.expect("expected context");
        assert_eq!(ctx.selector_index, Some(1));
    }

    #[test]
    fn styler_enforces_inline_style_byte_limit() {
        let mut styler = Styler::new(StyleConfig::default()).with_memory_budget(MemoryBudget {
            max_inline_style_bytes: 8,
            ..MemoryBudget::default()
        });
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let err = styler
            .style_chapter("<p style=\"font-weight: bold\">Hi</p>")
            .expect_err("should reject oversized inline style");
        assert_eq!(err.code, "STYLE_INLINE_BYTES_LIMIT");
        assert_eq!(err.phase, ErrorPhase::Style);
        let limit = err.limit.expect("expected limit context");
        assert_eq!(limit.kind.as_ref(), "max_inline_style_bytes");
        assert!(limit.actual > limit.limit);
        let ctx = err.context.expect("expected context");
        assert!(ctx.declaration.is_some());
        assert!(ctx.token_offset.is_some());
    }

    #[test]
    fn style_tokenize_error_sets_token_offset_context() {
        let mut styler = Styler::new(StyleConfig::default());
        styler
            .load_stylesheets(&ChapterStylesheets::default())
            .expect("load should succeed");
        let err = styler
            .style_chapter("<p class=\"x></p>")
            .expect_err("should reject malformed xml");
        assert_eq!(err.code, "STYLE_TOKENIZE_ERROR");
        let ctx = err.context.expect("expected context");
        assert!(ctx.token_offset.is_some());
    }

    #[test]
    fn render_prep_error_context_supports_typed_indices() {
        let err = RenderPrepError::new("TEST", "typed context")
            .with_phase(ErrorPhase::Style)
            .with_chapter_index(7)
            .with_limit("max_css_bytes", 10, 4)
            .with_selector_index(3)
            .with_declaration_index(1)
            .with_token_offset(9);
        assert_eq!(err.phase, ErrorPhase::Style);
        assert_eq!(err.chapter_index, Some(7));
        let limit = err.limit.expect("expected limit context");
        assert_eq!(limit.kind.as_ref(), "max_css_bytes");
        assert_eq!(limit.actual, 10);
        assert_eq!(limit.limit, 4);
        let ctx = err.context.expect("expected context");
        assert_eq!(ctx.selector_index, Some(3));
        assert_eq!(ctx.declaration_index, Some(1));
        assert_eq!(ctx.token_offset, Some(9));
    }

    #[test]
    fn render_prep_error_bridges_to_phase_error() {
        let err = RenderPrepError::new("STYLE_CSS_TOO_LARGE", "limit")
            .with_phase(ErrorPhase::Style)
            .with_path("styles/main.css")
            .with_chapter_index(2)
            .with_selector_index(4)
            .with_limit("max_css_bytes", 1024, 256);
        let phase: PhaseError = err.into();
        assert_eq!(phase.phase, ErrorPhase::Style);
        assert_eq!(phase.code, "STYLE_CSS_TOO_LARGE");
        let ctx = phase.context.expect("expected context");
        assert_eq!(ctx.chapter_index, Some(2));
        let limit = ctx.limit.expect("expected limit");
        assert_eq!(limit.actual, 1024);
        assert_eq!(limit.limit, 256);
    }

    #[test]
    fn font_resolver_trace_reports_fallback_chain() {
        let resolver = FontResolver::new(FontPolicy::serif_default());
        let style = ComputedTextStyle {
            family_stack: vec!["A".to_string(), "B".to_string()].into(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            block_role: BlockRole::Body,
        };
        let trace = resolver.resolve_with_trace(&style);
        assert_eq!(trace.face.family, "serif");
        assert!(trace.reason_chain.len() >= 2);
    }

    #[test]
    fn font_resolver_chooses_nearest_weight_and_style() {
        let mut resolver = FontResolver::new(FontPolicy::serif_default());
        let faces = vec![
            EmbeddedFontFace {
                family: "Literata".to_string(),
                weight: 400,
                style: EmbeddedFontStyle::Normal,
                stretch: None,
                href: "a.ttf".to_string(),
                format: None,
            },
            EmbeddedFontFace {
                family: "Literata".to_string(),
                weight: 700,
                style: EmbeddedFontStyle::Italic,
                stretch: None,
                href: "b.ttf".to_string(),
                format: None,
            },
        ];
        resolver
            .register_epub_fonts(faces, |_href| Ok(vec![1, 2, 3]))
            .expect("register should succeed");
        let style = ComputedTextStyle {
            family_stack: vec!["Literata".to_string()].into(),
            weight: 680,
            italic: true,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            block_role: BlockRole::Body,
        };
        let trace = resolver.resolve_with_trace(&style);
        assert_eq!(resolver.resolve_font_id(&style), trace.face.font_id);
        let chosen = trace.face.embedded.expect("should match embedded");
        assert_eq!(chosen.href, "b.ttf");
    }

    #[test]
    fn font_resolver_reports_missing_glyph_risk_for_non_ascii_fallback() {
        let resolver = FontResolver::new(FontPolicy::serif_default());
        let style = ComputedTextStyle {
            family_stack: vec!["NoSuchFamily".to_string()].into(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            block_role: BlockRole::Body,
        };
        let trace = resolver.resolve_with_trace_for_text(&style, Some("Привет"));
        assert!(trace
            .reason_chain
            .iter()
            .any(|v| v.contains("missing glyph risk")));
    }

    #[test]
    fn font_resolver_deduplicates_faces() {
        let mut resolver = FontResolver::new(FontPolicy::serif_default()).with_limits(FontLimits {
            max_faces: 8,
            ..FontLimits::default()
        });
        let face = EmbeddedFontFace {
            family: "Literata".to_string(),
            weight: 400,
            style: EmbeddedFontStyle::Normal,
            stretch: None,
            href: "a.ttf".to_string(),
            format: None,
        };
        resolver
            .register_epub_fonts(vec![face.clone(), face], |_href| Ok(vec![1, 2, 3]))
            .expect("register should succeed");
        let style = ComputedTextStyle {
            family_stack: vec!["Literata".to_string()].into(),
            weight: 400,
            italic: false,
            size_px: 16.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            block_role: BlockRole::Body,
        };
        let trace = resolver.resolve_with_trace(&style);
        assert!(trace.face.embedded.is_some());
    }

    #[test]
    fn font_resolver_register_rejects_too_many_faces() {
        let mut resolver = FontResolver::new(FontPolicy::serif_default()).with_limits(FontLimits {
            max_faces: 1,
            ..FontLimits::default()
        });
        let faces = vec![
            EmbeddedFontFace {
                family: "A".to_string(),
                weight: 400,
                style: EmbeddedFontStyle::Normal,
                stretch: None,
                href: "a.ttf".to_string(),
                format: None,
            },
            EmbeddedFontFace {
                family: "B".to_string(),
                weight: 400,
                style: EmbeddedFontStyle::Normal,
                stretch: None,
                href: "b.ttf".to_string(),
                format: None,
            },
        ];
        let err = resolver
            .register_epub_fonts(faces, |_href| Ok(vec![1, 2, 3]))
            .expect_err("should reject");
        assert_eq!(err.code, "FONT_FACE_LIMIT");
    }

    #[test]
    fn render_prep_with_registered_fonts_uses_external_loader() {
        let called = std::cell::Cell::new(0usize);
        let prep = RenderPrep::new(RenderPrepOptions::default()).with_registered_fonts(
            vec![EmbeddedFontFace {
                family: "Custom".to_string(),
                weight: 400,
                style: EmbeddedFontStyle::Normal,
                stretch: None,
                href: "fonts/custom.ttf".to_string(),
                format: Some("truetype".to_string()),
            }],
            |href| {
                assert_eq!(href, "fonts/custom.ttf");
                called.set(called.get() + 1);
                Ok(vec![1, 2, 3, 4])
            },
        );
        assert!(prep.is_ok());
        assert_eq!(called.get(), 1);
    }
}
