#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DATASET_DIR="${EPUB_STREAM_DATASET_DIR:-$ROOT_DIR/tests/datasets}"
BIN="${EPUB_STREAM_CLI_BIN:-$ROOT_DIR/target/debug/epub-stream}"
OUT_DIR="$ROOT_DIR/target/datasets"
STRICT=0
EXPECTATIONS_FILE="$ROOT_DIR/scripts/datasets/expectations.tsv"
MANIFEST_FILE=""
EXPECTATIONS_PROVIDED=0

RULE_PATTERNS=()
RULE_EXPECTED=()
RULE_CODES=()
RULE_NOTES=()

MANIFEST_FILES=()
MANIFEST_EXPECTED=()
MANIFEST_CODES=()
MANIFEST_NOTES=()

while [ "$#" -gt 0 ]; do
  case "$1" in
    --strict)
      STRICT=1
      shift
      ;;
    --dataset-dir)
      DATASET_DIR="$2"
      shift 2
      ;;
    --expectations)
      EXPECTATIONS_FILE="$2"
      EXPECTATIONS_PROVIDED=1
      shift 2
      ;;
    --manifest)
      MANIFEST_FILE="$2"
      shift 2
      ;;
    *)
      echo "unknown arg: $1" >&2
      echo "usage: $0 [--strict] [--dataset-dir PATH] [--expectations FILE] [--manifest FILE]" >&2
      exit 1
      ;;
  esac
done

if [ ! -x "$BIN" ]; then
  echo "epub-stream binary not found at $BIN" >&2
  echo "build with: cargo build --features cli --bin epub-stream" >&2
  exit 1
fi

if [ -z "$MANIFEST_FILE" ]; then
  if [ ! -d "$DATASET_DIR" ]; then
    echo "dataset directory not found: $DATASET_DIR" >&2
    exit 1
  fi
  if [ ! -f "$EXPECTATIONS_FILE" ]; then
    echo "expectations file not found: $EXPECTATIONS_FILE" >&2
    exit 1
  fi
else
  if [ ! -f "$MANIFEST_FILE" ]; then
    echo "manifest file not found: $MANIFEST_FILE" >&2
    exit 1
  fi
  if [ "$EXPECTATIONS_PROVIDED" -eq 1 ] && [ ! -f "$EXPECTATIONS_FILE" ]; then
    echo "expectations file not found: $EXPECTATIONS_FILE" >&2
    exit 1
  fi
fi

