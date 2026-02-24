# Heavy testing, render regression, memory linting, and embedded verification.

# --- Memory linting ---

# Strict memory-focused linting for constrained targets.
#
# - no_std pass: enforce core/alloc import discipline.
# - render pass: ban convenience constructors that hide allocation intent.
# - grep pass: catch patterns clippy can't (format!(), .to_string() in hot paths).
lint-memory:
    just testing lint-memory-no-std
    just testing lint-memory-render
    just testing lint-memory-patterns

# no_std/alloc discipline checks (core path only).
lint-memory-no-std:
    cargo clippy --no-default-features --lib -- -D warnings -W clippy::alloc_instead_of_core -W clippy::std_instead_of_alloc -W clippy::std_instead_of_core

# Render crate allocation-intent checks.
lint-memory-render:
    cargo clippy -p epub-stream-render --lib --no-deps -- -D warnings -W clippy::disallowed_methods

# Grep-based hot-path pattern lint (format!(), .to_string(), .collect::<Vec>, etc.).
lint-memory-patterns:
    bash scripts/lint-memory-patterns.sh

# --- Specialized test suites ---

# Memory budget tests (isolated allocators per integration test binary).
test-memory:
    cargo test --all-features --test budget_metadata --test budget_tokenize --test budget_render --test budget_full_flow -- --nocapture

# Fragmentation stress tests under allocator churn.
test-fragmentation:
    cargo test --all-features --test frag_epub_open --test frag_render -- --nocapture --test-threads=1

# Firmware-like open -> prepare -> render loop on host with deterministic fixtures.
test-firmware-path:
    cargo test --all-features --test firmware_path -- --nocapture

# Corpus stress test against in-repo fixtures and optional local Gutenberg datasets.
corpus-test:
    cargo test --all-features --test corpus_stress -- --nocapture

# Run ignored tests.
test-ignored:
    cargo test --all-features -- --ignored

# Run tests with output.
test-verbose:
    cargo test --all-features -- --nocapture

# Run allocation count tests.
test-alloc:
    just testing test-memory

# Run embedded-focused suites (tiny budgets + reflow regression matrix).
test-embedded:
    cargo test --all-features --test embedded_mode_tests -- --ignored --nocapture
    cargo test -p epub-stream-embedded-graphics --test embedded_reflow_regression -- --nocapture

# Verify benchmark fixture corpus integrity.
bench-fixtures-check:
    sha256sum -c tests/fixtures/bench/SHA256SUMS

# --- Render regression ---

# Check split render crates.
render-check:
    cargo check -p epub-stream-render -p epub-stream-embedded-graphics -p epub-stream-render-web

# Lint split render crates.
render-lint:
    cargo clippy -p epub-stream-render -p epub-stream-embedded-graphics -p epub-stream-render-web --all-targets -- -D warnings -A clippy::disallowed_methods

# Test split render crates.
render-test:
    cargo test -p epub-stream-render -p epub-stream-embedded-graphics -p epub-stream-render-web

# Run all split render crate checks.
render-all:
    just testing render-check
    just testing render-lint
    just testing render-test

# Deterministic reflow/config regression harness for reader controls.
render-regression:
    cargo test -p epub-stream-render --test corpus_regression_harness
    cargo test -p epub-stream-render --test typography_regression
    cargo test -p epub-stream-embedded-graphics --test embedded_reflow_regression
    cargo test -p epub-stream-render --test docs
    cargo test -p epub-stream-render-web --bin web-preview

# Focused embedded reflow regression harness.
embedded-reflow-regression:
    cargo test -p epub-stream-embedded-graphics --test embedded_reflow_regression -- --nocapture

# Focused low-RAM loop verification inside the embedded regression harness.
embedded-low-ram-matrix:
    cargo test -p epub-stream-embedded-graphics --test embedded_reflow_regression embedded_low_ram_reflow_and_page_turn_loops_are_stable -- --nocapture

# Focused budget/telemetry coverage inside the embedded regression harness.
embedded-budget-telemetry:
    cargo test -p epub-stream-embedded-graphics --test embedded_reflow_regression embedded_renderer_budget_diagnostics_cover_limit_and_fallback_paths -- --nocapture

# High-confidence typography gate:
# - run render-layout + typography regression tests
# - generate deterministic visual matrices for core Gutenberg fixtures
typography-confidence:
    cargo test -p epub-stream-render --tests
    cargo test -p epub-stream-render render_layout::tests:: -- --nocapture
    just visualize tests/fixtures/bench/pg84-frankenstein.epub 5 0 8 target/visualize-confidence/frankenstein/default
    just visualize justify tests/fixtures/bench/pg84-frankenstein.epub 5 0 8 target/visualize-confidence/frankenstein/justify
    just visualize large tests/fixtures/bench/pg84-frankenstein.epub 5 0 6 target/visualize-confidence/frankenstein/large
    just visualize tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 8 target/visualize-confidence/pride/default
    just visualize justify tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 8 target/visualize-confidence/pride/justify
    just visualize large tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 6 target/visualize-confidence/pride/large
    just visualize tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 8 target/visualize-confidence/sherlock/default
    just visualize justify tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 8 target/visualize-confidence/sherlock/justify
    just visualize large tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 6 target/visualize-confidence/sherlock/large

# Low-memory smoke suite for EPUB stability validation prior to flashing.
lowmem-confidence vm_kib="150000":
    just visualize lowmem tests/fixtures/bench/pg84-frankenstein.epub 5 0 16 target/visualize-lowmem/frankenstein {{vm_kib}}
    just visualize lowmem tests/fixtures/bench/pg1342-pride-and-prejudice.epub 7 0 16 target/visualize-lowmem/pride {{vm_kib}}
    just visualize lowmem tests/fixtures/bench/pg1661-sherlock-holmes.epub 3 0 16 target/visualize-lowmem/sherlock {{vm_kib}}
    just visualize lowmem tests/fixtures/bench/pg2701-moby-dick.epub 10 0 16 target/visualize-lowmem/moby {{vm_kib}}
    just visualize lowmem tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub 1 0 10 target/visualize-lowmem/fundamental {{vm_kib}}
