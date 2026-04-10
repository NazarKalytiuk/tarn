# Changelog

## 0.5.0 — Phase 3: Environments tree view

Second Phase 3 feature: a first-class Environments treeview under the
Tarn activity bar container, backed by `tarn env --json` (Tarn T56).
Replaces the glob-based discovery the extension used in Phase 1, and
becomes the shared env cache that upcoming authoring features
(completion, hover, go-to-definition) will read from.

### Added

- **`EnvironmentsView`** (NAZ-264). Implements `TreeDataProvider`,
  loads from `backend.envStructured` on activation and on
  `tarn.config.yaml` file changes (via `FileSystemWatcher`). Caches
  the last successful report so repeated queries are free.
- **Tree rendering**: one node per environment showing the active
  check mark, name, source file relative path, and inline var count.
  Tooltip lists every var with its redacted value. A click sets the
  environment as active and toggles off on a second click.
- **Per-node context menu**: `Open source file`, `Copy as --env flag`.
  Inline action buttons on each tree item.
- **`Tarn: Reload Environments`** command (also in the view title
  bar) to force a fresh `tarn env --json` spawn.
- **`Tarn: Set Active (from tree)`**, **`Tarn: Open Environment Source
  File`**, **`Tarn: Copy as --env Flag`** commands — hidden from the
  command palette (they require a context argument).
- **`backend.envStructured(cwd, token)`** on the `TarnBackend`
  interface. Returns `EnvReport | undefined`, parsing the output with
  a new `parseEnvReport` zod schema.
- **`envReportSchema`**, `EnvReport`, `EnvEntry` exported from
  `schemaGuards.ts`.
- **`TarnExtensionApi.testing.reloadEnvironments`**,
  **`listEnvironments`**, **`getActiveEnvironment`** test hooks so
  integration tests can drive the view without TreeDataProvider
  plumbing.

### Changed

- **`Tarn: Select Environment…`** command now reads from the view's
  cache instead of globbing `tarn.env.*.yaml` from the workspace
  root. Picks include source file and var count in the quick-pick
  item description.
- Removed the dead `collectEnvironments` helper in `commands/index.ts`
  that did the old glob-based discovery.
- Integration test fixture workspace (`tests/integration/fixtures/workspace/`)
  now ships a real `tarn.config.yaml` declaring staging + production
  environments plus a redaction rule for `api_token`, so every
  environments-related test runs against a realistic project layout.

### Tests

- Extension integration tests: **12 → 17 passing**. New
  `environments.test.ts` covers: loading two environments in
  alphabetical order, redaction of `api_token` via the project's
  `redaction.env` list, command registration for all four new
  commands, toggling the active environment via
  `tarn.setEnvironmentFromTree`, and clipboard output of
  `tarn.copyEnvironmentAsFlag`.
- Extension unit tests unchanged: 76/76 passing.

### Dependencies

- Tarn T56 (`tarn env --json` schema polish + redaction), shipped
  in `cfffb69`.

## 0.4.0 — Phase 3 kick-off: diagnostics on save

First Phase 3 feature: the extension now surfaces Tarn parse errors as
inline diagnostics on every save, powered by `tarn validate --format json`
(Tarn T52). This turns VS Code into a real linter for `.tarn.yaml` files.

### Added

- **`TarnDiagnosticsProvider`** (NAZ-263). On every save of a `.tarn.yaml`,
  spawns `tarn validate --format json <file>` via the backend, parses the
  structured output with a new `parseValidateReport` zod schema, and
  publishes `vscode.Diagnostic` entries into a dedicated
  `DiagnosticCollection('tarn')`.
  - YAML syntax errors anchor on the exact line/column reported by
    `serde_yaml` (converted from 1-based to 0-based and clamped to the
    document bounds).
  - Parser semantic errors without location fall back to a line-0
    marker so the Problems panel still shows them.
  - Diagnostics clear the moment the file becomes valid again.
  - Closing a document clears its diagnostics.
  - In-flight validations for a document are cancelled and replaced when
    a new save races ahead of the previous one — no stale diagnostics.
- **`tarn.validateOnSave`** setting (default `true`). Flip it to `false`
  to disable the behavior without uninstalling.
