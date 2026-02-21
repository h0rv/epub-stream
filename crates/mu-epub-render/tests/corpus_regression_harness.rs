use std::path::{Path, PathBuf};

use mu_epub::{BlockRole, EpubBook};
use mu_epub_render::{
    DrawCommand, RenderEngine, RenderEngineOptions, RenderPage, ResolvedTextStyle,
};

const MAX_FIXTURES: usize = 12;
const MAX_CHAPTERS_PER_FIXTURE: usize = 2;
const MAX_PAGES_FOR_RIGHT_EDGE_CHECK: usize = 4;

#[derive(Clone, Copy)]
struct HarnessProfile {
    name: &'static str,
    display_width: i32,
    display_height: i32,
    margin_left: i32,
    margin_right: i32,
    margin_top: i32,
    margin_bottom: i32,
    base_font_size_px: f32,
    text_scale: f32,
    line_gap_px: i32,
    paragraph_gap_px: i32,
    justify: bool,
}

const PROFILES: [HarnessProfile; 3] = [
    HarnessProfile {
        name: "baseline",
        display_width: 480,
        display_height: 800,
        margin_left: 10,
        margin_right: 10,
        margin_top: 10,
        margin_bottom: 24,
        base_font_size_px: 22.0,
        text_scale: 1.0,
        line_gap_px: 4,
        paragraph_gap_px: 8,
        justify: false,
    },
    HarnessProfile {
        name: "justify",
        display_width: 480,
        display_height: 800,
        margin_left: 10,
        margin_right: 10,
        margin_top: 10,
        margin_bottom: 24,
        base_font_size_px: 24.0,
        text_scale: 1.0,
        line_gap_px: 4,
        paragraph_gap_px: 8,
        justify: true,
    },
    HarnessProfile {
        name: "compact-large",
        display_width: 360,
        display_height: 640,
        margin_left: 8,
        margin_right: 8,
        margin_top: 8,
        margin_bottom: 20,
        base_font_size_px: 26.0,
        text_scale: 1.0,
        line_gap_px: 5,
        paragraph_gap_px: 10,
        justify: false,
    },
];

fn bench_fixture_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.push("../../tests/fixtures/bench");
    root
}

fn discover_bench_fixture_epubs(limit: usize) -> Vec<PathBuf> {
    let root = bench_fixture_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!(
            "Skipping corpus regression harness: fixture directory not found: {}",
            root.display()
        );
        return Vec::new();
    };

    let mut fixtures = Vec::with_capacity(limit);
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
        {
            fixtures.push(path);
        }
    }
    fixtures.sort_unstable();
    fixtures.truncate(limit);
    fixtures
}

fn build_engine(profile: HarnessProfile) -> RenderEngine {
    let mut opts = RenderEngineOptions::for_display(profile.display_width, profile.display_height);
    opts.layout.margin_left = profile.margin_left;
    opts.layout.margin_right = profile.margin_right;
    opts.layout.margin_top = profile.margin_top;
    opts.layout.margin_bottom = profile.margin_bottom;
    opts.layout.first_line_indent_px = 0;
    opts.layout.line_gap_px = profile.line_gap_px;
    opts.layout.paragraph_gap_px = profile.paragraph_gap_px;
    opts.layout.typography.justification.enabled = profile.justify;
    opts.layout.typography.justification.min_words = 6;
    opts.layout.typography.justification.min_fill_ratio = 0.78;

    opts.prep.layout_hints.base_font_size_px = profile.base_font_size_px;
    opts.prep.layout_hints.text_scale = profile.text_scale;
    opts.prep.layout_hints.min_font_size_px = 14.0;
    opts.prep.layout_hints.max_font_size_px = 72.0;
    opts.prep.layout_hints.min_line_height = 1.05;
    opts.prep.layout_hints.max_line_height = 1.45;
    opts.prep.style.hints = opts.prep.layout_hints;
    opts.prep.memory.max_pages_in_memory = 512;
    RenderEngine::new(opts)
}

fn page_has_meaningful_text(page: &RenderPage) -> bool {
    page.commands.iter().any(|cmd| match cmd {
        DrawCommand::Text(text) => {
            !text.text.trim().is_empty()
                && matches!(
                    text.style.role,
                    BlockRole::Body
                        | BlockRole::Paragraph
                        | BlockRole::ListItem
                        | BlockRole::Heading(_)
                )
        }
        _ => false,
    })
}

