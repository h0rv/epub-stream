//! XHTML to token stream converter for EPUB content
//!
//! Converts XHTML chapters into a simplified token format that's easier
//! to layout. Uses quick_xml for SAX-style parsing to handle large
//! documents efficiently without loading the entire DOM.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

/// Token types for simplified XHTML representation
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Token {
    /// Plain text content
    Text(String),
    /// New paragraph break
    ParagraphBreak,
    /// Heading with level 1-6
    Heading(u8),
    /// Start (true) or end (false) of italic emphasis
    Emphasis(bool),
    /// Start (true) or end (false) of bold strong
    Strong(bool),
    /// Line break (<br>)
    LineBreak,
    /// Start of a list (true = ordered, false = unordered)
    ListStart(bool),
    /// End of a list
    ListEnd,
    /// Start of a list item
    ListItemStart,
    /// End of a list item
    ListItemEnd,
    /// Start of a link with href
    LinkStart(String),
    /// End of a link
    LinkEnd,
    /// Image reference with src and alt text
    Image {
        /// Image source path (relative to EPUB content)
        src: String,
        /// Alternative text for the image
        alt: String,
    },
}

/// Error type for tokenization failures
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenizeError {
    /// XML parsing error
    ParseError(String),
    /// Invalid HTML structure
    InvalidStructure(String),
}

impl core::fmt::Display for TokenizeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TokenizeError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            TokenizeError::InvalidStructure(msg) => write!(f, "Invalid structure: {}", msg),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TokenizeError {}

/// Convert XHTML string into a token stream
///
/// Parses HTML tags: p, h1-h6, em, strong, br, span, div
/// Strips out: script, style, head, attributes (except class for styling)
/// Extracts text content and converts HTML entities
///
/// # Example
/// ```
/// use epub_stream::tokenizer::tokenize_html;
///
/// let html = "<p>Hello <em>world</em></p>";
/// let tokens = tokenize_html(html).unwrap();
/// ```
pub fn tokenize_html(html: &str) -> Result<Vec<Token>, TokenizeError> {
    // Estimate token count: roughly 1 token per 10 bytes of HTML
    let estimated_tokens = html.len() / 10;
    let cap = TokenizeLimits::default().max_initial_capacity;
    let mut tokens = Vec::with_capacity(estimated_tokens.min(cap));
    tokenize_html_into(html, &mut tokens)?;
    Ok(tokens)
}

/// Limits for bounded tokenization to prevent unbounded Vec growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenizeLimits {
    /// Maximum number of tokens to emit before returning an error.
    pub max_tokens: usize,
    /// Maximum nesting depth for element stack.
    pub max_nesting: usize,
    /// Maximum text node size in bytes before truncation.
    pub max_text_bytes: usize,
    /// Cap on the initial `Vec` capacity allocated from an estimate.
    ///
    /// Prevents a single large document from reserving an outsized
    /// allocation up-front.
    pub max_initial_capacity: usize,
}

impl Default for TokenizeLimits {
    fn default() -> Self {
        Self {
            max_tokens: 100_000,
            max_nesting: 256,
            max_text_bytes: 64 * 1024,
            max_initial_capacity: 10_000,
        }
    }
}

impl TokenizeLimits {
    /// Create limits suitable for embedded environments.
    pub fn embedded() -> Self {
        Self {
            max_tokens: 10_000,
            max_nesting: 64,
            max_text_bytes: 8 * 1024,
            max_initial_capacity: 2_000,
        }
    }
}

/// Convert XHTML string into a streamed token sequence.
///
/// This callback-oriented API keeps ownership of each token with the caller,
/// so downstream code can avoid storing a full token vector.
pub fn tokenize_html_with<F>(html: &str, mut on_token: F) -> Result<(), TokenizeError>
where
    F: FnMut(Token),
{
    for token in tokenize_html(html)? {
        on_token(token);
    }
    Ok(())
}

