use mu_epub::{
    BlockRole, ComputedTextStyle, StyledEvent, StyledEventOrRun, StyledImage, StyledRun,
};
use std::sync::Arc;

use crate::render_ir::{
    DrawCommand, JustifyMode, ObjectLayoutConfig, PageChromeCommand, PageChromeConfig,
    PageChromeKind, RectCommand, RenderIntent, RenderPage, ResolvedTextStyle, TextCommand,
    TypographyConfig,
};

const SOFT_HYPHEN: char = '\u{00AD}';
const LINE_FIT_GUARD_PX: f32 = 6.0;
#[cfg(target_os = "espidf")]
const MAX_BUFFERED_PARAGRAPH_WORDS: usize = 0;
#[cfg(not(target_os = "espidf"))]
const MAX_BUFFERED_PARAGRAPH_WORDS: usize = 64;
const MAX_BUFFERED_PARAGRAPH_CHARS: usize = 1200;

/// Optional text measurement hook for glyph-accurate line fitting.
pub trait TextMeasurer: Send + Sync {
    /// Measure rendered text width for the provided style.
    fn measure_text_px(&self, text: &str, style: &ResolvedTextStyle) -> f32;

    /// Conservative (safe upper-bound) width estimate.
    ///
    /// Default delegates to `measure_text_px`.
    fn conservative_text_px(&self, text: &str, style: &ResolvedTextStyle) -> f32 {
        self.measure_text_px(text, style)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HyphenationLang {
    Unknown,
    English,
}

impl HyphenationLang {
    fn from_tag(tag: &str) -> Self {
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("en") {
            Self::English
        } else {
            Self::Unknown
        }
    }
}

/// Policy for discretionary soft-hyphen handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftHyphenPolicy {
    /// Treat soft hyphens as invisible and never break on them.
    Ignore,
    /// Use soft hyphens as break opportunities and show `-` when broken.
    Discretionary,
}

/// Layout configuration for page construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutConfig {
    /// Physical display width.
    pub display_width: i32,
    /// Physical display height.
    pub display_height: i32,
    /// Left margin.
    pub margin_left: i32,
    /// Right margin.
    pub margin_right: i32,
    /// Top margin.
    pub margin_top: i32,
    /// Bottom margin.
    pub margin_bottom: i32,
    /// Extra gap between lines.
    pub line_gap_px: i32,
    /// Gap after paragraph/list item end.
    pub paragraph_gap_px: i32,
    /// Gap around heading blocks.
    pub heading_gap_px: i32,
    /// Keep headings with at least this many subsequent lines.
    pub heading_keep_with_next_lines: u8,
    /// Left indent for list items.
    pub list_indent_px: i32,
    /// First-line indent for paragraph/body text.
    pub first_line_indent_px: i32,
    /// Suppress first-line indent on paragraph immediately after a heading.
    pub suppress_indent_after_heading: bool,
    /// Minimum words for justification.
    pub justify_min_words: usize,
    /// Required fill ratio for justification.
    pub justify_min_fill_ratio: f32,
    /// Minimum final line height in px.
    pub min_line_height_px: i32,
    /// Maximum final line height in px.
    pub max_line_height_px: i32,
    /// Soft-hyphen handling policy.
    pub soft_hyphen_policy: SoftHyphenPolicy,
    /// Page chrome emission policy.
    pub page_chrome: PageChromeConfig,
    /// Typography policy surface.
    pub typography: TypographyConfig,
    /// Non-text object layout policy surface.
    pub object_layout: ObjectLayoutConfig,
    /// Theme/render intent surface.
    pub render_intent: RenderIntent,
}

impl LayoutConfig {
    /// Convenience for a display size with sensible defaults.
    pub fn for_display(width: i32, height: i32) -> Self {
        Self {
            display_width: width,
            display_height: height,
            ..Self::default()
        }
    }

    fn content_width(self) -> i32 {
        (self.display_width - self.margin_left - self.margin_right).max(1)
    }

    fn content_bottom(self) -> i32 {
        self.display_height - self.margin_bottom
    }
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            display_width: 480,
            display_height: 800,
            margin_left: 32,
            margin_right: 32,
            margin_top: 48,
            margin_bottom: 40,
            line_gap_px: 0,
            paragraph_gap_px: 8,
            heading_gap_px: 10,
            heading_keep_with_next_lines: 2,
            list_indent_px: 12,
            first_line_indent_px: 18,
            suppress_indent_after_heading: true,
            justify_min_words: 7,
            justify_min_fill_ratio: 0.75,
            min_line_height_px: 14,
            max_line_height_px: 48,
            soft_hyphen_policy: SoftHyphenPolicy::Discretionary,
            page_chrome: PageChromeConfig::default(),
            typography: TypographyConfig::default(),
            object_layout: ObjectLayoutConfig::default(),
            render_intent: RenderIntent::default(),
        }
    }
}

/// Deterministic layout engine that emits render pages.
#[derive(Clone)]
pub struct LayoutEngine {
    cfg: LayoutConfig,
    text_measurer: Option<Arc<dyn TextMeasurer>>,
}

/// Incremental layout session for streaming styled items into pages.
pub struct LayoutSession {
    engine: LayoutEngine,
    st: LayoutState,
    ctx: BlockCtx,
}

impl core::fmt::Debug for LayoutEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LayoutEngine")
            .field("cfg", &self.cfg)
            .field("has_text_measurer", &self.text_measurer.is_some())
            .finish()
    }
}

impl LayoutEngine {
    /// Create a layout engine.
    pub fn new(cfg: LayoutConfig) -> Self {
        Self {
            cfg,
            text_measurer: None,
        }
    }

    /// Install a shared text measurer for glyph-accurate width fitting.
    pub fn with_text_measurer(mut self, measurer: Arc<dyn TextMeasurer>) -> Self {
        self.text_measurer = Some(measurer);
        self
    }

    /// Layout styled items into pages.
    pub fn layout_items<I>(&self, items: I) -> Vec<RenderPage>
    where
        I: IntoIterator<Item = StyledEventOrRun>,
    {
        let mut pages = Vec::with_capacity(8);
        self.layout_with(items, |page| pages.push(page));
        pages
    }

    /// Start an incremental layout session.
    pub fn start_session(&self) -> LayoutSession {
        self.start_session_with_text_measurer(self.text_measurer.clone())
    }

    /// Start an incremental layout session with an explicit text measurer override.
    pub fn start_session_with_text_measurer(
        &self,
        measurer: Option<Arc<dyn TextMeasurer>>,
    ) -> LayoutSession {
        LayoutSession {
            engine: self.clone(),
            st: LayoutState::new(self.cfg, measurer),
            ctx: BlockCtx::default(),
        }
    }

    /// Layout styled items and stream each page.
    pub fn layout_with<I, F>(&self, items: I, mut on_page: F)
    where
        I: IntoIterator<Item = StyledEventOrRun>,
        F: FnMut(RenderPage),
    {
        let mut session = self.start_session();
        for item in items {
            session.push_item(item);
        }
        session.finish(&mut on_page);
    }

    fn handle_run(&self, st: &mut LayoutState, ctx: &mut BlockCtx, run: StyledRun) {
        let mut style = to_resolved_style(&run.style);
        style.font_id = Some(run.font_id);
        if !run.resolved_family.is_empty() {
            style.family = run.resolved_family.clone();
        }
        if let Some(level) = ctx.heading_level {
            style.role = BlockRole::Heading(level);
        }
        if ctx.in_list {
            style.role = BlockRole::ListItem;
        }

        for word in run.text.split_whitespace() {
            let mut extra_indent_px = 0;
            if ctx.pending_indent
                && matches!(style.role, BlockRole::Body | BlockRole::Paragraph)
                && !ctx.in_list
                && ctx.heading_level.is_none()
            {
                extra_indent_px = self.cfg.first_line_indent_px.max(0);
                ctx.pending_indent = false;
            }
            st.push_word(word, style.clone(), extra_indent_px);
        }
    }

