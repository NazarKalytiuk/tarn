# Changelog

## Unreleased

## 0.11.2 — Release-pipeline patch for 0.11.0

Hot-fix release for the broken 0.11.0 publish pipeline. Functionally
identical to 0.11.0 for end users; the only deltas are workflow
fixes that let `tarn-mcp`, `tarn-lsp`, and the VS Code extension
actually reach their registries.

(0.11.1 was tagged but never produced any artifacts: a YAML literal-
block indentation bug in the release.yml fix made the workflow file
fail to parse before any job started, so no commit at v0.11.1 ever
reached crates.io, the GitHub releases page, or the VS Code
Marketplace. 0.11.2 lands the version-bump-and-fix on a fresh tag
with the YAML fix applied.)

### Why a hot-fix release instead of re-running 0.11.0

The 0.11.0 tag baked in workflow files with two latent bugs that
only surfaced at publish time:

- `wait_for_crate_version` parsed `cargo search` output, which on
  the runner emits ANSI color codes because release.yml sets
  `CARGO_TERM_COLOR: always`. The grep never matched, so the wait
  always timed out and aborted the publish before `tarn-mcp` and
  `tarn-lsp` could be uploaded.
- The new VS Code extension integration tests (added in 0.11.0)
  expect a `target/debug/tarn` binary, but the workflow had no
  `cargo build` step before invoking them.

Re-running the v0.11.0 jobs would have re-executed the same broken
workflows because tag pushes pin the workflow file to the tagged
commit. 0.11.2 lands the fixes on a fresh tag so the publish
pipeline can finish end-to-end.

### Workflow fixes

- **`release.yml`**: `wait_for_crate_version` now polls the
  crates.io HTTP API directly (`/api/v1/crates/<name>` →
  `crate.max_version`) instead of grepping `cargo search`. The
  authoritative source removes the ANSI dependency, the sparse-
  index propagation lag, and the `cargo search` ranking quirks
  in one move. Retry budget bumped from 6 minutes to 9 minutes.
  The JSON parse uses an inline `python3 -c` so it stays a single
  YAML literal-block line and does not retrip the indentation
  hazard that bricked the v0.11.1 attempt.
- **`vscode-extension.yml` / `vscode-extension-release.yml`**:
  add `dtolnay/rust-toolchain@stable` and
  `cargo build -p tarn -p tarn-lsp` ahead of the integration
  test step so `target/debug/tarn` and `target/debug/tarn-lsp`
  exist when `runTest.ts` looks for them.

No source code or report-schema changes vs. 0.11.0.

## 0.11.0 — Release hardening + failure-category surface

Release-engineering polish on top of the 0.10.0 Agent Loop shipment,
plus a small surface bump in the report schema so the newer
cascade/shape-drift taxonomy is first-class in parser and docs.

### Report schema (`tarn`)

- `failure_category` now enumerates `response_shape_mismatch`,
  `skipped_due_to_fail_fast`, and `skipped_by_condition` alongside the
  existing categories; `error_code` adds `skipped_dependency`.
- `report::json_parse` recognizes the new categories so consumers of
  archived `report.json` files can round-trip them through the
  Rust types.
- Human-format failure grouping labels `response_shape_mismatch` steps
  as "Response shape mismatch" instead of falling back to a generic
  assertion label.

### Release + CI hardening

- Release workflow validates that `tarn`, `tarn-mcp`, and `tarn-lsp`
  all declare the tag version, and that `tarn-mcp`/`tarn-lsp` depend
  on the same `tarn` version — prevents skewed publishes.
- `publish-crates` job publishes in dependency order with a
  `cargo search`-based index wait between crates and treats "already
  uploaded" as success, so a partial publish can be safely retried.
- `release`, `publish-crates`, and `docker` jobs are now gated on
  `refs/tags/v*` so manual `workflow_dispatch` smoke runs can exercise
  `build` in isolation without accidentally cutting a release.