- **`backend.validateStructured`** method on the `TarnBackend` interface.
  Returns `ValidateReport | undefined` so callers don't have to parse
  stdout themselves. `TarnProcessRunner` implements it by invoking
  `tarn validate --format json`; returns `undefined` on stale Tarn
  versions so the fallback path is graceful.
- **`TarnExtensionApi.testing.validateDocument(uri)`** — test-only hook
  that awaits a validate run synchronously so integration tests can
  assert on diagnostics deterministically without polling.

### Tests

- Extension integration tests: **7 → 12 passing** against a real `tarn`
  binary. New `diagnostics.test.ts` covers valid file (no diagnostics),
  YAML syntax error (line/column preserved, source set to `tarn`),
  unknown-field semantic error, fix cycle (diagnostic clears when
  content becomes valid), and the `tarn.validateOnSave: false` toggle.
- Extension unit tests: still 76/76 passing (no regressions).

### Dependencies

- Tarn T52 (`tarn validate --format json`), shipped in `cfffb69`.

## 0.3.0 — Phase 2: live streaming and selective execution

Cashes in the Tarn-side `T51` (`--select`) and `T53` (NDJSON reporter) so
the Test Explorer updates live as each step finishes and editor-driven
"run test at cursor" / "rerun failed" workflows use precise selectors.

### Added

- **Live Test Explorer updates via NDJSON**. Runs now spawn `tarn run
  --ndjson --format json=<tmp>` and parse `step_finished` /
  `test_finished` / `file_finished` / `done` events from stdout as they
  arrive. Passing steps turn green the moment they complete instead of
  waiting for the final JSON report. Failures still use the final JSON
  report so the `TestMessage` keeps its rich expected/actual/diff, full
  request, full response, and remediation hints.
- **Selective execution**. When a user clicks Run on a specific test or
  step (from Test Explorer, CodeLens, or `Tarn: Run Current File`), the
  backend derives `--select FILE::TEST[::STEP]` selectors from the
  `TestRunRequest.include` items and forwards them to the CLI. Running
  a single file still uses positional args; running a test adds
  `FILE::TEST`; running a single step adds `FILE::TEST::index`.
- **`Tarn: Run Failed Tests` command**. Tracks the set of failed item IDs
  from the last completed run and reruns only those via selectors.
- **Per-item metadata map** (`discovery.ts`). Every discovered
  `TestItem` carries its structured kind (file / test / step), uri,
  test name, and step index via a `WeakMap`, so the run handler never
  has to parse item IDs.
- Backend interface: `RunOptions` gains `selectors`, `streamNdjson`,
  and `onEvent`. New `NdjsonEvent` union type for consumers that want
  to observe the raw stream.
- Extension API: `TarnExtensionApi` now exposes `testing.backend` so
  integration tests can exercise the backend directly.

### Changed

- `runHandler.ts` rewritten to plan selectors up front, stream events
  via `onEvent`, then apply the final JSON report with rich failure
  `TestMessage`s. Files resolved from NDJSON events using a suffix
  match against `parsedByPath`.
- `TarnProcessRunner` splits the run path: NDJSON mode uses a
  `readline` interface on stdout and writes the final report to a
  tmp file, while the legacy path still supports polling consumers.
  The tmp file is cleaned up on the way out.
- `Tarn: Rerun Last Run` unchanged in behavior but now also remembers
  whether the run was dry.

### Tests

- Extension unit tests: still 76/76 passing (no regressions).
- Extension integration tests: 4 → **7 passing** against a real `tarn`
  binary. New backend suite covers NDJSON streaming end-to-end, single
  test selection, and single step selection. The integration runner
  now writes `tarn.binaryPath` into the fixture workspace pointing at
  `target/debug/tarn` so the test always exercises the source-built
  CLI rather than whatever is on `PATH`.

## 0.2.0 — Phase 1 foundation

Adds extension host integration on top of the existing declarative package.

### Added

