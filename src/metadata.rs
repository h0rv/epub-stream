//! EPUB metadata parser using quick-xml SAX-style parsing
//!
//! Parses container.xml to find the OPF package file,
//! then extracts metadata and manifest from the OPF.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::EpubError;

/// Maximum number of manifest items (fixed-size constraint)
const MAX_MANIFEST_ITEMS: usize = 1024;

/// Maximum number of subject tags
const MAX_SUBJECTS: usize = 64;

/// Maximum number of guide references
const MAX_GUIDE_REFS: usize = 64;

/// A single item in the EPUB manifest (id -> href mapping)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestItem {
    /// Resource identifier
    pub id: String,
    /// Path relative to OPF
    pub href: String,
    /// MIME type
    pub media_type: String,
    /// Optional properties (e.g. "cover-image", "nav")
    pub properties: Option<String>,
}

/// A reference from the EPUB 2.0 `<guide>` element
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideRef {
    /// Reference type (e.g. "cover", "toc", "text")
    pub guide_type: String,
    /// Display title
    pub title: Option<String>,
    /// Path relative to OPF
    pub href: String,
}

/// EPUB metadata extracted from content.opf
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpubMetadata {
    /// Book title
    pub title: String,
    /// Author name
    pub author: String,
    /// Language code (e.g. "en")
    pub language: String,
    /// All resources declared in the manifest
    pub manifest: Vec<ManifestItem>,
    /// Manifest ID of the cover image, if any
    pub cover_id: Option<String>,

    // -- Dublin Core extended metadata --
    /// Publication date (dc:date)
    pub date: Option<String>,
    /// Publisher (dc:publisher)
    pub publisher: Option<String>,
    /// Rights statement (dc:rights)
    pub rights: Option<String>,
    /// Book description / blurb (dc:description)
    pub description: Option<String>,
    /// Subject tags (dc:subject) — can have multiple
    pub subjects: Vec<String>,
    /// Unique identifier (dc:identifier) — ISBN, UUID, etc.
    pub identifier: Option<String>,

    // -- EPUB-specific metadata --
    /// Last modified date (dcterms:modified)
    pub modified: Option<String>,
    /// Rendition layout (e.g. "reflowable", "pre-paginated")
    pub rendition_layout: Option<String>,

    // -- EPUB 2.0 guide --
    /// Guide references (EPUB 2.0, deprecated but common)
    pub guide: Vec<GuideRef>,

    // -- Container metadata --
    /// Path to the OPF file as specified in container.xml rootfile
    pub opf_path: Option<String>,
}

impl Default for EpubMetadata {
    fn default() -> Self {
        Self {
            title: String::with_capacity(0),
            author: String::with_capacity(0),
            language: String::from("en"),
            manifest: Vec::with_capacity(0),
            cover_id: None,
            date: None,
            publisher: None,
            rights: None,
            description: None,
            subjects: Vec::with_capacity(0),
            identifier: None,
            modified: None,
            rendition_layout: None,
            guide: Vec::with_capacity(0),
            opf_path: None,
        }
    }
}

impl EpubMetadata {
    /// Create empty metadata structure
    pub fn new() -> Self {
        Self::default()
    }

    /// Get manifest item by id
    pub fn get_item(&self, id: &str) -> Option<&ManifestItem> {
        self.manifest.iter().find(|item| item.id == id)
    }

    /// Get cover image manifest item
    pub fn get_cover_item(&self) -> Option<&ManifestItem> {
        self.cover_id.as_ref().and_then(|id| self.get_item(id))
    }

    /// Find item ID by href path
    pub fn find_item_by_href(&self, href: &str) -> Option<&str> {
        self.manifest
            .iter()
            .find(|item| item.href == href)
            .map(|item| item.id.as_str())
    }
}

fn local_attr_name(name: &str) -> &str {
    local_name(name)
}

fn is_supported_cover_ref(href: &str) -> bool {
    let trimmed = href.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !lower.starts_with("data:")
}