- CI runs `cargo audit`, the full `tarn` integration suite, and
  `cargo test -p tarn-lsp`; release binaries now include `tarn-lsp`.
- VS Code extension CI adds `npm audit --omit=dev` and integration
  tests; `RELEASE_VERIFICATION.md` documents the expanded gate.

### Installers

- `install.sh` and `action-install.sh` install `tarn-lsp` alongside
  `tarn` and `tarn-mcp` when the binary is present in the archive.

### Docs

- README, `plugin/skills/tarn-api-testing/SKILL.md`, and
  `docs/site/ai-workflows.html` list the full failure-category
  taxonomy and the new diagnosis-loop branch for response-shape
  drift.

## 0.10.0 — Agent Loop: artifact-oriented runs, root-cause-first diagnostics, MCP parity (NAZ-400..416)

Ships the Agent Loop epic (NAZ-409): every `tarn run` now persists an
immutable archive under `.tarn/runs/<run_id>/` with `report.json`,
`summary.json`, `failures.json`, `state.json`, and a streaming
`events.jsonl` so a failed run is triagable from disk without replay.
Diagnostics lead with root-cause-first fingerprinting plus
response-shape drift detection that proposes replacement JSONPaths,
and cascade fallout collapses under its upstream root cause instead
of inflating failure counts. Nine new CLI surfaces land in this
release — `tarn failures`, `tarn rerun --failed`, `tarn inspect`,
`tarn diff`, `tarn report`, `tarn lint`, `tarn impact`,
`tarn scaffold`, `tarn pack-context`, plus `tarn run --agent` — and
`tarn-mcp` reaches full CLI parity through artifact-oriented tools so
an agent can drive the whole impact → scaffold → run → inspect → fix
loop without shell fallbacks.

### Runner + CLI (`tarn`, unless marked otherwise)

Bullets below cover the `tarn` crate unless explicitly flagged
otherwise. Two bullets (NAZ-407, NAZ-416) affect `tarn-mcp`; NAZ-408
is docs-only.

#### Artifact & reporting pipeline (NAZ-400, NAZ-401, NAZ-403, NAZ-404, NAZ-405)

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
- **`tarn inspect` and `tarn diff` for run drill-down and comparison
  (NAZ-405).** `tarn inspect <run_id> [target]` loads
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

#### Diagnostics & agent loop (NAZ-402, NAZ-412, NAZ-413, NAZ-414, NAZ-415)

- **`tarn failures` groups failures by root cause and suppresses
  cascades (NAZ-402).** New top-level subcommand that loads a prior
  run's `failures.json` (defaulting to `.tarn/failures.json`, or the
  archive at `.tarn/runs/<id>/failures.json` with `--run`) and groups
  entries by a stable fingerprint. Status mismatches key off expected
  vs actual plus method and a UUID/digit-normalized URL path; body
  JSONPath misses split between `missing` and `value_mismatch`;
  connection/timeout errors key off host + coarse kind; unknown shapes
  fall back to a deterministic `unclassified` bucket. Cascade
  categories (`skipped_due_to_failed_capture`, `skipped_due_to_fail_fast`)
  are never fingerprinted — they attach as `blocked_steps` to the
  group they came from (via the `root_cause` pointer NAZ-401 writes,
  or a same-test fallback), so repeated fallout no longer dilutes the
  signal. Renders human-readable output by default (with
  `--include-cascades` to expand the blocked list) and a stable JSON
  envelope with `--format json`; exit code 0 on a clean archive, 1
  when failures exist, 2 on load/parse errors. The fingerprinter is
  exposed as a library (`tarn::report::failures_command`) so
  `tarn report`, `tarn diff`, `tarn run --agent`, and the MCP
  `tarn_last_root_causes` tool all group failures the same way.
