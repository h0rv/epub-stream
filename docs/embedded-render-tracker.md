# Embedded EPUB Rendering Production Tracker

Last updated: 2026-02-21

## Scope
This tracker covers remaining work to make embedded rendering production-grade across:

- `mu_epub`
- `mu-epub-render`
- `mu-epub-embedded-graphics`

## Status Legend

- `done`: implemented, tested, and documented.
- `partial`: implemented for common paths, with known gaps.
- `missing`: not implemented.
- `defer`: intentionally postponed for a later tier.

## Current Baseline

- Streaming parse + bounded prep APIs exist.
- Render pipeline split is in place (`mu-epub-render` + `mu-epub-embedded-graphics`).
- Web reflow regression harness exists for config mapping and text-boundary checks.
- Embedded tiny-budget and allocation tests exist and are passing locally (`just test-embedded`, `just test-alloc`).
- Embedded renderer now exposes deterministic fallback/budget diagnostics with low-overhead counters.
- Embedded low-RAM reflow/page-turn stress loops now run in the regression matrix.
- Render corpus regression harness now covers bench fixtures across multiple pagination profiles with deterministic invariant checks.

Known blockers in current code:

- TTF backend draw path still falls back to mono rasterization for glyph drawing.
- Image commands without registered bitmap payload still use deterministic placeholder fallback (outline or outline+label), not full source decode/raster.
- SVG vector rasterization remains unsupported in embedded backend; current render path relies on deterministic image/alt-text fallback policy.
- Default layout still uses heuristic width when no explicit measurer is injected.

## Memory-First Contract (Required For Every New Feature)

Every item below must follow `docs/memory-management.md`.

- [ ] Public APIs use caller-owned buffers or reusable scratch types on repeated paths.
- [ ] Every heavy path has explicit limits in options structs (bytes/items/pages/faces).
- [ ] No hidden per-page/per-line/per-glyph allocation in hot loops.
- [ ] Fallible growth uses `try_reserve` + `Result` errors, never panic.
- [ ] Streaming/chunked processing is used for large resources (fonts, images, stylesheets).
- [ ] A dedicated allocation regression test is added or updated for the new path.
- [ ] Docs include budget guidance and failure-mode behavior.

## Missing Features Backlog

### P0 Must-Have (production blocker)

- [ ] `EMB-001` Real embedded image rendering (`partial`)
  Current: image commands can render registered monochrome bitmaps; unresolved sources fallback to explicit policy-driven placeholders (`OutlineOnly` or `OutlineWithAltText`).
  Done when: decode and render core EPUB image types (PNG, JPEG, GIF, WebP where available) with deterministic scaling.
  Memory constraints: row/tile decode, bounded scratch, hard pixel and byte caps.
  Required tests: decode fixtures, pagination with images, allocation counter checks, out-of-budget failure behavior.

- [ ] `EMB-002` Non-fallback TTF backend (`partial`)
  Current: TTF backend now supports bounded registration + style/weight selection + metrics status, but draw path still falls back to mono raster.
  Done when: registered faces provide real glyph metrics + raster output for layout/render parity.
  Memory constraints: bounded face registry, bounded glyph cache, no per-glyph heap allocation in draw loop.
  Required tests: font registration limits, glyph rendering golden tests, allocation stability over repeated page turns.

- [ ] `EMB-003` Font fallback chain with reasoned resolution (`partial`)
  Current: fallback reason codes exist, but effective rendering still collapses to mono path for unresolved cases.
  Done when: fallback chain is deterministic, glyph-aware, and visible in trace output.
  Memory constraints: bounded fallback search depth and bounded metadata storage.
  Required tests: fallback-order tests, missing-glyph tests, style/weight/italic nearest-match tests.

- [ ] `EMB-004` Layout/renderer measurement parity (`partial`)
  Current: default layout width estimates are heuristic and can diverge from backend glyph widths.
  Done when: embedded rendering paths use measurer parity guarantees so text does not clip at right edge after reflow.
  Progress: `EgTextMeasurer` adapter plus `with_embedded_text_measurer(RenderConfig)` helper now drive embedded regression and visualize paths with backend-consistent width metrics.
  Memory constraints: reusable measurer state and fixed-size caches.
  Required tests: right-edge clipping invariants over font-size/family/spacing matrix.

- [x] `EMB-005` Embedded dynamic reflow regression matrix (`done`)
  Current: dedicated embedded matrix regression test now runs in `mu-epub-embedded-graphics` and is wired into `just render-regression`.
  Done when: embedded-oriented regression matrix covers font size, line spacing, justification, family override, viewport changes.
  Memory constraints: fixture-driven tests with strict page/item limits.
  Required tests: chapter page count monotonicity, page index monotonicity, progress monotonicity, no-overflow checks.

- [x] `EMB-006` Persistent page-map cache for fast reopen (`done`)
  Current: durable file-backed cache store exists with deterministic profile/chapter keying, schema versioning, and bounded file-size caps.
  Done when: chapter page maps persist by pagination profile and invalidate safely on config/content changes.
  Memory constraints: streaming encode/decode, bounded entry sizes, bounded in-memory page windows.
  Required tests: reopen latency sanity, cache-hit equivalence tests, invalidation tests.