load_expectations() {
  local pattern
  local expected
  local codes
  local note
  local i
  while IFS=$'\t' read -r pattern expected codes note || [ -n "${pattern:-}" ]; do
    # skip comments/blank lines
    if [ -z "${pattern:-}" ]; then
      continue
    fi
    if [[ "$pattern" == \#* ]]; then
      continue
    fi
    if [ -z "${expected:-}" ]; then
      expected="valid"
    fi
    if [ "$expected" != "valid" ] && [ "$expected" != "invalid" ]; then
      echo "invalid expectation '$expected' for pattern '$pattern'" >&2
      exit 1
    fi
    for i in "${!RULE_PATTERNS[@]}"; do
      if [ "${RULE_PATTERNS[$i]}" = "$pattern" ]; then
        echo "duplicate expectation pattern: $pattern" >&2
        exit 1
      fi
    done
    RULE_PATTERNS+=("$pattern")
    RULE_EXPECTED+=("${expected:-valid}")
    RULE_CODES+=("${codes:-}")
    RULE_NOTES+=("${note:-}")
  done <"$EXPECTATIONS_FILE"
}

expected_for_file() {
  local rel="$1"
  local i
  for i in "${!RULE_PATTERNS[@]}"; do
    if [[ "$rel" == ${RULE_PATTERNS[$i]} ]]; then
      echo "${RULE_EXPECTED[$i]}|${RULE_CODES[$i]}|${RULE_NOTES[$i]}"
      return 0
    fi
  done
  echo "valid||"
}

load_manifest() {
  local path
  local expected
  local codes
  local note
  local abs
  while IFS=$'\t' read -r path expected codes note || [ -n "${path:-}" ]; do
    if [ -z "${path:-}" ]; then
      continue
    fi
    if [[ "$path" == \#* ]]; then
      continue
    fi
    if [ -z "${expected:-}" ]; then
      expected="valid"
    fi
    if [ "$expected" != "valid" ] && [ "$expected" != "invalid" ]; then
      echo "invalid expectation '$expected' in manifest for '$path'" >&2
      exit 1
    fi
    if [[ "$path" = /* ]]; then
      abs="$path"
    else
      abs="$ROOT_DIR/$path"
    fi
    if [ ! -f "$abs" ]; then
      echo "manifest file not found: $path" >&2
      exit 1
    fi
    MANIFEST_FILES+=("$abs")
    MANIFEST_EXPECTED+=("$expected")
    MANIFEST_CODES+=("${codes:-}")
    MANIFEST_NOTES+=("${note:-}")
  done <"$MANIFEST_FILE"
}

has_code() {
  local output="$1"
  local code="$2"
  echo "$output" | grep -q "\"code\":\"$code\""
}

if [ -z "$MANIFEST_FILE" ] || [ "$EXPECTATIONS_PROVIDED" -eq 1 ]; then
  load_expectations
fi

if [ -n "$MANIFEST_FILE" ]; then
  load_manifest
fi

mkdir -p "$OUT_DIR"
STAMP="$(date -u +%Y%m%dT%H%M%S).$$"
REPORT_JSONL="$OUT_DIR/validate-${STAMP}.jsonl"
SUMMARY_TXT="$OUT_DIR/validate-${STAMP}.summary.txt"
MISMATCH_TSV="$OUT_DIR/validate-${STAMP}.mismatches.tsv"
LATEST_LINK="$OUT_DIR/latest.jsonl"
LATEST_MISMATCH_LINK="$OUT_DIR/latest.mismatches.tsv"

if [ -n "$MANIFEST_FILE" ]; then
  EPUBS=("${MANIFEST_FILES[@]}")
else
  EPUBS=()
  while IFS= read -r epub_path; do
    EPUBS+=("$epub_path")
  done < <(find "$DATASET_DIR" -type f -iname '*.epub' | sort)
fi
TOTAL="${#EPUBS[@]}"
if [ "$TOTAL" -eq 0 ]; then
  if [ -n "$MANIFEST_FILE" ]; then
    echo "no epub files listed in manifest $MANIFEST_FILE" >&2
  else
    echo "no epub files found under $DATASET_DIR" >&2
  fi
  exit 1
fi

echo "validating $TOTAL epub files (strict=$STRICT)"

MATCHED=0
MISMATCHED=0
EXPECTED_VALID=0
EXPECTED_INVALID=0
ACTUAL_VALID=0
ACTUAL_INVALID=0
FILES_WITH_WARNINGS=0
ERROR_DIAGS=0
WARN_DIAGS=0

echo -e "file\texpected\tactual\tstatus\treason\tnote" >"$MISMATCH_TSV"

for idx in "${!EPUBS[@]}"; do
  file="${EPUBS[$idx]}"
  rel="${file#$DATASET_DIR/}"
  expected=""
  required_codes=""
  note=""

  if [ -n "$MANIFEST_FILE" ]; then
    expected="${MANIFEST_EXPECTED[$idx]}"
    required_codes="${MANIFEST_CODES[$idx]}"
    note="${MANIFEST_NOTES[$idx]}"
  fi

  if [ -z "$expected" ] || { [ -z "$required_codes" ] && [ -z "$note" ]; }; then
    if [ "${#RULE_PATTERNS[@]}" -gt 0 ] && [[ "$file" == "$DATASET_DIR"/* ]]; then
      expectation="$(expected_for_file "$rel")"
      if [ -z "$expected" ]; then
        expected="${expectation%%|*}"
      fi
      if [ -z "$required_codes" ]; then
        required_codes="${expectation#*|}"
        required_codes="${required_codes%%|*}"
      fi
      if [ -z "$note" ]; then
        note="${expectation##*|}"
      fi
    fi
  fi

  if [ -z "$expected" ]; then
    expected="valid"
  fi

  if [ "$expected" = "valid" ]; then
    EXPECTED_VALID=$((EXPECTED_VALID + 1))
  else
    EXPECTED_INVALID=$((EXPECTED_INVALID + 1))
  fi

  if [ "$STRICT" -eq 1 ]; then
    set +e
    output="$("$BIN" validate "$file" --strict 2>/dev/null)"
    status=$?
    set -e
  else
    set +e
    output="$("$BIN" validate "$file" 2>/dev/null)"
    status=$?
    set -e
  fi

  # fast, dependency-free counters from compact JSON output
  if echo "$output" | grep -q '"warning_count":[1-9]'; then
    FILES_WITH_WARNINGS=$((FILES_WITH_WARNINGS + 1))
  fi

  if echo "$output" | grep -q '"valid":true'; then
    actual="valid"
    ACTUAL_VALID=$((ACTUAL_VALID + 1))
  else
    actual="invalid"
    ACTUAL_INVALID=$((ACTUAL_INVALID + 1))
  fi

  err_count="$(echo "$output" | sed -n 's/.*"error_count":\([0-9][0-9]*\).*/\1/p')"
  warn_count="$(echo "$output" | sed -n 's/.*"warning_count":\([0-9][0-9]*\).*/\1/p')"
  err_count="${err_count:-0}"
  warn_count="${warn_count:-0}"
  ERROR_DIAGS=$((ERROR_DIAGS + err_count))
  WARN_DIAGS=$((WARN_DIAGS + warn_count))

  reason=""
  if [ "$expected" = "valid" ]; then
    if [ "$STRICT" -eq 1 ]; then
      if [ "$status" -ne 0 ] && [ "$actual" != "valid" ]; then
        reason="expected valid+strict-pass, got nonzero status"
      fi
    else
      if [ "$actual" != "valid" ]; then
        reason="expected valid, got invalid"
      fi
    fi
  elif [ "$expected" = "invalid" ]; then
    if [ "$actual" != "invalid" ]; then
      reason="expected invalid, got valid"
    fi
  else
    reason="unknown expected value '$expected'"
  fi

  if [ -z "$reason" ] && [ -n "$required_codes" ]; then
    IFS=',' read -r -a codes_arr <<<"$required_codes"
    for code in "${codes_arr[@]}"; do
      code="$(echo "$code" | xargs)"
      [ -z "$code" ] && continue
      if ! has_code "$output" "$code"; then
        reason="missing required diagnostic code '$code'"
        break
      fi
    done
  fi

  if [ -z "$reason" ]; then
    MATCHED=$((MATCHED + 1))
  else
    MISMATCHED=$((MISMATCHED + 1))
    printf "%s\t%s\t%s\t%d\t%s\t%s\n" "$rel" "$expected" "$actual" "$status" "$reason" "$note" >>"$MISMATCH_TSV"
  fi

  printf '{"file":"%s","exit_status":%d,"result":%s}\n' \
    "$(printf "%s" "$file" | sed 's/"/\\"/g')" \
    "$status" \
    "$output" >>"$REPORT_JSONL"
done

{
  echo "dataset_dir=$DATASET_DIR"
  echo "manifest_file=${MANIFEST_FILE:-}"
  echo "expectations_file=$EXPECTATIONS_FILE"
  echo "report_jsonl=$REPORT_JSONL"
  echo "mismatch_tsv=$MISMATCH_TSV"
  echo "total_files=$TOTAL"
  echo "matched_files=$MATCHED"
  echo "mismatched_files=$MISMATCHED"
  echo "expected_valid=$EXPECTED_VALID"
  echo "expected_invalid=$EXPECTED_INVALID"
  echo "actual_valid=$ACTUAL_VALID"
  echo "actual_invalid=$ACTUAL_INVALID"
  echo "files_with_warnings=$FILES_WITH_WARNINGS"
  echo "diagnostics_error_total=$ERROR_DIAGS"
  echo "diagnostics_warning_total=$WARN_DIAGS"
  echo "strict_mode=$STRICT"
} | tee "$SUMMARY_TXT"

ln -sfn "$(basename "$REPORT_JSONL")" "$LATEST_LINK"
ln -sfn "$(basename "$MISMATCH_TSV")" "$LATEST_MISMATCH_LINK"

if [ "$MISMATCHED" -ne 0 ]; then
  echo "validation mismatches found. see: $MISMATCH_TSV" >&2
  exit 1
fi
