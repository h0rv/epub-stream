//! EPUB validation helpers and structured diagnostics.
//!
//! This module provides a non-panicking validation pass that reports
//! compliance-oriented diagnostics for common EPUB structural requirements.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::metadata::{parse_container_xml, parse_opf, EpubMetadata};
use crate::navigation::{parse_nav_xhtml, parse_ncx};
use crate::spine::Spine;
use crate::zip::{StreamingZip, ZipLimits};

/// Severity level for a validation diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationSeverity {
    /// Violates a required structural expectation.
    Error,
    /// Suspicious or non-ideal structure that may reduce compatibility.
    Warning,
}

/// Structured validation diagnostic entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationDiagnostic {
    /// Stable machine-readable diagnostic code.
    pub code: &'static str,
    /// Severity classification.
    pub severity: ValidationSeverity,
    /// Human-readable description.
    pub message: String,
    /// Optional path in archive related to this diagnostic.
    pub path: Option<String>,
    /// Optional section/hint location (manifest/spine/nav/etc).
    pub location: Option<String>,
    /// Optional EPUB spec reference label.
    pub spec_ref: Option<&'static str>,
    /// Optional remediation hint.
    pub hint: Option<String>,
}

impl ValidationDiagnostic {
    fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: ValidationSeverity::Error,
            message: message.into(),
            path: None,
            location: None,
            spec_ref: None,
            hint: None,
        }
    }

    fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: ValidationSeverity::Warning,
            message: message.into(),
            path: None,
            location: None,
            spec_ref: None,
            hint: None,
        }
    }
}

/// Validation report with all discovered diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationReport {
    diagnostics: Vec<ValidationDiagnostic>,
}

impl ValidationReport {
    /// Create an empty report.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return all collected diagnostics.
    pub fn diagnostics(&self) -> &[ValidationDiagnostic] {
        &self.diagnostics
    }

    /// Number of error diagnostics.
    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == ValidationSeverity::Error)
            .count()
    }

    /// Number of warning diagnostics.
    pub fn warning_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == ValidationSeverity::Warning)
            .count()
    }

    /// Returns `true` when no error-level diagnostics were found.
    pub fn is_valid(&self) -> bool {
        self.error_count() == 0
    }

    fn push(&mut self, diagnostic: ValidationDiagnostic) {
        self.diagnostics.push(diagnostic);
    }
}

/// Options for validation runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationOptions {
    /// Optional ZIP safety limits used while reading archive entries.
    pub zip_limits: Option<ZipLimits>,
}

/// Validate an EPUB from a filesystem path.
pub fn validate_epub_file<P: AsRef<Path>>(path: P) -> Result<ValidationReport, crate::EpubError> {
    validate_epub_file_with_options(path, ValidationOptions::default())
}

/// Validate an EPUB from a filesystem path with explicit options.
pub fn validate_epub_file_with_options<P: AsRef<Path>>(
    path: P,
    options: ValidationOptions,
) -> Result<ValidationReport, crate::EpubError> {
    let file = File::open(path).map_err(|e| crate::EpubError::Io(e.to_string()))?;
    Ok(validate_epub_reader_with_options(file, options))
}

/// Validate an EPUB from any `Read + Seek` reader.
pub fn validate_epub_reader<R: Read + Seek>(reader: R) -> ValidationReport {
    validate_epub_reader_with_options(reader, ValidationOptions::default())
}

