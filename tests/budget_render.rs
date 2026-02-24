mod common;

use common::budget_alloc::BudgetAlloc;
use common::fixtures::core_fixtures;
use epub_stream::EpubBook;
use epub_stream_embedded_graphics::with_embedded_text_measurer;
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;
// Current fixtures peak around 439KiB in render-prep paths.
// Keep a guardrail at 512KiB and ratchet downward as memory work lands.
const RENDER_PREP_BUDGET_BYTES: usize = 512 * 1024;

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

fn choose_render_chapter(path: &str) -> usize {
    let mut book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path, e));
    let chapter_count = book.chapter_count();
    if chapter_count == 0 {
        return 0;
    }
    for idx in 0..chapter_count.min(12) {
        if let Ok(tokens) = book.tokenize_spine_item(idx) {
            if !tokens.is_empty() {
                return idx;
            }
        }
    }
    0
}

#[test]
fn render_chapter_under_budget_for_core_fixtures() {
    let fixtures = core_fixtures();
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under tests/fixtures. Cannot run render budget test."
    );

    for path in fixtures {
        let chapter_index = choose_render_chapter(path);

        ALLOC.reset();
        let mut book = EpubBook::open(path).unwrap_or_else(|e| panic!("open {}: {}", path, e));
        let engine = RenderEngine::new(RenderEngineOptions::for_display(
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
        ));
        let config = with_embedded_text_measurer(RenderConfig::default());
        let pages = engine
            .prepare_chapter_with_config_collect(&mut book, chapter_index, config)
            .unwrap_or_else(|e| panic!("render prep {} chapter {}: {}", path, chapter_index, e));
        assert!(
            !pages.is_empty(),
            "fixture {} chapter {} produced no pages",
            path,
            chapter_index
        );

        let peak = ALLOC.peak_bytes();
        assert!(
            peak <= RENDER_PREP_BUDGET_BYTES,
            "render prep peak over budget for {} chapter {}: {} bytes ({:.1}KB), budget: {}KB",
            path,
            chapter_index,
            peak,
            peak as f64 / 1024.0,
            RENDER_PREP_BUDGET_BYTES / 1024
        );
        println!(
            "render fixture={} chapter={} peak_kib={:.1} allocs={}",
            path,
            chapter_index,
            peak as f64 / 1024.0,
            ALLOC.alloc_count()
        );
    }
}
