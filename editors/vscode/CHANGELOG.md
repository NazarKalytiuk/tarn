# Changelog

## 0.15.0 — Phase 4: Bench runner wizard

Fifth Phase 4 feature: `Tarn: Benchmark Step…` wraps the existing
`tarn bench` subcommand in an interactive quick-pick wizard and
renders the resulting JSON summary in a webview panel.

### Added

- **`tarn.benchStep`** command (NAZ-274) wired into the command
  palette and the `resourceLangId == tarn` editor title / context
  menu. Uses `$(dashboard)` codicon for visibility.
- **Bench wizard** (`src/commands/bench.ts`). Drives the user
  through four prompts:
  1. Quick-pick of every benchmarkable step in the active file
     (setup / test / teardown sections, labelled
     `"test_name / step_name"`). The last-selected step is hoisted
     to the top for one-click re-runs.
  2. Input box for request count (default 100, validated as a
     positive integer).
  3. Input box for concurrency (default 10, validated as a
     positive integer).
  4. Input box for optional ramp-up duration, validated against
     `^\d+(ms|s|m)?$` so only Tarn-recognized durations are
     accepted.
  The wizard persists every setting per-file in
  `context.workspaceState` under the `tarn.benchSettings:<path>`
  key, so repeat benchmarks require pressing Enter four times.
- **`TarnBackend.runBench`** and `TarnProcessRunner` implementation.
  Builds `tarn bench <file> -n N -c C --step IDX --format json
  [--ramp-up X] [--env E] [--var k=v …]`, parses stdout through a
  new `benchResultSchema` zod guard, and surfaces parse failures
  via the output channel rather than crashing the wizard.
- **`benchResultSchema`** in `src/util/schemaGuards.ts`. Mirrors
  the shape of `tarn bench --format json`: `step_name`, `method`,
  `url`, `concurrency`, `ramp_up_ms`, `total_requests`,
  `successful`, `failed`, `error_rate`, `total_duration_ms`,
  `throughput_rps`, per-phase `latency` and `timings` stats,
  `status_codes`, `errors`, `gates`, and `passed_gates`.
- **`BenchRunnerPanel`** webview (`src/views/BenchRunnerPanel.ts`).
  Singleton side-column panel with four sections:
  - *Summary grid* — throughput, totals, error rate, wall-clock,
    concurrency, and a gate outcome chip tinted green/red.
  - *Latency bars* — CSS-width bars for min / mean / p50 / p95 /
    p99 / max with a shared horizontal scale so relative spread is
    obvious at a glance. Std-dev surfaces below the bars.
  - *Status codes + errors + gates* — table and bullet list forms.
  - *Raw JSON* — pretty-printed, HTML-escaped `<pre>` block for
    copy-paste.
- **Extension host API**: `testing.showBenchResult` and
  `testing.lastBenchContext` so integration tests can drive the
  panel without spawning `tarn bench`.

### Changed

- **`CommandDeps`** now carries `benchRunnerPanel` and
  `workspaceState` so the bench command can open the panel and
  persist per-file settings through the standard command wiring.

### Deviations from the ticket

- **No chart.js dependency.** `tarn bench --format json` only emits
  aggregate percentiles (not histogram buckets), so there's no
  chartable data beyond p50/p95/p99. Four CSS-width bars render
  that data more honestly and skip a ~200 KB external asset, a
  CSP/`localResourceRoots` wiring round-trip, and a dead
  `media/webview/chart.js` path.
- **No live streaming progress.** `tarn bench` doesn't emit NDJSON
  while running; the panel opens once the final JSON summary is in
  hand. Users see a `withProgress` notification during the run.

### Tests

- **Unit** (`tests/unit/benchRunnerPanel.test.ts`, 13 tests).
  Covers `benchResultSchema` (real tarn output, minimal shape,
  rejection of malformed latency), `percentWidth` (scaling, zero/
  negative max, clamping, tiny-value floor), formatters
  (`formatNumber`, `formatPercent`, `formatDuration`), `renderBar`
  (label/value emission, HTML escaping), and `buildSettingsKey`
  (per-file namespacing).
