#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATASET_DIR="${EPUB_STREAM_DATASET_DIR:-$ROOT_DIR/tests/datasets}"
CONFORMANCE_DIR="$DATASET_DIR/conformance"
INTEROP_DIR="$DATASET_DIR/interop"
EPUBTEST_DIR="$DATASET_DIR/a11y/epubtest"
WILD_DIR="$DATASET_DIR/wild/gutenberg"

GUTENBERG_IDS_DEFAULT=(
  11 74 84 98 1342 1661 2701 345 5200 6130 64317
)

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

clone_or_update() {
  local repo="$1"
  local dest="$2"
  if [ -d "$dest/.git" ]; then
    echo "updating $repo -> $dest"
    git -C "$dest" fetch --depth=1 origin HEAD
    git -C "$dest" reset --hard FETCH_HEAD
  else
    echo "cloning $repo -> $dest"
    git clone --depth=1 --filter=blob:none "$repo" "$dest"
  fi
}

download_gutenberg_epub() {
  local id="$1"
  local out="$WILD_DIR/pg${id}.epub"
  if [ -f "$out" ]; then
    echo "exists $out"
    return 0
  fi

  local url="https://www.gutenberg.org/ebooks/${id}.epub.images"
  echo "downloading Gutenberg #$id"
  if ! curl -fsSL -A "epub-stream-dataset-bootstrap/1.0" "$url" -o "$out"; then
    echo "failed Gutenberg #$id ($url)" >&2
    rm -f "$out"
    return 1
  fi
}

main() {
  require_cmd git
  require_cmd curl

  mkdir -p "$CONFORMANCE_DIR" "$INTEROP_DIR" "$EPUBTEST_DIR" "$WILD_DIR"

  clone_or_update "https://github.com/w3c/epubcheck.git" "$CONFORMANCE_DIR/epubcheck"
  clone_or_update "https://github.com/w3c/epub-tests.git" "$CONFORMANCE_DIR/epub-tests"
  clone_or_update \
    "https://github.com/w3c/epub-structural-tests.git" \
    "$CONFORMANCE_DIR/epub-structural-tests"
  clone_or_update "https://github.com/IDPF/epub3-samples.git" "$INTEROP_DIR/epub3-samples"

  cat >"$EPUBTEST_DIR/README.txt" <<'TXT'
EPUBTest data bootstrap notes
=============================

Source:
  https://epubtest.org/test-books

EPUBTest does not currently provide one stable public bulk ZIP endpoint that this
script can rely on without scraping HTML contract details. Use the index above to
download selected books into this folder manually.

Suggested naming:
  <epubtest-id>-<slug>.epub
TXT

  local failed=0
  local ids=("${GUTENBERG_IDS_DEFAULT[@]}")
  if [ "${#}" -gt 0 ]; then
    ids=("$@")
  fi

  for id in "${ids[@]}"; do
    if ! download_gutenberg_epub "$id"; then
      failed=1
    fi
  done

  local count
  count="$(find "$DATASET_DIR" -type f -iname '*.epub' | wc -l | tr -d ' ')"
  echo "dataset bootstrap complete: epub_count=$count root=$DATASET_DIR"

  if [ "$failed" -ne 0 ]; then
    echo "one or more Gutenberg downloads failed" >&2
    exit 1
  fi
}

main "$@"
