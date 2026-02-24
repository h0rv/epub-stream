# External Dataset Bootstrap

This project supports local bootstrap of external EPUB datasets for compliance
and interoperability validation. Downloaded files are intentionally excluded from
git (`tests/datasets/` is ignored).

## Quick Start

```bash
just analysis dataset-bootstrap
just analysis dataset-list
just analysis dataset-validate
just analysis dataset-validate-mini
```

Strict mode (warnings fail):

```bash
just analysis dataset-validate-strict
```

## Sources Pulled by Script

- W3C EPUBCheck repository (`w3c/epubcheck`)
- W3C EPUB tests (`w3c/epub-tests`)
- W3C structural tests (`w3c/epub-structural-tests`)
- IDPF EPUB3 samples (`IDPF/epub3-samples`)
- Project Gutenberg sample IDs (default set in script)

EPUBTest books:

- A placeholder folder is prepared at `tests/datasets/a11y/epubtest/`.
- Use <https://epubtest.org/test-books> to add specific books manually.

## Output Reports

`just analysis dataset-validate` writes reports to `target/datasets/`:

- `validate-<timestamp>.jsonl`: one line per EPUB with CLI validation output
- `validate-<timestamp>.summary.txt`: aggregate counts
- `validate-<timestamp>.mismatches.tsv`: expectation mismatches only
- `latest.jsonl`: symlink to latest report

## Expectation-Aware Validation (Default)

`just analysis dataset-validate` and `just analysis dataset-validate-strict` evaluate parser behavior against
`scripts/datasets/expectations.tsv` instead of requiring every EPUB file to be valid.

This is important for corpora like EPUBCheck that intentionally include invalid
fixtures. The run fails only when observed results differ from expected outcomes.

Raw mode (all files must validate) is still available:

```bash
just analysis dataset-validate-raw
just analysis dataset-validate-raw-strict
```

## CI-Ready Mini Corpus

Large external corpora are not suitable for CI. Use the curated manifest for a
small, deterministic smoke run that still exercises typical EPUB structure.

```bash
just analysis dataset-validate-mini
```

Notes:

- The mini corpus lives at `tests/datasets/manifest-mini.tsv` and references
  `tests/fixtures/bench/` plus the accessibility fixture already tracked in git.
- The manifest supports expectations and required diagnostic codes per file.
- When ZIP64 fixtures are added, track expected outcomes/codes in
  `scripts/datasets/expectations.tsv` so regressions are visible.
