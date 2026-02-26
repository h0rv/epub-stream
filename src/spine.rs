//! EPUB spine parser and chapter navigation
//!
//! The spine defines the reading order of chapters. This module parses
//! the spine from content.opf and provides navigation utilities.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::EpubError;
use crate::metadata::MetadataLimits;

/// A single item in the EPUB spine (chapter reference)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpineItem {
    /// Manifest item this spine entry references
    pub idref: String,
    /// Optional spine-level ID
    pub id: Option<String>,
    /// Whether this item is part of the linear reading order
    pub linear: bool,
    /// Optional properties (e.g. "rendition:layout-pre-paginated")
    pub properties: Option<String>,
}

/// Spine represents the reading order of an EPUB
///
/// Tracks the ordered list of chapter IDs and provides navigation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Spine {
    /// Ordered spine entries
    items: Vec<SpineItem>,
    /// Index of the current chapter
    current: usize,
    /// Optional TOC item id (EPUB 2.0 NCX reference)
    toc_id: Option<String>,
}

impl Spine {
    /// Create a new empty spine
    pub fn new() -> Self {
        Self::default()
    }

    /// Create spine from a list of chapter IDs
    pub fn from_idrefs(idrefs: Vec<String>) -> Self {
        let items = idrefs
            .into_iter()
            .map(|idref| SpineItem {
                idref,
                id: None,
                linear: true,
                properties: None,
            })
            .collect();

        Self {
            items,
            current: 0,
            toc_id: None,
        }
    }

    /// Get a reference to the ordered spine entries
    pub fn items(&self) -> &[SpineItem] {
        &self.items
    }

    /// Get optional TOC item id from `<spine toc=\"...\">` (EPUB 2.0).
    pub fn toc_id(&self) -> Option<&str> {
        self.toc_id.as_deref()
    }

    /// Get total number of chapters
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if spine is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Get current chapter ID
    pub fn current_id(&self) -> Option<&str> {
        self.items.get(self.current).map(|item| item.idref.as_str())
    }

    /// Get current spine item
    pub fn current_item(&self) -> Option<&SpineItem> {
        self.items.get(self.current)
    }

    /// Get chapter ID at specific index
    pub fn get_id(&self, index: usize) -> Option<&str> {
        self.items.get(index).map(|item| item.idref.as_str())
    }

    /// Get spine item at specific index
    pub fn get_item(&self, index: usize) -> Option<&SpineItem> {
        self.items.get(index)
    }

    /// Get current position (0-indexed)
    pub fn position(&self) -> usize {
        self.current
    }

    /// Navigate to next chapter.
    /// Returns true if navigation succeeded, false if at end.
    pub fn advance(&mut self) -> bool {
        if self.current + 1 < self.items.len() {
            self.current += 1;
            true
        } else {
            false
        }
    }

    /// Navigate to previous chapter
    /// Returns true if navigation succeeded, false if at start
    pub fn prev(&mut self) -> bool {
        if self.current > 0 {
            self.current -= 1;
            true
        } else {
            false
        }
    }

    /// Navigate to specific chapter by index
    /// Returns true if successful
    pub fn go_to(&mut self, index: usize) -> bool {
        if index < self.items.len() {
            self.current = index;
            true
        } else {
            false
        }
    }

    /// Navigate to chapter by ID
    /// Returns true if found and navigated
    pub fn go_to_id(&mut self, idref: &str) -> bool {
        if let Some(index) = self.items.iter().position(|item| item.idref == idref) {
            self.current = index;
            true
        } else {
            false
        }
    }

    /// Get progress as percentage (0-100)
    pub fn progress_percent(&self) -> u8 {
        if self.items.is_empty() {
            0
        } else {
            ((self.current * 100) / self.items.len()).min(100) as u8
        }
    }

    /// Get progress as fraction (current, total)
    pub fn progress(&self) -> (usize, usize) {
        (self.current, self.items.len())
    }

    /// Check if at first chapter
    pub fn is_first(&self) -> bool {
        self.current == 0
    }

    /// Check if at last chapter
    pub fn is_last(&self) -> bool {
        self.current + 1 >= self.items.len()
    }

    /// Get all chapter IDs as strings
    pub fn chapter_ids(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.idref.as_str()).collect()
    }
}

/// Parse spine from OPF content
///
/// Extracts the ordered list of itemrefs from the spine element (default limits).
#[cfg(not(feature = "std"))]
pub fn parse_spine(content: &[u8]) -> Result<Spine, EpubError> {
    parse_spine_with_limits(content, &MetadataLimits::default())
}