    fn handle_event(&self, st: &mut LayoutState, ctx: &mut BlockCtx, ev: StyledEvent) {
        match ev {
            StyledEvent::ParagraphStart => {
                if ctx.keep_with_next_pending {
                    st.set_pending_keep_with_next(self.cfg.heading_keep_with_next_lines);
                    ctx.keep_with_next_pending = false;
                }
                st.begin_paragraph();
                if !ctx.suppress_next_indent {
                    ctx.pending_indent = true;
                }
                ctx.suppress_next_indent = false;
            }
            StyledEvent::ParagraphEnd => {
                st.flush_buffered_paragraph(false, true);
                st.flush_line(true, false);
                st.end_paragraph();
                st.add_vertical_gap(self.cfg.paragraph_gap_px);
                ctx.pending_indent = true;
            }
            StyledEvent::HeadingStart(level) => {
                st.flush_buffered_paragraph(false, true);
                st.flush_line(true, false);
                st.end_paragraph();
                st.add_vertical_gap(self.cfg.heading_gap_px);
                ctx.heading_level = Some(level.clamp(1, 6));
                ctx.pending_indent = false;
            }
            StyledEvent::HeadingEnd(_) => {
                st.flush_buffered_paragraph(false, true);
                st.flush_line(true, false);
                st.add_vertical_gap(self.cfg.heading_gap_px);
                ctx.heading_level = None;
                ctx.pending_indent = false;
                ctx.suppress_next_indent = self.cfg.suppress_indent_after_heading;
                ctx.keep_with_next_pending = true;
            }
            StyledEvent::ListItemStart => {
                if ctx.keep_with_next_pending {
                    st.set_pending_keep_with_next(self.cfg.heading_keep_with_next_lines);
                    ctx.keep_with_next_pending = false;
                }
                st.flush_buffered_paragraph(false, true);
                st.flush_line(true, false);
                st.end_paragraph();
                ctx.in_list = true;
                ctx.pending_indent = false;
            }
            StyledEvent::ListItemEnd => {
                st.flush_buffered_paragraph(false, true);
                st.flush_line(true, false);
                st.add_vertical_gap(self.cfg.paragraph_gap_px.saturating_sub(2));
                ctx.in_list = false;
                ctx.pending_indent = true;
            }
            StyledEvent::LineBreak => {
                st.flush_buffered_paragraph(true, false);
                st.flush_line(false, true);
                ctx.pending_indent = false;
            }
        }
    }

    fn handle_image(&self, st: &mut LayoutState, _ctx: &mut BlockCtx, image: StyledImage) {
        st.flush_buffered_paragraph(false, true);
        st.flush_line(true, false);
        st.place_inline_image_block(image);
    }
}

impl LayoutSession {
    fn push_item_impl(&mut self, item: StyledEventOrRun) {
        match item {
            StyledEventOrRun::Run(run) => self.engine.handle_run(&mut self.st, &mut self.ctx, run),
            StyledEventOrRun::Event(ev) => {
                self.engine.handle_event(&mut self.st, &mut self.ctx, ev);
            }
            StyledEventOrRun::Image(image) => {
                self.engine.handle_image(&mut self.st, &mut self.ctx, image);
            }
        }
    }

    /// Push one styled item into the layout state.
    pub fn push_item(&mut self, item: StyledEventOrRun) {
        self.push_item_impl(item);
    }

    /// Set the hyphenation language hint (e.g. "en", "en-US").
    pub fn set_hyphenation_language(&mut self, language_tag: &str) {
        self.st.hyphenation_lang = HyphenationLang::from_tag(language_tag);
    }

    /// Push one styled item and emit any fully closed pages.
    pub fn push_item_with_pages<F>(&mut self, item: StyledEventOrRun, on_page: &mut F)
    where
        F: FnMut(RenderPage),
    {
        self.push_item_impl(item);
        for page in self.st.drain_emitted_pages() {
            on_page(page);
        }
    }

    /// Finish the session and stream resulting pages.
    pub fn finish<F>(&mut self, on_page: &mut F)
    where
        F: FnMut(RenderPage),
    {
        self.st.flush_buffered_paragraph(false, true);
        self.st.flush_line(true, false);
        let chrome_cfg = self.engine.cfg.page_chrome;
        if !chrome_cfg.header_enabled && !chrome_cfg.footer_enabled && !chrome_cfg.progress_enabled
        {
            self.st.flush_page_if_non_empty();
            for page in self.st.drain_emitted_pages() {
                on_page(page);
            }
            return;
        }
        let mut pages = core::mem::take(&mut self.st).into_pages();
        annotate_page_chrome(&mut pages, self.engine.cfg);
        for page in pages {
            on_page(page);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct BlockCtx {
    heading_level: Option<u8>,
    in_list: bool,
    pending_indent: bool,
    suppress_next_indent: bool,
    keep_with_next_pending: bool,
}

#[derive(Clone, Debug)]
struct CurrentLine {
    text: String,
    style: ResolvedTextStyle,
    width_px: f32,
    line_height_px: i32,
    left_inset_px: i32,
}

#[derive(Clone, Debug)]
struct ParagraphWord {
    text: String,
    style: ResolvedTextStyle,
    left_inset_px: i32,
    width_px: f32,
}

#[derive(Clone)]
struct LayoutState {
    cfg: LayoutConfig,
    text_measurer: Option<Arc<dyn TextMeasurer>>,
    page_no: usize,
    cursor_y: i32,
    page: RenderPage,
    line: Option<CurrentLine>,
    emitted: Vec<RenderPage>,
    in_paragraph: bool,
    lines_on_current_paragraph_page: usize,
    pending_keep_with_next_lines: u8,
    hyphenation_lang: HyphenationLang,
    paragraph_words: Vec<ParagraphWord>,
    paragraph_chars: usize,
    replay_direct_mode: bool,
    buffered_flush_mode: bool,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self::new(LayoutConfig::default(), None)
    }
}

impl LayoutState {
    fn new(cfg: LayoutConfig, text_measurer: Option<Arc<dyn TextMeasurer>>) -> Self {
        Self {
            cfg,
            text_measurer,
            page_no: 1,
            cursor_y: cfg.margin_top,
            page: RenderPage::new(1),
            line: None,
            emitted: Vec::with_capacity(2),
            in_paragraph: false,
            lines_on_current_paragraph_page: 0,
            pending_keep_with_next_lines: 0,
            hyphenation_lang: HyphenationLang::Unknown,
            paragraph_words: Vec::with_capacity(MAX_BUFFERED_PARAGRAPH_WORDS),
            paragraph_chars: 0,
            replay_direct_mode: false,
            buffered_flush_mode: false,
        }
    }

    fn begin_paragraph(&mut self) {
        self.in_paragraph = true;
        self.lines_on_current_paragraph_page = 0;
        self.paragraph_words.clear();
        self.paragraph_chars = 0;
    }

    fn end_paragraph(&mut self) {
        self.paragraph_words.clear();
        self.paragraph_chars = 0;
        self.in_paragraph = false;
        self.lines_on_current_paragraph_page = 0;
    }

    fn set_pending_keep_with_next(&mut self, lines: u8) {
        self.pending_keep_with_next_lines = lines.max(1);
    }

    fn place_inline_image_block(&mut self, image: StyledImage) {
        let content_w = self.cfg.content_width().max(8);
        let content_h = (self.cfg.content_bottom() - self.cfg.margin_top).max(16);
        let max_h = ((content_h as f32) * self.cfg.object_layout.max_inline_image_height_ratio)
            .round()
            .clamp(40.0, content_h as f32) as i32;

        let (mut image_w, mut image_h) = match (image.width_px, image.height_px) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                let w = w as f32;
                let h = h as f32;
                let fit_w = content_w as f32 / w;
                let fit_h = max_h as f32 / h;
                let scale = fit_w.min(fit_h).min(1.0);
                let out_w = (w * scale).round().max(24.0) as i32;
                let out_h = (h * scale).round().max(18.0) as i32;
                (out_w, out_h)
            }
            _ => {
                let out_w = ((content_w as f32) * 0.72)
                    .round()
                    .clamp(60.0, content_w as f32) as i32;
                let out_h = ((out_w as f32) * 0.62).round().clamp(36.0, max_h as f32) as i32;
                (out_w, out_h)
            }
        };
        image_w = image_w.min(content_w);
        image_h = image_h.min(max_h).max(18);

        let mut caption = String::new();
        if self.cfg.object_layout.alt_text_fallback {
            let alt = image.alt.trim();
            if !alt.is_empty() {
                caption = alt.to_string();
            }
        }
        let caption_style = ResolvedTextStyle {
            font_id: None,
            family: "serif".to_string(),
            weight: 400,
            italic: true,
            size_px: 14.0,
            line_height: 1.2,
            letter_spacing: 0.0,
            role: BlockRole::Paragraph,
            justify_mode: JustifyMode::None,
        };
        let caption_line_h = line_height_px(&caption_style, &self.cfg);
        let caption_gap = if caption.is_empty() { 0 } else { 6 };
        let block_h = image_h
            + caption_gap
            + if caption.is_empty() {
                0
            } else {
                caption_line_h
            };

        if self.cursor_y + block_h > self.cfg.content_bottom() {
            self.start_next_page();
        }
        let x = self.cfg.margin_left + ((content_w - image_w) / 2);
        let y = self.cursor_y;
        self.page
            .push_content_command(DrawCommand::Rect(RectCommand {
                x,
                y,
                width: image_w.max(1) as u32,
                height: image_h.max(1) as u32,
                fill: false,
            }));
        // Visual header strip to signal an image object block.
        self.page
            .push_content_command(DrawCommand::Rect(RectCommand {
                x: x + 1,
                y: y + 1,
                width: (image_w - 2).max(1) as u32,
                height: ((image_h as f32) * 0.08).round().max(2.0) as u32,
                fill: true,
            }));
        // Keep src as structured annotation for debug/telemetry.
        if !image.src.is_empty() {
            self.page
                .annotations
                .push(crate::render_ir::PageAnnotation {
                    kind: "inline_image_src".to_string(),
                    value: Some(image.src),
                });
        }
        self.page.sync_commands();
        self.cursor_y += image_h;

        if !caption.is_empty() {
            self.cursor_y += caption_gap;
            let max_caption_w = (content_w - 8).max(1) as f32;
            let caption_text =
                truncate_text_to_width(self, &caption, &caption_style, max_caption_w);
            self.page
                .push_content_command(DrawCommand::Text(TextCommand {
                    x: self.cfg.margin_left + 4,
                    baseline_y: self.cursor_y + line_ascent_px(&caption_style, caption_line_h),
                    text: caption_text,
                    font_id: caption_style.font_id,
                    style: caption_style,
                }));
            self.page.sync_commands();
            self.cursor_y += caption_line_h;
        }
        self.cursor_y += self.cfg.paragraph_gap_px.max(4);
    }