- **Integration** (`tests/integration/suite/benchRunner.test.ts`,
  3 tests). Registers the command, primes the panel via
  `testing.showBenchResult` with a synthetic `BenchRunContext`,
  and asserts that repeat `show` calls replace the previous
  context.

Total: 191 unit tests, 63 integration tests passing.

## 0.14.0 — Phase 4: HTML report webview

Fourth Phase 4 feature: `Tarn: Open HTML Report` now generates a
Tarn HTML dashboard for the active `.tarn.yaml` and opens it in a
side-by-side webview. Clicking a failed step inside the report
jumps to the exact YAML line in the source file.

### Added

- **`ReportWebview`** (NAZ-273). New
  `src/views/ReportWebview.ts` singleton webview. Tarn's HTML report
  is self-contained (inline CSS/JS/data), so we load it into
  `webview.html` as a string — no `localResourceRoots` wiring
  required. The panel is reused across invocations via `reveal`.
- **`injectReportBridge`** pure helper that splices a small script
  into the generated HTML just before `</body>`. The injected bridge
  walks `.file-card[data-file]` and `.test-group[data-test]` nodes
  (already emitted by the Tarn HTML reporter), derives the
  `stepIndex` of each failed `.step` row, and attaches click
  handlers that post `{type: "jumpTo", file, test, stepIndex}`
  messages back to the extension host. Idempotent via a
  `BRIDGE_MARKER` comment, so re-rendering never double-injects.
- **Click-to-jump handler** in `ReportWebview.handleMessage`. Uses
  the `WorkspaceIndex` to resolve the file path (exact / suffix /
  basename match) and the YAML AST range for
  `(testName, stepIndex)`. Opens the document in `ViewColumn.One`
  so the report stays visible in the side column.
- **`TarnBackend.runHtmlReport`** and `TarnProcessRunner`
  implementation. Spawns `tarn run --format html=<tmpPath>
  --no-progress` with the current env / tag filter / selectors,
  returns `{ htmlPath, exitCode, stderr }`, and confirms the file
  landed on disk before handing the path back.
- **`tarn.openHtmlReport`** command. Resolves the active editor's
  `.tarn.yaml` file, runs the report through
  `vscode.window.withProgress`, reads the HTML, hands it to
  `ReportWebview`, and deletes the tmp file immediately (no need to
  keep it around since the content is already in the webview).
- **Editor title menu + command palette gating**: the command is
  available both in the palette and the `resourceLangId == tarn`
  context menu so users can invoke it directly from a test file.
- **Extension host API**: `testing.showReportHtml` and
  `testing.sendReportMessage` so integration tests can drive the
  panel and exercise the `jumpTo` handler without spawning tarn.

### Tests

- **Unit** (`tests/unit/reportWebview.test.ts`, 6 tests).
  Covers bridge placement before `</body>`, selector presence for
  the walker, idempotence, fallback when `</body>` is missing, and
  the swallow-list for inner expand/collapse controls.
- **Integration** (`tests/integration/suite/reportWebview.test.ts`,
  5 tests). Registers the command, drives `showReportHtml` with a
  minimal Tarn-style HTML fixture, sends synthetic `jumpTo` messages
  and asserts the active editor and cursor line, and verifies
  malformed messages are ignored.

Total: 178 unit tests, 60 integration tests passing.

## 0.13.0 — Phase 4: Fix Plan view

Third Phase 4 feature: a ranked remediation-hint tree for the most
recent failing run. Every failed step from the final JSON report
contributes one entry per `remediation_hint`, grouped by
`failure_category`, with click-to-jump that lands the cursor on the
offending step's line.

### Added

- **`FixPlanView`** (NAZ-271). New
  `src/views/FixPlanView.ts` tree data provider. Walks
  `files[].tests[].steps[]` on every completed run, filters to
  `status: "FAILED"`, and creates one leaf per `remediation_hint`.
  Failures that report a category but no hints surface as a
  placeholder entry so they still appear in the view (rather than
  silently disappearing).
