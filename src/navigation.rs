//! EPUB navigation parsing (TOC, page list, landmarks)
//!
//! Supports both EPUB 3.x XHTML navigation documents (`epub:type="toc"`)
//! and EPUB 2.0 NCX fallback (`toc.ncx`).
//!
//! # Usage
//!
//! ```rust,no_run
//! use epub_stream::navigation::{parse_nav_xhtml, parse_ncx, Navigation};
//!
//! # fn example() -> Result<(), epub_stream::error::EpubError> {
//! // EPUB 3.x: parse the XHTML nav document
//! let nav_xhtml_bytes = b"<html>...</html>";
//! let nav = parse_nav_xhtml(nav_xhtml_bytes)?;
//!
//! // EPUB 2.0 fallback: parse the NCX
//! let ncx_bytes = b"<ncx>...</ncx>";
//! let nav = parse_ncx(ncx_bytes)?;
//! # Ok(())
//! # }
//! ```

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::EpubError;

/// Limits for navigation parsing and structure growth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NavigationLimits {
    /// Maximum number of total navigation points across TOC/page-list/landmarks.
    pub max_points: usize,
    /// Maximum allowed nav tree depth.
    pub max_depth: usize,
    /// Maximum UTF-8 byte length for labels.
    pub max_label_bytes: usize,
    /// Maximum UTF-8 byte length for href values.
    pub max_href_bytes: usize,
}

impl Default for NavigationLimits {
    fn default() -> Self {
        Self {
            max_points: 4096,
            max_depth: 64,
            max_label_bytes: 4096,
            max_href_bytes: 4096,
        }
    }
}

impl NavigationLimits {
    /// Embedded-focused preset with smaller bounds.
    pub fn embedded() -> Self {
        Self {
            max_points: 1024,
            max_depth: 32,
            max_label_bytes: 1024,
            max_href_bytes: 2048,
        }
    }
}

/// A single navigation point (table of contents entry)
///
/// Navigation points can be nested to represent hierarchical structures
/// (e.g., chapters containing sections).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavPoint {
    /// Display label for this navigation point
    pub label: String,
    /// Content href (relative path, possibly with fragment)
    pub href: String,
    /// Child navigation points (for hierarchical TOC)
    pub children: Vec<NavPoint>,
}

/// Complete navigation structure for an EPUB
///
/// Contains table of contents, page list, and landmarks extracted
/// from either the EPUB 3.x nav document or EPUB 2.0 NCX.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Navigation {
    /// Table of contents entries
    pub toc: Vec<NavPoint>,
    /// Page list entries (mapping to page numbers)
    pub page_list: Vec<NavPoint>,
    /// Landmark entries (structural navigation: cover, toc, bodymatter, etc.)
    pub landmarks: Vec<NavPoint>,
}

impl Navigation {
    /// Create an empty navigation structure
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the navigation has any TOC entries
    pub fn has_toc(&self) -> bool {
        !self.toc.is_empty()
    }

    /// Check if the navigation has a page list
    pub fn has_page_list(&self) -> bool {
        !self.page_list.is_empty()
    }

    /// Check if the navigation has landmarks
    pub fn has_landmarks(&self) -> bool {
        !self.landmarks.is_empty()
    }

    /// Get total number of TOC entries (including nested)
    pub fn toc_count(&self) -> usize {
        count_nav_points(&self.toc)
    }

    /// Flatten the TOC into a linear list of (depth, NavPoint) pairs
    pub fn toc_flat(&self) -> Vec<(usize, &NavPoint)> {
        let mut result = Vec::with_capacity(8);
        flatten_nav_points(&self.toc, 0, &mut result);
        result
    }
}

/// Count all navigation points recursively
fn count_nav_points(points: &[NavPoint]) -> usize {
    points
        .iter()
        .map(|p| 1 + count_nav_points(&p.children))
        .sum()
}

/// Flatten navigation points into a list with depth info
fn flatten_nav_points<'a>(
    points: &'a [NavPoint],
    depth: usize,
    result: &mut Vec<(usize, &'a NavPoint)>,
) {
    for point in points {
        result.push((depth, point));
        flatten_nav_points(&point.children, depth + 1, result);
    }
}

/// Partial nav point being built during parsing
struct PartialNavPoint {
    href: Option<String>,
    label: Option<String>,
    children: Vec<NavPoint>,
}