    fn measure_text(&self, text: &str, style: &ResolvedTextStyle) -> f32 {
        self.text_measurer
            .as_ref()
            .map(|m| m.measure_text_px(text, style))
            .unwrap_or_else(|| heuristic_measure_text(text, style))
    }

    fn conservative_measure_text(&self, text: &str, style: &ResolvedTextStyle) -> f32 {
        self.text_measurer
            .as_ref()
            .map(|m| m.conservative_text_px(text, style))
            .unwrap_or_else(|| conservative_heuristic_measure_text(text, style))
    }

    fn push_word(&mut self, word: &str, style: ResolvedTextStyle, extra_first_line_indent_px: i32) {
        if word.is_empty() {
            return;
        }

        let mut left_inset_px = if matches!(style.role, BlockRole::ListItem) {
            self.cfg.list_indent_px
        } else {
            0
        };
        left_inset_px += extra_first_line_indent_px.max(0);

        let sanitized_word = strip_soft_hyphens(word);
        if self.should_buffer_paragraph_word(&style, word) {
            let projected_chars = self
                .paragraph_chars
                .saturating_add(sanitized_word.chars().count())
                .saturating_add(usize::from(!self.paragraph_words.is_empty()));
            if self.paragraph_words.len() >= MAX_BUFFERED_PARAGRAPH_WORDS
                || projected_chars > MAX_BUFFERED_PARAGRAPH_CHARS
            {
                self.flush_buffered_paragraph(false, false);
            }
            let word_w = self.measure_text(&sanitized_word, &style);
            self.paragraph_chars = self
                .paragraph_chars
                .saturating_add(sanitized_word.chars().count())
                .saturating_add(usize::from(!self.paragraph_words.is_empty()));
            self.paragraph_words.push(ParagraphWord {
                text: sanitized_word,
                style,
                left_inset_px,
                width_px: word_w,
            });
            return;
        }
        self.push_word_direct(word, style, left_inset_px);
    }

    fn should_buffer_paragraph_word(&self, style: &ResolvedTextStyle, raw_word: &str) -> bool {
        if cfg!(target_os = "espidf") {
            return false;
        }
        if self.replay_direct_mode {
            return false;
        }
        if !self.cfg.typography.justification.enabled {
            return false;
        }
        if !self.in_paragraph || self.line.is_some() {
            return false;
        }
        if raw_word.contains(SOFT_HYPHEN) {
            return false;
        }
        if raw_word.chars().count() > 18 {
            return false;
        }
        matches!(style.role, BlockRole::Body | BlockRole::Paragraph)
    }

    fn push_word_direct(&mut self, word: &str, style: ResolvedTextStyle, left_inset_px: i32) {
        if word.is_empty() {
            return;
        }
        let display_word = strip_soft_hyphens(word);
        if display_word.is_empty() {
            return;
        }

        if self.line.is_none() {
            self.prepare_for_new_line(&style);
            self.line = Some(CurrentLine {
                text: String::with_capacity(64),
                style: style.clone(),
                width_px: 0.0,
                line_height_px: line_height_px(&style, &self.cfg),
                left_inset_px,
            });
        }

        let Some(mut line) = self.line.take() else {
            return;
        };

        if line.text.is_empty() {
            line.style = style.clone();
            line.left_inset_px = left_inset_px;
            line.line_height_px = line_height_px(&style, &self.cfg);
        }

        let space_w = if line.text.is_empty() {
            0.0
        } else {
            self.measure_text(" ", &line.style)
        };
        let word_w = self.measure_text(&display_word, &style);
        let max_width = ((self.cfg.content_width() - line.left_inset_px).max(1) as f32
            - line_fit_guard_px(&style))
        .max(1.0);

        let projected = line.width_px + space_w + word_w;
        let overflow = projected - max_width;
        let hang_credit = if self.cfg.typography.hanging_punctuation.enabled {
            trailing_hang_credit_px(word, &style)
        } else {
            0.0
        };

        if overflow > hang_credit {
            if (self.cfg.soft_hyphen_policy == SoftHyphenPolicy::Discretionary
                || matches!(
                    self.cfg.typography.hyphenation.soft_hyphen_policy,
                    crate::render_ir::HyphenationMode::Discretionary
                ))
                && word.contains(SOFT_HYPHEN)
                && self.try_break_word_at_soft_hyphen(&mut line, word, &style, max_width, space_w)
            {
                return;
            }
            if (self.cfg.soft_hyphen_policy == SoftHyphenPolicy::Discretionary
                || matches!(
                    self.cfg.typography.hyphenation.soft_hyphen_policy,
                    crate::render_ir::HyphenationMode::Discretionary
                ))
                && !word.contains(SOFT_HYPHEN)
                && self.try_auto_hyphenate(&mut line, word, &style, max_width, space_w)
            {
                return;
            }
            #[cfg(not(target_os = "espidf"))]
            if let Some((left_text, right_text, left_w, right_w)) =
                self.optimize_overflow_break(&line, word, &style, max_width)
            {
                let continuation_inset = if matches!(style.role, BlockRole::ListItem) {
                    self.cfg.list_indent_px
                } else {
                    0
                };
                line.text = left_text;
                line.width_px = left_w;
                line.style = style.clone();
                self.line = Some(line);
                self.flush_line(false, false);
                self.line = Some(CurrentLine {
                    text: right_text,
                    style: style.clone(),
                    width_px: right_w,
                    line_height_px: line_height_px(&style, &self.cfg),
                    left_inset_px: continuation_inset,
                });
                return;
            }
            if line.text.is_empty() {
                line.text = display_word.clone();
                line.width_px = word_w;
                line.style = style;
                self.line = Some(line);
                return;
            }
            self.line = Some(line);
            self.flush_line(false, false);
            self.line = Some(CurrentLine {
                text: display_word,
                style: style.clone(),
                width_px: word_w,
                line_height_px: line_height_px(&style, &self.cfg),
                left_inset_px,
            });
            return;
        }

        if !line.text.is_empty() {
            line.text.push(' ');
            line.width_px += space_w;
        }
        line.text.push_str(&display_word);
        line.width_px += word_w;
        line.style = style;
        self.line = Some(line);
    }

    fn prepare_for_new_line(&mut self, style: &ResolvedTextStyle) {
        if self.pending_keep_with_next_lines > 0 {
            let reserve = self.pending_keep_with_next_lines as i32;
            let line_h = line_height_px(style, &self.cfg);
            let required = reserve.saturating_mul(line_h + self.cfg.line_gap_px.max(0));
            let remaining = self.cfg.content_bottom() - self.cursor_y;
            if remaining < required && !self.page.content_commands.is_empty() {
                self.start_next_page();
            }
            self.pending_keep_with_next_lines = 0;
        }
        if self.cfg.typography.widow_orphan_control.enabled
            && self.in_paragraph
            && self.lines_on_current_paragraph_page == 0
        {
            let min_lines = self.cfg.typography.widow_orphan_control.min_lines.max(1) as i32;
            let line_h = line_height_px(style, &self.cfg);
            let required = min_lines.saturating_mul(line_h + self.cfg.line_gap_px.max(0));
            let remaining = self.cfg.content_bottom() - self.cursor_y;
            if remaining < required && !self.page.content_commands.is_empty() {
                self.start_next_page();
            }
        }
    }