- **Category grouping** with a stable sort order: `assertion_failed`,
  `capture_error`, `unresolved_template`, `parse_error`,
  `connection_error`, `timeout`, then anything unknown.
  `humanizeCategory` renders friendly labels (`"Assertion failed"`,
  `"Connection error"`, etc.) and `iconForCategory` assigns a
  distinct codicon per group.
- **Click-to-jump**. Each entry's `TreeItem.command` fires
  `tarn.jumpToFailure` with the fixture URI and a serialized range,
  opening the file and placing the cursor on the failing step's
  line. The command registration lives in `src/commands/index.ts`
  and is hidden from the command palette (it requires arguments).
- **`tarn.fixPlan`** view declared under the `tarn` activity bar
  container with the `lightbulb` codicon. Ships with a placeholder
  node ("No fix plan available…") until the first failing run
  populates it.
- **Extension host API**: `testing.loadFixPlanFromReport` and
  `testing.fixPlanSnapshot` so integration tests can drive the view
  without spawning a real run.

### Changed

- **`runHandler`** now calls `fixPlanView.loadFromReport` after the
  existing `lastRunCache` / `capturesInspector` hooks. A passing run
  empties the view; a failing run repopulates it.
- **`createTarnTestController`** signature accepts a `FixPlanView`
  parameter so the run handler can forward reports to it.

### Tests

- **Unit** (`tests/unit/fixPlanView.test.ts`, 9 tests).
  Covers `flattenReportToPlan` (empty reports, per-category
  grouping, no-hint placeholder, default category fallback,
  location attachment, category ordering) plus `categoryOrder`,
  `humanizeCategory`, and `deserializeRange`.
- **Integration** (`tests/integration/suite/fixPlan.test.ts`, 6
  tests). Primes the view via `loadFixPlanFromReport` with a
  synthetic report pointing at the existing fixture
  `tests/health.tarn.yaml`, asserts category grouping, verifies
  that the resolved step range lands on the real line from the
  WorkspaceIndex, and exercises `tarn.jumpToFailure` end-to-end by
  asserting the active editor and cursor position after the command
  runs.

Total: 172 unit tests, 55 integration tests passing.

## 0.12.0 — Phase 4: Captures Inspector view

Second Phase 4 feature: a debugger-style "locals" tree for captured
variables. Every completed run now populates a `tarn.captures` view
under the Tarn activity bar, grouped `file > test > key = value` and
scoped to the most recent run.

### Added

- **`CapturesInspector`** (NAZ-270). New
  `src/views/CapturesInspector.ts` tree data provider. Walks the
  final JSON report's `files[].tests[].captures` map after every
  run and renders one node per captured value. Scalar values (string,
  number, boolean, null) render as leaves; objects and arrays expand
  into child nodes so users can drill into nested captures without
  leaving the sidebar.
- **Redaction awareness**. Reads `redaction.captures` from
  `tarn.config.yaml` and masks matching top-level capture keys as
  `***`. A `vscode.workspace.createFileSystemWatcher` pointed at
  `tarn.config.yaml` keeps the list fresh when users edit the config.
  Redacted nodes drop their children so users cannot drill past the
  mask.
- **"Hide all capture values" toggle**. New
  `tarn.toggleHideCaptures` command, wired into the view title bar
  as an `$(eye-closed)` action. Flips a demo-mode flag that redacts
  every capture regardless of the redaction list — useful for screen
  sharing and recordings.
- **Click-to-copy**. Clicking a capture row fires
  `tarn.copyCaptureValue`, which writes the redaction-aware rendered
  value (raw strings, JSON for arrays/objects, `***` when redacted)
  to the clipboard and shows a brief status bar confirmation. The
  command never leaks a real value from a redacted node.
- **`tarn.captures` view** declared under the `tarn` activity bar
  container with the `list-tree` codicon. Ships with a placeholder
  node ("No captures from the last run…") until the first run
  completes.
