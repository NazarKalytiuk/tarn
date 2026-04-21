# Changelog

## Unreleased

### Runner + CLI (tarn)

- **Immutable per-run artifact directories (NAZ-400).** Every `tarn run`
  now writes its JSON report and `state.json` into
  `.tarn/runs/<run_id>/`, where `<run_id>` is a stable identifier of
  the form `YYYYmmdd-HHMMSS-xxxxxx` (6 hex chars of random suffix to
  break same-second ties). A second run no longer destroys the
  previous run's debugging context — the archive is append-only and
  compares cleanly between runs. The existing `.tarn/last-run.json`
  and `.tarn/state.json` paths are preserved as pointers to the most
  recent run so tooling that already reads them keeps working. The
  CLI announces `run id:` and `run artifacts:` on stderr at the end
  of every run; both files embed the same `run_id` so automation can
  correlate archives without string-matching on paths.
  `--no-last-run-json` now suppresses both the pointer and the
  per-run directory (the transient-run behavior it already
  advertised).
  - **Triage artifacts `summary.json` and `failures.json` (NAZ-401).**
    Every run now also writes a condensed `summary.json` (run id,
    timings, exit code, total/failed counts, list of failed files) and
    a `failures.json` (one entry per failing step with file/test/step
    coordinates, failure category, message, request method/url,
    response status, a ~500-char redacted body excerpt, and — when
    trivially derivable — a `root_cause` pointer for cascade skips).
    Both land next to `report.json` under `.tarn/runs/<run_id>/` and
    are mirrored to `.tarn/summary.json` / `.tarn/failures.json` as
    discoverability pointers. A failed run can now be triaged from
    `failures.json` alone, without parsing the full report; the full
    report stays available for deep inspection. Both artifacts are
    emitted unconditionally (passing runs still produce
    `failures: []`) so tooling can key off stable filenames, and both
    are suppressed under `--no-last-run-json`.
  - **`tarn rerun --failed` reruns only the failing subset (NAZ-403).**
    New subcommand that reads `failures.json` from a prior run and
    executes just the `(file, test)` pairs that failed. `--failed`
    is required (so `tarn rerun` never silently becomes a full run);
    `--run <run_id>` selects a specific historical archive under
    `.tarn/runs/<id>/failures.json`, otherwise the workspace-level
    latest-run pointer at `.tarn/failures.json` is used. Granularity
    is test-level (a failing `FILE::TEST` becomes one selector);
    setup/teardown failures escalate to a whole-file rerun because
    their fixtures feed every test in the file. User-supplied
    `--select` / `--test-filter` compose with the rerun selection
    (union), so additional narrowing still works. The command prints
    `rerun: selected N tests from run <id>:` followed by a bullet
    list of `FILE::TEST` labels (truncated at 20 with `…and M more`)
    on stderr before dispatching to the runner. The rerun produces a
    fresh run artifact set — its own `run_id`, `report.json`,
    `summary.json`, `failures.json`, `state.json`, and refreshed
    `last-run.json` pointer — and stamps the source provenance onto
    both `report.json` and `summary.json` under a new `rerun_source`
    field (`{run_id, source_path, selected_count}`) so automation can
    chain reruns. A source run with no failures exits 0 with
    `rerun: no failing tests to rerun` and does not create an empty
    archive. Source lookup is anchored at the current workspace root;
    `cd` into the project before invoking the command if you have
    moved away.
  - **Built-in concise report subcommand `tarn report` (NAZ-404).**
    New CLI subcommand that re-renders a prior run's persisted
    `summary.json` + `failures.json` without re-running the tests,
    replacing external helper scripts (such as `parse-results.py`) in
    the common local-debugging loop. `tarn report` reads the
    workspace-level pointers at `.tarn/summary.json` /
    `.tarn/failures.json` by default; `--run <run_id>` (with the same
    `last` / `latest` / `@latest` / `prev` / bare-id aliases supported
    by `tarn inspect` and `tarn diff`) opens any historical archive
    under `.tarn/runs/<run_id>/`. `--format concise` (the default)
    prints a one-line verdict header followed — on failing runs — by
    up to 10 root-cause groups (reusing the NAZ-402 fingerprinting so
    cascade fallout collapses into `└─ cascades: N skipped` rather
    than inflating the group count), with a trailing
    `…and N more groups (run \`tarn failures\` for full list)` when
    the run has more. `--format json` emits a stable envelope
    (`schema_version: 1`) with totals, failed counts, and a
    `groups_truncated` / `groups_total` pair so agents can paginate or
    fall back to `tarn failures`. Color output is honored on a TTY and
    suppressed automatically on a pipe (plus `--no-color` for explicit
    override), mirroring the llm renderer. Exit codes: 0 when the
    loaded `failures.json` is empty, 1 when it has any failure, 2 on
    missing / malformed artifacts or an unknown `--run <id>`.
  - **`tarn inspect` and `tarn diff` for run drill-down and
    comparison (NAZ-405).** `tarn inspect <run_id> [target]` loads
    `.tarn/runs/<run_id>/report.json` and renders a run / file /
    test / step view depending on the `FILE[::TEST[::STEP]]` address
    passed — so opening a single failing step no longer needs `jq`
    against the full report. Run id aliases `last` / `latest` /
    `@latest` / `prev` are supported (`prev` resolves to the archive
    immediately before the latest). The run-level view accepts
    `--filter-category <cat>` to narrow the listed failed files to
    those carrying a failure in the given category; pass
    `--fail-on-failure` to exit 1 when the inspected entity is
    failing. `tarn diff <run_a> <run_b>` loads both runs'
    `summary.json` and `failures.json`, computes the totals delta,
    and classifies failure fingerprints (reusing the grouping
    machinery from `tarn failures`) into `new` (only in B), `fixed`
    (only in A), and `persistent` (both). `--file`, `--test`, and
    `--filter-category` compose additively against the root-cause
    coordinates so diffs can be scoped to a single suite. Both
    subcommands support `--format human|json` with a stable JSON
    envelope (`schema_version` 1). Exit codes: 0 on success, 2 on
    unknown run id / missing artifact / parse error.
  - **Failures-first debugging workflow in docs + skill (NAZ-408,
    docs-only).** The `tarn-api-testing` skill, README, AI workflow
    demo, troubleshooting guide, docs index, and MCP workflow doc now
    lead with the canonical failures-first loop
    (`validate → run → failures → inspect last FILE::TEST::STEP →
    patch → rerun --failed → diff prev last`) and deprecate
    full-`report.json` parsing to a last-resort path. Agents are
    explicitly instructed never to slurp `report.json` when
    `failures.json` suffices, never to open cascade skips
    (`skipped_due_to_failed_capture`) individually, and to rule out
    response-shape drift before blaming business logic. A
    reusable "reopen-request" incident walkthrough (mutation response
    changed from `{"uuid": "..."}` to `{"request": {"uuid": "..."}}`
    → capture path needs to move from `$.uuid` to `$.request.uuid` +
    envelope type assertion) lands in `docs/TROUBLESHOOTING.md` and
    the skill. Mutation-response vs read-response conventions are
    documented so tests default to asserting the envelope on `POST`/
    `PUT`/`PATCH` responses.