/// Convert XHTML string into a token stream with bounded limits.
///
/// Enforces `max_tokens` limit to prevent unbounded Vec growth on malicious
/// or extremely large inputs. Returns an error if limits are exceeded.
///
/// # Allocation behavior
/// - Allocates token Vec with capacity hints
/// - Returns error instead of unbounded growth
/// - Stack usage is bounded by `max_nesting`
pub fn tokenize_html_limited(
    html: &str,
    limits: TokenizeLimits,
) -> Result<Vec<Token>, TokenizeError> {
    let mut reader = Reader::from_str(html);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    let mut buf = Vec::with_capacity(8);
    let mut tokens = Vec::with_capacity(limits.max_tokens.min(1024));

    // Stack to track nested elements for proper closing
    let mut element_stack: Vec<ElementType> = Vec::with_capacity(limits.max_nesting.min(64));
    // Track if we're inside a tag that should be skipped (script, style, head)
    let mut skip_depth: usize = 0;
    // Track if we need a paragraph break after current block element
    let mut pending_paragraph_break: bool = false;
    // Track if we need a heading close after text content
    let mut pending_heading_close: Option<u8> = None;

    let mut token_count: usize = 0;
    let mut entity_buf = String::with_capacity(16);

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Check if we should skip this element and its children
                if should_skip_element(&name) {
                    skip_depth += 1;
                    continue;
                }

                // If skipping, don't process anything
                if skip_depth > 0 {
                    continue;
                }

                // Check nesting limit
                if element_stack.len() >= limits.max_nesting {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Nesting depth exceeds max_nesting ({})",
                        limits.max_nesting
                    )));
                }

                // Flush any pending paragraph break from previous block
                if pending_paragraph_break && !tokens.is_empty() {
                    if token_count >= limits.max_tokens {
                        return Err(TokenizeError::InvalidStructure(format!(
                            "Token count exceeds max_tokens ({}",
                            limits.max_tokens
                        )));
                    }
                    tokens.push(Token::ParagraphBreak);
                    token_count += 1;
                    pending_paragraph_break = false;
                }

                // Flush any pending heading close
                if let Some(level) = pending_heading_close.take() {
                    if token_count >= limits.max_tokens {
                        return Err(TokenizeError::InvalidStructure(format!(
                            "Token count exceeds max_tokens ({}",
                            limits.max_tokens
                        )));
                    }
                    tokens.push(Token::Heading(level));
                    token_count += 1;
                    pending_paragraph_break = true;
                }

                match &*name {
                    "p" | "div" => {
                        element_stack.push(ElementType::Paragraph);
                    }
                    "span" => {
                        element_stack.push(ElementType::Span);
                    }
                    h if h.starts_with('h') && h.len() == 2 => {
                        if let Some(level) = h.chars().nth(1).and_then(|c| c.to_digit(10)) {
                            if (1..=6).contains(&level) {
                                element_stack.push(ElementType::Heading(level as u8));
                                pending_heading_close = Some(level as u8);
                            }
                        }
                    }
                    "em" | "i" => {
                        element_stack.push(ElementType::Emphasis);
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Emphasis(true));
                        token_count += 1;
                    }
                    "strong" | "b" => {
                        element_stack.push(ElementType::Strong);
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Strong(true));
                        token_count += 1;
                    }
                    "ul" => {
                        element_stack.push(ElementType::UnorderedList);
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::ListStart(false));
                        token_count += 1;
                    }
                    "ol" => {
                        element_stack.push(ElementType::OrderedList);
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::ListStart(true));
                        token_count += 1;
                    }
                    "li" => {
                        element_stack.push(ElementType::ListItem);
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::ListItemStart);
                        token_count += 1;
                    }
                    "a" => {
                        if let Some(href) = get_attribute(&e, &reader, "href") {
                            element_stack.push(ElementType::Link);
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::LinkStart(href));
                            token_count += 1;
                        } else {
                            // No href — treat as generic container
                            element_stack.push(ElementType::Generic);
                        }
                    }
                    "img" => {
                        // <img> as a start tag (non-self-closing)
                        if let Some(src) = get_attribute(&e, &reader, "src") {
                            let alt = get_attribute(&e, &reader, "alt").unwrap_or_default();
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::Image { src, alt });
                            token_count += 1;
                        }
                        element_stack.push(ElementType::Generic);
                    }
                    _ => {
                        // Unknown element, treat as generic container
                        element_stack.push(ElementType::Generic);
                    }
                }
            }
            Ok(Event::Text(e)) => {
                // Skip text if we're inside a script/style/head block
                if skip_depth > 0 {
                    continue;
                }

                let text = e
                    .decode()
                    .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;

                // Normalize whitespace: collapse multiple spaces/newlines
                let normalized = normalize_whitespace_limited(text.as_ref(), limits.max_text_bytes);

                if !normalized.is_empty() {
                    // Flush any pending heading close
                    if let Some(level) = pending_heading_close.take() {
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Heading(level));
                        token_count += 1;
                    }
                    if token_count >= limits.max_tokens {
                        return Err(TokenizeError::InvalidStructure(format!(
                            "Token count exceeds max_tokens ({}",
                            limits.max_tokens
                        )));
                    }
                    tokens.push(Token::Text(normalized));
                    token_count += 1;
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Check if we're ending a skip element
                if should_skip_element(&name) {
                    skip_depth = skip_depth.saturating_sub(1);
                    continue;
                }

                // If skipping, don't process end tags
                if skip_depth > 0 {
                    continue;
                }

                // Pop the element from stack and emit appropriate close token
                if let Some(element) = element_stack.pop() {
                    match element {
                        ElementType::Paragraph => {
                            pending_paragraph_break = true;
                        }
                        ElementType::Heading(_level) => {
                            // Heading already emitted on start, just mark for paragraph break
                            pending_paragraph_break = true;
                            // Clear any pending close since we already handled it
                            pending_heading_close = None;
                        }
                        ElementType::Emphasis => {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::Emphasis(false));
                            token_count += 1;
                        }
                        ElementType::Strong => {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::Strong(false));
                            token_count += 1;
                        }
                        ElementType::UnorderedList | ElementType::OrderedList => {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::ListEnd);
                            token_count += 1;
                        }
                        ElementType::ListItem => {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::ListItemEnd);
                            token_count += 1;
                        }
                        ElementType::Link => {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::LinkEnd);
                            token_count += 1;
                        }
                        ElementType::Span | ElementType::Generic => {
                            // No tokens needed for these
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Skip empty elements inside script/style blocks
                if skip_depth > 0 {
                    continue;
                }

                // Flush any pending paragraph break
                if pending_paragraph_break && !tokens.is_empty() {
                    if token_count >= limits.max_tokens {
                        return Err(TokenizeError::InvalidStructure(format!(
                            "Token count exceeds max_tokens ({}",
                            limits.max_tokens
                        )));
                    }
                    tokens.push(Token::ParagraphBreak);
                    token_count += 1;
                    pending_paragraph_break = false;
                }

                // Flush any pending heading close
                if let Some(level) = pending_heading_close.take() {
                    if token_count >= limits.max_tokens {
                        return Err(TokenizeError::InvalidStructure(format!(
                            "Token count exceeds max_tokens ({}",
                            limits.max_tokens
                        )));
                    }
                    tokens.push(Token::Heading(level));
                    token_count += 1;
                    pending_paragraph_break = true;
                }

                match &*name {
                    "br" => {
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::LineBreak);
                        token_count += 1;
                    }
                    "p" | "div" => {
                        // Empty paragraph still creates a paragraph break
                        pending_paragraph_break = true;
                    }
                    h if h.starts_with('h') && h.len() == 2 => {
                        if let Some(level) = h.chars().nth(1).and_then(|c| c.to_digit(10)) {
                            if (1..=6).contains(&level) {
                                // Empty heading - just emit the heading token
                                if token_count >= limits.max_tokens {
                                    return Err(TokenizeError::InvalidStructure(format!(
                                        "Token count exceeds max_tokens ({}",
                                        limits.max_tokens
                                    )));
                                }
                                tokens.push(Token::Heading(level as u8));
                                token_count += 1;
                                pending_paragraph_break = true;
                            }
                        }
                    }
                    "img" => {
                        if let Some(src) = get_attribute(&e, &reader, "src") {
                            let alt = get_attribute(&e, &reader, "alt").unwrap_or_default();
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::Image { src, alt });
                            token_count += 1;
                        }
                        // No src → skip
                    }
                    _ => {
                        // Other empty elements are ignored
                    }
                }
            }
            Ok(Event::CData(e)) => {
                // CDATA content is treated as raw text
                if skip_depth == 0 {
                    let text = reader
                        .decoder()
                        .decode(&e)
                        .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;

                    let normalized =
                        normalize_whitespace_limited(text.as_ref(), limits.max_text_bytes);
                    if !normalized.is_empty() {
                        if let Some(level) = pending_heading_close.take() {
                            if token_count >= limits.max_tokens {
                                return Err(TokenizeError::InvalidStructure(format!(
                                    "Token count exceeds max_tokens ({}",
                                    limits.max_tokens
                                )));
                            }
                            tokens.push(Token::Heading(level));
                            token_count += 1;
                        }
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Text(normalized));
                        token_count += 1;
                    }
                }
            }
            Ok(Event::GeneralRef(e)) => {
                // Entity references: &amp; &lt; &gt; &quot; &apos; &#8220; etc.
                if skip_depth > 0 {
                    continue;
                }

                let entity_name = e
                    .decode()
                    .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;
                entity_buf.clear();
                entity_buf.push('&');
                entity_buf.push_str(entity_name.as_ref());
                entity_buf.push(';');
                let resolved = unescape(&entity_buf)
                    .map_err(|e| TokenizeError::ParseError(format!("Unescape error: {:?}", e)))?;
                let resolved_str = resolved.as_ref();

                if !resolved_str.is_empty() {
                    // Flush any pending heading close
                    if let Some(level) = pending_heading_close.take() {
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Heading(level));
                        token_count += 1;
                    }
                    // Append to the last Text token if possible, otherwise create new one
                    if let Some(Token::Text(ref mut last_text)) = tokens.last_mut() {
                        if last_text.len() + resolved_str.len() <= limits.max_text_bytes {
                            last_text.push_str(resolved_str);
                        }
                    } else {
                        if token_count >= limits.max_tokens {
                            return Err(TokenizeError::InvalidStructure(format!(
                                "Token count exceeds max_tokens ({}",
                                limits.max_tokens
                            )));
                        }
                        tokens.push(Token::Text(resolved.into_owned()));
                        token_count += 1;
                    }
                }
            }
            Ok(Event::Comment(_)) => {
                // Comments are ignored
            }
            Ok(Event::Decl(_)) => {
                // XML declaration is ignored
            }
            Ok(Event::PI(_)) => {
                // Processing instructions are ignored
            }
            Ok(Event::DocType(_)) => {
                // DOCTYPE is ignored
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(TokenizeError::ParseError(format!("XML error: {:?}", e)));
            }
        }
        buf.clear();
    }

    // Close any unclosed formatting tags
    while let Some(element) = element_stack.pop() {
        match element {
            ElementType::Emphasis => {
                if token_count >= limits.max_tokens {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Token count exceeds max_tokens ({}",
                        limits.max_tokens
                    )));
                }
                tokens.push(Token::Emphasis(false));
                token_count += 1;
            }
            ElementType::Strong => {
                if token_count >= limits.max_tokens {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Token count exceeds max_tokens ({}",
                        limits.max_tokens
                    )));
                }
                tokens.push(Token::Strong(false));
                token_count += 1;
            }
            ElementType::UnorderedList | ElementType::OrderedList => {
                if token_count >= limits.max_tokens {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Token count exceeds max_tokens ({}",
                        limits.max_tokens
                    )));
                }
                tokens.push(Token::ListEnd);
                token_count += 1;
            }
            ElementType::ListItem => {
                if token_count >= limits.max_tokens {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Token count exceeds max_tokens ({}",
                        limits.max_tokens
                    )));
                }
                tokens.push(Token::ListItemEnd);
                token_count += 1;
            }
            ElementType::Link => {
                if token_count >= limits.max_tokens {
                    return Err(TokenizeError::InvalidStructure(format!(
                        "Token count exceeds max_tokens ({}",
                        limits.max_tokens
                    )));
                }
                tokens.push(Token::LinkEnd);
                token_count += 1;
            }
            ElementType::Paragraph | ElementType::Heading(_) => {
                // These already handled via pending_paragraph_break
            }
            _ => {}
        }
    }

    // Flush any pending heading close
    if let Some(level) = pending_heading_close {
        if token_count >= limits.max_tokens {
            return Err(TokenizeError::InvalidStructure(format!(
                "Token count exceeds max_tokens ({}",
                limits.max_tokens
            )));
        }
        tokens.push(Token::Heading(level));
    }

    Ok(tokens)
}