- **Extension host API**: `testing.loadCapturesFromReport`,
  `testing.capturesTotalCount`, `testing.isCaptureKeyRedacted`,
  `testing.isHidingAllCaptures`, `testing.toggleHideCaptures`. Used
  by the integration suite to drive the view without spawning a
  real run.

### Changed

- **`schemaGuards.testResultSchema`** now includes an optional
  `captures: Record<string, unknown>` field, matching what Tarn
  already emits in `tests[].captures`. Previous versions silently
  stripped this field via zod's object mode.
- **`runHandler`** now calls `capturesInspector.loadFromReport`
  alongside `lastRunCache.loadFromReport` after every successful
  run, replacing the previous view state with the latest captures.
- **`createTarnTestController`** signature accepts a
  `CapturesInspector` parameter so the run handler can forward
  reports to it without additional plumbing.

### Tests

- **Unit** (`tests/unit/capturesInspector.test.ts`, 12 tests).
  Covers `extractRedactionCaptures` (missing config, malformed
  config, string filtering), `isExpandable` (arrays, objects,
  scalars, null/undefined), and `renderRawValue` (strings with
  truncation, numbers, booleans, null, arrays, objects).
- **Integration**
  (`tests/integration/suite/captures.test.ts`, 7 tests). Primes the
  view via `loadCapturesFromReport` with a synthetic report that
  includes the `auth_token` key listed in the fixture workspace's
  `tarn.config.yaml` redaction list. Asserts total count, per-key
  redaction, toggle behavior, command wiring, and empty-report
  handling.
- The fixture `tarn.config.yaml` gained `redaction.captures:
  [auth_token]` to exercise the redaction code path end-to-end.

Total: 163 unit tests, 49 integration tests passing.

## 0.11.0 — Phase 4: Request/Response Inspector webview

First Phase 4 feature: rich step-detail inspector that beats the
markdown-in-a-popover `TestMessage` the extension shipped in Phase 1.
When a step fails, users can now open a dedicated webview panel with
Request / Response / Assertions tabs, pretty-printed headers, JSON
body pretty-printing, a 10 KB truncation guard with an "Open full
in new editor" action, and a per-assertion pass/fail breakdown with
diff rendering.

### Added

- **`RequestResponsePanel`** (NAZ-272). Singleton webview panel that
  opens beside the editor, reuses the existing panel on repeat
  invocations, and preserves context when hidden. No framework —
  the HTML is rendered from a plain template string with CSP-locked
  scripts and VS Code theme CSS variables.
- **Tabbed UI**:
  - *Request* — method + URL, headers table, body with JSON
    pretty-printing.
  - *Response* — status, headers table, body. When the step passed,
    Tarn only records the status code (bodies are failure-only), so
    the panel shows a helpful "Response bodies are only included
    for failed steps" empty state.
  - *Assertions* — one row per assertion with expected, actual,
    message, and colored diff (if present). Prefers the full
    `details` array when available, falls back to `failures` only.
- **Body truncation** at 10 KB with a `data-open-full` button that
  posts a message to the extension host, which opens the full body
  in a new editor with auto-detected language (JSON / XML / plain).
- **`LastRunCache`** (`src/testing/LastRunCache.ts`). Tiny
  in-memory map keyed by `file::test::index` that stores every
  step result from the most recent run. Populated by `runHandler`
  after `applyReport` so the panel can look up any step on demand
  without re-running tests.
- **`tarn.showStepDetails`** command (hidden from the command
  palette because it requires an argument). Accepts either a
  `StepKey` `{ file, test, stepIndex }` or a wrapper
  `{ encodedKey }` string, looks the step up in the cache, and
  opens the panel.
- **Extension API test hooks**:
  `testing.lastRunCacheSize()`, `testing.loadLastRunFromReport()`,
  and `testing.showStepDetails(key)`. The second lets integration
  tests prime the cache from a synthetic report without running
  real tests; the third drives the panel deterministically and
  returns a boolean so tests can assert hit/miss.

