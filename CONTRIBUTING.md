# Contributing

## Development

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test --all
bash scripts/ci/smoke.sh
```

## Before Opening a PR

- keep changes focused;
- add tests for behavior changes;
- update docs when CLI behavior or output changes;
- keep the JSON contract backward-compatible within the same schema version.

## Test Expectations

- new behavior needs tests;
- bug fixes need a regression test;
- prefer realistic integration coverage over shallow unit coverage when possible.

## Release-Sensitive Areas

Be careful when changing:

- `tarn run --format json`
- env/config resolution
- install/update scripts
- GitHub Action behavior
- MCP tool behavior
