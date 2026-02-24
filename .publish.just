# Release and publishing recipes.

# Crates.io publish order (dependency-aware).
publish-order:
    @echo "epub-stream epub-stream-render epub-stream-embedded-graphics epub-stream-render-web"

# Local package sanity check for one crate.
package crate:
    RUSTC_WRAPPER= cargo package -p {{crate}}

# Local package sanity check without dependency verification (for unpublished local deps).
package-no-verify crate:
    RUSTC_WRAPPER= cargo package -p {{crate}} --no-verify

# Local package sanity check for all crates in publish order.
package-all:
    just publish package epub-stream
    just publish package-no-verify epub-stream-render
    just publish package-no-verify epub-stream-embedded-graphics
    just publish package-no-verify epub-stream-render-web

# Dry-run publish for one crate.
publish-dry-run crate:
    RUSTC_WRAPPER= cargo publish -p {{crate}} --dry-run

# Dry-run publish without dependency verification (for unpublished local deps).
publish-dry-run-no-verify crate:
    RUSTC_WRAPPER= cargo publish -p {{crate}} --dry-run --no-verify

# Dry-run publish for all crates in dependency order.
publish-dry-run-all:
    just publish publish-dry-run epub-stream
    just publish publish-dry-run-no-verify epub-stream-render
    just publish publish-dry-run-no-verify epub-stream-embedded-graphics
    just publish publish-dry-run-no-verify epub-stream-render-web

# Full release preflight before publishing.
release-preflight:
    just ci
    just publish package-all
    just publish publish-dry-run-all

# Publish all crates to crates.io in dependency order.
# Requires CARGO_REGISTRY_TOKEN to be configured.
publish-all:
    @bash -eu -o pipefail -c '\
      publish_or_skip() { \
        crate="$1"; \
        echo "Publishing $crate..."; \
        log_file=$(mktemp); \
        if RUSTC_WRAPPER= cargo publish -p "$crate" 2>&1 | tee "$log_file"; then \
          rm -f "$log_file"; \
          return 0; \
        fi; \
        if rg -q "already exists on crates.io index" "$log_file"; then \
          echo "Skipping $crate (already published)."; \
          rm -f "$log_file"; \
          return 0; \
        fi; \
        rm -f "$log_file"; \
        return 1; \
      }; \
      publish_or_skip epub-stream; \
      sleep 30; \
      publish_or_skip epub-stream-render; \
      sleep 30; \
      publish_or_skip epub-stream-embedded-graphics; \
      sleep 30; \
      publish_or_skip epub-stream-render-web; \
    '
