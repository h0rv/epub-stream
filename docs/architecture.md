# Architecture

Goal: A production EPUB reader library with real typography, chapter navigation,
page counts, font size/family switching, and persistence -- all within tight
memory limits. Designed for `no_std` environments with `alloc`, but works on
any platform. Full language support is gated behind an optional feature flag;
the default path is Latin-only for fast bring-up.

Non-goals (v1): full CSS2/3 layout, JavaScript, SVG, MathML, audio/video,
complex floats/tables.

## Pipeline

```
EPUB (.epub file)
  |
  v
1. Streaming ZIP reader (bounded buffer, miniz_oxide)
  |
  v
2. container.xml -> content.opf (quick-xml, SAX-style)
  |
  v
3. XHTML tokenizer -> token stream
  |
  v
4. Layout engine: tokens -> line breaks -> pages
  |
  v
5. Renderer: render IR -> backend (embedded-graphics / web preview)
```

Decoupling parsing from layout via the token stream means reflow (font size
change, font switch) only re-runs steps 4-5.

## Library Stack

| Purpose         | Library                     | Notes                                  |
|-----------------|-----------------------------|----------------------------------------|
| ZIP             | Custom streaming + `miniz_oxide` | Bounded buffer, EOCD, Stored + DEFLATE, CRC32 |
| XML/XHTML       | `quick-xml`                 | Pull parser, SAX-style streaming       |
| Small buffers   | `heapless`                  | Fixed-capacity collections for bounded internals |
| CRC             | `crc32fast`                 | ZIP integrity verification             |
| Async (optional)| `tokio`                     | Async file-open helpers                |

## Data Model

### Token stream

Each chapter is tokenized into a stream of:
- `Text(string)`
- `ParagraphBreak`
- `Heading(level)`
- `ListStart(ordered)`, `ListItemStart/End`, `ListEnd`
- `Emphasis(bool)`, `Strong(bool)`
- `LinkStart(href)`, `LinkEnd`
- `Image { src, alt }`
- `LineBreak`

## CSS Subset (v1)

Supported: `font-size` (px, em), `font-family`, `font-weight` (normal/bold),
`font-style` (normal/italic), `text-align`, `line-height`, `margin-top/bottom`.

Selectors: tag, class, and inline style only.

Everything else is ignored.

## Fonts

Font handling is in the `epub-stream-embedded-graphics` backend:

1. Default mono backend (always available).
2. TTF font registration with bounded face registry and style/weight selection.
3. Embedded `@font-face` discovery from EPUB resources with size caps.
4. Fallback chain with explicit reason codes for resolution decisions.

## Memory Budget

| Component              | Budget    |
|------------------------|-----------|
| ZIP + XML buffers      | 8-16 KB   |
| Chapter token cache    | 20-32 KB  |
| Glyph cache            | 24-32 KB  |
| Layout state           | 8 KB      |
| Metadata + spine       | 4-8 KB    |
| **Total (excl. framebuffer)** | **<120 KB** |

## Performance Targets

| Operation        | Target  |
|------------------|---------|
| Open (first)     | <2s     |
| Open (cached)    | <1s     |
| Page turn        | <200ms  |
| Font size change | <5s     |

## Risks

- Large embedded fonts: enforce size cap, fall back.
- Complex scripts: shaping adds CPU/RAM; behind optional feature flag.
- CSS complexity: only a subset is supported.
- Storage performance: keep reads aligned, cache headers.
