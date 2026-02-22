# Render API Guide

This guide covers the recommended render pipeline APIs across:

- `epub-stream` (parse + style/font prep)
- `epub-stream-render` (layout and render IR)
- `epub-stream-embedded-graphics` (draw execution backend)

## Minimal Flow

```rust
use epub_stream::{EpubBook, RenderPrep, RenderPrepOptions};
use epub_stream_render::{LayoutConfig, RenderEngine, RenderEngineOptions};

fn render_chapter_pages<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    chapter_index: usize,
) -> Result<Vec<epub_stream_render::RenderPage>, Box<dyn std::error::Error>> {
    let opts = RenderEngineOptions {
        prep: RenderPrepOptions::default(),
        layout: LayoutConfig::default(),
    };
    let engine = RenderEngine::new(opts);
    let pages = engine.prepare_chapter(book, chapter_index)?;
    Ok(pages)
}
```

## Streaming Layout Flow

```rust
use epub_stream::{EpubBook, RenderPrep, RenderPrepOptions};
use epub_stream_render::{LayoutConfig, RenderEngine, RenderEngineOptions, RenderPage};

fn stream_pages<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    chapter_index: usize,
    mut on_page: impl FnMut(RenderPage),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut layout = LayoutConfig::default();
    layout.page_chrome.progress_enabled = true;
    let opts = RenderEngineOptions {
        prep: RenderPrepOptions::default(),
        layout,
    };
    let engine = RenderEngine::new(opts);
    engine.prepare_chapter_with(book, chapter_index, |page| on_page(page))?;
    Ok(())
}
```

## Range and Lazy Pagination

```rust
use epub_stream::{EpubBook, RenderPrep, RenderPrepOptions};
use epub_stream_render::{LayoutConfig, RenderEngine, RenderEngineOptions};

fn read_page_window<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    chapter_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let opts = RenderEngineOptions {
        prep: RenderPrepOptions::default(),
        layout: LayoutConfig::default(),
    };
    let engine = RenderEngine::new(opts);

    let first_five = engine.prepare_chapter_page_range(book, chapter_index, 0, 5)?;
    let all_pages = engine.prepare_chapter_iter(book, chapter_index)?;

    assert!(first_five.len() <= all_pages.len());
    Ok(())
}
```

## Advanced Trace + Embedded Fonts

```rust
use epub_stream::{
    EmbeddedFontFace, EmbeddedFontStyle, EpubBook, RenderPrep, RenderPrepOptions, StyledEventOrRun,
};

fn inspect_traced_runs<R: std::io::Read + std::io::Seek>(
    book: &mut EpubBook<R>,
    chapter_index: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut prep = RenderPrep::new(RenderPrepOptions::default())
        .with_serif_default()
        .with_registered_fonts(
            vec![EmbeddedFontFace {
                family: "Custom".to_string(),
                weight: 400,
                style: EmbeddedFontStyle::Normal,
                stretch: None,
                href: "fonts/Custom-Regular.ttf".to_string(),
                format: Some("truetype".to_string()),
            }],
            |_href| Ok(vec![0u8; 128]),
        )?;

    prep.prepare_chapter_with_trace_context(book, chapter_index, |item, trace| {
        if let StyledEventOrRun::Run(run) = item {
            if let Some(font_trace) = trace.font_trace() {
                let _font_id = run.font_id;
                let _resolved_family = run.resolved_family.clone();
                let _reason_chain = font_trace.reason_chain.clone();
            }
        }
    })?;

    Ok(())
}
```

## Trace API Recommendation

- Primary API: `RenderPrep::prepare_chapter_with_trace_context(...)`
- Legacy API: `RenderPrep::prepare_chapter_with_trace(...)` is deprecated and maps to the structured trace context.