- **Streaming run lifecycle events to `events.jsonl` (NAZ-413).**
  Every `tarn run` now also writes an append-only NDJSON stream to
  `.tarn/runs/<run_id>/events.jsonl`, mirrored to `.tarn/events.jsonl`
  as a convenience pointer. Each line is one event; consumers tail
  the file to react to failures as they happen rather than waiting
  for `report.json` to land. The envelope is stable and versioned
  (`schema_version: 1`, plus `run_id`, UTC `ts`, monotonic 0-based
  `seq`), and every event carries `file_id` / `test_id` (8-hex
  truncation of SHA-256) so lines correlate with `failures.json`
  entries and the full report after the fact. Emitted kinds:
  `run_started`, `file_started`, `file_completed`, `test_started`,
  `test_completed`, `step_started`, `step_completed`,
  `capture_failure`, `polling_timeout`, `run_completed`. Bodies and
  headers are intentionally absent — the stream is a correlation
  spine, not a payload transport. Each write flushes so an early
  crash still produces a correct line-bounded prefix; parallel
  workers serialize through one mutex, with `seq` providing the
  total order readers should sort by. `--no-last-run-json`
  suppresses both the archive file and the pointer, matching
  existing transient-run semantics.
- **Response-shape drift detection with candidate fix paths
  (NAZ-415).** A body-assertion or capture JSONPath miss on a JSON
  object response now runs a pure tail-segment heuristic that
  proposes replacement paths: `$.uuid` missing on
  `{"request": {"uuid": "…"}}` suggests `$.request.uuid` at high
  confidence, `$.data.items[0].id` on `{"items":[…]}` suggests
  `$.items[0].id` at medium confidence. When at least one candidate
  is high confidence the failure is reclassified from
  `assertion_failed` / `capture_error` to a new
  `response_shape_mismatch` category; lower-confidence drift keeps
  the original category but still carries the observed shape and
  candidates on a new `response_shape_mismatch` field of each
  `failures.json` entry. `tarn failures` groups drift by
  `shape_drift:<expected_path>:<observed_keys_hash>` so one
  contract change surfaces as a single group across every file
  that hit it, and cascade fallout of drift still points its
  `root_cause` back at the drift step so agents can trace blocked
  skips to the real cause.
- **Agent-oriented compact run payload `tarn run --agent` (NAZ-412).**
  New flag on `tarn run` that emits a single `AgentReport` JSON
  object on stdout — run id, pass/fail status, exit code, totals,
  root-cause-first failure groups (reusing the NAZ-402 fingerprinter
  so `tarn failures` and the agent payload agree), per-cause
  request/response excerpts capped at 300 chars, the full NAZ-415
  `response_shape_mismatch` hint, a `cascaded_steps` list that
  folds downstream `skipped_due_to_failed_capture` noise under its
  upstream root cause, and up to three machine-dispatchable
  `next_actions` per cause (`replace_jsonpath` first when a
  high-confidence drift candidate exists, always an `inspect_step`
  command, plus `rerun_failed` when there are ≥2 causes and
  `check_server_reachable` for network failures). Root causes are
  capped at 10 with a `notes` entry pointing to `tarn failures`
  for the full list. Stdout is the AgentReport only; normal
  stdout-bound formatters are suppressed, progress is silenced, and
  stderr still prints `run id:` and `run artifacts:` so humans
  watching the run see progress. File-bound `--format` targets
  compose; `--agent` refuses combination with `--ndjson`, `--watch`,
  or another stdout-bound non-JSON format with exit code 2.
  Payload is `schema_version: 1` so MCP / agent clients can depend
  on the envelope.
