# Analysis Tooling

What each tool catches, when to run it, and how results feed back into the
[memory-management spec](../specs/memory-management.md).

All recipes live under `just analysis <recipe>`. Run `just analysis
check-tools` to verify prerequisites.

## Tools

### Static binary analysis

| Recipe | What it catches | Output |
|--------|----------------|--------|
| `bloat-cli` | Largest symbols in the CLI binary — spots unexpected generic bloat or large inline expansions | `target/analysis/bloat-cli-top.txt` |
| `bloat-cli-crates` | Per-crate contribution to binary size — identifies dependency bloat | `target/analysis/bloat-cli-crates.txt` |
| `size-cli` | ELF/Mach-O section sizes — shows text/data/bss balance | `target/analysis/size-cli-sections.txt` |
| `nm-cli-top` | Largest linked symbols by size — finds outlier monomorphizations | `target/analysis/nm-cli-top.txt` |
| `lint-perf` | Clippy perf lints (large enums, redundant clones, rc buffers) | terminal |

### Runtime profiling

| Recipe | What it catches | Output |
|--------|----------------|--------|
| `mem-profile` | Heap allocation hotspots during real EPUB operations (uses xctrace on macOS, heaptrack on Linux) | `target/memory/` |
| `bench` / `bench-quick` | Parse/tokenize/layout throughput regressions | `target/bench/latest.txt` |
| `bench-report` | Timestamped benchmark snapshot for A/B comparison | `target/bench/bench-<timestamp>.txt` |
| `miri` | Undefined behavior in unsafe code (runs under Miri interpreter) | terminal |

### Composite

| Recipe | What it runs |
|--------|-------------|
| `analyze-static` | `lint-perf` + `bloat-cli-crates` + `bloat-cli` + `size-cli` + `nm-cli-top` |
| `setup` | Installs all prerequisites (`cargo-bloat`, `cargo-binutils`, `llvm-tools-preview`, `miri`) |

## When to run

- **After perf-sensitive changes** (hot-path refactors, new allocations, dependency bumps): `just analysis analyze-static` + `just analysis bench-quick`.
- **Before a release**: `just analysis analyze-static` + `just analysis bench-report` to capture a baseline snapshot.
- **Investigating OOM or allocation growth**: `just analysis mem-profile` to capture a heap trace, then compare against the budgets in the memory-management spec.
- **After unsafe changes**: `just analysis miri`.

## Feedback loop

Results from analysis inform the [memory-management spec](../specs/memory-management.md):

1. `bloat-cli-crates` reveals dependency weight — if a crate grows past expectations, evaluate alternatives or feature-gate it.
2. `mem-profile` traces map directly to the audit checklist (no per-chapter allocations, scratch reuse, streaming chunks).
3. `bench-report` snapshots track whether performance targets in [architecture.md](../architecture.md) still hold.
4. `miri` validates that unsafe blocks (ZIP CRC, buffer tricks) remain sound.
