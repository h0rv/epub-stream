# EPUB Spec Compliance

Target: EPUB 3.2 subset (structure is forward-compatible with 3.3).
EPUB 2.0 fallbacks (NCX, guide element) are implemented for compatibility with
older files.

Status key: **done** | **partial** | **--** (not started) | **n/a** (out of scope for v1)

## Container (OCF)

| Feature                          | Status  | Notes                                                        |
|----------------------------------|---------|--------------------------------------------------------------|
| ZIP container reading            | done    | Streaming, bounded buffer, EOCD, Stored + DEFLATE, CRC32    |
| `mimetype` file validation       | done    | `validate_mimetype()` checks content = `application/epub+zip` |
| `META-INF/container.xml` parsing | done    | Extracts rootfile `full-path`                                |
| Encryption (`encryption.xml`)    | n/a     |                                                              |
| Digital signatures               | n/a     |                                                              |

## Package Document (OPF)

| Feature                                | Status  | Notes                                                   |
|----------------------------------------|---------|---------------------------------------------------------|
| `<metadata>` (title, author, language) | done    | dc:title, dc:creator, dc:language                       |
| Full Dublin Core metadata              | done    | date, publisher, rights, description, subjects, identifier |
| `<manifest>` (resource list)           | done    | id, href, media-type, properties (max 64)               |
| `<spine>` (reading order)              | done    | idref, id, linear, properties (max 256)                 |
| Cover image detection                  | done    | EPUB 2.0 meta tag + EPUB 3.x `cover-image`             |
| EPUB-specific metadata                 | done    | `dcterms:modified`, `rendition:layout`                  |
| `<guide>` (EPUB 2.0, deprecated)       | done    | Parses `<reference>` with type, title, href             |
| Media overlay references               | n/a     |                                                         |

## Navigation

| Feature                          | Status  | Notes                                                    |
|----------------------------------|---------|----------------------------------------------------------|
| XHTML nav (`epub:type="toc"`)    | done    | Nested `<ol>/<li>/<a>` parsing with hierarchical output  |
| NCX (`toc.ncx`)                  | done    | `<navMap>` with nested `<navPoint>` support              |
| Page list                        | done    | XHTML `epub:type="page-list"` + NCX `<pageList>`        |
| Landmarks                        | done    | XHTML `epub:type="landmarks"`                            |

## Content Documents

| Feature                          | Status  | Notes                                                    |
|----------------------------------|---------|----------------------------------------------------------|
| XHTML tokenization (SAX)         | done    | No DOM; quick-xml pull parser                            |
| Paragraphs (`<p>`)               | done    | Emits `ParagraphBreak`                                   |
| Headings (`<h1>`-`<h6>`)         | done    | Emits `Heading(level)`                                   |
| Emphasis (`<em>`, `<i>`)         | done    | Emits `Emphasis(bool)`, nesting supported                |
| Strong (`<strong>`, `<b>`)       | done    | Emits `Strong(bool)`, nesting supported                  |
| Line breaks (`<br>`)             | done    | Emits `LineBreak`                                        |
| Block containers (`<div>`)       | done    | Treated as block (emits `ParagraphBreak`)                |
| Inline containers (`<span>`)     | done    | Transparent, text extracted                              |
| Skipped elements                 | done    | script, style, head, nav, header, footer, aside, noscript |
| Lists (`<ul>`, `<ol>`, `<li>`)   | done    | `ListStart(ordered)`, `ListItemStart/End`, `ListEnd`     |
| Links (`<a>`)                    | done    | `LinkStart(href)` / `LinkEnd`; no-href treated as generic |
| Images (`<img>`)                 | done    | `Image { src, alt }`; missing src skipped                |
| Tables                           | n/a     |                                                          |
| SVG content documents            | n/a     |                                                          |
| MathML                           | n/a     |                                                          |
| JavaScript / forms               | n/a     |                                                          |
| Audio / video                    | n/a     |                                                          |

## CSS (Subset)