/// Validate an EPUB from any `Read + Seek` reader with explicit options.
pub fn validate_epub_reader_with_options<R: Read + Seek>(
    reader: R,
    options: ValidationOptions,
) -> ValidationReport {
    let mut report = ValidationReport::new();
    let mut zip = match StreamingZip::new_with_limits(reader, options.zip_limits) {
        Ok(zip) => zip,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "ZIP_INVALID_ARCHIVE",
                format!("Failed to parse ZIP container: {}", err),
            );
            d.spec_ref = Some("OCF ZIP container");
            report.push(d);
            return report;
        }
    };

    if let Err(err) = zip.validate_mimetype() {
        let mut d = ValidationDiagnostic::error(
            "OCF_INVALID_MIMETYPE",
            format!("Invalid or missing mimetype entry: {}", err),
        );
        d.path = Some("mimetype".to_string());
        d.spec_ref = Some("OCF mimetype");
        d.hint = Some("Ensure `mimetype` exists and equals `application/epub+zip`.".to_string());
        report.push(d);
    }

    let container_entry = match zip.get_entry("META-INF/container.xml").cloned() {
        Some(entry) => entry,
        None => {
            let mut d = ValidationDiagnostic::error(
                "OCF_CONTAINER_XML_MISSING",
                "Missing required `META-INF/container.xml`.",
            );
            d.path = Some("META-INF/container.xml".to_string());
            d.spec_ref = Some("OCF container.xml");
            report.push(d);
            return report;
        }
    };

    let container_xml = match read_entry(&mut zip, container_entry.local_header_offset) {
        Ok(bytes) => bytes,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "OCF_CONTAINER_XML_UNREADABLE",
                format!("Failed to read `container.xml`: {}", err),
            );
            d.path = Some("META-INF/container.xml".to_string());
            report.push(d);
            return report;
        }
    };

    let opf_path = match parse_container_xml(&container_xml) {
        Ok(path) => path,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "OPF_ROOTFILE_MISSING",
                format!("container.xml does not declare a usable rootfile: {}", err),
            );
            d.path = Some("META-INF/container.xml".to_string());
            d.spec_ref = Some("EPUB package document discovery");
            report.push(d);
            return report;
        }
    };

    let opf_entry = match zip.get_entry(&opf_path).cloned() {
        Some(entry) => entry,
        None => {
            let mut d = ValidationDiagnostic::error(
                "OPF_FILE_MISSING",
                format!(
                    "Rootfile path '{}' from container.xml is missing.",
                    opf_path
                ),
            );
            d.path = Some(opf_path.clone());
            d.spec_ref = Some("Package document");
            report.push(d);
            return report;
        }
    };

    let opf_bytes = match read_entry(&mut zip, opf_entry.local_header_offset) {
        Ok(bytes) => bytes,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "OPF_FILE_UNREADABLE",
                format!("Failed to read package document '{}': {}", opf_path, err),
            );
            d.path = Some(opf_path.clone());
            report.push(d);
            return report;
        }
    };

    let metadata = match parse_opf(&opf_bytes) {
        Ok(metadata) => metadata,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "OPF_PARSE_ERROR",
                format!("Failed to parse package document '{}': {}", opf_path, err),
            );
            d.path = Some(opf_path.clone());
            d.spec_ref = Some("OPF package document");
            report.push(d);
            return report;
        }
    };

    let spine = match crate::spine::parse_spine(&opf_bytes) {
        Ok(spine) => spine,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                "SPINE_PARSE_ERROR",
                format!("Failed to parse `<spine>` in '{}': {}", opf_path, err),
            );
            d.path = Some(opf_path.clone());
            d.location = Some("spine".to_string());
            report.push(d);
            return report;
        }
    };

    validate_manifest_integrity(&metadata, &mut report);
    validate_manifest_fallbacks(&opf_bytes, &mut report);
    validate_manifest_resources_exist(&zip, &metadata, &opf_path, &mut report);
    validate_spine_integrity(&metadata, &spine, &mut report);
    validate_navigation_integrity(&mut zip, &metadata, &spine, &opf_path, &mut report);
    validate_container_sidecars(&mut zip, &mut report);

    report
}

#[derive(Clone, Debug)]
struct OpfManifestAttrs {
    id: String,
    href: String,
    media_type: String,
    fallback: Option<String>,
}