/// Extracts the ordered list of itemrefs with caller-provided limits.
#[cfg(not(feature = "std"))]
pub fn parse_spine_with_limits(
    content: &[u8],
    limits: &MetadataLimits,
) -> Result<Spine, EpubError> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(8);
    #[cfg(target_os = "espidf")]
    let mut skip_buf = Vec::with_capacity(8);
    let mut spine = Spine::new();
    let mut in_spine = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();
                #[cfg(target_os = "espidf")]
                if (name == "metadata" || name == "manifest" || name == "guide") && !e.is_empty() {
                    reader
                        .read_to_end_into(e.name(), &mut skip_buf)
                        .map_err(|e| EpubError::Parse(format!("XML parse error: {:?}", e)))?;
                    buf.clear();
                    continue;
                }

                if name == "spine" {
                    in_spine = true;

                    // Check for toc attribute (EPUB2 NCX reference)
                    for attr in e.attributes() {
                        let attr =
                            attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                        let key = reader
                            .decoder()
                            .decode(attr.key.as_ref())
                            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                        if key == "toc" {
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                                .to_string();
                            if !value.is_empty() {
                                spine.toc_id = Some(value);
                            }
                        }
                    }
                }

                if in_spine && name == "itemref" && spine.items.len() < limits.max_spine_items {
                    if let Some(item) = parse_spine_item(&e, &reader)? {
                        spine.items.push(item);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                if name == "spine" {
                    in_spine = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(spine)
}

#[cfg(feature = "std")]
fn parse_spine_reader<R: std::io::BufRead>(
    reader: R,
    limits: &MetadataLimits,
) -> Result<Spine, EpubError> {
    let mut reader = Reader::from_reader(reader);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(8);
    let mut spine = Spine::new();
    let mut in_spine = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                if name == "spine" {
                    in_spine = true;

                    for attr in e.attributes() {
                        let attr =
                            attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                        let key = reader
                            .decoder()
                            .decode(attr.key.as_ref())
                            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                        if key == "toc" {
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                                .to_string();
                            if !value.is_empty() {
                                spine.toc_id = Some(value);
                            }
                        }
                    }
                }

                if in_spine && name == "itemref" && spine.items.len() < limits.max_spine_items {
                    if let Some(item) = parse_spine_item_reader(&e, &reader)? {
                        spine.items.push(item);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                if name == "spine" {
                    in_spine = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(spine)
}

#[cfg(feature = "std")]
/// Parse spine from OPF content (default limits).
pub fn parse_spine(content: &[u8]) -> Result<Spine, EpubError> {
    parse_spine_reader(content, &MetadataLimits::default())
}

#[cfg(feature = "std")]
/// Parse spine from OPF content with caller-provided limits.
pub fn parse_spine_with_limits(
    content: &[u8],
    limits: &MetadataLimits,
) -> Result<Spine, EpubError> {
    parse_spine_reader(content, limits)
}

#[cfg(feature = "std")]
/// Parse spine from a file-backed OPF stream (default limits).
pub fn parse_spine_file<P: AsRef<std::path::Path>>(path: P) -> Result<Spine, EpubError> {
    parse_spine_file_with_limits(path, &MetadataLimits::default())
}

#[cfg(feature = "std")]
/// Parse spine from a file-backed OPF stream with caller-provided limits.
pub fn parse_spine_file_with_limits<P: AsRef<std::path::Path>>(
    path: P,
    limits: &MetadataLimits,
) -> Result<Spine, EpubError> {
    let file = std::fs::File::open(path)
        .map_err(|e| EpubError::Io(format!("Failed to open OPF: {}", e)))?;
    let reader = std::io::BufReader::new(file);
    parse_spine_reader(reader, limits)
}

/// Parse a spine itemref from XML element attributes
#[cfg(not(feature = "std"))]
fn parse_spine_item<'a>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<&[u8]>,
) -> Result<Option<SpineItem>, EpubError> {
    let mut idref = None;
    let mut id = None;
    let mut linear = true;
    let mut properties = None;

    for attr in e.attributes() {
        let attr = attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
        let key = reader
            .decoder()
            .decode(attr.key.as_ref())
            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
        let value = reader
            .decoder()
            .decode(&attr.value)
            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
            .to_string();

        match key.as_ref() {
            "idref" => idref = Some(value),
            "id" => id = Some(value),
            "linear" => linear = value != "no",
            "properties" => properties = Some(value),
            _ => {}
        }
    }

    idref
        .map(|idref| {
            Ok(SpineItem {
                idref,
                id,
                linear,
                properties,
            })
        })
        .transpose()
}

#[cfg(feature = "std")]
fn parse_spine_item_reader<'a, R: std::io::BufRead>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<R>,
) -> Result<Option<SpineItem>, EpubError> {
    let mut idref = None;
    let mut id = None;
    let mut linear = true;
    let mut properties = None;

    for attr in e.attributes() {
        let attr = attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
        let key = reader
            .decoder()
            .decode(attr.key.as_ref())
            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
        let value = reader
            .decoder()
            .decode(&attr.value)
            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
            .to_string();

        match key.as_ref() {
            "idref" => idref = Some(value),
            "id" => id = Some(value),
            "linear" => linear = value != "no",
            "properties" => properties = Some(value),
            _ => {}
        }
    }

    idref
        .map(|idref| {
            Ok(SpineItem {
                idref,
                id,
                linear,
                properties,
            })
        })
        .transpose()
}

/// Parse both metadata and spine from OPF content
///
/// Convenience function that extracts both structures in one pass.
/// Note: This is less efficient than separate parsing if you only need one.
pub fn parse_opf_spine(content: &[u8]) -> Result<Spine, EpubError> {
    parse_spine(content)
}

/// Create a spine from raw chapter IDs (for testing or simple EPUBs)
pub fn create_spine(chapter_ids: &[&str]) -> Spine {
    let items = chapter_ids
        .iter()
        .map(|id| SpineItem {
            idref: id.to_string(),
            id: None,
            linear: true,
            properties: None,
        })
        .collect();

    Spine {
        items,
        current: 0,
        toc_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_spine_basic() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine>
    <itemref idref="cover"/>
    <itemref idref="chapter1"/>
    <itemref idref="chapter2"/>
    <itemref idref="chapter3"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        assert_eq!(spine.len(), 4);
        assert_eq!(spine.get_id(0), Some("cover"));
        assert_eq!(spine.get_id(1), Some("chapter1"));
        assert_eq!(spine.get_id(2), Some("chapter2"));
        assert_eq!(spine.get_id(3), Some("chapter3"));
        assert_eq!(spine.toc_id(), None);
    }

    #[test]
    fn test_parse_spine_toc_attribute() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <spine toc="ncx">
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        assert_eq!(spine.toc_id(), Some("ncx"));
    }

    #[test]
    fn test_spine_navigation() {
        let mut spine = create_spine(&["a", "b", "c", "d"]);

        assert_eq!(spine.position(), 0);
        assert_eq!(spine.current_id(), Some("a"));
        assert!(spine.is_first());
        assert!(!spine.is_last());

        assert!(spine.advance());
        assert_eq!(spine.position(), 1);
        assert_eq!(spine.current_id(), Some("b"));

        assert!(spine.advance());
        assert!(spine.advance());
        assert_eq!(spine.position(), 3);
        assert!(spine.is_last());

        // Can't go past end
        assert!(!spine.advance());
        assert_eq!(spine.position(), 3);

        // Go back
        assert!(spine.prev());
        assert_eq!(spine.position(), 2);

        // Jump to position
        assert!(spine.go_to(0));
        assert_eq!(spine.position(), 0);

        // Invalid jump
        assert!(!spine.go_to(100));
        assert_eq!(spine.position(), 0);
    }

    #[test]
    fn test_go_to_id() {
        let mut spine = create_spine(&["cover", "ch1", "ch2"]);

        assert!(spine.go_to_id("ch1"));
        assert_eq!(spine.position(), 1);

        assert!(!spine.go_to_id("nonexistent"));
        assert_eq!(spine.position(), 1); // Unchanged
    }

    #[test]
    fn test_progress() {
        let mut spine = create_spine(&["a", "b", "c", "d"]);

        assert_eq!(spine.progress(), (0, 4));
        assert_eq!(spine.progress_percent(), 0);

        spine.go_to(2);
        assert_eq!(spine.progress(), (2, 4));
        assert_eq!(spine.progress_percent(), 50);

        spine.go_to(3);
        assert_eq!(spine.progress_percent(), 75);
    }

    #[test]
    fn test_parse_spine_with_attributes() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine toc="ncx">
    <itemref idref="cover" id="item-1" linear="yes"/>
    <itemref idref="nav" id="item-2" linear="no" properties="nav"/>
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        assert_eq!(spine.len(), 3);

        let item0 = spine.get_item(0).unwrap();
        assert_eq!(item0.idref, "cover");
        assert_eq!(item0.id, Some("item-1".to_string()));
        assert!(item0.linear);

        let item1 = spine.get_item(1).unwrap();
        assert_eq!(item1.idref, "nav");
        assert_eq!(item1.id, Some("item-2".to_string()));
        assert!(!item1.linear); // linear="no"
        assert_eq!(item1.properties, Some("nav".to_string()));
    }

    #[test]
    fn test_empty_spine() {
        let mut spine = Spine::new();
        assert!(spine.is_empty());
        assert_eq!(spine.progress_percent(), 0);
        assert!(!spine.advance());
        assert!(!spine.prev());
    }

    #[test]
    fn test_chapter_ids() {
        let spine = create_spine(&["a", "b", "c"]);
        let ids = spine.chapter_ids();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    // -- Additional edge case tests ---

    #[test]
    fn test_parse_spine_linear_no() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine>
    <itemref idref="cover" linear="yes"/>
    <itemref idref="nav" linear="no"/>
    <itemref idref="chapter1"/>
    <itemref idref="appendix" linear="no"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        assert_eq!(spine.len(), 4);

        // linear="yes" or absent → linear=true
        assert!(spine.get_item(0).unwrap().linear);
        assert!(spine.get_item(2).unwrap().linear);

        // linear="no" → linear=false, but still included
        assert!(!spine.get_item(1).unwrap().linear);
        assert!(!spine.get_item(3).unwrap().linear);
        assert_eq!(spine.get_item(1).unwrap().idref, "nav");
        assert_eq!(spine.get_item(3).unwrap().idref, "appendix");
    }

    #[test]
    fn test_parse_spine_duplicate_idrefs() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine>
    <itemref idref="chapter1"/>
    <itemref idref="chapter1"/>
    <itemref idref="chapter2"/>
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        assert_eq!(spine.len(), 4);
        assert_eq!(spine.get_id(0), Some("chapter1"));
        assert_eq!(spine.get_id(1), Some("chapter1"));
        assert_eq!(spine.get_id(2), Some("chapter2"));
        assert_eq!(spine.get_id(3), Some("chapter1"));

        // go_to_id finds the first occurrence
        let mut spine = spine;
        assert!(spine.go_to_id("chapter1"));
        assert_eq!(spine.position(), 0);
    }

    #[test]
    fn test_from_idrefs_constructor() {
        let idrefs = alloc::vec![
            "intro".to_string(),
            "ch1".to_string(),
            "ch2".to_string(),
            "epilogue".to_string(),
        ];
        let spine = Spine::from_idrefs(idrefs);
        assert_eq!(spine.len(), 4);
        assert_eq!(spine.position(), 0);
        assert_eq!(spine.current_id(), Some("intro"));

        // All items should be linear by default
        for i in 0..4 {
            let item = spine.get_item(i).unwrap();
            assert!(item.linear);
            assert!(item.id.is_none());
            assert!(item.properties.is_none());
        }

        assert_eq!(spine.get_id(3), Some("epilogue"));
    }

    #[test]
    fn test_from_idrefs_empty() {
        let spine = Spine::from_idrefs(alloc::vec![]);
        assert!(spine.is_empty());
        assert_eq!(spine.len(), 0);
        assert_eq!(spine.current_id(), None);
    }

    #[test]
    fn test_very_large_spine() {
        let mut idrefs = Vec::with_capacity(8);
        for i in 0..150 {
            idrefs.push(alloc::format!("chapter{}", i));
        }
        let spine = Spine::from_idrefs(idrefs);
        assert_eq!(spine.len(), 150);

        // Verify first, middle, last
        assert_eq!(spine.get_id(0), Some("chapter0"));
        assert_eq!(spine.get_id(74), Some("chapter74"));
        assert_eq!(spine.get_id(149), Some("chapter149"));

        // Check out-of-bounds
        assert_eq!(spine.get_id(150), None);

        // Navigate through all
        let mut spine = spine;
        for i in 0..149 {
            assert_eq!(spine.position(), i);
            assert!(spine.advance());
        }
        assert_eq!(spine.position(), 149);
        assert!(!spine.advance()); // Can't go past end
    }

    #[test]
    fn test_go_to_boundary_conditions() {
        let mut spine = create_spine(&["a", "b", "c", "d", "e"]);

        // go_to(0) when already at 0
        assert_eq!(spine.position(), 0);
        assert!(spine.go_to(0));
        assert_eq!(spine.position(), 0);

        // go_to(len-1) — last valid index
        assert!(spine.go_to(4));
        assert_eq!(spine.position(), 4);
        assert!(spine.is_last());
        assert_eq!(spine.current_id(), Some("e"));

        // go_to(len) — just past the end, should fail
        assert!(!spine.go_to(5));
        assert_eq!(spine.position(), 4); // unchanged

        // go_to(0) from end
        assert!(spine.go_to(0));
        assert_eq!(spine.position(), 0);
        assert!(spine.is_first());
    }

    #[test]
    fn test_navigation_advance_to_end_then_prev_back() {
        let mut spine = create_spine(&["a", "b", "c", "d", "e"]);

        // Advance all the way to the end
        assert!(spine.advance()); // 0 -> 1
        assert!(spine.advance()); // 1 -> 2
        assert!(spine.advance()); // 2 -> 3
        assert!(spine.advance()); // 3 -> 4
        assert!(!spine.advance()); // at end, can't advance
        assert_eq!(spine.position(), 4);
        assert!(spine.is_last());

        // Prev all the way back to start
        assert!(spine.prev()); // 4 -> 3
        assert_eq!(spine.current_id(), Some("d"));
        assert!(spine.prev()); // 3 -> 2
        assert_eq!(spine.current_id(), Some("c"));
        assert!(spine.prev()); // 2 -> 1
        assert_eq!(spine.current_id(), Some("b"));
        assert!(spine.prev()); // 1 -> 0
        assert_eq!(spine.current_id(), Some("a"));
        assert!(!spine.prev()); // at start, can't go back
        assert_eq!(spine.position(), 0);
        assert!(spine.is_first());
    }

    #[test]
    fn test_progress_percent_edge_cases() {
        // Empty spine
        let spine = Spine::new();
        assert_eq!(spine.progress_percent(), 0);

        // Single item
        let spine = create_spine(&["only"]);
        assert_eq!(spine.progress_percent(), 0); // 0/1 * 100 = 0

        // Two items
        let mut spine = create_spine(&["a", "b"]);
        assert_eq!(spine.progress_percent(), 0); // 0/2 * 100 = 0
        spine.advance();
        assert_eq!(spine.progress_percent(), 50); // 1/2 * 100 = 50

        // Three items
        let mut spine = create_spine(&["a", "b", "c"]);
        assert_eq!(spine.progress_percent(), 0); // 0/3 = 0%
        spine.advance();
        assert_eq!(spine.progress_percent(), 33); // 1/3 = 33%
        spine.advance();
        assert_eq!(spine.progress_percent(), 66); // 2/3 = 66%

        // 10 items, check various positions
        let mut spine = create_spine(&["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
        assert_eq!(spine.progress_percent(), 0);
        spine.go_to(5);
        assert_eq!(spine.progress_percent(), 50);
        spine.go_to(9);
        assert_eq!(spine.progress_percent(), 90);
    }

    #[test]
    fn test_progress_fraction() {
        let mut spine = create_spine(&["a", "b", "c"]);
        assert_eq!(spine.progress(), (0, 3));
        spine.advance();
        assert_eq!(spine.progress(), (1, 3));
        spine.advance();
        assert_eq!(spine.progress(), (2, 3));
    }

    #[test]
    fn test_single_item_spine() {
        let mut spine = create_spine(&["only"]);
        assert_eq!(spine.len(), 1);
        assert!(spine.is_first());
        assert!(spine.is_last());
        assert!(!spine.advance());
        assert!(!spine.prev());
        assert_eq!(spine.current_id(), Some("only"));
    }

    #[test]
    fn test_current_item_returns_full_spine_item() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine>
    <itemref idref="ch1" id="spine-1" properties="rendition:layout-pre-paginated"/>
  </spine>
</package>"#;

        let spine = parse_spine(opf).unwrap();
        let item = spine.current_item().unwrap();
        assert_eq!(item.idref, "ch1");
        assert_eq!(item.id, Some("spine-1".to_string()));
        assert!(item.linear);
        assert_eq!(
            item.properties,
            Some("rendition:layout-pre-paginated".to_string())
        );
    }

    #[test]
    fn test_parse_opf_spine_convenience() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#;

        let spine = parse_opf_spine(opf).unwrap();
        assert_eq!(spine.len(), 2);
        assert_eq!(spine.get_id(0), Some("ch1"));
    }

    #[test]
    fn test_get_item_out_of_bounds() {
        let spine = create_spine(&["a", "b"]);
        assert!(spine.get_item(0).is_some());
        assert!(spine.get_item(1).is_some());
        assert!(spine.get_item(2).is_none());
        assert!(spine.get_id(2).is_none());
    }
}
