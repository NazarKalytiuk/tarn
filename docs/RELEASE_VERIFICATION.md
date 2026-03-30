# Release Verification

This checklist covers the remaining local verification steps before publishing a release.

## Automated Checks

Run these before tagging:

```bash
cargo test -q -p tarn --lib --bins
cargo test -p tarn --test integration_test
bash scripts/ci/smoke.sh
```

What these cover:
- first-run scaffold via `tarn init`
- runtime JSON failures
- demo-server end-to-end flow
- non-JSON, empty, redirect, Unicode, large body, and invalid TLS responses
- large dry-run suites with parallel execution

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

### Installer Safety

For a published release:

```bash
curl -LO https://github.com/NazarKalytiuk/tarn/releases/download/v0.1.0/tarn-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/NazarKalytiuk/tarn/releases/download/v0.1.0/tarn-checksums.txt
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
