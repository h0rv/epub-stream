# epub-stream justfile

mod analysis ".analysis.just"
mod testing ".testing.just"
mod visualize ".visualize.just"
mod publish ".publish.just"

# --- Core dev loop ---

# Format code.
fmt:
    cargo fmt --all

# Check formatting without changes.
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

# Integration tests (slower / broader; useful in CI).
test-integration:
    cargo test --workspace --all-features --tests

# --- Check variants ---

# Check no_std (no default features).
check-no-std:
    cargo check --no-default-features

# Check no_std + layout.
check-no-std-layout:
    cargo check --no-default-features --features layout

# MSRV check (matches Cargo.toml rust-version).
check-msrv:
    cargo +1.85.0 check --all-features

# --- Build / docs / CLI ---

# Build release.
build:
    cargo build --release --all-features

# Clean build artifacts.
clean:
    cargo clean

# Build docs.
doc:
    cargo doc --all-features --no-deps

# Build docs and fail on warnings.
doc-check:
    RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

# Build docs and open locally.
doc-open:
    cargo doc --all-features --no-deps --open

# Check CLI build.
cli-check:
    cargo check --features cli --bin epub-stream

# Run CLI.
cli *args:
    cargo run --features cli --bin epub-stream -- {{args}}

# --- Composite workflows ---

# Fast local confidence loop after each change.
test-all:
    just fmt-check
    just lint
    just test
    just testing test-memory
    just testing test-firmware-path
    just analysis bench-quick

# Full local validation before merge.
validate:
    just test-all
    just testing test-fragmentation
    just testing corpus-test
    just testing render-regression
