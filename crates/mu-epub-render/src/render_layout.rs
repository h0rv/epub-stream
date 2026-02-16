use mu_epub::{BlockRole, ComputedTextStyle, StyledEvent, StyledEventOrRun, StyledRun};

use crate::render_ir::{
    DrawCommand, JustifyMode, ObjectLayoutConfig, PageChromeCommand, PageChromeConfig,
    PageChromeKind, RenderIntent, RenderPage, ResolvedTextStyle, TextCommand, TypographyConfig,
};

const SOFT_HYPHEN: char = '\u{00AD}';
const LINE_FIT_GUARD_PX: f32 = 6.0;

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
#[derive(Clone, Debug)]
pub struct LayoutEngine {
    cfg: LayoutConfig,
}

/// Incremental layout session for streaming styled items into pages.
pub struct LayoutSession {
    engine: LayoutEngine,
    st: LayoutState,
    ctx: BlockCtx,
}

impl LayoutEngine {
    /// Create a layout engine.
    pub fn new(cfg: LayoutConfig) -> Self {
        Self { cfg }
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
        LayoutSession {
            engine: self.clone(),
            st: LayoutState::new(self.cfg),
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
                st.begin_paragraph();
                if !ctx.suppress_next_indent {
                    ctx.pending_indent = true;
                }
                ctx.suppress_next_indent = false;
            }
            StyledEvent::ParagraphEnd => {
                st.flush_line(true, false);
                st.end_paragraph();
                st.add_vertical_gap(self.cfg.paragraph_gap_px);
                ctx.pending_indent = true;
            }
            StyledEvent::HeadingStart(level) => {
                st.flush_line(true, false);
                st.end_paragraph();
                st.add_vertical_gap(self.cfg.heading_gap_px);
                ctx.heading_level = Some(level.clamp(1, 6));
                ctx.pending_indent = false;
            }
            StyledEvent::HeadingEnd(_) => {
                st.flush_line(true, false);
                st.add_vertical_gap(self.cfg.heading_gap_px);
                ctx.heading_level = None;
                ctx.pending_indent = false;
                ctx.suppress_next_indent = self.cfg.suppress_indent_after_heading;
            }
            StyledEvent::ListItemStart => {
                st.flush_line(true, false);
                st.end_paragraph();
                ctx.in_list = true;
                ctx.pending_indent = false;
            }
            StyledEvent::ListItemEnd => {
                st.flush_line(true, false);
                st.add_vertical_gap(self.cfg.paragraph_gap_px.saturating_sub(2));
                ctx.in_list = false;
                ctx.pending_indent = true;
            }
            StyledEvent::LineBreak => {
                st.flush_line(false, true);
                ctx.pending_indent = false;
            }
        }
    }
}

impl LayoutSession {
    fn push_item_impl(&mut self, item: StyledEventOrRun) {
        match item {
            StyledEventOrRun::Run(run) => self.engine.handle_run(&mut self.st, &mut self.ctx, run),
            StyledEventOrRun::Event(ev) => {
                self.engine.handle_event(&mut self.st, &mut self.ctx, ev);
            }
        }
    }

    /// Push one styled item into the layout state.
    pub fn push_item(&mut self, item: StyledEventOrRun) {
        self.push_item_impl(item);
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
        self.st.flush_line(true, false);
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
struct LayoutState {
    cfg: LayoutConfig,
    page_no: usize,
    cursor_y: i32,
    page: RenderPage,
    line: Option<CurrentLine>,
    emitted: Vec<RenderPage>,
    in_paragraph: bool,
    lines_on_current_paragraph_page: usize,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self::new(LayoutConfig::default())
    }
}

impl LayoutState {
    fn new(cfg: LayoutConfig) -> Self {
        Self {
            cfg,
            page_no: 1,
            cursor_y: cfg.margin_top,
            page: RenderPage::new(1),
            line: None,
            emitted: Vec::with_capacity(2),
            in_paragraph: false,
            lines_on_current_paragraph_page: 0,
        }
    }

