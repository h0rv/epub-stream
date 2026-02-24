# Analysis, benchmarking, and dataset validation recipes.

profile_memory_script := justfile_directory() + "/scripts/profile_memory.sh"

# Install all analysis tools.
setup:
    just analysis install-all
    rustup component add llvm-tools-preview
    rustup +nightly component add miri

# Install cargo analysis binaries.
install-all:
    cargo install --force cargo-bloat cargo-binutils

# Verify analysis tool prerequisites.
check-tools:
    @echo "Checking analysis tools..."
    @cargo bloat --help >/dev/null 2>&1 || echo "  cargo-bloat missing (run: just analysis install-all)"
    @cargo size --help >/dev/null 2>&1 || echo "  cargo-size missing (run: just analysis install-all)"
    @cargo nm --help >/dev/null 2>&1 || echo "  cargo-nm missing (run: just analysis install-all)"
    @cargo +nightly miri --help >/dev/null 2>&1 || echo "  miri missing (run: just analysis setup)"
    @if [ "$(uname -s)" = "Darwin" ]; then \
        command -v xctrace >/dev/null 2>&1 || echo "  xctrace missing (install full Xcode)"; \
      elif [ "$(uname -s)" = "Linux" ]; then \
        command -v heaptrack >/dev/null 2>&1 || echo "  heaptrack missing (install via system package manager)"; \
      fi
    @echo "Analysis tools check complete"

# --- Static binary analysis ---

# Clippy perf lints (large enums, redundant clones, rc buffers).
lint-perf:
    cargo clippy --workspace --all-features -- -W clippy::perf -W clippy::large_enum_variant -W clippy::large_futures -W clippy::large_stack_frames -W clippy::rc_buffer -W clippy::redundant_clone

# Strict perf lints (warnings denied).
lint-perf-strict:
    cargo clippy --workspace --all-features -- -D warnings -W clippy::perf -W clippy::large_enum_variant -W clippy::large_futures -W clippy::large_stack_frames -W clippy::rc_buffer -W clippy::redundant_clone

# Top symbols by size in CLI binary.
bloat-cli n="30":
    @cargo bloat --help >/dev/null 2>&1 || (echo "cargo-bloat is not installed (install with: cargo install cargo-bloat)" && exit 1)
    @mkdir -p target/analysis
    @cargo bloat --release --features cli --bin epub-stream -n {{ n }} | tee target/analysis/bloat-cli-top.txt

# Per-crate contribution to CLI binary size.
bloat-cli-crates n="20":
    @cargo bloat --help >/dev/null 2>&1 || (echo "cargo-bloat is not installed (install with: cargo install cargo-bloat)" && exit 1)
    @mkdir -p target/analysis
    @cargo bloat --release --features cli --bin epub-stream --crates -n {{ n }} | tee target/analysis/bloat-cli-crates.txt

# ELF/Mach-O section sizes for CLI binary.
size-cli:
    @cargo size --help >/dev/null 2>&1 || (echo "cargo-size is not installed (install with: cargo install cargo-binutils; rustup component add llvm-tools-preview)" && exit 1)
    @mkdir -p target/analysis
    @cargo size --release --features cli --bin epub-stream -- -A | tee target/analysis/size-cli-sections.txt

# Largest linked symbols by size.
nm-cli-top lines="40":
    @cargo nm --help >/dev/null 2>&1 || (echo "cargo-nm is not installed (install with: cargo install cargo-binutils; rustup component add llvm-tools-preview)" && exit 1)
    @mkdir -p target/analysis
    @cargo nm --release --features cli --bin epub-stream -- --print-size --size-sort | tail -n {{ lines }} | tee target/analysis/nm-cli-top.txt

# Full static analysis sweep (lint + bloat + size + nm).
analyze-static crates_n="20" symbols_n="30" nm_lines="40":
    just analysis lint-perf
    just analysis bloat-cli-crates {{ crates_n }}
    just analysis bloat-cli {{ symbols_n }}
    just analysis size-cli
    just analysis nm-cli-top {{ nm_lines }}

# --- Runtime profiling ---

# Heap allocation profiling (xctrace on macOS, heaptrack on Linux).
mem-profile mode="quick":
    @test -f "{{ profile_memory_script }}" || (echo "profile script not found: {{ profile_memory_script }}" && exit 1)
    "{{ profile_memory_script }}" {{ mode }}

# Heap allocation profiling with custom output directory.
mem-profile-out mode="quick" out="target/memory":
    @test -f "{{ profile_memory_script }}" || (echo "profile script not found: {{ profile_memory_script }}" && exit 1)
    "{{ profile_memory_script }}" {{ mode }} {{ out }}

# Run Miri to detect undefined behavior in unsafe code.
miri:
    cargo +nightly miri test --all-features --lib

# --- Benchmarks ---

# Run benchmarks and save latest text report.
bench:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features | tee target/bench/latest.txt

# Fast benchmark smoke pass for local edit loops.
bench-quick:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features -- --quick | tee target/bench/quick.txt

# Timestamped benchmark artifact for A/B comparison.
bench-report:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features | tee target/bench/bench-$(date +%Y%m%d-%H%M%S).txt

# --- Datasets ---

# Bootstrap external test datasets (not committed).
dataset-bootstrap:
    ./scripts/datasets/bootstrap.sh

# Bootstrap with explicit Gutenberg IDs (space-separated).
dataset-bootstrap-gutenberg *ids:
    ./scripts/datasets/bootstrap.sh {{ids}}

# List all discovered dataset EPUB files.
dataset-list:
    ./scripts/datasets/list_epubs.sh

# Validate all dataset EPUB files (expectation-aware by default).
# Pass extra args to the validate script, e.g.:
#   just analysis dataset-validate --dataset-dir tests/datasets/wild/gutenberg
dataset-validate *args:
    @cargo build --features cli --bin epub-stream
    ./scripts/datasets/validate.sh --expectations scripts/datasets/expectations.tsv {{args}}

# Validate in strict mode (warnings fail too).
dataset-validate-strict *args:
    @cargo build --features cli --bin epub-stream
    ./scripts/datasets/validate.sh --strict --expectations scripts/datasets/expectations.tsv {{args}}

# Validate a small, CI-ready mini corpus from a manifest.
dataset-validate-mini:
    @cargo build --features cli --bin epub-stream
    ./scripts/datasets/validate.sh --manifest tests/datasets/manifest-mini.tsv

# Time Gutenberg corpus smoke path (validate + chapters + first chapter text).
dataset-profile-gutenberg *args:
    @cargo build --release --features cli --bin epub-stream
    EPUB_STREAM_CLI_BIN=target/release/epub-stream ./scripts/datasets/gutenberg_smoke.sh {{args}}
