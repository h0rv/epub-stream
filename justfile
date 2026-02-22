# mu-epub justfile

# Host target used for local simulator/CLI binaries.
host_target := `if [ -n "${HOST_TEST_TARGET:-}" ]; then \
    echo "$HOST_TEST_TARGET"; \
else \
    rustc -vV | awk '/^host: / { print $2 }'; \
fi`

# Format code
fmt:
    cargo fmt --all

# Check formatting without changes
fmt-check:
    cargo fmt --all -- --check

# Type-check (default dev target matrix).
check:
    cargo check --workspace --all-features

# Lint with clippy (single strict pass).
lint:
    cargo clippy --workspace --all-features -- -D warnings

# Unit tests (fast default loop).
test:
    cargo test --workspace --all-features --lib --bins

# Default developer loop: auto-format + check + lint + unit tests.
all:
    just fmt
    just check
    just lint
    just test

# CI add-on: run integration tests after baseline all.
ci:
    just all
    just test-integration

# Backward-compatible aliases.
strict:
    just all

harden:
    just all

# Integration tests (slower / broader; useful in CI).
test-integration:
    cargo test --workspace --all-features --tests

# Strict memory-focused linting for constrained targets.
#
# - no_std pass: enforce core/alloc import discipline.
# - render pass: ban convenience constructors that hide allocation intent.
lint-memory:
    just lint-memory-no-std
    just lint-memory-render

# no_std/alloc discipline checks (core path only).
lint-memory-no-std:
    cargo clippy --no-default-features --lib -- -D warnings -W clippy::alloc_instead_of_core -W clippy::std_instead_of_alloc -W clippy::std_instead_of_core

# Render crate allocation-intent checks.
lint-memory-render:
    cargo clippy -p mu-epub-render --lib --no-deps -- -D warnings -W clippy::disallowed_methods

# Check split render crates
render-check:
    cargo check -p mu-epub-render -p mu-epub-embedded-graphics -p mu-epub-render-web

# Lint split render crates
render-lint:
    cargo clippy -p mu-epub-render -p mu-epub-embedded-graphics -p mu-epub-render-web --all-targets -- -D warnings -A clippy::disallowed_methods

# Test split render crates
render-test:
    cargo test -p mu-epub-render -p mu-epub-embedded-graphics -p mu-epub-render-web

# Run all split render crate checks
render-all:
    just render-check
    just render-lint
    just render-test

# Check no_std (no default features)
check-no-std:
    cargo check --no-default-features

# Run ignored tests
test-ignored:
    cargo test --all-features -- --ignored

# Run tests with output
test-verbose:
    cargo test --all-features -- --nocapture

# Run allocation count tests
test-alloc:
    cargo test --all-features --test allocation_tests -- --ignored --nocapture --test-threads=1

# Run embedded-focused suites (tiny budgets + reflow regression matrix).
test-embedded:
    cargo test --all-features --test embedded_mode_tests -- --ignored --nocapture
    cargo test -p mu-epub-embedded-graphics --test embedded_reflow_regression -- --nocapture

# Verify benchmark fixture corpus integrity
bench-fixtures-check:
    sha256sum -c tests/fixtures/bench/SHA256SUMS

# Build docs
doc:
    cargo doc --all-features --no-deps

# Build docs and fail on warnings
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

# Build docs and open locally
doc-open:
    cargo doc --all-features --no-deps --open

# Build release
build:
    cargo build --release --all-features

# Check CLI build
cli-check:
    cargo check --features cli --bin mu-epub

# Run CLI
cli *args:
    cargo run --features cli --bin mu-epub -- {{args}}

# Render EPUB pages to PNG snapshots for local visual layout debugging.
#
# Usage:
#   just visualize
#   just visualize tests/fixtures/bench/pg84-frankenstein.epub 5 0 12 target/visualize-default
visualize epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-default" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p mu-epub-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --cover-page-mode {{cover_page_mode}}

# Launch interactive web preview with live re-render API.
# Exposes primary typography + cover policy controls for quick e-reader tuning.
web-preview epub="tests/fixtures/bench/pg84-frankenstein.epub" port="42817" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p mu-epub-render-web --bin web-preview -- \
      {{epub}} \
      --serve \
      --open \
      --port {{port}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}

# Export standalone HTML preview snapshot (non-interactive reflow).
web-preview-export epub="tests/fixtures/bench/pg84-frankenstein.epub" out="target/web-preview/index.html" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p mu-epub-render-web --bin web-preview -- \
      {{epub}} \
      --out {{out}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}