    fn flush_buffered_paragraph(&mut self, mark_last_hard_break: bool, is_last_in_block: bool) {
        if self.paragraph_words.is_empty() {
            return;
        }
        if self.paragraph_words.len() < 2 {
            let replay = core::mem::take(&mut self.paragraph_words);
            self.paragraph_chars = 0;
            self.replay_direct_mode = true;
            for word in replay {
                self.push_word_direct(&word.text, word.style, word.left_inset_px);
            }
            self.replay_direct_mode = false;
            return;
        }
        let breaks = match self.optimize_paragraph_breaks() {
            Some(breaks) if !breaks.is_empty() => breaks,
            _ => {
                let replay = core::mem::take(&mut self.paragraph_words);
                self.paragraph_chars = 0;
                self.replay_direct_mode = true;
                for word in replay {
                    self.push_word_direct(&word.text, word.style, word.left_inset_px);
                }
                self.replay_direct_mode = false;
                return;
            }
        };

        let words = core::mem::take(&mut self.paragraph_words);
        self.paragraph_chars = 0;
        self.buffered_flush_mode = true;
        let mut start = 0usize;
        for (idx, end) in breaks.iter().copied().enumerate() {
            if end <= start || end > words.len() {
                break;
            }
            let mut text = String::with_capacity(64);
            for (offset, word) in words[start..end].iter().enumerate() {
                if offset > 0 {
                    text.push(' ');
                }
                text.push_str(&word.text);
            }
            let style = words[end - 1].style.clone();
            let left_inset_px = words[start].left_inset_px;
            self.prepare_for_new_line(&style);
            let width_px = self.measure_text(&text, &style);
            self.line = Some(CurrentLine {
                text,
                style,
                width_px,
                line_height_px: line_height_px(&words[end - 1].style, &self.cfg),
                left_inset_px,
            });
            let is_last_line = idx + 1 == breaks.len();
            self.flush_line(
                is_last_line && is_last_in_block,
                is_last_line && mark_last_hard_break,
            );
            start = end;
        }
        self.buffered_flush_mode = false;
    }

    fn optimize_paragraph_breaks(&self) -> Option<Vec<usize>> {
        let words = &self.paragraph_words;
        let n = words.len();
        if n == 0 {
            return Some(Vec::with_capacity(0));
        }
        if n == 1 {
            return Some(vec![1]);
        }

        for (idx, word) in words.iter().enumerate() {
            let available = ((self.cfg.content_width() - word.left_inset_px).max(1) as f32
                - line_fit_guard_px(&word.style))
            .max(1.0);
            if word.width_px > available && idx == 0 {
                return None;
            }
        }

        let inf = i64::MAX / 4;
        let mut dp = Vec::with_capacity(n + 1);
        dp.resize(n + 1, inf);
        let mut next_break = Vec::with_capacity(n + 1);
        next_break.resize(n + 1, n);
        dp[n] = 0;

        for i in (0..n).rev() {
            let available = ((self.cfg.content_width() - words[i].left_inset_px).max(1) as f32
                - line_fit_guard_px(&words[i].style))
            .max(1.0);
            let mut line_width = 0.0f32;
            for j in i..n {
                if j == i {
                    line_width += words[j].width_px;
                } else {
                    line_width += self.measure_text(" ", &words[j - 1].style) + words[j].width_px;
                }
                let slack = available - line_width;
                if slack < 0.0 {
                    break;
                }
                let is_last = j + 1 == n;
                let words_in_line = j + 1 - i;
                let fill_ratio = if available > 0.0 {
                    line_width / available
                } else {
                    0.0
                };
                let mut badness = if is_last {
                    let rag = (1.0 - fill_ratio).max(0.0);
                    (rag * rag * 120.0).round() as i64
                } else {
                    let ratio = (slack / available).clamp(0.0, 1.2);
                    (ratio * ratio * ratio * 2400.0).round() as i64
                };
                let min_fill = self
                    .cfg
                    .typography
                    .justification
                    .min_fill_ratio
                    .max(self.cfg.justify_min_fill_ratio);
                if !is_last && fill_ratio < min_fill {
                    badness += ((min_fill - fill_ratio) * 8000.0).round() as i64;
                }
                if !is_last && words_in_line == 1 {
                    badness += 3000;
                }
                if !is_last && words[j].text.chars().count() <= 2 {
                    badness += 4200;
                }
                if i > 0 && words[i].text.chars().count() <= 2 {
                    badness += 1000;
                }
                let candidate = badness.saturating_add(dp[j + 1]);
                if candidate < dp[i] {
                    dp[i] = candidate;
                    next_break[i] = j + 1;
                }
            }
        }

        if dp[0] >= inf {
            return None;
        }
        let mut out = Vec::with_capacity(n / 2 + 1);
        let mut cursor = 0usize;
        while cursor < n {
            let next = next_break[cursor];
            if next <= cursor || next > n {
                return None;
            }
            out.push(next);
            cursor = next;
        }
        Some(out)
    }

    fn optimize_overflow_break(
        &self,
        line: &CurrentLine,
        incoming_word: &str,
        style: &ResolvedTextStyle,
        max_width: f32,
    ) -> Option<(String, String, f32, f32)> {
        if line.text.is_empty() || incoming_word.is_empty() {
            return None;
        }
        let mut words: Vec<&str> = line.text.split_whitespace().collect();
        words.push(incoming_word);
        if words.len() < 3 {
            return None;
        }
        let mut best: Option<(String, String, f32, f32, i32)> = None;
        // Keep at least one word on each side.
        for break_idx in 1..words.len() {
            let left_words = &words[..break_idx];
            let right_words = &words[break_idx..];
            let left = left_words.join(" ");
            let right = right_words.join(" ");
            let left_w = self.measure_text(&left, style);
            if left_w > max_width {
                continue;
            }
            let right_w = self.measure_text(&right, style);
            let slack = (max_width - left_w).max(0.0);
            let last_left_len = left_words
                .last()
                .map(|w| w.chars().count() as i32)
                .unwrap_or_default();
            let first_right_len = right_words
                .first()
                .map(|w| w.chars().count() as i32)
                .unwrap_or_default();
            let mut score = (slack * slack).round() as i32;
            if last_left_len <= 2 {
                score += 1400;
            }
            if first_right_len <= 2 {
                score += 900;
            }
            if right_words.len() == 1 {
                score += 400;
            }
            match best {
                Some((_, _, _, _, best_score)) if score >= best_score => {}
                _ => best = Some((left, right, left_w, right_w, score)),
            }
        }
        best.map(|(left, right, left_w, right_w, _)| (left, right, left_w, right_w))
    }

    fn try_break_word_at_soft_hyphen(
        &mut self,
        line: &mut CurrentLine,
        raw_word: &str,
        style: &ResolvedTextStyle,
        max_width: f32,
        space_w: f32,
    ) -> bool {
        let parts: Vec<&str> = raw_word.split(SOFT_HYPHEN).collect();
        if parts.len() < 2 {
            return false;
        }

        let mut best_prefix: Option<(String, String)> = None;
        for i in 1..parts.len() {
            let prefix = parts[..i].concat();
            let suffix = parts[i..].concat();
            if prefix.is_empty() || suffix.is_empty() {
                continue;
            }
            let candidate = format!("{prefix}-");
            let candidate_w = self.measure_text(&candidate, style);
            let added = if line.text.is_empty() {
                candidate_w
            } else {
                space_w + candidate_w
            };
            if line.width_px + added <= max_width {
                best_prefix = Some((candidate, suffix));
            } else {
                break;
            }
        }

        let Some((prefix_with_hyphen, remainder)) = best_prefix else {
            return false;
        };

        if !line.text.is_empty() {
            line.text.push(' ');
            line.width_px += space_w;
        }
        line.text.push_str(&prefix_with_hyphen);
        line.width_px += self.measure_text(&prefix_with_hyphen, style);

        self.line = Some(line.clone());
        self.flush_line(false, false);
        self.push_word(&remainder, style.clone(), 0);
        true
    }

