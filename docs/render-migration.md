# Render Migration Notes

This document captures migration updates for downstream consumers moving to the split renderer stack.

## Crate Split

- Use `epub-stream` for EPUB parse/style/font preparation.
- Use `epub-stream-render` for layout and render IR generation.
- Use `epub-stream-embedded-graphics` for embedded-graphics draw execution.

## Render Engine Construction

- `RenderEngine::new(...)` takes `RenderEngineOptions`.
- Build options with:
  - `RenderEngineOptions::for_display(width, height)`, or
  - explicit `RenderEngineOptions { prep, layout }`.

## Trace API

- Preferred:
  - `RenderPrep::prepare_chapter_with_trace_context(...)`
- Deprecated compatibility:
  - `RenderPrep::prepare_chapter_with_trace(...)`

If you currently rely on optional trace callbacks, switch to `RenderPrepTrace` and call:

- `trace.font_trace()`
- `trace.style_context()`

## Pagination APIs

- New APIs in `epub-stream-render`:
  - `prepare_chapter_page_range(...)`
  - `prepare_chapter_iter(...)`
  - `prepare_chapter_iter_streaming(...)` (owned-book, backpressured streaming iterator)

These complement the existing:

- `prepare_chapter(...)`
- `prepare_chapter_with(...)`

## Page Chrome Policy

- Chrome behavior is now configurable via `PageChromeConfig`.
- `LayoutConfig` and `EgRenderConfig` both expose `page_chrome`.

Defaults:

- Layout defaults are opt-in for chrome markers.
- Embedded renderer defaults preserve historical geometry when chrome commands are present.

## Custom Fonts

- `RenderPrep::with_registered_fonts(...)` allows external/custom font face registration via loader callback.
- `RenderPrep::with_embedded_fonts_from_book(...)` remains available for EPUB-discovered `@font-face` resources.