/// Extract the first cover image reference from cover XHTML content.
///
/// Supports common EPUB cover patterns:
/// - `<img src="...">`
/// - `<svg:image href="...">` / `xlink:href`
///
/// Returns the referenced href exactly as declared (relative path, absolute path,
/// or fragment-stripped value), so callers can resolve it relative to the XHTML file.
pub fn extract_cover_image_href_from_xhtml(content: &[u8]) -> Option<String> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(0);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader.decoder().decode(e.name().as_ref()).ok()?.to_string();
                let local = local_name(&name);
                if local != "img" && local != "image" {
                    buf.clear();
                    continue;
                }

                for attr in e.attributes() {
                    let attr = attr.ok()?;
                    let key = reader.decoder().decode(attr.key.as_ref()).ok()?.to_string();
                    let key_local = local_attr_name(&key);
                    if key_local != "src" && key_local != "href" {
                        continue;
                    }
                    let value = reader.decoder().decode(&attr.value).ok()?.to_string();
                    if is_supported_cover_ref(&value) {
                        return Some(value);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
    None
}

/// Parse container.xml to find the OPF package file path
///
/// Returns the full-path attribute from the rootfile element
#[cfg(not(feature = "std"))]
pub fn parse_container_xml(content: &[u8]) -> Result<String, EpubError> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(0);
    let mut opf_path: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();
                let local = local_name(&name);

                if local == "rootfile" {
                    // Extract full-path attribute
                    for attr in e.attributes() {
                        let attr =
                            attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                        let key = reader
                            .decoder()
                            .decode(attr.key.as_ref())
                            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                        if key == "full-path" {
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                                .to_string();
                            opf_path = Some(value);
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    opf_path.ok_or_else(|| EpubError::InvalidEpub("No rootfile found in container.xml".into()))
}

#[cfg(feature = "std")]
fn parse_container_xml_reader<R: std::io::BufRead>(reader: R) -> Result<String, EpubError> {
    let mut reader = Reader::from_reader(reader);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(0);
    let mut opf_path: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();
                let local = local_name(&name);

                if local == "rootfile" {
                    for attr in e.attributes() {
                        let attr =
                            attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                        let key = reader
                            .decoder()
                            .decode(attr.key.as_ref())
                            .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                        if key == "full-path" {
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                                .to_string();
                            opf_path = Some(value);
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    opf_path.ok_or_else(|| EpubError::InvalidEpub("No rootfile found in container.xml".into()))
}

/// Parse container.xml to find the OPF package file path
///
/// Returns the full-path attribute from the rootfile element
#[cfg(feature = "std")]
pub fn parse_container_xml(content: &[u8]) -> Result<String, EpubError> {
    parse_container_xml_reader(content)
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Parse content.opf to extract metadata and manifest
///
/// Uses SAX-style parsing with quick-xml
#[cfg(not(feature = "std"))]
pub fn parse_opf(content: &[u8]) -> Result<EpubMetadata, EpubError> {
    let mut reader = Reader::from_reader(content);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(0);
    let mut metadata = EpubMetadata::new();

    // State tracking
    let mut current_element: Option<String> = None;
    let mut in_metadata = false;
    let mut in_manifest = false;
    let mut in_spine = false;
    let mut in_guide = false;
    let mut current_meta_property: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                let local = local_name(&name);
                // Track which section we're in
                match local {
                    "metadata" => in_metadata = true,
                    "manifest" => in_manifest = true,
                    "spine" => in_spine = true,
                    "guide" => in_guide = true,
                    _ => {}
                }

                // Parse manifest item
                if in_manifest && local == "item" && metadata.manifest.len() < MAX_MANIFEST_ITEMS {
                    if let Some(item) = parse_manifest_item(&e, &reader)? {
                        // Check if this is a cover image (EPUB3)
                        if item
                            .properties
                            .as_ref()
                            .is_some_and(|p| p.contains("cover-image"))
                        {
                            metadata.cover_id = Some(item.id.clone());
                        }
                        metadata.manifest.push(item);
                    }
                }

                // Track metadata elements
                if in_metadata {
                    current_element = Some(local.to_string());

                    // Check for EPUB2 cover meta tag and EPUB3 meta properties
                    if local == "meta" {
                        let mut name_attr = None;
                        let mut content_attr = None;
                        let mut property_attr = None;

                        for attr in e.attributes() {
                            let attr =
                                attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                            let key = reader
                                .decoder()
                                .decode(attr.key.as_ref())
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;

                            if key == "name" && value == "cover" {
                                name_attr = Some(value.to_string());
                            }
                            if key == "content" {
                                content_attr = Some(value.to_string());
                            }
                            if key == "property" {
                                property_attr = Some(value.to_string());
                            }
                        }

                        if name_attr.is_some() && content_attr.is_some() {
                            metadata.cover_id = content_attr;
                        }

                        // Track EPUB3 meta property for upcoming Text event
                        current_meta_property = property_attr;
                    }
                }

                // Parse guide reference (Start variant, in case it has children)
                if in_guide && local == "reference" && metadata.guide.len() < MAX_GUIDE_REFS {
                    if let Some(guide_ref) = parse_guide_reference(&e, &reader)? {
                        metadata.guide.push(guide_ref);
                    }
                }

                // Track spine itemref
                if in_spine && local == "itemref" {
                    // Spine items are collected separately by spine.rs
                    // We just validate the structure here
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(ref elem) = current_element {
                    let text = reader
                        .decoder()
                        .decode(&e)
                        .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                        .to_string();

                    // Handle EPUB3 <meta property="..."> text content
                    if elem == "meta" {
                        if let Some(ref prop) = current_meta_property {
                            match prop.as_str() {
                                "dcterms:modified" => {
                                    metadata.modified = Some(text.clone());
                                }
                                "rendition:layout" => {
                                    metadata.rendition_layout = Some(text.clone());
                                }
                                _ => {}
                            }
                        }
                    }

                    // Extract metadata fields (Dublin Core only)
                    match elem.as_str() {
                        "title" => {
                            metadata.title = text;
                        }
                        "creator" => {
                            metadata.author = text;
                        }
                        "language" => {
                            metadata.language = text;
                        }
                        "date" => {
                            metadata.date = Some(text);
                        }
                        "publisher" => {
                            metadata.publisher = Some(text);
                        }
                        "rights" => {
                            metadata.rights = Some(text);
                        }
                        "description" => {
                            metadata.description = Some(text);
                        }
                        "subject" => {
                            if metadata.subjects.len() < MAX_SUBJECTS {
                                metadata.subjects.push(text);
                            }
                        }
                        "identifier" => {
                            metadata.identifier = Some(text);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                match local_name(&name) {
                    "metadata" => in_metadata = false,
                    "manifest" => in_manifest = false,
                    "spine" => in_spine = false,
                    "guide" => in_guide = false,
                    _ => {}
                }

                current_element = None;
                current_meta_property = None;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(metadata)
}

#[cfg(feature = "std")]
fn parse_opf_reader<R: std::io::BufRead>(reader: R) -> Result<EpubMetadata, EpubError> {
    let mut reader = Reader::from_reader(reader);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::with_capacity(0);
    let mut metadata = EpubMetadata::new();

    let mut current_element: Option<String> = None;
    let mut in_metadata = false;
    let mut in_manifest = false;
    let mut in_spine = false;
    let mut in_guide = false;
    let mut current_meta_property: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .to_string();

                let local = local_name(&name);
                match local {
                    "metadata" => in_metadata = true,
                    "manifest" => in_manifest = true,
                    "spine" => in_spine = true,
                    "guide" => in_guide = true,
                    _ => {}
                }

                if in_manifest && local == "item" && metadata.manifest.len() < MAX_MANIFEST_ITEMS {
                    if let Some(item) = parse_manifest_item_reader(&e, &reader)? {
                        if item
                            .properties
                            .as_ref()
                            .is_some_and(|p| p.contains("cover-image"))
                        {
                            metadata.cover_id = Some(item.id.clone());
                        }
                        metadata.manifest.push(item);
                    }
                }

                if in_metadata {
                    current_element = Some(local.to_string());

                    if local == "meta" {
                        let mut name_attr = None;
                        let mut content_attr = None;
                        let mut property_attr = None;

                        for attr in e.attributes() {
                            let attr =
                                attr.map_err(|e| EpubError::Parse(format!("Attr error: {:?}", e)))?;
                            let key = reader
                                .decoder()
                                .decode(attr.key.as_ref())
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;
                            let value = reader
                                .decoder()
                                .decode(&attr.value)
                                .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?;

                            if key == "name" && value == "cover" {
                                name_attr = Some(value.to_string());
                            }
                            if key == "content" {
                                content_attr = Some(value.to_string());
                            }
                            if key == "property" {
                                property_attr = Some(value.to_string());
                            }
                        }

                        if name_attr.is_some() && content_attr.is_some() {
                            metadata.cover_id = content_attr;
                        }

                        current_meta_property = property_attr;
                    }
                }

                if in_guide && local == "reference" && metadata.guide.len() < MAX_GUIDE_REFS {
                    if let Some(guide_ref) = parse_guide_reference_reader(&e, &reader)? {
                        metadata.guide.push(guide_ref);
                    }
                }

                if in_spine && local == "itemref" {}
            }
            Ok(Event::Text(e)) => {
                if let Some(ref elem) = current_element {
                    let text = reader
                        .decoder()
                        .decode(&e)
                        .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                        .to_string();

                    match elem.as_str() {
                        "title" => metadata.title = text,
                        "creator" => metadata.author = text,
                        "language" => metadata.language = text,
                        "date" => metadata.date = Some(text),
                        "publisher" => metadata.publisher = Some(text),
                        "rights" => metadata.rights = Some(text),
                        "description" => metadata.description = Some(text),
                        "subject" => {
                            if metadata.subjects.len() < MAX_SUBJECTS {
                                metadata.subjects.push(text);
                            }
                        }
                        "identifier" => metadata.identifier = Some(text),
                        "meta" => {
                            if let Some(property) = current_meta_property.take() {
                                if property == "dcterms:modified" {
                                    metadata.modified = Some(text);
                                } else if property == "rendition:layout" {
                                    metadata.rendition_layout = Some(text);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| EpubError::Parse(format!("Decode error: {:?}", e)))?
                    .into_owned();
                match local_name(&name) {
                    "metadata" => in_metadata = false,
                    "manifest" => in_manifest = false,
                    "spine" => in_spine = false,
                    "guide" => in_guide = false,
                    _ => {}
                }
                current_element = None;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(EpubError::Parse(format!("XML parse error: {:?}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(metadata)
}

#[cfg(feature = "std")]
/// Parse content.opf to extract metadata and manifest.
pub fn parse_opf(content: &[u8]) -> Result<EpubMetadata, EpubError> {
    parse_opf_reader(content)
}

/// Parse a manifest item from XML element attributes
#[cfg(not(feature = "std"))]
fn parse_manifest_item<'a>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<&[u8]>,
) -> Result<Option<ManifestItem>, EpubError> {
    let mut id = None;
    let mut href = None;
    let mut media_type = None;
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
            "id" => id = Some(value),
            "href" => href = Some(value),
            "media-type" => media_type = Some(value),
            "properties" => properties = Some(value),
            _ => {}
        }
    }

    if let (Some(id), Some(href), Some(media_type)) = (id, href, media_type) {
        Ok(Some(ManifestItem {
            id,
            href,
            media_type,
            properties,
        }))
    } else {
        Ok(None) // Skip incomplete items
    }
}

/// Parse a guide reference from XML element attributes
#[cfg(not(feature = "std"))]
fn parse_guide_reference<'a>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<&[u8]>,
) -> Result<Option<GuideRef>, EpubError> {
    let mut guide_type = None;
    let mut title = None;
    let mut href = None;

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
            "type" => guide_type = Some(value),
            "title" => title = Some(value),
            "href" => href = Some(value),
            _ => {}
        }
    }

    if let (Some(guide_type), Some(href)) = (guide_type, href) {
        Ok(Some(GuideRef {
            guide_type,
            title,
            href,
        }))
    } else {
        Ok(None) // Skip incomplete references
    }
}

#[cfg(feature = "std")]
fn parse_manifest_item_reader<'a, R: std::io::BufRead>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<R>,
) -> Result<Option<ManifestItem>, EpubError> {
    let mut id = None;
    let mut href = None;
    let mut media_type = None;
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
            "id" => id = Some(value),
            "href" => href = Some(value),
            "media-type" => media_type = Some(value),
            "properties" => properties = Some(value),
            _ => {}
        }
    }

    if let (Some(id), Some(href), Some(media_type)) = (id, href, media_type) {
        Ok(Some(ManifestItem {
            id,
            href,
            media_type,
            properties,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(feature = "std")]
fn parse_guide_reference_reader<'a, R: std::io::BufRead>(
    e: &quick_xml::events::BytesStart<'a>,
    reader: &Reader<R>,
) -> Result<Option<GuideRef>, EpubError> {
    let mut guide_type = None;
    let mut title = None;
    let mut href = None;

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
            "type" => guide_type = Some(value),
            "title" => title = Some(value),
            "href" => href = Some(value),
            _ => {}
        }
    }

    if let (Some(guide_type), Some(href)) = (guide_type, href) {
        Ok(Some(GuideRef {
            guide_type,
            title,
            href,
        }))
    } else {
        Ok(None)
    }
}

/// Full EPUB metadata extraction from both container.xml and content.opf
///
/// This is a convenience function that takes both file contents and returns
/// the complete metadata structure. The function parses container.xml to
/// extract the rootfile path and stores it in the metadata result.
pub fn extract_metadata(
    container_xml: &[u8],
    opf_content: &[u8],
) -> Result<EpubMetadata, EpubError> {
    // Parse container.xml to get the OPF path from rootfile element
    let opf_path = parse_container_xml(container_xml)?;

    // Parse the OPF content
    let mut metadata = parse_opf(opf_content)?;

    // Store the OPF path in the metadata (we use the cover_id field's Option<String> type pattern)
    // Adding opf_path field to track which path was used
    metadata.opf_path = Some(opf_path);

    Ok(metadata)
}

/// Parse container.xml from a file path (SD card backed)
#[cfg(feature = "std")]
pub fn parse_container_xml_file<P: AsRef<std::path::Path>>(path: P) -> Result<String, EpubError> {
    let file = std::fs::File::open(path)
        .map_err(|e| EpubError::Io(format!("Failed to open container.xml: {}", e)))?;
    let reader = std::io::BufReader::new(file);
    parse_container_xml_reader(reader)
}

/// Parse content.opf from a file path (SD card backed)
#[cfg(feature = "std")]
pub fn parse_opf_file<P: AsRef<std::path::Path>>(path: P) -> Result<EpubMetadata, EpubError> {
    let file = std::fs::File::open(path)
        .map_err(|e| EpubError::Io(format!("Failed to open OPF: {}", e)))?;
    let reader = std::io::BufReader::new(file);
    parse_opf_reader(reader)
}

/// Full EPUB metadata extraction using file-based parsing
///
/// This is a memory-efficient alternative that reads from temp files
/// on SD card instead of loading everything into RAM.
#[cfg(feature = "std")]
pub fn extract_metadata_from_files<P: AsRef<std::path::Path>>(
    container_path: P,
    opf_path: P,
) -> Result<EpubMetadata, EpubError> {
    // Parse container.xml from file to get the OPF path
    let opf_relative_path = parse_container_xml_file(&container_path)?;

    // Parse the OPF content from file
    let mut metadata = parse_opf_file(&opf_path)?;

    // Store the OPF path in the metadata
    metadata.opf_path = Some(opf_relative_path);

    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_container_xml() {
        let container = br#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
   <rootfiles>
      <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#;

        let result = parse_container_xml(container).unwrap();
        assert_eq!(result, "EPUB/package.opf");
    }

    #[test]
    fn test_parse_opf_basic() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
    <item id="chapter1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.title, "Test Book");
        assert_eq!(metadata.author, "Test Author");
        assert_eq!(metadata.language, "en");
        assert_eq!(metadata.manifest.len(), 2);
        assert_eq!(metadata.manifest[0].id, "cover");
        assert_eq!(metadata.manifest[1].href, "chapter1.xhtml");
    }

    #[test]
    fn test_parse_opf_with_cover() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Book with Cover</dc:title>
    <meta name="cover" content="cover-image"/>
  </metadata>
  <manifest>
    <item id="cover-image" href="images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>
  </manifest>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.title, "Book with Cover");
        assert_eq!(metadata.cover_id, Some("cover-image".to_string()));
    }

    #[test]
    fn test_get_item() {
        let mut metadata = EpubMetadata::new();
        metadata.manifest.push(ManifestItem {
            id: "item1".to_string(),
            href: "chapter1.xhtml".to_string(),
            media_type: "application/xhtml+xml".to_string(),
            properties: None,
        });

        let item = metadata.get_item("item1");
        assert!(item.is_some());
        assert_eq!(item.unwrap().href, "chapter1.xhtml");

        assert!(metadata.get_item("nonexistent").is_none());
    }

    #[test]
    fn test_parse_opf_dublin_core_date() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:date>2024-01-15</dc:date>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.date, Some("2024-01-15".to_string()));
    }

    #[test]
    fn test_parse_opf_dublin_core_publisher() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:publisher>Acme Publishing</dc:publisher>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.publisher, Some("Acme Publishing".to_string()));
    }

    #[test]
    fn test_parse_opf_dublin_core_rights() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:rights>Copyright 2024 Author</dc:rights>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.rights, Some("Copyright 2024 Author".to_string()));
    }

    #[test]
    fn test_parse_opf_dublin_core_description() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:description>A fascinating story about testing parsers.</dc:description>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(
            metadata.description,
            Some("A fascinating story about testing parsers.".to_string())
        );
    }

    #[test]
    fn test_parse_opf_dublin_core_identifier() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:identifier>urn:isbn:978-3-16-148410-0</dc:identifier>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(
            metadata.identifier,
            Some("urn:isbn:978-3-16-148410-0".to_string())
        );
    }

    #[test]
    fn test_parse_opf_single_subject() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:subject>Fiction</dc:subject>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.subjects, vec!["Fiction".to_string()]);
    }

    #[test]
    fn test_parse_opf_multiple_subjects() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:subject>Fiction</dc:subject>
    <dc:subject>Science Fiction</dc:subject>
    <dc:subject>Adventure</dc:subject>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.subjects.len(), 3);
        assert_eq!(metadata.subjects[0], "Fiction");
        assert_eq!(metadata.subjects[1], "Science Fiction");
        assert_eq!(metadata.subjects[2], "Adventure");
    }

    #[test]
    fn test_parse_opf_modified_date() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <meta property="dcterms:modified">2024-06-01T12:00:00Z</meta>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.modified, Some("2024-06-01T12:00:00Z".to_string()));
    }

    #[test]
    fn test_parse_opf_rendition_layout() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <meta property="rendition:layout">pre-paginated</meta>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.rendition_layout, Some("pre-paginated".to_string()));
    }

    #[test]
    fn test_parse_opf_rendition_layout_reflowable() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <meta property="rendition:layout">reflowable</meta>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.rendition_layout, Some("reflowable".to_string()));
    }

    #[test]
    fn test_parse_opf_guide_single_reference() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
  </metadata>
  <manifest/>
  <guide>
    <reference type="cover" title="Cover" href="cover.xhtml"/>
  </guide>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.guide.len(), 1);
        assert_eq!(metadata.guide[0].guide_type, "cover");
        assert_eq!(metadata.guide[0].title, Some("Cover".to_string()));
        assert_eq!(metadata.guide[0].href, "cover.xhtml");
    }

    #[test]
    fn test_parse_opf_guide_multiple_references() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
  </metadata>
  <manifest/>
  <guide>
    <reference type="cover" title="Cover" href="cover.xhtml"/>
    <reference type="toc" title="Table of Contents" href="toc.xhtml"/>
    <reference type="text" title="Beginning" href="chapter1.xhtml"/>
  </guide>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.guide.len(), 3);
        assert_eq!(metadata.guide[0].guide_type, "cover");
        assert_eq!(metadata.guide[0].href, "cover.xhtml");
        assert_eq!(metadata.guide[1].guide_type, "toc");
        assert_eq!(
            metadata.guide[1].title,
            Some("Table of Contents".to_string())
        );
        assert_eq!(metadata.guide[1].href, "toc.xhtml");
        assert_eq!(metadata.guide[2].guide_type, "text");
        assert_eq!(metadata.guide[2].href, "chapter1.xhtml");
    }

    #[test]
    fn test_parse_opf_guide_without_title() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
  </metadata>
  <manifest/>
  <guide>
    <reference type="cover" href="cover.xhtml"/>
  </guide>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.guide.len(), 1);
        assert_eq!(metadata.guide[0].guide_type, "cover");
        assert_eq!(metadata.guide[0].title, None);
        assert_eq!(metadata.guide[0].href, "cover.xhtml");
    }

    #[test]
    fn test_parse_opf_empty_optional_fields() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Minimal Book</dc:title>
    <dc:creator>Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.title, "Minimal Book");
        assert_eq!(metadata.author, "Author");
        assert_eq!(metadata.language, "en");
        // All optional fields should be None / empty
        assert_eq!(metadata.date, None);
        assert_eq!(metadata.publisher, None);
        assert_eq!(metadata.rights, None);
        assert_eq!(metadata.description, None);
        assert!(metadata.subjects.is_empty());
        assert_eq!(metadata.identifier, None);
        assert_eq!(metadata.modified, None);
        assert_eq!(metadata.rendition_layout, None);
        assert!(metadata.guide.is_empty());
    }

    #[test]
    fn test_parse_opf_all_dublin_core_fields() {
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Complete Book</dc:title>
    <dc:creator>Jane Doe</dc:creator>
    <dc:language>fr</dc:language>
    <dc:date>2023-03-20</dc:date>
    <dc:publisher>Example Press</dc:publisher>
    <dc:rights>All rights reserved</dc:rights>
    <dc:description>A comprehensive test book.</dc:description>
    <dc:subject>Testing</dc:subject>
    <dc:subject>Software</dc:subject>
    <dc:identifier>urn:uuid:12345678-1234-1234-1234-123456789abc</dc:identifier>
    <meta property="dcterms:modified">2023-06-15T10:30:00Z</meta>
    <meta property="rendition:layout">reflowable</meta>
  </metadata>
  <manifest>
    <item id="ch1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <guide>
    <reference type="toc" title="Contents" href="toc.xhtml"/>
  </guide>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.title, "Complete Book");
        assert_eq!(metadata.author, "Jane Doe");
        assert_eq!(metadata.language, "fr");
        assert_eq!(metadata.date, Some("2023-03-20".to_string()));
        assert_eq!(metadata.publisher, Some("Example Press".to_string()));
        assert_eq!(metadata.rights, Some("All rights reserved".to_string()));
        assert_eq!(
            metadata.description,
            Some("A comprehensive test book.".to_string())
        );
        assert_eq!(metadata.subjects, vec!["Testing", "Software"]);
        assert_eq!(
            metadata.identifier,
            Some("urn:uuid:12345678-1234-1234-1234-123456789abc".to_string())
        );
        assert_eq!(metadata.modified, Some("2023-06-15T10:30:00Z".to_string()));
        assert_eq!(metadata.rendition_layout, Some("reflowable".to_string()));
        assert_eq!(metadata.manifest.len(), 1);
        assert_eq!(metadata.guide.len(), 1);
        assert_eq!(metadata.guide[0].guide_type, "toc");
    }

    #[test]
    fn test_parse_opf_backward_compat_basic() {
        // Ensure the existing basic test still works with the new code
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
    <item id="chapter1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
</package>"#;

        let metadata = parse_opf(opf).unwrap();
        assert_eq!(metadata.title, "Test Book");
        assert_eq!(metadata.author, "Test Author");
        assert_eq!(metadata.language, "en");
        assert_eq!(metadata.manifest.len(), 2);
        // New fields should be None/empty by default
        assert_eq!(metadata.date, None);
        assert!(metadata.subjects.is_empty());
        assert!(metadata.guide.is_empty());
    }

    #[test]
    fn test_extract_metadata_uses_container_xml_path() {
        let container = br#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
   <rootfiles>
      <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = extract_metadata(container, opf).unwrap();
        assert_eq!(metadata.title, "Test Book");
        assert_eq!(metadata.opf_path, Some("EPUB/package.opf".to_string()));
    }

    #[test]
    fn test_extract_metadata_different_rootfile_path() {
        let container = br#"<?xml version="1.0"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
   <rootfiles>
      <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Another Book</dc:title>
    <dc:creator>Another Author</dc:creator>
    <dc:language>fr</dc:language>
  </metadata>
  <manifest/>
</package>"#;

        let metadata = extract_metadata(container, opf).unwrap();
        assert_eq!(metadata.title, "Another Book");
        assert_eq!(metadata.opf_path, Some("OEBPS/content.opf".to_string()));
    }

    #[test]
    fn test_extract_cover_image_href_from_xhtml_img_src() {
        let xhtml = br#"<?xml version="1.0"?>
<!DOCTYPE html PUBLIC '-//W3C//DTD XHTML 1.1//EN' 'http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd'>
<html xmlns="http://www.w3.org/1999/xhtml" lang="en">
  <body>
    <div style="text-align: center">
      <img src="7086129663063996408_cover.jpg" alt="" class="x-ebookmaker-cover" />
    </div>
  </body>
</html>"#;
        let href = extract_cover_image_href_from_xhtml(xhtml)
            .expect("img src should be discovered from cover xhtml");
        assert_eq!(href, "7086129663063996408_cover.jpg");
    }

    #[test]
    fn test_extract_cover_image_href_from_xhtml_svg_image() {
        let xhtml = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:svg="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
  <body>
    <svg:svg viewBox="0 0 100 100">
      <svg:image xlink:href="../images/cover.png" width="100" height="100"/>
    </svg:svg>
  </body>
</html>"#;
        let href = extract_cover_image_href_from_xhtml(xhtml)
            .expect("svg image href should be discovered from cover xhtml");
        assert_eq!(href, "../images/cover.png");
    }
}