    fn try_auto_hyphenate(
        &mut self,
        line: &mut CurrentLine,
        word: &str,
        style: &ResolvedTextStyle,
        max_width: f32,
        space_w: f32,
    ) -> bool {
        if !matches!(self.hyphenation_lang, HyphenationLang::English) {
            return false;
        }
        let candidates = english_hyphenation_candidates(word);
        if candidates.is_empty() {
            return false;
        }

        let mut best_split: Option<(String, String, f32, i32)> = None;
        for split in candidates {
            let Some((left, right)) = split_word_at_char_boundary(word, split) else {
                continue;
            };
            if left.chars().count() < 3 || right.chars().count() < 3 {
                continue;
            }
            let left_h = format!("{left}-");
            let candidate_w = self.measure_text(&left_h, style);
            let added = if line.text.is_empty() {
                candidate_w
            } else {
                space_w + candidate_w
            };
            if line.width_px + added <= max_width {
                let left_len = left.chars().count() as i32;
                let right_len = right.chars().count() as i32;
                let balance_penalty = (left_len - right_len).abs();
                // Prefer fitting split near the right edge while avoiding overly
                // unbalanced chunks that produce bad rhythm.
                let fit_slack = (max_width - (line.width_px + added)).round() as i32;
                let score = fit_slack.saturating_mul(2).saturating_add(balance_penalty);
                match best_split {
                    Some((_, _, _, best_score)) if score >= best_score => {}
                    _ => best_split = Some((left_h, right.to_string(), candidate_w, score)),
                }
            }
        }

        let Some((prefix_with_hyphen, remainder, _, _)) = best_split else {
            return false;
        };

        if !line.text.is_empty() {
            line.text.push(' ');
            line.width_px += space_w;
        }
        line.text.push_str(&prefix_with_hyphen);
        line.width_px += self.measure_text(&prefix_with_hyphen, style);

        self.line = Some(line.clone());
        self.flush_line(false, false);
        self.push_word(&remainder, style.clone(), 0);
        true
    }

    fn flush_line(&mut self, is_last_in_block: bool, hard_break: bool) {
        let Some(mut line) = self.line.take() else {
            return;
        };
        if line.text.trim().is_empty() {
            return;
        }

        if self.cursor_y + line.line_height_px > self.cfg.content_bottom() {
            self.start_next_page();
        }
        let short_line_words = line.text.split_whitespace().count();
        let remaining_after = self.cfg.content_bottom()
            - (self.cursor_y + line.line_height_px + self.cfg.line_gap_px.max(0));
        if short_line_words <= 2
            && !is_last_in_block
            && !self.page.content_commands.is_empty()
            && remaining_after < line.line_height_px
        {
            self.start_next_page();
        }

        let available_width = ((self.cfg.content_width() - line.left_inset_px) as f32
            - line_fit_guard_px(&line.style)) as i32;
        let quality_remainder = if self.cfg.typography.justification.enabled
            && !is_last_in_block
            && !hard_break
            && !self.buffered_flush_mode
            && !cfg!(target_os = "espidf")
        {
            self.rebalance_line_for_quality(&mut line, available_width)
        } else {
            None
        };
        if let Some(overflow_word) = quality_remainder {
            let continuation_inset = if matches!(line.style.role, BlockRole::ListItem) {
                self.cfg.list_indent_px
            } else {
                0
            };
            self.line = Some(CurrentLine {
                text: overflow_word.clone(),
                style: line.style.clone(),
                width_px: self.measure_text(&overflow_word, &line.style),
                line_height_px: line_height_px(&line.style, &self.cfg),
                left_inset_px: continuation_inset,
            });
        }
        if !self.buffered_flush_mode && !cfg!(target_os = "espidf") {
            if let Some(overflow_word) =
                self.rebalance_line_for_right_edge(&mut line, available_width)
            {
                let continuation_inset = if matches!(line.style.role, BlockRole::ListItem) {
                    self.cfg.list_indent_px
                } else {
                    0
                };
                self.line = Some(CurrentLine {
                    text: overflow_word.clone(),
                    style: line.style.clone(),
                    width_px: self.measure_text(&overflow_word, &line.style),
                    line_height_px: line_height_px(&line.style, &self.cfg),
                    left_inset_px: continuation_inset,
                });
            }
        }
        let words = line.text.split_whitespace().count();
        let spaces = line.text.chars().filter(|c| *c == ' ').count() as i32;
        let fill_ratio = if available_width > 0 {
            line.width_px / available_width as f32
        } else {
            0.0
        };

        if self.cfg.typography.justification.enabled
            && matches!(line.style.role, BlockRole::Body | BlockRole::Paragraph)
            && !is_last_in_block
            && !hard_break
            && words
                >= self
                    .cfg
                    .typography
                    .justification
                    .min_words
                    .max(self.cfg.justify_min_words)
            && spaces > 0
            && fill_ratio
                >= self
                    .cfg
                    .typography
                    .justification
                    .min_fill_ratio
                    .max(self.cfg.justify_min_fill_ratio)
        {
            let extra = (available_width as f32 - line.width_px).max(0.0) as i32;
            let space_w = self.measure_text(" ", &line.style).max(1.0);
            let max_extra_per_space = (space_w * 0.45).round().max(1.0) as i32;
            let max_extra_total = spaces.saturating_mul(max_extra_per_space);
            let punctuation_terminal = ends_with_terminal_punctuation(&line.text);
            let extra = extra.min(max_extra_total);
            if punctuation_terminal || extra <= 0 {
                line.style.justify_mode = JustifyMode::None;
            } else {
                line.style.justify_mode = JustifyMode::InterWord {
                    extra_px_total: extra,
                };
            }
        } else {
            line.style.justify_mode = JustifyMode::None;
        }

        self.page
            .push_content_command(DrawCommand::Text(TextCommand {
                x: self.cfg.margin_left + line.left_inset_px,
                baseline_y: self.cursor_y + line_ascent_px(&line.style, line.line_height_px),
                text: line.text,
                font_id: line.style.font_id,
                style: line.style,
            }));
        self.page.sync_commands();

        self.cursor_y += line.line_height_px + self.cfg.line_gap_px;
        if self.in_paragraph {
            self.lines_on_current_paragraph_page =
                self.lines_on_current_paragraph_page.saturating_add(1);
        }
    }

    fn rebalance_line_for_right_edge(
        &self,
        line: &mut CurrentLine,
        available_width: i32,
    ) -> Option<String> {
        if available_width <= 0 {
            return None;
        }
        let conservative = self.conservative_measure_text(&line.text, &line.style);
        if conservative <= available_width as f32 {
            return None;
        }
        let source = line.text.clone();
        let (head, tail) = source.rsplit_once(' ')?;
        let head = head.trim_end();
        let tail = tail.trim_start();
        if head.is_empty() || tail.is_empty() {
            return None;
        }
        line.text = head.to_string();
        line.width_px = self.measure_text(&line.text, &line.style);
        Some(tail.to_string())
    }

    fn rebalance_line_for_quality(
        &self,
        line: &mut CurrentLine,
        available_width: i32,
    ) -> Option<String> {
        if available_width <= 0 {
            return None;
        }
        let words: Vec<&str> = line.text.split_whitespace().collect();
        let tail = *words.last()?;
        if tail.chars().count() > 2 || words.len() < 3 {
            return None;
        }
        let source = line.text.clone();
        let (head, tail) = source.rsplit_once(' ')?;
        let head = head.trim_end();
        let tail = tail.trim_start();
        if head.is_empty() || tail.is_empty() {
            return None;
        }
        let head_w = self.measure_text(head, &line.style);
        let fill = head_w / available_width as f32;
        if fill < 0.55 {
            return None;
        }
        line.text = head.to_string();
        line.width_px = head_w;
        Some(tail.to_string())
    }

    fn add_vertical_gap(&mut self, gap_px: i32) {
        if gap_px <= 0 {
            return;
        }
        self.cursor_y += gap_px;
        if self.cursor_y >= self.cfg.content_bottom() {
            self.start_next_page();
        }
    }

    fn start_next_page(&mut self) {
        self.flush_page_if_non_empty();
        self.page_no += 1;
        self.page = RenderPage::new(self.page_no);
        self.cursor_y = self.cfg.margin_top;
        if self.in_paragraph {
            self.lines_on_current_paragraph_page = 0;
        }
    }

    fn flush_page_if_non_empty(&mut self) {
        if self.page.content_commands.is_empty()
            && self.page.chrome_commands.is_empty()
            && self.page.overlay_commands.is_empty()
        {
            return;
        }
        let mut page = core::mem::replace(&mut self.page, RenderPage::new(self.page_no + 1));
        page.metrics.chapter_page_index = page.page_number.saturating_sub(1);
        page.sync_commands();
        self.emitted.push(page);
    }

    fn into_pages(mut self) -> Vec<RenderPage> {
        self.flush_page_if_non_empty();
        self.emitted
    }

    fn drain_emitted_pages(&mut self) -> Vec<RenderPage> {
        core::mem::take(&mut self.emitted)
    }
}