## 0.9.0 — UUID version assertions & generators, basic faker with seeded RNG

### Runner + CLI (tarn)

- **UUID v4/v7 awareness (NAZ-366).** New body assertions `is_uuid_v4`
  and `is_uuid_v7` sit alongside the existing `is_uuid` (which still
  matches any version). Matching built-ins `$uuid_v4` and `$uuid_v7`
  complement the existing `$uuid` (now an alias for `$uuid_v4`), so
  tests can both generate and verify version-specific UUIDs.
- **Basic faker surface (NAZ-398).** New EN-locale interpolation
  built-ins for realistic payloads: `$email`, `$first_name`,
  `$last_name`, `$name`, `$username`, `$phone`, `$word`, `$words(n)`,
  `$sentence`, `$slug`, `$alpha(n)`, `$alnum(n)`,
  `$choice(a, b, …)`, `$bool`, `$ipv4`, `$ipv6`.
- **Reproducible runs via seeded RNG (NAZ-398).** Set
  `TARN_FAKER_SEED=<u64>` or `faker.seed: <u64>` in `tarn.config.yaml`
  to pin every RNG-backed built-in — including `$uuid`, `$uuid_v4`,
  `$uuid_v7`, `$random_hex`, `$random_int`, and the new faker
  generators — so the same test file produces byte-identical payloads
  across processes. Wall-clock values (`$timestamp`, `$now_iso`, and
  the timestamp prefix of `$uuid_v7`) stay real-time; only the RNG
  path is frozen. The environment variable wins over the config field
  when both are set.

