# Tarn Competitiveness Roadmap

**Date**: 2026-03-30
**Status**: Completed on 2026-04-01

This document is now a historical roadmap record.

All planned items `T01` through `T50` were implemented. Use this file for scope history and sequencing context, not as the live backlog. For current product direction, use [`docs/TARN_PRODUCT_STRATEGY.md`](./TARN_PRODUCT_STRATEGY.md).

One post-roadmap addition has been appended: `T51`–`T58` capture the additive CLI contract needed by the new VS Code extension work. See [Post-Roadmap Additions](#post-roadmap-additions-vs-code-extension-contract) at the end of this file and [`docs/VSCODE_EXTENSION.md`](./VSCODE_EXTENSION.md) for the full extension spec.

## Completion Summary

- `v0.2` goals completed: transport parity baseline, cookies, redaction, reporter hardening, CI matrix
- `v0.3` goals completed: form support, custom methods, richer captures, transform-lite, more assertions, multi-output, curl export, better validation
- `v0.4` goals completed: machine error codes, remediation hints, JSON modes, MCP diagnostics, VS Code support, formatter, docs site, Hurl migration tooling
- `v0.5` goals completed: benchmark thresholds and exports, project policy, include params, named environments, auth helpers, HTML report upgrades, impacted watch mode, public conformance suite

## Goal

Turn Tarn into a competitive `AI-first JSON/API test runner` with enough HTTP and reporting depth for real teams, without bloating it into a full Hurl clone.

## Positioning

Tarn should not try to become "another Hurl".

The right strategy is:

- Close the minimum enterprise HTTP gap that blocks adoption.
- Keep doubling down on the features Hurl does not have: MCP, setup/teardown, named cookie jars, polling, Lua, structured failure taxonomy, AI-friendly JSON.
- Avoid large detours that add surface area but do not strengthen Tarn's core wedge.

## Prioritization Rules

- `P0`: Adoption blockers. Without these, Tarn is hard to justify in serious environments.
- `P1`: Expressiveness improvements with strong practical payoff.
- `P2`: Product and ecosystem improvements that reduce switching cost and improve usability.
- `P3`: Moat-building features that make Tarn uniquely valuable.
- `Not now`: Features that are expensive, off-strategy, or premature.

## P0 · v0.2 · Adoption Blockers

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| T01 | Add `proxy` / `no-proxy` in CLI and runtime config | 10 | M | - | `tarn/src/main.rs`, `tarn/src/http.rs`, `tarn/src/model.rs` |
| T02 | Add `cacert`, `cert`, `key`, `insecure` | 10 | M | - | `tarn/src/main.rs`, `tarn/src/http.rs` |
| T03 | Split `connect-timeout` from total timeout | 9 | S | - | `tarn/src/http.rs`, `tarn/src/runner.rs` |
| T04 | Add first-class `follow-redirects` and `max-redirs` | 9 | S | - | `tarn/src/http.rs`, `tarn/src/model.rs` |
| T05 | Add explicit `http1.1` / `http2` control | 7 | M | - | `tarn/src/http.rs`, `tarn/src/main.rs` |
| T06 | Replace the simple cookie jar with a spec-aware jar | 10 | L | - | `tarn/src/cookie.rs`, `tarn/src/runner.rs` |
| T07 | Support `domain`, `path`, `expiry`, `secure`, `httponly`, `samesite` | 9 | M | T06 | `tarn/src/cookie.rs` |
| T08 | Support cookie jar import/export file | 7 | M | T06 | `tarn/src/cookie.rs`, `tarn/src/main.rs` |
| T09 | Add whole-body assertions for text/JSON with unified diff | 9 | M | - | `tarn/src/assert/body.rs`, `tarn/src/report/human.rs`, `tarn/src/report/json.rs` |
| T10 | Replace hardcoded secret redaction with configurable redaction | 8 | M | - | `tarn/src/report/json.rs`, `tarn/src/report/human.rs`, `tarn/src/model.rs` |
| T11 | Add redacted captures/vars at DSL level | 7 | M | T10 | `tarn/src/capture.rs`, `tarn/src/interpolation.rs` |
| T12 | Add golden tests for JSON/JUnit/TAP/HTML reporters | 9 | M | T09, T10 | `tarn/tests/integration_test.rs`, `tarn/src/report/` |
| T13 | Add integration fixtures for redirects/TLS/cookies/timeouts | 10 | L | T01-T08 | `demo-server/`, `tarn/tests/integration_test.rs` |
| T14 | Add Linux/macOS/Windows CI matrix | 8 | S | T12, T13 | `.github/workflows/ci.yml` |

## P1 · v0.3 · Expressiveness Without DSL Bloat

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| T15 | Add first-class `form:` support | 8 | S | - | `tarn/src/model.rs`, `tarn/src/http.rs` |
| T16 | Add custom HTTP method support | 6 | S | - | `tarn/src/http.rs`, `tarn/src/parser.rs` |
| T17 | Add capture from `status` | 7 | S | - | `tarn/src/capture.rs` |
| T18 | Add capture from final `url` | 7 | S | T04 | `tarn/src/capture.rs`, `tarn/src/http.rs` |
| T19 | Add captures from cookie / header regex / body regex | 8 | M | - | `tarn/src/capture.rs` |
| T20 | Add transform-lite: `first`, `last`, `count`, `join` | 8 | M | T17-T19 | `tarn/src/capture.rs`, `tarn/src/interpolation.rs` |
| T21 | Add transform-lite: `split`, `replace`, `to_int`, `to_string` | 8 | M | T20 | `tarn/src/capture.rs`, `tarn/src/interpolation.rs` |
| T22 | Add assertions `is_uuid`, `is_date`, `is_ipv4`, `is_ipv6` | 6 | S | - | `tarn/src/assert/body.rs` |
| T23 | Add `is_empty` / `empty` semantics separate from `not_empty` | 5 | S | - | `tarn/src/assert/body.rs` |
| T24 | Add `bytes`, `sha256`, `md5` assertions | 6 | M | T09 | `tarn/src/assert/body.rs`, `tarn/src/http.rs` |
| T25 | Add redirect assertions: final URL and redirect count | 7 | M | T04, T18 | `tarn/src/assert/`, `tarn/src/http.rs` |
| T26 | Support multiple output formats in one run | 6 | M | T12 | `tarn/src/main.rs`, `tarn/src/report/mod.rs` |
| T27 | Add curl export for failed requests and full suite | 7 | M | T26 | `tarn/src/report/`, `tarn/src/assert/types.rs` |
| T28 | Improve fuzzy validation for all keys/operators, not just known typos | 7 | S | - | `tarn/src/parser.rs` |

## P2 · v0.4 · AI Loop and Product DX

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| T29 | Add stable machine error codes next to `failure_category` | 8 | S | - | `tarn/src/assert/types.rs`, `tarn/src/report/json.rs` |
| T30 | Add remediation hints in JSON output | 9 | M | T29 | `tarn/src/report/json.rs`, `tarn/src/runner.rs` |
| T31 | Add compact/verbose JSON modes for LLM vs CI usage | 7 | S | T29 | `tarn/src/main.rs`, `tarn/src/report/json.rs` |
| T32 | Add MCP diagnostics helper such as `tarn_fix_plan` | 8 | M | T29, T30 | `tarn-mcp/src/tools.rs` |
| T33 | Add MCP contract examples and snapshot tests | 7 | S | T32 | `tarn-mcp/src/tools.rs`, `schemas/v1/report.json` |
| T34 | Build VS Code extension: syntax, schema wiring, snippets | 10 | M | T31 | `schemas/v1/testfile.json`, `schemas/v1/report.json`, `docs/` |
| T35 | Add `tarn fmt` or format/normalize command | 7 | M | T28 | `tarn/src/main.rs`, `tarn/src/parser.rs` |
| T36 | Improve `tarn init` templates for auth, polling, multipart, multi-user | 6 | S | - | `tarn/src/main.rs`, `examples/` |
| T37 | Expand example corpus by 5x and use it as regression coverage | 8 | M | T15-T25 | `examples/`, `tarn/tests/integration_test.rs` |
| T38 | Build docs site with canonical guides | 8 | M | T34, T37 | `README.md`, `docs/` |
| T39 | Publish to crates.io, Homebrew, Docker, Windows releases | 10 | M | T14 | `.github/workflows/release.yml`, `Cargo.toml` |
| T40 | Write Hurl -> Tarn migration guide and parity matrix | 8 | S | T15-T27 | `docs/` |
| T41 | Build a simple Hurl converter for common cases | 7 | L | T40 | `tarn/`, `docs/` |

## P3 · v0.5 · Moat and Advanced Workflow

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| T42 | Deepen benchmark mode: thresholds, exports, CI gates | 7 | M | T12 | `tarn/src/bench.rs`, `tarn/src/main.rs` |
| T43 | Add more detailed timings: connect/TLS/TTFB where feasible | 6 | L | T03 | `tarn/src/http.rs`, `tarn/src/bench.rs` |
| T44 | Add project-level policy/config file for defaults, redaction, retries | 8 | M | T10 | `tarn/src/parser.rs`, `tarn/src/model.rs` |
| T45 | Add richer include system with params/overrides for reusable step packs | 8 | M | T28, T44 | `tarn/src/parser.rs`, `tarn/src/model.rs` |
| T46 | Make named environments a first-class project concept | 7 | M | T44 | `tarn/src/env.rs`, `tarn/src/main.rs` |
| T47 | Add auth helpers: bearer/basic, while keeping raw-header escape hatch | 6 | S | T15 | `tarn/src/model.rs`, `tarn/src/http.rs` |
| T48 | Improve HTML report: diff views, collapsible payloads, copy-curl | 6 | M | T09, T27 | `tarn/src/report/html.rs`, `tarn/src/report/json.rs` |
| T49 | Make watch mode rerun only impacted files/includes | 5 | M | T45 | `tarn/src/watch.rs`, `tarn/src/parser.rs` |
| T50 | Publish a public conformance suite and CI compatibility badge | 7 | M | T37, T39 | `tarn/tests/`, `.github/workflows/ci.yml` |

## Not Now

| ID | Idea | Why not now |
|---|---|---|
| N01 | Full XPath/HTML engine | Not Tarn's core wedge. Large surface, limited strategic payoff. |
| N02 | Full Hurl-style filter DSL | Too much complexity. Transform-lite + Lua is a better tradeoff. |
| N03 | NTLM / Negotiate / exotic auth parity | Lower value than proxy/certs/cookies. |
| N04 | Full protocol-completeness race with libcurl | Tarn should not try to beat libcurl at libcurl's own game. |
| N05 | Another scripting language on top of YAML | Lua already provides the necessary escape hatch. |

## Recommended Release Order

1. `v0.2`: T01-T14
2. `v0.3`: T15-T28
3. `v0.4`: T29-T41
4. `v0.5`: T42-T50
5. `v1.0`: hardening, documentation freeze, compatibility guarantees

## Highest Leverage Work

### Biggest business impact

- T01-T10
- T34
- T39

### Biggest moat

- T29-T33
- T42-T45

### Cheapest quick wins

- T03
- T04
- T15
- T22
- T23
- T28
- T36
- T40

## Short Version

1. Close the transport gap.
2. Fix cookies properly.
3. Add body diff and stronger captures/transforms.
4. Double down on MCP, failure taxonomy, and setup/teardown workflows.
5. Improve distribution, editor support, documentation, and test trust.

## Post-Roadmap Additions: VS Code Extension Contract

**Date added**: 2026-04-10
**Status**: Planning
**Tracking issue**: T51 (umbrella)
**Spec**: [`docs/VSCODE_EXTENSION.md`](./VSCODE_EXTENSION.md)

`T34` shipped the declarative VS Code extension (language id, grammar, snippets, schema wiring) that lives in `editors/vscode/`. The next phase promotes it into a full extension host integration: Testing API, CodeLens, live streaming, run-at-cursor, authoring features, and MCP backend support.

`T51`–`T57` are the additive CLI and runtime changes needed to unlock that extension work. All are backwards compatible — existing output formats, existing flags, existing schemas stay untouched. Each item below corresponds to a numbered section in `docs/VSCODE_EXTENSION.md`.

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| T51 | Add `--select FILE::TEST::STEP` flag, repeatable, ANDs with `--tag`; enables run-test-at-cursor, rerun-failed, per-step curl export | 9 | M | - | `tarn/src/main.rs`, `tarn/src/runner.rs`, `tarn/tests/integration_test.rs` |
| T52 | Add `tarn validate --format json` emitting `{files:[{file, valid, errors:[{message,line,column,path}]}]}` | 8 | S | - | `tarn/src/main.rs`, `tarn/src/parser.rs`, `schemas/v1/` |
| T53 | Add NDJSON progress reporter behind `--ndjson` / `--format ndjson`; emits `file_started`, `step_finished`, `test_finished`, `file_finished`, `done`; co-exists with final JSON | 9 | S | - | `tarn/src/report/progress.rs`, `tarn/src/report/mod.rs`, `tarn/src/main.rs` |
| T54 | Add `cookies: per-test` model option and `--cookie-jar-per-test` flag; resets jar between named tests within a file **[shipped v0.4.1]** | 6 | M | - | `tarn/src/cookie.rs`, `tarn/src/model.rs`, `tarn/src/runner.rs` |
| T55 | Add `location: {file, line, column}` to `StepResult` and `AssertionFailure`; optional field in `schemas/v1/report.json` **[shipped v0.4.2]** | 8 | M | T52 | `tarn/src/assert/types.rs`, `tarn/src/parser.rs`, `tarn/src/runner.rs`, `schemas/v1/report.json` |
| T56 | Add `tarn env --json` returning named environments, source files, and resolved variables with redaction applied | 6 | S | - | `tarn/src/main.rs`, `tarn/src/env.rs`, `tarn/src/config.rs` |
| T57 | Add `tarn list --file PATH --format json` for scoped discovery of a single file **[shipped v0.4.3]** | 5 | S | - | `tarn/src/main.rs`, `tarn/src/parser.rs` |
| T58 | Add `--redact-header NAME` CLI flag (repeatable) merging with the configured redaction list; enables editors and CI to extend redaction without editing `tarn.config.yaml` **[shipped v0.4.4]** | 5 | S | - | `tarn/src/main.rs`, `tarn/src/model.rs`, `tarn/src/report/json.rs` |

### Acceptance Criteria

**T51** `--select`
- `tarn run --select tests/users.tarn.yaml::create_and_verify_user` runs exactly that test, skips siblings in the same file, still runs setup and teardown.
- Step form: `--select tests/users.tarn.yaml::create_and_verify_user::"Create user"` runs exactly that step.
- Flag is repeatable; multiple selectors union within a file and across files.
- Combined with `--tag`, the selectors AND with tag filters.
- Exit codes unchanged.
- Integration test coverage: select-test, select-step, select-across-files, select-plus-tag, unknown selector is a parse error with `error_code: validation_failed`.

**T52** Structured validate
- `tarn validate --format json` exits `0` iff all files are valid.
- Output schema documented and added to `schemas/v1/` (new file or extension of an existing one).
- Every error row has `line` and `column` derived from serde_yaml, pointing at the offending node.
- Human output unchanged when `--format human` (or default) is used.
- Golden test added to `tarn/tests/integration_test.rs` covering parse error, assertion-block error, unknown field.

**T53** NDJSON reporter
- `tarn run --ndjson` streams one JSON object per line to stdout as events occur, flushes after every line.
- Events: `file_started`, `setup_finished`, `step_finished`, `test_finished`, `teardown_finished`, `file_finished`, `done`.
- `step_finished` includes `{file, test, step, step_index, status, duration_ms}` plus `failure_category`, `error_code`, `remediation_hints` on failure.
- `done` event carries the final summary identical to the final JSON report's `summary` block.
- `--ndjson` composes with `--format json=path` (NDJSON to stdout, final report to file).
- Parallel mode: events are emitted under the existing progress mutex so interleaved lines never tear.
- Polling steps emit a `poll_attempt` event per retry.
- Unit test covering reporter trait implementation; integration test covering one full run's event sequence.

**T54** Per-test cookie jar
- `cookies: per-test` in the test file clears the jar between named tests; setup and teardown share the file-level jar.
- `--cookie-jar-per-test` CLI flag overrides the file setting.
- Existing `cookies: "auto"` / `cookies: "off"` behavior unchanged.
- Named jars (multi-user scenarios) take precedence over per-test reset.
- Integration fixture uses a session cookie to prove isolation between tests.

**T55** Result location metadata
- Every `StepResult` in JSON output has `location: {file, line, column}` pointing at the step name node.
- Every `AssertionFailure` has `location` pointing at the failing assertion node.
- Field is optional for backwards compatibility; existing consumers keep working.
- `schema_version` stays `1`; a new optional field does not break compat.
- Setup and teardown steps also carry location.

**T56** `tarn env --json`
- `tarn env --json` prints `{environments: [{name, source_file, vars: {...}, is_active}]}`.
- Respects the full env resolution chain documented in `tarn/src/env.rs`.
- Redaction is applied: values matching the configured redaction patterns print as `***`.
- Exit code `0` on success, `2` on configuration error.

**T57** Scoped `tarn list`
- `tarn list --file path/to/file.tarn.yaml --format json` prints only that file's tests and steps.
- Output is a strict subset of the current `tarn list` output so existing consumers are unaffected.
- Unknown file is exit code `2`.

**T58** `--redact-header` CLI flag
- `tarn run --redact-header x-custom-token --redact-header x-debug` merges these header names into the effective redaction list.
- Flag is repeatable; values are case-insensitive.
- Merges with both the default built-in list and any `redaction:` block loaded from config or test files. Never narrows — only widens.
- Applies to both JSON and human reports.
- No change to `tarn.config.yaml` semantics; this is a pure CLI augmentation.

### Sequencing and Release Plan

- `T51`, `T52`, `T53` land together in Tarn `v0.5.0` to unblock extension Phase 2.
- `T54` and `T55` land in Tarn `v0.5.1` to unlock extension Phase 3 authoring features and Phase 5 subset-run correctness.
- `T56` and `T57` land in Tarn `v0.5.2`, small and independent.
- The VS Code extension ships Phase 1 against Tarn `v0.4.0` using the fallback polling path and does not block on any of these.

### Mandatory Pre-Commit Checks

Every `T5x` item is subject to the existing gate in `CLAUDE.md`:

```
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Each item also adds its own golden or integration test so the new surface is covered from day one.

## Phase L: LSP for Claude Code

Epic **NAZ-289** — `tarn-lsp` Language Server for Claude Code and non-VS-Code editors. Delivered as five tickets (L1.1 through L1.5) that each flip on exactly one LSP capability. Canonical spec lives in [`docs/TARN_LSP.md`](./TARN_LSP.md).

| ID | Task | Impact | Effort | Depends | Files |
|---|---|---:|---:|---|---|
| L1.1 | Bootstrap `tarn-lsp` workspace crate with stdio lifecycle, full text document sync, and `DocumentStore` **[shipped — NAZ-290]** | 8 | M | - | `tarn-lsp/`, `Cargo.toml`, `docs/TARN_LSP.md` |
| L1.2 | Wire `tarn::parser` diagnostics through `textDocument/publishDiagnostics` on open/change/save **[shipped — NAZ-291]** | 9 | M | L1.1 | `tarn-lsp/src/server.rs`, `tarn-lsp/src/diagnostics.rs`, `tarn-lsp/src/debounce.rs`, `tarn/src/validation.rs` |
| L1.3 | Hover provider for env / capture references and assertion keywords | 7 | M | L1.2 | `tarn-lsp/src/server.rs`, `tarn-lsp/src/capabilities.rs` |
| L1.4 | Completion provider for snippets, assertions, env / capture identifiers, HTTP methods | 8 | M | L1.2 | `tarn-lsp/src/server.rs`, `tarn-lsp/src/capabilities.rs` |
| L1.5 | Document symbol provider for test/step tree plus finalised Claude Code docs and release pipeline entry | 7 | S | L1.1-L1.4 | `tarn-lsp/src/server.rs`, `docs/TARN_LSP.md`, release workflow |

### L1.1 scope (shipped)

- New workspace member `tarn-lsp/` with path-dep on `tarn = 0.5.0`.
- `lsp-server = "0.7"` + `lsp-types = "0.95"` — no `tokio`, no `tower-lsp`, no async runtime.
- `cargo build -p tarn-lsp` produces a single binary `target/debug/tarn-lsp`.
- Handles the full LSP lifecycle: `initialize` (responds with `ServerCapabilities { text_document_sync: Full }` and `serverInfo { name, version }`), `initialized`, `shutdown`, `exit`.
- `DocumentStore: HashMap<Url, String>` populated by `didOpen`/`didChange` and cleared by `didClose`. `didSave` is accepted as a no-op.
- `eprintln!` server-info banner on initialize so clients can confirm the handshake in their output pane.
- Integration tests in `tarn-lsp/tests/initialize_test.rs` drive the full handshake over `lsp_server::Connection::memory()`, assert the capability set, and cover the "unknown request → MethodNotFound" fallthrough.
- No language intelligence yet — diagnostics, hover, completion, and symbols are owned by L1.2 through L1.5.