- [x] `EMB-007` TOC/locator to rendered page mapping (`done`)
  Current: `RenderBookPageMap` provides compact chapter page spans plus `resolve_href`/`resolve_toc_href` APIs and optional fragment-progress mapping for anchor-aware page targeting.
  Done when: jump-to-TOC lands on deterministic page offsets and remains stable after bounded reflow.
  Memory constraints: compact index structure and bounded locator table.
  Required tests: TOC jump accuracy, chapter boundary transitions, reflow remap correctness.

- [x] `EMB-008` Reflow-safe reading position retention (`done`)
  Current: `RenderReadingPositionToken` APIs (`reading_position_token_for_page_index` + `remap_reading_position_token`) preserve chapter/global progress with chapter-href hints across reflow profile changes.
  Done when: nearest logical position survives re-render and resumes on equivalent content.
  Memory constraints: compact persisted locator representation.
  Required tests: reflow while on middle pages, chapter jumps, resume after reopen.

- [x] `EMB-009` Embedded renderer no-std/low-RAM verification matrix (`done`)
  Current: embedded regression suite now includes constrained-budget repeated reflow/page-turn loops with stability assertions and panic-free verification.
  Done when: render crates have explicit compile/test gates for constrained profiles.
  Memory constraints: documented stack/heap expectations per profile.
  Required tests: profile-specific compile checks and small-budget runtime suites.

- [x] `EMB-010` Feature-level memory budget telemetry (`done`)
  Current: renderer exposes image-registry pressure diagnostics, image fallback counters, and text fallback reason counters through structured APIs.
  Done when: feature paths expose diagnostics for limit pressure and failure reasons.
  Memory constraints: counters/telemetry implemented without per-event allocation churn.
  Required tests: budget-overrun diagnostics and structured error coverage.

### P1 High-Value (after blockers)

- [x] `EMB-011` SVG support policy (`done`)
  Current: prep emits SVG `<image>` (`xlink:href`/`href`) events and layout/backend apply deterministic fallback policy (`svg_mode` + `alt_text_fallback` + embedded image placeholder policy).
  Done when: either deterministic raster fallback or explicit alt-text fallback policy per device profile.

- [ ] `EMB-012` CSS subset expansion for ebook realism (`partial`)
  Current: CSS subset covers font family/size/weight/style, text align, line height, letter-spacing (`px` + `normal`), and paragraph margins with stylesheet+inline precedence tests.
  Done when: high-impact properties used by common EPUBs are covered with deterministic limits.

- [ ] `EMB-013` RTL/BiDi baseline support (`missing`)
  Done when: right-to-left paragraph flow and punctuation placement pass basic mixed-direction fixtures.

- [x] `EMB-014` Table rendering strategy (`done`)
  Current: prep linearizes table rows/cells into deterministic paragraph-safe fallback text flow with row boundaries and per-cell separators.
  Done when: readable table fallback (stacked or simplified layout) is implemented with bounded memory.

- [ ] `EMB-015` Hyphenation dictionary integration (`missing`)
  Done when: optional dictionary path improves breaks while preserving deterministic bounded behavior.

- [ ] `EMB-016` Robust corpus regression at scale (`partial`)
  Current: `mu-epub-render` corpus harness now discovers bench fixtures dynamically and validates multi-profile invariants (right-edge safety, monotonic progress, page metrics sanity, and sampled page-range consistency).
  Done when: large fixture corpus includes layout/render invariants and expected-failure baselines.

### P2 Strategic (defer until P0/P1 stable)

- [ ] `EMB-017` Optional complex-script shaping tier (`defer`)
  Done when: feature-gated path supports richer script shaping with clear CPU/RAM tradeoffs.

- [ ] `EMB-018` Offline font/image preprocessing pipeline (`defer`)
  Done when: optional preprocessing reduces on-device memory and startup cost with compatible cache schema.

## Regression/Test Harness Plan

Required gate set for production progression:

- `just test-embedded`
- `just embedded-low-ram-matrix`
- `just embedded-budget-telemetry`
- `just test-alloc`
- `cargo test -p mu-epub-embedded-graphics`
- `cargo test -p mu-epub-render --test corpus_regression_harness`
- `cargo test -p mu-epub-render --test typography_regression`
- `cargo test -p mu-epub-render-web --bin web-preview`
- `just lint-memory`
- `just check-no-std-layout`

Planned additions to make this tracker enforceable:

- [ ] Add embedded reflow matrix tests parallel to web-preview regression matrix.
- [ ] Add golden render snapshots for embedded backend across font/image combinations.
- [x] Add stress tests for repeated reflow/page-turn loops under tight budgets.

## Exit Criteria For "Great Embedded EPUB Rendering"

All `P0` items are `done`, at least four `P1` items are `done`, and the full gate set passes on CI with no ignored failures for embedded regression suites.
