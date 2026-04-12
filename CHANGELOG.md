# Changelog

## 0.6.2 — Fix crates.io publish blocker, configure release channels (NAZ-314)

- **Fix `cargo publish -p tarn-lsp`**: the compile-time `include_str!`
  for `testfile.json` reached outside the crate directory, which
  `cargo publish` rejects. Bundled a copy of the schema inside
  `tarn-lsp/schemas/v1/` with a sync-verification test to catch drift.
- **Simplified Dockerfile**: removed the separate `COPY schemas`
  directive now that the schema is embedded in the `tarn-lsp` crate.
- **Release secrets configured**: `CRATES_IO_TOKEN`, `VSCE_PAT`,
  `OVSX_PAT`, `HOMEBREW_TAP_TOKEN`/`HOMEBREW_TAP_REPO`, and
  `DOCKERHUB_USERNAME`/`DOCKERHUB_TOKEN` are now set under GitHub
  Actions repository secrets. First fully automated multi-channel
  release.

## 0.6.1 — Dockerfile hotfix: ship tarn-lsp in the image (NAZ-313)

Patch release. The v0.6.0 `Publish Docker image` release job failed
because the Dockerfile was still on the pre-tarn-lsp workspace shape
— it didn't copy `tarn-lsp/` or the workspace-level `schemas/`
directory that `tarn-lsp/src/schema.rs` includes at compile time,
and it didn't pass `-p tarn-lsp` to `cargo build`. The fix:

- Copy `tarn-lsp/` alongside `tarn/`, `tarn-mcp/`, `demo-server/`.
- Copy `schemas/` so the compile-time `include_str!("../../schemas/v1/testfile.json")` resolves.
- `cargo build --release -p tarn -p tarn-mcp -p tarn-lsp`.
- Final stage `COPY` includes `/usr/local/bin/tarn-lsp` so the
  runtime image ships all three binaries.

Also caught in the process: `tarn-lsp/src/schema.rs`'s
`include_str!` pattern reaches out of the crate directory, which
would also block `cargo publish -p tarn-lsp` if a `CRATES_IO_TOKEN`
were set. Documented in NAZ-313 as a latent follow-up to resolve
before the first crates.io publish. Not user-visible for this
release since the publish job silently skips without a token.

No other changes. Paired with **Tarn VS Code extension 0.6.1** —
version-only bump to stay aligned with the NAZ-288 policy that
requires identical tag, `tarn/Cargo.toml`, and
`editors/vscode/package.json` versions at release time.

## 0.6.0 — Phase L: tarn-lsp shipped + Claude Code plugin + VS Code LSP scaffolding

First coordinated release of the **tarn-lsp** language server, plus a
Claude Code plugin that registers it for `.tarn.yaml` files and the
first scaffolding step of the VS Code extension's migration to a
`vscode-languageclient` front-end.

Paired with **Tarn VS Code extension `0.6.0`** — see
[`editors/vscode/CHANGELOG.md`](editors/vscode/CHANGELOG.md) for the
matching extension release notes. Version alignment policy from
NAZ-288 still holds: `tarn` `0.6.x`, `tarn-mcp` `0.6.x`, `tarn-lsp`
`0.6.x`, and the extension `0.6.x` all ship from the same tag.

### New crate: `tarn-lsp`

A standalone LSP 3.17 stdio server that delivers the same
`.tarn.yaml` intelligence the VS Code extension ships — but to
every LSP client, including **Claude Code**, Neovim, Helix, Zed,
IntelliJ, and any other editor with an LSP bridge. Written in
Rust, zero runtime dependencies (sync `lsp-server` + `lsp-types`,
no tokio), and depends on the `tarn` crate directly — no
subprocess spawn, no IPC over stdout, in-process parser and
validator.

**Phase L1 — read surface (MVP)**

* **Diagnostics** (NAZ-291) — publishDiagnostics on open/save/debounced
  change via `tarn::validation::validate_document`, with ranges
  taken from the NAZ-260 location metadata.