/// Normalize whitespace with a byte limit.
fn normalize_whitespace_limited(text: &str, max_bytes: usize) -> String {
    let mut result = String::with_capacity(text.len().min(max_bytes));
    let mut prev_was_space = true; // Start true to trim leading whitespace

    for ch in text.chars() {
        if result.len() >= max_bytes {
            break;
        }
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

    // Trim trailing space if present
    if result.ends_with(' ') {
        result.pop();
    }

    result
}

/// Types of elements we track in the stack
#[derive(Clone, Debug, PartialEq)]
enum ElementType {
    Paragraph,
    Heading(u8),
    Emphasis,
    Strong,
    Span,
    UnorderedList,
    OrderedList,
    ListItem,
    Link,
    Generic,
}

/// Check if an element should be skipped entirely (with its children)
fn should_skip_element(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "head" | "nav" | "header" | "footer" | "aside" | "noscript"
    )
}

/// Normalize whitespace in text content
/// Collapses multiple spaces/newlines and trims ends
fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_was_space = true; // Start true to trim leading whitespace

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

    // Trim trailing space if present
    if result.ends_with(' ') {
        result.pop();
    }

    result
}

/// Extract a named attribute value from a start/empty element
fn get_attribute(e: &BytesStart, reader: &Reader<&[u8]>, name: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = reader.decoder().decode(attr.key.as_ref()).ok()?;
        if key.as_ref() == name {
            let value = reader.decoder().decode(&attr.value).ok()?;
            return Some(value.to_string());
        }
    }
    None
}

