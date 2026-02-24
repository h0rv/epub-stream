# Memory Management

The library allocates nothing it controls the size of. Every buffer is either
passed in by the caller or bounded by a limit the caller specified. Same code
paths on desktop and ESP32 — different constraints.

## Core Principle

This is the pattern shared by `embedded-hal`, `smoltcp`, and
`embedded-graphics`: the library defines behavior, the caller owns the memory.
If an EPUB exceeds a limit the library returns `Result::Err`, not a crash.

```rust
let limits = ZipLimits::new(8 * 1024 * 1024, 2048);
let mut book = EpubBook::builder()
    .with_zip_limits(limits)
    .open("book.epub")?;
```

On a desktop you set limits to 64MB and forget them. On an ESP32-C3 you set
them to what fits in 230KB of free heap. Same library, same code paths,
different constraints.

## Three-Tier Allocation Strategy

Each tier serves a different purpose. Use all three in the right places.

### Tier 1: Caller-Owned Reusable Buffers

The hot path — chapter navigation, page turns. The caller allocates once at
startup (while the heap is still contiguous), the library fills repeatedly.

```rust
// Allocate once during init, right after framebuffer
let mut chapter_buf = Vec::with_capacity(128 * 1024);
let mut scratch = ScratchBuffers::embedded();

// Every chapter navigation reuses the same memory — zero alloc, zero free
book.chapter_events_with_scratch(
    chapter_index,
    options,
    &mut chapter_buf,
    &mut scratch,
    |event| { /* process */ Ok(()) },
)?;
```

The `_into` suffix is the Rust convention: `chapter_html_into`,
`tokenize_html_into`, `read_resource_into`. The library calls `.clear()` then
fills. The Vec keeps its allocation — no alloc, no free, no fragmentation.

**This single pattern eliminates ~80% of fragmentation issues.**

#### Implemented `_into` / scratch APIs

| Method | Buffer type | Purpose |
|--------|------------|---------|
| `chapter_html_into` | `&mut String` | Read chapter XHTML |
| `chapter_text_into` | `&mut String` | Extract plain text |
| `tokenize_html_into` | `&mut Vec<Token>` | Parse into token buffer |
| `tokenize_html_with_scratch` | `&mut Vec<Token>` + `&mut TokenizeScratch` | Parse with full scratch reuse |
| `chapter_events_with_scratch` | `&mut Vec<u8>` + `&mut ScratchBuffers` | True zero-alloc streaming |
| `embedded_fonts_with_scratch` | `&mut Vec<u8>` | Font enumeration I/O buffer |
| `read_file_with_scratch` | `&mut [u8]` + `&mut [u8]` | ZIP entry with scratch I/O |
| `read_file_to_writer_with_scratch` | `&mut [u8]` + `&mut [u8]` | Chunked ZIP stream |

#### Scratch types with embedded/desktop presets

| Type | `embedded()` | `desktop()` | Purpose |
|------|-------------|------------|---------|
| `TokenizeScratch` | 4KB XML, 8KB text | 32KB XML, 64KB text | Tokenizer working memory |
| `ScratchBuffers` | 8KB read, 4KB XML | 65KB read, 32KB XML | Streaming chapter I/O |
| `ChunkLimits` | 4KB chunks, 2KB text acc | 16KB chunks, 8KB text acc | Streaming flow control |

### Tier 2: `heapless` for Fixed-Size Structures

Some structures have natural upper bounds small enough for compile-time sizing.
`heapless` collections are linker-visible — you know exactly how much memory
they use.

```rust
use heapless::Vec as HeaplessVec;

// Good: ZIP central directory has a natural upper bound
entries: HeaplessVec<CdEntry, 256>  // ~4KB, stack-safe
```

**Current heapless usage:** ZIP central directory (256 entries max).

**Rule of thumb:** use `heapless` for things where the bound is inherent to the
domain (language codes, media types, CD entries). Use `Vec::with_capacity` for
things where the bound is a resource limit that varies per device (token
streams, chapter content, image data).

