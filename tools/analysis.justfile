# Shared analysis/perf/memory tooling recipes for epub-stream.

profile_memory_script := justfile_directory() + "/scripts/profile_memory.sh"

setup-analysis-tools:
    just analysis install-all
    rustup component add llvm-tools-preview
    rustup +nightly component add miri

install-all:
    cargo install --force cargo-bloat cargo-binutils

check-analysis-tools:
    @echo "Checking analysis tools..."
    @cargo bloat --help >/dev/null 2>&1 || echo "⚠️  cargo-bloat missing (run: just analysis install-all)"
    @cargo size --help >/dev/null 2>&1 || echo "⚠️  cargo-size missing (run: just analysis install-all)"
    @cargo nm --help >/dev/null 2>&1 || echo "⚠️  cargo-nm missing (run: just analysis install-all)"
    @cargo +nightly miri --help >/dev/null 2>&1 || echo "⚠️  miri missing (run: just analysis setup-analysis-tools)"
    @if [ "$(uname -s)" = "Darwin" ]; then \
        command -v xctrace >/dev/null 2>&1 || echo "⚠️  xctrace missing (install full Xcode)"; \
      elif [ "$(uname -s)" = "Linux" ]; then \
        command -v heaptrack >/dev/null 2>&1 || echo "⚠️  heaptrack missing (install via system package manager)"; \
      fi
    @echo "✅ Analysis tools check complete"

lint-perf:
    cargo clippy --workspace --all-features -- -W clippy::perf -W clippy::large_enum_variant -W clippy::large_futures -W clippy::large_stack_frames -W clippy::rc_buffer -W clippy::redundant_clone

lint-perf-strict:
    cargo clippy --workspace --all-features -- -D warnings -W clippy::perf -W clippy::large_enum_variant -W clippy::large_futures -W clippy::large_stack_frames -W clippy::rc_buffer -W clippy::redundant_clone

bloat-cli n="30":
    @cargo bloat --help >/dev/null 2>&1 || (echo "cargo-bloat is not installed (install with: cargo install cargo-bloat)" && exit 1)
    @mkdir -p target/analysis
    @cargo bloat --release --features cli --bin epub-stream -n {{ n }} | tee target/analysis/bloat-cli-top.txt

bloat-cli-crates n="20":
    @cargo bloat --help >/dev/null 2>&1 || (echo "cargo-bloat is not installed (install with: cargo install cargo-bloat)" && exit 1)
    @mkdir -p target/analysis
    @cargo bloat --release --features cli --bin epub-stream --crates -n {{ n }} | tee target/analysis/bloat-cli-crates.txt

size-cli:
    @cargo size --help >/dev/null 2>&1 || (echo "cargo-size is not installed (install with: cargo install cargo-binutils; rustup component add llvm-tools-preview)" && exit 1)
    @mkdir -p target/analysis
    @cargo size --release --features cli --bin epub-stream -- -A | tee target/analysis/size-cli-sections.txt

nm-cli-top lines="40":
    @cargo nm --help >/dev/null 2>&1 || (echo "cargo-nm is not installed (install with: cargo install cargo-binutils; rustup component add llvm-tools-preview)" && exit 1)
    @mkdir -p target/analysis
    @cargo nm --release --features cli --bin epub-stream -- --print-size --size-sort | tail -n {{ lines }} | tee target/analysis/nm-cli-top.txt

bench:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features | tee target/bench/latest.txt

bench-quick:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features -- --quick | tee target/bench/quick.txt

bench-report:
    @mkdir -p target/bench
    @cargo bench --bench epub_bench --all-features | tee target/bench/bench-$(date +%Y%m%d-%H%M%S).txt

mem-profile mode="quick":
    @test -f "{{ profile_memory_script }}" || (echo "profile script not found: {{ profile_memory_script }}" && exit 1)
    "{{ profile_memory_script }}" {{ mode }}

mem-profile-out mode="quick" out="target/memory":
    @test -f "{{ profile_memory_script }}" || (echo "profile script not found: {{ profile_memory_script }}" && exit 1)
    "{{ profile_memory_script }}" {{ mode }} {{ out }}

analyze-static crates_n="20" symbols_n="30" nm_lines="40":
    just analysis lint-perf
    just analysis bloat-cli-crates {{ crates_n }}
    just analysis bloat-cli {{ symbols_n }}
    just analysis size-cli
    just analysis nm-cli-top {{ nm_lines }}

miri:
    cargo +nightly miri test --all-features --lib
