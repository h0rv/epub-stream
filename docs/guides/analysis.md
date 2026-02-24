# Analysis Tooling

What each tool catches, when to run it, and how results feed back into the
[memory-management spec](../specs/memory-management.md).

Analysis recipes live under `just analysis <recipe>`. Memory linting lives
under `just testing lint-memory`. Run `just analysis check-tools` to verify
prerequisites.

## Memory linting

`just testing lint-memory` runs three passes:

| Pass | What it catches | Gate |
|------|----------------|------|
| `lint-memory-no-std` | `std::` imports where `core::`/`alloc::` suffice (import discipline) | hard — fails on any violation |
| `lint-memory-render` | `Vec::new()`, `String::new()`, `HashMap::new()`, `BTreeMap::new()` in library code (via `clippy.toml` disallowed-methods) | hard — fails on any violation |
| `lint-memory-patterns` | 12 pattern categories across all library code (via `scripts/lint-memory-patterns.sh`) | soft — fails if count exceeds baseline |

The grep-based lint (`lint-memory-patterns`) scans all production library code
(core, render, embedded-graphics, render-web lib) for allocation anti-patterns:

- **Constructors**: `Box::new()`, `Map::new()`
- **String materialisation**: `format!()`, `.to_string()`, `.to_owned()`, `.join()`, `.concat()`
- **Collection materialisation**: `.collect::<Vec>`, `.collect()` (inferred type)
- **Cloning**: `.clone()` on owned types
- **Capacity**: `Vec::with_capacity(0)`, `String::with_capacity(0)`

Error paths, comments, and test modules (after `#[cfg(test)]`) are excluded.
Uses a baseline threshold — lower `BASELINE` in the script as you fix hits.
New violations above baseline fail CI.

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

- **After any code change**: `just testing lint-memory` — catches allocation anti-patterns at introduction time.
- **After perf-sensitive changes** (hot-path refactors, new allocations, dependency bumps): `just analysis analyze-static` + `just analysis bench-quick`.
- **Before a release**: `just analysis analyze-static` + `just analysis bench-report` to capture a baseline snapshot.
- **Investigating OOM or allocation growth**: `just analysis mem-profile` to capture a heap trace, then compare against the budgets in the memory-management spec.
- **After unsafe changes**: `just analysis miri`.

## Feedback loop

Results from analysis inform the [memory-management spec](../specs/memory-management.md):

1. `lint-memory` catches anti-patterns at introduction time — the first line of defense.
2. `bloat-cli-crates` reveals dependency weight — if a crate grows past expectations, evaluate alternatives or feature-gate it.
3. `mem-profile` traces map directly to the audit checklist (no per-chapter allocations, scratch reuse, streaming chunks).
4. `bench-report` snapshots track whether performance targets in [architecture.md](../architecture.md) still hold.
5. `miri` validates that unsafe blocks (ZIP CRC, buffer tricks) remain sound.