### Editor integrations

- **tarn-lsp + VS Code extension.** Hover and completion surface every
  new built-in and every new assertion, including snippet placeholders
  for the parameterized forms (`$words(n)`, `$alpha(n)`, `$alnum(n)`,
  `$choice(...)`). JSON schema (`schemas/v1/testfile.json`) now
  documents `is_uuid_v4` / `is_uuid_v7`.

## 0.8.0 — Optional captures, conditional steps, LLM/compact output, fixture store, debug surface, parallel safety, and VS Code MCP backend

### Runner + CLI (tarn)

- **Optional / conditional captures (NAZ-242).** Captures now support
  `optional: true` (missing JSONPath → variable unset, not a failure),
  `default:` (numeric, string, or `null` fallback), and `when: { status: ... }`
  (only attempt capture when the response status matches). Step-level
  `if:` / `unless:` expressions skip the whole step when the template
  interpolates to falsy / truthy — truthy rules match empty / `"false"`
  / `"0"` / `"null"` and unresolved `{{ ... }}` placeholders as falsy.
  Optional-unset references produce a distinct "template variable
  'X' was declared optional and not set" error.
- **Step-level `description:` field (NAZ-243).** Optional
  human-readable description on any step (matches file/test-level
  semantics, supports multi-line `|` / `>` YAML). Included in the
  JSON report and rendered dimmed under the step name in human output.
- **LLM and compact output formats (NAZ-349, NAZ-240).** `--format llm`
  emits a grep-friendly verdict line followed by only failing blocks
  with request/response/assertion details — no boxed headers, stable
  ordering. `--format compact` is the middle ground: one-line header,
  per-file badges, inline failure expansion, grouped failure summary.
  When stdout is not a TTY and no `--format` is set, tarn now
  auto-selects `llm` so piping no longer dumps boxed human output. New
  `tarn summary <run.json>` subcommand re-renders a saved JSON report
  without re-running tests (accepts `-` for stdin).
- **Verbose response capture on passing steps (NAZ-244).**
  `--verbose-responses` embeds request/response bodies and resolved
  captures in the JSON report for every step, not just failed ones.
  Per-step `debug: true` opts individual steps in without a global
  flag. Bodies truncate to 8 KiB with a `"...<truncated: N bytes>"`
  marker; `--max-body <bytes>` overrides the cap. Replaces the
  `status: 999` workaround that everyone was using to see passing
  response bodies.
- **Parallel safety markers (NAZ-249).** `serial_only: true` on a file
  or a named test pins the whole file onto the serial worker under
  `--parallel` (file-level isolation remains the unit). `group:
  <name>` buckets files sharing a resource onto the same worker so
  Postgres-hitting tests run sequentially even when other buckets run
  in parallel. Running `--parallel` without `parallel_opt_in: true` in
  `tarn.config.yaml` now prints a one-line warning on stderr;
  `--no-parallel-warning` suppresses it for CI that has already opted
  in via flags.
