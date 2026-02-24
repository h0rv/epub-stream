# Repository Guidelines

## Project Structure & Module Organization
`epub-stream` is a Rust workspace focused on memory-efficient EPUB parsing.
- `src/`: core library modules (`zip`, `streaming`, `tokenizer`, `metadata`, `book`, `validate`, etc.).
- `src/bin/epub-stream.rs`: CLI entrypoint (enabled by `cli` feature).
- `crates/epub-stream-render`: render IR + layout orchestration.
- `crates/epub-stream-embedded-graphics`: `embedded-graphics` backend.
- `tests/`: integration, regression, allocation, and embedded-mode suites.
- `tests/fixtures/`: sample EPUBs and benchmark corpus checksums.
- `benches/epub_bench.rs`: benchmark target.
- `docs/`: architecture, specs (render, memory, compliance, tracker), and guides (embedded, datasets, publishing, analysis). See `docs/README.md` for the full index.
- `scripts/datasets/`: dataset bootstrap and validation tooling.

## Build, Test, and Development Commands
Use `just` recipes (CI uses these exact commands):
- `just all`: full local CI pass (fmt, clippy, checks, tests, docs, CLI).
- `just fmt` / `just fmt-check`: format code or verify formatting.
- `just lint`: clippy with `--all-features` and warnings denied.
- `just check`, `just check-no-std`, `just check-no-std-layout`, `just check-msrv`: compile matrix validation.
- `just test`, `just testing test-ignored`, `just testing test-alloc`, `just testing test-embedded`: test suites.
- `just doc-check`: build docs with warnings as errors.
- `just cli-check` or `just cli -- validate book.epub --pretty`: CLI validation/run.

## Coding Style & Naming Conventions
Target Rust 2021 (`rust-version = 1.85`).
- Follow `cargo fmt` output (4-space indentation, standard Rust formatting).
- Keep clippy-clean across feature sets; warnings are treated as errors in CI.
- Public APIs should be documented (`missing_docs` is warned in crate attributes).
- Naming: modules/files `snake_case`, types/traits `UpperCamelCase`, functions/tests `snake_case`.
- Prefer bounded-memory APIs (`*_into`, scratch-buffer variants) on hot paths.

## Testing Guidelines
- Add behavior tests in `tests/*.rs`; keep regression coverage in `tests/regression.rs`.
- Name tests by behavior (example: `xml_entity_ampersand_unescaped`).
- Fixture-heavy tests should use `#[ignore]` and guard for missing local fixtures.
- Before opening a PR, run at least `just fmt-check`, `just lint`, and `just test`; run `just all` for full validation.

## Commit & Pull Request Guidelines
- Match existing commit style: short, imperative subject lines (`Add ...`, `Fix ...`, `Reduce ...`).
- Keep each commit focused on one logical change.
- PRs should include: what changed, why, affected features (`std`/`layout`/`async`/`cli`), and exact commands run.
- For CLI or performance/memory changes, include sample output or benchmark/allocation evidence (`just analysis bench`, `just testing test-alloc`).

## Documentation

Read these when the context calls for it:

- `docs/architecture.md` — Start here. Pipeline, crate stack, memory budgets, perf targets.
- `docs/specs/memory-management.md` — Before writing any allocation, buffer, or limit code. The three-tier pattern and gap analysis.
- `docs/specs/render.md` — Before touching render/layout/font code. API contracts and migration notes.
- `docs/specs/spec-compliance.md` — When adding EPUB feature support. What's done, what's not.
- `docs/specs/embedded-render-tracker.md` — For prioritizing embedded render work. P0/P1/P2 backlog.
- `docs/guides/embedded.md` — API examples for embedded-focused usage paths.
- `docs/guides/analysis.md` — Before profiling or investigating memory/perf issues. What tools exist and when to run them.
- `docs/guides/datasets.md` — When working with test corpora or validation.
- `docs/guides/publishing.md` — Release procedures.
