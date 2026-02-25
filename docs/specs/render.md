# EPUB-STREAM Renderer Spec

Generic rendering spec for the epub-stream ecosystem. No app/device/project-specific behavior.

## Architecture Contract

1. `epub-stream`: EPUB parsing, metadata/spine/resources, CSS/style/font discovery + computed style stream.
2. `epub-stream-render`: Layout/pagination engine from styled stream -> backend-agnostic draw IR.
3. `epub-stream-embedded-graphics`: Draw IR execution on embedded-graphics targets with pluggable font backend.
4. `epub-stream-render-web`: Web-preview tooling that executes render IR into a browser-facing, self-contained HTML artifact.

Validation gates: `just all`, `just testing render-all`.

## Status

**Completed:**
- Crate split and workspace wiring
- `RenderEngine`, `LayoutConfig`, `LayoutEngine`, render IR commands, page model
- `EgRenderer` command execution in embedded-graphics backend
- Web-preview with interactive TOC/image/font/layout validation

**In progress:** See `docs/specs/embedded-render-tracker.md` for production readiness gaps.

## Goals

1. Production-grade typography and layout for common EPUBs.
2. Deterministic, testable rendering pipeline.
3. Streaming-friendly, memory-bounded operation for embedded constraints.
4. Strong API ergonomics for both simple and advanced consumers.

## Required APIs

In `epub-stream` (render_prep/book layer):

1. `prepare_chapter_with(...)` streaming API (stable).
2. `prepare_chapter_with_trace_context(...)` with structured font/style trace.
   - `prepare_chapter_with_trace(...)` is deprecated.
3. Preserve resolved face identity in styled output (not recomputed later).
4. Structured error context: resource `href`/path, selector/declaration index, token/source context.

In `epub-stream-render`:

1. `RenderEngine::prepare_chapter(...) -> Vec<RenderPage>`.
2. `RenderEngine::prepare_chapter_with(...)` page streaming callback.
3. `RenderEngine::prepare_chapter_with_page_refs(...)` borrowed page-view streaming callback for low-allocation embedded loops.
4. `LayoutConfig` with all typography knobs: margins, paragraph/list/heading gaps, indent policies, justification thresholds, line-height controls, soft-hyphen policy.
5. IR must include resolved font identity in text commands (`font_id`, not weight/italic guesses).
6. Header/footer/progress represented in IR as commands, not backend special-casing.

In `epub-stream-embedded-graphics`:

1. Font backend abstraction trait: register face(s), map `font_id` -> glyph metrics/rasterization, fallback chain with reason codes.
2. Default mono backend and optional TTF backend.
3. Consistent rendering path for justified/non-justified text using same font backend.
4. Zero-allocation (or amortized reusable buffer) glyph drawing in hot paths.

## Typography and Layout Requirements

1. Correct block semantics: paragraph, heading(level), list item, line break.
2. Justification resolved in layout stage only, deterministic in IR.
3. First-line indent policy configurable and role-aware.
4. Post-heading paragraph indent suppression configurable.
5. Soft hyphen handling: invisible when not used, visible hyphen when break occurs at SHY.
6. Whitespace normalization must preserve preformatted/significant sections.
7. CSS cascade precedence deterministic and covered by tests.
8. Inline style and class-aware resolution supported within declared limits.

## Font System Requirements

1. Family normalization + dedupe.
2. Nearest-match resolution across weight/style.
3. Embedded `@font-face` support with bounded limits.
4. Explicit fallback trace: requested families, chosen face, rejected candidates with reason.
5. Public API for consumers to inject default families and fallback order.

## Performance/Memory Requirements

1. Streaming-first APIs for prep and layout to avoid large intermediate vectors.
2. Bounded allocations and limits options exposed on all heavy paths.
3. No per-glyph heap allocation in draw loop.
4. Stable behavior under small stack settings (document expected minimums).

## Testing Requirements

1. Unit tests: CSS precedence, inline style handling, font matching/fallback, SHY behavior, whitespace-sensitive sections, justification decisions in layout IR.
2. Golden tests: Render IR snapshots for representative EPUB fragments.
3. Backend tests: command execution correctness, justified text spacing distribution, font backend parity.
4. Property/invariant tests: pagination monotonicity, non-overlapping line baselines, deterministic output.
5. Web-preview regression harness: config-to-engine mapping, dynamic reflow matrix, text-boundary safety assertions, chapter progress monotonicity.

## Non-Goals

1. Full complex-script shaping engine.
2. Full browser-grade CSS support.
3. Platform-specific UI policy decisions.

---

## API Guide

### Minimal Flow

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

### Streaming Layout Flow

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

### Range and Lazy Pagination

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

### Advanced Trace + Embedded Fonts

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

---

## Migration Notes

### Crate Split

- Use `epub-stream` for EPUB parse/style/font preparation.
- Use `epub-stream-render` for layout and render IR generation.
- Use `epub-stream-embedded-graphics` for embedded-graphics draw execution.

### Render Engine Construction

- `RenderEngine::new(...)` takes `RenderEngineOptions`.
- Build options with `RenderEngineOptions::for_display(width, height)`, or explicit `RenderEngineOptions { prep, layout }`.

### Trace API

- Preferred: `RenderPrep::prepare_chapter_with_trace_context(...)`
- Deprecated: `RenderPrep::prepare_chapter_with_trace(...)`

### Pagination APIs

New APIs in `epub-stream-render`:
- `prepare_chapter_page_range(...)`
- `prepare_chapter_iter(...)`
- `prepare_chapter_iter_streaming(...)` (owned-book, backpressured streaming iterator)

### Page Chrome Policy

Chrome behavior is configurable via `PageChromeConfig`. Both `LayoutConfig` and `EgRenderConfig` expose `page_chrome`.

### Custom Fonts

- `RenderPrep::with_registered_fonts(...)` for external/custom font face registration.
- `RenderPrep::with_embedded_fonts_from_book(...)` for EPUB-discovered `@font-face` resources.