| Feature                          | Status  | Notes                                                    |
|----------------------------------|---------|----------------------------------------------------------|
| `font-size` (px, em)             | done    | `FontSize::Px` / `FontSize::Em`                         |
| `font-family`                    | done    | Strips quotes, first family                              |
| `font-weight` (normal, bold)     | done    | Also numeric: 400=normal, 700/800/900=bold               |
| `font-style` (normal, italic)    | done    | Also `oblique` maps to italic                            |
| `text-align`                     | done    | left, center, right, justify                             |
| `line-height`                    | done    | px values                                                |
| `margin-top`, `margin-bottom`    | done    | px values; `margin` shorthand (single value)             |
| Inline styles                    | done    | `parse_inline_style()` for `style=""` attributes         |
| Tag / class selectors            | done    | Tag, `.class`, `tag.class` selectors                     |
| Stylesheet resolution            | done    | `Stylesheet::resolve()` cascades matching rules          |
| Complex selectors                | n/a     |                                                          |
| Floats / positioning / grid      | n/a     |                                                          |

## Layout

| Feature                          | Status  | Notes                                                    |
|----------------------------------|---------|----------------------------------------------------------|
| Greedy line breaking             | done    | Word-level greedy in `layout.rs`                         |
| Multi-page pagination            | done    | Page/Line/TextStyle model                                |
| Heading spacing                  | done    | Extra space before headings, always bold                 |
| Paragraph spacing                | done    | Half-line gap between paragraphs                         |
| Style tracking                   | done    | Normal, Bold, Italic, BoldItalic                         |
| List layout                      | done    | Bullets (•) / numbered (1. 2. 3.), nested indentation    |
| Image placeholders               | done    | `[Image: alt]` or `[Image]` placeholder text             |
| Link rendering                   | done    | Text flows normally; link tokens are informational       |
| Page map persistence             | --      |                                                          |
| Fixed layouts                    | n/a     |                                                          |
| Spreads (two-page view)          | n/a     |                                                          |
| Bidirectional text (RTL)         | --      | Behind `epub_full` flag                                  |
| Ruby annotations                 | n/a     |                                                          |

## Error Handling

| Feature                          | Status  | Notes                                                    |
|----------------------------------|---------|----------------------------------------------------------|
| Unified `EpubError` type         | done    | Wraps ZIP, parse, navigation, CSS, I/O errors            |
| `From` conversions               | done    | `ZipError`, `TokenizeError` → `EpubError`                |

## Fonts

| Feature                          | Status  | Notes                                          |
|----------------------------------|---------|-------------------------------------------------|
| Built-in fonts                   | partial | Mono backend default; TTF registration + metrics available |
| User fonts from storage           | partial | Size cap ~200KB; registration API exists       |
| Embedded fonts from EPUB         | partial | `@font-face` discovery + bounded loading       |
| Font fallback chain              | partial | Reason codes exist; draw falls back to mono    |
| Complex script shaping           | --      | Behind `epub_full` flag                        |

## Test Coverage

Comprehensive test suite across unit tests, integration tests, memory budget
tests, fragmentation simulation, corpus stress tests, firmware path replica,
embedded reflow regression, and typography regression harnesses.

Run `just test` for unit tests, `just test-integration` for the full suite,
or `just validate` for the complete pre-merge gate.

## Next Steps (80/20 Roadmap)

1. Expand validator diagnostics for package semantics (required manifest/media rules).
2. Add golden tests for `epub-stream validate` JSON output and diagnostic code stability.
3. Add corpus fixtures for tricky EPUB2/EPUB3 edge cases and regressions.
4. Add differential tests against `epub-rs`/`epub-parser` on shared corpora.
5. Add targeted fuzzing for ZIP/XML/tokenizer parsing paths.
6. Add ZIP64 fixtures to expectation corpus and implement read-only ZIP64 container parsing.

## References

- EPUB 3.3: https://www.w3.org/TR/epub-33/
- EPUB 3.3 Reading Systems: https://www.w3.org/TR/epub-rs-33/
- EPUBCheck (validator): https://github.com/w3c/epubcheck
- W3C test suite: https://w3c.github.io/epub-tests/
