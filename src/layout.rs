//! Text layout engine for EPUB pagination
//!
//! Converts tokens into laid-out pages for display.
//! Uses greedy line breaking with embedded-graphics font metrics.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::tokenizer::Token;

/// Text style for layout (bold, italic, etc.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum TextStyle {
    /// Normal text
    #[default]
    Normal,
    /// Bold text
    Bold,
    /// Italic text
    Italic,
    /// Bold and italic text
    BoldItalic,
}

impl TextStyle {
    /// Check if style is bold
    pub fn is_bold(&self) -> bool {
        matches!(self, TextStyle::Bold | TextStyle::BoldItalic)
    }

    /// Check if style is italic
    pub fn is_italic(&self) -> bool {
        matches!(self, TextStyle::Italic | TextStyle::BoldItalic)
    }

    /// Apply bold flag to current style
    pub fn with_bold(&self, bold: bool) -> Self {
        match (bold, self.is_italic()) {
            (true, true) => TextStyle::BoldItalic,
            (true, false) => TextStyle::Bold,
            (false, true) => TextStyle::Italic,
            (false, false) => TextStyle::Normal,
        }
    }

    /// Apply italic flag to current style
    pub fn with_italic(&self, italic: bool) -> Self {
        match (self.is_bold(), italic) {
            (true, true) => TextStyle::BoldItalic,
            (true, false) => TextStyle::Bold,
            (false, true) => TextStyle::Italic,
            (false, false) => TextStyle::Normal,
        }
    }
}

/// A span of text with a single style within a line
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextSpan {
    /// Text content of this span
    pub text: String,
    /// Style for this span
    pub style: TextStyle,
}

impl TextSpan {
    /// Create a new text span
    pub fn new(text: String, style: TextStyle) -> Self {
        Self { text, style }
    }
}

/// A single laid-out line of text
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Styled text spans that make up this line
    pub spans: Vec<TextSpan>,
    /// Y position on the page
    pub y: i32,
}

impl Line {
    /// Create a new line (convenience: single span)
    pub fn new(text: String, y: i32, style: TextStyle) -> Self {
        Self {
            spans: vec![TextSpan::new(text, style)],
            y,
        }
    }

    /// Get concatenated text content of all spans
    pub fn text(&self) -> String {
        self.spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Get the primary style (first span's style, or Normal)
    pub fn style(&self) -> TextStyle {
        self.spans
            .first()
            .map(|s| s.style)
            .unwrap_or(TextStyle::Normal)
    }

    /// Check if line is empty
    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| s.text.is_empty())
    }

    /// Get line length in characters
    pub fn len(&self) -> usize {
        self.spans.iter().map(|s| s.text.len()).sum()
    }
}

/// A single page of laid-out content
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// Lines on this page
    pub lines: Vec<Line>,
    /// Page number (1-indexed)
    pub page_number: usize,
}

impl Page {
    /// Create a new empty page
    pub fn new(page_number: usize) -> Self {
        Self {
            lines: Vec::with_capacity(8),
            page_number,
        }
    }

    /// Add a line to the page
    pub fn add_line(&mut self, line: Line) {
        self.lines.push(line);
    }

    /// Check if page has no lines
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Get number of lines on page
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// Font metrics for text measurement
#[derive(Clone, Debug)]
pub struct FontMetrics {
    /// Character width in pixels
    pub char_width: f32,
    /// Character height in pixels
    pub char_height: f32,
    /// Bold character width (typically same or slightly wider)
    pub bold_char_width: f32,
    /// Italic character width (typically same)
    pub italic_char_width: f32,
}

impl Default for FontMetrics {
    fn default() -> Self {
        // Use FONT_10X20 metrics as a reasonable default
        Self::font_10x20()
    }
}

impl FontMetrics {
    /// Create metrics for FONT_10X20
    pub fn font_10x20() -> Self {
        Self {
            char_width: 10.0,
            char_height: 20.0,
            bold_char_width: 10.0,
            italic_char_width: 10.0,
        }
    }

    /// Get character width for a specific style
    pub fn char_width_for_style(&self, style: TextStyle) -> f32 {
        match style {
            TextStyle::Normal | TextStyle::Italic => self.char_width,
            TextStyle::Bold | TextStyle::BoldItalic => self.bold_char_width,
        }
    }

    /// Measure text width for given style
    pub fn text_width(&self, text: &str, style: TextStyle) -> f32 {
        text.chars().count() as f32 * self.char_width_for_style(style)
    }
}

/// Layout engine for converting tokens to paginated content
pub struct LayoutEngine {
    /// Available page width (pixels)
    page_width: f32,
    /// Line height in pixels
    line_height: f32,
    /// Font metrics for text measurement
    font_metrics: FontMetrics,
    /// Left margin in pixels
    left_margin: f32,
    /// Top margin in pixels
    top_margin: f32,
    /// Finalized spans for the current line
    current_spans: Vec<TextSpan>,
    /// Text being accumulated for the current span
    current_span_text: String,
    /// Style of the current span being built
    current_span_style: TextStyle,
    /// Current Y position on page
    current_y: f32,
    /// Current line width used
    current_line_width: f32,
    /// Current page being built
    current_page_lines: Vec<Line>,
    /// Completed pages
    pages: Vec<Page>,
    /// Current page number
    page_number: usize,
    /// Maximum lines per page
    max_lines_per_page: usize,
    /// Current line count on page
    current_line_count: usize,
    /// Current list nesting depth
    list_depth: usize,
    /// Stack tracking ordered vs unordered at each nesting level
    list_ordered_stack: Vec<bool>,
    /// Item counter at each list nesting level
    list_item_counters: Vec<usize>,
}

impl LayoutEngine {
    /// Default display width in pixels
    pub const DISPLAY_WIDTH: f32 = 480.0;
    /// Default display height in pixels
    pub const DISPLAY_HEIGHT: f32 = 800.0;
    /// Default side margins in pixels
    pub const DEFAULT_MARGIN: f32 = 32.0;
    /// Top margin - minimal
    pub const DEFAULT_TOP_MARGIN: f32 = 0.0;
    /// Header area for title (must match renderer HEADER_HEIGHT)
    pub const DEFAULT_HEADER_HEIGHT: f32 = 45.0;
    /// Footer area for progress (must match renderer FOOTER_HEIGHT)
    pub const DEFAULT_FOOTER_HEIGHT: f32 = 40.0;

