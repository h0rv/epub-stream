//! Integration tests for epub-stream
//!
//! Tests marked #[ignore] require a sample EPUB in tests/fixtures/.
//! Run all:    cargo test --all-features -- --include-ignored
//! Run fast:   cargo test --all-features

use std::fs::File;

use epub_stream::book::{EpubBook, ValidationMode};
#[cfg(feature = "layout")]
use epub_stream::layout::LayoutEngine;
use epub_stream::metadata::parse_opf;
use epub_stream::spine::parse_spine;
use epub_stream::tokenizer::{tokenize_html, Token};
use epub_stream::zip::StreamingZip;

const SAMPLE_EPUB_PATH: &str =
    "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub";

// -- ZIP tests ----------------------------------------------------------------

#[test]
#[ignore]
fn test_zip_open_sample_epub() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    assert!(zip.get_entry("META-INF/container.xml").is_some());
    assert!(zip.num_entries() > 0);
}

#[test]
#[ignore]
fn test_zip_read_container() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("META-INF/container.xml")
        .expect("container.xml not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    assert!(bytes_read > 0);

    let content = String::from_utf8_lossy(&buf[..bytes_read]);
    assert!(content.contains("container"));
    assert!(content.contains("rootfile"));
    assert!(content.contains("EPUB/package.opf"));
}

#[test]
#[ignore]
fn test_zip_list_entries() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entries: Vec<_> = zip.entries().collect();
    assert!(!entries.is_empty());

    let filenames: Vec<_> = entries.iter().map(|e| e.filename.as_str()).collect();
    assert!(filenames.contains(&"META-INF/container.xml"));
    assert!(filenames.contains(&"mimetype"));
}

#[test]
#[ignore]
fn test_zip_read_package_opf() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    let content = String::from_utf8_lossy(&buf[..bytes_read]);
    assert!(content.contains("<package"));
    assert!(content.contains("<metadata"));
    assert!(content.contains("<manifest"));
}

#[test]
#[ignore]
fn test_zip_entry_not_found() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    assert!(zip.get_entry("nonexistent/file.txt").is_none());
}

// -- Metadata tests -----------------------------------------------------------

#[test]
#[ignore]
fn test_parse_opf_from_sample() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    let metadata = parse_opf(&buf[..bytes_read]).expect("Failed to parse OPF");

    assert!(!metadata.title.is_empty());
    assert!(!metadata.author.is_empty());
    assert!(!metadata.manifest.is_empty());
}

#[test]
#[ignore]
fn test_manifest_lookup() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    let metadata = parse_opf(&buf[..bytes_read]).expect("Failed to parse OPF");

    let cover_item = metadata.get_item("cover");
    assert!(cover_item.is_some());

    if let Some(item) = cover_item {
        assert!(item.href.ends_with(".xhtml") || item.href.ends_with(".html"));
        assert_eq!(item.media_type, "application/xhtml+xml");
    }

    assert!(metadata.get_item("nonexistent").is_none());
}

#[test]
#[ignore]
fn test_manifest_items_have_valid_properties() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    let metadata = parse_opf(&buf[..bytes_read]).expect("Failed to parse OPF");

    for item in &metadata.manifest {
        assert!(!item.id.is_empty());
        assert!(!item.href.is_empty());
        assert!(!item.media_type.is_empty());
    }
}

#[test]
#[ignore]
fn test_find_item_by_href() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();

    let mut buf = vec![0u8; entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&entry, &mut buf)
        .expect("Failed to read file");

    let metadata = parse_opf(&buf[..bytes_read]).expect("Failed to parse OPF");

    if let Some(first_item) = metadata.manifest.first() {
        let found_id = metadata.find_item_by_href(&first_item.href);
        assert!(found_id.is_some());
        assert_eq!(found_id.unwrap(), first_item.id);
    }

    assert!(metadata.find_item_by_href("nonexistent.xhtml").is_none());
}

// -- Tokenizer tests ----------------------------------------------------------

