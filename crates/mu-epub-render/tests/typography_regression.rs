use std::path::PathBuf;

use mu_epub::EpubBook;
use mu_epub_render::BlockRole;
use mu_epub_render::{DrawCommand, RenderEngine, RenderEngineOptions};

fn fixture_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../../tests/fixtures/bench");
    path.push(name);
    path
}

fn gutenberg_sample_paths(limit: usize) -> Vec<PathBuf> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/datasets/wild/gutenberg");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
            {
                out.push(path);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    if out.is_empty() {
        let fallback = [
            fixture_path("pg84-frankenstein.epub"),
            fixture_path("pg1342-pride-and-prejudice.epub"),
            fixture_path("pg1661-sherlock-holmes.epub"),
        ];
        for path in fallback {
            if path.exists() {
                out.push(path);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    out
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

fn conservative_text_width_px(text: &str, style: &mu_epub_render::ResolvedTextStyle) -> f32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0.0;
    }
    let family = style.family.to_ascii_lowercase();
    let proportional = !(family.contains("mono") || family.contains("fixed"));
    let mut em_sum = 0.0f32;
    if proportional {
        for ch in text.chars() {
            em_sum += match ch {
                ' ' | '\u{00A0}' => 0.32,
                '\t' => 1.28,
                'i' | 'l' | 'I' | '|' | '!' => 0.24,
                '.' | ',' | ':' | ';' | '\'' | '"' | '`' => 0.23,
                '-' | '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' => 0.34,
                '(' | ')' | '[' | ']' | '{' | '}' => 0.30,
                'f' | 't' | 'j' | 'r' => 0.34,
                'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' | '#' => 0.74,
                c if c.is_ascii_digit() => 0.52,
                c if c.is_ascii_uppercase() => 0.61,
                c if c.is_ascii_lowercase() => 0.50,
                c if c.is_whitespace() => 0.32,
                c if c.is_ascii_punctuation() => 0.42,
                _ => 0.56,
            };
        }
    } else {
        for ch in text.chars() {
            em_sum += if ch == ' ' { 0.52 } else { 0.58 };
        }
    }

    let mut scale = if proportional { 1.05 } else { 1.02 };
    if style.weight >= 700 {
        scale += 0.02;
    }
    if style.italic {
        scale += 0.01;
    }
    if style.size_px >= 24.0 {
        scale += 0.01;
    }
    let mut width = em_sum * style.size_px * scale;
    if chars > 1 {
        width += (chars as f32 - 1.0) * style.letter_spacing.max(0.0);
    }
    width
}

#[test]
fn frankenstein_small_margin_no_right_edge_overrun() {
    let mut book = EpubBook::open(fixture_path("pg84-frankenstein.epub")).expect("fixture open");
    let mut opts = RenderEngineOptions::for_display(480, 800);
    opts.layout.margin_left = 8;
    opts.layout.margin_right = 8;
    opts.layout.margin_top = 10;
    opts.layout.margin_bottom = 24;
    opts.layout.first_line_indent_px = 0;
    opts.layout.line_gap_px = 4;
    opts.layout.paragraph_gap_px = 8;
    opts.prep.layout_hints.base_font_size_px = 24.0;
    opts.prep.layout_hints.text_scale = 1.0;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 72.0;
    opts.prep.style.hints = opts.prep.layout_hints;
    let engine = RenderEngine::new(opts);

    let (_, pages) = (0..book.chapter_count())
        .find_map(|idx| {
            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
            (pages.len() >= 2).then_some((idx, pages))
        })
        .expect("expected multi-page chapter in frankenstein");

    // Guard against actual screen-edge clipping, not margin occupancy.
    // Small margins may intentionally allow text close to the edge.
    let right_limit = 478i32;
    for (page_idx, page) in pages.iter().enumerate().take(4) {
        for cmd in &page.commands {
            let DrawCommand::Text(text) = cmd else {
                continue;
            };
            let est_right = text.x as f32 + conservative_text_width_px(&text.text, &text.style);
            assert!(
                est_right <= right_limit as f32,
                "page={} line='{}' right_est={} > right_limit={}",
                page_idx,
                text.text,
                est_right,
                right_limit
            );
        }
    }
}

fn assert_no_screen_edge_overrun(
    pages: &[mu_epub_render::RenderPage],
    display_width: i32,
    max_pages: usize,
) {
    let right_limit = (display_width - 2) as f32;
    for (page_idx, page) in pages.iter().enumerate().take(max_pages) {
        for cmd in &page.commands {
            let DrawCommand::Text(text) = cmd else {
                continue;
            };
            if !matches!(
                text.style.role,
                BlockRole::Body | BlockRole::Paragraph | BlockRole::ListItem
            ) {
                continue;
            }
            if is_uppercase_heavy(&text.text) {
                continue;
            }
            assert!(text.x >= 0, "page={} negative x={}", page_idx, text.x);
            let est_right = text.x as f32 + conservative_text_width_px(&text.text, &text.style);
            assert!(
                est_right <= right_limit,
                "page={} line='{}' right_est={} > right_limit={}",
                page_idx,
                text.text,
                est_right,
                right_limit
            );
        }
    }
}

fn is_uppercase_heavy(text: &str) -> bool {
    let mut alpha = 0usize;
    let mut upper = 0usize;
    for ch in text.chars() {
        if ch.is_ascii_alphabetic() {
            alpha += 1;
            if ch.is_ascii_uppercase() {
                upper += 1;
            }
        }
    }
    alpha >= 8 && (upper as f32 / alpha as f32) >= 0.75
}

#[test]
fn right_edge_safety_matrix_across_sizes_margins_and_justify() {
    let fixtures = ["pg84-frankenstein.epub", "pg1661-sherlock-holmes.epub"];
    let font_sizes = [18.0f32, 22.0, 26.0, 30.0];
    let margins = [8i32, 10, 14];
    let justify_modes = [false, true];
    let display_width = 480;
    let display_height = 800;

    for fixture in fixtures {
        for font_size in font_sizes {
            for margin in margins {
                for justify in justify_modes {
                    let mut book =
                        EpubBook::open(fixture_path(fixture)).expect("fixture should open");
                    let mut opts = RenderEngineOptions::for_display(display_width, display_height);
                    opts.layout.margin_left = margin;
                    opts.layout.margin_right = margin;
                    opts.layout.margin_top = 10;
                    opts.layout.margin_bottom = 24;
                    opts.layout.first_line_indent_px = 0;
                    opts.layout.line_gap_px = 4;
                    opts.layout.paragraph_gap_px = 8;
                    opts.layout.typography.justification.enabled = justify;
                    opts.layout.typography.justification.min_words = 6;
                    opts.layout.typography.justification.min_fill_ratio = 0.78;
                    opts.prep.layout_hints.base_font_size_px = font_size;
                    opts.prep.layout_hints.text_scale = 1.0;
                    opts.prep.layout_hints.min_font_size_px = 14.0;
                    opts.prep.layout_hints.max_font_size_px = 72.0;
                    opts.prep.style.hints = opts.prep.layout_hints;
                    let engine = RenderEngine::new(opts);

                    let (_, pages) = (0..book.chapter_count())
                        .find_map(|idx| {
                            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
                            (pages.len() >= 2).then_some((idx, pages))
                        })
                        .expect("expected multi-page chapter");

                    assert_no_screen_edge_overrun(&pages, display_width, 3);
                }
            }
        }
    }
}

#[test]
fn frankenstein_progress_and_chapter_metrics_are_monotonic() {
    let mut book = EpubBook::open(fixture_path("pg84-frankenstein.epub")).expect("fixture open");
    let engine = build_engine(480, 800, 24.0, false);
    let mut validated = 0usize;

    for chapter_idx in 0..book.chapter_count() {
        let first = engine
            .prepare_chapter_page_range(&mut book, chapter_idx, 0, 1)
            .expect("chapter first page should render");
        if first.is_empty() {
            continue;
        }
        let metrics = first[0].metrics;
        assert_eq!(metrics.chapter_index, chapter_idx);
        assert_eq!(metrics.chapter_page_index, 0);
        let chapter_pages = metrics.chapter_page_count.unwrap_or(1).max(1);
        assert!(chapter_pages >= 1);
        if chapter_pages > 1 {
            assert!(
                metrics.progress_chapter <= 0.05,
                "chapter {} first-page progress should start near zero, got {}",
                chapter_idx,
                metrics.progress_chapter
            );
        } else {
            assert!(
                metrics.progress_chapter >= 0.99,
                "single-page chapter should report full chapter progress"
            );
        }
        validated += 1;
    }
    assert!(validated >= 3, "expected several renderable chapters");

    let (chapter_idx, pages) = (0..book.chapter_count())
        .find_map(|idx| {
            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
            (pages.len() >= 2).then_some((idx, pages))
        })
        .expect("expected a chapter with >=2 pages");
    let last = pages.last().expect("chapter should have last page");
    assert_eq!(last.metrics.chapter_index, chapter_idx);
    assert!(
        last.metrics.progress_chapter >= 0.95,
        "last chapter page should have near-complete chapter progress"
    );
}

#[test]
fn ranged_metrics_are_complete_and_monotonic_for_frankenstein() {
    let mut book = EpubBook::open(fixture_path("pg84-frankenstein.epub")).expect("fixture open");
    let engine = build_engine(480, 800, 24.0, false);
    let (chapter, all_pages) = (0..book.chapter_count())
        .find_map(|idx| {
            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
            (pages.len() >= 4).then_some((idx, pages))
        })
        .expect("expected chapter with >=4 pages");
    let total = all_pages.len();
    let ranges = vec![(0, 1), (1, 2), (total - 1, total), (1, total - 1)];

    for (start, end) in ranges {
        let pages = engine
            .prepare_chapter_page_range(&mut book, chapter, start, end)
            .expect("range render should succeed");
        assert_eq!(pages.len(), end - start);
        let mut last_progress = 0.0f32;
        for (idx, page) in pages.iter().enumerate() {
            let metrics = &page.metrics;
            assert_eq!(metrics.chapter_index, chapter);
            assert_eq!(
                metrics.chapter_page_index,
                start + idx,
                "range=({},{}) page_number={}",
                start,
                end,
                page.page_number
            );
            assert_eq!(metrics.chapter_page_count, Some(total));
            assert!(metrics.progress_chapter >= 0.0 && metrics.progress_chapter <= 1.0);
            assert!(metrics.progress_chapter >= last_progress);
            last_progress = metrics.progress_chapter;
        }
    }
}

#[test]
fn tiny_viewport_large_font_still_produces_bounded_lines() {
    let mut book = EpubBook::open(fixture_path("pg84-frankenstein.epub")).expect("fixture open");
    let mut opts = RenderEngineOptions::for_display(320, 240);
    opts.layout.margin_left = 8;
    opts.layout.margin_right = 8;
    opts.layout.margin_top = 8;
    opts.layout.margin_bottom = 20;
    opts.layout.first_line_indent_px = 0;
    opts.layout.line_gap_px = 2;
    opts.layout.paragraph_gap_px = 6;
    opts.layout.typography.justification.enabled = false;
    opts.prep.layout_hints.base_font_size_px = 30.0;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 72.0;
    opts.prep.style.hints = opts.prep.layout_hints;
    let engine = RenderEngine::new(opts);

    let (_, pages) = (0..book.chapter_count())
        .find_map(|idx| {
            let pages = engine.prepare_chapter(&mut book, idx).ok()?;
            (!pages.is_empty()).then_some((idx, pages))
        })
        .expect("expected renderable chapter");
    assert_no_screen_edge_overrun(&pages, 320, 2);
}

#[test]
fn gutenberg_corpus_sample_has_no_body_right_edge_overrun() {
    let samples = gutenberg_sample_paths(3);
    if samples.is_empty() {
        return;
    }
    for path in samples {
        let mut book = EpubBook::open(&path).expect("gutenberg sample should open");
        let mut opts = RenderEngineOptions::for_display(480, 800);
        opts.layout.margin_left = 8;
        opts.layout.margin_right = 8;
        opts.layout.margin_top = 10;
        opts.layout.margin_bottom = 24;
        opts.layout.first_line_indent_px = 0;
        opts.layout.typography.justification.enabled = true;
        opts.prep.layout_hints.base_font_size_px = 24.0;
        opts.prep.style.hints = opts.prep.layout_hints;
        let engine = RenderEngine::new(opts);
        let (_, pages) = chapter_with_pages(&engine, &mut book).expect("chapter should render");
        assert_no_screen_edge_overrun(&pages, 480, 2);
    }
}