- **Filter flags for the debug surface (NAZ-256).** `--test-filter
  <name>` and `--step-filter <name-or-index>` narrow a run to a single
  test or step without building a `--select` expression. Synthesizes a
  wildcard selector that applies across every discovered file.
- **Route-ordering diagnostic hint (NAZ-250).** When a status
  assertion fails with a 4xx response whose body carries a textual
  signal ("route not found", "invalid uuid", validation-param error,
  etc.), the report now includes a `note:` line pointing at the
  NestJS-style dynamic-route trap. Hint appears inline in human
  output and as a `hints: [...]` array on the failing status
  assertion in JSON output. New `docs/TROUBLESHOOTING.md` covers the
  pattern.
- **Fixture store writer (NAZ-252).** Every step now writes a fixture
  to `.tarn/fixtures/<file-hash>/<test-slug>/<step-index>/` (redacted
  same as the JSON report), retaining a rolling history (5 by default,
  `--fixture-retention N` overrides) plus a `latest-passed.json` copy
  for every successful run. `--no-fixtures` disables writes. Feeds
  the LSP hover/diff/JSONPath evaluator and the debug-surface `diff
  last passing` command. `.tarn/fixtures/` is gitignored by default.
- **`.tarn/state.json` sidecar (NAZ-257).** Human-readable state
  snapshot written atomically after every run — last-run summary,
  failure list, current debug session, env metadata. Schema-versioned
  so LLM tooling can tail it without breaking when fields evolve.
- **`.tarn/last-run.json` augmented (NAZ-256).** The always-on
  artifact now also records `args`, `env_name`, `working_directory`,
  `start_time`, and `end_time` so "rerun last failures" replays with
  the same env and flags even after the user has changed directories.

### Language server (tarn-lsp)

- **Fixture-driven inline JSONPath evaluator (NAZ-254).** The existing
  `tarn.evaluateJsonpath` custom command now returns a typed
  three-variant result: `{ result: "match", value }` on hit, `{ result:
  "no_match", available_top_keys }` on miss, or `{ result: "no_fixture",
  message }` when no recorded response exists for the step. Hover on a
  JSONPath literal inside `capture:` or `assert.body.*` shows the
  matched value inline against the latest fixture.
- **LLM command surface (NAZ-257).** `docs/commands.json` is the
  authoritative manifest for every stable `tarn.*` command the LSP
  exposes. All commands return `{ schema_version: 1, data: ... }`
  envelopes for forward compatibility. New commands:
  `tarn.explainFailure`, `tarn.getFixture`, `tarn.clearFixtures`.
  `tarn.explainFailure` returns a structured blob (expected vs actual,
  preceding step captures, root-cause hint heuristics for 5xx / auth /
  JSONPath-no-match / unresolved-capture) that agents can paste into a
  reasoning loop without further context.
- **Debug surface (NAZ-256).** New commands:
  `tarn.runFile`, `tarn.runTest`, `tarn.runStep`, `tarn.runLastFailures`
  stream `tarn/progress` notifications during execution.
  `tarn.debugTest` starts a step-through session that publishes
  `tarn/captureState` notifications between steps; agents drive it
  with `tarn.debugContinue`, `tarn.debugStepOver`,
  `tarn.debugRerunStep`, `tarn.debugRestart`, `tarn.debugStop`.
  `tarn.getCaptureState` polls the current snapshot.
  `tarn.diffLastPassing` returns a structured status / headers / body
  diff against the most recent passing fixture, or `{ error:
  "no_baseline" }` when none exists. A new `run_test_steps` callback
  API in the core runner drives the debugger without forking a child
  process.

### MCP server (tarn-mcp)