fn parse_opf_manifest_attrs(opf_bytes: &[u8]) -> Result<Vec<OpfManifestAttrs>, String> {
    let mut reader = Reader::from_reader(opf_bytes);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(8);
    let mut in_manifest = false;
    let mut out = Vec::with_capacity(8);

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| format!("decode error: {:?}", e))?
                    .to_string();
                if name == "manifest" {
                    in_manifest = true;
                } else if in_manifest && name == "item" {
                    if let Some(attrs) = parse_manifest_item_attrs(&reader, &e)? {
                        out.push(attrs);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| format!("decode error: {:?}", e))?
                    .to_string();
                if in_manifest && name == "item" {
                    if let Some(attrs) = parse_manifest_item_attrs(&reader, &e)? {
                        out.push(attrs);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = reader
                    .decoder()
                    .decode(e.name().as_ref())
                    .map_err(|e| format!("decode error: {:?}", e))?
                    .to_string();
                if name == "manifest" {
                    in_manifest = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error: {:?}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

fn parse_manifest_item_attrs(
    reader: &Reader<&[u8]>,
    e: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<OpfManifestAttrs>, String> {
    let mut id = None;
    let mut href = None;
    let mut media_type = None;
    let mut fallback = None;

    for attr in e.attributes() {
        let attr = attr.map_err(|err| format!("attr error: {:?}", err))?;
        let key = reader
            .decoder()
            .decode(attr.key.as_ref())
            .map_err(|err| format!("decode error: {:?}", err))?;
        let value = reader
            .decoder()
            .decode(&attr.value)
            .map_err(|err| format!("decode error: {:?}", err))?
            .to_string();
        match key.as_ref() {
            "id" => id = Some(value),
            "href" => href = Some(value),
            "media-type" => media_type = Some(value),
            "fallback" => fallback = Some(value),
            _ => {}
        }
    }

    match (id, href, media_type) {
        (Some(id), Some(href), Some(media_type)) => Ok(Some(OpfManifestAttrs {
            id,
            href,
            media_type,
            fallback,
        })),
        _ => Ok(None),
    }
}

fn is_epub_core_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/xhtml+xml"
            | "application/x-dtbncx+xml"
            | "text/css"
            | "image/gif"
            | "image/jpeg"
            | "image/png"
            | "image/svg+xml"
            | "font/otf"
            | "font/ttf"
            | "font/woff"
            | "font/woff2"
            | "application/vnd.ms-opentype"
            | "audio/mpeg"
            | "audio/mp4"
            | "video/mp4"
    )
}

fn validate_manifest_fallbacks(opf_bytes: &[u8], report: &mut ValidationReport) {
    let items = match parse_opf_manifest_attrs(opf_bytes) {
        Ok(items) => items,
        Err(err) => {
            let mut d = ValidationDiagnostic::warning(
                "OPF_MANIFEST_PARSE_PARTIAL",
                format!("Could not analyze manifest fallback attributes: {}", err),
            );
            d.location = Some("manifest".to_string());
            report.push(d);
            return;
        }
    };

    let by_id: BTreeMap<&str, &OpfManifestAttrs> =
        items.iter().map(|item| (item.id.as_str(), item)).collect();

    for item in &items {
        if !is_epub_core_media_type(&item.media_type) && item.fallback.is_none() {
            let mut d = ValidationDiagnostic::warning(
                "MANIFEST_FOREIGN_NO_FALLBACK",
                format!(
                    "Manifest item '{}' has non-core media-type '{}' without fallback.",
                    item.id, item.media_type
                ),
            );
            d.location = Some("manifest".to_string());
            d.path = Some(item.href.clone());
            d.hint = Some(
                "Add `fallback=\"...\"` to a supported content-document representation."
                    .to_string(),
            );
            report.push(d);
        }

        if let Some(fallback_id) = item.fallback.as_deref() {
            if fallback_id == item.id {
                let mut d = ValidationDiagnostic::error(
                    "MANIFEST_FALLBACK_SELF_REFERENCE",
                    format!("Manifest item '{}' fallback points to itself.", item.id),
                );
                d.location = Some("manifest".to_string());
                d.path = Some(item.href.clone());
                report.push(d);
                continue;
            }

            if !by_id.contains_key(fallback_id) {
                let mut d = ValidationDiagnostic::error(
                    "MANIFEST_FALLBACK_TARGET_MISSING",
                    format!(
                        "Manifest item '{}' fallback references missing id '{}'.",
                        item.id, fallback_id
                    ),
                );
                d.location = Some("manifest".to_string());
                d.path = Some(item.href.clone());
                report.push(d);
                continue;
            }

            let mut seen = BTreeSet::new();
            let mut cursor = fallback_id;
            while let Some(next) = by_id
                .get(cursor)
                .and_then(|entry| entry.fallback.as_deref())
            {
                if !seen.insert(cursor) {
                    let mut d = ValidationDiagnostic::error(
                        "MANIFEST_FALLBACK_CYCLE",
                        format!(
                            "Fallback chain from '{}' contains a cycle at id '{}'.",
                            item.id, cursor
                        ),
                    );
                    d.location = Some("manifest".to_string());
                    d.path = Some(item.href.clone());
                    report.push(d);
                    break;
                }
                cursor = next;
            }
        }
    }
}

fn validate_container_sidecars<F: Read + Seek>(
    zip: &mut StreamingZip<F>,
    report: &mut ValidationReport,
) {
    validate_optional_xml_sidecar(
        zip,
        report,
        "META-INF/encryption.xml",
        "ENCRYPTION_XML_UNREADABLE",
        "ENCRYPTION_XML_PARSE_ERROR",
    );
    validate_optional_xml_sidecar(
        zip,
        report,
        "META-INF/rights.xml",
        "RIGHTS_XML_UNREADABLE",
        "RIGHTS_XML_PARSE_ERROR",
    );
    validate_encryption_references(zip, report);
}

fn validate_optional_xml_sidecar<F: Read + Seek>(
    zip: &mut StreamingZip<F>,
    report: &mut ValidationReport,
    path: &str,
    unreadable_code: &'static str,
    parse_code: &'static str,
) {
    let Some(entry) = zip.get_entry(path).cloned() else {
        return;
    };
    let bytes = match read_entry(zip, entry.local_header_offset) {
        Ok(bytes) => bytes,
        Err(err) => {
            let mut d = ValidationDiagnostic::error(
                unreadable_code,
                format!("Failed to read '{}': {}", path, err),
            );
            d.path = Some(path.to_string());
            d.location = Some("ocf".to_string());
            report.push(d);
            return;
        }
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(8);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(err) => {
                let mut d = ValidationDiagnostic::error(
                    parse_code,
                    format!("Failed to parse '{}': {:?}", path, err),
                );
                d.path = Some(path.to_string());
                d.location = Some("ocf".to_string());
                report.push(d);
                return;
            }
        }
        buf.clear();
    }
}

fn validate_encryption_references<F: Read + Seek>(
    zip: &mut StreamingZip<F>,
    report: &mut ValidationReport,
) {
    let Some(entry) = zip.get_entry("META-INF/encryption.xml").cloned() else {
        return;
    };
    let bytes = match read_entry(zip, entry.local_header_offset) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };

    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().trim_text(true);
    let mut buf = Vec::with_capacity(8);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let tag = match reader.decoder().decode(e.name().as_ref()) {
                    Ok(v) => v.to_string(),
                    Err(_) => {
                        buf.clear();
                        continue;
                    }
                };
                if !tag.ends_with("CipherReference") {
                    buf.clear();
                    continue;
                }
                for attr in e.attributes().flatten() {
                    let key = match reader.decoder().decode(attr.key.as_ref()) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if key != "URI" {
                        continue;
                    }
                    let uri = match reader.decoder().decode(&attr.value) {
                        Ok(v) => v.to_string(),
                        Err(_) => continue,
                    };
                    if uri.contains("://") || uri.starts_with('/') || uri.trim().is_empty() {
                        continue;
                    }
                    let full_path = resolve_opf_relative("META-INF/encryption.xml", &uri);
                    if zip.get_entry(&full_path).is_none() {
                        let mut d = ValidationDiagnostic::error(
                            "ENCRYPTION_REFERENCE_MISSING",
                            format!(
                                "`encryption.xml` references missing encrypted resource '{}'.",
                                full_path
                            ),
                        );
                        d.location = Some("ocf".to_string());
                        d.path = Some("META-INF/encryption.xml".to_string());
                        report.push(d);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        buf.clear();
    }
}

fn read_entry<F: Read + Seek>(
    zip: &mut StreamingZip<F>,
    local_header_offset: u64,
) -> Result<Vec<u8>, crate::ZipError> {
    let entry = zip
        .entries()
        .find(|e| e.local_header_offset == local_header_offset)
        .ok_or(crate::ZipError::FileNotFound)?
        .clone();
    let size =
        usize::try_from(entry.uncompressed_size).map_err(|_| crate::ZipError::FileTooLarge)?;
    let mut buf = vec![0u8; size];
    let n = zip.read_file_at_offset(local_header_offset, &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

fn validate_manifest_integrity(metadata: &EpubMetadata, report: &mut ValidationReport) {
    let mut ids = BTreeSet::new();
    let mut hrefs = BTreeSet::new();
    for item in &metadata.manifest {
        if item.id.trim().is_empty() {
            let mut d = ValidationDiagnostic::error(
                "MANIFEST_ID_EMPTY",
                "Manifest item has empty `id` attribute.",
            );
            d.location = Some("manifest".to_string());
            d.path = Some(item.href.clone());
            report.push(d);
        }
        if item.href.trim().is_empty() {
            let mut d = ValidationDiagnostic::error(
                "MANIFEST_HREF_EMPTY",
                format!("Manifest item '{}' has empty `href`.", item.id),
            );
            d.location = Some("manifest".to_string());
            report.push(d);
        }
        if item.media_type.trim().is_empty() {
            let mut d = ValidationDiagnostic::error(
                "MANIFEST_MEDIA_TYPE_EMPTY",
                format!("Manifest item '{}' has empty `media-type`.", item.id),
            );
            d.location = Some("manifest".to_string());
            d.path = Some(item.href.clone());
            report.push(d);
        }

        if !ids.insert(item.id.clone()) {
            let mut d = ValidationDiagnostic::error(
                "MANIFEST_ID_DUPLICATE",
                format!("Duplicate manifest id '{}'.", item.id),
            );
            d.location = Some("manifest".to_string());
            report.push(d);
        }

        let href_key = item.href.to_ascii_lowercase();
        if !href_key.is_empty() && !hrefs.insert(href_key) {
            let mut d = ValidationDiagnostic::warning(
                "MANIFEST_HREF_DUPLICATE",
                format!("Multiple manifest items reference href '{}'.", item.href),
            );
            d.location = Some("manifest".to_string());
            d.path = Some(item.href.clone());
            report.push(d);
        }
    }
}

fn validate_manifest_resources_exist<F: Read + Seek>(
    zip: &StreamingZip<F>,
    metadata: &EpubMetadata,
    opf_path: &str,
    report: &mut ValidationReport,
) {
    for item in &metadata.manifest {
        if item.href.contains("://") || item.href.trim().is_empty() {
            continue;
        }
        let full_path = resolve_opf_relative(opf_path, &item.href);
        if zip.get_entry(&full_path).is_none() {
            let mut d = ValidationDiagnostic::error(
                "MANIFEST_RESOURCE_MISSING",
                format!(
                    "Manifest item '{}' points to missing resource '{}'.",
                    item.id, full_path
                ),
            );
            d.location = Some("manifest".to_string());
            d.path = Some(full_path);
            report.push(d);
        }
    }
}

fn validate_spine_integrity(metadata: &EpubMetadata, spine: &Spine, report: &mut ValidationReport) {
    if spine.is_empty() {
        let mut d =
            ValidationDiagnostic::warning("SPINE_EMPTY", "Spine has no reading-order entries.");
        d.location = Some("spine".to_string());
        report.push(d);
    }

    for (index, item) in spine.items().iter().enumerate() {
        if let Some(manifest_item) = metadata.get_item(&item.idref) {
            if manifest_item.media_type != "application/xhtml+xml" {
                let mut d = ValidationDiagnostic::warning(
                    "SPINE_ITEM_NON_XHTML",
                    format!(
                        "Spine item '{}' references media-type '{}' (expected application/xhtml+xml).",
                        item.idref, manifest_item.media_type
                    ),
                );
                d.location = Some("spine".to_string());
                d.path = Some(manifest_item.href.clone());
                report.push(d);
            }
        } else {
            let mut d = ValidationDiagnostic::error(
                "SPINE_IDREF_NOT_IN_MANIFEST",
                format!(
                    "Spine item at index {} references unknown manifest id '{}'.",
                    index, item.idref
                ),
            );
            d.location = Some("spine".to_string());
            d.spec_ref = Some("OPF spine/itemref");
            d.hint = Some(
                "Ensure each `<itemref idref=\"...\">` matches a manifest `<item id=\"...\">`."
                    .to_string(),
            );
            report.push(d);
        }
    }
}

fn validate_navigation_integrity<F: Read + Seek>(
    zip: &mut StreamingZip<F>,
    metadata: &EpubMetadata,
    spine: &Spine,
    opf_path: &str,
    report: &mut ValidationReport,
) {
    let nav_item = metadata
        .manifest
        .iter()
        .find(|item| item.properties.as_deref().unwrap_or("").contains("nav"));

    if let Some(nav_item) = nav_item {
        if nav_item.media_type != "application/xhtml+xml"
            && nav_item.media_type != "application/x-dtbncx+xml"
        {
            let mut d = ValidationDiagnostic::error(
                "NAV_DOCUMENT_MEDIA_TYPE_INVALID",
                format!(
                    "Navigation item '{}' has unexpected media-type '{}'.",
                    nav_item.id, nav_item.media_type
                ),
            );
            d.path = Some(nav_item.href.clone());
            d.location = Some("navigation".to_string());
            report.push(d);
        }
        let full_path = resolve_opf_relative(opf_path, &nav_item.href);
        let nav_entry = match zip.get_entry(&full_path).cloned() {
            Some(entry) => entry,
            None => {
                let mut d = ValidationDiagnostic::error(
                    "NAV_DOCUMENT_MISSING",
                    format!("Manifest nav item points to missing file '{}'.", full_path),
                );
                d.path = Some(full_path);
                d.location = Some("navigation".to_string());
                report.push(d);
                return;
            }
        };

        match read_entry(zip, nav_entry.local_header_offset) {
            Ok(bytes) => {
                if let Err(err) = parse_nav_xhtml(&bytes) {
                    let mut d = ValidationDiagnostic::error(
                        "NAV_DOCUMENT_PARSE_ERROR",
                        format!("Failed to parse nav document: {}", err),
                    );
                    d.path = Some(full_path);
                    d.location = Some("navigation".to_string());
                    report.push(d);
                }
            }
            Err(err) => {
                let mut d = ValidationDiagnostic::error(
                    "NAV_DOCUMENT_UNREADABLE",
                    format!("Failed to read nav document: {}", err),
                );
                d.path = Some(full_path);
                d.location = Some("navigation".to_string());
                report.push(d);
            }
        }
        return;
    }

    if let Some(toc_id) = spine.toc_id() {
        let ncx_item = metadata.get_item(toc_id);
        match ncx_item {
            Some(item) => {
                let full_path = resolve_opf_relative(opf_path, &item.href);
                match zip.get_entry(&full_path).cloned() {
                    Some(entry) => match read_entry(zip, entry.local_header_offset) {
                        Ok(bytes) => {
                            if let Err(err) = parse_ncx(&bytes) {
                                let mut d = ValidationDiagnostic::error(
                                    "NCX_PARSE_ERROR",
                                    format!("Failed to parse NCX document: {}", err),
                                );
                                d.path = Some(full_path);
                                d.location = Some("navigation".to_string());
                                report.push(d);
                            }
                        }
                        Err(err) => {
                            let mut d = ValidationDiagnostic::error(
                                "NCX_UNREADABLE",
                                format!("Failed to read NCX document: {}", err),
                            );
                            d.path = Some(full_path);
                            d.location = Some("navigation".to_string());
                            report.push(d);
                        }
                    },
                    None => {
                        let mut d = ValidationDiagnostic::error(
                            "NCX_MISSING",
                            format!(
                                "Spine `toc` references '{}' but resolved path '{}' is missing.",
                                toc_id, full_path
                            ),
                        );
                        d.path = Some(full_path);
                        d.location = Some("navigation".to_string());
                        report.push(d);
                    }
                }
            }
            None => {
                let mut d = ValidationDiagnostic::error(
                    "NCX_IDREF_NOT_IN_MANIFEST",
                    format!("Spine `toc` references unknown manifest id '{}'.", toc_id),
                );
                d.location = Some("spine".to_string());
                report.push(d);
            }
        }
        return;
    }

    let mut d = ValidationDiagnostic::warning(
        "NAV_MISSING",
        "No EPUB3 nav document and no EPUB2 NCX reference found.",
    );
    d.location = Some("navigation".to_string());
    d.hint = Some(
        "Add a manifest nav item (`properties=\"nav\"`) or spine `toc` NCX fallback.".to_string(),
    );
    report.push(d);
}

fn resolve_opf_relative(opf_path: &str, href: &str) -> String {
    if href.contains("://") || href.starts_with('/') {
        return href.to_string();
    }
    match opf_path.rfind('/') {
        Some(idx) => format!("{}/{}", &opf_path[..idx], href),
        None => href.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIG_LOCAL_FILE_HEADER: u32 = 0x04034b50;
    const SIG_CD_ENTRY: u32 = 0x02014b50;
    const SIG_EOCD: u32 = 0x06054b50;

    fn build_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        struct FileMeta {
            name: String,
            local_offset: u32,
            crc32: u32,
            size: u32,
        }

        let mut zip = Vec::with_capacity(8);
        let mut metas = Vec::with_capacity(8);

        for (name, content) in files {
            let name_bytes = name.as_bytes();
            let name_len = name_bytes.len() as u16;
            let content_len = content.len() as u32;
            let crc = crc32fast::hash(content);
            let local_offset = zip.len() as u32;

            zip.extend_from_slice(&SIG_LOCAL_FILE_HEADER.to_le_bytes());
            zip.extend_from_slice(&20u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes()); // stored
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&crc.to_le_bytes());
            zip.extend_from_slice(&content_len.to_le_bytes());
            zip.extend_from_slice(&content_len.to_le_bytes());
            zip.extend_from_slice(&name_len.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(name_bytes);
            zip.extend_from_slice(content);

            metas.push(FileMeta {
                name: (*name).to_string(),
                local_offset,
                crc32: crc,
                size: content_len,
            });
        }

        let cd_offset = zip.len() as u32;
        for meta in &metas {
            let name_bytes = meta.name.as_bytes();
            let name_len = name_bytes.len() as u16;
            zip.extend_from_slice(&SIG_CD_ENTRY.to_le_bytes());
            zip.extend_from_slice(&20u16.to_le_bytes());
            zip.extend_from_slice(&20u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes()); // stored
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&meta.crc32.to_le_bytes());
            zip.extend_from_slice(&meta.size.to_le_bytes());
            zip.extend_from_slice(&meta.size.to_le_bytes());
            zip.extend_from_slice(&name_len.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u16.to_le_bytes());
            zip.extend_from_slice(&0u32.to_le_bytes());
            zip.extend_from_slice(&meta.local_offset.to_le_bytes());
            zip.extend_from_slice(name_bytes);
        }

        let cd_size = (zip.len() as u32) - cd_offset;
        let entries = metas.len() as u16;

        zip.extend_from_slice(&SIG_EOCD.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());
        zip.extend_from_slice(&entries.to_le_bytes());
        zip.extend_from_slice(&entries.to_le_bytes());
        zip.extend_from_slice(&cd_size.to_le_bytes());
        zip.extend_from_slice(&cd_offset.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes());

        zip
    }

    fn minimal_valid_epub_zip() -> Vec<u8> {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Tester</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

        let nav = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body>
    <nav epub:type="toc">
      <ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol>
    </nav>
  </body>
</html>"#;

        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#;

        build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/ch1.xhtml", ch1),
        ])
    }

    #[test]
    fn validate_minimal_valid_epub() {
        let data = minimal_valid_epub_zip();
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report.is_valid(), "expected valid report: {:?}", report);
        assert_eq!(report.error_count(), 0);
    }

    #[test]
    fn validate_detects_missing_container() {
        let data = build_zip(&[("mimetype", b"application/epub+zip")]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(!report.is_valid());
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "OCF_CONTAINER_XML_MISSING"));
    }

    #[test]
    fn validate_detects_spine_manifest_mismatch() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title><dc:creator>A</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="only" href="only.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="missing"/>
  </spine>
