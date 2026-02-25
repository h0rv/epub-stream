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

### Heap profiling (DHAT)

Uses [dhat-rs](https://docs.rs/dhat) — a pure-Rust, cross-platform heap profiler that
replaces the global allocator and records every allocation/deallocation. Outputs
JSON files viewable in the [DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html).

| Recipe | What it does | Output |
|--------|-------------|--------|
| `heap-profile` | Profile a single pipeline phase (default: `render`), one JSON per file | `target/memory/dhat-<phase>-<book>.json` |
| `heap-profile-all` | Profile all phases (open, cover, tokenize, render, full, session_once, session) | per-phase per-file JSONs |
| `heap-profile-view` | Profile a phase and open the viewer | browser + JSON |
| `heap-view` | Open the DHAT viewer and list available profiles | browser |
| `heap-list` | List available profile files | terminal |
| `heap-analyze` | Run analysis subcommands (see below) | terminal |
| `heap-report` | Full report: summary + hotspots + churn + peak + budget | terminal |
| `heap-budget-all` | Fail-fast budget guardrail across all phases | terminal |

By default, each EPUB gets its own clean DHAT profile (separate process), so
allocations from one book don't bleed into another. Pass `--aggregate` to get
a single combined profile for all files.

The DHAT viewer shows:
- **Total bytes allocated** and **peak heap usage** per call site
- **Allocation hotspots** — which functions allocate the most
- **Short-lived allocations** — objects allocated and freed quickly (optimization candidates)
- **Call tree** — full backtrace for every allocation site

Usage:

```sh
# Profile the render phase (one JSON per Gutenberg book)
just analysis heap-profile

# Profile all phases
just analysis heap-profile-all

# Profile reading session behavior (cover + chapter flip-through)
just analysis heap-profile session_once
just analysis heap-profile session

# Profile and immediately open the viewer
just analysis heap-profile-view render

# Open the viewer and list available profiles
just analysis heap-view

# List available profiles
just analysis heap-list

# Profile a specific file
just analysis heap-profile render path/to/book.epub

# Profile the full pipeline (all chapters) for a specific file
just analysis heap-profile full path/to/book.epub
```

After profiling, analyze the results:

```sh
# Full report with budget check (default 512KB for ESP)
just analysis heap-report

# Budget guardrail for all supported phases (fails on first over-budget phase)
just analysis heap-budget-all

# Individual analysis commands
just analysis heap-analyze summary --phase full
just analysis heap-analyze hotspots --phase full -n 20
just analysis heap-analyze churn --phase full
just analysis heap-analyze peak --phase render
just analysis heap-analyze budget --target 4MB
just analysis heap-analyze compare target/memory/before.json target/memory/after.json
```

Phase notes:
- `open`: open + metadata/navigation parse.
- `cover`: cover discovery + bounded cover reads.
- `tokenize`: open + chapter tokenization.
- `render`: open + single chapter style/layout/render.
- `full`: full-book chapter render loop.
- `session_once`: cover reads + one full chapter flip pass.
- `session`: cover reads + two full flip passes (accumulation check).

Or open a JSON file in the
[DHAT viewer](https://nnethercote.github.io/dh_view/dh_view.html) for
interactive exploration.

### Runtime

| Recipe | What it catches | Output |
|--------|----------------|--------|
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
- **Investigating OOM or allocation growth**: `just analysis heap-profile-all` to capture heap profiles for every pipeline phase, then compare allocation hotspots against the budgets in the memory-management spec.
- **After unsafe changes**: `just analysis miri`.

## Feedback loop

Results from analysis inform the [memory-management spec](../specs/memory-management.md):

1. `lint-memory` catches anti-patterns at introduction time — the first line of defense.
2. `bloat-cli-crates` reveals dependency weight — if a crate grows past expectations, evaluate alternatives or feature-gate it.
3. `heap-profile` traces map directly to the audit checklist (no per-chapter allocations, scratch reuse, streaming chunks).
4. `bench-report` snapshots track whether performance targets in [architecture.md](../architecture.md) still hold.
5. `miri` validates that unsafe blocks (ZIP CRC, buffer tricks) remain sound.