fn truncate_text_to_width(
    st: &LayoutState,
    text: &str,
    style: &ResolvedTextStyle,
    max_width: f32,
) -> String {
    if st.measure_text(text, style) <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut candidate = out.clone();
        candidate.push(ch);
        candidate.push('…');
        if st.measure_text(&candidate, style) > max_width {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

fn to_resolved_style(style: &ComputedTextStyle) -> ResolvedTextStyle {
    let family = style
        .family_stack
        .first()
        .cloned()
        .unwrap_or_else(|| "serif".to_string());
    ResolvedTextStyle {
        font_id: None,
        family,
        weight: style.weight,
        italic: style.italic,
        size_px: style.size_px,
        line_height: style.line_height,
        letter_spacing: style.letter_spacing,
        role: style.block_role,
        justify_mode: JustifyMode::None,
    }
}

fn heuristic_measure_text(text: &str, style: &ResolvedTextStyle) -> f32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0.0;
    }
    // Dynamic width model:
    // - per-glyph class widths (narrow/regular/wide/punctuation/digit)
    // - style/family modifiers
    // This is more stable across font sizes/families than a single scalar.
    let family = style.family.to_ascii_lowercase();
    let proportional = !(family.contains("mono") || family.contains("fixed"));
    let mut em_sum = 0.0f32;
    if proportional {
        for ch in text.chars() {
            em_sum += proportional_glyph_em_width(ch);
        }
    } else {
        // Fixed-width fallback still uses a small class delta for punctuation.
        for ch in text.chars() {
            em_sum += if ch == ' ' { 0.52 } else { 0.58 };
        }
    }

    let mut family_scale = if family.contains("serif") {
        1.03
    } else if family.contains("sans") {
        0.99
    } else {
        1.00
    };
    if style.weight >= 700 {
        family_scale += 0.03;
    }
    if style.italic {
        family_scale += 0.01;
    }
    if style.size_px >= 24.0 {
        family_scale += 0.01;
    }

    let mut width = em_sum * style.size_px * family_scale;
    if chars > 1 {
        width += (chars as f32 - 1.0) * style.letter_spacing;
    }
    width
}

fn proportional_glyph_em_width(ch: char) -> f32 {
    match ch {
        ' ' => 0.32,
        '\t' => 1.28,
        '\u{00A0}' => 0.32,
        'i' | 'l' | 'I' | '|' | '!' => 0.24,
        '.' | ',' | ':' | ';' | '\'' | '"' | '`' => 0.23,
        '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' => 0.34,
        '(' | ')' | '[' | ']' | '{' | '}' => 0.30,
        'f' | 't' | 'j' | 'r' => 0.34,
        'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' | '#' => 0.74,
        c if c.is_ascii_digit() => 0.52,
        c if c.is_ascii_uppercase() => 0.64,
        c if c.is_ascii_lowercase() => 0.52,
        c if c.is_whitespace() => 0.32,
        c if c.is_ascii_punctuation() => 0.42,
        _ => 0.56,
    }
}

fn line_height_px(style: &ResolvedTextStyle, cfg: &LayoutConfig) -> i32 {
    let min_lh = cfg.min_line_height_px.min(cfg.max_line_height_px);
    let max_lh = cfg.max_line_height_px.max(cfg.min_line_height_px);
    (style.size_px * style.line_height)
        .round()
        .clamp(min_lh as f32, max_lh as f32) as i32
}

fn line_fit_guard_px(style: &ResolvedTextStyle) -> f32 {
    let family = style.family.to_ascii_lowercase();
    let proportional = !(family.contains("mono") || family.contains("fixed"));
    let mut guard = LINE_FIT_GUARD_PX;
    // Proportional and larger sizes can have right-side overhangs in rendered
    // glyph bitmaps; reserve a tiny extra safety band to avoid clipping.
    if proportional {
        guard += 2.0;
    }
    if style.size_px >= 24.0 {
        guard += 2.0;
    }
    if style.weight >= 700 {
        guard += 1.0;
    }
    guard
}

fn conservative_heuristic_measure_text(text: &str, style: &ResolvedTextStyle) -> f32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0.0;
    }
    let family = style.family.to_ascii_lowercase();
    let proportional = !(family.contains("mono") || family.contains("fixed"));
    let mut em_sum = 0.0f32;
    if proportional {
        for ch in text.chars() {
            em_sum += match ch {
                ' ' | '\u{00A0}' => 0.32,
                '\t' => 1.28,
                'i' | 'l' | 'I' | '|' | '!' => 0.24,
                '.' | ',' | ':' | ';' | '\'' | '"' | '`' => 0.23,
                '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' => 0.34,
                '(' | ')' | '[' | ']' | '{' | '}' => 0.30,
                'f' | 't' | 'j' | 'r' => 0.34,
                'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' | '#' => 0.74,
                c if c.is_ascii_digit() => 0.52,
                c if c.is_ascii_uppercase() => 0.61,
                c if c.is_ascii_lowercase() => 0.50,
                c if c.is_whitespace() => 0.32,
                c if c.is_ascii_punctuation() => 0.42,
                _ => 0.56,
            };
        }
    } else {
        for ch in text.chars() {
            em_sum += if ch == ' ' { 0.52 } else { 0.58 };
        }
    }
    let mut scale = if proportional { 1.05 } else { 1.02 };
    if style.weight >= 700 {
        scale += 0.02;
    }
    if style.italic {
        scale += 0.01;
    }
    if style.size_px >= 24.0 {
        scale += 0.01;
    }
    let mut width = em_sum * style.size_px * scale;
    if chars > 1 {
        width += (chars as f32 - 1.0) * style.letter_spacing.max(0.0);
    }
    width
}

fn line_ascent_px(style: &ResolvedTextStyle, line_height_px: i32) -> i32 {
    let approx = (style.size_px * 0.78).round() as i32;
    approx.clamp(1, line_height_px.saturating_sub(1).max(1))
}

fn trailing_hang_credit_px(word: &str, style: &ResolvedTextStyle) -> f32 {
    let Some(last) = word.chars().last() else {
        return 0.0;
    };
    if matches!(
        last,
        '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | ')' | ']' | '}' | '»'
    ) {
        (style.size_px * 0.18).clamp(1.0, 4.0)
    } else {
        0.0
    }
}

fn ends_with_terminal_punctuation(line: &str) -> bool {
    let Some(last) = line.trim_end().chars().last() else {
        return false;
    };
    matches!(
        last,
        '.' | ';' | ':' | '!' | '?' | '"' | '\'' | ')' | ']' | '}'
    )
}

fn split_word_at_char_boundary(word: &str, split_chars: usize) -> Option<(&str, &str)> {
    if split_chars == 0 {
        return None;
    }
    let mut split_byte = None;
    for (idx, (byte, _)) in word.char_indices().enumerate() {
        if idx == split_chars {
            split_byte = Some(byte);
            break;
        }
    }
    let split_byte = split_byte?;
    Some((&word[..split_byte], &word[split_byte..]))
}

fn english_hyphenation_candidates(word: &str) -> Vec<usize> {
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < 7 {
        return Vec::with_capacity(0);
    }
    let mut candidates = Vec::with_capacity(chars.len() / 2);
    if let Some(exception) = english_hyphenation_exception(word) {
        candidates.extend_from_slice(exception);
    }
    let is_vowel = |c: char| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y');

    for i in 3..(chars.len().saturating_sub(3)) {
        let prev = chars[i - 1];
        let next = chars[i];
        if !prev.is_ascii_alphabetic() || !next.is_ascii_alphabetic() {
            continue;
        }
        if is_vowel(prev) != is_vowel(next) {
            candidates.push(i);
        }
    }

    const SUFFIXES: &[&str] = &[
        "tion", "sion", "ment", "ness", "less", "able", "ible", "ally", "ingly", "edly", "ing",
        "ed", "ly",
    ];
    let lower = word.to_ascii_lowercase();
    for suffix in SUFFIXES {
        if lower.ends_with(suffix) {
            let split = chars.len().saturating_sub(suffix.chars().count());
            if split >= 3 && split + 3 <= chars.len() {
                candidates.push(split);
            }
        }
    }

    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn english_hyphenation_exception(word: &str) -> Option<&'static [usize]> {
    let lower = word.to_ascii_lowercase();
    match lower.as_str() {
        "characteristically" => Some(&[4, 6, 9, 12]),
        "accessibility" => Some(&[3, 6, 9]),
        "fundamental" => Some(&[3, 6]),
        "functionality" => Some(&[4, 7, 10]),
        "publication" => Some(&[3, 6]),
        "consortium" => Some(&[3, 6]),
        "adventure" => Some(&[2, 5]),
        "marvellous" => Some(&[3, 6]),
        "extraordinary" => Some(&[5, 8]),
        "responsibility" => Some(&[3, 6, 9]),
        "determined" => Some(&[3, 6]),
        "encounter" => Some(&[2, 5]),
        "obedient" => Some(&[2, 5]),
        "endeavour" => Some(&[2, 5]),
        "providence" => Some(&[3, 6]),
        "language" => Some(&[3]),
        "fortune" => Some(&[3]),
        "navigator" => Some(&[3, 6]),
        "navigators" => Some(&[3, 6]),
        "apparently" => Some(&[3, 6]),
        "hitherto" => Some(&[3]),
        "merchantman" => Some(&[4, 7]),
        _ => None,
    }
}