- **Minimum remediation bundles `tarn pack-context` (NAZ-414).**
  New CLI subcommand that reads a run's persisted `summary.json` +
  `failures.json` (and opportunistically `report.json` / `state.json`)
  and emits a compact, deterministic payload carrying the same
  small bundle of context an agent needs after a failed run: failed
  YAML snippets (re-extracted from source with the parser's line
  locations so the exact failing step block is surfaced verbatim
  with two lines of leading context, capped at 40 lines), request
  and response excerpts for only the failing steps, captures
  lineage (produced by the failing step, `consumed_by` later steps
  via `{{ capture.X }}` interpolation, `blocked` from cascade
  assertions and shape-drift diagnoses), related same-test cascade
  fallout, and a rerun command + selector per entry. `--failed`
  (default), `--file`, and `--test` filters compose as AND and are
  repeatable. `--run <id>` reads any historical archive (`last` /
  `latest` / `@latest` / `prev` aliases honored); without it the
  workspace-level pointers under `.tarn/` are used. `--format json`
  (default) emits a stable envelope (`schema_version: 1`);
  `--format markdown` renders the same data as headings + fenced
  YAML + bullet lists. `--max-chars N` caps serialized output
  (default 16000) and trims lowest-priority sections first
  (markdown-only snippet stripping past entry 3 → drop
  `consumed_by` past 3 per entry → drop `related_steps` past 3 per
  entry → truncate response bodies → drop entries past the 10th)
  with a trailing `notes` entry pointing to the full `report.json`
  when anything was cut. Source files edited since the run degrade
  gracefully with a `yaml_snippet_warning: "source changed since
  run"` instead of blocking the whole command. Exit codes: 0 on
  success, 2 on unknown run id / missing artifacts / parse error.

#### CLI UX & workflows (NAZ-404, NAZ-406, NAZ-408)

- **`tarn lint` with eight structural reliability rules (NAZ-406).**
  New subcommand separate from `tarn validate` — validate answers
  "will this parse?"; lint answers "will this test fall over next
  month?". Ships eight rules with stable ids: TL001 (positional
  capture on a shared list endpoint), TL002 (same-list capture
  reused across tests in a file), TL003 (polling with a weak stop
  condition — no body assertion, broad status shorthand), TL004
  (mutation step asserts a body but no status), TL005 (shorthand
  `"2xx"` status on a step whose name implies a specific code like
  201/204), TL006 (capture from response body with no body
  assertion to anchor shape drift), TL007 (duplicate named test
  within a file — correctness error), and TL008 (hard-coded
  absolute URL). Severity is bucketed into `error` (TL007),
  `warning` (TL001..TL004), and `info` (TL005, TL006, TL008). CLI
  supports `--format human|json`, `--severity error|warning|info`
  (default `warning`), `--lint-allow-absolute-urls`, and
  `--no-default-excludes` for discovery parity with `run`. JSON
  output carries a stable `schema_version: 1` envelope with
  `files_scanned` and an array of `findings` whose entries include
  `rule_id`, `severity`, `file`, `line`, `column`, `step_path`
  (`FILE::TEST::STEP` for jump-to-source), `message`, and `hint`.
  Exit codes: 0 when no findings reach the threshold, 1 when
  findings at or above threshold exist, 2 on I/O or parse error.
  Each rule lives in its own `src/lint/tl00N_*.rs` module with
  per-rule unit tests; the orchestrator runs all rules, merges and
  sorts findings by `(line, rule_id)`, and hands the result to the
  CLI for rendering.
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

#### Code generation & test discovery (NAZ-410, NAZ-411)