- **`cwd` parameter on every tool (NAZ-248).** `tarn_run`, `tarn_list`,
  `tarn_validate`, and `tarn_fix_plan` now accept an optional absolute
  `cwd` that drives `tarn.config.yaml`, `tarn.env*.yaml`, include
  paths, and multipart file paths. Defaults to the workspace root the
  MCP client announced during `initialize`, falling back to the server
  process `cwd`. An explicit `cwd` without a `tarn.config.yaml` now
  fails fast with the resolved path in the error message instead of
  silently defaulting — removes the `{{ env.base_url }}` resolution
  bug that forced the team to "always use the CLI" when running via
  Claude Code.

### VS Code extension

- **MCP backend (NAZ-279).** New `tarn.backend: "cli" | "mcp"`
  setting. When set to `mcp`, the extension spawns `tarn-mcp` once per
  workspace and dispatches `tarn_run` / `tarn_list` / `tarn_validate`
  / `tarn_fix_plan` as JSON-RPC calls over stdio, reducing per-run
  latency and sharing process state across commands. NDJSON streaming
  gracefully degrades to final-report polling (documented). When the
  MCP binary is missing, falls back to the CLI with a one-shot
  notification. New `tarn.mcpPath` override for non-standard
  installations. Test Explorer and every other existing feature runs
  unchanged against either backend.
- **Debug panel (NAZ-256).** New `Tarn: Debug Test` and `Tarn: Diff
  Last Passing` commands wired from code lens. A minimal HTML webview
  shows the current step, captures, last response, and assertion
  failures with Continue / Step Over / Rerun Step / Restart / Stop
  buttons. `Tarn: Diff Last Passing` opens a standard VS Code diff tab
  comparing the most recent passing fixture against the current one.

## 0.7.0 — Last-run JSON artifact, discovery excludes, capture cascade, `exists_where`, tarn-lsp YAML gating, and opencode support

### Runner + CLI (tarn)

- **Always-on `.tarn/last-run.json` artifact.** Every run now writes a
  machine-readable JSON report alongside whatever format the human
  asked for, so failed runs can be inspected after the fact without
  rerunning in `--format json`. New `--report-json PATH` override and
  `--no-last-run-json` opt-out. Human-mode runs announce the artifact
  path on stderr.
- **Default directory excludes during discovery.** Test walking now
  skips `.git`, `.worktrees`, `node_modules`, `.venv`, `venv`, `dist`,
  `build`, `target`, `tmp`, `.tarn` by default — no more stale
  worktree copies silently doubling the run. Human-mode prints a
  discovery summary (files found, excluded roots, duplicate `tests/`
  tree warnings). `--no-default-excludes` disables the gate when the
  user genuinely wants the old behavior. Subcommands `run`, `validate`,
  and `list` all accept the flag.
- **Cascading capture-failure skip (NAZ-342).** When a step's capture
  fails, every downstream step referencing `{{ capture.<name> }}` is
  now marked `failure_category: skipped_due_to_failed_capture` with
  `error_code: skipped_dependency`, instead of flooding the report
  with unresolved-template failures. Exit code stays at 3 (the root
  cause is still `capture_error`); the skips do not escalate it.
- **Identity-based array assertions (NAZ-341).** New `exists_where`,
  `not_exists_where`, and `contains_object` (alias) operators under
  `assert.body.<path>`. Use these instead of `$[0]` lookups and exact
  `length:` assertions on shared list endpoints so tests stay green
  when an unrelated writer appends a new row.
- **Capture `where:` predicate filter.** Select array elements by
  `{ field: value }` identity before the `first` transform, so
  captures survive list ordering changes the same way identity-based
  assertions do.
- **Richer poll-timeout diagnostics.** Timeouts now carry the final
  observed `response_status`, `response_summary`, and a `poll final:`
  assertion surfacing the last-seen value so operators can
  distinguish "stuck" from "progressing but never matched" without
  rerunning. Previous behavior left both fields `null` on timeout.
- **New `fail_fast_within_test` config option.**

### Language server (tarn-lsp)