    fn begin_paragraph(&mut self) {
        self.in_paragraph = true;
        self.lines_on_current_paragraph_page = 0;
    }

    fn end_paragraph(&mut self) {
        self.in_paragraph = false;
        self.lines_on_current_paragraph_page = 0;
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

        if self.line.is_none() {
            if self.cfg.typography.widow_orphan_control.enabled
                && self.in_paragraph
                && self.lines_on_current_paragraph_page == 0
            {
                let min_lines = self.cfg.typography.widow_orphan_control.min_lines.max(1) as i32;
                let line_h = line_height_px(&style, &self.cfg);
                let required = min_lines.saturating_mul(line_h + self.cfg.line_gap_px.max(0));
                let remaining = self.cfg.content_bottom() - self.cursor_y;
                if remaining < required && !self.page.content_commands.is_empty() {
                    self.start_next_page();
                }
            }
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
            measure_text(" ", &line.style)
        };
        let sanitized_word = strip_soft_hyphens(word);
        let word_w = measure_text(&sanitized_word, &style);
        let max_width = ((self.cfg.content_width() - line.left_inset_px).max(1) as f32
            - LINE_FIT_GUARD_PX)
            .max(1.0);

        let projected = line.width_px + space_w + word_w;
        let overflow = projected - max_width;
        let hang_credit = if self.cfg.typography.hanging_punctuation.enabled {
            trailing_hang_credit_px(&sanitized_word, &style)
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
            if line.text.is_empty() {
                line.text = sanitized_word;
                line.width_px = word_w;
                line.style = style;
                self.line = Some(line);
                return;
            }
            self.line = Some(line);
            self.flush_line(false, false);
            self.line = Some(CurrentLine {
                text: sanitized_word,
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
        line.text.push_str(&sanitized_word);
        line.width_px += word_w;
        line.style = style;
        self.line = Some(line);
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
            let candidate_w = measure_text(&candidate, style);
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
        line.width_px += measure_text(&prefix_with_hyphen, style);

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

        let available_width =
            ((self.cfg.content_width() - line.left_inset_px) as f32 - LINE_FIT_GUARD_PX) as i32;
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
            let space_w = measure_text(" ", &line.style).max(1.0);
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

fn measure_text(text: &str, style: &ResolvedTextStyle) -> f32 {
    let chars = text.chars().count() as f32;
    if chars == 0.0 {
        return 0.0;
    }
    // Width estimation tuned for mixed backends.
    // Large sizes get a slightly smaller width factor to avoid under-utilizing
    // the right edge with conservative wrapping.
    let family = style.family.to_ascii_lowercase();
    let proportional = !(family.contains("mono") || family.contains("fixed"));
    let large_size_adjust = if style.size_px >= 24.0 {
        0.03
    } else if style.size_px >= 20.0 {
        0.02
    } else {
        0.0
    };
    let width_factor = if proportional {
        if style.weight >= 700 {
            0.47 - large_size_adjust
        } else if style.italic {
            0.44 - large_size_adjust
        } else {
            0.45 - large_size_adjust
        }
    } else if style.weight >= 700 {
        0.40 - (large_size_adjust * 0.5)
    } else if style.italic {
        0.37 - (large_size_adjust * 0.5)
    } else {
        0.38 - (large_size_adjust * 0.5)
    };
    let mut width = chars * style.size_px * width_factor;
    if chars > 1.0 {
        width += (chars - 1.0) * style.letter_spacing;
    }
    width
}

fn line_height_px(style: &ResolvedTextStyle, cfg: &LayoutConfig) -> i32 {
    let min_lh = cfg.min_line_height_px.min(cfg.max_line_height_px);
    let max_lh = cfg.max_line_height_px.max(cfg.min_line_height_px);
    (style.size_px * style.line_height)
        .round()
        .clamp(min_lh as f32, max_lh as f32) as i32
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