impl PartialNavPoint {
    fn new() -> Self {
        Self {
            href: None,
            label: None,
            children: Vec::with_capacity(8),
        }
    }

    fn into_nav_point(self) -> Option<NavPoint> {
        match (self.href, self.label) {
            (Some(href), Some(label)) => Some(NavPoint {
                label,
                href,
                children: self.children,
            }),
            _ => None,
        }
    }
}

/// Parse an EPUB 3.x XHTML navigation document
///
/// Extracts TOC (`epub:type="toc"`), page list (`epub:type="page-list"`),
/// and landmarks (`epub:type="landmarks"`) from the nav XHTML.
///
/// The nav document uses nested `<ol>/<li>/<a>` structures within
/// `<nav>` elements identified by `epub:type` attributes.
pub fn parse_nav_xhtml(content: &[u8]) -> Result<Navigation, EpubError> {
    parse_nav_xhtml_with_limits(content, NavigationLimits::default())
}

/// Parse an EPUB 3.x XHTML navigation document with explicit limits.
pub fn parse_nav_xhtml_with_limits(
    content: &[u8],
    limits: NavigationLimits,
) -> Result<Navigation, EpubError> {
    let mut reader = quick_xml::reader::Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut nav = Navigation::new();
    let mut buf = alloc::vec::Vec::with_capacity(8);

    // State: which nav section we're inside (None = outside any nav)
    let mut current_nav_type: Option<NavType> = None;
    // Stack of list items being built (one per <li> nesting level)
    let mut item_stack: Vec<PartialNavPoint> = Vec::with_capacity(8);
    // Completed top-level results for the current nav section
    let mut results: Vec<NavPoint> = Vec::with_capacity(8);
    // Whether we're inside an <a> tag (collecting label text)
    let mut in_anchor = false;
    let mut point_count = 0usize;

    use quick_xml::events::Event;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"nav" => {
                    for attr in e.attributes().flatten() {
                        let key = attr.key.as_ref();
                        if key == b"epub:type" || key.ends_with(b":type") {
                            let value = reader
                                .decoder()
                                .decode(attr.value.as_ref())
                                .unwrap_or_default();
                            current_nav_type = NavType::from_str(value.as_ref());
                            results.clear();
                        }
                    }
                }
                b"li" if current_nav_type.is_some() => {
                    if item_stack.len() >= limits.max_depth {
                        return Err(EpubError::Navigation(alloc::format!(
                            "Navigation depth exceeds max_depth ({} > {})",
                            item_stack.len() + 1,
                            limits.max_depth
                        )));
                    }
                    item_stack.push(PartialNavPoint::new());
                }
                b"a" if current_nav_type.is_some() => {
                    in_anchor = true;
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" {
                            let href = reader
                                .decoder()
                                .decode(attr.value.as_ref())
                                .unwrap_or_default();
                            if href.len() > limits.max_href_bytes {
                                return Err(EpubError::Navigation(alloc::format!(
                                    "Navigation href exceeds max_href_bytes ({} > {})",
                                    href.len(),
                                    limits.max_href_bytes
                                )));
                            }
                            if let Some(item) = item_stack.last_mut() {
                                item.href = Some(href.into_owned());
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_anchor && current_nav_type.is_some() {
                    let text = reader.decoder().decode(&e).unwrap_or_default();
                    if let Some(item) = item_stack.last_mut() {
                        match &mut item.label {
                            Some(existing) => {
                                // Add space separator when concatenating text segments
                                // from formatted anchors (e.g. "Part <em>One</em>")
                                if !existing.is_empty()
                                    && !existing.ends_with(' ')
                                    && !text.starts_with(' ')
                                {
                                    existing.push(' ');
                                }
                                existing.push_str(text.as_ref());
                                if existing.len() > limits.max_label_bytes {
                                    return Err(EpubError::Navigation(alloc::format!(
                                        "Navigation label exceeds max_label_bytes ({} > {})",
                                        existing.len(),
                                        limits.max_label_bytes
                                    )));
                                }
                            }
                            None => {
                                if text.len() > limits.max_label_bytes {
                                    return Err(EpubError::Navigation(alloc::format!(
                                        "Navigation label exceeds max_label_bytes ({} > {})",
                                        text.len(),
                                        limits.max_label_bytes
                                    )));
                                }
                                item.label = Some(text.into_owned());
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                match e.name().as_ref() {
                    b"a" => {
                        in_anchor = false;
                    }
                    b"li" if current_nav_type.is_some() => {
                        // Pop the current item and finalize it
                        if let Some(partial) = item_stack.pop() {
                            if let Some(point) = partial.into_nav_point() {
                                point_count += 1;
                                if point_count > limits.max_points {
                                    return Err(EpubError::Navigation(alloc::format!(
                                        "Navigation points exceed max_points ({} > {})",
                                        point_count,
                                        limits.max_points
                                    )));
                                }
                                if let Some(parent) = item_stack.last_mut() {
                                    // Nested: add as child of parent item
                                    parent.children.push(point);
                                } else {
                                    // Top-level: add to results
                                    results.push(point);
                                }
                            }
                        }
                    }
                    b"nav" if current_nav_type.is_some() => {
                        // Assign collected results to the appropriate nav section
                        let completed = core::mem::take(&mut results);
                        match current_nav_type.take() {
                            Some(NavType::Toc) => nav.toc = completed,
                            Some(NavType::PageList) => nav.page_list = completed,
                            Some(NavType::Landmarks) => nav.landmarks = completed,
                            None => {
                                return Err(EpubError::Navigation(
                                    "Nav section ended without a section type".into(),
                                ))
                            }
                        }
                        item_stack.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                // Handle self-closing <a href="..."/> (rare but valid)
                if e.name().as_ref() == b"a" && current_nav_type.is_some() {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" {
                            let href = reader
                                .decoder()
                                .decode(attr.value.as_ref())
                                .unwrap_or_default();
                            if href.len() > limits.max_href_bytes {
                                return Err(EpubError::Navigation(alloc::format!(
                                    "Navigation href exceeds max_href_bytes ({} > {})",
                                    href.len(),
                                    limits.max_href_bytes
                                )));
                            }
                            if let Some(item) = item_stack.last_mut() {
                                item.href = Some(href.into_owned());
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(EpubError::Navigation(alloc::format!(
                    "Nav XML parse error: {:?}",
                    e
                )))
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(nav)
}

/// Parse an EPUB 2.0 NCX navigation document
///
/// Extracts the navigation map (`<navMap>`) and optional page list
/// (`<pageList>`) from the NCX XML.
pub fn parse_ncx(content: &[u8]) -> Result<Navigation, EpubError> {
    parse_ncx_with_limits(content, NavigationLimits::default())
}

/// Parse an EPUB 2.0 NCX navigation document with explicit limits.
pub fn parse_ncx_with_limits(
    content: &[u8],
    limits: NavigationLimits,
) -> Result<Navigation, EpubError> {
    let mut reader = quick_xml::reader::Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut nav = Navigation::new();
    let mut buf = alloc::vec::Vec::with_capacity(8);

    // State tracking
    let mut in_nav_map = false;
    let mut in_page_list = false;
    let mut nav_point_stack: Vec<NavPoint> = Vec::with_capacity(8);
    let mut current_label: Option<String> = None;
    let mut current_src: Option<String> = None;
    let mut in_text = false;
    let mut in_page_target = false;
    let mut point_count = 0usize;

    use quick_xml::events::Event;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"navMap" => {
                    in_nav_map = true;
                }
                b"pageList" => {
                    in_page_list = true;
                }
                b"navPoint" if in_nav_map => {
                    if nav_point_stack.len() >= limits.max_depth {
                        return Err(EpubError::Navigation(alloc::format!(
                            "Navigation depth exceeds max_depth ({} > {})",
                            nav_point_stack.len() + 1,
                            limits.max_depth
                        )));
                    }
                    nav_point_stack.push(NavPoint {
                        label: String::with_capacity(32),
                        href: String::with_capacity(32),
                        children: Vec::with_capacity(8),
                    });
                }
                b"pageTarget" if in_page_list => {
                    in_page_target = true;
                    current_label = None;
                    current_src = None;
                }
                b"text" => {
                    in_text = true;
                }
                b"content" => {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"src" {
                            let src = reader
                                .decoder()
                                .decode(attr.value.as_ref())
                                .unwrap_or_default();
                            if src.len() > limits.max_href_bytes {
                                return Err(EpubError::Navigation(alloc::format!(
                                    "Navigation href exceeds max_href_bytes ({} > {})",
                                    src.len(),
                                    limits.max_href_bytes
                                )));
                            }
                            let src = src.into_owned();
                            if in_page_target {
                                current_src = Some(src);
                            } else if let Some(point) = nav_point_stack.last_mut() {
                                point.href = src;
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Text(e)) => {
                if in_text {
                    let text = reader.decoder().decode(&e).unwrap_or_default();
                    if in_page_target {
                        match &mut current_label {
                            Some(existing) => {
                                existing.push_str(text.as_ref());
                                if existing.len() > limits.max_label_bytes {
                                    return Err(EpubError::Navigation(alloc::format!(
                                        "Navigation label exceeds max_label_bytes ({} > {})",
                                        existing.len(),
                                        limits.max_label_bytes
                                    )));
                                }
                            }
                            None => {
                                if text.len() > limits.max_label_bytes {
                                    return Err(EpubError::Navigation(alloc::format!(
                                        "Navigation label exceeds max_label_bytes ({} > {})",
                                        text.len(),
                                        limits.max_label_bytes
                                    )));
                                }
                                current_label = Some(text.into_owned());
                            }
                        }
                    } else if let Some(point) = nav_point_stack.last_mut() {
                        if point.label.is_empty() {
                            if text.len() > limits.max_label_bytes {
                                return Err(EpubError::Navigation(alloc::format!(
                                    "Navigation label exceeds max_label_bytes ({} > {})",
                                    text.len(),
                                    limits.max_label_bytes
                                )));
                            }
                            point.label = text.into_owned();
                        } else {
                            point.label.push_str(text.as_ref());
                            if point.label.len() > limits.max_label_bytes {
                                return Err(EpubError::Navigation(alloc::format!(
                                    "Navigation label exceeds max_label_bytes ({} > {})",
                                    point.label.len(),
                                    limits.max_label_bytes
                                )));
                            }
                        }
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"text" => {
                    in_text = false;
                }
                b"navPoint" => {
                    if let Some(completed) = nav_point_stack.pop() {
                        point_count += 1;
                        if point_count > limits.max_points {
                            return Err(EpubError::Navigation(alloc::format!(
                                "Navigation points exceed max_points ({} > {})",
                                point_count,
                                limits.max_points
                            )));
                        }
                        if let Some(parent) = nav_point_stack.last_mut() {
                            parent.children.push(completed);
                        } else {
                            nav.toc.push(completed);
                        }
                    }
                }
                b"pageTarget" => {
                    if let (Some(label), Some(src)) = (current_label.take(), current_src.take()) {
                        point_count += 1;
                        if point_count > limits.max_points {
                            return Err(EpubError::Navigation(alloc::format!(
                                "Navigation points exceed max_points ({} > {})",
                                point_count,
                                limits.max_points
                            )));
                        }
                        nav.page_list.push(NavPoint {
                            label,
                            href: src,
                            children: Vec::with_capacity(8),
                        });
                    }
                    in_page_target = false;
                }
                b"navMap" => {
                    in_nav_map = false;
                }
                b"pageList" => {
                    in_page_list = false;
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(EpubError::Navigation(alloc::format!(
                    "NCX parse error: {:?}",
                    e
                )))
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(nav)
}

/// Internal enum for tracking which nav section we're in
#[derive(Clone, Debug, PartialEq)]
enum NavType {
    Toc,
    PageList,
    Landmarks,
}

impl NavType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "toc" => Some(NavType::Toc),
            "page-list" => Some(NavType::PageList),
            "landmarks" => Some(NavType::Landmarks),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- NavPoint / Navigation struct tests ---

    #[test]
    fn test_navigation_default() {
        let nav = Navigation::new();
        assert!(!nav.has_toc());
        assert!(!nav.has_page_list());
        assert!(!nav.has_landmarks());
        assert_eq!(nav.toc_count(), 0);
    }

    #[test]
    fn test_navigation_toc_count() {
        let nav = Navigation {
            toc: vec![
                NavPoint {
                    label: "Ch 1".into(),
                    href: "ch1.xhtml".into(),
                    children: vec![NavPoint {
                        label: "Sec 1.1".into(),
                        href: "ch1.xhtml#s1".into(),
                        children: vec![],
                    }],
                },
                NavPoint {
                    label: "Ch 2".into(),
                    href: "ch2.xhtml".into(),
                    children: vec![],
                },
            ],
            ..Default::default()
        };
        assert!(nav.has_toc());
        assert_eq!(nav.toc_count(), 3); // Ch1 + Sec1.1 + Ch2
    }

    #[test]
    fn test_toc_flat() {
        let nav = Navigation {
            toc: vec![NavPoint {
                label: "Ch 1".into(),
                href: "ch1.xhtml".into(),
                children: vec![NavPoint {
                    label: "Sec 1.1".into(),
                    href: "ch1.xhtml#s1".into(),
                    children: vec![],
                }],
            }],
            ..Default::default()
        };
        let flat = nav.toc_flat();
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].0, 0); // depth 0
        assert_eq!(flat[0].1.label, "Ch 1");
        assert_eq!(flat[1].0, 1); // depth 1
        assert_eq!(flat[1].1.label, "Sec 1.1");
    }

    // -- XHTML nav parsing tests ---

    #[test]
    fn test_parse_nav_xhtml_basic_toc() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="chapter1.xhtml">Chapter 1</a></li>
    <li><a href="chapter2.xhtml">Chapter 2</a></li>
    <li><a href="chapter3.xhtml">Chapter 3</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert_eq!(nav.toc.len(), 3);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[0].href, "chapter1.xhtml");
        assert_eq!(nav.toc[2].label, "Chapter 3");
    }

    #[test]
    fn test_parse_nav_xhtml_nested_toc() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml">Chapter 1</a>
      <ol>
        <li><a href="ch1.xhtml#s1">Section 1.1</a></li>
        <li><a href="ch1.xhtml#s2">Section 1.2</a></li>
      </ol>
    </li>
    <li><a href="ch2.xhtml">Chapter 2</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert_eq!(nav.toc.len(), 2);
        assert_eq!(nav.toc[0].children.len(), 2);
        assert_eq!(nav.toc[0].children[0].label, "Section 1.1");
        assert_eq!(nav.toc[0].children[1].href, "ch1.xhtml#s2");
    }

    #[test]
    fn test_parse_nav_xhtml_page_list() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
</nav>
<nav epub:type="page-list">
  <ol>
    <li><a href="ch1.xhtml#p1">1</a></li>
    <li><a href="ch1.xhtml#p2">2</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert!(nav.has_toc());
        assert!(nav.has_page_list());
        assert_eq!(nav.page_list.len(), 2);
        assert_eq!(nav.page_list[0].label, "1");
    }

    #[test]
    fn test_parse_nav_xhtml_landmarks() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="landmarks">
  <ol>
    <li><a href="cover.xhtml">Cover</a></li>
    <li><a href="toc.xhtml">Table of Contents</a></li>
    <li><a href="ch1.xhtml">Begin Reading</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert!(nav.has_landmarks());
        assert_eq!(nav.landmarks.len(), 3);
        assert_eq!(nav.landmarks[0].label, "Cover");
    }

    #[test]
    fn test_parse_nav_xhtml_empty() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<body></body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert!(!nav.has_toc());
        assert!(!nav.has_page_list());
        assert!(!nav.has_landmarks());
    }

    // -- NCX parsing tests ---

    #[test]
    fn test_parse_ncx_basic() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="ch1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="chapter1.xhtml"/>
    </navPoint>
    <navPoint id="ch2" playOrder="2">
      <navLabel><text>Chapter 2</text></navLabel>
      <content src="chapter2.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

        let nav = parse_ncx(ncx).unwrap();
        assert_eq!(nav.toc.len(), 2);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[0].href, "chapter1.xhtml");
        assert_eq!(nav.toc[1].label, "Chapter 2");
    }

    #[test]
    fn test_parse_ncx_nested() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="ch1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="ch1.xhtml"/>
      <navPoint id="s1" playOrder="2">
        <navLabel><text>Section 1.1</text></navLabel>
        <content src="ch1.xhtml#s1"/>
      </navPoint>
    </navPoint>
  </navMap>
</ncx>"#;

        let nav = parse_ncx(ncx).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].children.len(), 1);
        assert_eq!(nav.toc[0].children[0].label, "Section 1.1");
    }

    #[test]
    fn test_parse_ncx_with_page_list() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="ch1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="ch1.xhtml"/>
    </navPoint>
  </navMap>
  <pageList>
    <pageTarget id="p1" type="normal" value="1">
      <navLabel><text>1</text></navLabel>
      <content src="ch1.xhtml#page1"/>
    </pageTarget>
    <pageTarget id="p2" type="normal" value="2">
      <navLabel><text>2</text></navLabel>
      <content src="ch1.xhtml#page2"/>
    </pageTarget>
  </pageList>
</ncx>"#;

        let nav = parse_ncx(ncx).unwrap();
        assert!(nav.has_toc());
        assert!(nav.has_page_list());
        assert_eq!(nav.page_list.len(), 2);
        assert_eq!(nav.page_list[0].label, "1");
        assert_eq!(nav.page_list[0].href, "ch1.xhtml#page1");
    }

    #[test]
    fn test_parse_ncx_empty() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap/>
</ncx>"#;

        let nav = parse_ncx(ncx).unwrap();
        assert!(!nav.has_toc());
    }

    // -- Additional edge case tests ---

    #[test]
    fn test_parse_nav_xhtml_all_three_sections() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml">Chapter 1</a></li>
    <li><a href="ch2.xhtml">Chapter 2</a></li>
  </ol>
</nav>
<nav epub:type="page-list">
  <ol>
    <li><a href="ch1.xhtml#p1">1</a></li>
    <li><a href="ch1.xhtml#p2">2</a></li>
    <li><a href="ch2.xhtml#p3">3</a></li>
  </ol>
</nav>
<nav epub:type="landmarks">
  <ol>
    <li><a href="cover.xhtml">Cover</a></li>
    <li><a href="toc.xhtml">Table of Contents</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert!(nav.has_toc());
        assert!(nav.has_page_list());
        assert!(nav.has_landmarks());
        assert_eq!(nav.toc.len(), 2);
        assert_eq!(nav.page_list.len(), 3);
        assert_eq!(nav.landmarks.len(), 2);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.page_list[2].label, "3");
        assert_eq!(nav.landmarks[1].label, "Table of Contents");
    }

    #[test]
    fn test_parse_nav_xhtml_deeply_nested_toc() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml">Chapter 1</a>
      <ol>
        <li><a href="ch1.xhtml#s1">Section 1.1</a>
          <ol>
            <li><a href="ch1.xhtml#s1a">Subsection 1.1.1</a>
              <ol>
                <li><a href="ch1.xhtml#s1a1">Sub-subsection 1.1.1.1</a></li>
              </ol>
            </li>
            <li><a href="ch1.xhtml#s1b">Subsection 1.1.2</a></li>
          </ol>
        </li>
        <li><a href="ch1.xhtml#s2">Section 1.2</a></li>
      </ol>
    </li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[0].children.len(), 2);
        assert_eq!(nav.toc[0].children[0].label, "Section 1.1");
        assert_eq!(nav.toc[0].children[0].children.len(), 2);
        assert_eq!(nav.toc[0].children[0].children[0].label, "Subsection 1.1.1");
        assert_eq!(nav.toc[0].children[0].children[0].children.len(), 1);
        assert_eq!(
            nav.toc[0].children[0].children[0].children[0].label,
            "Sub-subsection 1.1.1.1"
        );
        assert_eq!(nav.toc[0].children[0].children[1].label, "Subsection 1.1.2");
        assert_eq!(nav.toc[0].children[1].label, "Section 1.2");

        // toc_count should count all 6 entries
        assert_eq!(nav.toc_count(), 6);

        // toc_flat should have correct depths
        let flat = nav.toc_flat();
        assert_eq!(flat.len(), 6);
        assert_eq!(flat[0], (0, &nav.toc[0])); // Chapter 1, depth 0
        assert_eq!(flat[1].0, 1); // Section 1.1, depth 1
        assert_eq!(flat[2].0, 2); // Subsection 1.1.1, depth 2
        assert_eq!(flat[3].0, 3); // Sub-subsection 1.1.1.1, depth 3
        assert_eq!(flat[4].0, 2); // Subsection 1.1.2, depth 2
        assert_eq!(flat[5].0, 1); // Section 1.2, depth 1
    }

    #[test]
    fn test_parse_nav_xhtml_empty_label() {
        // An <li> with <a> but no text content — should be skipped
        // because into_nav_point requires both href and label
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml"></a></li>
    <li><a href="ch2.xhtml">Chapter 2</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        // The first item has no text label, so it should be skipped
        // (PartialNavPoint has href but no label → into_nav_point returns None)
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].label, "Chapter 2");
    }

    #[test]
    fn test_parse_nav_xhtml_fragment_only_href() {
        let nav_xhtml = br##"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="#section1">Section 1</a></li>
    <li><a href="#section2">Section 2</a></li>
    <li><a href="ch2.xhtml#intro">Chapter 2 Intro</a></li>
  </ol>
</nav>
</body>
</html>"##;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        assert_eq!(nav.toc.len(), 3);
        assert_eq!(nav.toc[0].href, "#section1");
        assert_eq!(nav.toc[1].href, "#section2");
        assert_eq!(nav.toc[2].href, "ch2.xhtml#intro");
    }

    #[test]
    fn test_parse_ncx_deeply_nested() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="ch1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="ch1.xhtml"/>
      <navPoint id="s1">
        <navLabel><text>Section 1.1</text></navLabel>
        <content src="ch1.xhtml#s1"/>
        <navPoint id="ss1">
          <navLabel><text>Subsection 1.1.1</text></navLabel>
          <content src="ch1.xhtml#ss1"/>
          <navPoint id="sss1">
            <navLabel><text>Sub-subsection 1.1.1.1</text></navLabel>
            <content src="ch1.xhtml#sss1"/>
          </navPoint>
        </navPoint>
      </navPoint>
      <navPoint id="s2">
        <navLabel><text>Section 1.2</text></navLabel>
        <content src="ch1.xhtml#s2"/>
      </navPoint>
    </navPoint>
  </navMap>