* **Hover** (NAZ-292) — context-aware hovers for `{{ env.* }}`
  (with the full env resolution chain provenance),
  `{{ capture.* }}` (capturing step + JSONPath source),
  `{{ $builtin }}` (signature docs), and top-level schema keys.
* **Completion** (NAZ-293) — `.` / `$` trigger characters. Env
  keys sorted by resolution priority, captures-in-scope for the
  current step, builtin snippets with parameter placeholders,
  top-level YAML schema keys per scope (root/test/step). Graceful
  degradation when the buffer is mid-edit.
* **Document symbols** (NAZ-294) — hierarchical outline
  (file → setup/tests/teardown/top-level steps → step children).
  Ranges match diagnostics exactly, so "jump to symbol" and
  "jump to error" agree.

**Phase L2 — navigation**

* **Go-to-definition** (NAZ-297) — jump from a capture use to its
  declaring `capture:` block in the same test; jump from an env
  use to the key declaration in whichever file wins the
  resolution chain. Shell-expansion / CLI `--var` / named-profile
  vars return empty (no declaration site to jump to).
* **References** (NAZ-298) — same-file per-test for captures;
  workspace-wide for env keys via a new `WorkspaceIndex` with
  cached outlines. 5000-file safety cap + log warning; cache
  invalidated on didChange / didSave / didClose.
* **Rename** (NAZ-299) — `prepareRename` + `rename` for captures
  (per-test, single file) and env keys (every source file in the
  resolution chain + every `.tarn.yaml` use site workspace-wide).
  Identifier grammar `^[A-Za-z_][A-Za-z0-9_]*$` with unicode
  explicitly rejected. Collision detection against existing
  captures in the same test or env keys in the same source file.
* **Code lens** (NAZ-300) — `Run test` and `Run step` above every
  test and step. Stable command IDs `tarn.runTest` /
  `tarn.runStep`. Selector format `FILE::TEST::STEP_INDEX`
  (zero-based), matching Tarn's CLI parser. Extracted
  `tarn::selector` as a public module so the LSP and the VS Code
  extension compose selectors from one source of truth.

**Phase L3 — editing polish**

* **Formatting** (NAZ-302) — whole-document formatting via a new
  public `tarn::format::format_document` library surface. The
  `tarn fmt` CLI is now a one-line wrapper over the same
  function. Range formatting is deliberately not supported.
* **Code action framework + extract env var** (NAZ-303) —
  `textDocument/codeAction` dispatcher + the first concrete
  action, which takes a selected string literal inside a
  `.tarn.yaml` step and lifts it into an env key, creating the
  inline `env:` block if missing and counter-suffixing on
  collision. Shared `tarn-lsp::identifier` helper split out of
  `rename.rs` so both paths validate the same way.
* **Capture-this-field + scaffold-assert-from-response** (NAZ-304)
  — two more code actions plugged into the NAZ-303 dispatcher.
  `capture-this-field` inserts a `capture:` stub from the
  JSONPath literal under the cursor in an assert body, with
  leaf-name derivation (`$.data[0].id` → `id`) and counter
  suffixing on collision. `scaffold-assert-from-response` reads
  the last recorded response from a new sidecar convention
  (`<file>.tarn.yaml.last-run/<test-slug>/<step-slug>.response.json`)
  and generates a pre-typed `assert.body` block.
* **Quick fix via shared `tarn::fix_plan`** (NAZ-305) — surfaces
  `tarn-mcp`'s fix plan machinery as an LSP
  `CodeActionKind::QUICKFIX`. The library was lifted out of the
  MCP tool into `tarn::fix_plan::generate_fix_plan` so the LSP
  and MCP surface share one source of truth. Golden contract
  tests for `tarn_fix_plan` in the MCP tool pass byte-for-byte
  unchanged.
* **Nested schema completion** (NAZ-306) — the completion
  provider now offers schema-aware child keys for cursors nested
  below the top-level / step mapping. Schema walker supports
  `properties`, `items`, `additionalProperties`, local `$ref`,
  and `oneOf` / `anyOf` / `allOf` union descent.
  `patternProperties`, `if` / `then` / `else`, and external refs
  are deferred — the bundled Tarn schema does not use them.