    /// Create a new layout engine
    ///
    /// # Arguments
    /// * `page_width` - Available width for content (excluding margins)
    /// * `page_height` - Available height for content (excluding header/footer)
    /// * `line_height` - Height of each line in pixels
    pub fn new(page_width: f32, page_height: f32, line_height: f32) -> Self {
        let font_metrics = FontMetrics::default();
        // Reserve 2 extra line heights: 1 for font descent, 1 for safety margin
        let max_lines = ((page_height - line_height * 2.0) / line_height)
            .floor()
            .max(1.0) as usize;

        Self {
            page_width,
            line_height,
            font_metrics,
            left_margin: Self::DEFAULT_MARGIN,
            top_margin: Self::DEFAULT_TOP_MARGIN,
            current_spans: Vec::with_capacity(8),
            current_span_text: String::with_capacity(32),
            current_span_style: TextStyle::Normal,
            current_y: Self::DEFAULT_TOP_MARGIN,
            current_line_width: 0.0,
            current_page_lines: Vec::with_capacity(8),
            pages: Vec::with_capacity(8),
            page_number: 1,
            max_lines_per_page: max_lines.max(1),
            current_line_count: 0,
            list_depth: 0,
            list_ordered_stack: Vec::with_capacity(8),
            list_item_counters: Vec::with_capacity(8),
        }
    }

    /// Create layout engine with default display dimensions
    ///
    /// Content area: 416x715 (accounting for margins, header, footer)
    /// Uses 10x20 font with 26px line height for comfortable reading
    pub fn with_defaults() -> Self {
        LayoutConfig::default().create_engine()
    }

    /// Set font metrics
    pub fn with_font_metrics(mut self, metrics: FontMetrics) -> Self {
        self.font_metrics = metrics;
        self
    }

    /// Set margins
    pub fn with_margins(mut self, left: f32, top: f32) -> Self {
        self.left_margin = left;
        self.top_margin = top;
        self.current_y = top;
        self
    }

    /// Convert tokens into laid-out pages
    pub fn layout_tokens(&mut self, tokens: &[Token]) -> Vec<Page> {
        self.reset();

        let mut bold_active = false;
        let mut italic_active = false;
        let mut heading_bold = false;

        for token in tokens {
            match token {
                Token::Text(ref text) => {
                    let style =
                        self.current_style_from_flags(bold_active || heading_bold, italic_active);
                    self.add_text(text, style);
                }
                Token::ParagraphBreak => {
                    self.flush_line();
                    self.add_paragraph_space();
                    heading_bold = false;
                }
                Token::Heading(level) => {
                    self.flush_line();
                    // Headings get extra space before (more space for higher level headings)
                    if self.current_line_count > 0 {
                        // Add 1-2 lines of space before heading based on level
                        let space_lines = if *level <= 2 { 2 } else { 1 };
                        for _ in 0..space_lines {
                            self.add_paragraph_space();
                        }
                    }
                    // Headings are always bold (via heading_bold, not bold_active)
                    heading_bold = true;
                    // Note: Currently we use same font size for all headings
                    // Future: could use larger fonts for h1-h2
                }
                Token::Emphasis(start) => {
                    self.flush_partial_word();
                    italic_active = *start;
                    self.current_span_style =
                        self.current_style_from_flags(bold_active || heading_bold, italic_active);
                }
                Token::Strong(start) => {
                    self.flush_partial_word();
                    bold_active = *start;
                    self.current_span_style =
                        self.current_style_from_flags(bold_active || heading_bold, italic_active);
                }
                Token::LineBreak => {
                    self.flush_line();
                }
                // List tokens — track nesting and emit bullet/number prefixes
                Token::ListStart(ordered) => {
                    self.flush_line();
                    self.list_depth += 1;
                    self.list_ordered_stack.push(*ordered);
                    self.list_item_counters.push(0);
                }
                Token::ListEnd => {
                    self.flush_line();
                    self.list_depth = self.list_depth.saturating_sub(1);
                    self.list_ordered_stack.pop();
                    self.list_item_counters.pop();
                    if self.list_depth == 0 {
                        self.add_paragraph_space();
                    }
                }
                Token::ListItemStart => {
                    self.flush_line();
                    // Increment item counter for the current list level
                    if let Some(counter) = self.list_item_counters.last_mut() {
                        *counter += 1;
                    }
                    // Build indentation prefix based on nesting depth
                    let indent = "  ".repeat(self.list_depth.saturating_sub(1));
                    let is_ordered = self.list_ordered_stack.last().copied().unwrap_or(false);
                    let marker = if is_ordered {
                        let count = self.list_item_counters.last().copied().unwrap_or(1);
                        format!("{}{}.", indent, count)
                    } else {
                        format!("{}\u{2022}", indent) // bullet: •
                    };
                    let marker_width = self.font_metrics.text_width(&marker, TextStyle::Normal);
                    self.current_span_text.push_str(&marker);
                    self.current_span_style = TextStyle::Normal;
                    self.current_line_width = marker_width;
                }
                Token::ListItemEnd => {
                    // Nothing needed — next ListItemStart or ListEnd handles spacing
                }
                // Link tokens — text content flows through as Token::Text normally
                Token::LinkStart(_href) => {
                    // Text inside the link renders normally via Token::Text
                    // Future: could track link state for underline rendering
                }
                Token::LinkEnd => {
                    // End of link — no special rendering needed
                }
                // Image tokens — render a placeholder line
                Token::Image { src: _, ref alt } => {
                    self.flush_line();
                    let placeholder = if alt.is_empty() {
                        String::from("[Image]")
                    } else {
                        format!("[Image: {}]", alt)
                    };
                    let width = self
                        .font_metrics
                        .text_width(&placeholder, TextStyle::Normal);
                    self.current_span_text = placeholder;
                    self.current_span_style = TextStyle::Normal;
                    self.current_line_width = width;
                    self.flush_line();
                    self.add_paragraph_space();
                }
            }
        }

        // Flush any remaining content
        self.flush_line();
        self.finalize_page();

        core::mem::take(&mut self.pages)
    }