fn select_text_chapters(
    fixture_path: &Path,
    profile: HarnessProfile,
    max_chapters: usize,
) -> Vec<usize> {
    let mut book = match EpubBook::open(fixture_path) {
        Ok(book) => book,
        Err(err) => {
            eprintln!(
                "Skipping fixture {}: failed to open EPUB ({err})",
                fixture_path.display()
            );
            return Vec::new();
        }
    };
    let engine = build_engine(profile);
    let mut selected = Vec::with_capacity(max_chapters);

    for chapter_idx in 0..book.chapter_count() {
        let pages = engine
            .prepare_chapter(&mut book, chapter_idx)
            .unwrap_or_else(|err| {
                panic!(
                    "fixture={} chapter={} profile={} failed while selecting chapters: {}",
                    fixture_path.display(),
                    chapter_idx,
                    profile.name,
                    err
                )
            });
        if pages.is_empty() {
            continue;
        }
        if pages.iter().any(page_has_meaningful_text) {
            selected.push(chapter_idx);
            if selected.len() >= max_chapters {
                break;
            }
        }
    }

    selected
}

fn assert_page_metrics_sanity(
    pages: &[RenderPage],
    chapter_idx: usize,
    fixture_path: &Path,
    profile: HarnessProfile,
) {
    assert!(
        !pages.is_empty(),
        "fixture={} profile={} chapter={} should render at least one page",
        fixture_path.display(),
        profile.name,
        chapter_idx
    );

    let expected_count = pages.len();
    for (idx, page) in pages.iter().enumerate() {
        let metrics = &page.metrics;
        assert_eq!(
            page.page_number,
            idx + 1,
            "fixture={} profile={} chapter={} page idx={} has unexpected page_number={}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            page.page_number
        );
        assert_eq!(
            metrics.chapter_index,
            chapter_idx,
            "fixture={} profile={} chapter={} page idx={} has chapter_index={}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            metrics.chapter_index
        );
        assert_eq!(
            metrics.chapter_page_index,
            idx,
            "fixture={} profile={} chapter={} page idx={} has chapter_page_index={}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            metrics.chapter_page_index
        );
        assert_eq!(
            metrics.chapter_page_count,
            Some(expected_count),
            "fixture={} profile={} chapter={} page idx={} has chapter_page_count={:?}, expected {}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            metrics.chapter_page_count,
            expected_count
        );
        if let Some(global_idx) = metrics.global_page_index {
            assert_eq!(
                global_idx,
                idx,
                "fixture={} profile={} chapter={} page idx={} has global_page_index={}",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                idx,
                global_idx
            );
        }
        if let Some(global_count) = metrics.global_page_count_estimate {
            assert_eq!(
                global_count,
                expected_count,
                "fixture={} profile={} chapter={} page idx={} has global_page_count_estimate={}",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                idx,
                global_count
            );
        }
        if let Some(progress_book) = metrics.progress_book {
            assert!(
                progress_book.is_finite() && (0.0..=1.0).contains(&progress_book),
                "fixture={} profile={} chapter={} page idx={} has invalid progress_book={}",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                idx,
                progress_book
            );
        }
    }
}

fn assert_progress_monotonicity(
    pages: &[RenderPage],
    chapter_idx: usize,
    fixture_path: &Path,
    profile: HarnessProfile,
) {
    let mut last_chapter = 0.0f32;
    let mut last_book = 0.0f32;

    for (idx, page) in pages.iter().enumerate() {
        let chapter_progress = page.metrics.progress_chapter;
        assert!(
            chapter_progress.is_finite() && (0.0..=1.0).contains(&chapter_progress),
            "fixture={} profile={} chapter={} page idx={} has invalid chapter progress={}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            chapter_progress
        );
        if idx > 0 {
            assert!(
                chapter_progress + f32::EPSILON >= last_chapter,
                "fixture={} profile={} chapter={} chapter progress regressed at page idx={} ({} -> {})",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                idx,
                last_chapter,
                chapter_progress
            );
        }
        last_chapter = chapter_progress;

        if let Some(book_progress) = page.metrics.progress_book {
            if idx > 0 {
                assert!(
                    book_progress + f32::EPSILON >= last_book,
                    "fixture={} profile={} chapter={} book progress regressed at page idx={} ({} -> {})",
                    fixture_path.display(),
                    profile.name,
                    chapter_idx,
                    idx,
                    last_book,
                    book_progress
                );
            }
            last_book = book_progress;
        }
    }

    if pages.len() > 1 {
        assert!(
            pages[0].metrics.progress_chapter <= 0.05,
            "fixture={} profile={} chapter={} first-page chapter progress should start near zero, got {}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            pages[0].metrics.progress_chapter
        );
        assert!(
            last_chapter >= 0.95,
            "fixture={} profile={} chapter={} last-page chapter progress should end near one, got {}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            last_chapter
        );
    } else {
        assert!(
            last_chapter >= 0.99,
            "fixture={} profile={} chapter={} single-page chapter should report full progress, got {}",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            last_chapter
        );
    }
}

