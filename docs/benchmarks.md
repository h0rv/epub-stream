# Benchmarks

This project uses a benchmark harness at `benches/epub_bench.rs`.

## Goals

- Reproducible: same fixture bytes, fixed benchmark code path, explicit commands.
- Transparent: benchmark scope and limitations are documented.

## Fixture Corpus

- `tests/fixtures/Fundamental-Accessibility-Tests-Basic-Functionality-v2.0.0.epub`
- `tests/fixtures/bench/pg84-frankenstein.epub`
- `tests/fixtures/bench/pg1661-sherlock-holmes.epub`
- `tests/fixtures/bench/pg2701-moby-dick.epub`
- `tests/fixtures/bench/pg1342-pride-and-prejudice.epub`

All fixtures are embedded with `include_bytes!`, so runs do not depend on runtime filesystem I/O.
See `tests/fixtures/bench/SHA256SUMS` and `tests/fixtures/bench/README.md` for source and integrity metadata.

## Bench Cases

- `zip/open_archive`
  - Cost of parsing ZIP central directory and opening archive state.
- `parse/package`
  - `container.xml` parse + OPF parse + spine parse.
- `tokenize/first_spine_item`
  - Tokenization of first spine chapter.
- `tokenize/all_spine_items`
  - Tokenization across all spine chapters.
- `high_level/open_book`
  - `EpubBook::from_reader(...)` parse cost.
- `high_level/open_and_tokenize_first`
  - High-level open + first chapter tokenize.
- `high_level/open_tokenize_layout_first` (only with `layout` feature)
  - High-level open + tokenize + layout first chapter.
- `compare/epub-rs/open_book`
  - Open the same fixture with `epub` crate and report chapter count.
- `compare/epub-rs/open_and_get_current`
  - Open with `epub` and read current chapter bytes.
- `compare/epub-parser/parse`
  - Parse the same fixture with `epub-parser`.
- `compare/epub-parser/parse_and_first_page_len`
  - Parse with `epub-parser` and access first extracted page.

Compared crate versions are pinned in `Cargo.toml`:

- `epub = 2.1.5`
- `epub-parser = 0.3.4`

## How To Run

```bash
just bench
```

## Reproducibility Checklist

- Use release benches (`cargo bench`) only.
- Run on an otherwise idle machine.
- Keep power/performance profile stable for repeated runs.
- Use the same feature set for all compared runs (recommended: `--all-features`).
- Record toolchain and host info:

```bash
rustc -Vv
cargo -V
uname -a
```

- Keep fixture hash pinned (above). If fixture changes, update this document.
- Verify corpus integrity:

```bash
just bench-fixtures-check
```

## Output Format

The harness prints CSV-like sections to stdout:

- Results:
  `fixture,case,iterations,min_ns,median_ns,p90_ns,mean_ns,max_ns,min_peak_heap_bytes,median_peak_heap_bytes,p90_peak_heap_bytes,mean_peak_heap_bytes,max_peak_heap_bytes`
- Summary:
  `fixture,metric,epub-stream_median_ns,other_median_ns,ratio_x,delta_percent`
- Memory summary:
  `fixture,metric,epub-stream_median_peak_heap_bytes,other_median_peak_heap_bytes,ratio_x,delta_percent`
- Fixture metadata:
  `key,filename,size_bytes`

`just bench` stores output in `target/bench/latest.csv`.

## Limitations

- Memory measurements are benchmark-process allocator observations, not full OS RSS.
  Metric semantics: per measured iteration, report peak extra heap bytes above the
  baseline (`current_alloc_bytes`) observed immediately before case execution.
- Current harness is fixed-corpus, single-process, and microbenchmark-oriented; use additional
  system-level profiling if you need end-to-end RSS/cgroup memory characterization.
- Current harness uses two external comparison libraries (`epub`, `epub-parser`), but
  it is still a microbenchmark-style comparison, not a full workload simulation.