    /// Reset the layout engine state
    fn reset(&mut self) {
        self.current_spans.clear();
        self.current_span_text.clear();
        self.current_span_style = TextStyle::Normal;
        self.current_y = self.top_margin;
        self.current_line_width = 0.0;
        self.current_page_lines.clear();
        self.pages.clear();
        self.page_number = 1;
        self.current_line_count = 0;
        self.list_depth = 0;
        self.list_ordered_stack.clear();
        self.list_item_counters.clear();
    }

    /// Get current style based on bold/italic flags
    fn current_style_from_flags(&self, bold: bool, italic: bool) -> TextStyle {
        match (bold, italic) {
            (true, true) => TextStyle::BoldItalic,
            (true, false) => TextStyle::Bold,
            (false, true) => TextStyle::Italic,
            (false, false) => TextStyle::Normal,
        }
    }

    /// Add text content, breaking into words and laying out
    fn add_text(&mut self, text: &str, style: TextStyle) {
        // Split text into words
        for word in text.split_whitespace() {
            self.add_word(word, style);
        }
    }

    /// Check if the current line being built is empty (no spans and no pending text)
    fn current_line_is_empty(&self) -> bool {
        self.current_spans.is_empty() && self.current_span_text.is_empty()
    }

    /// Add a single word with greedy line breaking
    fn add_word(&mut self, word: &str, style: TextStyle) {
        let word_width = self.font_metrics.text_width(word, style);
        let space_width = if self.current_line_is_empty() {
            0.0
        } else {
            self.font_metrics.char_width_for_style(style)
        };

        let total_width = self.current_line_width + space_width + word_width;

        if total_width <= self.page_width || self.current_line_is_empty() {
            // If style changed from current span, finalize previous span and start new
            if style != self.current_span_style {
                if !self.current_span_text.is_empty() {
                    self.current_spans.push(TextSpan::new(
                        core::mem::take(&mut self.current_span_text),
                        self.current_span_style,
                    ));
                }
                self.current_span_style = style;
            }
            // Word fits on current line
            if !self.current_line_is_empty() {
                self.current_span_text.push(' ');
                self.current_line_width += space_width;
            }
            self.current_span_text.push_str(word);
            self.current_line_width += word_width;
        } else {
            // Word doesn't fit, start new line
            self.flush_line();
            self.current_span_style = style;
            self.current_span_text.push_str(word);
            self.current_line_width = word_width;
        }
    }

    /// Flush current span text (used when style changes mid-line)
    fn flush_partial_word(&mut self) {
        if !self.current_span_text.is_empty() {
            self.current_spans.push(TextSpan::new(
                core::mem::take(&mut self.current_span_text),
                self.current_span_style,
            ));
        }
    }

    /// Flush current line and add to page
    fn flush_line(&mut self) {
        // Finalize current span
        if !self.current_span_text.is_empty() {
            self.current_spans.push(TextSpan::new(
                core::mem::take(&mut self.current_span_text),
                self.current_span_style,
            ));
        }

        if self.current_spans.is_empty() {
            return;
        }

        // Check if we need a new page
        if self.current_line_count >= self.max_lines_per_page {
            self.finalize_page();
            self.current_y = self.top_margin;
            self.current_line_count = 0;
        }

        // Create the line from accumulated spans
        let line = Line {
            spans: core::mem::take(&mut self.current_spans),
            y: self.current_y as i32,
        };

        self.current_page_lines.push(line);
        self.current_line_count += 1;
        self.current_y += self.line_height;
        self.current_line_width = 0.0;
    }

    /// Add paragraph spacing (half line for compact layout)
    fn add_paragraph_space(&mut self) {
        // Check if we need a new page for the space
        if self.current_line_count >= self.max_lines_per_page {
            self.finalize_page();
            self.current_y = self.top_margin;
            self.current_line_count = 0;
        }

        // Add half line space between paragraphs (12px for 24px line height)
        // This saves space while maintaining visual separation
        if self.current_line_count > 0 {
            self.current_y += self.line_height * 0.5;
        }
    }

