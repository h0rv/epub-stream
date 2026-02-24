mod common;

use common::budget_alloc::BudgetAlloc;
use epub_stream::EpubBook;
use epub_stream_embedded_graphics::with_embedded_text_measurer;
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions};

const FIXTURE: &str = "tests/fixtures/bench/pg84-frankenstein.epub";
const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

fn fragment_heap() -> Vec<Box<[u8]>> {
    let mut fragments: Vec<Option<Box<[u8]>>> = Vec::with_capacity(180);
    for idx in 0..180usize {
        let size = match idx % 5 {
            0 => 96,
            1 => 384,
            2 => 768,
            3 => 1280,
            _ => 2048,
        };
        fragments.push(Some(vec![0x5A; size].into_boxed_slice()));
    }
    for (idx, slot) in fragments.iter_mut().enumerate() {
        if idx % 2 == 0 {
            *slot = None;
        }
    }
    fragments.into_iter().flatten().collect()
}

#[test]
fn render_navigation_survives_fragmented_heap() {
    let kept_fragments = fragment_heap();
    ALLOC.reset();

    let mut book = EpubBook::open(FIXTURE).expect("open fixture");
    assert!(book.chapter_count() > 0, "fixture has no chapters");

    let engine = RenderEngine::new(RenderEngineOptions::for_display(
        DISPLAY_WIDTH,
        DISPLAY_HEIGHT,
    ));
    let mut rendered_any = false;
    let chapter_count = book.chapter_count().min(5);

    for chapter_idx in 0..chapter_count {
        let config = with_embedded_text_measurer(RenderConfig::default());
        let pages = engine
            .prepare_chapter_with_config_collect(&mut book, chapter_idx, config)
            .unwrap_or_else(|e| panic!("render chapter {} on fragmented heap: {}", chapter_idx, e));
        if !pages.is_empty() {
            rendered_any = true;
        }
    }

    let final_config = with_embedded_text_measurer(RenderConfig::default());
    let final_pages = engine
        .prepare_chapter_with_config_collect(&mut book, 0, final_config)
        .expect("re-render chapter 0 after churn");

    drop(kept_fragments);

    assert!(rendered_any, "No chapter rendered during navigation churn");
    assert!(
        !final_pages.is_empty(),
        "Re-render chapter 0 failed after navigation churn on fragmented heap"
    );
    let peak = ALLOC.peak_bytes();
    assert!(
        peak <= 400 * 1024,
        "fragmented render peak too high: {} bytes ({:.1}KB)",
        peak,
        peak as f64 / 1024.0
    );
}
