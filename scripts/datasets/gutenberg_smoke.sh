#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATASET_ROOT="${EPUB_STREAM_DATASET_DIR:-$ROOT_DIR/tests/datasets}"
GUTENBERG_DIR="$DATASET_ROOT/wild/gutenberg"
BIN="${EPUB_STREAM_CLI_BIN:-$ROOT_DIR/target/debug/epub-stream}"
OUT_DIR="$ROOT_DIR/target/datasets"
STRICT=0
EXPECTATIONS_FILE="$ROOT_DIR/scripts/datasets/gutenberg-smoke-expectations.tsv"

RULE_PATTERNS=()
RULE_ALLOWED=()
RULE_NOTES=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --strict)
      STRICT=1
      shift
      ;;
    --dataset-dir)
      GUTENBERG_DIR="$2"
      shift 2
      ;;
    --expectations)
      EXPECTATIONS_FILE="$2"
      shift 2
      ;;
    *)
      echo "unknown arg: $1" >&2
      echo "usage: $0 [--strict] [--dataset-dir PATH] [--expectations FILE]" >&2
      exit 1
      ;;
  esac
done

if [ ! -x "$BIN" ]; then
  echo "epub-stream binary not found at $BIN" >&2
  echo "build with: cargo build --features cli --bin epub-stream" >&2
  exit 1
fi

if [ ! -d "$GUTENBERG_DIR" ]; then
  echo "gutenberg directory not found: $GUTENBERG_DIR" >&2
  echo "bootstrap with: just dataset-bootstrap-gutenberg" >&2
  exit 1
fi

