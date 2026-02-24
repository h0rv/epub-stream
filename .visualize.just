# Visualization and web preview recipes.

# Host target used for local simulator/CLI binaries.
host_target := `if [ -n "${HOST_TEST_TARGET:-}" ]; then \
    echo "$HOST_TEST_TARGET"; \
else \
    rustc -vV | awk '/^host: / { print $2 }'; \
fi`

# Render EPUB pages to PNG snapshots for local visual layout debugging.
#
# Usage:
#   just visualize render
#   just visualize render tests/fixtures/bench/pg84-frankenstein.epub 5 0 12 target/visualize-default
render epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-default" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p epub-stream-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --cover-page-mode {{cover_page_mode}}

# Same as render, but with inter-word justification enabled.
justify epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-justify":
    RUSTC_WRAPPER= cargo run -p epub-stream-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --justify

# Larger type profile for wrap/spacing validation.
large epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="8" out="target/visualize-large":
    RUSTC_WRAPPER= cargo run -p epub-stream-embedded-graphics --bin visualize --target {{ host_target }} -- \
      {{epub}} \
      --chapter {{chapter}} \
      --start-page {{start}} \
      --pages {{pages}} \
      --out {{out}} \
      --font-size 28 \
      --line-gap 5 \
      --paragraph-gap 10

# Render with a constrained virtual-memory budget to catch large transient
# allocations locally before flashing firmware.
lowmem epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="12" out="target/visualize-lowmem" vm_kib="180000" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo build -p epub-stream-embedded-graphics --bin visualize --target {{ host_target }}
    bash -lc "ulimit -Sv {{vm_kib}}; target/{{host_target}}/debug/visualize {{epub}} --chapter {{chapter}} --start-page {{start}} --pages {{pages}} --out {{out}} --cover-page-mode {{cover_page_mode}}"

# Deterministic typography sweep for local golden-like visual review.
matrix epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" start="0" pages="6":
    just visualize render {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-default
    just visualize justify {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-justify
    just visualize large {{epub}} {{chapter}} {{start}} {{pages}} target/visualize-matrix-large

# --- Web preview ---

# Launch interactive web preview with live re-render API.
# Exposes primary typography + cover policy controls for quick e-reader tuning.
web-preview epub="tests/fixtures/bench/pg84-frankenstein.epub" port="42817" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p epub-stream-render-web --bin web-preview -- \
      {{epub}} \
      --serve \
      --open \
      --port {{port}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}

# Export standalone HTML preview snapshot (non-interactive reflow).
web-preview-export epub="tests/fixtures/bench/pg84-frankenstein.epub" out="target/web-preview/index.html" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p epub-stream-render-web --bin web-preview -- \
      {{epub}} \
      --out {{out}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}

# Chapter-scoped web preview variant.
web-preview-chapter epub="tests/fixtures/bench/pg84-frankenstein.epub" chapter="5" out="target/web-preview/chapter.html" justify_mode="adaptive-inter-word" justify_max_space_stretch="0.45" cover_page_mode="contain":
    RUSTC_WRAPPER= cargo run -p epub-stream-render-web --bin web-preview -- \
      {{epub}} \
      --chapter {{chapter}} \
      --out {{out}} \
      --justify-mode {{justify_mode}} \
      --justify-max-space-stretch {{justify_max_space_stretch}} \
      --cover-page-mode {{cover_page_mode}}
