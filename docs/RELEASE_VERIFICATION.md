# Release Verification

This checklist covers the local verification steps before publishing a release.

## Automated Checks

Run these before tagging:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo audit
cargo test -p demo-server
cargo test -q -p tarn --lib --bins
cargo test -p tarn --test conformance_test
cargo test -p tarn --test integration_test
cargo test -p tarn-mcp
cargo test -p tarn-lsp
bash scripts/ci/smoke.sh
(cd editors/vscode && npm ci && npm audit --omit=dev && npm run lint && npm run test:unit && npm run test:integration && npm run build)
(cd editors/zed && rustup target add wasm32-unknown-unknown && cargo fmt --check && cargo clippy --release --target wasm32-unknown-unknown -- -D warnings && cargo build --release --target wasm32-unknown-unknown)
```

What these cover:
- first-run scaffold via `tarn init`
- parser and formatter stability
- runtime JSON failures and reporter surfaces
- demo-server end-to-end flow
- conformance fixtures and example corpus
- MCP tool behavior
- LSP language-server behavior
- VS Code extension unit/integration behavior
- Zed extension WASM build behavior
- release-path smoke checks

## Manual Checks

These are still worth doing once per release candidate:

### Watch Mode Smoke

```bash
PORT=3000 cargo run -p demo-server &
cargo run -p tarn -- run examples/demo-server/hello-world.tarn.yaml --watch
```

While `--watch` is running:
- edit the expected status or body in `examples/demo-server/hello-world.tarn.yaml`
- confirm Tarn reruns automatically after the debounce window
- restore the passing assertion and confirm the next rerun goes green

### HTML and curl Export Smoke

```bash
PORT=3000 cargo run -p demo-server &
cargo run -p tarn -- run examples/demo-server/assertions.tarn.yaml \
  --format html=reports/run.html \
  --format curl=reports/failures.sh \
  --format json=reports/run.json
```

Verify that:

- the HTML report opens and renders assertion diffs
- failed steps expose `Copy cURL`
- `reports/failures.sh` contains replayable requests
- JSON includes `failure_category`, `error_code`, and remediation hints on failures

### Installer Safety

For a published release:

```bash
VERSION=vX.Y.Z
curl -LO https://github.com/NazarKalytiuk/tarn/releases/download/${VERSION}/tarn-linux-amd64.tar.gz
curl -LO https://github.com/NazarKalytiuk/tarn/releases/download/${VERSION}/tarn-checksums.txt
shasum -a 256 -c tarn-checksums.txt
```

### Update Path

After a real GitHub release exists:

```bash
tarn update
tarn --version
```

Verify that:
- the downloaded asset matches the host platform
- the installed version matches the release tag
- rerunning `tarn update` reports that you are already up to date

## GitHub Metadata

These cannot be enforced from the repo contents alone:
- repo description
- topics
- social preview image
- Discussions enabled
- release notes content