if [ -n "$EXPECTATIONS_FILE" ] && [ -f "$EXPECTATIONS_FILE" ]; then
  while IFS=$'\t' read -r pattern allowed note || [ -n "${pattern:-}" ]; do
    if [ -z "${pattern:-}" ]; then
      continue
    fi
    if [[ "$pattern" == \#* ]]; then
      continue
    fi
    RULE_PATTERNS+=("$pattern")
    RULE_ALLOWED+=("${allowed:-}")
    RULE_NOTES+=("${note:-}")
  done <"$EXPECTATIONS_FILE"
fi

EPUBS=()
while IFS= read -r epub_path; do
  EPUBS+=("$epub_path")
done < <(find "$GUTENBERG_DIR" -type f -iname '*.epub' | sort)
TOTAL="${#EPUBS[@]}"
if [ "$TOTAL" -eq 0 ]; then
  echo "no epub files found under $GUTENBERG_DIR" >&2
  echo "bootstrap with: just dataset-bootstrap-gutenberg" >&2
  exit 1
fi

now_ms() {
  perl -MTime::HiRes=time -e 'printf("%.0f", time()*1000)'
}

expectation_for_file() {
  local rel="$1"
  local i
  for i in "${!RULE_PATTERNS[@]}"; do
    if [[ "$rel" == ${RULE_PATTERNS[$i]} ]]; then
      echo "${RULE_ALLOWED[$i]}|${RULE_NOTES[$i]}"
      return 0
    fi
  done
  echo "|"
}

run_timed_to_file() {
  local out_file="$1"
  local err_file="$2"
  shift 2
  local start end status
  start="$(now_ms)"
  set +e
  "$@" >"$out_file" 2>"$err_file"
  status=$?
  set -e
  end="$(now_ms)"
  echo "$status|$((end - start))"
}

mkdir -p "$OUT_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%S).$$"
REPORT_CSV="$OUT_DIR/gutenberg-smoke-${STAMP}.csv"
SUMMARY_TXT="$OUT_DIR/gutenberg-smoke-${STAMP}.summary.txt"
LATEST_CSV="$OUT_DIR/gutenberg-smoke-latest.csv"

echo "file,size_bytes,strict,validate_status,validate_ms,chapters_status,chapters_ms,chapter_count,chapter_text_status,chapter_text_ms,total_ms,status,reason" >"$REPORT_CSV"

FAILED=0
PASSED=0
EXPECTED_FAIL=0
TOTAL_MS=0

for file in "${EPUBS[@]}"; do
  rel="${file#$DATASET_ROOT/}"
  size_bytes="$(wc -c <"$file" | tr -d ' ')"
  expected_meta="$(expectation_for_file "$rel")"
  allowed_reason="${expected_meta%%|*}"
  note="${expected_meta#*|}"

  tmp_validate="$(mktemp)"
  tmp_validate_err="$(mktemp)"
  tmp_chapters="$(mktemp)"
  tmp_chapters_err="$(mktemp)"
  tmp_text="$(mktemp)"
  tmp_text_err="$(mktemp)"

  reason=""
  status="ok"
  chapter_count=0
  validate_status=0
  validate_ms=0
  chapters_status=0
  chapters_ms=0
  chapter_text_status=0
  chapter_text_ms=0

  if [ "$STRICT" -eq 1 ]; then
    timed="$(run_timed_to_file "$tmp_validate" "$tmp_validate_err" "$BIN" validate "$file" --strict)"
  else
    timed="$(run_timed_to_file "$tmp_validate" "$tmp_validate_err" "$BIN" validate "$file")"
  fi
  validate_status="${timed%%|*}"
  validate_ms="${timed##*|}"

  if [ "$validate_status" -ne 0 ]; then
    status="fail"
    reason="validate command returned $validate_status"
  elif ! grep -q '"valid":true' "$tmp_validate"; then
    status="fail"
    reason="validator reported invalid"
  fi

  timed="$(run_timed_to_file "$tmp_chapters" "$tmp_chapters_err" "$BIN" chapters "$file")"
  chapters_status="${timed%%|*}"
  chapters_ms="${timed##*|}"
  if [ "$chapters_status" -ne 0 ]; then
    status="fail"
    if [ -z "$reason" ]; then
      reason="chapters command returned $chapters_status"
    fi
  else
    chapter_count="$(sed -n 's/.*"count":\([0-9][0-9]*\).*/\1/p' "$tmp_chapters" | head -n 1)"
    chapter_count="${chapter_count:-0}"
  fi

  if [ "$chapter_count" -gt 0 ]; then
    timed="$(run_timed_to_file "$tmp_text" "$tmp_text_err" "$BIN" chapter-text "$file" --index 0 --raw)"
    chapter_text_status="${timed%%|*}"
    chapter_text_ms="${timed##*|}"
    if [ "$chapter_text_status" -ne 0 ]; then
      status="fail"
      if [ -z "$reason" ]; then
        reason="chapter-text command returned $chapter_text_status"
      fi
    fi
  fi

  if [ "$status" = "fail" ] && [ "$allowed_reason" = "chapters_decompression_failed" ]; then
    if [ "$chapters_status" -ne 0 ] && grep -qi "decompression failed" "$tmp_chapters_err"; then
      status="expected_fail"
      reason="expected chapters decompression failure"
    fi
  fi

  total_ms=$((validate_ms + chapters_ms + chapter_text_ms))
  TOTAL_MS=$((TOTAL_MS + total_ms))

  if [ "$status" = "ok" ]; then
    PASSED=$((PASSED + 1))
  elif [ "$status" = "expected_fail" ]; then
    EXPECTED_FAIL=$((EXPECTED_FAIL + 1))
  else
    FAILED=$((FAILED + 1))
  fi

  # Escape commas in reason for CSV readability.
  if [ -n "$note" ]; then
    if [ -n "$reason" ]; then
      reason="$reason ($note)"
    else
      reason="$note"
    fi
  fi
  reason="${reason//,/;}"
  echo "$rel,$size_bytes,$STRICT,$validate_status,$validate_ms,$chapters_status,$chapters_ms,$chapter_count,$chapter_text_status,$chapter_text_ms,$total_ms,$status,$reason" >>"$REPORT_CSV"

  rm -f "$tmp_validate" "$tmp_validate_err" "$tmp_chapters" "$tmp_chapters_err" "$tmp_text" "$tmp_text_err"
done

{
  echo "gutenberg smoke profile"
  echo "strict=$STRICT"
  echo "dataset_dir=$GUTENBERG_DIR"
  echo "total_files=$TOTAL"
  echo "passed=$PASSED"
  echo "expected_failures=$EXPECTED_FAIL"
  echo "failed=$FAILED"
  echo "total_ms=$TOTAL_MS"
  if [ "$TOTAL" -gt 0 ]; then
    echo "avg_ms=$((TOTAL_MS / TOTAL))"
  fi
  echo "report=$REPORT_CSV"
} | tee "$SUMMARY_TXT"

ln -sfn "$(basename "$REPORT_CSV")" "$LATEST_CSV"

if [ "$FAILED" -ne 0 ]; then
  echo "gutenberg smoke profiling found failures (see $REPORT_CSV)" >&2
  exit 1
fi

echo "gutenberg smoke profiling passed"
