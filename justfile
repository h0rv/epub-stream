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
    cargo check -p mu-epub-render -p mu-epub-embedded-graphics

# Lint split render crates
render-lint:
    cargo clippy -p mu-epub-render -p mu-epub-embedded-graphics --all-targets -- -D warnings -A clippy::disallowed_methods

# Test split render crates
render-test:
    cargo test -p mu-epub-render -p mu-epub-embedded-graphics

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

# Run embedded mode tests with tiny budgets
test-embedded:
    cargo test --all-features --test embedded_mode_tests -- --ignored --nocapture

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
visualize epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-default":
    RUSTC_WRAPPER= cargo run -p mu-epub-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}}

# One-command sane default render pass for local layout iteration.
visualize-default:
    just visualize

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