</ncx>"#;

        let nav = parse_ncx(ncx).unwrap();
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[0].children.len(), 2);
        assert_eq!(nav.toc[0].children[0].label, "Section 1.1");
        assert_eq!(nav.toc[0].children[0].children.len(), 1);
        assert_eq!(nav.toc[0].children[0].children[0].label, "Subsection 1.1.1");
        assert_eq!(nav.toc[0].children[0].children[0].children.len(), 1);
        assert_eq!(
            nav.toc[0].children[0].children[0].children[0].label,
            "Sub-subsection 1.1.1.1"
        );
        assert_eq!(nav.toc[0].children[1].label, "Section 1.2");
        assert_eq!(nav.toc_count(), 5);
    }

    #[test]
    fn test_parse_nav_xhtml_large_toc() {
        // Build a nav document with 25 entries to check for off-by-one errors
        let mut items = alloc::string::String::with_capacity(32);
        for i in 1..=25 {
            items.push_str(&alloc::format!(
                "    <li><a href=\"ch{}.xhtml\">Chapter {}</a></li>\n",
                i,
                i
            ));
        }
        let nav_xhtml = alloc::format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
{}  </ol>
</nav>
</body>
</html>"#,
            items
        );

        let nav = parse_nav_xhtml(nav_xhtml.as_bytes()).unwrap();
        assert_eq!(nav.toc.len(), 25);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[0].href, "ch1.xhtml");
        assert_eq!(nav.toc[24].label, "Chapter 25");
        assert_eq!(nav.toc[24].href, "ch25.xhtml");
        // Verify no off-by-one: check middle entry
        assert_eq!(nav.toc[12].label, "Chapter 13");
        assert_eq!(nav.toc[12].href, "ch13.xhtml");
    }

    #[test]
    fn test_parse_nav_xhtml_duplicate_nav_type_overwrites() {
        // Two nav elements with type="toc" — second should overwrite first
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="old1.xhtml">Old Chapter 1</a></li>
    <li><a href="old2.xhtml">Old Chapter 2</a></li>
  </ol>
</nav>
<nav epub:type="toc">
  <ol>
    <li><a href="new1.xhtml">New Chapter 1</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        // Second nav with same type should overwrite the first
        assert_eq!(nav.toc.len(), 1);
        assert_eq!(nav.toc[0].label, "New Chapter 1");
        assert_eq!(nav.toc[0].href, "new1.xhtml");
    }

    #[test]
    fn test_parse_nav_xhtml_extra_html_elements_wrapping_anchor() {
        // Spans and divs wrapping anchor text — only text inside <a> is captured
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml"><span>Chapter 1</span></a></li>
    <li><a href="ch2.xhtml">Chapter 2</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let nav = parse_nav_xhtml(nav_xhtml).unwrap();
        // The parser captures text inside <a> — text inside nested <span> within <a>
        // is still a Text event while in_anchor=true, so it should be captured
        assert_eq!(nav.toc.len(), 2);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[1].label, "Chapter 2");
    }

    #[test]
    fn test_parse_ncx_large_toc() {
        // Build an NCX with 20+ entries
        let mut nav_points = alloc::string::String::with_capacity(32);
        for i in 1..=22 {
            nav_points.push_str(&alloc::format!(
                r#"    <navPoint id="ch{}" playOrder="{}">
      <navLabel><text>Chapter {}</text></navLabel>
      <content src="ch{}.xhtml"/>
    </navPoint>
"#,
                i,
                i,
                i,
                i
            ));
        }
        let ncx = alloc::format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
{}  </navMap>
</ncx>"#,
            nav_points
        );

        let nav = parse_ncx(ncx.as_bytes()).unwrap();
        assert_eq!(nav.toc.len(), 22);
        assert_eq!(nav.toc[0].label, "Chapter 1");
        assert_eq!(nav.toc[21].label, "Chapter 22");
        assert_eq!(nav.toc[10].label, "Chapter 11");
    }

    #[test]
    fn test_toc_flat_empty() {
        let nav = Navigation::new();
        let flat = nav.toc_flat();
        assert!(flat.is_empty());
    }

    #[test]
    fn test_toc_count_deeply_nested() {
        let nav = Navigation {
            toc: vec![NavPoint {
                label: "Root".into(),
                href: "root.xhtml".into(),
                children: vec![
                    NavPoint {
                        label: "A".into(),
                        href: "a.xhtml".into(),
                        children: vec![NavPoint {
                            label: "A1".into(),
                            href: "a1.xhtml".into(),
                            children: vec![],
                        }],
                    },
                    NavPoint {
                        label: "B".into(),
                        href: "b.xhtml".into(),
                        children: vec![],
                    },
                ],
            }],
            ..Default::default()
        };
        // Root + A + A1 + B = 4
        assert_eq!(nav.toc_count(), 4);
    }

    #[test]
    fn test_navigation_has_page_list_and_landmarks() {
        let nav = Navigation {
            toc: vec![],
            page_list: vec![NavPoint {
                label: "1".into(),
                href: "p1.xhtml".into(),
                children: vec![],
            }],
            landmarks: vec![NavPoint {
                label: "Cover".into(),
                href: "cover.xhtml".into(),
                children: vec![],
            }],
        };
        assert!(!nav.has_toc());
        assert!(nav.has_page_list());
        assert!(nav.has_landmarks());
    }

    #[test]
    fn test_parse_nav_xhtml_respects_max_points_limit() {
        let nav_xhtml = br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body>
<nav epub:type="toc">
  <ol>
    <li><a href="ch1.xhtml">Chapter 1</a></li>
    <li><a href="ch2.xhtml">Chapter 2</a></li>
  </ol>
</nav>
</body>
</html>"#;

        let err = parse_nav_xhtml_with_limits(
            nav_xhtml,
            NavigationLimits {
                max_points: 1,
                ..NavigationLimits::default()
            },
        )
        .expect_err("navigation should fail when max_points is exceeded");
        match err {
            EpubError::Navigation(msg) => assert!(msg.contains("max_points")),
            other => panic!("expected navigation error, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_ncx_respects_depth_limit() {
        let ncx = br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
  <navMap>
    <navPoint id="n1" playOrder="1">
      <navLabel><text>Root</text></navLabel>
      <content src="root.xhtml"/>
      <navPoint id="n2" playOrder="2">
        <navLabel><text>Child</text></navLabel>
        <content src="child.xhtml"/>
      </navPoint>
    </navPoint>
  </navMap>
</ncx>"#;

        let err = parse_ncx_with_limits(
            ncx,
            NavigationLimits {
                max_depth: 1,
                ..NavigationLimits::default()
            },
        )
        .expect_err("navigation should fail when max_depth is exceeded");
        match err {
            EpubError::Navigation(msg) => assert!(msg.contains("max_depth")),
            other => panic!("expected navigation error, got {:?}", other),
        }
    }
}