# Chapter-scoped web preview variant.
web-preview-chapter epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" out="target/web-preview/chapter.html" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p mu-epub-render-web --bin web-preview -- \
      {{epub}} \
      --chapter {{chapter}} \
      --out {{out}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}

# One-command sane default render pass for local layout iteration.
visualize-default:
    just visualize

# Render with a constrained virtual-memory budget to catch large transient
# allocations locally before flashing firmware.
visualize-lowmem epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-lowmem" vm_kib="180000" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo build -p mu-epub-embedded-graphics --bin visualize --target {{ host_target }}
    bash -lc "ulimit -Sv {{vm_kib}}; target/{{host_target}}/debug/visualize {{epub}} --chapter {{chapter}} --start-page {{start}} --pages {{pages}} --out {{out}} --cover-page-mode {{cover_page_mode}}"

# Low-memory smoke suite for EPUB stability validation prior to flashing.
lowmem-confidence vm_kib="150000":
    just visualize-lowmem tests/fixtures/bench/pg84-frankenstein.epub 5 0 16 target/visualize-lowmem/frankenstein {{vm_kib}}
    just visualize-lowmem tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 16 target/visualize-lowmem/pride {{vm_kib}}
    just visualize-lowmem tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 16 target/visualize-lowmem/sherlock {{vm_kib}}
    just visualize-lowmem tests/fixtures/bench/pg2701-moby-dick.epub 10 0 16 target/visualize-lowmem/moby {{vm_kib}}
    just visualize-lowmem tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub 1 0 10 target/visualize-lowmem/fundamental {{vm_kib}}

# Same as visualize, but with inter-word justification enabled.
visualize-justify epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-justify":
    RUSTC_WRAPPER= cargo run -p mu-epub-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --justify

# Larger type profile for wrap/spacing validation.
visualize-large epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="8" out="target/visualize-large":
    RUSTC_WRAPPER= cargo run -p mu-epub-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --font-size 28 \
      --line-gap 5 \
      --paragraph-gap 10