### Scope note

v1 ships the panel plus the command. Auto-triggering on Test
Explorer selection, CodeLens buttons on failed steps, and a
`Copy as curl` action inside the panel are intentional follow-ups
(NAZ-272 comments in Linear) — the infrastructure is now in place
and those integrations are small additions later.

### Tests

- **19 new unit tests** across two files:
  - `lastRunCache.test.ts` (7 tests) — empty state, indexing
    setup/test/teardown, setup lookup via `test: "setup"`, test
    lookup by name + index, replace-on-reload semantics, `clear()`,
    and `encode`/`decode` round-trip plus malformed handling.
  - `requestResponsePanel.test.ts` (12 tests) — `stringifyBody`
    for strings/objects/arrays/circular refs, `detectLanguage`
    for JSON / XML / plaintext, and `truncateBody` above/below/at
    the 10 KB threshold.
- **5 new integration tests** in `stepDetails.test.ts` against a
  real VS Code instance: command registration, cache sizing after
  loading a synthetic report, panel-open success for a known key,
  panel-open miss for an unknown key, and a full
  `vscode.commands.executeCommand` round-trip.
- Extension unit tests: **131 → 151 passing**.
- Extension integration tests: **37 → 42 passing**.

## 0.10.0 — Phase 3 complete: YAML grammar injection for interpolation

Seventh and final Phase 3 feature: the TextMate grammar shipped in
`syntaxes/tarn.tmLanguage.json` now assigns distinct scope names to
every part of a `{{ ... }}` interpolation so themes and the language
providers can discriminate between env keys, capture names, and
built-in functions.

### Changed

- **`meta.template.tarn`** is the wrapper scope on the entire
  `{{ ... }}` range (previously `meta.interpolation.tarn`).
- **`keyword.control.template.begin.tarn`** and
  **`keyword.control.template.end.tarn`** scopes are added to the
  `{{` / `}}` delimiters alongside the standard
  `punctuation.definition.template.begin/end.tarn` names so both
  theme families highlight the braces.
- **`variable.other.env.tarn`** now scopes `env.KEY` references
  (was grouped with captures under `variable.other.readwrite.tarn`).
  The rule splits into two capture groups — the `env` namespace
  token and the key identifier — so themes that distinguish
  namespace from identifier render correctly.
- **`variable.other.capture.tarn`** does the same for
  `capture.NAME` references.
- **`support.function.builtin.tarn`** (unchanged) matches every
  Tarn runtime built-in: `$uuid`, `$timestamp`, `$now_iso`,
  `$random_hex`, `$random_int`.
- `support.function.transform.tarn` and
  `keyword.operator.pipeline.tarn` are preserved from the
  previous grammar for the transform-lite pipeline.

### Tests

- **10 new unit tests** in `grammar.test.ts` load the grammar JSON,
  walk every pattern + capture entry, flatten space-separated scope
  names, and assert the expected scopes are declared. The tests
  also exercise the actual regexes against realistic tokens
  (`env.base_url`, `capture.auth_token`, every `$builtin` name) so
  regressions on the underlying matching rules surface immediately.
- Extension unit tests: **121 → 131 passing**.
- Integration tests unchanged: 37/37 passing.

### Manual verification

The grammar's theme rendering is intentionally not covered by
automated tests (VS Code does not expose tokenization via a public
API). Visual verification on the Default Dark+ and Default Light+
themes stays the responsibility of the release smoke-test.

### Phase 3 status

Phase 3 is now complete (NAZ-263 through NAZ-269). The extension
has a full authoring surface: diagnostics on save, environments
tree view, completion, hover, go-to-definition / references /
rename, `tarn fmt` format provider, and distinct grammar scopes.
Next up: Phase 4 rich-UX features (NAZ-270 through NAZ-278), with
NAZ-272 Request/Response Inspector as the highest priority.

## 0.9.0 — Phase 3: `tarn fmt` format provider