fn strip_soft_hyphens(text: &str) -> String {
    if text.contains(SOFT_HYPHEN) {
        text.chars().filter(|ch| *ch != SOFT_HYPHEN).collect()
    } else {
        text.to_string()
    }
}

fn annotate_page_chrome(pages: &mut [RenderPage], cfg: LayoutConfig) {
    if pages.is_empty() {
        return;
    }
    let total = pages.len();
    for page in pages.iter_mut() {
        if cfg.page_chrome.header_enabled {
            page.push_chrome_command(DrawCommand::PageChrome(PageChromeCommand {
                kind: PageChromeKind::Header,
                text: Some(format!("Page {}", page.page_number)),
                current: None,
                total: None,
            }));
        }
        if cfg.page_chrome.footer_enabled {
            page.push_chrome_command(DrawCommand::PageChrome(PageChromeCommand {
                kind: PageChromeKind::Footer,
                text: Some(format!("Page {}", page.page_number)),
                current: None,
                total: None,
            }));
        }
        if cfg.page_chrome.progress_enabled {
            page.push_chrome_command(DrawCommand::PageChrome(PageChromeCommand {
                kind: PageChromeKind::Progress,
                text: None,
                current: Some(page.page_number),
                total: Some(total),
            }));
        }
        page.sync_commands();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WideMeasurer;

    impl TextMeasurer for WideMeasurer {
        fn measure_text_px(&self, text: &str, style: &ResolvedTextStyle) -> f32 {
            text.chars().count() as f32 * (style.size_px * 0.9).max(1.0)
        }
    }

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

    fn inline_image(
        src: &str,
        alt: &str,
        width_px: Option<u16>,
        height_px: Option<u16>,
    ) -> StyledEventOrRun {
        StyledEventOrRun::Image(StyledImage {
            src: src.to_string(),
            alt: alt.to_string(),
            width_px,
            height_px,
        })
    }

    #[test]
    fn layout_splits_into_multiple_pages() {
        let cfg = LayoutConfig {
            display_height: 120,
            margin_top: 8,
            margin_bottom: 8,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let mut items = Vec::new();
        for _ in 0..50 {
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
            items.push(body_run("hello world mu-epub renderer pipeline"));
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
        }

        let pages = engine.layout_items(items);
        assert!(pages.len() > 1);
    }

    #[test]
    fn inline_image_emits_rect_annotation_and_caption() {
        let cfg = LayoutConfig {
            display_width: 320,
            display_height: 480,
            margin_left: 20,
            margin_right: 20,
            margin_top: 20,
            margin_bottom: 20,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("Before image"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
            inline_image("images/pic.jpg", "Picture caption", Some(600), Some(400)),
        ];
        let pages = engine.layout_items(items);
        assert!(!pages.is_empty());
        let first = &pages[0];
        assert!(first
            .annotations
            .iter()
            .any(|a| a.kind == "inline_image_src" && a.value.as_deref() == Some("images/pic.jpg")));
        assert!(first
            .commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Rect(_))));
        assert!(first.commands.iter().any(|cmd| match cmd {
            DrawCommand::Text(t) => t.text.contains("Picture caption"),
            _ => false,
        }));
    }

    #[test]
    fn inline_image_moves_to_next_page_when_remaining_space_is_too_small() {
        let cfg = LayoutConfig {
            display_width: 320,
            display_height: 120,
            margin_left: 8,
            margin_right: 8,
            margin_top: 8,
            margin_bottom: 8,
            paragraph_gap_px: 8,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("This paragraph uses enough space to force an image page break."),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
            inline_image("images/diagram.png", "Diagram", Some(240), Some(180)),
        ];
        let pages = engine.layout_items(items);
        assert!(pages.len() >= 2);
        let page0_has_image_rect = pages[0]
            .commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Rect(_)));
        let page1_has_image_rect = pages[1]
            .commands
            .iter()
            .any(|cmd| matches!(cmd, DrawCommand::Rect(_)));
        assert!(!page0_has_image_rect);
        assert!(page1_has_image_rect);
    }

    #[test]
    fn layout_assigns_justify_mode_for_body_lines() {
        let engine = LayoutEngine::new(LayoutConfig::default());
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("one two three four five six seven eight nine ten eleven twelve"),
            body_run("one two three four five six seven eight nine ten eleven twelve"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];

        let pages = engine.layout_items(items);
        let mut saw_justified = false;
        for page in pages {
            for cmd in page.commands {
                if let DrawCommand::Text(t) = cmd {
                    if matches!(t.style.justify_mode, JustifyMode::InterWord { .. }) {
                        saw_justified = true;
                    }
                }
            }
        }
        assert!(saw_justified);
    }

    #[test]
    fn custom_text_measurer_changes_wrap_behavior() {
        let cfg = LayoutConfig {
            display_width: 280,
            margin_left: 12,
            margin_right: 12,
            ..LayoutConfig::default()
        };
        let default_engine = LayoutEngine::new(cfg);
        let measured_engine = LayoutEngine::new(cfg).with_text_measurer(Arc::new(WideMeasurer));
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("one two three four five six seven eight nine ten eleven twelve"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];

        let default_lines = default_engine
            .layout_items(items.clone())
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter(|cmd| matches!(cmd, DrawCommand::Text(_)))
            .count();
        let measured_lines = measured_engine
            .layout_items(items)
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter(|cmd| matches!(cmd, DrawCommand::Text(_)))
            .count();
        assert!(measured_lines > default_lines);
    }

    #[test]
    fn soft_hyphen_is_invisible_when_not_broken() {
        let engine = LayoutEngine::new(LayoutConfig {
            display_width: 640,
            ..LayoutConfig::default()
        });
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("co\u{00AD}operate"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["cooperate".to_string()]);
    }

    #[test]
    fn soft_hyphen_emits_visible_hyphen_on_break() {
        let engine = LayoutEngine::new(LayoutConfig {
            display_width: 150,
            soft_hyphen_policy: SoftHyphenPolicy::Discretionary,
            ..LayoutConfig::default()
        });
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("extra\u{00AD}ordinary"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.ends_with('-')));
        assert!(!texts.iter().any(|t| t.contains('\u{00AD}')));
    }

    #[test]
    fn golden_ir_fragment_includes_font_id_and_page_chrome() {
        let engine = LayoutEngine::new(LayoutConfig {
            page_chrome: PageChromeConfig {
                header_enabled: true,
                footer_enabled: true,
                progress_enabled: true,
                ..PageChromeConfig::default()
            },
            ..LayoutConfig::default()
        });
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma delta"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];

        let pages = engine.layout_items(items);
        assert_eq!(pages.len(), 1);
        let page = &pages[0];
        let first_text = page
            .commands
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t),
                _ => None,
            })
            .expect("missing text command");
        assert_eq!(first_text.text, "alpha beta gamma delta");
        assert_eq!(first_text.font_id, Some(0));
        assert_eq!(first_text.style.font_id, Some(0));

        let chrome_kinds: Vec<PageChromeKind> = page
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::PageChrome(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            chrome_kinds,
            vec![
                PageChromeKind::Header,
                PageChromeKind::Footer,
                PageChromeKind::Progress
            ]
        );
    }

    #[test]
    fn page_chrome_policy_controls_emitted_markers() {
        let engine = LayoutEngine::new(LayoutConfig {
            page_chrome: PageChromeConfig {
                header_enabled: false,
                footer_enabled: true,
                progress_enabled: true,
                ..PageChromeConfig::default()
            },
            ..LayoutConfig::default()
        });
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma delta"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];

        let pages = engine.layout_items(items);
        assert_eq!(pages.len(), 1);
        let chrome_kinds: Vec<PageChromeKind> = pages[0]
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::PageChrome(c) => Some(c.kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            chrome_kinds,
            vec![PageChromeKind::Footer, PageChromeKind::Progress]
        );
    }

    #[test]
    fn finish_without_chrome_streams_pages_without_marker_commands() {
        let engine = LayoutEngine::new(LayoutConfig::default());
        let mut session = engine.start_session();
        session.push_item(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
        session.push_item(body_run(
            "A long enough paragraph to produce wrapped lines without any page chrome markers.",
        ));
        session.push_item(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
        let mut pages = Vec::with_capacity(2);
        session.finish(&mut |page| pages.push(page));
        assert!(!pages.is_empty());
        for page in pages {
            assert!(!page.content_commands.is_empty());
            let has_chrome = page
                .commands
                .iter()
                .any(|cmd| matches!(cmd, DrawCommand::PageChrome(_)));
            assert!(!has_chrome);
        }
    }

    #[test]
    fn explicit_line_break_line_is_not_justified() {
        let cfg = LayoutConfig {
            display_width: 640,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("one two three four five six seven eight nine ten"),
            StyledEventOrRun::Event(StyledEvent::LineBreak),
            body_run("eleven twelve thirteen fourteen fifteen sixteen"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let first_line = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .find_map(|cmd| match cmd {
                DrawCommand::Text(t) if t.text.starts_with("one two") => Some(t),
                _ => None,
            })
            .expect("line-break line should exist");
        assert_eq!(first_line.style.justify_mode, JustifyMode::None);
    }

    #[test]
    fn widow_orphan_control_moves_new_paragraph_to_next_page_when_needed() {
        let cfg = LayoutConfig {
            display_width: 320,
            display_height: 70,
            margin_top: 8,
            margin_bottom: 8,
            paragraph_gap_px: 8,
            line_gap_px: 0,
            typography: TypographyConfig {
                widow_orphan_control: crate::render_ir::WidowOrphanControl {
                    enabled: true,
                    min_lines: 2,
                },
                ..TypographyConfig::default()
            },
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("delta epsilon zeta"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        assert!(pages.len() >= 2);
        let page1_text: Vec<String> = pages[0]
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        let page2_text: Vec<String> = pages[1]
            .commands
            .iter()
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(page1_text.iter().any(|t| t.contains("alpha")));
        assert!(!page1_text.iter().any(|t| t.contains("delta")));
        assert!(page2_text.iter().any(|t| t.contains("delta")));
    }

    #[test]
    fn first_line_baseline_accounts_for_ascent() {
        let cfg = LayoutConfig {
            margin_top: 8,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let first_line = pages[0]
            .commands
            .iter()
            .find_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t),
                _ => None,
            })
            .expect("text command should exist");
        assert!(first_line.baseline_y > cfg.margin_top);
    }

    #[test]
    fn heading_keep_with_next_moves_following_paragraph_to_next_page() {
        let cfg = LayoutConfig {
            display_height: 96,
            margin_top: 8,
            margin_bottom: 8,
            heading_keep_with_next_lines: 2,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma delta epsilon"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
            StyledEventOrRun::Event(StyledEvent::HeadingStart(1)),
            body_run("Heading"),
            StyledEventOrRun::Event(StyledEvent::HeadingEnd(1)),
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("next paragraph should move"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        assert!(pages.len() >= 2);
    }

    #[test]
    fn english_auto_hyphenation_breaks_long_word_when_needed() {
        let cfg = LayoutConfig {
            display_width: 170,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let mut session = engine.start_session();
        session.set_hyphenation_language("en-US");
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("characteristically"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        for item in items {
            session.push_item(item);
        }
        let mut pages = Vec::new();
        session.finish(&mut |p| pages.push(p));
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.ends_with('-')));
    }

    #[test]
    fn english_exception_hyphenation_handles_accessibility() {
        let cfg = LayoutConfig {
            display_width: 170,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let mut session = engine.start_session();
        session.set_hyphenation_language("en");
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("accessibility"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        for item in items {
            session.push_item(item);
        }
        let mut pages = Vec::with_capacity(2);
        session.finish(&mut |p| pages.push(p));
        let texts: Vec<String> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.ends_with('-')));
    }

    #[test]
    fn quality_rebalance_avoids_short_trailing_word_on_wrapped_line() {
        let cfg = LayoutConfig {
            display_width: 220,
            typography: TypographyConfig {
                justification: crate::render_ir::JustificationConfig {
                    enabled: true,
                    min_words: 3,
                    min_fill_ratio: 0.5,
                },
                ..TypographyConfig::default()
            },
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma delta to epsilon zeta eta theta"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let lines: Vec<String> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect();
        assert!(
            lines.iter().all(|line| !line.ends_with(" to")),
            "quality rebalance should avoid short trailing 'to': {:?}",
            lines
        );
    }

    #[test]
    fn paragraph_optimizer_keeps_non_terminal_lines_reasonably_filled() {
        let cfg = LayoutConfig {
            display_width: 260,
            margin_left: 10,
            margin_right: 10,
            typography: TypographyConfig {
                justification: crate::render_ir::JustificationConfig {
                    enabled: true,
                    min_words: 4,
                    min_fill_ratio: 0.72,
                },
                ..TypographyConfig::default()
            },
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run(
                "The quick brown fox jumps over the lazy dog while curious readers inspect global paragraph balancing across many lines and widths",
            ),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];
        let pages = engine.layout_items(items);
        let lines: Vec<&TextCommand> = pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .filter_map(|cmd| match cmd {
                DrawCommand::Text(t)
                    if matches!(t.style.role, BlockRole::Body | BlockRole::Paragraph) =>
                {
                    Some(t)
                }
                _ => None,
            })
            .collect();
        assert!(lines.len() >= 3, "expected wrapped paragraph");

        for line in lines.iter().take(lines.len().saturating_sub(1)) {
            let words = line.text.split_whitespace().count();
            if words < 4 {
                continue;
            }
            let available =
                ((cfg.content_width() - 0).max(1) as f32 - line_fit_guard_px(&line.style)).max(1.0);
            let ratio = heuristic_measure_text(&line.text, &line.style) / available;
            assert!(
                ratio >= 0.60,
                "non-terminal line underfilled too much: '{}' ratio={}",
                line.text,
                ratio
            );
        }
    }

    #[test]
    fn layout_invariants_are_deterministic_and_non_overlapping() {
        let cfg = LayoutConfig {
            display_height: 180,
            margin_top: 10,
            margin_bottom: 10,
            page_chrome: PageChromeConfig {
                progress_enabled: true,
                ..PageChromeConfig::default()
            },
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let mut items = Vec::new();
        for _ in 0..30 {
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
            items.push(body_run(
                "one two three four five six seven eight nine ten eleven twelve",
            ));
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
        }

        let first = engine.layout_items(items.clone());
        let second = engine.layout_items(items);
        assert_eq!(first, second);

        let mut prev_page_no = 0usize;
        for page in &first {
            assert!(page.page_number > prev_page_no);
            prev_page_no = page.page_number;

            let mut prev_baseline = i32::MIN;
            for cmd in &page.commands {
                if let DrawCommand::Text(text) = cmd {
                    assert!(text.baseline_y > prev_baseline);
                    prev_baseline = text.baseline_y;
                }
            }
        }
    }

    #[test]
    fn incremental_session_matches_batch_layout() {
        let cfg = LayoutConfig {
            page_chrome: PageChromeConfig {
                progress_enabled: true,
                footer_enabled: true,
                ..PageChromeConfig::default()
            },
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let items = vec![
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("alpha beta gamma delta epsilon zeta eta theta"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
            StyledEventOrRun::Event(StyledEvent::ParagraphStart),
            body_run("iota kappa lambda mu nu xi omicron pi rho"),
            StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        ];

        let batch = engine.layout_items(items.clone());
        let mut session = engine.start_session();
        for item in items {
            session.push_item(item);
        }
        let mut streamed = Vec::new();
        session.finish(&mut |page| streamed.push(page));
        assert_eq!(batch, streamed);
    }

    #[test]
    fn incremental_push_item_with_pages_matches_batch_layout() {
        let cfg = LayoutConfig {
            display_height: 130,
            margin_top: 8,
            margin_bottom: 8,
            ..LayoutConfig::default()
        };
        let engine = LayoutEngine::new(cfg);
        let mut items = Vec::new();
        for _ in 0..40 {
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
            items.push(body_run("one two three four five six seven eight nine ten"));
            items.push(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
        }

        let batch = engine.layout_items(items.clone());
        assert!(batch.len() > 1);

        let mut session = engine.start_session();
        let mut streamed = Vec::new();
        let mut during_push = Vec::new();
        for item in items {
            session.push_item_with_pages(item, &mut |page| {
                during_push.push(page.clone());
                streamed.push(page);
            });
        }
        session.finish(&mut |page| streamed.push(page));

        assert_eq!(batch, streamed);
        assert!(!during_push.is_empty());
        assert_eq!(during_push, batch[..during_push.len()].to_vec());
        let during_push_numbers: Vec<usize> =
            during_push.iter().map(|page| page.page_number).collect();
        let batch_prefix_numbers: Vec<usize> = batch
            .iter()
            .take(during_push_numbers.len())
            .map(|page| page.page_number)
            .collect();
        assert_eq!(during_push_numbers, batch_prefix_numbers);
    }
}
