# mu-epub (epμb)

Memory-efficient EPUB parser for embedded systems.

Streaming architecture targeting constrained devices.
`no_std` compatible with optional `alloc`.

## Status

Core EPUB parsing, tokenization, navigation, CSS subset, and layout engine are
implemented. See [docs/spec-compliance.md](docs/spec-compliance.md) and
[docs/compliance.md](docs/compliance.md) for current coverage details.
Dataset bootstrap and corpus validation flow is documented in
[docs/datasets.md](docs/datasets.md).
Embedded-focused usage patterns are documented in [docs/embedded.md](docs/embedded.md).

Note: ZIP64 archives are currently not supported and are rejected explicitly.

## Features

| Feature  | Description              | Default |
|----------|--------------------------|---------|
| `std`    | Standard library + ZIP   | yes     |
| `layout` | Text layout / pagination | no      |
| `async`  | Async file-open helpers  | no      |
| `cli`    | `mu-epub` inspect binary | no      |

## Usage

```toml
[dependencies]
mu_epub = "0.2"
```

### Quick Start

```rust,no_run
use mu_epub::EpubBook;

fn main() -> Result<(), mu_epub::EpubError> {
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
use mu_epub::{EpubBook, ZipLimits};

let limits = ZipLimits::new(8 * 1024 * 1024, 1024); // explicit caps
let mut book = EpubBook::builder()
    .with_zip_limits(limits)
    .open("book.epub")?;
# Ok::<(), mu_epub::EpubError>(())
```

### Chapter Ergonomics

```rust,no_run
use mu_epub::EpubBook;

let mut book = EpubBook::open("book.epub")?;
for chapter in book.chapters() {
    println!("#{} {} ({})", chapter.index, chapter.idref, chapter.href);
}

let first_text = book.chapter_text(0)?;
println!("chars={}", first_text.len());
# Ok::<(), mu_epub::EpubError>(())
```

### Rendering Stack (Decoupled Crates)

`mu_epub` remains the EPUB parse/prep crate.
Rendering is split into:

1. `mu-epub-render`: render IR + layout engine + chapter-to-pages orchestration
2. `mu-epub-embedded-graphics`: `embedded-graphics` backend executor for render commands
3. `mu-epub-render-web`: self-contained HTML preview generator for rapid layout/font/TOC validation

Add local workspace deps:

```toml
[dependencies]
mu_epub = { path = "." }
mu-epub-render = { path = "crates/mu-epub-render" }
mu-epub-embedded-graphics = { path = "crates/mu-epub-embedded-graphics" }
mu-epub-render-web = { path = "crates/mu-epub-render-web" }
```

Prepare a chapter into backend-agnostic render pages:

```rust,no_run
use mu_epub::EpubBook;
use mu_epub_render::{RenderEngine, RenderEngineOptions};

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
use mu_epub_embedded_graphics::EgRenderer;

# use mu_epub::EpubBook;
# use mu_epub_render::{RenderEngine, RenderEngineOptions};
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
just web-preview tests/fixtures/bench/pg84-frankenstein.epub
```

Or export a standalone HTML snapshot:

```bash
just web-preview-export tests/fixtures/bench/pg84-frankenstein.epub target/web-preview/index.html
```

### CLI (Unix-Friendly)

Install from crates.io:

```bash
cargo install mu_epub --features cli --bin mu-epub
mu-epub --help
```

Inspect metadata and chapter lists:

```bash
mu-epub metadata book.epub --pretty
mu-epub chapters book.epub --ndjson
```

Extract chapter text for LLM/pipe workflows:

```bash
mu-epub chapter-text book.epub --index 0 --raw > chapter-0.txt
mu-epub toc book.epub --flat | jq .
```

Validate structure/compliance signals:

```bash
mu-epub validate book.epub --pretty
mu-epub validate book.epub --strict
```

#### Functional API

```rust,no_run
use mu_epub::{parse_epub_file_with_options, EpubBookOptions, ZipLimits};

let limits = ZipLimits::new(8 * 1024 * 1024, 1024);
let options = EpubBookOptions {
    zip_limits: Some(limits),
    ..EpubBookOptions::default()
};

let summary = parse_epub_file_with_options("book.epub", options)?;
println!("Title: {}", summary.metadata().title);
# Ok::<(), mu_epub::EpubError>(())
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