Sixth Phase 3 feature: VS Code's Format Document action now
routes `.tarn.yaml` files through `tarn fmt`, so Shift+Alt+F
(and `editor.formatOnSave: true`) normalizes Tarn YAML to the
canonical form without leaving the editor.

### Added

- **`TarnFormatProvider`** (NAZ-268). Implements
  `vscode.DocumentFormattingEditProvider` on the `tarn` language
  and returns a single full-document `TextEdit` so undo is one
  step. Parse errors in the source file are surfaced via the
  output channel plus a one-shot warning notification, and the
  buffer is left untouched — formatting an invalid file never
  corrupts it.
- **`backend.formatDocument(content, cwd, token)`**. The Tarn CLI
  has no `--stdout` or stdin mode, so the backend writes the
  document content to a tmp `.tarn.yaml` file, runs
  `tarn fmt <tmp>`, reads the formatted result back, and cleans
  up the tmp file. Errors and cancellation are propagated so the
  provider can decide whether to emit any edits at all.
- **`TarnExtensionApi.testing.formatDocument(uri)`** test hook.
  Calls the provider directly so integration tests do not have
  to fight VS Code's formatter selection when another extension
  (e.g., `redhat.vscode-yaml`) also registers a document
  formatter for the same file.

### Behavior

- Already-canonical files produce an empty edit list so
  `formatOnSave` is a no-op and the document is never dirty-marked
  unnecessarily.
- Files with YAML parse errors log to the Tarn output channel and
  return no edits.
- Cancellation honors the provided `CancellationToken` and leaves
  the document untouched.

### Tests

- **3 new integration tests** in `format.test.ts` against a real
  `tarn` binary: a messy fixture with non-canonical indentation
  and quoting produces exactly one full-document edit with
  canonical output; an already-canonical fixture produces zero
  edits; a fixture with an unterminated quoted string produces
  zero edits and leaves the on-disk content untouched.
- Extension integration tests: **34 → 37 passing**.
- Unit tests unchanged: 121/121 passing.

## 0.8.0 — Phase 3: Symbol navigation (definition, references, rename)

Fifth Phase 3 feature: Tarn interpolation tokens now participate in
VS Code's standard symbol navigation surface. Right-click a capture
or an env key inside `{{ ... }}` and the usual "Go to Definition",
"Find All References", and "Rename Symbol" actions Just Work.

### Added

- **`TarnDefinitionProvider`** (NAZ-267). Jumps from a
  `{{ capture.x }}` reference to the step that declares
  `capture: { x: ... }`, respecting the capture's file scope.
  Jumps from `{{ env.key }}` to the file(s) listed by the
  `EnvironmentsView` cache, with an in-file line lookup that
  grep-matches `^<key>:` in each source file so the cursor lands
  on the actual declaration line instead of line 0.
- **`TarnReferencesProvider`**. Returns every
  `{{ capture.NAME }}` usage in the current file, plus the
  declaration if `includeDeclaration` is true. Works equally well
  when the cursor is on a reference or on the declaration key
  inside a `capture:` block. Env references are explicitly
  out-of-scope for v1 (they span multiple files); the provider
  returns an empty list rather than a misleading partial answer.
- **`TarnRenameProvider`**. Renames a capture across its
  declaration and every in-file reference as a single
  WorkspaceEdit. `prepareRename` narrows the edit range to just
  the identifier (so renaming from a reference doesn't accidentally
  rewrite the `{{ }}` punctuation). Rejects renames when:
  - the cursor is on an env token (edit the source file directly);
  - the file has YAML parse errors (fix first);
  - the capture is not declared in the current file (probably came
    from an `include:` directive — edit the included file);
  - the new name isn't a valid identifier
    (`^[A-Za-z_][A-Za-z0-9_]*$`).
- **`buildCaptureIndex(source)`** helper in
  `src/language/completion/captures.ts`. Walks the YAML CST once
  and returns a searchable index of every capture declaration with
  its phase, test name, step info, and byte-offset key range.
  Shared by definition / references / rename so they stay
  consistent and cheap.
