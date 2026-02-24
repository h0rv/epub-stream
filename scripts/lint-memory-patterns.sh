#!/usr/bin/env bash
# Grep-based memory anti-pattern lint for patterns clippy can't catch.
#
# Targets the hot-path crates: render engine, layout, embedded-graphics,
# and the core library's render_prep/tokenizer/streaming modules.
#
# Exit 0 = clean (at or below baseline), exit 1 = new violations found.
#
# Suppress a specific line with '// allow: <reason>' on the same line.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
total_hits=0

# Known baseline: number of existing hits that are reviewed and acceptable.
# Bump this down as you fix them; the lint fails if hits > BASELINE.
BASELINE=30

# Files to scan: hot-path production code only (not tests, not CLI).
HOT_PATHS=(
    "$ROOT/src/render_prep.rs"
    "$ROOT/src/tokenizer.rs"
    "$ROOT/src/streaming.rs"
    "$ROOT/src/zip.rs"
    "$ROOT/crates/epub-stream-render/src/render_layout.rs"
    "$ROOT/crates/epub-stream-render/src/render_engine.rs"
    "$ROOT/crates/epub-stream-render/src/render_ir.rs"
    "$ROOT/crates/epub-stream-embedded-graphics/src/lib.rs"
)

# Layout/render files only (tighter checks).
RENDER_PATHS=(
    "$ROOT/crates/epub-stream-render/src/render_layout.rs"
    "$ROOT/crates/epub-stream-render/src/render_engine.rs"
    "$ROOT/crates/epub-stream-embedded-graphics/src/lib.rs"
)

# Find the line number where #[cfg(test)] starts in a file, or 999999 if absent.
test_boundary() {
    local file="$1"
    local line
    line=$(grep -n '#\[cfg(test)\]' "$file" 2>/dev/null | head -1 | cut -d: -f1)
    echo "${line:-999999}"
}

# Helper: scan files for a pattern, filtering out allowed lines and test code.
# Skips: #[allow, // allow:, comments, everything at/after #[cfg(test)]
# Usage: scan_pattern [-e] "description" "grep_pattern" file...
#   -e  = exclude error paths (lines containing Err(, Error, error, panic, assert, warn)
scan_pattern() {
    local exclude_errors=false
    if [ "${1:-}" = "-e" ]; then
        exclude_errors=true
        shift
    fi
    local desc="$1"
    local pattern="$2"
    shift 2
    local category_hits=0
    for file in "$@"; do
        [ -f "$file" ] || continue

        local boundary
        boundary=$(test_boundary "$file")

        local matches
        matches=$(grep -nE "$pattern" "$file" 2>/dev/null \
            | grep -v '#\[allow' \
            | grep -v '// allow:' \
            | grep -v '^ *[0-9]*: *#' \
            | grep -v '^ *[0-9]*: *//' \
            || true)

        # Filter lines at or after #[cfg(test)] boundary
        if [ -n "$matches" ] && [ "$boundary" -lt 999999 ]; then
            matches=$(echo "$matches" | awk -F: -v b="$boundary" '$1 < b' || true)
        fi

        # Filter error paths if requested
        if $exclude_errors && [ -n "$matches" ]; then
            matches=$(echo "$matches" \
                | grep -v 'Err(' \
                | grep -v 'Error' \
                | grep -v 'error' \
                | grep -v 'panic' \
                | grep -v 'assert' \
                | grep -v 'warn' \
                | grep -v 'debug' \
                | grep -v 'return Err' \
                | grep -v 'map_err' \
                || true)
        fi

        if [ -n "$matches" ]; then
            local count
            count=$(echo "$matches" | wc -l | tr -d ' ')
            category_hits=$((category_hits + count))
            if [ "$category_hits" -eq "$count" ]; then
                echo ""
                echo "=== $desc ==="
            fi
            while IFS= read -r line; do
                echo "  ${file#"$ROOT/"}:$line"
            done <<< "$matches"
        fi
    done
    total_hits=$((total_hits + category_hits))
}

echo "Memory anti-pattern lint (hot paths only)"
echo "=========================================="

# 1. .collect::<Vec in hot-path code (materializes iterator into heap alloc)
scan_pattern \
    ".collect::<Vec<_>>() — materializes iterator into heap allocation" \
    '\.collect::<Vec' \
    "${HOT_PATHS[@]}"

# 2. format!() in render/layout hot paths (not error paths)
#    Error-path format! is fine; layout/render format! is a per-page String alloc.
scan_pattern -e \
    "format!() in render/layout — hidden String allocation per call" \
    'format!\(' \
    "${RENDER_PATHS[@]}"

# 3. .to_string() in render/layout production code (not error paths, not tests)
scan_pattern -e \
    ".to_string() in render/layout — prefer write! or caller buffer" \
    '\.to_string\(\)' \
    "${RENDER_PATHS[@]}"

# 4. HashMap::new() or BTreeMap::new() in hot paths
scan_pattern \
    "Map::new() — prefer with_capacity() or avoid maps in hot paths" \
    '(HashMap|BTreeMap)::new\(\)' \
    "${HOT_PATHS[@]}"

# 5. Box::new() in layout/render code
scan_pattern \
    "Box::new() — heap allocation; prefer stack or caller-owned storage" \
    'Box::new\(' \
    "${RENDER_PATHS[@]}"

echo ""
echo "Total hits: $total_hits (baseline: $BASELINE)"
if [ "$total_hits" -gt "$BASELINE" ]; then
    echo "FAIL — $((total_hits - BASELINE)) new violation(s) above baseline."
    echo "Fix the new violations or suppress with '// allow: <reason>'."
    exit 1
else
    echo "OK — at or below baseline. Fix existing hits to lower the baseline."
    exit 0
fi