**Don't go overboard.** `heapless` makes APIs harder to use, bakes assumptions
into the library, and adds const generics everywhere. Use it for small internal
fixed-size buffers, not main data paths.

### Tier 3: Arena Allocators (Not Yet Used)

When profiling shows that XML/CSS parsing temporaries cause fragmentation, a
bump allocator like `bumpalo` can eliminate it. All parsing temporaries for one
chapter are born together and die together — perfect for arena allocation.

```rust
use bumpalo::Bump;

let arena = Bump::with_capacity(32 * 1024);
let tokens = tokenizer.tokenize_in(&chapter_raw, &arena)?;
arena.reset(); // instant free of everything, zero fragmentation
```

**Status:** not implemented. The scratch buffer pattern has been sufficient so
far. Arena allocation is the next tool to reach for if profiling reveals
fragmentation from parsing temporaries.

## Limits

Every public API entry point accepts limits. Any `Vec` that could grow
unbounded gets checked before pushing. Any `read_to_end` gets capped.

### Limit types

| Struct | Key fields | Where |
|--------|-----------|-------|
| `ZipLimits` | `max_file_read_size`, `max_mimetype_size`, `max_eocd_scan` | ZIP layer |
| `TokenizeLimits` | `max_tokens` (100K), `max_nesting` (256), `max_text_bytes` (64KB) | Tokenizer |
| `StyleLimits` | `max_selectors` (4096), `max_css_bytes` (512KB), `max_nesting` (32) | CSS/style |
| `FontLimits` | `max_faces` (64), `max_bytes_per_font` (8MB), `max_total_font_bytes` (64MB) | Font loader |
| `MemoryBudget` | `max_entry_bytes` (4MB), `max_css_bytes` (512KB), `max_nav_bytes` (512KB), `max_pages_in_memory` (128) | Render prep |
| `ChunkLimits` | `max_read_chunk` (16KB/4KB), `max_text_accumulation` (8KB/2KB) | Streaming |
| `ImageRegistryLimits` | `max_images`, `max_total_pixels` | Embedded renderer |

Defaults in parentheses. Embedded presets available via `::embedded()` where
applicable.

### Limit behavior

Most limits return `Err` immediately. Text extraction truncates safely on
UTF-8 boundaries — better to show a truncated chapter than refuse to open the
book.

| Limit | Behavior |
|-------|----------|
| Token count exceeded | `Err(TokenizeError)` |
| Text node too large | **Truncate** to `max_text_bytes` |
| Chapter HTML too large | **Truncate** on UTF-8 boundary |
| Chapter text too large | **Truncate** on UTF-8 boundary |
| CSS/nesting/selectors exceeded | `Err` |
| ZIP file too large | `Err(ZipError::FileTooLarge)` |
| Image registry full | `Err(ImageRegistryError)` |

## Images

Images are the hardest part on constrained devices. A 480x800 grayscale PNG
might be 384KB uncompressed — exceeding the entire free heap.

**Current approach:** images flow as reference strings (`src`, `alt`,
dimensions) through the pipeline. Pixel data is only materialized in the
embedded-graphics `ImageRegistry` with strict slot and pixel-budget limits.
Unregistered images fall back to deterministic placeholders (outline or
outline+alt-text).

**Target architecture:** streaming decode with on-the-fly downscale, writing
scan lines directly to the framebuffer. At any point, hold at most 2-3 rows
of pixels — not the full image.

```
compressed bytes -> streaming decoder (one row)
    -> downscale to display width
    -> dither to 1-bit
    -> write row to framebuffer
```

The library exposes image data as a `Read` stream (compressed bytes from ZIP).
The rendering crate handles decode-to-display. Memory used: one or two row
buffers regardless of image size.

**Gap:** real streaming image decode is not yet implemented (tracked as
`EMB-001` in the [embedded render tracker](embedded-render-tracker.md)).