- **`findCaptureReferences(source, nameFilter?)`** helper.
  Regex-scans the document for `{{ capture.NAME }}` tokens and
  returns the identifier-only byte ranges (not the `{{`/`}}`
  punctuation) so rename edits are precise.
- **`cursorSymbol(document, position)`** helper in
  `src/language/SymbolProviders.ts`. Wraps `findHoverToken` and
  falls back to the capture index to detect clicks on declaration
  keys inside `capture:` blocks. Returns a tagged union
  (`env` / `capture-ref` / `capture-decl`) the three providers
  consume.

### Tests

- **10 new unit tests** in `captureSymbols.test.ts` exercise
  `buildCaptureIndex` and `findCaptureReferences` against a
  multi-test fixture: capture collection from setup / tests /
  teardown, per-declaration phase and test-name recording,
  `findDeclarationAt` offset hit-testing, `findByName` lookup,
  reference scanning over mixed env/capture/builtin interpolations,
  name filtering, whitespace tolerance inside `{{ ... }}`, and
  empty-source handling.
- **6 new integration tests** in `symbols.test.ts` drive the
  providers through VS Code's public commands
  (`vscode.executeDefinitionProvider`,
  `vscode.executeReferenceProvider`,
  `vscode.executeDocumentRenameProvider`, `vscode.prepareRename`)
  against a real extension host: go-to-definition on a capture
  reference lands on the declaring key line, go-to-definition on
  `{{ env.base_url }}` produces locations in at least one env
  source file, find-all-references on a capture returns all 3
  expected locations (2 refs + 1 decl), renaming a capture
  replaces every occurrence via a single WorkspaceEdit, invalid
  new names are rejected, and `prepareRename` on a reference
  returns only the identifier range (not the whole `{{ }}` token).
- Extension unit tests: **111 → 121 passing**.
- Extension integration tests: **28 → 34 passing**.

## 0.7.0 — Phase 3: Hover provider for interpolation

Fourth Phase 3 feature: hovering over any `{{ env.x }}`,
`{{ capture.y }}`, or `{{ $builtin }}` token now shows a
context-aware markdown tooltip with resolved values, source files,
capturing step, or built-in signature + docs.

### Added

- **`TarnHoverProvider`** (NAZ-266). Registered on the `tarn`
  language and reuses the same env cache, capture walker, and
  builtin list as `TarnCompletionProvider`.
- **Hover on `{{ env.KEY }}`** lists every configured environment
  that declares the key with its source file and (already-redacted)
  value. Missing keys get a "not declared in any configured
  environment — will resolve at runtime" hint so the user isn't
  misled into thinking the key is broken.
- **Hover on `{{ capture.NAME }}`** shows the step that captured
  it, whether the step lives in setup or a named test, the step
  index, and the phase. Missing names get a "not in scope" hint
  with a reminder about the capture scoping rules.
- **Hover on `{{ $builtin }}`** shows the signature and doc.
  Parameterized builtins like `$random_hex(n)` are matched by
  stripping the arg list before the lookup.
- **Hover on the bare `{{ }}`** (empty interpolation or just the
  keyword `env` / `capture` / `$`) shows a quick help card
  describing every available form.
- **`findHoverToken(line, column)`** — a new token finder in
  `src/language/completion/hoverToken.ts` that does a single-pass
  scan for the enclosing `{{ ... }}` pair, classifies its contents,
  and returns the token range for VS Code to highlight. Handles
  multi-interpolation lines, cursor-on-boundary edge cases, and
  unclosed expressions.

### Out of scope (follow-up)

- **Dry-run URL preview** on hover over a `url:` field (spawning
  `tarn run --dry-run --select ...` with a per-hover cache). Still
  valuable but too much machinery for this ticket. Tracked as a
  follow-up to NAZ-266 in `docs/VSCODE_EXTENSION.md`.

### Tests

- **13 new unit tests** in `hoverToken.test.ts` exercising the
  finder against every expected shape: cursor outside/inside/on
  boundary of a `{{ ... }}`, env with cursor on key vs on `env`
  itself, capture, `$uuid` bare and `$random_hex(n)` with args,
  empty interpolation, multi-interpolation lines, unknown
  expressions, and unclosed `{{`.
