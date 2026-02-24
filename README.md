# epub-stream

Memory-efficient EPUB parser for embedded systems.

Streaming architecture targeting constrained devices.
`no_std` compatible with optional `alloc`.

## Status

Core EPUB parsing, tokenization, navigation, CSS subset, and layout engine are
implemented. See [docs/specs/spec-compliance.md](docs/specs/spec-compliance.md) for current
coverage details.
Dataset bootstrap and corpus validation flow is documented in
[docs/guides/datasets.md](docs/guides/datasets.md).
Embedded-focused usage patterns are documented in [docs/guides/embedded.md](docs/guides/embedded.md).

Note: ZIP64 archives are currently not supported and are rejected explicitly.

## Features

| Feature  | Description              | Default |
|----------|--------------------------|---------|
| `std`    | Standard library + ZIP   | yes     |
| `layout` | Text layout / pagination | no      |
| `async`  | Async file-open helpers  | no      |
| `cli`    | `epub-stream` inspect binary | no      |

## Usage

```toml
[dependencies]
epub-stream = "0.2"
```

### Quick Start

```rust,no_run
use epub_stream::EpubBook;

fn main() -> Result<(), epub_stream::EpubError> {
    let mut book = EpubBook::open("book.epub")?;

    println!("Title: {}", book.title());
    println!("Author: {}", book.author());
    println!("Chapters: {}", book.chapter_count());

    // Read and tokenize first spine chapter
    let tokens = book.tokenize_spine_item(0)?;
    println!("First chapter token count: {}", tokens.len());

    Ok(())
}
```

### Optional Safety Limits

By default, EPUB reading does not enforce implicit file-size caps.
To enforce explicit limits, use either API below.

#### Builder API

```rust,no_run
use epub_stream::{EpubBook, ZipLimits};

let limits = ZipLimits::new(8 * 1024 * 1024, 1024); // explicit caps
let mut book = EpubBook::builder()
    .with_zip_limits(limits)
    .open("book.epub")?;
# Ok::<(), epub_stream::EpubError>(())
```

### Chapter Ergonomics

```rust,no_run
use epub_stream::EpubBook;

let mut book = EpubBook::open("book.epub")?;
for chapter in book.chapters() {
    println!("#{} {} ({})", chapter.index, chapter.idref, chapter.href);
}

let first_text = book.chapter_text(0)?;
println!("chars={}", first_text.len());
# Ok::<(), epub_stream::EpubError>(())
```

### Rendering Stack (Decoupled Crates)

`epub-stream` remains the EPUB parse/prep crate.
Rendering is split into:

1. `epub-stream-render`: render IR + layout engine + chapter-to-pages orchestration
2. `epub-stream-embedded-graphics`: `embedded-graphics` backend executor for render commands
3. `epub-stream-render-web`: self-contained HTML preview generator for rapid layout/font/TOC validation

Add local workspace deps:

```toml
[dependencies]
epub-stream = { path = "." }
epub-stream-render = { path = "crates/epub-stream-render" }
epub-stream-embedded-graphics = { path = "crates/epub-stream-embedded-graphics" }
epub-stream-render-web = { path = "crates/epub-stream-render-web" }
```

Prepare a chapter into backend-agnostic render pages:

```rust,no_run
use epub_stream::EpubBook;
use epub_stream_render::{RenderEngine, RenderEngineOptions};

let mut book = EpubBook::open("book.epub")?;
let engine = RenderEngine::new(RenderEngineOptions::for_display(480, 800));
let pages = engine.prepare_chapter(&mut book, 0)?;
println!("render pages: {}", pages.len());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Execute those pages on `embedded-graphics`:

```rust,no_run
use embedded_graphics::mock_display::MockDisplay;
use embedded_graphics::pixelcolor::BinaryColor;
use epub_stream_embedded_graphics::EgRenderer;

# use epub_stream::EpubBook;
# use epub_stream_render::{RenderEngine, RenderEngineOptions};
# let mut book = EpubBook::open("book.epub")?;
# let engine = RenderEngine::new(RenderEngineOptions::for_display(480, 800));
# let pages = engine.prepare_chapter(&mut book, 0)?;
let mut display: MockDisplay<BinaryColor> = MockDisplay::new();
let renderer = EgRenderer::default();
renderer.render_page(&pages[0], &mut display)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Generate a web preview with TOC navigation, image embedding, embedded-font loading,
and runtime font/layout controls:

```bash
just visualize web-preview tests/fixtures/bench/pg84-frankenstein.epub
```

Or export a standalone HTML snapshot:

```bash
just visualize web-preview-export tests/fixtures/bench/pg84-frankenstein.epub target/web-preview/index.html
```

Run the full reader-control regression harness (dynamic reflow + pagination + config mapping):

```bash
just testing render-regression
```

### CLI (Unix-Friendly)

Install from crates.io:

```bash
cargo install epub-stream --features cli --bin epub-stream
epub-stream --help
```

Inspect metadata and chapter lists:

```bash
epub-stream metadata book.epub --pretty
epub-stream chapters book.epub --ndjson
```

Extract chapter text for LLM/pipe workflows:

```bash
epub-stream chapter-text book.epub --index 0 --raw > chapter-0.txt
epub-stream toc book.epub --flat | jq .
```

Validate structure/compliance signals:

```bash
epub-stream validate book.epub --pretty
epub-stream validate book.epub --strict
```

#### Functional API

```rust,no_run
use epub_stream::{parse_epub_file_with_options, EpubBookOptions, ZipLimits};

let limits = ZipLimits::new(8 * 1024 * 1024, 1024);
let options = EpubBookOptions {
    zip_limits: Some(limits),
    ..EpubBookOptions::default()
};

let summary = parse_epub_file_with_options("book.epub", options)?;
println!("Title: {}", summary.metadata().title);
# Ok::<(), epub_stream::EpubError>(())
```

## Design

See [docs/architecture.md](docs/architecture.md) for the full plan. The short
version:

1. Stream ZIP entries from storage with a bounded buffer.
2. Parse OPF metadata and spine with `quick-xml` (SAX-style, no DOM).
3. Tokenize XHTML chapters into a compact token stream.
4. Lay out tokens into pages with greedy line breaking.
5. Render glyphs from an LRU cache to a framebuffer.

Target peak RAM: <120KB beyond the framebuffer.

## License

MIT