/// Decode element name from bytes
fn decode_name(name: &[u8], reader: &Reader<&[u8]>) -> Result<String, TokenizeError> {
    reader
        .decoder()
        .decode(name)
        .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))
        .map(|s| s.to_string())
}

/// Scratch buffer pool for tokenization to minimize allocations.
///
/// Pre-allocated buffers that can be reused across tokenization operations
/// to avoid repeated allocations in hot paths. This is critical for embedded
/// environments where allocation overhead must be minimized.
#[derive(Debug)]
pub struct TokenizeScratch {
    /// Buffer for XML parsing
    pub xml_buf: Vec<u8>,
    /// Buffer for text accumulation and normalization
    pub text_buf: String,
    /// Buffer for element tracking (private, accessed via methods)
    element_buf: Vec<ElementType>,
}

impl TokenizeScratch {
    /// Create scratch buffers with specified capacities.
    ///
    /// # Arguments
    /// * `xml_capacity` - Initial capacity for XML parsing buffer
    /// * `text_capacity` - Initial capacity for text accumulation buffer
    ///
    /// # Example
    /// ```
    /// use epub_stream::tokenizer::TokenizeScratch;
    ///
    /// let scratch = TokenizeScratch::new(4096, 8192);
    /// ```
    pub fn new(xml_capacity: usize, text_capacity: usize) -> Self {
        Self {
            xml_buf: Vec::with_capacity(xml_capacity),
            text_buf: String::with_capacity(text_capacity),
            element_buf: Vec::with_capacity(64),
        }
    }

    /// Create buffers suitable for embedded use (small, bounded).
    ///
    /// Uses conservative buffer sizes suitable for constrained environments:
    /// - XML buffer: 4KB
    /// - Text buffer: 8KB
    /// - Element stack: 64 elements
    pub fn embedded() -> Self {
        Self::new(4096, 8192)
    }

    /// Create buffers for desktop use (larger, more performant).
    ///
    /// Uses larger buffer sizes for better performance on desktop:
    /// - XML buffer: 32KB
    /// - Text buffer: 64KB
    /// - Element stack: 64 elements
    pub fn desktop() -> Self {
        Self::new(32768, 65536)
    }

    /// Clear all buffers without deallocating.
    ///
    /// This preserves the allocated capacity while resetting the length to zero,
    /// allowing the buffers to be reused for subsequent tokenization operations
    /// without requiring new allocations.
    pub fn clear(&mut self) {
        self.xml_buf.clear();
        self.text_buf.clear();
        self.element_buf.clear();
    }

    /// Ensure text buffer has at least the given capacity.
    ///
    /// # Arguments
    /// * `min_cap` - Minimum capacity required
    ///
    /// If the current capacity is less than `min_cap`, the buffer will be
    /// expanded to at least that capacity.
    pub fn ensure_text_capacity(&mut self, min_cap: usize) {
        if self.text_buf.capacity() < min_cap {
            self.text_buf.reserve(min_cap - self.text_buf.capacity());
        }
    }
}

/// Tokenize XHTML into a caller-provided Vec to avoid allocation.
///
/// This variant allows the caller to provide their own `Vec<Token>` buffer,
/// avoiding the need to allocate a new Vec for each tokenization call.
/// The existing contents of the Vec are cleared before tokenization begins.
///
/// # Allocation behavior
/// - **Zero token buffer allocations**: Reuses caller's Vec
/// - Internal buffers: Allocates temporarily during parsing (use `tokenize_html_with_scratch` to avoid)
/// - Caller buffer required: Yes (tokens_out)
/// - **Preferred for embedded**: Avoids Vec allocation
///
/// # Arguments
/// * `html` - The XHTML content to tokenize
/// * `tokens_out` - Output buffer for tokens (will be cleared first)
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(TokenizeError)` on parse failure
///
/// # Example
/// ```
/// use epub_stream::tokenizer::{tokenize_html_into, Token};
///
/// let html = "<p>Hello <em>world</em></p>";
/// let mut tokens: Vec<Token> = Vec::with_capacity(8);
/// tokenize_html_into(html, &mut tokens).unwrap();
/// ```
pub fn tokenize_html_into(html: &str, tokens_out: &mut Vec<Token>) -> Result<(), TokenizeError> {
    let mut scratch = TokenizeScratch::embedded();
    tokenize_html_with_scratch(html, tokens_out, &mut scratch)
}