    /// Finalize current page and start new one
    fn finalize_page(&mut self) {
        if !self.current_page_lines.is_empty() {
            let mut page = Page::new(self.page_number);
            core::mem::swap(&mut page.lines, &mut self.current_page_lines);
            self.pages.push(page);
            self.page_number += 1;
        }
    }

    /// Get the completed pages
    pub fn into_pages(mut self) -> Vec<Page> {
        self.finalize_page();
        self.pages
    }

    /// Get current page number
    pub fn current_page_number(&self) -> usize {
        self.page_number
    }

    /// Get total pages created so far
    pub fn total_pages(&self) -> usize {
        self.pages.len()
    }

    /// Measure text width for given string and style
    pub fn measure_text(&self, text: &str, style: TextStyle) -> f32 {
        self.font_metrics.text_width(text, style)
    }
}

/// Layout configuration for the engine
#[derive(Clone, Debug)]
pub struct LayoutConfig {
    /// Page width in pixels
    pub page_width: f32,
    /// Page height in pixels
    pub page_height: f32,
    /// Line height in pixels
    pub line_height: f32,
    /// Left margin in pixels
    pub left_margin: f32,
    /// Top margin in pixels
    pub top_margin: f32,
    /// Font metrics
    pub font_metrics: FontMetrics,
}

impl Default for LayoutConfig {
    /// Default configuration for e-reader display layout.
    ///
    /// Uses the same constants as `LayoutEngine::with_defaults()` for consistency.
    /// This provides a 416x715 content area with 26px line height for comfortable reading.
    fn default() -> Self {
        // Use LayoutEngine constants as single source of truth
        let content_width = LayoutEngine::DISPLAY_WIDTH - (LayoutEngine::DEFAULT_MARGIN * 2.0);
        let content_height = LayoutEngine::DISPLAY_HEIGHT
            - LayoutEngine::DEFAULT_HEADER_HEIGHT
            - LayoutEngine::DEFAULT_FOOTER_HEIGHT;

        Self {
            page_width: content_width,
            page_height: content_height,
            line_height: 26.0, // ~1.3x font height for comfortable reading
            left_margin: LayoutEngine::DEFAULT_MARGIN,
            top_margin: 0.0, // No top margin - header area handled separately
            font_metrics: FontMetrics::default(),
        }
    }
}

impl LayoutConfig {
    /// Create layout engine from this configuration
    pub fn create_engine(&self) -> LayoutEngine {
        LayoutEngine::new(self.page_width, self.page_height, self.line_height)
            .with_font_metrics(self.font_metrics.clone())
            .with_margins(self.left_margin, self.top_margin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tokens() -> Vec<Token> {
        vec![
            Token::Text("This is ".to_string()),
            Token::Emphasis(true),
            Token::Text("italic".to_string()),
            Token::Emphasis(false),
            Token::Text(" and ".to_string()),
            Token::Strong(true),
            Token::Text("bold".to_string()),
            Token::Strong(false),
            Token::Text(" text.".to_string()),
            Token::ParagraphBreak,
            Token::Heading(1),
            Token::Text("Chapter Title".to_string()),
            Token::ParagraphBreak,
            Token::Text("Another paragraph with more content here.".to_string()),
            Token::ParagraphBreak,
        ]
    }

    #[test]
    fn test_layout_engine_new() {
        let engine = LayoutEngine::new(460.0, 650.0, 20.0);
        assert_eq!(engine.current_page_number(), 1);
        assert_eq!(engine.total_pages(), 0);
    }

    #[test]
    fn test_text_style() {
        let mut style = TextStyle::Normal;
        assert!(!style.is_bold());
        assert!(!style.is_italic());

        style = style.with_bold(true);
        assert!(style.is_bold());
        assert!(!style.is_italic());

        style = style.with_italic(true);
        assert!(style.is_bold());
        assert!(style.is_italic());

        style = style.with_bold(false);
        assert!(!style.is_bold());
        assert!(style.is_italic());
    }

    #[test]
    fn test_layout_tokens_basic() {
        let tokens = create_test_tokens();
        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        assert_eq!(pages[0].page_number, 1);

        // Check that we have lines
        let total_lines: usize = pages.iter().map(|p| p.line_count()).sum();
        assert!(total_lines > 0);
    }

    #[test]
    fn test_pagination() {
        // Create a lot of text to force pagination
        let mut tokens = Vec::with_capacity(8);
        for i in 0..50 {
            tokens.push(Token::Text(format!(
                "This is paragraph number {} with some content. ",
                i
            )));
            tokens.push(Token::Text(
                "Here is more text to fill the line. ".to_string(),
            ));
            tokens.push(Token::Text(
                "And even more words here to make it long enough.".to_string(),
            ));
            tokens.push(Token::ParagraphBreak);
        }

        let mut engine = LayoutEngine::new(460.0, 200.0, 20.0); // Small page height
        let pages = engine.layout_tokens(&tokens);

        // Should have multiple pages
        assert!(pages.len() > 1);

        // Page numbers should be sequential
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.page_number, i + 1);
        }
    }