## Streaming Architecture

Chapter content never materializes fully in memory. ZIP reading uses streaming
decompression with `miniz_oxide::InflateState`, processing in bounded chunks.

```
ZIP entry (compressed)
  -> read_file_to_writer_with_scratch (chunked decompression)
    -> chapter_events_with_scratch (chunked XML processing)
      -> callback per event (render, layout, etc.)
```

Each chunk reads `ChunkLimits::max_read_chunk` bytes, processes events, and
yields. The caller controls flow with the callback.

## Rules

1. **Never allocate in a per-chapter or per-page function.** No `Vec::new()`,
   `String::new()`, `.clone()`, `.collect()`, `.to_string()` on any path
   called repeatedly.

2. **Caller owns buffers.** Functions take `&mut Vec<T>` or `&mut Scratch` and
   write into them. Caller reuses across calls.

3. **`.clear()`, never drop + recreate.** Resets length, keeps capacity. The
   memory stays in place.

4. **Stream large data.** Never load a full chapter or stylesheet into memory.
   Process in bounded chunks, render, discard.

5. **Return `Result`, never panic.** Every capacity limit produces
   `EpubError::LimitExceeded` or `EpubError::BufferTooSmall`.

6. **Use `heapless` for small internal buffers with known bounds.** ZIP central
   directory, element stacks, attribute parsing. Never in public API.

7. **`Vec::with_capacity` + never exceed = one allocation, zero fragmentation.**
   Don't avoid `Vec`. Create with known capacity from limits, check before
   growing.

8. **Allocate early, reuse forever.** On firmware, allocate buffers once during
   init right after the framebuffer while heap is contiguous. They sit there
   for the app lifetime.

## Audit Checklist

- [ ] No `Vec::new()` / `String::new()` in any per-chapter or per-page function
- [ ] No `.clone()` on token streams, styled events, or other large structures
- [ ] No `.collect::<Vec<_>>()` in rendering or pagination loops
- [ ] All scratch buffers reused via `.clear()`
- [ ] All capacity limits return `Result`
- [ ] Streaming chunk size configurable (4KB embedded / 16KB std)
- [ ] Every `_into` method calls `.clear()` before filling
- [ ] No per-glyph heap allocation in draw loop
- [ ] Image pipeline streams rows, never holds full decoded image

## Gap Analysis

Areas where the codebase doesn't yet fully follow these patterns:

| Gap | Current state | Target | Priority |
|-----|--------------|--------|----------|
| Image streaming decode | Reference strings + registry with pixel-budget limits | Row-by-row decode through `ImageSink` trait | P0 (`EMB-001`) |
| TTF glyph rasterization | Falls back to mono backend | Real glyph metrics + raster from registered faces | P0 (`EMB-002`) |
| Layout measurement parity | Heuristic width estimates | Backend-consistent measurer | P0 (`EMB-004`) |
| Arena for parse temps | Not used | `bumpalo` if profiling shows temp fragmentation | P2 (profile first) |
| Unified `EpubLimits` preset | Limits spread across separate structs | Single `EpubLimits` entry point with `embedded_small()` / `desktop()` | Nice-to-have |

## Hardening Loop

- `just fmt` — auto-format.
- `just check` — workspace type-check.
- `just lint` — Clippy with warnings denied.
- `just test` — fast unit tests.
- `just all` — canonical single command.
- `just ci` — CI gate (`just all` + integration).
- `just testing lint-memory` — `no_std` core/alloc discipline + renderer allocation-intent checks.
- `just testing test-alloc` — allocation-counter stress tests.
- `just testing test-embedded` — tiny-budget embedded-path tests.
- `just analysis mem-profile` — heap allocation profiling (xctrace/heaptrack).
- `just analysis analyze-static` — binary bloat + symbol analysis.
- `just analysis dataset-profile-gutenberg` — per-book timings across corpus.