- **New `is_tarn_file_uri` gate on every request handler.** Claude
  Code's and opencode's LSP plugin formats both register servers by
  bare extension (`.yaml` / `.yml`) with no compound-extension or
  glob support, so `tarn-lsp` ends up attached to every YAML buffer
  in any project where the plugin is installed — Kubernetes manifests,
  Compose files, CI configs. Diagnostics, hover, completion,
  definition, references, prepareRename, rename, codeLens, formatting,
  codeAction, and documentSymbol now short-circuit through the
  predicate and return LSP-appropriate empty results for
  non-`*.tarn.yaml` URIs, so `tarn-lsp` stays silent on foreign YAML
  instead of emitting bogus results. Full-lifecycle regression test
  at `tarn-lsp/tests/non_tarn_yaml_gating_test.rs`.

### MCP server (tarn-mcp)

- Tool-call surface extended with `fail_fast_within_test` so the new
  `RunOptions` field threads through MCP-initiated runs.

### Schema (v1)

- `capture.where` object for predicate filters on array results.
- `assert.body.<path>.{exists_where, not_exists_where, contains_object}`.
- Mirrored in `tarn-lsp/schemas/v1/testfile.json` with the existing
  sync-verification test from 0.6.2 keeping them from drifting.

### opencode integration

- **New first-class opencode support.** Tarn is now integrated into
  [opencode](https://opencode.ai) with the same MCP + LSP + skill
  surface Claude Code gets. Since opencode has no plugin installer or
  marketplace for Rust CLIs, integration is config-driven:
  - `opencode.jsonc` at the repo root registers `tarn-mcp` (as
    `mcp.tarn` with `type: "local"`) and `tarn-lsp` (as `lsp.tarn`
    with `extensions: [".yaml", ".yml"]`).
  - `.opencode/skills/tarn-api-testing/` is a relative symlink to the
    canonical `plugin/skills/tarn-api-testing/` — no content
    duplication. Agents running `opencode` inside this repo pick up
    the skill automatically.
  - `editors/opencode/README.md` + `editors/opencode/opencode.example.jsonc`
    document how to mirror the setup in a third-party repo.
- Compound-extension caveat documented throughout: opencode's LSP
  matcher uses `path.parse(file).ext`, so the `tarn` LSP entry claims
  every `.yaml` / `.yml` in the workspace (not just `.tarn.yaml`) —
  the new `is_tarn_file_uri` gate above keeps this from being noisy.
  Install the `lsp.tarn` entry at project level only, not in global
  `~/.config/opencode/config.json`.
- opencode threaded into `README.md`, `AGENTS.md`, `docs/TARN_LSP.md`,
  `docs/MCP_WORKFLOW.md`, `docs/INDEX.md`, `docs/LAUNCH_PLAYBOOK.md`,
  `docs/site/index.html`, `docs/site/mcp.html`, `docs/site/tarn-lsp.html`,
  `plugin/skills/tarn-api-testing/SKILL.md`, and
  `plugin/skills/tarn-api-testing/references/mcp-integration.md`.

### Claude Code marketplace consolidation

- The previously separate `editors/claude-code/.claude-plugin/`
  marketplace is merged into the repo-root
  `.claude-plugin/marketplace.json`, so both `tarn` (MCP + skill) and
  `tarn-lsp` (LSP) ship from a single marketplace. New install flow:
  ```
  /plugin marketplace add NazarKalytiuk/hive
  /plugin install tarn@tarn
  /plugin install tarn-lsp@tarn --scope project
  ```
  **Breaking:** `tarn-lsp@tarn-lsp` and `tarn-lsp@tarn-plugins` (the
  pre-consolidation plugin identifiers) no longer resolve.
- `plugin/.claude-plugin/plugin.json` version field fixed — it had
  drifted to 0.2.0 while everything else was on 0.6.x; now re-aligned
  to 0.7.0 alongside the rest of the release.

### Housekeeping

- `.gitignore` gates `/.claude/` so Claude Code's session-private
  auto-memory and per-session settings never land in git.

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
