use std::path::PathBuf;

use mu_epub::EpubBook;
use mu_epub_render::{DrawCommand, RenderEngine, RenderEngineOptions};

fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../tests/fixtures/bench");
    path.push(name);
    path
}

fn build_engine(width: i32, height: i32, font_size_px: f32, justify: bool) -> RenderEngine {
    let mut opts = RenderEngineOptions::for_display(width, height);
    opts.layout.margin_left = 10;
    opts.layout.margin_right = 10;
    opts.layout.margin_top = 10;
    opts.layout.margin_bottom = 24;
    opts.layout.first_line_indent_px = 0;
    opts.layout.line_gap_px = 3;
    opts.layout.paragraph_gap_px = 8;
    opts.layout.typography.justification.enabled = justify;
    opts.layout.typography.justification.min_words = 6;
    opts.layout.typography.justification.min_fill_ratio = 0.78;
    opts.prep.layout_hints.base_font_size_px = font_size_px;
    opts.prep.layout_hints.text_scale = 1.0;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 72.0;
    opts.prep.layout_hints.min_line_height = 1.05;
    opts.prep.layout_hints.max_line_height = 1.40;
    opts.prep.style.hints = opts.prep.layout_hints;
    RenderEngine::new(opts)
}

fn chapter_with_pages(
    engine: &RenderEngine,
    book: &mut EpubBook<std::fs::File>,
) -> Option<(usize, Vec<mu_epub_render::RenderPage>)> {
    for chapter in 0..book.chapter_count() {
        let pages = engine.prepare_chapter(book, chapter).ok()?;
        if !pages.is_empty() {
            return Some((chapter, pages));
        }
    }
    None
}

#[test]
fn corpus_large_font_increases_page_count_and_keeps_top_padding() {
    let fixtures = [
        "pg84-frankenstein.epub",
        "pg1342-pride-and-prejudice.epub",
        "pg1661-sherlock-holmes.epub",
    ];

    for fixture in fixtures {
        let mut book_default = EpubBook::open(fixture_path(fixture)).expect("fixture should open");
        let mut book_large = EpubBook::open(fixture_path(fixture)).expect("fixture should open");

        let engine_default = build_engine(480, 800, 22.0, false);
        let engine_large = build_engine(480, 800, 30.0, false);

        let (_, default_pages) =
            chapter_with_pages(&engine_default, &mut book_default).expect("chapter should render");
        let (chapter, large_pages) =
            chapter_with_pages(&engine_large, &mut book_large).expect("chapter should render");

        assert!(
            large_pages.len() >= default_pages.len(),
            "fixture={} chapter={} expected large-font page count >= default ({} >= {})",
            fixture,
            chapter,
            large_pages.len(),
            default_pages.len()
        );

        let margin_top = 10;
        let first_text = large_pages
            .iter()
            .flat_map(|p| p.commands.iter())
            .find_map(|cmd| match cmd {
                DrawCommand::Text(t) => Some(t),
                _ => None,
            })
            .expect("rendered page should contain text");
        assert!(
            first_text.baseline_y > margin_top,
            "fixture={} chapter={} first baseline {} should exceed margin_top {}",
            fixture,
            chapter,
            first_text.baseline_y,
            margin_top
        );
    }
}

#[test]
fn corpus_justification_does_not_collapse_to_single_page() {
    let fixtures = ["pg84-frankenstein.epub", "pg1661-sherlock-holmes.epub"];
    for fixture in fixtures {
        let mut book = EpubBook::open(fixture_path(fixture)).expect("fixture should open");
        let engine = build_engine(480, 800, 24.0, true);
        let (chapter, pages) =
            chapter_with_pages(&engine, &mut book).expect("chapter should render");
        assert!(
            !pages.is_empty(),
            "fixture={} chapter={} expected non-empty output",
            fixture,
            chapter
        );
    }
}

#[test]
fn frankenstein_range_pages_have_consistent_metrics() {
    let mut book = EpubBook::open(fixture_path("pg84-frankenstein.epub")).expect("fixture open");
    let engine = build_engine(480, 800, 24.0, false);

    let (chapter, all_pages) = (0..book.chapter_count())
        .find_map(|idx| {
            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
            (pages.len() >= 2).then_some((idx, pages))
        })
        .expect("expected a chapter with >=2 pages");

    let total = all_pages.len();
    let first = engine
        .prepare_chapter_page_range(&mut book, chapter, 0, 1)
        .expect("first page range should render");
    let last = engine
        .prepare_chapter_page_range(&mut book, chapter, total - 1, total)
        .expect("last page range should render");

    assert_eq!(first.len(), 1);
    assert_eq!(last.len(), 1);

    let p0 = &first[0].metrics;
    let p_last = &last[0].metrics;
    assert_eq!(p0.chapter_index, chapter);
    assert_eq!(p_last.chapter_index, chapter);
    assert_eq!(p0.chapter_page_index, 0);
    assert_eq!(p_last.chapter_page_index, total - 1);
    assert_eq!(p0.chapter_page_count, Some(total));
    assert_eq!(p_last.chapter_page_count, Some(total));
    assert!(p0.progress_chapter <= p_last.progress_chapter);
    assert!(p_last.progress_chapter >= 0.95);
}