/// Tokenize XHTML with caller-provided scratch buffers for minimal allocations.
///
/// This is the most memory-efficient tokenization API. It uses pre-allocated
/// scratch buffers to minimize allocations during parsing. All internal buffers
/// are provided by the caller and can be reused across multiple tokenization calls.
///
/// # Allocation behavior
/// - **Minimal allocations**: Reuses all scratch buffers
/// - Still allocates for Token::Text content (owned String required by Token type)
/// - Caller buffer required: Yes (tokens_out, scratch)
/// - **Preferred for embedded**: Minimal allocation path
///
/// # Arguments
/// * `html` - The XHTML content to tokenize
/// * `tokens_out` - Output buffer for tokens (will be cleared first)
/// * `scratch` - Scratch buffers for internal parsing state
///
/// # Returns
/// * `Ok(())` on success
/// * `Err(TokenizeError)` on parse failure
///
/// # Example
/// ```
/// use epub_stream::tokenizer::{tokenize_html_with_scratch, TokenizeScratch, Token};
///
/// let html = "<p>Hello <em>world</em></p>";
/// let mut tokens: Vec<Token> = Vec::with_capacity(8);
/// let mut scratch = TokenizeScratch::embedded();
/// tokenize_html_with_scratch(html, &mut tokens, &mut scratch).unwrap();
///
/// // Reuse scratch for subsequent calls
/// let html2 = "<p>Second paragraph</p>";
/// tokenize_html_with_scratch(html2, &mut tokens, &mut scratch).unwrap();
/// ```
pub fn tokenize_html_with_scratch(
    html: &str,
    tokens_out: &mut Vec<Token>,
    scratch: &mut TokenizeScratch,
) -> Result<(), TokenizeError> {
    tokens_out.clear();
    scratch.clear();

    let mut reader = Reader::from_str(html);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    // Track if we're inside a tag that should be skipped (script, style, head)
    let mut skip_depth: usize = 0;
    // Track if we need a paragraph break after current block element
    let mut pending_paragraph_break: bool = false;
    // Track if we need a heading close after text content
    let mut pending_heading_close: Option<u8> = None;
    let mut entity_buf = String::with_capacity(16);

    loop {
        match reader.read_event_into(&mut scratch.xml_buf) {
            Ok(Event::Start(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Check if we should skip this element and its children
                if should_skip_element(&name) {
                    skip_depth += 1;
                    continue;
                }

                // If skipping, don't process anything
                if skip_depth > 0 {
                    continue;
                }

                // Flush any pending paragraph break from previous block
                if pending_paragraph_break && !tokens_out.is_empty() {
                    tokens_out.push(Token::ParagraphBreak);
                    pending_paragraph_break = false;
                }

                // Flush any pending heading close
                if let Some(level) = pending_heading_close.take() {
                    tokens_out.push(Token::Heading(level));
                    pending_paragraph_break = true;
                }

                match &*name {
                    "p" | "div" => {
                        scratch.element_buf.push(ElementType::Paragraph);
                    }
                    "span" => {
                        scratch.element_buf.push(ElementType::Span);
                    }
                    h if h.starts_with('h') && h.len() == 2 => {
                        if let Some(level) = h.chars().nth(1).and_then(|c| c.to_digit(10)) {
                            if (1..=6).contains(&level) {
                                scratch.element_buf.push(ElementType::Heading(level as u8));
                                pending_heading_close = Some(level as u8);
                            }
                        }
                    }
                    "em" | "i" => {
                        scratch.element_buf.push(ElementType::Emphasis);
                        tokens_out.push(Token::Emphasis(true));
                    }
                    "strong" | "b" => {
                        scratch.element_buf.push(ElementType::Strong);
                        tokens_out.push(Token::Strong(true));
                    }
                    "ul" => {
                        scratch.element_buf.push(ElementType::UnorderedList);
                        tokens_out.push(Token::ListStart(false));
                    }
                    "ol" => {
                        scratch.element_buf.push(ElementType::OrderedList);
                        tokens_out.push(Token::ListStart(true));
                    }
                    "li" => {
                        scratch.element_buf.push(ElementType::ListItem);
                        tokens_out.push(Token::ListItemStart);
                    }
                    "a" => {
                        if let Some(href) = get_attribute(&e, &reader, "href") {
                            scratch.element_buf.push(ElementType::Link);
                            tokens_out.push(Token::LinkStart(href));
                        } else {
                            // No href — treat as generic container
                            scratch.element_buf.push(ElementType::Generic);
                        }
                    }
                    "img" => {
                        // <img> as a start tag (non-self-closing)
                        if let Some(src) = get_attribute(&e, &reader, "src") {
                            let alt = get_attribute(&e, &reader, "alt").unwrap_or_default();
                            tokens_out.push(Token::Image { src, alt });
                        }
                        scratch.element_buf.push(ElementType::Generic);
                    }
                    _ => {
                        // Unknown element, treat as generic container
                        scratch.element_buf.push(ElementType::Generic);
                    }
                }
            }
            Ok(Event::Text(e)) => {
                // Skip text if we're inside a script/style/head block
                if skip_depth > 0 {
                    continue;
                }

                let text = e
                    .decode()
                    .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;

                // Normalize whitespace: collapse multiple spaces/newlines
                let normalized = normalize_whitespace(text.as_ref());

                if !normalized.is_empty() {
                    // Flush any pending heading close
                    if let Some(level) = pending_heading_close.take() {
                        tokens_out.push(Token::Heading(level));
                    }
                    tokens_out.push(Token::Text(normalized));
                }
            }
            Ok(Event::End(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Check if we're ending a skip element
                if should_skip_element(&name) {
                    skip_depth = skip_depth.saturating_sub(1);
                    continue;
                }

                // If skipping, don't process end tags
                if skip_depth > 0 {
                    continue;
                }

                // Pop the element from stack and emit appropriate close token
                if let Some(element) = scratch.element_buf.pop() {
                    match element {
                        ElementType::Paragraph => {
                            pending_paragraph_break = true;
                        }
                        ElementType::Heading(_level) => {
                            // Heading already emitted on start, just mark for paragraph break
                            pending_paragraph_break = true;
                            // Clear any pending close since we already handled it
                            pending_heading_close = None;
                        }
                        ElementType::Emphasis => {
                            tokens_out.push(Token::Emphasis(false));
                        }
                        ElementType::Strong => {
                            tokens_out.push(Token::Strong(false));
                        }
                        ElementType::UnorderedList | ElementType::OrderedList => {
                            tokens_out.push(Token::ListEnd);
                        }
                        ElementType::ListItem => {
                            tokens_out.push(Token::ListItemEnd);
                        }
                        ElementType::Link => {
                            tokens_out.push(Token::LinkEnd);
                        }
                        ElementType::Span | ElementType::Generic => {
                            // No tokens needed for these
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let qname = e.name();
                let name = decode_name(qname.as_ref(), &reader)?;

                // Skip empty elements inside script/style blocks
                if skip_depth > 0 {
                    continue;
                }

                // Flush any pending paragraph break
                if pending_paragraph_break && !tokens_out.is_empty() {
                    tokens_out.push(Token::ParagraphBreak);
                    pending_paragraph_break = false;
                }

                // Flush any pending heading close
                if let Some(level) = pending_heading_close.take() {
                    tokens_out.push(Token::Heading(level));
                    pending_paragraph_break = true;
                }

                match &*name {
                    "br" => {
                        tokens_out.push(Token::LineBreak);
                    }
                    "p" | "div" => {
                        // Empty paragraph still creates a paragraph break
                        pending_paragraph_break = true;
                    }
                    h if h.starts_with('h') && h.len() == 2 => {
                        if let Some(level) = h.chars().nth(1).and_then(|c| c.to_digit(10)) {
                            if (1..=6).contains(&level) {
                                // Empty heading - just emit the heading token
                                tokens_out.push(Token::Heading(level as u8));
                                pending_paragraph_break = true;
                            }
                        }
                    }
                    "img" => {
                        if let Some(src) = get_attribute(&e, &reader, "src") {
                            let alt = get_attribute(&e, &reader, "alt").unwrap_or_default();
                            tokens_out.push(Token::Image { src, alt });
                        }
                        // No src → skip
                    }
                    _ => {
                        // Other empty elements are ignored
                    }
                }
            }
            Ok(Event::CData(e)) => {
                // CDATA content is treated as raw text
                if skip_depth == 0 {
                    let text = reader
                        .decoder()
                        .decode(&e)
                        .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;

                    let normalized = normalize_whitespace(text.as_ref());
                    if !normalized.is_empty() {
                        // Flush any pending heading close
                        if let Some(level) = pending_heading_close.take() {
                            tokens_out.push(Token::Heading(level));
                        }
                        tokens_out.push(Token::Text(normalized));
                    }
                }
            }
            Ok(Event::GeneralRef(e)) => {
                // Entity references: &amp; &lt; &gt; &quot; &apos; &#8220; etc.
                if skip_depth > 0 {
                    continue;
                }

                let entity_name = e
                    .decode()
                    .map_err(|e| TokenizeError::ParseError(format!("Decode error: {:?}", e)))?;
                entity_buf.clear();
                entity_buf.push('&');
                entity_buf.push_str(entity_name.as_ref());
                entity_buf.push(';');
                let resolved = unescape(&entity_buf)
                    .map_err(|e| TokenizeError::ParseError(format!("Unescape error: {:?}", e)))?;
                let resolved_str = resolved.as_ref();

                if !resolved_str.is_empty() {
                    // Flush any pending heading close
                    if let Some(level) = pending_heading_close.take() {
                        tokens_out.push(Token::Heading(level));
                    }
                    // Append to the last Text token if possible, otherwise create new one
                    if let Some(Token::Text(ref mut last_text)) = tokens_out.last_mut() {
                        last_text.push_str(resolved_str);
                    } else {
                        tokens_out.push(Token::Text(resolved.into_owned()));
                    }
                }
            }
            Ok(Event::Comment(_)) => {
                // Comments are ignored
            }
            Ok(Event::Decl(_)) => {
                // XML declaration is ignored
            }
            Ok(Event::PI(_)) => {
                // Processing instructions are ignored
            }
            Ok(Event::DocType(_)) => {
                // DOCTYPE is ignored
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(TokenizeError::ParseError(format!("XML error: {:?}", e)));
            }
        }
        scratch.xml_buf.clear();
    }

    // Flush any remaining pending paragraph break
    if pending_paragraph_break && !tokens_out.is_empty() {
        // Don't add trailing paragraph break
        // tokens_out.push(Token::ParagraphBreak);
    }

    // Close any unclosed formatting tags
    while let Some(element) = scratch.element_buf.pop() {
        match element {
            ElementType::Emphasis => {
                tokens_out.push(Token::Emphasis(false));
            }
            ElementType::Strong => {
                tokens_out.push(Token::Strong(false));
            }
            ElementType::UnorderedList | ElementType::OrderedList => {
                tokens_out.push(Token::ListEnd);
            }
            ElementType::ListItem => {
                tokens_out.push(Token::ListItemEnd);
            }
            ElementType::Link => {
                tokens_out.push(Token::LinkEnd);
            }
            ElementType::Paragraph | ElementType::Heading(_) => {
                // These already handled via pending_paragraph_break
            }
            _ => {}
        }
    }

    // Flush any pending heading close
    if let Some(level) = pending_heading_close {
        tokens_out.push(Token::Heading(level));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_tokenize_simple_paragraph() {
        let html = "<p>Hello world</p>";
        let tokens = tokenize_html(html).unwrap();
        // No trailing ParagraphBreak — only emitted between blocks
        assert_eq!(tokens, vec![Token::Text("Hello world".to_string())]);
    }

    #[test]
    fn test_tokenize_emphasis() {
        let html = "<p>This is <em>italic</em> and <strong>bold</strong> text.</p>";
        let tokens = tokenize_html(html).unwrap();
        // normalize_whitespace strips leading/trailing spaces from text nodes
        assert_eq!(
            tokens,
            vec![
                Token::Text("This is".to_string()),
                Token::Emphasis(true),
                Token::Text("italic".to_string()),
                Token::Emphasis(false),
                Token::Text("and".to_string()),
                Token::Strong(true),
                Token::Text("bold".to_string()),
                Token::Strong(false),
                Token::Text("text.".to_string()),
            ]
        );
    }

    #[test]
    fn test_tokenize_heading_and_paragraphs() {
        let html = "<h1>Chapter Title</h1><p>First paragraph.</p><p>Second paragraph.</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Heading(1),
                Token::Text("Chapter Title".to_string()),
                Token::ParagraphBreak,
                Token::Text("First paragraph.".to_string()),
                Token::ParagraphBreak,
                Token::Text("Second paragraph.".to_string()),
            ]
        );
    }

    #[test]
    fn test_tokenize_multiple_headings() {
        let html = "<h1>Title</h1><h2>Subtitle</h2><h3>Section</h3>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Heading(1),
                Token::Text("Title".to_string()),
                Token::ParagraphBreak,
                Token::Heading(2),
                Token::Text("Subtitle".to_string()),
                Token::ParagraphBreak,
                Token::Heading(3),
                Token::Text("Section".to_string()),
            ]
        );
    }

    #[test]
    fn test_tokenize_line_break() {
        // XHTML requires self-closing <br/>
        let html = "<p>Line one<br/>Line two</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Line one".to_string()),
                Token::LineBreak,
                Token::Text("Line two".to_string()),
            ]
        );
    }

    #[test]
    fn test_tokenize_nested_formatting() {
        let html = "<p>Text with <strong>bold and <em>italic nested</em></strong>.</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Text with".to_string()),
                Token::Strong(true),
                Token::Text("bold and".to_string()),
                Token::Emphasis(true),
                Token::Text("italic nested".to_string()),
                Token::Emphasis(false),
                Token::Strong(false),
                Token::Text(".".to_string()),
            ]
        );
    }

    #[test]
    fn test_strip_script_and_style() {
        let html = r#"<p>Visible text</p><script>alert("hidden");</script><style>.hidden{}</style><p>More visible</p>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Visible text".to_string()),
                Token::ParagraphBreak,
                Token::Text("More visible".to_string()),
            ]
        );
    }

    #[test]
    fn test_strip_head() {
        let html = "<head><title>Title</title></head><body><p>Content</p></body>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(tokens, vec![Token::Text("Content".to_string())]);
    }

    #[test]
    fn test_whitespace_normalization() {
        let html = "<p>  Multiple   spaces   and\n\nnewlines  </p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![Token::Text("Multiple spaces and newlines".to_string())]
        );
    }

    #[test]
    fn test_empty_paragraph() {
        let html = "<p></p>";
        let tokens = tokenize_html(html).unwrap();
        // Empty paragraph with nothing following produces no tokens
        assert_eq!(tokens, vec![]);
    }

    #[test]
    fn test_unclosed_tags_rejected() {
        // quick-xml is a strict XML parser; mismatched tags are errors
        let html = "<p>Text with <em>italic</p>";
        assert!(tokenize_html(html).is_err());
    }

    #[test]
    fn test_b_and_i_tags() {
        let html = "<p><b>bold</b> and <i>italic</i></p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Strong(true),
                Token::Text("bold".to_string()),
                Token::Strong(false),
                Token::Text("and".to_string()),
                Token::Emphasis(true),
                Token::Text("italic".to_string()),
                Token::Emphasis(false),
            ]
        );
    }

    #[test]
    fn test_div_handling() {
        let html = "<div>Block content</div><div>Another block</div>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Block content".to_string()),
                Token::ParagraphBreak,
                Token::Text("Another block".to_string()),
            ]
        );
    }

    #[test]
    fn test_span_handling() {
        let html = "<p>Text with <span>spanned</span> content</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Text with".to_string()),
                Token::Text("spanned".to_string()),
                Token::Text("content".to_string()),
            ]
        );
    }

    #[test]
    fn test_example_from_spec() {
        let html = r#"<p>This is <em>italic</em> and <strong>bold</strong> text.</p>
<h1>Chapter Title</h1>
<p>Another paragraph.</p>"#;

        let tokens = tokenize_html(html).unwrap();

        let expected = vec![
            Token::Text("This is".to_string()),
            Token::Emphasis(true),
            Token::Text("italic".to_string()),
            Token::Emphasis(false),
            Token::Text("and".to_string()),
            Token::Strong(true),
            Token::Text("bold".to_string()),
            Token::Strong(false),
            Token::Text("text.".to_string()),
            Token::ParagraphBreak,
            Token::Heading(1),
            Token::Text("Chapter Title".to_string()),
            Token::ParagraphBreak,
            Token::Text("Another paragraph.".to_string()),
        ];

        assert_eq!(tokens, expected);
    }

    #[test]
    fn test_all_heading_levels() {
        let html = "<h1>H1</h1><h2>H2</h2><h3>H3</h3><h4>H4</h4><h5>H5</h5><h6>H6</h6>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Heading(1),
                Token::Text("H1".to_string()),
                Token::ParagraphBreak,
                Token::Heading(2),
                Token::Text("H2".to_string()),
                Token::ParagraphBreak,
                Token::Heading(3),
                Token::Text("H3".to_string()),
                Token::ParagraphBreak,
                Token::Heading(4),
                Token::Text("H4".to_string()),
                Token::ParagraphBreak,
                Token::Heading(5),
                Token::Text("H5".to_string()),
                Token::ParagraphBreak,
                Token::Heading(6),
                Token::Text("H6".to_string()),
            ]
        );
    }

    // ---- List tests ----

    #[test]
    fn test_simple_unordered_list() {
        let html = "<ul><li>Item 1</li><li>Item 2</li></ul>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Text("Item 1".to_string()),
                Token::ListItemEnd,
                Token::ListItemStart,
                Token::Text("Item 2".to_string()),
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_simple_ordered_list() {
        let html = "<ol><li>First</li><li>Second</li></ol>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::ListStart(true),
                Token::ListItemStart,
                Token::Text("First".to_string()),
                Token::ListItemEnd,
                Token::ListItemStart,
                Token::Text("Second".to_string()),
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_nested_lists() {
        let html = "<ul><li>A<ul><li>B</li></ul></li></ul>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Text("A".to_string()),
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Text("B".to_string()),
                Token::ListItemEnd,
                Token::ListEnd,
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_list_with_formatted_text() {
        let html = "<ul><li><em>italic</em> item</li></ul>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Emphasis(true),
                Token::Text("italic".to_string()),
                Token::Emphasis(false),
                Token::Text("item".to_string()),
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_empty_list() {
        let html = "<ul></ul>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(tokens, vec![Token::ListStart(false), Token::ListEnd]);
    }

    // ---- Link tests ----

    #[test]
    fn test_link_with_href() {
        let html = r#"<a href="ch2.xhtml">Next Chapter</a>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::LinkStart("ch2.xhtml".to_string()),
                Token::Text("Next Chapter".to_string()),
                Token::LinkEnd,
            ]
        );
    }

    #[test]
    fn test_link_without_href() {
        let html = "<a>No link</a>";
        let tokens = tokenize_html(html).unwrap();

        // No href → treated as generic container, no LinkStart/LinkEnd
        assert_eq!(tokens, vec![Token::Text("No link".to_string())]);
    }

    #[test]
    fn test_link_with_formatted_text() {
        let html = r#"<a href="x.html"><em>italic link</em></a>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::LinkStart("x.html".to_string()),
                Token::Emphasis(true),
                Token::Text("italic link".to_string()),
                Token::Emphasis(false),
                Token::LinkEnd,
            ]
        );
    }

    // ---- Image tests ----

    #[test]
    fn test_image_self_closing() {
        let html = r#"<img src="cover.jpg" alt="Cover Image"/>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![Token::Image {
                src: "cover.jpg".to_string(),
                alt: "Cover Image".to_string(),
            }]
        );
    }

    #[test]
    fn test_image_without_alt() {
        let html = r#"<img src="photo.jpg"/>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![Token::Image {
                src: "photo.jpg".to_string(),
                alt: String::with_capacity(32),
            }]
        );
    }

    #[test]
    fn test_image_without_src() {
        let html = r#"<img alt="Missing"/>"#;
        let tokens = tokenize_html(html).unwrap();

        // No src → image is skipped
        assert_eq!(tokens, vec![]);
    }

    #[test]
    fn test_image_as_start_tag() {
        // Some XHTML may have <img></img> instead of self-closing
        let html = r#"<img src="pic.png" alt="Pic"></img>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![Token::Image {
                src: "pic.png".to_string(),
                alt: "Pic".to_string(),
            }]
        );
    }

    // ---- Mixed content tests ----

    #[test]
    fn test_mixed_content() {
        let html = r#"<p>See <a href="ch2.xhtml">chapter 2</a> for details.</p><ul><li>Item with <img src="icon.png" alt="icon"/></li></ul>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("See".to_string()),
                Token::LinkStart("ch2.xhtml".to_string()),
                Token::Text("chapter 2".to_string()),
                Token::LinkEnd,
                Token::Text("for details.".to_string()),
                Token::ParagraphBreak,
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Text("Item with".to_string()),
                Token::Image {
                    src: "icon.png".to_string(),
                    alt: "icon".to_string(),
                },
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    // ---- Edge case tests for existing features ----

    #[test]
    fn test_deeply_nested_formatting() {
        let html = "<em><strong><em>triple</em></strong></em>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Emphasis(true),
                Token::Strong(true),
                Token::Emphasis(true),
                Token::Text("triple".to_string()),
                Token::Emphasis(false),
                Token::Strong(false),
                Token::Emphasis(false),
            ]
        );
    }

    #[test]
    fn test_consecutive_headings_same_level() {
        let html = "<h2>First</h2><h2>Second</h2>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Heading(2),
                Token::Text("First".to_string()),
                Token::ParagraphBreak,
                Token::Heading(2),
                Token::Text("Second".to_string()),
            ]
        );
    }

    #[test]
    fn test_multiple_consecutive_line_breaks() {
        let html = "<p>A<br/><br/><br/>B</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("A".to_string()),
                Token::LineBreak,
                Token::LineBreak,
                Token::LineBreak,
                Token::Text("B".to_string()),
            ]
        );
    }

    #[test]
    fn test_cdata_sections() {
        let html = "<p><![CDATA[Some raw content]]></p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(tokens, vec![Token::Text("Some raw content".to_string())]);
    }

    #[test]
    fn test_whitespace_only_text_nodes() {
        // Whitespace between block elements should be normalized away
        let html = "<p>First</p>   \n   <p>Second</p>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("First".to_string()),
                Token::ParagraphBreak,
                Token::Text("Second".to_string()),
            ]
        );
    }

    #[test]
    fn test_very_long_text() {
        // Performance sanity check with long text
        let long_word = "word ".repeat(10_000);
        let html = format!("<p>{}</p>", long_word);
        let tokens = tokenize_html(&html).unwrap();

        assert_eq!(tokens.len(), 1);
        if let Token::Text(ref text) = tokens[0] {
            assert!(text.len() > 40_000);
        } else {
            panic!("Expected Token::Text");
        }
    }

    #[test]
    fn test_mixed_block_and_inline() {
        let html = "<div><p><em>text</em></p></div>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Emphasis(true),
                Token::Text("text".to_string()),
                Token::Emphasis(false),
            ]
        );
    }

    #[test]
    fn test_block_inside_inline_no_crash() {
        // Malformed HTML: block element inside inline — should not crash
        let html = "<em><p>text</p></em>";
        // We just verify it doesn't panic; token output may vary
        let result = tokenize_html(html);
        assert!(result.is_ok());
        let tokens = result.unwrap();
        // Should at least contain the text
        assert!(tokens
            .iter()
            .any(|t| matches!(t, Token::Text(s) if s == "text")));
    }

    #[test]
    fn test_link_in_paragraph() {
        let html = r#"<p>Click <a href="http://example.com">here</a> to continue.</p>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Click".to_string()),
                Token::LinkStart("http://example.com".to_string()),
                Token::Text("here".to_string()),
                Token::LinkEnd,
                Token::Text("to continue.".to_string()),
            ]
        );
    }

    #[test]
    fn test_image_in_paragraph() {
        let html = r#"<p>An image: <img src="fig1.png" alt="Figure 1"/></p>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("An image:".to_string()),
                Token::Image {
                    src: "fig1.png".to_string(),
                    alt: "Figure 1".to_string(),
                },
            ]
        );
    }

    #[test]
    fn test_list_after_paragraph() {
        let html = "<p>Intro:</p><ul><li>One</li><li>Two</li></ul>";
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::Text("Intro:".to_string()),
                Token::ParagraphBreak,
                Token::ListStart(false),
                Token::ListItemStart,
                Token::Text("One".to_string()),
                Token::ListItemEnd,
                Token::ListItemStart,
                Token::Text("Two".to_string()),
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_ordered_list_with_links() {
        let html = r#"<ol><li><a href="ch1.html">Chapter 1</a></li><li><a href="ch2.html">Chapter 2</a></li></ol>"#;
        let tokens = tokenize_html(html).unwrap();

        assert_eq!(
            tokens,
            vec![
                Token::ListStart(true),
                Token::ListItemStart,
                Token::LinkStart("ch1.html".to_string()),
                Token::Text("Chapter 1".to_string()),
                Token::LinkEnd,
                Token::ListItemEnd,
                Token::ListItemStart,
                Token::LinkStart("ch2.html".to_string()),
                Token::Text("Chapter 2".to_string()),
                Token::LinkEnd,
                Token::ListItemEnd,
                Token::ListEnd,
            ]
        );
    }

    #[test]
    fn test_tokenize_html_with_matches_tokenize_html() {
        let html = "<h1>T</h1><p>Hello <em>world</em><br/>line 2</p>";
        let baseline = tokenize_html(html).unwrap();
        let mut streamed = Vec::with_capacity(8);
        tokenize_html_with(html, |token| streamed.push(token)).unwrap();
        assert_eq!(baseline, streamed);
    }
}