# Deterministic typography sweep for local golden-like visual review.
visualize-matrix epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="6":
    just visualize {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-default
    just visualize-justify {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-justify
    just visualize-large {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-large

# High-confidence typography gate:
# - run render-layout + typography regression tests
# - generate deterministic visual matrices for core Gutenberg fixtures
typography-confidence:
    cargo test -p mu-epub-render --tests
    cargo test -p mu-epub-render render_layout::tests:: -- --nocapture
    just visualize tests/fixtures/bench/pg84-frankenstein.epub 5 0 8 target/visualize-confidence/frankenstein/default
    just visualize-justify tests/fixtures/bench/pg84-frankenstein.epub 5 0 8 target/visualize-confidence/frankenstein/justify
    just visualize-large tests/fixtures/bench/pg84-frankenstein.epub 5 0 6 target/visualize-confidence/frankenstein/large
    just visualize tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 8 target/visualize-confidence/pride/default
    just visualize-justify tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 8 target/visualize-confidence/pride/justify
    just visualize-large tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 6 target/visualize-confidence/pride/large
    just visualize tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 8 target/visualize-confidence/sherlock/default
    just visualize-justify tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 8 target/visualize-confidence/sherlock/justify
    just visualize-large tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 6 target/visualize-confidence/sherlock/large

# Deterministic reflow/config regression harness for reader controls.
render-regression:
    cargo test -p mu-epub-render --test corpus_regression_harness
    cargo test -p mu-epub-render --test typography_regression
    cargo test -p mu-epub-embedded-graphics --test embedded_reflow_regression
    cargo test -p mu-epub-render --test docs
    cargo test -p mu-epub-render-web --bin web-preview

# Focused embedded reflow regression harness.
embedded-reflow-regression:
    cargo test -p mu-epub-embedded-graphics --test embedded_reflow_regression -- --nocapture

# Focused low-RAM loop verification inside the embedded regression harness.
embedded-low-ram-matrix:
    cargo test -p mu-epub-embedded-graphics --test embedded_reflow_regression embedded_low_ram_reflow_and_page_turn_loops_are_stable -- --nocapture

# Focused budget/telemetry coverage inside the embedded regression harness.
embedded-budget-telemetry:
    cargo test -p mu-epub-embedded-graphics --test embedded_reflow_regression embedded_renderer_budget_diagnostics_cover_limit_and_fallback_paths -- --nocapture

# Bootstrap external test datasets (not committed)
dataset-bootstrap:
    ./scripts/datasets/bootstrap.sh

# Bootstrap with explicit Gutenberg IDs (space-separated)
dataset-bootstrap-gutenberg *ids:
    ./scripts/datasets/bootstrap.sh {{ids}}

# List all discovered dataset EPUB files
dataset-list:
    ./scripts/datasets/list_epubs.sh

# Validate all dataset EPUB files
dataset-validate:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --expectations scripts/datasets/expectations.tsv

# Validate only Gutenberg EPUB corpus under tests/datasets/wild/gutenberg.
dataset-validate-gutenberg:
    @cargo build --features cli --bin mu-epub
    DATASET_ROOT="${MU_EPUB_DATASET_DIR:-tests/datasets}" && \
    ./scripts/datasets/validate.sh --dataset-dir "$DATASET_ROOT/wild/gutenberg" --expectations scripts/datasets/expectations.tsv

# Validate only Gutenberg EPUB corpus in strict mode.
dataset-validate-gutenberg-strict:
    @cargo build --features cli --bin mu-epub
    DATASET_ROOT="${MU_EPUB_DATASET_DIR:-tests/datasets}" && \
    ./scripts/datasets/validate.sh --strict --dataset-dir "$DATASET_ROOT/wild/gutenberg" --expectations scripts/datasets/expectations.tsv

# Time Gutenberg corpus smoke path (validate + chapters + first chapter text).
dataset-profile-gutenberg:
    @cargo build --release --features cli --bin mu-epub
    MU_EPUB_CLI_BIN=target/release/mu-epub ./scripts/datasets/gutenberg_smoke.sh

# Time Gutenberg corpus smoke path in strict validation mode.
dataset-profile-gutenberg-strict:
    @cargo build --release --features cli --bin mu-epub
    MU_EPUB_CLI_BIN=target/release/mu-epub ./scripts/datasets/gutenberg_smoke.sh --strict

# Full pre-flash gate including local Gutenberg corpus (if bootstrapped).
harden-gutenberg:
    just all
    just dataset-validate-gutenberg
    just dataset-profile-gutenberg

# Validate all dataset EPUB files in strict mode (warnings fail too)
dataset-validate-strict:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --strict --expectations scripts/datasets/expectations.tsv

# Validate against expectation manifest (default mode)
dataset-validate-expected:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --expectations scripts/datasets/expectations.tsv

# Validate against expectation manifest in strict mode
dataset-validate-expected-strict:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --strict --expectations scripts/datasets/expectations.tsv

# Raw validate mode (every file must pass validation)
dataset-validate-raw:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh

# Raw strict validate mode (warnings fail too)
dataset-validate-raw-strict:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --strict

# Validate a small, CI-ready mini corpus from a manifest
dataset-validate-mini:
    @cargo build --features cli --bin mu-epub
    ./scripts/datasets/validate.sh --manifest tests/datasets/manifest-mini.tsv

# Run benchmarks and save latest CSV report
bench:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features | tee target/bench/latest.csv

# Check no_std + layout
check-no-std-layout:
    cargo check --no-default-features --features layout

# MSRV check (matches Cargo.toml rust-version)
check-msrv:
    cargo +1.85.0 check --all-features

# Clean build artifacts
clean:
    cargo clean

# Crates.io publish order (dependency-aware).
publish-order:
    @echo "mu-epub mu-epub-render mu-epub-embedded-graphics mu-epub-render-web"

# Local package sanity check for one crate.
package crate:
    RUSTC_WRAPPER= cargo package -p {{crate}}

# Local package sanity check for all crates in publish order.
package-all:
    just package mu-epub
    just package mu-epub-render
    just package mu-epub-embedded-graphics
    just package mu-epub-render-web

# Dry-run publish for one crate.
publish-dry-run crate:
    RUSTC_WRAPPER= cargo publish -p {{crate}} --dry-run

# Dry-run publish for all crates in dependency order.
publish-dry-run-all:
    just publish-dry-run mu-epub
    just publish-dry-run mu-epub-render
    just publish-dry-run mu-epub-embedded-graphics
    just publish-dry-run mu-epub-render-web

# Full release preflight before publishing.
release-preflight:
    just ci
    just package-all
    just publish-dry-run-all

# Publish all crates to crates.io in dependency order.
# Requires CARGO_REGISTRY_TOKEN to be configured.
publish-all:
    @bash -eu -o pipefail -c '\
      crates="mu-epub mu-epub-render mu-epub-embedded-graphics mu-epub-render-web"; \
      for c in $crates; do \
        echo "Publishing $$c..."; \
        cargo publish -p "$$c"; \
        sleep 30; \
      done'
