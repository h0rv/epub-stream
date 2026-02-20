# MU-EPUB Renderer Plan and Spec

This document tracks completed renderer work and defines the active generic rendering spec for:

- `mu-epub`
- `mu-epub-render`
- `mu-epub-embedded-graphics`
- `mu-epub-render-web` (preview tooling backend)

No app/device/project-specific behavior is part of this contract.

## Status Snapshot

## Completed

1. Crate split is in place:
- `mu-epub-render` exists as a separate crate for IR/layout/orchestration.
- `mu-epub-embedded-graphics` exists as a separate crate for embedded-graphics execution.

2. Workspace wiring is complete:
- Workspace members include all three crates.
- Build/test/lint commands run across the split crates.

3. Initial render stack APIs are present:
- `RenderEngine::prepare_chapter(...)` in `mu-epub-render`.
- `LayoutConfig` and `LayoutEngine` in `mu-epub-render`.
- Render IR commands and page model in `mu-epub-render`.
- `EgRenderer` command execution in `mu-epub-embedded-graphics`.

4. Backend naming and references are complete:
- Backend crate renamed to `mu-epub-embedded-graphics`.
- README/justfile/docs references updated.

5. Web-preview backend is available:
- `mu-epub-render-web` provides an interactive web UI + local render API for TOC/image/font/layout validation with live option re-rendering.

5. Validation gates are passing:
- `just all` (format/lint/check/tests/docs/cli).
- `just render-all` (render crates check/lint/test).

## In Progress

1. Generic renderer API expansion from `API_ISSUES.md`:
- high-priority items `#9-#15`
- medium-priority items `#16-#20`

## Remaining Major Work

1. Complete remaining items in `API_ISSUES.md` under "Additional API Gaps (Requested)".
2. Align docs/spec status with implemented vs planned APIs before release.

## MU-EPUB Renderer Spec (Generic Only)

## Scope

Implement high-fidelity EPUB rendering in mu-epub ecosystem only:

- `mu-epub`
- `mu-epub-render`
- `mu-epub-embedded-graphics`

No app/device/project-specific behavior in this spec.

## Goals

1. Production-grade typography and layout for common EPUBs.
2. Deterministic, testable rendering pipeline.
3. Streaming-friendly, memory-bounded operation for embedded constraints.
4. Strong API ergonomics for both simple and advanced consumers.

## Architecture Contract

1. `mu-epub`:

- EPUB parsing, metadata/spine/resources.
- CSS/style/font discovery + computed style stream.

2. `mu-epub-render`:

- Layout/pagination engine from styled stream -> backend-agnostic draw IR.

3. `mu-epub-embedded-graphics`:

- Draw IR execution on embedded-graphics targets with pluggable font backend.

4. `mu-epub-render-web`:

- Web-preview tooling that executes render IR into a browser-facing, self-contained HTML artifact.

## Required APIs

In `mu-epub` (render_prep/book layer):

1. `prepare_chapter_with(...)` streaming API (already present; keep stable).
2. `prepare_chapter_with_trace_context(...)` with structured font/style trace.
   - `prepare_chapter_with_trace(...)` remains as deprecated compatibility wrapper.
3. Preserve resolved face identity in styled output (not recomputed later).
4. Structured error context:

- resource `href`/path
- selector/declaration index
- token/source context.

In `mu-epub-render`:

1. `RenderEngine::prepare_chapter(...) -> Vec<RenderPage>`.
2. `RenderEngine::prepare_chapter_with(...)` page streaming callback.
3. `LayoutConfig` with all typography knobs:

- margins
- paragraph/list/heading gaps
- indent policies
- justification thresholds
- line-height controls
- soft-hyphen policy.

4. IR must include resolved font identity in text commands:

- `font_id` (or equivalent stable handle)
- not just weight/italic guesses.

5. Header/footer/progress must be represented in IR as commands, not backend special-casing.

In `mu-epub-embedded-graphics`:

1. Font backend abstraction trait:

- register face(s)
- map `font_id` -> glyph metrics/rasterization
- fallback chain with reason codes.

2. Default mono backend (current behavior) and optional TTF backend.
3. Consistent rendering path for justified/non-justified text using same font backend.
4. Zero-allocation (or amortized reusable buffer) glyph drawing in hot paths.

## Typography and Layout Requirements

1. Correct block semantics:

- paragraph
- heading(level)
- list item
- line break.

2. Justification resolved in layout stage only, deterministic in IR.
3. First-line indent policy configurable and role-aware.
4. Post-heading paragraph indent suppression configurable.
5. Soft hyphen handling:

- invisible when not used
- visible hyphen when break occurs at SHY.

6. Whitespace normalization must preserve preformatted/significant sections.
7. CSS cascade precedence deterministic and covered by tests.
8. Inline style and class-aware resolution supported within declared limits.

## Font System Requirements

1. Family normalization + dedupe.
2. Nearest-match resolution across weight/style.
3. Embedded `@font-face` support with bounded limits.
4. Explicit fallback trace:

- requested families
- chosen face
- rejected candidates with reason (`missing_glyph`, `weight_unavailable`, `policy_clamp`).

5. Public API for consumers to inject default families and fallback order.

## Performance/Memory Requirements

1. Streaming-first APIs for prep and layout to avoid large intermediate vectors.
2. Bounded allocations and limits options exposed on all heavy paths.
3. No per-glyph heap allocation in draw loop.
4. Stable behavior under small stack settings (document expected minimums).

## Error Model

1. Keep layered error enums:

- parse/resource
- style/cascade
- font resolution
- layout/render-prep.

2. Include actionable context in messages and structured fields.

## Testing Requirements

1. Unit tests:

- CSS precedence
- inline style handling
- font matching/fallback
- SHY behavior
- whitespace-sensitive sections
- justification decisions in layout IR.

2. Golden tests:

- Render IR snapshots for representative EPUB fragments.

3. Backend tests (`mu-epub-embedded-graphics`):

- command execution correctness
- justified text spacing distribution
- font backend parity between justification modes.

4. Property/invariant tests:

- pagination monotonicity
- non-overlapping line baselines
- deterministic output across repeated runs.

5. Web-preview regression harness:

- config-to-engine mapping coverage for exposed reader knobs
- dynamic reflow matrix (font size/scale, spacing, justification, family override)
- text-boundary safety assertions (no visible right-edge clipping in sampled pages)
- chapter page-number/progress monotonicity checks after each reflow scenario.

## Deliverables

1. API docs for all new public types/options/errors.
2. Migration notes for downstream consumers.
3. Example code:

- minimal "open -> render chapter page"
- advanced "streaming prep/layout with trace + custom fonts".

## Non-Goals

1. Full complex-script shaping engine.
2. Full browser-grade CSS support.
3. Platform-specific UI policy decisions.