    #[test]
    fn test_line_breaking() {
        // Create text that should wrap
        let tokens = vec![
            Token::Text("This is a very long line of text that should definitely wrap to multiple lines because it is longer than the available width".to_string()),
            Token::ParagraphBreak,
        ];

        let mut engine = LayoutEngine::new(100.0, 200.0, 20.0); // Narrow page
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        // Should have multiple lines on the page
        assert!(pages[0].line_count() > 1);
    }

    #[test]
    fn test_empty_input() {
        let tokens: Vec<Token> = vec![];
        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        // Should have no pages for empty input
        assert!(pages.is_empty());
    }

    #[test]
    fn test_font_metrics() {
        let metrics = FontMetrics::default(); // default is 10x20
        assert_eq!(metrics.text_width("hello", TextStyle::Normal), 50.0); // 5 * 10.0
        assert_eq!(metrics.text_width("hello", TextStyle::Bold), 50.0); // 5 * 10.0

        let metrics_10x20 = FontMetrics::font_10x20();
        assert_eq!(metrics_10x20.text_width("hello", TextStyle::Normal), 50.0);
    }

    #[test]
    fn test_page_struct() {
        let mut page = Page::new(1);
        assert!(page.is_empty());
        assert_eq!(page.line_count(), 0);

        page.add_line(Line::new("Test".to_string(), 10, TextStyle::Normal));
        assert!(!page.is_empty());
        assert_eq!(page.line_count(), 1);
    }

    #[test]
    fn test_line_struct() {
        let line = Line::new("Hello".to_string(), 50, TextStyle::Bold);
        assert_eq!(line.text(), "Hello");
        assert_eq!(line.y, 50);
        assert_eq!(line.style(), TextStyle::Bold);
        assert!(!line.is_empty());
        assert_eq!(line.len(), 5);
    }

    #[test]
    fn test_layout_config() {
        let config = LayoutConfig::default();
        // Content width: 480 - 2*32 = 416
        assert_eq!(config.page_width, 416.0);
        // Content height: 800 - 45 - 40 = 715
        assert_eq!(config.page_height, 715.0);
        // Line height: 26px for comfortable reading (~1.3x font height)
        assert_eq!(config.line_height, 26.0);
        // Margins: left=32 (default), top=0 (header handled separately)
        assert_eq!(config.left_margin, 32.0);
        assert_eq!(config.top_margin, 0.0);

        let engine = config.create_engine();
        assert_eq!(engine.current_page_number(), 1);
    }

    #[test]
    fn test_with_defaults() {
        let engine = LayoutEngine::with_defaults();
        assert_eq!(engine.current_page_number(), 1);
        // Default content area: 480 - 2*32 = 416 width
        // 800 - 45 - 40 = 715 height
    }

    /// Helper: collect all line texts from laid-out pages
    fn collect_line_texts(pages: &[Page]) -> Vec<String> {
        pages
            .iter()
            .flat_map(|p| p.lines.iter())
            .map(|l| l.text())
            .collect()
    }

