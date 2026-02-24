#!/usr/bin/env bash
# Grep-based memory anti-pattern lint for patterns clippy can't catch.
#
# Scans ALL production code in the workspace (core library, render, embedded-
# graphics). Excludes test modules, CLI/dev binaries, and comments.
#
# Exit 0 = clean (at or below baseline), exit 1 = new violations found.
#
# Suppress a specific line with '// allow: <reason>' on the same line.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
total_hits=0

# Known baseline: number of existing hits that are reviewed and acceptable.
# Bump this down as you fix them; the lint fails if hits > BASELINE.
BASELINE=494

# ---------------------------------------------------------------------------
# File sets
# ---------------------------------------------------------------------------

# All production library code (not binaries, not tests-only files).
ALL_LIB=(
    # Core library
    "$ROOT/src/book.rs"
    "$ROOT/src/css.rs"
    "$ROOT/src/error.rs"
    "$ROOT/src/layout.rs"
    "$ROOT/src/lib.rs"
    "$ROOT/src/metadata.rs"
    "$ROOT/src/navigation.rs"
    "$ROOT/src/render_prep.rs"
    "$ROOT/src/spine.rs"
    "$ROOT/src/streaming.rs"
    "$ROOT/src/tokenizer.rs"
    "$ROOT/src/validate.rs"
    "$ROOT/src/zip.rs"
    "$ROOT/src/async_api.rs"
    # Render crate
    "$ROOT/crates/epub-stream-render/src/lib.rs"
    "$ROOT/crates/epub-stream-render/src/render_engine.rs"
    "$ROOT/crates/epub-stream-render/src/render_ir.rs"
    "$ROOT/crates/epub-stream-render/src/render_layout.rs"
    # Embedded-graphics crate
    "$ROOT/crates/epub-stream-embedded-graphics/src/lib.rs"
    # Render-web crate (library portion)
    "$ROOT/crates/epub-stream-render-web/src/lib.rs"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# Find the line number where #[cfg(test)] starts in a file, or 999999 if absent.
test_boundary() {
    local file="$1"
    local line
    line=$(grep -n '#\[cfg(test)\]' "$file" 2>/dev/null | head -1 | cut -d: -f1)
    echo "${line:-999999}"
}

# scan_pattern [-e] "description" "grep_pattern" file...
#   -e = exclude error paths (Err(, Error, panic, assert, warn, debug, map_err)
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
                | grep -v 'warn(' \
                | grep -v 'debug(' \
                | grep -v 'return Err' \
                | grep -v 'map_err' \
                | grep -v 'anyhow' \
                | grep -v 'bail!' \
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

echo "Memory anti-pattern lint"
echo "========================"

# ---------------------------------------------------------------------------
# Allocation constructors
# ---------------------------------------------------------------------------

# 1. HashMap::new() or BTreeMap::new()
scan_pattern \
    "Map::new() — prefer with_capacity() or avoid maps" \
    '(HashMap|BTreeMap)::new\(\)' \
    "${ALL_LIB[@]}"

# 2. Box::new() — heap allocation
scan_pattern \
    "Box::new() — heap allocation; prefer stack or caller-owned" \
    'Box::new\(' \
    "${ALL_LIB[@]}"

# ---------------------------------------------------------------------------
# String/collection materialisation
# ---------------------------------------------------------------------------

# 3. format!() in production code (not error paths)
scan_pattern -e \
    "format!() — hidden String allocation" \
    'format!\(' \
    "${ALL_LIB[@]}"

# 4. .to_string() (not error paths)
scan_pattern -e \
    ".to_string() — prefer .into(), Cow, or caller buffer" \
    '\.to_string\(\)' \
    "${ALL_LIB[@]}"

# 5. .to_owned() (not error paths)
scan_pattern -e \
    ".to_owned() — prefer .into(), Cow, or borrowing" \
    '\.to_owned\(\)' \
    "${ALL_LIB[@]}"

# 6. .collect::<Vec — turbofish form
scan_pattern \
    ".collect::<Vec<_>>() — materializes iterator into heap Vec" \
    '\.collect::<Vec' \
    "${ALL_LIB[@]}"

# 7. .collect() — inferred type (catches chars().collect(), etc.)
#    Exclude itertools, error paths, and test builders.
scan_pattern -e \
    ".collect() — inferred-type collection; verify not materializing needlessly" \
    '\.collect\(\)' \
    "${ALL_LIB[@]}"

# 8. .join() — allocates a new String from slices
scan_pattern -e \
    ".join() — allocates new String; prefer write! or reusable buffer" \
    '\.join\(' \
    "${ALL_LIB[@]}"

# 9. .concat() — allocates new String/Vec from slices
scan_pattern \
    ".concat() — allocates new String/Vec; prefer push_str into buffer" \
    '\.concat\(\)' \
    "${ALL_LIB[@]}"

# ---------------------------------------------------------------------------
# Cloning owned types
# ---------------------------------------------------------------------------

# 10. .clone() — the broadest check; catches per-word style clones, String
#     clones, Vec clones, etc. Error paths excluded.
scan_pattern -e \
    ".clone() — owned-type clone; verify not in a loop or avoidable" \
    '\.clone\(\)' \
    "${ALL_LIB[@]}"

# ---------------------------------------------------------------------------
# Vec/String capacity anti-patterns
# ---------------------------------------------------------------------------

# 11. Vec::with_capacity(0) — allocates nothing, but if items follow, use
#     a real estimate. (Vec::new() is already banned by clippy.toml.)
scan_pattern \
    "Vec::with_capacity(0) — if items follow, use a real capacity estimate" \
    'Vec::with_capacity\(0\)' \
    "${ALL_LIB[@]}"

# 12. String::with_capacity(0) — same issue
scan_pattern \
    "String::with_capacity(0) — if content follows, use a real capacity estimate" \
    'String::with_capacity\(0\)' \
    "${ALL_LIB[@]}"

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