- **`tarn impact` maps changes to test targets (NAZ-410).** New CLI
  subcommand that answers "what should I run next for this change?"
  without executing any tests. Accepts four additive input flavors:
  `--diff` (reads `git diff --name-only HEAD` in the CWD), `--files
  PATH[,…]`, `--endpoints METHOD:PATH[,…]` (e.g.
  `GET:/users/:id,POST:/users`), and `--openapi-ops ID[,…]`. At
  least one source is required (exit 2 otherwise). Matching is a
  ranked sum of boring per-signal heuristics — URL method+path
  equality after normalizing query strings, scheme+host, leading
  `{{ env.* }}`, and concrete UUID / integer / `{id}` / `:id` /
  `{{ capture.x }}` segments (high, weight 40); path prefix match
  on the same method (medium, 15); shared `openapi_operation_ids`
  (high, 38; new optional `Vec<String>` field on `TestFile`
  persisted via serde `#[serde(default)]`); tag token synthesized
  from the endpoint path (medium, 12); direct edit to a
  `.tarn.yaml` (high, 50); shared topic directory segment between
  a changed source file and a test file (medium, 10, with
  `src`/`tests`/`lib`/… treated as non-informative); `include:` /
  multipart fixture reference match via literal substring search in
  the test's raw YAML (medium, 8); and a last-resort name-token
  substring hit on test file name / test name / step URL (low, 3).
  Output is available as `--format human` (grouped by confidence,
  one line per match, tailed with an advice section) or
  `--format json` (stable `schema_version: 1` envelope with
  `inputs`, `matches[]` carrying `file` / `test` / `confidence` /
  `score` / `reasons[]` / `run_hint.command`, `low_confidence_only`
  boolean, and `advice[]`). `--min-confidence low|medium|high`
  filters the result set post-scoring. `--path` narrows test
  discovery and `--no-default-excludes` disables the standard
  ignore list; otherwise the same discovery contract as `tarn run`
  applies. Advice messages call out weak signals explicitly —
  empty results get "provide `--endpoints` or narrow `--path`",
  low-only results get "declare `openapi_operation_ids:` to
  strengthen the signal", and any `--openapi-ops` call on a suite
  without declarations gets "adopting the field sharpens impact
  analysis". Exit codes: 0 on success, 2 on missing inputs /
  invalid `METHOD:PATH` / git or filesystem errors.
- **`tarn scaffold` bootstraps a valid `.tarn.yaml` skeleton
  deterministically (NAZ-411).** New CLI subcommand that turns one
  of four inputs — an OpenAPI operation id
  (`--from-openapi SPEC --op-id ID`), a raw `curl` command
  (`--from-curl FILE`, backslash-continuations folded and
  `$VAR` / `${VAR}` rewritten to `{{ env.VAR }}`), an explicit
  method + URL pair (`--method M --url U`), or a previously
  recorded fixture (`--from-recorded PATH`, accepts a file, a step
  directory auto-picking `latest-passed.json` / the newest history
  entry, or the legacy `request.json` + `response.json` split
  form) — into a scaffold-quality Tarn file with the request
  block, default headers/body shape where known, a placeholder
  `status: 2xx` + `body: $: { type: object }` assertion, obvious
  id-shaped captures pulled from the response schema / recorded
  response, and a machine-greppable `# TODO:` comment on every
  inferred-but-unverified field (categories: `env`, `method`,
  `url`, `path_param`, `headers`, `auth`, `body`, `assertion`,
  `capture`). Exactly one input mode is required (exit 2 on zero
  or multiple). `--out PATH` writes to disk; default is stdout.
  `--force` is required to overwrite an existing `--out`.
  `--name NAME` overrides the inferred top-level `name:`.
  `--format yaml` (default) emits the skeleton itself; `--format
  json` emits a stable `schema_version: 1` envelope carrying
  `source_mode`, the inferred request (method / url / headers /
  body_shape / response_captures / response_shape_keys /
  path_params), every TODO with final 1-based `line` number, the
  YAML as a string, and a `validation.parsed_ok` / `schema_ok`
  round-trip summary. Output is deterministic byte-for-byte across
  runs with identical inputs: headers use `BTreeMap`, body keys
  preserve the input order, captures are rendered sorted, and no
  clock / RNG state is read at scaffold time (random placeholders
  emit Tarn built-in names like `$uuid_v4` / `$random_hex(8)` so
  resolution happens under the faker seed at run time, not at
  scaffold time). Every generated file round-trips through
  `parser::parse_str` before being written — a scaffold that
  produces invalid YAML is reported as an internal bug with the
  offending text attached, never silently persisted. Exit codes:
  0 on success, 2 on bad inputs / I/O errors / round-trip
  validation failures.

