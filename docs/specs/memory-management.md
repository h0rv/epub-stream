# Memory Management Guide

Target: <32KB RAM, zero hidden allocations on hot paths.

## Rules

1. **Never allocate in a per-chapter or per-page function.** No `Vec::new()`, `String::new()`, `.clone()`, `.collect()`, `.to_string()` on any path called repeatedly.

2. **Caller owns buffers.** Functions take `&mut Vec<T>` or `&mut Scratch` and write into them. Caller reuses across calls.

3. **`.clear()`, never drop + recreate.** `clear()` resets length but keeps allocated capacity. The memory stays in place.

4. **Stream large data.** Never load a full chapter or stylesheet into memory. Process in bounded chunks, render, discard.

5. **Return `Result`, never panic.** Every capacity limit produces `EpubError::LimitExceeded` or `EpubError::BufferTooSmall`.

6. **Use `heapless` for small internal buffers with known bounds** (element stacks, attribute parsing, selector lists). Never in public API.

## Patterns

### Buffer reuse

```rust
// Function signature: caller provides output + scratch
fn tokenize_chapter(
    &self,
    input: &[u8],
    tokens: &mut Vec<Token>,
    scratch: &mut TokenizeScratch,
) -> Result<(), EpubError> {
    tokens.clear();
    scratch.clear();
    // ... write into tokens and scratch ...
}

// Call site: allocate once, reuse across chapters
let mut tokens = Vec::with_capacity(512);
let mut scratch = TokenizeScratch::new();

for i in 0..book.chapter_count() {
    book.tokenize_chapter(i, &mut tokens, &mut scratch)?;
    render(&tokens);
}
```

### Render with caller-owned chapter bytes

```rust
let mut chapter_buf = Vec::with_capacity(128 * 1024);
let chapter = book.chapter(chapter_index)?;
chapter_buf.clear();
book.read_resource_into_with_hard_cap(
    &chapter.href,
    &mut chapter_buf,
    render_opts.prep.memory.max_entry_bytes,
)?;

engine.prepare_chapter_bytes_with(
    &mut book,
    chapter_index,
    &chapter_buf,
    |page| consume_page(page),
)?;

// Optional low-RAM mode: skip EPUB embedded-font registration per run.
engine.prepare_chapter_with_config(
    &mut book,
    chapter_index,
    RenderConfig::default().with_embedded_fonts(false),
    |page| consume_page(page),
)?;
```

### Scratch structs

```rust
pub struct TokenizeScratch {
    pub xml_buf: Vec<u8>,
    pub text_buf: String,
    pub element_stack: heapless::Vec<ElementType, 256>,
}

impl TokenizeScratch {
    pub fn new() -> Self { /* ... */ }

    pub fn clear(&mut self) {
        self.xml_buf.clear();
        self.text_buf.clear();
        self.element_stack.clear();
    }
}
```

### Fallible allocation

```rust
tokens.try_reserve(needed)
    .map_err(|_| EpubError::LimitExceeded {
        kind: LimitKind::TokenBuffer,
        actual: tokens.len() + needed,
        limit: max_tokens,
    })?;
```

### heapless for bounded internals

```rust
use heapless::Vec as HVec;

// Good: domain has a natural upper bound
let mut element_stack: HVec<ElementType, 256> = HVec::new();
element_stack.push(el).map_err(|_| EpubError::LimitExceeded { .. })?;

// Bad: chapter size is unbounded, use alloc::Vec with scratch pattern
let mut chapter_tokens: HVec<Token, ???> = HVec::new(); // don't do this
```

## Audit Checklist

- [ ] No `Vec::new()` / `String::new()` in any per-chapter or per-page function
- [ ] No `.clone()` on token streams, styled events, or other large structures
- [ ] No `.collect::<Vec<_>>()` in rendering or pagination loops
- [ ] All scratch buffers reused via `.clear()`
- [ ] All capacity limits return `Result`
- [ ] Streaming chunk size configurable, defaults to 4KB embedded / 16KB std

## Local Hardening Loop

- `just fmt`
: auto-format source.
- `just check`
: workspace type-check with `all-features`.
- `just lint`
: workspace Clippy with warnings denied.
- `just test`
: fast default unit-test loop (`--lib --bins`).
- `just all`
: canonical single command (`fmt` + `check` + `lint` + `test`).
- `just ci`
: CI gate (`just all` + `test-integration`).
- `just testing lint-memory`
: optional deep memory linting (`no_std` core/alloc discipline + renderer allocation-intent checks).
- `just testing test-alloc`
: optional allocation-counter stress tests (`--ignored`, serialized threads).
- `just testing test-embedded`
: optional tiny-budget embedded-path tests (`--ignored`).
- `just analysis dataset-profile-gutenberg`
: generates `target/datasets/gutenberg-smoke-*.csv` with per-book timings for `validate`, `chapters`, and `chapter-text --index 0`.
  Known corpus-specific failures can be tracked in `scripts/datasets/gutenberg-smoke-expectations.tsv` and are reported as `expected_fail`.