    #[test]
    fn test_unordered_list_layout() {
        let tokens = vec![
            Token::ListStart(false),
            Token::ListItemStart,
            Token::Text("First".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Second".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "\u{2022} First");
        assert_eq!(texts[1], "\u{2022} Second");
    }

    #[test]
    fn test_ordered_list_layout() {
        let tokens = vec![
            Token::ListStart(true),
            Token::ListItemStart,
            Token::Text("Alpha".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Beta".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Gamma".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 3);
        assert_eq!(texts[0], "1. Alpha");
        assert_eq!(texts[1], "2. Beta");
        assert_eq!(texts[2], "3. Gamma");
    }

    #[test]
    fn test_nested_list_layout() {
        let tokens = vec![
            Token::ListStart(false),
            Token::ListItemStart,
            Token::Text("Outer".to_string()),
            Token::ListItemEnd,
            // Nested ordered list
            Token::ListStart(true),
            Token::ListItemStart,
            Token::Text("Inner A".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Inner B".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
            Token::ListItemStart,
            Token::Text("Outer again".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 4);
        // Depth 1 unordered — no indentation
        assert_eq!(texts[0], "\u{2022} Outer");
        // Depth 2 ordered — 2-space indentation
        assert_eq!(texts[1], "  1. Inner A");
        assert_eq!(texts[2], "  2. Inner B");
        // Back to depth 1 unordered
        assert_eq!(texts[3], "\u{2022} Outer again");
    }

    #[test]
    fn test_image_placeholder_with_alt() {
        let tokens = vec![Token::Image {
            src: "img/cover.jpg".to_string(),
            alt: "Book cover".to_string(),
        }];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "[Image: Book cover]");
    }

    #[test]
    fn test_image_placeholder_without_alt() {
        let tokens = vec![Token::Image {
            src: "img/photo.png".to_string(),
            alt: String::with_capacity(32),
        }];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "[Image]");
    }

    #[test]
    fn test_link_text_renders_normally() {
        let tokens = vec![
            Token::Text("Click ".to_string()),
            Token::LinkStart("https://example.com".to_string()),
            Token::Text("here".to_string()),
            Token::LinkEnd,
            Token::Text(" for info.".to_string()),
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        // Link text should render inline with surrounding text
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "Click here for info.");
    }

    #[test]
    fn test_mixed_content() {
        let tokens = vec![
            // Heading
            Token::Heading(1),
            Token::Text("My Chapter".to_string()),
            Token::ParagraphBreak,
            // Paragraph
            Token::Text("Some introductory text.".to_string()),
            Token::ParagraphBreak,
            // Unordered list
            Token::ListStart(false),
            Token::ListItemStart,
            Token::Text("Item one".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Item two".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
            // Image
            Token::Image {
                src: "fig1.png".to_string(),
                alt: "Figure 1".to_string(),
            },
            // Link in paragraph
            Token::Text("Visit ".to_string()),
            Token::LinkStart("https://example.com".to_string()),
            Token::Text("example".to_string()),
            Token::LinkEnd,
            Token::Text(" site.".to_string()),
            Token::ParagraphBreak,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        // Verify all content types appear in order
        assert!(texts.len() >= 6);
        assert_eq!(texts[0], "My Chapter");
        assert_eq!(texts[1], "Some introductory text.");
        assert_eq!(texts[2], "\u{2022} Item one");
        assert_eq!(texts[3], "\u{2022} Item two");
        assert_eq!(texts[4], "[Image: Figure 1]");
        assert_eq!(texts[5], "Visit example site.");
    }

    #[test]
    fn test_list_counters_reset_between_lists() {
        let tokens = vec![
            // First ordered list
            Token::ListStart(true),
            Token::ListItemStart,
            Token::Text("A".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("B".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
            // Second ordered list — counters should restart at 1
            Token::ListStart(true),
            Token::ListItemStart,
            Token::Text("X".to_string()),
            Token::ListItemEnd,
            Token::ListItemStart,
            Token::Text("Y".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);
        let texts = collect_line_texts(&pages);

        assert_eq!(texts.len(), 4);
        assert_eq!(texts[0], "1. A");
        assert_eq!(texts[1], "2. B");
        // Second list resets
        assert_eq!(texts[2], "1. X");
        assert_eq!(texts[3], "2. Y");
    }

    // -- Additional edge case tests ---

    #[test]
    fn test_layout_all_token_types_together() {
        // Exercise every token variant in a single layout pass
        let tokens = vec![
            Token::Heading(2),
            Token::Text("Title".to_string()),
            Token::ParagraphBreak,
            Token::Text("Normal ".to_string()),
            Token::Strong(true),
            Token::Text("bold".to_string()),
            Token::Strong(false),
            Token::Emphasis(true),
            Token::Text("italic".to_string()),
            Token::Emphasis(false),
            Token::ParagraphBreak,
            Token::ListStart(false),
            Token::ListItemStart,
            Token::Text("Bullet".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
            Token::ListStart(true),
            Token::ListItemStart,
            Token::Text("Numbered".to_string()),
            Token::ListItemEnd,
            Token::ListEnd,
            Token::LinkStart("http://example.com".to_string()),
            Token::Text("link text".to_string()),
            Token::LinkEnd,
            Token::ParagraphBreak,
            Token::Image {
                src: "img.png".to_string(),
                alt: "Alt text".to_string(),
            },
            Token::LineBreak,
            Token::Text("Final line.".to_string()),
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        let total_lines: usize = pages.iter().map(|p| p.line_count()).sum();
        assert!(
            total_lines >= 5,
            "Expected at least 5 lines, got {}",
            total_lines
        );
    }

    #[test]
    fn test_layout_only_headings_no_body() {
        let tokens = vec![
            Token::Heading(1),
            Token::Text("Chapter One".to_string()),
            Token::ParagraphBreak,
            Token::Heading(2),
            Token::Text("Section A".to_string()),
            Token::ParagraphBreak,
            Token::Heading(3),
            Token::Text("Subsection i".to_string()),
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        let texts = collect_line_texts(&pages);
        assert!(texts.len() >= 3);

        // All heading text should be bold
        for page in &pages {
            for line in &page.lines {
                if !line.is_empty() {
                    assert!(
                        line.style().is_bold(),
                        "Heading line '{}' should be bold, was {:?}",
                        line.text(),
                        line.style()
                    );
                }
            }
        }
    }

    #[test]
    fn test_layout_very_long_single_word() {
        // A word much wider than the page width
        let long_word = "superlongwordthatdoesnotfitinpagewidthatall";
        let tokens = vec![Token::Text(long_word.to_string()), Token::ParagraphBreak];

        // 100px page width / 10px per char = 10 chars fit
        let mut engine = LayoutEngine::new(100.0, 400.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        // The word should be placed even though it overflows — greedy algorithm
        // allows a word on an empty line regardless of width
        let texts = collect_line_texts(&pages);
        assert!(texts.iter().any(|t| t == long_word));
    }

    #[test]
    fn test_page_boundary_exact_fill() {
        // Create a small page: height=100, line_height=20
        // max_lines = floor((100 - 2*20) / 20) = 3
        let mut engine = LayoutEngine::new(400.0, 100.0, 20.0);

        // Exactly 3 lines of content
        let tokens = vec![
            Token::Text("Line one text".to_string()),
            Token::LineBreak,
            Token::Text("Line two text".to_string()),
            Token::LineBreak,
            Token::Text("Line three text".to_string()),
        ];

        let pages = engine.layout_tokens(&tokens);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].line_count(), 3);
    }

    #[test]
    fn test_page_boundary_overflow_by_one() {
        // 3 lines fit, 4th should overflow
        let mut engine = LayoutEngine::new(400.0, 100.0, 20.0);

        let tokens = vec![
            Token::Text("Line one".to_string()),
            Token::LineBreak,
            Token::Text("Line two".to_string()),
            Token::LineBreak,
            Token::Text("Line three".to_string()),
            Token::LineBreak,
            Token::Text("Line four overflow".to_string()),
        ];

        let pages = engine.layout_tokens(&tokens);
        assert!(pages.len() >= 2, "Expected 2+ pages, got {}", pages.len());
        assert_eq!(pages[0].line_count(), 3);
        assert!(pages[1].line_count() >= 1);
    }

    #[test]
    fn test_style_transitions_in_paragraph() {
        // normal → bold → italic → bolditalic → normal
        let tokens = vec![
            Token::Text("normal".to_string()),
            Token::Strong(true),
            Token::Text("bold".to_string()),
            Token::Strong(false),
            Token::Emphasis(true),
            Token::Text("italic".to_string()),
            Token::Strong(true),
            Token::Text("bolditalic".to_string()),
            Token::Strong(false),
            Token::Emphasis(false),
            Token::Text("normal_again".to_string()),
        ];

        // Very wide page so everything fits on one line
        let mut engine = LayoutEngine::new(2000.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        let texts = collect_line_texts(&pages);
        // All words should end up in the output
        let joined = texts.join(" ");
        assert!(joined.contains("normal"));
        assert!(joined.contains("bold"));
        assert!(joined.contains("italic"));
        assert!(joined.contains("bolditalic"));
        assert!(joined.contains("normal_again"));
    }

    #[test]
    fn test_multiple_paragraph_breaks_in_sequence() {
        let tokens = vec![
            Token::Text("First paragraph.".to_string()),
            Token::ParagraphBreak,
            Token::ParagraphBreak,
            Token::ParagraphBreak,
            Token::Text("After multiple breaks.".to_string()),
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        let texts = collect_line_texts(&pages);
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[0], "First paragraph.");
        assert_eq!(texts[1], "After multiple breaks.");
    }

    #[test]
    fn test_layout_with_custom_font_metrics() {
        let custom_metrics = FontMetrics {
            char_width: 8.0,
            char_height: 16.0,
            bold_char_width: 9.0,
            italic_char_width: 8.0,
        };

        // Verify metric calculations
        assert_eq!(custom_metrics.text_width("hello", TextStyle::Normal), 40.0);
        assert_eq!(custom_metrics.text_width("hello", TextStyle::Bold), 45.0);
        assert_eq!(custom_metrics.char_width_for_style(TextStyle::Italic), 8.0);
        assert_eq!(
            custom_metrics.char_width_for_style(TextStyle::BoldItalic),
            9.0
        );

        // Use in engine
        let tokens = vec![
            Token::Text("Testing custom font metrics.".to_string()),
            Token::ParagraphBreak,
        ];
        let mut engine = LayoutEngine::new(200.0, 400.0, 20.0).with_font_metrics(custom_metrics);
        let pages = engine.layout_tokens(&tokens);
        assert!(!pages.is_empty());
    }

    #[test]
    fn test_layout_config_create_engine_works() {
        let config = LayoutConfig {
            page_width: 300.0,
            page_height: 500.0,
            line_height: 18.0,
            left_margin: 15.0,
            top_margin: 20.0,
            font_metrics: FontMetrics {
                char_width: 8.0,
                char_height: 16.0,
                bold_char_width: 9.0,
                italic_char_width: 8.0,
            },
        };

        let mut engine = config.create_engine();
        assert_eq!(engine.current_page_number(), 1);

        let tokens = vec![
            Token::Text("Config engine test.".to_string()),
            Token::ParagraphBreak,
            Token::Text("Second paragraph.".to_string()),
        ];
        let pages = engine.layout_tokens(&tokens);
        assert!(!pages.is_empty());
        assert!(pages[0].line_count() >= 1);
    }

    #[test]
    fn test_layout_default_config_create_engine() {
        let config = LayoutConfig::default();
        let mut engine = config.create_engine();

        let tokens = vec![
            Token::Text("Default config test.".to_string()),
            Token::ParagraphBreak,
        ];
        let pages = engine.layout_tokens(&tokens);
        assert!(!pages.is_empty());
    }

    #[test]
    fn test_layout_zero_length_text_tokens() {
        let tokens = vec![
            Token::Text(String::with_capacity(32)),
            Token::Text("visible".to_string()),
            Token::Text(String::with_capacity(32)),
            Token::ParagraphBreak,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        let texts = collect_line_texts(&pages);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "visible");
    }

    #[test]
    fn test_heading_gets_extra_space() {
        // When heading follows body text, it should have more spacing
        let tokens = vec![
            Token::Text("Intro paragraph.".to_string()),
            Token::ParagraphBreak,
            Token::Heading(1),
            Token::Text("Chapter Title".to_string()),
            Token::ParagraphBreak,
            Token::Text("Body text.".to_string()),
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        let intro_line = &pages[0].lines[0];
        let heading_line = pages[0]
            .lines
            .iter()
            .find(|l| l.text().contains("Chapter Title"))
            .expect("Heading line should exist");

        // Heading should be bold
        assert!(heading_line.style().is_bold());

        // Gap between intro and heading should be bigger than a normal line gap
        let gap = heading_line.y - intro_line.y;
        assert!(
            gap > 20,
            "Expected extra spacing before heading, gap was {}",
            gap
        );
    }

    #[test]
    fn test_heading_level_spacing_difference() {
        // h1/h2 should get 2 lines of extra space; h3+ only 1 line
        let tokens_h1 = vec![
            Token::Text("Intro.".to_string()),
            Token::ParagraphBreak,
            Token::Heading(1),
            Token::Text("H1 Title".to_string()),
        ];
        let tokens_h4 = vec![
            Token::Text("Intro.".to_string()),
            Token::ParagraphBreak,
            Token::Heading(4),
            Token::Text("H4 Title".to_string()),
        ];

        let mut engine1 = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages_h1 = engine1.layout_tokens(&tokens_h1);

        let mut engine4 = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages_h4 = engine4.layout_tokens(&tokens_h4);

        let h1_y = pages_h1[0]
            .lines
            .iter()
            .find(|l| l.text().contains("H1 Title"))
            .unwrap()
            .y;
        let h4_y = pages_h4[0]
            .lines
            .iter()
            .find(|l| l.text().contains("H4 Title"))
            .unwrap()
            .y;

        // h1 gets 2 lines of extra space, h4 gets 1 → h1 should have larger Y
        assert!(
            h1_y > h4_y,
            "h1 spacing (y={}) should be greater than h4 spacing (y={})",
            h1_y,
            h4_y
        );
    }

    #[test]
    fn test_layout_engine_reuse() {
        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);

        // First layout
        let tokens1 = vec![Token::Text("First run.".to_string()), Token::ParagraphBreak];
        let pages1 = engine.layout_tokens(&tokens1);
        assert!(!pages1.is_empty());

        // Second layout — engine should be reset
        let tokens2 = vec![
            Token::Text("Second run.".to_string()),
            Token::ParagraphBreak,
        ];
        let pages2 = engine.layout_tokens(&tokens2);
        assert!(!pages2.is_empty());
        assert_eq!(pages2[0].page_number, 1);
        assert_eq!(pages2[0].lines[0].text(), "Second run.");
    }

    #[test]
    fn test_line_empty_and_len_edge_cases() {
        let empty_line = Line::new(String::with_capacity(32), 0, TextStyle::Normal);
        assert!(empty_line.is_empty());
        assert_eq!(empty_line.len(), 0);

        let line = Line::new("Hello World!".to_string(), 100, TextStyle::Italic);
        assert!(!line.is_empty());
        assert_eq!(line.len(), 12);
        assert_eq!(line.style(), TextStyle::Italic);
    }

    #[test]
    fn test_text_style_combinations() {
        // Normal → with_bold(true) → Bold
        assert_eq!(TextStyle::Normal.with_bold(true), TextStyle::Bold);
        // Normal → with_italic(true) → Italic
        assert_eq!(TextStyle::Normal.with_italic(true), TextStyle::Italic);
        // Bold → with_italic(true) → BoldItalic
        assert_eq!(TextStyle::Bold.with_italic(true), TextStyle::BoldItalic);
        // BoldItalic → with_bold(false) → Italic
        assert_eq!(TextStyle::BoldItalic.with_bold(false), TextStyle::Italic);
        // BoldItalic → with_italic(false) → Bold
        assert_eq!(TextStyle::BoldItalic.with_italic(false), TextStyle::Bold);
        // Idempotent
        assert_eq!(TextStyle::Bold.with_bold(true), TextStyle::Bold);
        assert_eq!(TextStyle::Normal.with_bold(false), TextStyle::Normal);
    }

    #[test]
    fn test_measure_text_various() {
        let engine = LayoutEngine::new(460.0, 650.0, 20.0);
        // Default 10x20 font: 10px per char
        assert_eq!(engine.measure_text("hello", TextStyle::Normal), 50.0);
        assert_eq!(engine.measure_text("", TextStyle::Normal), 0.0);
        assert_eq!(engine.measure_text("a", TextStyle::Bold), 10.0);
        assert_eq!(engine.measure_text("test string", TextStyle::Italic), 110.0);
    }

    #[test]
    fn test_with_margins_affects_layout() {
        let tokens = vec![
            Token::Text("Margin test.".to_string()),
            Token::ParagraphBreak,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0).with_margins(25.0, 40.0);
        let pages = engine.layout_tokens(&tokens);

        assert!(!pages.is_empty());
        // First line Y should start at the top margin
        assert_eq!(pages[0].lines[0].y, 40);
    }

    #[test]
    fn test_into_pages_empty_engine() {
        let engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.into_pages();
        assert!(pages.is_empty());
    }

    #[test]
    fn test_layout_whitespace_only_text() {
        let tokens = vec![
            Token::Text("   ".to_string()),
            Token::Text("visible".to_string()),
            Token::ParagraphBreak,
        ];

        let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        // Whitespace-only text produces no words after split_whitespace
        let texts = collect_line_texts(&pages);
        assert_eq!(texts.len(), 1);
        assert_eq!(texts[0], "visible");
    }

    #[test]
    fn test_large_document_many_paragraphs() {
        let mut tokens = Vec::with_capacity(8);
        for i in 0..50 {
            tokens.push(Token::Text(alloc::format!(
                "Paragraph {} with enough text to be meaningful.",
                i
            )));
            tokens.push(Token::ParagraphBreak);
        }

        let mut engine = LayoutEngine::new(460.0, 200.0, 20.0);
        let pages = engine.layout_tokens(&tokens);

        // Should produce multiple pages
        assert!(pages.len() > 1);

        // Page numbers should be sequential
        for (i, page) in pages.iter().enumerate() {
            assert_eq!(page.page_number, i + 1);
        }

        // Total lines should account for all 50 paragraphs
        let total_lines: usize = pages.iter().map(|p| p.line_count()).sum();
        assert!(total_lines >= 50);
    }
}