#[test]
fn test_tokenize_simple_html() {
    let html = "<p>Hello <em>world</em></p>";
    let tokens = tokenize_html(html).expect("Failed to tokenize");

    // Whitespace normalization strips the trailing space from "Hello "
    assert_eq!(tokens.len(), 4);
    assert!(matches!(tokens[0], Token::Text(ref t) if t == "Hello"));
    assert_eq!(tokens[1], Token::Emphasis(true));
    assert!(matches!(tokens[2], Token::Text(ref t) if t == "world"));
    assert_eq!(tokens[3], Token::Emphasis(false));
}

#[test]
#[ignore]
fn test_tokenize_chapter_from_sample() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let chapter_entry = zip
        .entries()
        .find(|e| e.filename.contains("Basic-functionality-tests.xhtml"))
        .expect("Chapter not found")
        .clone();

    let mut buf = vec![0u8; chapter_entry.uncompressed_size as usize];
    let bytes_read = zip
        .read_file(&chapter_entry, &mut buf)
        .expect("Failed to read file");

    let html = String::from_utf8_lossy(&buf[..bytes_read]);
    let tokens = tokenize_html(&html).expect("Failed to tokenize chapter");

    assert!(!tokens.is_empty());

    let text_tokens: Vec<_> = tokens
        .iter()
        .filter(|t| matches!(t, Token::Text(_)))
        .collect();
    assert!(!text_tokens.is_empty());
}

#[test]
fn test_tokenize_with_headings() {
    let html = "<h1>Title</h1><p>Content</p>";
    let tokens = tokenize_html(html).expect("Failed to tokenize");

    assert!(tokens.iter().any(|t| matches!(t, Token::Heading(1))));
    assert!(tokens
        .iter()
        .any(|t| matches!(t, Token::Text(ref txt) if txt == "Title")));
    assert!(tokens
        .iter()
        .any(|t| matches!(t, Token::Text(ref txt) if txt == "Content")));
}

#[test]
fn test_tokenize_complex_formatting() {
    let html = r#"<p>Normal <strong>bold <em>bold+italic</em> bold</strong> normal</p>"#;
    let tokens = tokenize_html(html).expect("Failed to tokenize");

    let mut found_bold_start = false;
    let mut found_italic_start = false;
    let mut found_italic_end = false;
    let mut found_bold_end = false;

    for token in &tokens {
        match token {
            Token::Strong(true) => found_bold_start = true,
            Token::Emphasis(true) => found_italic_start = true,
            Token::Emphasis(false) => found_italic_end = true,
            Token::Strong(false) => found_bold_end = true,
            _ => {}
        }
    }

    assert!(found_bold_start);
    assert!(found_italic_start);
    assert!(found_italic_end);
    assert!(found_bold_end);
}

// -- Layout tests -------------------------------------------------------------

#[cfg(feature = "layout")]
#[test]
fn test_layout_single_page() {
    let tokens = vec![
        Token::Text("Short text.".to_string()),
        Token::ParagraphBreak,
    ];

    let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].page_number, 1);
    assert!(!pages[0].is_empty());
}