* **JSONPath evaluator** (NAZ-307) — new public
  `tarn::jsonpath::evaluate_path` library function plus two LSP
  affordances. Hover over a JSONPath literal in an `assert.body.*`
  key evaluates the path against the step's last recorded
  response and appends the result inline to the hover markdown.
  `workspace/executeCommand` with command `tarn.evaluateJsonpath`
  lets any LSP client evaluate a JSONPath against either an
  inline `response` value or a step reference.

### Claude Code plugin (NAZ-310)

New `editors/claude-code/tarn-lsp-plugin/` ships a ready-to-use
Claude Code plugin that registers `tarn-lsp` for `.tarn.yaml` /
`.yaml` files via the Claude Code plugin system. A local marketplace
at `editors/claude-code/` lets users install via
`/plugin marketplace add` + `/plugin install`. Documented
compound-extension caveat: the plugin claims all `.yaml` files
(Claude Code's LSP plugin schema doesn't support compound
extensions like `.tarn.yaml`), so install at `--scope project`
in Tarn-focused repos only.

### VS Code extension — Phase V scaffolding (NAZ-309)

Adds `vscode-languageclient` 9.0.1 as a dependency and wires it
into `editors/vscode/src/extension.ts` behind a new
`tarn.experimentalLspClient` window-scoped setting. Default `false`.
When enabled, the extension spawns `tarn-lsp` side-by-side with
the existing in-process providers. **No feature has moved to the
LSP path in this release.** Phase V2 will migrate features one
ticket at a time while this flag soaks; Phase V3 will delete the
direct providers and the flag together. Tracked under Epic
NAZ-308.

### Public API growth (`tarn` crate)

Phase L grew several new public modules on the `tarn` crate that
downstream consumers can now depend on:

* `tarn::validation` — `validate_document(path, source)`.
* `tarn::outline` — `outline_document`,
  `find_capture_declarations`, `find_scalar_at_position`,
  `CaptureScope`, `PathSegment`, `ScalarAtPosition`, etc.
* `tarn::env::EnvEntry.declaration_range`,
  `resolve_env_with_sources`, `inline_env_locations_from_source`,
  `scan_top_level_key_locations`.
* `tarn::selector::format_*` — shared selector composer for
  `FILE::TEST::STEP_INDEX`.
* `tarn::format::format_document` — the formatter library surface
  that `tarn fmt` CLI and `tarn-lsp` both consume.
* `tarn::fix_plan::generate_fix_plan` — the quick-fix engine
  shared with `tarn-mcp`.
* `tarn::jsonpath::evaluate_path` — thin `serde_json_path`
  wrapper for the LSP's JSONPath features.

### Release pipeline (NAZ-311)

`.github/workflows/release.yml` now builds and publishes
`tarn-lsp` alongside `tarn` and `tarn-mcp` — tarball / Windows
zip / Homebrew formula all include the new binary. Fabricated
`documentation` URLs in all three Cargo manifests that pointed at
a non-existent `nazarkalytiuk.github.io/tarn/` path were
corrected to the real docs site at
`nazarkalytiuk.github.io/hive/`.

### Bug fixes

* **NAZ-295** — `tarn/tests/integration_test.rs` `ProxyServer::start`
  flaked with `AddrInUse` roughly 10% of the time due to a
  classic TOCTOU race in `free_port`: the helper bound a listener
  to `127.0.0.1:0`, read the port, then dropped the listener and
  returned. A parallel test or the kernel's ephemeral pool could
  snatch the port in the gap. New `bind_ephemeral_listener` helper
  keeps the listener alive across the handoff. 0 failures across
  20 consecutive `cargo test --test integration_test` runs
  post-fix.
* **NAZ-312** — `apiSurface.test.ts` golden-snapshot test failed
  on `windows-latest` because Windows git checkouts rewrite text
  files to CRLF by default, and the test compared an LF-normalized
  `src/api.ts` against a raw (CRLF) golden read. Both sides now
  strip `\r\n` → `\n` before comparison.

### Test count

