# Documentation

## Architecture

- [architecture.md](architecture.md) — Pipeline, library stack, data model, memory budgets, performance targets.

## Specs

Design contracts and acceptance criteria.

- [specs/render.md](specs/render.md) — Renderer spec, required APIs, typography/font/memory requirements, migration notes.
- [specs/memory-management.md](specs/memory-management.md) — Design philosophy, three-tier allocation strategy, limits reference, gap analysis.
- [specs/spec-compliance.md](specs/spec-compliance.md) — EPUB feature matrix (container, OPF, nav, content, CSS, layout, fonts).
- [specs/embedded-render-tracker.md](specs/embedded-render-tracker.md) — Production readiness tracker (P0/P1/P2 feature backlog).

## Guides

How-to for developers.

- [guides/embedded.md](guides/embedded.md) — Embedded-focused API usage (lazy open, bounded streaming, pagination, diagnostics).
- [guides/datasets.md](guides/datasets.md) — External corpus bootstrap, validation, expectation-aware testing.
- [guides/publishing.md](guides/publishing.md) — Release procedures, publish order, CI workflow.
- [guides/analysis.md](guides/analysis.md) — Analysis tooling: what each tool catches, when to run them, commands.