- Test Explorer integration via the VS Code Testing API.
  - Hierarchical discovery: workspace → file → test → step.
  - `Run` and `Dry Run` test run profiles.
  - Cancellation via SIGINT with SIGKILL fallback after 2 s, plus a configurable watchdog timeout.
  - Result mapping from `tarn run --format json --json-mode verbose` into `TestRun.passed / failed`.
  - Rich failure `TestMessage` with expected/actual, unified diff, request, response, remediation hints, and failure category/error code.
- CodeLens above each test and step: `Run`, `Dry Run`, `Run step`.
- Document symbol provider: outline view of tests, steps, setup, teardown with scope-aware hierarchy.
- Tarn activity bar container with a **Run History** tree view persisting the last 20 runs (status, env, tags, duration, files).
- Status bar entries: active environment (click to pick) and last run summary (click to open output).
- Commands:
  - `Tarn: Run All Tests`
  - `Tarn: Run Current File`
  - `Tarn: Dry Run Current File`
  - `Tarn: Validate Current File`
  - `Tarn: Rerun Last Run`
  - `Tarn: Select Environment…`
  - `Tarn: Set Tag Filter…`, `Tarn: Clear Tag Filter`
  - `Tarn: Export Current File as curl` (all or failed-only via `--format curl-all` / `--format curl`)
  - `Tarn: Clear Run History`
  - `Tarn: Open Getting Started`
  - `Tarn: Show Output`
  - `Tarn: Install / Update Tarn`
- **Getting Started walkthrough** with five steps: install, open example, run, select env, inspect failure.
- Workspace indexing with on-change reparsing via `FileSystemWatcher`, idempotent initialization.
- YAML AST with range maps for tests, steps, setup, and teardown — foundation for CodeLens, document symbols, result anchoring, and future authoring features.
- Settings namespace `tarn.*` with 13 keys covering binary path, discovery globs, parallelism, JSON mode, timeouts, redaction passthrough, and UI toggles.
- Workspace trust gating: untrusted workspaces keep grammar, snippets, and schema wiring but do not spawn the Tarn binary.
- Shell-free process spawning via Node's built-in `child_process.spawn` with an argv array, plus a log formatter for copyable command lines in the output channel.
- Zod-validated parsing of Tarn JSON reports.

### Tests

- **Unit tests** (vitest, 76 tests across 5 files):
  - `shellEscape` — safe identifier passthrough, space/quote/dollar/backtick escaping.
  - `schemaGuards` — passing report, failing report with full rich detail, enum rejection, missing-field rejection.
  - `YamlAst` — file name, tests, steps, setup, teardown, flat `steps`, malformed input.
  - `YamlAstSweep` — parses every `.tarn.yaml` fixture in `examples/` and verifies non-empty names plus non-negative ranges (55 dynamic tests).
  - `ResultMapper.buildFailureMessages` — rich assertion failure, multi-assertion, generic fallback, and every `failure_category` enum value.
- **Integration tests** (`@vscode/test-electron` + mocha): smoke suite covering activation, test controller registration, discovery of a fixture workspace, document symbols, and command registration. Runs via `npm run test:integration`.

### CI

- GitHub Actions workflow `.github/workflows/vscode-extension.yml` running typecheck, unit tests, and build across `ubuntu-latest`, `macos-latest`, `windows-latest`; Ubuntu job also packages a VSIX artifact.

### Preserved from 0.1.0

- Language id `tarn` for `*.tarn.yaml` / `*.tarn.yml`.
- Grammar at `syntaxes/tarn.tmLanguage.json`.
- Snippets (`tarn-test`, `tarn-step`, `tarn-capture`, `tarn-poll`, `tarn-form`, `tarn-graphql`, `tarn-multipart`, `tarn-lifecycle`, `tarn-include`).
- Schema wiring for test files and report files via `redhat.vscode-yaml`.

### Known gaps (tracked in `docs/VSCODE_EXTENSION.md` and `T51`–`T57`)

- Streaming progress requires Tarn NDJSON reporter (`T53`); Phase 1 uses the final JSON report.
- Run-at-cursor and run-failed-only require selective execution (`T51`).
- Structured validation diagnostics require `tarn validate --format json` (`T52`).
- Runtime result ranges are AST-inferred until Tarn exposes location metadata (`T55`).

## 0.1.0

Initial declarative package: language id, grammar, snippets, schema wiring.