fn assert_page_range_consistency(
    engine: &RenderEngine,
    book: &mut EpubBook<std::fs::File>,
    chapter_idx: usize,
    full_pages: &[RenderPage],
    fixture_path: &Path,
    profile: HarnessProfile,
) {
    let total = full_pages.len();
    let candidates = [0usize, total / 2, total.saturating_sub(1)];
    let mut last_checked = usize::MAX;

    for idx in candidates {
        if idx >= total || idx == last_checked {
            continue;
        }
        let ranged = engine
            .prepare_chapter_page_range(book, chapter_idx, idx, idx + 1)
            .unwrap_or_else(|err| {
                panic!(
                    "fixture={} profile={} chapter={} range [{}, {}) failed: {}",
                    fixture_path.display(),
                    profile.name,
                    chapter_idx,
                    idx,
                    idx + 1,
                    err
                )
            });
        assert_eq!(
            ranged.len(),
            1,
            "fixture={} profile={} chapter={} range [{}, {}) should return one page",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            idx + 1
        );
        assert_eq!(
            ranged[0],
            full_pages[idx],
            "fixture={} profile={} chapter={} range [{}, {}) should match full render page",
            fixture_path.display(),
            profile.name,
            chapter_idx,
            idx,
            idx + 1
        );
        last_checked = idx;
    }
}

fn assert_no_right_edge_overrun(
    pages: &[RenderPage],
    profile: HarnessProfile,
    fixture_path: &Path,
    chapter_idx: usize,
) {
    let right_limit = (profile.display_width - 2) as f32;
    let mut sampled = 0usize;

    for (page_idx, page) in pages
        .iter()
        .enumerate()
        .take(MAX_PAGES_FOR_RIGHT_EDGE_CHECK)
    {
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
            if text.text.trim().is_empty() || is_uppercase_heavy(&text.text) {
                continue;
            }

            assert!(
                text.x >= 0,
                "fixture={} profile={} chapter={} page={} has negative x={} for line '{}'",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                page_idx,
                text.x,
                text.text
            );
            let estimated_right =
                text.x as f32 + conservative_text_width_px(&text.text, &text.style);
            assert!(
                estimated_right <= right_limit,
                "fixture={} profile={} chapter={} page={} line '{}' right estimate {} exceeds {}",
                fixture_path.display(),
                profile.name,
                chapter_idx,
                page_idx,
                text.text,
                estimated_right,
                right_limit
            );
            sampled += 1;
        }
    }

    assert!(
        sampled > 0,
        "fixture={} profile={} chapter={} should include sampled body lines",
        fixture_path.display(),
        profile.name,
        chapter_idx
    );
}

fn conservative_text_width_px(text: &str, style: &ResolvedTextStyle) -> f32 {
    let chars = text.chars().count();
    if chars == 0 {
        return 0.0;
    }

    let proportional = !looks_monospace_family(&style.family);
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

fn looks_monospace_family(family: &str) -> bool {
    contains_ascii_case_insensitive(family, b"mono")
        || contains_ascii_case_insensitive(family, b"fixed")
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &[u8]) -> bool {
    let bytes = haystack.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    bytes
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
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
fn bench_corpus_profiles_preserve_deterministic_invariants() {
    let fixtures = discover_bench_fixture_epubs(MAX_FIXTURES);
    if fixtures.is_empty() {
        eprintln!(
            "Skipping corpus regression harness: no .epub fixtures found under {}",
            bench_fixture_root().display()
        );
        return;
    }

    let baseline_profile = PROFILES[0];
    let mut validated_cases = 0usize;

    for fixture in fixtures {
        let selected = select_text_chapters(&fixture, baseline_profile, MAX_CHAPTERS_PER_FIXTURE);
        if selected.is_empty() {
            eprintln!(
                "Skipping fixture {}: no text-bearing chapters were discoverable",
                fixture.display()
            );
            continue;
        }

        for profile in PROFILES {
            let engine = build_engine(profile);
            let mut book = EpubBook::open(&fixture).unwrap_or_else(|err| {
                panic!(
                    "fixture={} profile={} failed to open EPUB: {}",
                    fixture.display(),
                    profile.name,
                    err
                )
            });

            for chapter_idx in selected.iter().copied() {
                let pages = engine
                    .prepare_chapter(&mut book, chapter_idx)
                    .unwrap_or_else(|err| {
                        panic!(
                            "fixture={} profile={} chapter={} failed to render: {}",
                            fixture.display(),
                            profile.name,
                            chapter_idx,
                            err
                        )
                    });
                assert_page_metrics_sanity(&pages, chapter_idx, &fixture, profile);
                assert_progress_monotonicity(&pages, chapter_idx, &fixture, profile);
                assert_no_right_edge_overrun(&pages, profile, &fixture, chapter_idx);
                assert_page_range_consistency(
                    &engine,
                    &mut book,
                    chapter_idx,
                    &pages,
                    &fixture,
                    profile,
                );
                validated_cases += 1;
            }
        }
    }

    if validated_cases == 0 {
        eprintln!(
            "Skipping corpus regression harness: no renderable fixture/profile chapter cases"
        );
    }
}
