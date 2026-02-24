mod common;

use core::convert::Infallible;

use common::fixtures::core_fixtures;
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::BinaryColor,
    Pixel,
};
use epub_stream::EpubBook;
use epub_stream_embedded_graphics::{with_embedded_text_measurer, EgRenderer};
use epub_stream_render::{RenderConfig, RenderEngine, RenderEngineOptions, RenderPage};

const DISPLAY_WIDTH: i32 = 480;
const DISPLAY_HEIGHT: i32 = 800;

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

fn render_first_page(renderer: &EgRenderer, page: &RenderPage, fixture: &str, chapter_idx: usize) {
    let mut display = PixelCaptureDisplay::new(DISPLAY_WIDTH as u32, DISPLAY_HEIGHT as u32);
    renderer
        .render_page(page, &mut display)
        .unwrap_or_else(|e| panic!("render {} chapter {}: {}", fixture, chapter_idx, e));
    assert!(
        display.on_pixels > 0,
        "render produced no pixels for fixture {} chapter {}",
        fixture,
        chapter_idx
    );
}

#[test]
fn firmware_open_prepare_render_loop_is_stable() {
    let fixtures = core_fixtures();
    assert!(
        !fixtures.is_empty(),
        "No fixtures found under tests/fixtures. Cannot run firmware-path test."
    );

    let renderer: EgRenderer = EgRenderer::default();

    for fixture in fixtures {
        let mut book =
            EpubBook::open(fixture).unwrap_or_else(|e| panic!("open {}: {}", fixture, e));
        let chapter_count = book.chapter_count();
        assert!(chapter_count > 0, "fixture {} has no chapters", fixture);

        let engine = RenderEngine::new(RenderEngineOptions::for_display(
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
        ));

        let mut rendered_chapters = 0usize;
        for chapter_idx in 0..chapter_count.min(5) {
            let config = with_embedded_text_measurer(RenderConfig::default());
            let pages = engine
                .prepare_chapter_with_config_collect(&mut book, chapter_idx, config)
                .unwrap_or_else(|e| {
                    panic!(
                        "prepare {} chapter {} in firmware-path loop: {}",
                        fixture, chapter_idx, e
                    )
                });
            if pages.is_empty() {
                continue;
            }

            render_first_page(&renderer, &pages[0], fixture, chapter_idx);
            if let Some(next_page) = pages.get(1) {
                render_first_page(&renderer, next_page, fixture, chapter_idx);
            }
            rendered_chapters += 1;
        }

        assert!(
            rendered_chapters > 0,
            "fixture {} produced no renderable chapters in first 5 chapters",
            fixture
        );
    }
}
