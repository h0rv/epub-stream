# EPUB Compliance Matrix (80/20 Roadmap)

Last updated: 2026-02-08

This matrix tracks practical EPUB compatibility in `epub-stream` and focuses on
high-impact features first (80/20), while making gaps explicit.

Status legend:

- `pass`: implemented and covered by tests/fixtures.
- `partial`: implemented for common cases with known gaps.
- `planned`: not yet implemented.

## Container / Packaging

| Area | Status | Notes |
|---|---|---|
| ZIP container parse | pass | Streaming ZIP parser with configurable limits. |
| ZIP64 archives | planned | Explicitly rejected with `UnsupportedZip64`; read-only ZIP64 support is on roadmap. |
| `mimetype` validation | pass | `validate` checks required content and encoding. |
| `META-INF/container.xml` discovery | pass | Rootfile path extraction supported. |
| Multiple rootfiles handling | partial | First usable rootfile path flow; multi-rootfile semantics not exhaustive. |

## OPF / Manifest / Spine

| Area | Status | Notes |
|---|---|---|
| OPF parse (core metadata) | pass | Title/author/language and common DC fields. |
| Manifest extraction | pass | Core fields parsed and exposed. |
| Spine parse | pass | Itemrefs, linear/properties, EPUB2 `toc` attr. |
| Spine->manifest reference validation | pass | `validate` emits structured errors. |
| Duplicate/empty manifest id detection | pass | `validate` emits diagnostics. |
| Full OPF semantic validation | partial | Core structure checks; full spec-level semantic coverage still expanding. |

## Navigation

| Area | Status | Notes |
|---|---|---|
| EPUB3 nav parse | pass | TOC/page-list/landmarks parse supported. |
| EPUB2 NCX parse | pass | NCX fallback supported via spine `toc`. |
| Nav/NCX presence validation | pass | `validate` warns/errors for missing/broken docs. |
| Deep cross-reference validation | partial | Basic parse and existence checks; richer cross-link checks planned. |

## Content Readability

| Area | Status | Notes |
|---|---|---|
| Chapter lookup and iteration | pass | High-level ergonomic API available. |
| Chapter HTML/text extraction | pass | API and CLI support. |
| Tokenization for XHTML | pass | Common formatting/list structures supported. |
| Full HTML/CSS rendering parity | planned | Not a full browser engine. |

## Validation / Tooling

| Area | Status | Notes |
|---|---|---|
| Structured validator API | pass | `ValidationReport` + typed diagnostics. |
| CLI validator command | pass | `epub-stream validate [--strict]`. |
| Diagnostic code stability policy | partial | Codes added; policy docs can be tightened further. |
| Broad malformed corpus coverage | partial | Growing fixture set; still expanding edge cases. |
| Differential/fuzz validation | planned | Recommended next for confidence at scale. |

## 80/20 Next Steps

1. Expand validator diagnostics for package semantics (required manifest/media rules).
2. Add golden tests for `epub-stream validate` JSON output and diagnostic code stability.
3. Add corpus fixtures for tricky EPUB2/EPUB3 edge cases and regressions.
4. Add differential tests against `epub-rs`/`epub-parser` on shared corpora.
5. Add targeted fuzzing for ZIP/XML/tokenizer parsing paths.
6. Add ZIP64 fixtures to expectation corpus and implement read-only ZIP64 container parsing.
