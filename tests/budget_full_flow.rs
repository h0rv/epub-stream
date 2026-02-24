mod common;

use core::convert::Infallible;

use common::budget_alloc::BudgetAlloc;
use common::fixtures::core_fixtures;
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
    Pixel,
};
use epub_stream::EpubBook;
use epub_stream_embedded_graphics::{with_embedded_text_measurer, EgRenderer};
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;
// Current fixtures peak around 439KiB in open->prepare->render flow.
// Keep a guardrail at 512KiB and tighten with each optimization pass.
const FULL_FLOW_BUDGET_BYTES: usize = 512 * 1024;

#[global_allocator]
static ALLOC: BudgetAlloc = BudgetAlloc::new();

#[derive(Default)]
struct PixelCaptureDisplay {
    size: Size,
    on_pixels: usize,
}

impl PixelCaptureDisplay {
    fn new(width: u32, height: u32) -> Self {
        Self {
            size: Size::new(width, height),
            on_pixels: 0,
        }
    }
}

impl OriginDimensions for PixelCaptureDisplay {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for PixelCaptureDisplay {
    type Color = BinaryColor;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(_, color) in pixels {
            if color == BinaryColor::On {
                self.on_pixels += 1;
            }
        }
        Ok(())
    }
}

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
fn open_prepare_render_under_budget_for_core_fixtures() {
    let fixtures = core_fixtures();
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under tests/fixtures. Cannot run full-flow budget test."
    );

    let renderer: EgRenderer = EgRenderer::default();

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

        let mut display = PixelCaptureDisplay::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32);
        renderer
            .render_page(&pages[0], &mut display)
            .unwrap_or_else(|e| {
                panic!(
                    "render first page {} chapter {}: {}",
                    path, chapter_index, e
                )
            });
        assert!(
            display.on_pixels > 0,
            "fixture {} chapter {} produced an empty raster output",
            path,
            chapter_index
        );

        let peak = ALLOC.peak_bytes();
        assert!(
            peak <= FULL_FLOW_BUDGET_BYTES,
            "full flow peak over budget for {} chapter {}: {} bytes ({:.1}KB), budget: {}KB",
            path,
            chapter_index,
            peak,
            peak as f64 / 1024.0,
            FULL_FLOW_BUDGET_BYTES / 1024
        );
        println!(
            "full_flow fixture={} chapter={} peak_kib={:.1} allocs={}",
            path,
            chapter_index,
            peak as f64 / 1024.0,
            ALLOC.alloc_count()
        );
    }
}