### MCP server (`tarn-mcp`) — NAZ-407, NAZ-416

- **MCP parity with the CLI plus artifact-oriented APIs (NAZ-407).**
  The MCP server now writes every run's artifacts under
  `.tarn/runs/<run_id>/` — `report.json`, `summary.json`, `failures.json`,
  `state.json`, `events.jsonl` plus the `.tarn/` pointer files — through
  the same library helpers the CLI uses (`report::state_writer`,
  `report::summary`, `report::agent_report`, etc.) so an MCP-driven run
  is byte-identical to a CLI-driven one for the same inputs. `tarn_run`
  gained a `report_mode` enum (`full`/`summary`/`failures`/`agent`,
  default `agent`) that selects which slice of the run is returned
  inline; every response carries `{run_id, exit_code, report, artifacts}`
  so agents can open the heavy payloads back from disk instead of
  keeping them in context. Five new tools surface the prior work on the
  agent loop: `tarn_last_failures` (grouped failures JSON per NAZ-402),
  `tarn_get_run_artifacts` (paths + existence flags, no payload load),
  `tarn_rerun_failed` (wraps NAZ-403's `RerunSelection` and mints a
  fresh run id/archive), `tarn_report` (concise NAZ-404 view from disk),
  and `tarn_inspect` (NAZ-405 run/file/test/step views). Every handler
  now returns a structured `ToolError { code, message, data }` instead
  of a plain string — error codes live in the reserved `-32050..-32099`
  JSON-RPC server block and are surfaced both as an `isError: true`
  `tools/call` content block and as an embedded `error` object on the
  response envelope. The library helper `tarn::report::compute_exit_code`
  was promoted out of `main.rs` so the CLI and MCP agree on exit-code
  precedence without drift.
- **High-level MCP APIs for the agent inner loop (NAZ-416).** Five new
  tools wrap the prior library work so an agent can drive
  impact → scaffold → run → inspect → fix without shell fallbacks:
  `tarn_impact` (wraps `tarn::impact::analyze` with the same JSON shape
  `tarn impact --format json` emits, plus a structured error for
  missing inputs that carries a hint), `tarn_scaffold` (wraps
  `tarn::scaffold::generate` for all four input modes
  openapi/curl/explicit/recorded, optionally writing the rendered YAML
  to disk with an overwrite guard), `tarn_run_agent` (convenience
  surface over `tarn_run` with `report_mode: agent` pre-selected and
  the full selector grammar — `test_filter`, `step_filter`, `select`,
  `tag` — exposed), `tarn_last_root_causes` (failures-first read that
  returns only the fingerprinted groups from NAZ-402, no cascade noise),
  and `tarn_pack_context` (wraps NAZ-414's remediation bundle with both
  JSON and markdown render targets and the `max_chars` truncation
  budget). `tarn_rerun_failed` now echoes the caller's `env_name`/
  `vars` and the selection slice the runner executed so an agent can
  confirm its own intent without re-parsing. Every new response carries
  `schema_version: 1` — the same version the underlying CLI artifacts
  already emit so a consumer reading both surfaces sees consistent
  versioning. Four new error codes in the reserved `-32050..-32099`
  window (`ERR_IMPACT_INVALID_INPUT`, `ERR_IMPACT_PARSE_FAILED`,
  `ERR_SCAFFOLD_INVALID_INPUT`, `ERR_SCAFFOLD_FAILED`,
  `ERR_PACK_CONTEXT_INVALID_INPUT`) keep domain errors structured
  instead of stringified. The golden `tools-list.json.golden` is
  refreshed with descriptions ending in `equivalent to: tarn …` so
  agents can cross-check behaviour against the CLI.

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
  /plugin marketplace add NazarKalytiuk/tarn
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
`nazarkalytiuk.github.io/tarn/`.

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
