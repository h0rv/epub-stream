# Embedded Usage

This guide documents the embedded-focused API surface in the `mu_epub` stack.

## Production Tracker

For production-readiness gaps and priorities, see
`docs/embedded-render-tracker.md`.

## Open Lazily and Bound Navigation

```rust,no_run
use mu_epub::{EpubBook, EpubBookOptions, OpenConfig, ZipLimits};

let options = EpubBookOptions {
    zip_limits: Some(ZipLimits::new(8 * 1024 * 1024, 2048)),
    max_nav_bytes: Some(512 * 1024),
    ..EpubBookOptions::default()
};

let mut book = EpubBook::from_reader_with_config(
    std::fs::File::open("book.epub")?,
    OpenConfig {
        options,
        lazy_navigation: true,
    },
)?;

// Navigation parse is deferred until needed.
let _ = book.ensure_navigation()?;
# Ok::<(), mu_epub::EpubError>(())
```

## Bounded Resource Streaming

```rust,no_run
use mu_epub::EpubBook;

let mut book = EpubBook::open("book.epub")?;
let mut out = Vec::new();
book.read_resource_into_with_limit("xhtml/nav.xhtml", &mut out, 1024 * 1024)?;
# Ok::<(), mu_epub::EpubError>(())
```

## Stream Chapter Events

```rust,no_run
use mu_epub::{ChapterEventsOptions, EpubBook, StyledEventOrRun};

let mut book = EpubBook::open("book.epub")?;
let mut count = 0usize;
book.chapter_events(0, ChapterEventsOptions::default(), |item| {
    match item {
        StyledEventOrRun::Event(_) => {}
        StyledEventOrRun::Run(run) => {
            let _ = run.text.len();
        }
    }
    count += 1;
    Ok(())
})?;
# Ok::<(), mu_epub::EpubError>(())
```

## Incremental Pagination and Cache Hooks

```rust,no_run
use mu_epub::EpubBook;
use mu_epub_embedded_graphics::with_embedded_text_measurer;
use mu_epub_render::{RenderConfig, RenderEngine, RenderEngineOptions};

let mut book = EpubBook::open("book.epub")?;
let engine = RenderEngine::new(RenderEngineOptions::for_display(480, 800));

// Streaming callback path.
engine.prepare_chapter_with(&mut book, 0, |page| {
    let _meta = page.page_meta();
})?;

// Range path.
let _subset = engine.page_range(&mut book, 0, 0..3)?;

// Explicit session path.
let mut session = engine.begin(0, with_embedded_text_measurer(RenderConfig::default()));
let _ = &mut session;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Memory Budgets in Render Prep

```rust,no_run
use mu_epub::{MemoryBudget, RenderPrepOptions};
use mu_epub_render::{RenderEngine, RenderEngineOptions};

let opts = RenderEngineOptions {
    prep: RenderPrepOptions {
        memory: MemoryBudget {
            max_entry_bytes: 4 * 1024 * 1024,
            max_css_bytes: 512 * 1024,
            max_nav_bytes: 512 * 1024,
            max_inline_style_bytes: 16 * 1024,
            max_pages_in_memory: 64,
        },
        ..RenderPrepOptions::default()
    },
    ..RenderEngineOptions::for_display(480, 800)
};

let _engine = RenderEngine::new(opts);
```

## Embedded Renderer Diagnostics

```rust,no_run
use embedded_graphics::{mock_display::MockDisplay, pixelcolor::BinaryColor};
use mu_epub_embedded_graphics::{
    EgRenderConfig, EgRenderer, ImageFallbackPolicy, ImageRegistryLimits,
};
use mu_epub_render::RenderPage;

let renderer = EgRenderer::with_image_registry_limits(
    EgRenderConfig {
        image_fallback: ImageFallbackPolicy::OutlineWithAltText,
        ..EgRenderConfig::default()
    },
    ImageRegistryLimits {
        max_images: 8,
        max_total_pixels: 128 * 1024,
    },
);
let _registry = renderer.image_registry_diagnostics();

let page = RenderPage::new(1);
let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
let diagnostics = renderer.render_page_with_diagnostics(&page, &mut display)?;
let _fallbacks = diagnostics.text_fallbacks.total();
# Ok::<(), core::convert::Infallible>(())
```