`cargo test` grew from **664** to **1156** tests across the
workspace (+492, ~74% growth). The extension test suites grew
from **233 unit + 81 integration** to **339 unit + 95 integration**.

## 0.5.0 — Phase 6: Coordinated release (NAZ-288)

First release of Tarn cut under the **coordinated-release** policy
introduced by NAZ-288: a single git tag (`v0.5.0`) now triggers both
the Rust binary pipeline (`.github/workflows/release.yml`) and the VS
Code extension publish pipeline (`.github/workflows/vscode-extension-release.yml`).
Both artifacts ship from the same commit, and both declare the same
version number.

Paired with **Tarn VS Code extension `0.5.0`** — see
[`editors/vscode/CHANGELOG.md`](editors/vscode/CHANGELOG.md) for the
matching extension release notes.

### Version alignment policy

Extension `X.Y.*` tracks Tarn `X.Y.*`: the minor number is always
identical, so a user on Tarn `0.5.x` knows any extension `0.5.x` is
tested against their CLI. Patch numbers may diverge — a hotfix to the
CLI can ship as Tarn `0.5.1` against extension `0.5.0` without a
matching extension bump, and vice versa. A new minor always bumps
both sides in lockstep.

The invariant is enforced by a unit test in the extension
(`editors/vscode/tests/unit/version.test.ts`) that cross-reads
`editors/vscode/package.json` and `tarn/Cargo.toml` on every CI pass
and fails the build if they drift. The extension also spawns
`tarn --version` at activation and warns the user if the installed
CLI is older than its declared `tarn.minVersion` field.

### Added

- **`tarn 0.5.0` is the first CLI release paired with a Marketplace
  extension drop.** All earlier CLI releases (`0.1.0 – 0.4.x`) shipped
  standalone with no Marketplace presence.
- **Phase 6 T-tickets bundled into 0.5.0** (shipped across prior
  commits, now cut as a coordinated release):
  - **T54** per-test cookie jar isolation (NAZ-259)
  - **T55** test-file location metadata on JSON report (NAZ-260)
  - **T57** scoped `tarn list --file` discovery (NAZ-261)
  - **T58** `--redact-header` flag (NAZ-262)
  See the `Unreleased` section below for the full per-ticket detail;
  that content has been promoted in this release.

### Changed

- **`tarn/Cargo.toml` version**: `0.4.4 → 0.5.0` (coordinated minor
  bump to join the extension alignment track).

## 0.1.0

- initial public Tarn release
- YAML-based API tests in `.tarn.yaml`
- structured JSON, JUnit, TAP, HTML, and human output
- setup/teardown, captures, cookies, includes, polling, retries, Lua scripting
- GraphQL support
- MCP server (`tarn-mcp`)
- benchmark mode (`tarn bench`)

## 0.4.0

### Bug Fixes

- **Unresolved template detection** (NAZ-233): steps using `{{ capture.x }}` or `{{ env.x }}` that failed to resolve now fail immediately with a clear error (`failure_category: "unresolved_template"`) instead of sending garbled requests with literal `%7B%7B` in URLs
- **Lua `json` global** (NAZ-231): `json.decode(string)` and `json.encode(value)` are now available in Lua scripts — previously `json` was nil at runtime
- **MCP env var resolution** (NAZ-232): `tarn_run` MCP tool now resolves `tarn.env.yaml` from the project root (matching CLI behavior) instead of only looking in the test file's directory

### Improvements

- **AI-optimized JSON output** (NAZ-235, NAZ-234):
  - `response_status` and `response_summary` fields on all steps (passed and failed) — AI agents can see what a passed step returned without forcing a failure
  - `captures_set` field on steps listing which capture variables were set
  - `captures` map on test groups showing all captured values at end of test
  - Response bodies truncated to ~200 chars in `--json-mode compact`
  - `response_summary` provides brief descriptions like `"200 OK: Array[20]"` or `"403 Forbidden: error message"`
- **JSONPath array search** (NAZ-230): documented and tested that wildcard paths (`$[*].field`) with `contains` and filter expressions (`$[?@.field == 'value']`) work in poll `until` assertions for searching object arrays

