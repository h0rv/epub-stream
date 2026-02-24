# Publishing

This workspace publishes multiple crates and should be released in dependency order.

## Best practice

Use a hybrid model:

1. Local preflight for fast feedback:
   - `just publish release-preflight`
2. Actual publish via CI (manual `workflow_dispatch`) for auditability and repeatability.

This keeps daily development fast while avoiding ad-hoc local release mistakes.

## Release order

1. `epub-stream`
2. `epub-stream-render`
3. `epub-stream-embedded-graphics`
4. `epub-stream-render-web`

The `just` recipes enforce this order.

## Local commands

- Full preflight:
  - `just publish release-preflight`
- Publish dry-run only:
  - `just publish publish-dry-run-all`
- Publish for real (requires `CARGO_REGISTRY_TOKEN`):
  - `just publish publish-all`

## CI publish

Use `.github/workflows/publish.yml` with `workflow_dispatch`:

- `dry_run=true`: validates publishability without uploading.
- `dry_run=false`: publishes all crates in order.

The workflow calls the same `just` recipes as local usage to keep the process DRY.
