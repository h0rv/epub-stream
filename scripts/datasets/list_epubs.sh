#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATASET_DIR="${EPUB_STREAM_DATASET_DIR:-$ROOT_DIR/tests/datasets}"

if [ ! -d "$DATASET_DIR" ]; then
  echo "dataset directory not found: $DATASET_DIR"
  echo "run: just dataset-bootstrap"
  exit 0
fi

echo "dataset root: $DATASET_DIR"
echo
echo "by category:"
for category in conformance interop a11y wild; do
  dir="$DATASET_DIR/$category"
  if [ -d "$dir" ]; then
    count="$(find "$dir" -type f -iname '*.epub' | wc -l | tr -d ' ')"
    echo "  $category: $count"
  else
    echo "  $category: 0"
  fi
done

echo
echo "all epub files:"
find "$DATASET_DIR" -type f -iname '*.epub' | sort
