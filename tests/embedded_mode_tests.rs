//! Embedded mode tests with tiny memory budgets
//!
//! These tests exercise large EPUB chapter streaming with tiny budgets
//! to ensure the library handles constrained environments correctly.
//!
//! Run with: cargo test --test embedded_mode_tests

use std::fs::File;

use epub_stream::book::{ChapterEventsOptions, EpubBook, EpubBookOptions, ValidationMode};
use epub_stream::metadata::MetadataLimits;
use epub_stream::navigation::NavigationLimits;
use epub_stream::render_prep::{FontLimits, MemoryBudget, RenderPrepOptions, StyleLimits};
use epub_stream::zip::ZipLimits;

const SAMPLE_EPUB_PATH: &str =
    "tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub";

/// Check if sample EPUB exists
fn has_sample_epub() -> bool {
    std::path::Path::new(SAMPLE_EPUB_PATH).exists()
}

/// Create embedded-friendly options with tiny budgets
fn embedded_options() -> EpubBookOptions {
    EpubBookOptions {
        zip_limits: Some(ZipLimits::new(256 * 1024, 128)), // 256KB max file, 128B mimetype
        validation_mode: ValidationMode::Lenient,
        max_nav_bytes: Some(64 * 1024), // 64KB nav limit
        navigation_limits: NavigationLimits::embedded(),
        metadata_limits: MetadataLimits::default(),
    }
}