#[cfg(feature = "layout")]
#[test]
fn test_pagination() {
    let mut tokens = Vec::new();
    for i in 0..100 {
        tokens.push(Token::Text(format!(
            "This is paragraph {} with enough text to fill some space.",
            i
        )));
        tokens.push(Token::Text(
            "Here is additional text to make the paragraph longer.".to_string(),
        ));
        tokens.push(Token::Text(
            "And even more content to ensure proper pagination testing.".to_string(),
        ));
        tokens.push(Token::ParagraphBreak);
    }

    let mut engine = LayoutEngine::new(460.0, 300.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    assert!(pages.len() > 1);

    for (i, page) in pages.iter().enumerate() {
        assert_eq!(page.page_number, i + 1);
    }
}

#[cfg(feature = "layout")]
#[test]
fn test_layout_with_formatting() {
    let tokens = vec![
        Token::Text("Normal ".to_string()),
        Token::Strong(true),
        Token::Text("bold".to_string()),
        Token::Strong(false),
        Token::Text(" text.".to_string()),
        Token::ParagraphBreak,
    ];

    let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    assert!(!pages.is_empty());
    assert!(!pages[0].is_empty());
}

#[cfg(feature = "layout")]
#[test]
fn test_layout_headings() {
    let tokens = vec![
        Token::Heading(1),
        Token::Text("Chapter Title".to_string()),
        Token::ParagraphBreak,
        Token::Text("Chapter content here.".to_string()),
        Token::ParagraphBreak,
    ];

    let mut engine = LayoutEngine::new(460.0, 650.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    assert!(!pages.is_empty());

    let all_text: String = pages
        .iter()
        .flat_map(|p| &p.lines)
        .map(|l| l.text())
        .collect();

    assert!(all_text.contains("Chapter Title"));
    assert!(all_text.contains("Chapter content"));
}

#[cfg(feature = "layout")]
#[test]
fn test_layout_line_breaking() {
    // Use many short words so the greedy breaker can split between them.
    // (A single long token won't wrap because the breaker doesn't split mid-word.)
    let words: Vec<String> = (0..40).map(|i| format!("word{}", i)).collect();
    let long_text = words.join(" ");
    let tokens = vec![Token::Text(long_text), Token::ParagraphBreak];

    let mut engine = LayoutEngine::new(100.0, 200.0, 20.0);
    let pages = engine.layout_tokens(&tokens);

    assert!(!pages.is_empty());

    let total_lines: usize = pages.iter().map(|p| p.line_count()).sum();
    assert!(total_lines > 1);
}

// -- End-to-end ---------------------------------------------------------------

#[cfg(feature = "layout")]
#[test]
#[ignore]
fn test_epub_full_pipeline() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let opf_entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();
    let mut opf_buf = vec![0u8; opf_entry.uncompressed_size as usize];
    let opf_bytes = zip
        .read_file(&opf_entry, &mut opf_buf)
        .expect("Failed to read OPF");

    let metadata = parse_opf(&opf_buf[..opf_bytes]).expect("Failed to parse OPF");
    assert!(!metadata.title.is_empty());
    assert!(!metadata.manifest.is_empty());

    let spine = parse_spine(&opf_buf[..opf_bytes]).expect("Failed to parse spine");
    assert!(!spine.is_empty());

    if spine.current_id().is_some() {
        // Find a content chapter (skip cover -- it's typically just an image)
        let mut found_content = false;
        for item in spine.items() {
            let manifest_item = match metadata.get_item(item.idref.as_str()) {
                Some(m) => m,
                None => continue,
            };
            let path = format!("EPUB/{}", manifest_item.href);
            let entry = match zip.get_entry(&path) {
                Some(e) => e.clone(),
                None => continue,
            };
            let mut buf = vec![0u8; entry.uncompressed_size as usize];
            let n = zip
                .read_file(&entry, &mut buf)
                .expect("Failed to read chapter");

            let html = String::from_utf8_lossy(&buf[..n]);
            let tokens = tokenize_html(&html).expect("Failed to tokenize");

            if tokens.is_empty() {
                continue; // image-only pages produce no tokens
            }

            let mut engine = LayoutEngine::with_defaults();
            let pages = engine.layout_tokens(&tokens);
            assert!(!pages.is_empty());

            let total_lines: usize = pages.iter().map(|p| p.line_count()).sum();
            assert!(total_lines > 0);
            found_content = true;
            break;
        }
        assert!(found_content, "Should find at least one content chapter");
    }
}

#[test]
#[ignore]
fn test_high_level_book_open_and_tokenize() {
    let mut book = EpubBook::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");

    assert!(!book.metadata().manifest.is_empty());
    assert!(!book.spine().is_empty());

    let tokens = book
        .tokenize_spine_item(0)
        .expect("Failed to tokenize first spine item");
    assert!(!tokens.is_empty());

    let chapter = book.chapter(0).expect("Missing first chapter descriptor");
    assert!(!chapter.idref.is_empty());
    assert!(!chapter.href.is_empty());

    let html = book.chapter_html(0).expect("Failed to decode chapter html");
    assert!(!html.is_empty());

    let has_nonempty_chapter_text = (0..book.chapter_count()).any(|idx| {
        book.chapter_text(idx)
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false)
    });
    assert!(
        has_nonempty_chapter_text,
        "Expected at least one chapter with extracted text content"
    );
}

#[test]
#[ignore]
fn test_high_level_chapter_iteration_and_lookup() {
    let book = EpubBook::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let chapters: Vec<_> = book.chapters().collect();
    assert!(!chapters.is_empty());

    let first = &chapters[0];
    let by_id = book
        .chapter_by_id(&first.idref)
        .expect("chapter_by_id should resolve existing idref");
    assert_eq!(by_id.index, first.index);
    assert_eq!(by_id.href, first.href);
}

