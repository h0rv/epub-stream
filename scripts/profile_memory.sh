#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

MODE="${1:-quick}"
OUT_DIR="${2:-target/memory}"

case "$MODE" in
quick|full) ;;
*)
  echo "usage: $0 [quick|full] [out_dir]" >&2
  exit 2
  ;;
esac

mkdir -p "$OUT_DIR"

echo "Building benchmark binary..."
cargo bench --bench epub_bench --all-features --no-run

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [[ "$TARGET_DIR" != /* ]]; then
  TARGET_DIR="$PROJECT_ROOT/$TARGET_DIR"
fi

BENCH_DIR="$TARGET_DIR/release/deps"
BENCH_BIN=""
shopt -s nullglob
for candidate in "$BENCH_DIR"/epub_bench-*; do
  if [[ -f "$candidate" && -x "$candidate" && "$candidate" != *.d ]]; then
    BENCH_BIN="$candidate"
    break
  fi
done
shopt -u nullglob

if [[ -z "$BENCH_BIN" ]]; then
  echo "failed to locate benchmark binary under $BENCH_DIR/epub_bench-*" >&2
  exit 1
fi

BENCH_ARGS=()
if [[ "$MODE" == "quick" ]]; then
  BENCH_ARGS+=(--quick)
fi

TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
OS="$(uname -s)"

if [[ "$OS" == "Darwin" ]] && xcrun xctrace version >/dev/null 2>&1; then
  TRACE_PATH="$OUT_DIR/alloc-${TIMESTAMP}.trace"
  echo "Profiling with xctrace Allocations template..."
  xcrun xctrace record \
    --template "Allocations" \
    --output "$TRACE_PATH" \
    --launch -- "$BENCH_BIN" "${BENCH_ARGS[@]}"
  echo "trace saved: $TRACE_PATH"
  echo "open trace: open \"$TRACE_PATH\""
  exit 0
fi

if command -v heaptrack >/dev/null 2>&1; then
  HEAPTRACK_OUT="$OUT_DIR/heaptrack-${TIMESTAMP}.gz"
  echo "Profiling with heaptrack..."
  heaptrack -o "$HEAPTRACK_OUT" "$BENCH_BIN" "${BENCH_ARGS[@]}"
  echo "heaptrack saved: $HEAPTRACK_OUT"
  echo "text report: heaptrack_print \"$HEAPTRACK_OUT\" | less"
  echo "gui report: heaptrack_gui \"$HEAPTRACK_OUT\""
  exit 0
fi

echo "No supported memory profiler found for this host." >&2
echo "Install one of:" >&2
echo "  - macOS: full Xcode app (xctrace requires Xcode, not just CLT)" >&2
echo "  - Linux: heaptrack" >&2
exit 1