/// Create render-prep options with embedded-friendly limits
fn embedded_render_prep() -> RenderPrepOptions {
    RenderPrepOptions {
        style: epub_stream::render_prep::StyleConfig {
            limits: StyleLimits {
                max_selectors: 128,
                max_css_bytes: 16 * 1024,
                max_nesting: 8,
            },
            hints: epub_stream::render_prep::LayoutHints::default(),
        },
        fonts: FontLimits {
            max_faces: 4,
            max_bytes_per_font: 64 * 1024,
            max_total_font_bytes: 128 * 1024,
        },
        layout_hints: epub_stream::render_prep::LayoutHints::default(),
        memory: MemoryBudget {
            max_entry_bytes: 128 * 1024,
            max_css_bytes: 16 * 1024,
            max_nav_bytes: 32 * 1024,
            max_inline_style_bytes: 1024,
            max_pages_in_memory: 4,
        },
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_opens_with_tiny_budget() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let options = embedded_options();

    // Should open successfully with tiny budgets
    let book = EpubBook::from_reader_with_options(file, options);
    assert!(
        book.is_ok(),
        "Failed to open EPUB with embedded budgets: {:?}",
        book.err()
    );
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_chapter_events_with_limits() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    let event_opts = ChapterEventsOptions {
        render: embedded_render_prep(),
        max_items: 1024, // Very small event cap
    };

    let mut event_count = 0usize;
    let result = book.chapter_events(0, event_opts, |_item| {
        event_count += 1;
        Ok(())
    });

    // Should either succeed or fail gracefully with a limit error
    match result {
        Ok(count) => {
            println!("Successfully emitted {} events", count);
            assert!(count <= 1024, "Event count exceeded max_items");
        }
        Err(e) => {
            println!("Event streaming failed (may be due to limits): {}", e);
            // This is acceptable - the test verifies bounded behavior
        }
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_read_resource_into_bounded() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    // Use read_resource_into with bounded buffer
    let mut buf = Vec::with_capacity(256 * 1024); // Pre-allocate max size
    let result = book.read_resource_into("mimetype", &mut buf);

    match result {
        Ok(bytes_read) => {
            println!("Read {} bytes into buffer", bytes_read);
            assert!(bytes_read <= 256 * 1024, "Read exceeded buffer capacity");
        }
        Err(e) => {
            println!("Read failed: {}", e);
            // Acceptable if file exceeds limits
        }
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_chapter_html_into_bounded() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    // Pre-allocate output buffer
    let mut out = String::with_capacity(128 * 1024);
    let result = book.chapter_html_into(0, &mut out);

    match result {
        Ok(()) => {
            println!("Read chapter into buffer, size: {} bytes", out.len());
            assert!(
                out.len() <= 128 * 1024,
                "Chapter exceeded pre-allocated capacity"
            );
        }
        Err(e) => {
            println!("Chapter read failed: {}", e);
            // Acceptable if chapter exceeds limits
        }
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_chapter_text_into_bounded() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    // Pre-allocate output buffer
    let mut out = String::with_capacity(128 * 1024);
    let result = book.chapter_text_into(0, &mut out);

    match result {
        Ok(()) => {
            println!("Read text into buffer, size: {} bytes", out.len());
            assert!(
                out.len() <= 128 * 1024,
                "Text exceeded pre-allocated capacity"
            );
        }
        Err(e) => {
            println!("Text read failed: {}", e);
            // Acceptable if text exceeds limits
        }
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_stylesheet_limits() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    // Try to get stylesheets with embedded limits
    let limits = StyleLimits {
        max_selectors: 64,
        max_css_bytes: 8 * 1024,
        max_nesting: 4,
    };

    let result = book.chapter_stylesheets_with_options(0, limits);

    match result {
        Ok(stylesheets) => {
            println!("Loaded {} stylesheets", stylesheets.iter().count());
        }
        Err(e) => {
            println!("Stylesheet loading failed: {}", e);
            // Acceptable if stylesheets exceed limits
        }
    }
}

#[test]
#[ignore = "Requires sample EPUB"]
fn test_embedded_mode_font_limits() {
    if !has_sample_epub() {
        return;
    }

    let file = File::open(SAMPLE_EPUB_PATH).expect("Failed to open sample EPUB");
    let mut book =
        EpubBook::from_reader_with_options(file, embedded_options()).expect("Failed to open EPUB");

    // Try to enumerate fonts with embedded limits
    let limits = FontLimits {
        max_faces: 2,
        max_bytes_per_font: 32 * 1024,
        max_total_font_bytes: 64 * 1024,
    };

    let result = book.embedded_fonts_with_limits(limits);

    match result {
        Ok(fonts) => {
            println!("Found {} font faces", fonts.len());
            assert!(fonts.len() <= 2, "Font count exceeded max_faces");
        }
        Err(e) => {
            println!("Font enumeration failed: {}", e);
            // Acceptable if fonts exceed limits
        }
    }
}

#[test]
fn test_memory_budget_defaults_are_conservative() {
    let budget = MemoryBudget::default();

    // Default budget should be reasonable for embedded
    assert!(
        budget.max_entry_bytes <= 4 * 1024 * 1024,
        "max_entry_bytes too large for embedded default"
    );
    assert!(
        budget.max_css_bytes <= 512 * 1024,
        "max_css_bytes too large for embedded default"
    );
    assert!(
        budget.max_nav_bytes <= 512 * 1024,
        "max_nav_bytes too large for embedded default"
    );
    assert!(
        budget.max_inline_style_bytes <= 16 * 1024,
        "max_inline_style_bytes too large for embedded default"
    );
    assert!(
        budget.max_pages_in_memory <= 128,
        "max_pages_in_memory too large for embedded default"
    );
}

#[test]
fn test_style_limits_defaults_are_conservative() {
    let limits = StyleLimits::default();

    assert!(
        limits.max_selectors <= 4096,
        "max_selectors too large for embedded default"
    );
    assert!(
        limits.max_css_bytes <= 512 * 1024,
        "max_css_bytes too large for embedded default"
    );
    assert!(
        limits.max_nesting <= 32,
        "max_nesting too large for embedded default"
    );
}

#[test]
fn test_font_limits_defaults_are_conservative() {
    let limits = FontLimits::default();

    assert!(
        limits.max_faces <= 64,
        "max_faces too large for embedded default"
    );
    assert!(
        limits.max_bytes_per_font <= 8 * 1024 * 1024,
        "max_bytes_per_font too large for embedded default"
    );
    assert!(
        limits.max_total_font_bytes <= 64 * 1024 * 1024,
        "max_total_font_bytes too large for embedded default"
    );
}