</package>"#;

        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
            ("EPUB/only.xhtml", b"<html/>"),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(!report.is_valid());
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "SPINE_IDREF_NOT_IN_MANIFEST"));
    }

    #[test]
    fn validate_detects_missing_manifest_resource() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title><dc:creator>A</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="missing.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "MANIFEST_RESOURCE_MISSING"));
    }

    #[test]
    fn validate_warns_on_non_xhtml_spine_item() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title><dc:creator>A</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="ch1.txt" media-type="text/plain"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
            ("EPUB/ch1.txt", b"hello"),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report.warning_count() > 0);
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "SPINE_ITEM_NON_XHTML"));
    }

    #[test]
    fn validate_detects_missing_manifest_fallback_target() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title><dc:creator>A</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="scripted" href="script.js" media-type="text/javascript" fallback="missing"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

        let nav = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol></nav></body>
</html>"#;

        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
            ("EPUB/ch1.xhtml", b"<html/>"),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/script.js", b"alert('x');"),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "MANIFEST_FALLBACK_TARGET_MISSING"));
    }

    #[test]
    fn validate_warns_on_foreign_resource_without_fallback() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title><dc:creator>A</dc:creator><dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="foreign" href="script.js" media-type="text/javascript"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

        let nav = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol></nav></body>
