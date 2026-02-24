//! Regression tests for known bugs.
//!
//! Each test documents a specific bug. Once fixed, these become permanent
//! regression tests to prevent regressions.

// =============================================================================
// XML Entity Handling
// =============================================================================

#[test]
fn xml_entity_ampersand_unescaped() {
    use epub_stream::tokenizer::{tokenize_html, Token};
    let html = "<p>Barnes &amp; Noble</p>";
    let tokens = tokenize_html(html).unwrap();
    let text: String = tokens
        .iter()
        .filter_map(|t| match t {
            Token::Text(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains("Barnes & Noble") || text.contains("&"),
        "Entity &amp; should be unescaped to &, got: {:?}",
        text
    );
    assert!(
        !text.contains("&amp;"),
        "Literal &amp; should not appear in output"
    );
}

#[test]
fn xml_entity_less_greater_than_unescaped() {
    use epub_stream::tokenizer::{tokenize_html, Token};
    let html = "<p>x &lt; y &gt; z</p>";
    let tokens = tokenize_html(html).unwrap();
    let text: String = tokens
        .iter()
        .filter_map(|t| match t {
            Token::Text(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        text.contains('<') && text.contains('>'),
        "Entities &lt; and &gt; should be unescaped, got: {:?}",
        text
    );
}

#[test]
fn xml_entity_numeric_unescaped() {
    use epub_stream::tokenizer::{tokenize_html, Token};
    let html = "<p>&#8220;Hello&#8221;</p>";
    let tokens = tokenize_html(html).unwrap();
    let text: String = tokens
        .iter()
        .filter_map(|t| match t {
            Token::Text(s) => Some(s.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert!(
        text.contains('\u{201C}') && text.contains('\u{201D}'),
        "Numeric entities should be unescaped to actual chars, got: {:?}",
        text
    );
}

// =============================================================================
// Heading Style Isolation
// =============================================================================

#[cfg(feature = "layout")]
#[test]
fn heading_bold_does_not_bleed_into_body() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    let tokens = vec![
        Token::Heading(1),
        Token::Text("Title".to_string()),
        Token::ParagraphBreak,
        Token::Text("Body text after heading.".to_string()),
        Token::ParagraphBreak,
    ];
    let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);
    let body_line = pages
        .iter()
        .flat_map(|p| &p.lines)
        .find(|l| l.text().contains("Body"))
        .expect("Should have a line with 'Body'");
    assert_eq!(
        body_line.style(),
        TextStyle::Normal,
        "Body text after heading should NOT be bold, but got {:?}",
        body_line.style()
    );
}

// =============================================================================
// Text Width Measurement
// =============================================================================

#[cfg(feature = "layout")]
#[test]
fn text_width_uses_char_count_not_bytes() {
    use epub_stream::layout::{FontMetrics, TextStyle};
    let metrics = FontMetrics::font_10x20();
    let width = metrics.text_width("café", TextStyle::Normal);
    assert_eq!(
        width, 40.0,
        "text_width should use char count (4), not byte count (5)"
    );
}

#[cfg(feature = "layout")]
#[test]
fn text_width_em_dash_correct() {
    use epub_stream::layout::{FontMetrics, TextStyle};
    let metrics = FontMetrics::font_10x20();
    let width = metrics.text_width("—", TextStyle::Normal);
    assert_eq!(
        width, 10.0,
        "em-dash should be 1 char width (10.0), not 3 bytes (30.0)"
    );
}

#[cfg(feature = "layout")]
#[test]
fn layout_new_uses_default_top_margin() {
    use epub_stream::layout::LayoutEngine;
    use epub_stream::tokenizer::Token;

    let tokens = vec![Token::Text("Line one".to_string()), Token::ParagraphBreak];
    let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    let first_line = pages
        .first()
        .and_then(|page| page.lines.first())
        .expect("Expected at least one laid out line");
    assert_eq!(
        first_line.y, 0,
        "LayoutEngine::new should start at DEFAULT_TOP_MARGIN (0)"
    );
}

// =============================================================================
// CSS Line Height Parsing
// =============================================================================

#[test]
fn css_line_height_unitless_parsed_as_multiplier() {
    use epub_stream::css::{parse_stylesheet, LineHeight};
    let css = "p { line-height: 1.5; }";
    let ss = parse_stylesheet(css).unwrap();
    assert_eq!(
        ss.rules[0].style.line_height,
        Some(LineHeight::Multiplier(1.5)),
        "Unitless line-height 1.5 should be stored as LineHeight::Multiplier(1.5)"
    );
}

#[test]
fn css_line_height_pixels_parsed_correctly() {
    use epub_stream::css::{parse_stylesheet, LineHeight};
    let css = "p { line-height: 24px; }";
    let ss = parse_stylesheet(css).unwrap();
    assert_eq!(
        ss.rules[0].style.line_height,
        Some(LineHeight::Px(24.0)),
        "line-height: 24px should be stored as LineHeight::Px(24.0)"
    );
}

// =============================================================================
// Navigation Label Handling
// =============================================================================

#[test]
fn nav_label_concatenates_formatted_anchors() {
    use epub_stream::navigation::parse_nav_xhtml;
    let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml">Part <em>One</em></a></li>
  </ol>
</nav>
</body>
</html>"#;
    let nav = parse_nav_xhtml(nav_xhtml).unwrap();
    assert_eq!(nav.toc.len(), 1);
    assert_eq!(
        nav.toc[0].label, "Part One",
        "Nav label should concatenate all text nodes, got: {:?}",
        nav.toc[0].label
    );
}

// =============================================================================
// Metadata Parsing Precision
// =============================================================================

#[test]
fn metadata_subtitle_not_matched_as_title() {
    use epub_stream::metadata::parse_opf;
    let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Real Title</dc:title>
    <subtitle>Should Not Match</subtitle>
  </metadata>
  <manifest/>
</package>"#;
    let metadata = parse_opf(opf).unwrap();
    assert_eq!(
        metadata.title, "Real Title",
        "Subtitle should not overwrite title, got: {:?}",
        metadata.title
    );
}

#[test]
fn missing_title_and_author_distinguishable() {
    use epub_stream::metadata::parse_opf;
    let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
</package>"#;
    let metadata = parse_opf(opf).unwrap();
    assert!(
        metadata.title.is_empty(),
        "Missing title should be empty string, got: {:?}",
        metadata.title
    );
    assert!(
        metadata.author.is_empty(),
        "Missing author should be empty string, got: {:?}",
        metadata.author
    );
}

// =============================================================================
// Error Trait Implementations
// =============================================================================

#[test]
fn epub_error_implements_std_error() {
    use epub_stream::error::EpubError;
    fn assert_error<T: std::error::Error>() {}
    assert_error::<EpubError>();
}

#[test]
fn tokenize_error_implements_std_error() {
    use epub_stream::tokenizer::TokenizeError;
    fn assert_error<T: std::error::Error>() {}
    assert_error::<TokenizeError>();
}

#[test]
fn zip_error_implements_std_error() {
    use epub_stream::zip::ZipError;
    fn assert_error<T: std::error::Error>() {}
    assert_error::<ZipError>();
}

// =============================================================================
// API Surface Stability
// =============================================================================

#[test]
fn parser_apis_use_epub_error() {
    use epub_stream::css::{parse_inline_style, parse_stylesheet, CssStyle, Stylesheet};
    use epub_stream::error::EpubError;
    use epub_stream::metadata::{parse_container_xml, parse_opf, EpubMetadata};
    use epub_stream::navigation::{parse_nav_xhtml, parse_ncx, Navigation};
    use epub_stream::spine::{parse_opf_spine, parse_spine, Spine};

    let _parse_container_xml: fn(&[u8]) -> Result<String, EpubError> = parse_container_xml;
    let _parse_opf: fn(&[u8]) -> Result<EpubMetadata, EpubError> = parse_opf;
    let _parse_spine: fn(&[u8]) -> Result<Spine, EpubError> = parse_spine;
    let _parse_opf_spine: fn(&[u8]) -> Result<Spine, EpubError> = parse_opf_spine;
    let _parse_stylesheet: fn(&str) -> Result<Stylesheet, EpubError> = parse_stylesheet;
    let _parse_inline_style: fn(&str) -> Result<CssStyle, EpubError> = parse_inline_style;
    let _parse_nav_xhtml: fn(&[u8]) -> Result<Navigation, EpubError> = parse_nav_xhtml;
    let _parse_ncx: fn(&[u8]) -> Result<Navigation, EpubError> = parse_ncx;
}

#[test]
fn zip_error_alias_matches_kind() {
    use epub_stream::error::{ZipError, ZipErrorKind};

    fn takes_zip_error(err: ZipError) -> ZipErrorKind {
        err
    }

    let kind = ZipErrorKind::FileNotFound;
    let roundtrip = takes_zip_error(kind.clone());
    assert_eq!(roundtrip, kind);
}

// =============================================================================
// Mixed Formatting Preservation
// =============================================================================

#[cfg(feature = "layout")]
#[test]
fn mixed_formatting_preserved() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    let tokens = vec![
        Token::Text("normal ".to_string()),
        Token::Strong(true),
        Token::Text("bold".to_string()),
        Token::Strong(false),
        Token::Text(" text".to_string()),
        Token::ParagraphBreak,
    ];
    let mut engine = LayoutEngine::new(2000.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);
    let line = &pages[0].lines[0];
    assert!(
        line.spans.len() >= 2,
        "Expected multiple spans, got {}",
        line.spans.len()
    );
    let has_bold = line.spans.iter().any(|s| s.style == TextStyle::Bold);
    assert!(has_bold, "Should have a bold span");
    let has_normal = line.spans.iter().any(|s| s.style == TextStyle::Normal);
    assert!(has_normal, "Should have a normal span");
    assert_eq!(line.text(), "normal bold text");
}

#[cfg(feature = "layout")]
#[test]
fn mixed_formatting_multiple_transitions() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    // Test: normal → bold → italic → bolditalic → normal in one line
    let tokens = vec![
        Token::Text("normal ".to_string()),
        Token::Strong(true),
        Token::Text("bold ".to_string()),
        Token::Strong(false),
        Token::Emphasis(true),
        Token::Text("italic ".to_string()),
        Token::Strong(true),
        Token::Text("bolditalic ".to_string()),
        Token::Strong(false),
        Token::Emphasis(false),
        Token::Text("normal".to_string()),
        Token::ParagraphBreak,
    ];
    let mut engine = LayoutEngine::new(2000.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);
    let line = &pages[0].lines[0];

    // Should have 5 spans: normal, bold, italic, bolditalic, normal
    assert!(
        line.spans.len() >= 4,
        "Expected multiple spans for style transitions, got {}",
        line.spans.len()
    );

    // Verify each style is present
    assert!(
        line.spans.iter().any(|s| s.style == TextStyle::Normal),
        "Should have Normal span"
    );
    assert!(
        line.spans.iter().any(|s| s.style == TextStyle::Bold),
        "Should have Bold span"
    );
    assert!(
        line.spans.iter().any(|s| s.style == TextStyle::Italic),
        "Should have Italic span"
    );
    assert!(
        line.spans.iter().any(|s| s.style == TextStyle::BoldItalic),
        "Should have BoldItalic span"
    );

    // Verify complete text
    assert_eq!(line.text(), "normal bold italic bolditalic normal");
}

#[cfg(feature = "layout")]
#[test]
fn mixed_formatting_span_content_correct() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    let tokens = vec![
        Token::Text("Start ".to_string()),
        Token::Strong(true),
        Token::Text("bold".to_string()),
        Token::Strong(false),
        Token::Text(" End".to_string()),
        Token::ParagraphBreak,
    ];
    let mut engine = LayoutEngine::new(2000.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);
    let line = &pages[0].lines[0];

    // Note: The layout engine splits on whitespace and reconstructs with single spaces
    // Verify we have at least 3 spans with correct styles
    assert!(
        line.spans.len() >= 3,
        "Expected at least 3 spans, got {}",
        line.spans.len()
    );

    // Check styles are present in correct order
    assert_eq!(
        line.spans[0].style,
        TextStyle::Normal,
        "First span should be Normal"
    );
    assert!(
        line.spans[1].style == TextStyle::Bold,
        "Second span should be Bold"
    );
    // Last span should be Normal
    let last_span = line.spans.last().unwrap();
    assert_eq!(
        last_span.style,
        TextStyle::Normal,
        "Last span should be Normal"
    );

    // Verify complete text
    assert_eq!(line.text(), "Start bold End");
}

#[cfg(feature = "layout")]
#[test]
fn mixed_formatting_with_line_wrapping() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    // Create text that will wrap with mixed formatting
    let tokens = vec![
        Token::Text("First ".to_string()),
        Token::Strong(true),
        Token::Text("bold middle".to_string()),
        Token::Strong(false),
        Token::Text(" last words".to_string()),
        Token::ParagraphBreak,
    ];
    // Narrow page to force wrapping
    let mut engine = LayoutEngine::new(100.0, 400.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    // Should have multiple lines due to wrapping
    let total_lines: usize = pages.iter().map(|p| p.lines.len()).sum();
    assert!(
        total_lines >= 2,
        "Expected multiple lines due to wrapping, got {}",
        total_lines
    );

    // Verify that formatting is preserved across lines
    let all_spans: Vec<_> = pages
        .iter()
        .flat_map(|p| &p.lines)
        .flat_map(|l| &l.spans)
        .collect();
    let has_bold = all_spans.iter().any(|s| s.style == TextStyle::Bold);
    let has_normal = all_spans.iter().any(|s| s.style == TextStyle::Normal);
    assert!(has_bold, "Should have bold spans after wrapping");
    assert!(has_normal, "Should have normal spans after wrapping");
}

#[cfg(feature = "layout")]
#[test]
fn mixed_formatting_adjacent_styles() {
    use epub_stream::layout::{LayoutEngine, TextStyle};
    use epub_stream::tokenizer::Token;
    // Test adjacent formatting without space between
    let tokens = vec![
        Token::Text("A".to_string()),
        Token::Strong(true),
        Token::Text("B".to_string()),
        Token::Strong(false),
        Token::Text("C".to_string()),
        Token::ParagraphBreak,
    ];
    let mut engine = LayoutEngine::new(2000.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);
    let line = &pages[0].lines[0];

    // The layout engine adds spaces between words during layout
    assert_eq!(line.text(), "A B C");
    assert!(
        line.spans.len() >= 2,
        "Should have multiple spans for adjacent styles"
    );

    // Verify the bold span contains "B" (may have trailing space from word separation)
    let bold_span = line.spans.iter().find(|s| s.style == TextStyle::Bold);
    assert!(bold_span.is_some(), "Should have a bold span");
    assert!(
        bold_span.unwrap().text.contains('B'),
        "Bold span should contain 'B'"
    );
}