### Schema

- Added `unresolved_template` to `failureCategory` enum
- Added optional `response_status`, `response_summary`, `captures_set` to step results
- Added optional `captures` to test results

## Unreleased

- **Per-test cookie jar isolation** (NAZ-259): new `cookies: "per-test"` file-level mode and `--cookie-jar-per-test` CLI flag clear the default cookie jar between named tests within a file so IDE subset runs and flaky integration suites never see session state from a prior test. Setup and teardown still share the file-level jar. Named cookie jars (multi-user scenarios) are untouched. The CLI flag overrides whatever the file declares, except when the file sets `cookies: "off"` — that always wins. Unknown `cookies:` values now fail parsing with a clear error instead of silently falling back to auto.
- **`tarn validate --format json`**: structured validation output for editors and CI. Emits `{"files": [{"file", "valid", "errors": [{"message", "line", "column"}]}]}`. YAML syntax errors include precise `line` and `column` extracted from `serde_yaml`. Parser semantic errors fall back to `message`-only when no location is known (`line`/`column` are optional). Exit codes unchanged: `0` when every file is valid, `2` otherwise. Unknown format values are rejected with exit `2`. The human format (the default) is unchanged.
- **`tarn env --json` schema polish + redaction**: inline vars declared in `tarn.config.yaml` environments are now redacted when they match `redaction.env` (case-insensitive) so `tarn env --json` never prints literal secrets. Renamed the per-environment file field from `env_file` to `source_file` for consistency with the VS Code extension contract. Environments are sorted alphabetically. Exit code stays `0` on success, `2` on configuration error. Human output is unchanged.
- **`--ndjson` flag**: `tarn run --ndjson` streams machine-readable events to stdout, one JSON object per line. Events: `file_started`, `step_finished` (per step, with `phase` set to `setup` / `test` / `teardown`), `test_finished`, `file_finished`, and a final `done` event carrying the aggregated summary. Failing `step_finished` events include `failure_category`, `error_code`, and `assertion_failures`. Composes with `--format json=path` to write the final report to a file while streaming NDJSON on stdout. In parallel mode, each file's event stream is emitted atomically on `file_finished` to avoid interleaving across files. The default human format is silently suppressed on stdout when `--ndjson` is set; other stdout-bound formats raise an error. Primary consumer: the VS Code extension's live Test Explorer updates.
- **`--select` flag**: `tarn run --select FILE[::TEST[::STEP]]` narrows execution to specific files, tests, or steps. Repeatable (multiple selectors union). ANDs with `--tag`. STEP accepts either a name or a 0-based integer index. Step selection runs only that step with no prior steps — captures from earlier steps will be unset, so prefer test-level selectors for chained flows. Enables editor-driven "run test at cursor" and "rerun failed" workflows.
- **Streaming progress output**: `tarn run` now prints results as each test (sequential) or file (parallel) finishes instead of dumping everything at the end. When stdout is `--format human` the stream writes directly to stdout; when stdout is a structured format (`json`, `junit`, `tap`, etc.) the stream goes to stderr so stdout stays parseable. Parallel mode buffers per file and emits each file atomically to avoid interleaving. Add `--no-progress` to restore batch-only output.
- **`--only-failed` flag**: `tarn run --only-failed` hides passing tests and steps from human and JSON output, keeping only the failures. Summary counts still reflect the full run. Works with streaming too.
- transport and runtime parity work: proxy, TLS controls, redirects, HTTP version selection, richer cookies, form support, custom methods
- richer assertion/capture surface: whole-body diffs, more format/hash operators, status/url/header/cookie/body captures, transform-lite pipeline
- machine-oriented diagnostics: `error_code`, remediation hints, compact/verbose JSON, curl export, richer HTML, golden reporter coverage
- product DX: VS Code extension, `tarn fmt`, improved `tarn init`, docs site, Hurl migration guide, conservative Hurl importer
- project workflow: config defaults/redaction/environments, include params and overrides, auth helpers, impacted watch mode, public conformance suite
- benchmark upgrades: thresholds, exports, and timing breakdowns