</html>"#;

        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            ("META-INF/container.xml", container_xml),
            ("EPUB/package.opf", opf),
            ("EPUB/ch1.xhtml", b"<html/>"),
            ("EPUB/nav.xhtml", nav),
            ("EPUB/script.js", b"alert('x');"),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "MANIFEST_FOREIGN_NO_FALLBACK"));
    }

    #[test]
    fn validate_detects_missing_encryption_cipher_reference_target() {
        let encryption_xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"
  xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
  <enc:EncryptedData>
    <enc:CipherData>
      <enc:CipherReference URI="../EPUB/missing-font.otf"/>
    </enc:CipherData>
  </enc:EncryptedData>
</encryption>"#;
        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
            ),
            (
                "EPUB/package.opf",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Tester</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
            ),
            (
                "EPUB/nav.xhtml",
                br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol></nav></body>
</html>"#,
            ),
            (
                "EPUB/ch1.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#,
            ),
            ("META-INF/encryption.xml", encryption_xml),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "ENCRYPTION_REFERENCE_MISSING"));
    }

    #[test]
    fn validate_detects_invalid_rights_xml() {
        let data = build_zip(&[
            ("mimetype", b"application/epub+zip"),
            (
                "META-INF/container.xml",
                br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="EPUB/package.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
            ),
            (
                "EPUB/package.opf",
                br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Tester</dc:creator>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#,
            ),
            (
                "EPUB/nav.xhtml",
                br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
  <body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">Chapter 1</a></li></ol></nav></body>
</html>"#,
            ),
            (
                "EPUB/ch1.xhtml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#,
            ),
            ("META-INF/rights.xml", b"<rights><broken></rights>"),
        ]);
        let report = validate_epub_reader(std::io::Cursor::new(data));
        assert!(report
            .diagnostics()
            .iter()
            .any(|d| d.code == "RIGHTS_XML_PARSE_ERROR"));
    }
}