#[test]
#[ignore]
fn test_high_level_strict_mode() {
    let book = EpubBook::builder()
        .validation_mode(ValidationMode::Strict)
        .open(SAMPLE_EPUB_PATH)
        .expect("Strict mode should open valid fixture");
    assert!(book.chapter_count() > 0);
}

#[test]
#[ignore]
fn test_spine_navigation_integration() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let opf_entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();
    let mut opf_buf = vec![0u8; opf_entry.uncompressed_size as usize];
    let opf_bytes = zip
        .read_file(&opf_entry, &mut opf_buf)
        .expect("Failed to read OPF");

    let mut spine = parse_spine(&opf_buf[..opf_bytes]).expect("Failed to parse spine");

    assert_eq!(spine.position(), 0);

    let chapter_count = spine.len();
    if chapter_count > 1 {
        assert!(spine.advance());
        assert_eq!(spine.position(), 1);

        assert!(spine.prev());
        assert_eq!(spine.position(), 0);
    }

    let (current, total) = spine.progress();
    assert_eq!(current, 0);
    assert_eq!(total, chapter_count);
}

#[test]
#[ignore]
fn test_cover_detection() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let opf_entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();
    let mut opf_buf = vec![0u8; opf_entry.uncompressed_size as usize];
    let opf_bytes = zip
        .read_file(&opf_entry, &mut opf_buf)
        .expect("Failed to read OPF");

    let metadata = parse_opf(&opf_buf[..opf_bytes]).expect("Failed to parse OPF");

    let cover_items: Vec<_> = metadata
        .manifest
        .iter()
        .filter(|item| {
            item.id.to_lowercase().contains("cover")
                || item
                    .properties
                    .as_ref()
                    .is_some_and(|p| p.contains("cover-image"))
        })
        .collect();

    assert!(!cover_items.is_empty());
}

#[test]
#[ignore]
fn test_spine_manifest_consistency() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let opf_entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();
    let mut opf_buf = vec![0u8; opf_entry.uncompressed_size as usize];
    let opf_bytes = zip
        .read_file(&opf_entry, &mut opf_buf)
        .expect("Failed to read OPF");

    let metadata = parse_opf(&opf_buf[..opf_bytes]).expect("Failed to parse OPF");
    let spine = parse_spine(&opf_buf[..opf_bytes]).expect("Failed to parse spine");

    for item in spine.items() {
        assert!(
            metadata.get_item(&item.idref).is_some(),
            "Spine item '{}' should have corresponding manifest entry",
            item.idref
        );
    }
}

#[test]
#[ignore]
fn test_read_all_chapters() {
    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut zip = StreamingZip::new(file).expect("Failed to parse ZIP");

    let opf_entry = zip
        .get_entry("EPUB/package.opf")
        .expect("package.opf not found")
        .clone();
    let mut opf_buf = vec![0u8; opf_entry.uncompressed_size as usize];
    let opf_bytes = zip
        .read_file(&opf_entry, &mut opf_buf)
        .expect("Failed to read OPF");

    let metadata = parse_opf(&opf_buf[..opf_bytes]).expect("Failed to parse OPF");
    let spine = parse_spine(&opf_buf[..opf_bytes]).expect("Failed to parse spine");

    let mut chapters_read = 0;

    for spine_item in spine.items() {
        if let Some(manifest_item) = metadata.get_item(&spine_item.idref) {
            let chapter_path = format!("EPUB/{}", manifest_item.href);

            if let Some(entry) = zip.get_entry(&chapter_path) {
                let entry = entry.clone();
                let mut buf = vec![0u8; entry.uncompressed_size as usize];
                if zip.read_file(&entry, &mut buf).is_ok() {
                    chapters_read += 1;

                    let content = String::from_utf8_lossy(&buf);
                    assert!(
                        content.contains("<html") || content.contains("<body"),
                        "Chapter should be valid XHTML"
                    );
                }
            }
        }
    }

    assert!(chapters_read > 0);
    assert_eq!(chapters_read, spine.len());
}