- **6 new integration tests** in `hover.test.ts` against a real
  VS Code instance: env hover lists both staging and production
  environments and their values, capture hover shows the
  capturing step name, missing capture shows a "not in scope"
  warning, `$uuid` hover shows the doc text, `$random_hex(8)`
  hover shows the `$random_hex(n)` signature (not the literal
  argument), and hovering outside any interpolation never fires
  our provider (verified by checking for stable substrings our
  provider emits).
- Extension unit tests: **98 → 111 passing**.
- Extension integration tests: **22 → 28 passing**.

### Dependencies

- NAZ-264 Environments tree view (env cache) — shipped in `990182e`.
- NAZ-265 Completion provider (shares `collectVisibleCaptures` and
  `BUILTIN_FUNCTIONS`) — shipped in `946bec9`.

## 0.6.0 — Phase 3: Interpolation completion provider

Third Phase 3 feature: VS Code now offers IntelliSense for Tarn's
`{{ ... }}` template expressions. Type `{{ env.`, `{{ capture.`, or
`{{ $` inside any string in a `.tarn.yaml` and the completion widget
fills in the matching names from the merged env resolution chain,
visible captures, or the built-in function list.

### Added

- **`TarnCompletionProvider`** (NAZ-265). Registered on the `tarn`
  language with trigger characters `{`, `.`, `$`, and space. The
  provider itself is string-based (grammar scope detection is a
  separate ticket, NAZ-269); on each invocation it inspects the line
  prefix to decide whether the cursor sits inside an open `{{ ... }}`
  expression and what kind.
- **Env key completions** come from the `EnvironmentsView` cache
  (NAZ-264), so the first Phase 3 feature is now delivering value
  beyond its tree. Every env key is labeled with the list of
  environments that declare it.
- **Capture completions** use a new `collectVisibleCaptures` helper
  that walks the YAML CST and returns only captures visible at the
  cursor: setup captures are always in scope, test captures only when
  the cursor is in that same test and only from strictly earlier
  steps, teardown sees everything. Captures from other tests are
  never offered.
- **Built-in function completions** for the full Tarn interpolation
  runtime list: `$uuid`, `$timestamp`, `$now_iso`, `$random_hex(n)`,
  `$random_int(min, max)`. Parameterized builtins insert a snippet
  with argument placeholders.
- **Top-level completions** offer `env`, `capture`, and `$uuid` when
  the user just typed `{{ ` with nothing after it. Picking `env` or
  `capture` re-triggers the suggest widget automatically.

### Tests

- **22 new unit tests** across two new files:
  - `interpolationContext.test.ts` (13 tests) exercises
    `detectInterpolationContext` against every expected shape: empty
    interpolation, env context with and without a prefix, capture
    context, builtin context, closed interpolations, nested braces,
    and unknown prefixes.
  - `visibleCaptures.test.ts` (9 tests) covers the capture visibility
    rules end-to-end: setup captures from the outside, scope
    narrowing inside earlier vs later test steps, sibling test
    isolation, teardown seeing everything, and graceful handling of
    malformed YAML.
- **5 new integration tests** in `completion.test.ts` against a real
  VS Code instance: env completions come from `tarn.config.yaml`,
  capture completions respect step ordering, same-step and sibling-
  test captures are excluded, builtin completions contain every name
  from the Tarn runtime, and typing outside any interpolation never
  fires our provider (verified by filtering results by our
  provider's stable `detail` prefix to ignore the built-in word
  completer's noise).
- Extension unit tests: **76 → 98 passing**.
- Extension integration tests: **17 → 22 passing**.

### Dependencies

- Tarn T56 (`tarn env --json`), shipped in `cfffb69` — provides the
  env data the completion reads from.
- NAZ-264 Environments tree view, shipped in `990182e` — owns the
  env cache that NAZ-265 reuses.

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
