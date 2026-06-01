---
name: tarn-api-testing
description: |
  Write, run, debug, and iterate on API tests using Tarn — a CLI-first API testing tool with .tarn.yaml files and structured JSON output. Use when: writing API tests, running tarn commands, debugging test failures, generating tests from OpenAPI specs or curl commands, setting up CI smoke tests, or integrating with tarn-mcp. Triggers: "API test", "tarn", ".tarn.yaml", "test this endpoint", "smoke test", "integration test for API".
---

# Tarn API Testing Skill

Tarn is a CLI-first API testing tool. Tests are declarative YAML files (`.tarn.yaml`). Results are structured JSON designed for AI agent consumption. Single binary, zero runtime dependencies.

## Core Workflow — Failures-First Loop

**This is the canonical sequence.** Default to it for every debugging task. Do not reach for the full JSON report until steps 3–6 have failed to answer the question.

```
1. tarn validate <path>                  # syntax/config before running
2. tarn run <path>                       # produces .tarn/runs/<run_id>/
3. tarn failures                         # root-cause groups; cascades collapsed
4. tarn inspect last FILE::TEST::STEP    # full context for ONE failure
5. Patch tests or application code
6. tarn rerun --failed                   # reruns only the failing subset
7. tarn diff prev last                   # confirm fixed / new / persistent
8. Full report.json only when 3–6 are insufficient
```

### Agent rules (read before doing anything else)

- **Never slurp `report.json` if `failures.json` is sufficient.** `failures.json` is a 1–50 KB file with one entry per failing step. `report.json` is often megabytes. `tarn failures` is the correct entry point, not `cat .tarn/last-run.json | jq ...`.
- **Never parse the full run JSON when a summary + inspect will answer the question.** `tarn inspect last FILE::TEST::STEP` opens one step's request/response/assertion block directly. Use it instead of grepping the whole report.
- **Never blame business logic before confirming the response shape didn't drift.** See the "Response-shape drift" section below — most `assertion_failed` + `skipped_due_to_failed_capture` clusters are a single upstream capture path that no longer matches the new response body.
- **Cascade skips (`skipped_due_to_failed_capture`) are suppressed noise.** Fix the root cause first. `tarn failures` already collapses them into `└─ cascades: N skipped`. Do NOT open each cascade skip individually; they will all resolve when the upstream step is fixed.
- Always `tarn validate` before `tarn run`. Always use `--format json` (or `llm`) when parsing results programmatically.

### Failures-first commands

```bash
tarn failures                         # group latest run's failures by root cause, collapse cascades
tarn failures --format json           # structured root-cause groups for programmatic consumption
tarn failures --include-cascades      # expand the cascade list (only when you actually need it)
tarn failures --run <run_id>          # triage a specific historical archive
tarn inspect last                     # run-level summary of the latest archive
tarn inspect last tests/users.tarn.yaml::create-user::1   # one failing step, full context
tarn inspect last --format json --filter-category assertion_failed
tarn rerun --failed                   # rerun only failing (file, test) pairs from the latest run
tarn rerun --failed --run <run_id>    # rerun failures from a specific archive
tarn diff prev last                   # see which failures were fixed / added / persistent
tarn diff prev last --format json --filter-category assertion_failed
```

Aliases for run ids: `last`, `latest`, `@latest` (most recent archive), `prev` (the one before latest).

### Deep-inspection flow (only when failures-first is insufficient)

Reach for the full JSON report ONLY when you genuinely need something `failures` + `inspect` can't surface — e.g. a cross-step captures map, a passed-step response body, or a bespoke grep against every executed request. Otherwise the small artifacts are always preferred:

- `.tarn/runs/<run_id>/summary.json` — run id, timings, exit code, failed-file list
- `.tarn/runs/<run_id>/failures.json` — one entry per failing step with coordinates, category, message, request method/url, response status, ~500-char redacted body excerpt
- `.tarn/runs/<run_id>/report.json` — full structured report (megabytes; last resort)
- `.tarn/summary.json` / `.tarn/failures.json` — pointers to the latest run's artifacts

## Commands

```bash
tarn run                              # run all .tarn.yaml files in tests/
tarn run tests/users.tarn.yaml        # run specific file
tarn run --format json                # structured JSON output
tarn run --format json --json-mode compact  # compact JSON (no pretty-print)
tarn run --format llm                 # LLM-friendly: verdict line + failures only
tarn run --format compact             # one-line-per-file with inline failures
tarn run --tag smoke                  # run only tests tagged "smoke"
tarn run --env staging                # use tarn.env.staging.yaml
tarn run --only-failed                # hide passing tests, show failures only
tarn run --only-fails                 # alias for --only-failed
tarn run --verbose-responses          # embed request/response on passing steps too
tarn run --max-body 16384             # override 8 KiB truncation cap
tarn run --no-progress                # disable streaming progress (batch dump)
tarn run --test-filter create-user    # run only the named test across all files
tarn run --step-filter 2              # run only step #2 (0-based)
tarn run --step-filter "Login"        # run only steps named "Login"
tarn run --select tests/users.tarn.yaml::create-user        # narrow to a single test
tarn run --select tests/users.tarn.yaml::create-user::0     # narrow to a single step (0-based)
tarn run --ndjson                     # stream one JSON event per line on stdout
tarn run --ndjson --format json=out.json  # NDJSON stream + final JSON report to file
tarn run --redact-header x-custom-secret  # ad-hoc redaction for this run
tarn run --no-fixtures                # skip writing .tarn/fixtures/** this run
tarn run --fixture-retention 10       # keep last N fixtures per step (default 5)
tarn run --parallel -j 4              # 4-worker parallel file scheduling
tarn run --parallel --no-parallel-warning  # silence the opt-in warning in CI
tarn summary .tarn/last-run.json      # re-render a saved JSON report as llm output
tarn summary - --format compact       # re-render from stdin as compact output
tarn validate                         # check syntax without executing
tarn validate tests/users.tarn.yaml   # validate specific file
tarn validate --format json           # structured validation output (line/column errors)
tarn env                              # print resolved env chain (human)
tarn env --json                       # structured env dump with provenance
tarn fmt                              # reformat all .tarn.yaml files in place
tarn fmt tests/users.tarn.yaml        # reformat a specific file
tarn fmt --check                      # CI mode: exit 1 if any file needs formatting
tarn list                             # list all tests and steps (dry run)
tarn failures                         # group latest run's failures by root cause (see Failures-First Loop)
tarn failures --run <id> --format json
tarn rerun --failed                   # rerun just the failing (file, test) pairs
tarn inspect last                     # run-level summary of the latest archive
tarn inspect last FILE::TEST::STEP    # drill into one failing step
tarn diff prev last                   # diff two runs' failure sets (fixed/new/persistent)
```

`failures`, `rerun --failed`, `inspect`, and `diff` all read the per-run archives under `.tarn/runs/<run_id>/` introduced in NAZ-400/401. They are the preferred failure-triage entry points. See the **Failures-First Loop** section near the top of this file.

### Output formats for agents vs humans

When stdout is not a TTY and no `--format` is set, tarn auto-selects `llm` so piped output is always LLM-digestible without an external parser. When stdout is a TTY, the default is `human`. Pass `--format human` explicitly to force boxed human output into a pipe.

- **`--format llm`** — one-line grep-friendly verdict (`tarn: PASS 498/500 steps, 2 failed, 14 files, 16.3s`) followed by only failure blocks with request/response/assertion details. Stable ordering across runs. Use this when an LLM will read the output.
- **`--format compact`** — per-file badge + inline failure expansion + grouped failure summary. Middle ground between `human` and `llm`. Respects `-v` to show captured values per test.
- **`--format human`** — the colored TTY-friendly boxed output. Default on a TTY.
- **`--format json`** — the full structured report. Huge; pipe into `tarn summary` or `--only-failed`.
- **`tarn summary <run.json>`** — re-render a saved JSON report as `llm` or `compact` format without re-running the tests. Accepts `-` for stdin (`tarn run --format json | tarn summary -`).

### Streaming and filtering

- `tarn run` streams per-test output as each test finishes (per-file in `--parallel` mode). With `--format human` the stream writes to stdout; with structured formats (`json`, `junit`, `tap`, `html`, `curl`) it writes to stderr so stdout stays parseable.
- `--only-failed` drops passing files, tests, and steps from both human and JSON output. Summary counts still reflect the full run.
- `--no-progress` disables streaming and prints the final report in one batch at the end — use it when a CI harness already timestamps every line.
- `--verbose-responses` embeds request/response bodies and resolved captures in the JSON / compact / llm report for every step, pass or fail. Per-step opt-in via step-level `debug: true`. Default body cap is 8 KiB; override with `--max-body <bytes>`.

### Streaming and selective execution

- `--select FILE[::TEST[::STEP]]` narrows a run to specific files, tests, or steps. Repeatable — multiple selectors union. ANDs with `--tag`. `STEP` accepts either a step name or a 0-based integer index. Step-level selection runs ONLY that step with no prior steps, so any captures produced by earlier steps will be unset — prefer test-level selectors for chained flows and reserve step-level selection for isolated smoke checks.
- `--test-filter NAME` / `--step-filter NAME-OR-INDEX` apply a wildcard selector across every discovered file — shorter than `--select` when you want the same filter everywhere.
- `--ndjson` streams one JSON event per line on stdout: `file_started`, `step_finished` (with `phase: setup|test|teardown`), `test_finished`, `file_finished`, and a final `done` event carrying the aggregated summary. Failing `step_finished` events include `failure_category`, `error_code`, and `assertion_failures`. In parallel mode each file's events are emitted atomically on `file_finished` to avoid interleaving across files. `--ndjson` composes with `--format json=path` to write a final report to a file while streaming events on stdout. It collides with any other structured format that would also write to stdout — pick one stdout consumer.

## Writing Tests

### Minimal Test

```yaml
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 200
```

### Full-Featured Test

```yaml
name: User CRUD
env:
  base_url: "http://localhost:3000"

defaults:
  headers:
    Content-Type: "application/json"

setup:
  - name: Authenticate
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "admin@example.com"
        password: "{{ env.admin_password }}"
    capture:
      token: "$.token"
    assert:
      status: 200

tests:
  create-and-fetch:
    description: Create a user then fetch it
    steps:
      - name: Create user
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          headers:
            Authorization: "Bearer {{ capture.token }}"
          body:
            name: "Jane Doe"
            email: "jane-{{ $uuid }}@example.com"
        capture:
          user_id: "$.id"
        assert:
          status: 201
          duration: "< 500ms"
          body:
            "$.name": "Jane Doe"
            "$.id": { type: string, not_empty: true }

      - name: Fetch user
        request:
          method: GET
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
          headers:
            Authorization: "Bearer {{ capture.token }}"
        assert:
          status: 200
          body:
            "$.name": "Jane Doe"

teardown:
  - name: Cleanup
    request:
      method: DELETE
      url: "{{ env.base_url }}/users/{{ capture.user_id }}"
      headers:
        Authorization: "Bearer {{ capture.token }}"
    assert:
      status: { in: [200, 204, 404] }
```

### Key Rules

- File extension must be `.tarn.yaml`
- `name` is required at top level
- Must have either `steps` (flat) or `tests` (grouped) — not both
- `setup` runs once before all tests; `teardown` runs even if tests fail
- Steps within a test are sequential and share captures
- Each test group is independent
- Captures preserve JSON types (numbers stay numbers, not strings)
- Automatic cookie jar is on by default; disable with `cookies: "off"` or set `cookies: "per-test"` to clear the default jar between named tests within a file. Setup and teardown still share the file-level jar, and named cookie jars (multi-user scenarios) are untouched. The `--cookie-jar-per-test` CLI flag is equivalent at the command line and overrides the file setting except when the file sets `cookies: "off"` — that always wins.

## Environment Variables

Six-layer resolution (highest priority wins):

1. `--var key=value` CLI flag
2. Shell environment `$VAR`
3. `tarn.env.local.yaml` (git-ignored secrets)
4. `tarn.env.{name}.yaml` (per-environment, selected via `--env name`)
5. `tarn.env.yaml` (shared defaults)
6. Inline `env:` block in the test file

Reference in YAML: `{{ env.variable_name }}`

**Environment file format:**

```yaml
# tarn.env.yaml
base_url: "http://localhost:3000"
api_version: "v1"
```

## Captures

Capture values from responses for use in subsequent steps via `{{ capture.name }}`.

```yaml
capture:
  user_id: "$.id"                          # JSONPath (shorthand)
  token: "$.auth.token"                    # nested JSONPath
  session:                                 # header with regex
    header: "set-cookie"
    regex: "session=([^;]+)"
  csrf_cookie:                             # cookie by name
    cookie: "csrf_token"
  final_url:                               # final URL after redirects
    url: true
  status_code:                             # HTTP status code
    status: true
  body_text:                               # whole response body
    body: true
    regex: "ID: (\\d+)"                    # optional regex extraction
```

### Optional / conditional captures

Use when a field may or may not appear in the response — typical for
idempotent endpoints that return different shapes on `201 Created` vs
`409 Conflict`, or list endpoints that might be empty.

```yaml
capture:
  # optional: missing path → variable unset, not an error. Downstream
  # `{{ capture.X }}` references yield a distinct "was declared
  # optional and not set" error so the root cause is clear.
  middle_name:
    jsonpath: "$.middle_name"
    optional: true

  # default: use this value when the path is missing. Implies optional.
  count:
    jsonpath: "$.total"
    default: 0
  label:
    jsonpath: "$.label"
    default: "unnamed"

  # when: only attempt capture when the status matches. Same matcher
  # grammar as the `status:` assertion (exact, range, `in: [...]`).
  created_id:
    jsonpath: "$.id"
    when:
      status: 201
  ok_id:
    jsonpath: "$.id"
    when:
      status:
        in: [200, 201]
```

### Step-level `if:` / `unless:`

Skip an entire step when an interpolated expression is falsy / truthy.
Pairs well with `optional:` captures for idempotent flows.

```yaml
steps:
  - name: Lookup widget
    request:
      method: GET
      url: "{{ env.base_url }}/widgets/{{ env.slug }}"
    capture:
      widget_id:
        jsonpath: "$.id"
        optional: true

  - name: Create widget (only when missing)
    unless: "{{ capture.widget_id }}"   # unset optional → falsy → run
    request:
      method: POST
      url: "{{ env.base_url }}/widgets"
      body: { slug: "{{ env.slug }}" }

  - name: Audit widget
    if: "{{ capture.widget_id }}"       # only if we have an id
    request:
      method: POST
      url: "{{ env.base_url }}/widgets/{{ capture.widget_id }}/audit"
```

Truthy rules: empty / whitespace / `"false"` / `"0"` / `"null"` / unresolved `{{ ... }}` placeholders are falsy; every other value is truthy. Skipped steps report with `failure_category: skipped_by_condition` and `passed: true` so they never flip the exit code.

## Assertions

See `references/assertion-reference.md` for the complete operator list.

**Quick reference for the most common assertions:**

```yaml
assert:
  status: 200                              # exact
  status: "2xx"                            # range shorthand
  status: { in: [200, 201] }              # set
  duration: "< 500ms"                      # response time
  headers:
    content-type: "application/json"       # exact
    x-request-id: 'matches "^[a-f0-9-]+"' # regex
  body:
    "$.name": "Jane"                       # exact match
    "$.id": { type: string, not_empty: true }
    "$.age": { gt: 0, lt: 200 }           # numeric range
    "$.tags": { type: array, contains: "admin" }
    "$.email": { matches: "^[^@]+@[^@]+$" }
```

Multiple operators on the same JSONPath combine with AND logic.

## Built-in Functions

Use in any string field:

- `{{ $uuid }}` — UUID v4 (alias for `$uuid_v4`)
- `{{ $uuid_v4 }}` — random UUID v4
- `{{ $uuid_v7 }}` — time-ordered UUID v7 (Unix-ms prefix)
- `{{ $timestamp }}` — Unix epoch seconds
- `{{ $now_iso }}` — ISO 8601 datetime
- `{{ $random_hex(8) }}` — random hex string of length N
- `{{ $random_int(1, 100) }}` — random integer in range

### Faker (EN locale)

- `{{ $email }}`, `{{ $first_name }}`, `{{ $last_name }}`, `{{ $name }}`, `{{ $username }}`, `{{ $phone }}`
- `{{ $word }}`, `{{ $words(3) }}`, `{{ $sentence }}`, `{{ $slug }}`
- `{{ $alpha(8) }}`, `{{ $alnum(8) }}`, `{{ $choice(a, b, c) }}`, `{{ $bool }}`
- `{{ $ipv4 }}`, `{{ $ipv6 }}`

### Reproducible runs

Set `TARN_FAKER_SEED=<u64>` or `tarn.config.yaml: faker.seed: <u64>` to freeze every RNG-backed built-in for the process. The environment variable wins. Wall-clock generators (`$timestamp`, `$now_iso`, and the timestamp prefix of `$uuid_v7`) stay real-time — seeding only pins randomness.

## Formatting

Tarn ships `tarn fmt`, a canonical `.tarn.yaml` formatter.

- `tarn fmt [PATH]` reformats whole files in place. Omit `PATH` to reformat every `.tarn.yaml` under the current working directory.
- `tarn fmt --check` is the CI variant: exit `0` if every file is already formatted, `1` if any file would change. No files are written in check mode.
- The `tarn::format::format_document` library surface is shared with `tarn-lsp`'s `textDocument/formatting` request, so CLI and editor output are byte-identical.
- Range formatting is deliberately unsupported — `tarn fmt` is whole-document only.

## Structured Validation and Env Introspection

Two CLI subcommands return structured JSON for editors and CI.

- `tarn validate --format json` emits `{"files": [{"file", "valid", "errors": [{"message", "line", "column"}]}]}`. YAML syntax errors carry precise `line` and `column` extracted from `serde_yaml`; semantic parser errors without a known location fall back to `message`-only (`line` and `column` are optional). Exit code is `0` when every file is valid, `2` otherwise. The default human format is unchanged.
- `tarn env --json` dumps the resolved env chain with provenance. Inline vars declared in `tarn.config.yaml` environments are redacted against `redaction.env` (case-insensitive) so secrets never appear in stdout. The per-environment file field is `source_file` (matching the VS Code extension contract). Environments are sorted alphabetically. Exit code is `0` on success, `2` on configuration error.

## JSON Output

`tarn run --format json` returns structured results. Key fields for diagnosis:

- `summary.status` — `"PASSED"` or `"FAILED"`
- `files[].tests[].steps[].status` — per-step result
- `files[].tests[].steps[].failure_category` — why it failed
- `files[].tests[].steps[].assertions.failures[]` — assertion details with `expected`, `actual`, `message`
- `files[].tests[].steps[].request` — sent request (failed steps only)
- `files[].tests[].steps[].response` — received response (failed steps only)
- `files[].tests[].steps[].remediation_hints` — suggested fixes

See `references/json-output.md` for the full schema.

### Failure Categories

| Category | Meaning | Typical Fix |
|----------|---------|-------------|
| `assertion_failed` | Request succeeded, assertion mismatch | Fix assertion or application |
| `response_shape_mismatch` | JSONPath missed because the response shape drifted | Replace the JSONPath with the suggested candidate and add a guard assertion |
| `connection_error` | DNS/connect/TLS failure | Check URL, server status, network |
| `timeout` | Request exceeded allowed time | Increase timeout or fix server |
| `parse_error` | Invalid YAML, JSONPath, or config | Fix syntax |
| `capture_error` | Capture extraction failed | Fix JSONPath or response shape |
| `unresolved_template` | `{{ ... }}` couldn't resolve before send | Check env or upstream capture; optional captures produce a distinct message |
| `skipped_due_to_failed_capture` | Downstream step skipped because a capture it needed failed earlier | Fix the root cause; skips are suppressed noise, not new failures |
| `skipped_due_to_fail_fast` | Downstream step skipped after an earlier failure in the same test | Fix the first failing step |
| `skipped_by_condition` | Step-level `if:` / `unless:` evaluated to skip | Intentional; `passed: true`, never flips exit code |
| `skipped_by_policy` | Shell `command:` step skipped because the run was not granted `--allow-exec` | Intentional security default; pass `--allow-exec` (CLI) or set `allow_exec: true` in `tarn.config.yaml` once the file is trusted |
| `command_failed` | Shell `command:` step exited non-zero, was killed, or its `stdout_regex` capture missed | Inspect the failure message for the exit code / signal / regex pattern; rerun the script manually to reproduce |

### Diagnosis Loop (structured report, step level)

Use this when you already have a single failing step open (e.g. via `tarn inspect last FILE::TEST::STEP --format json`):

```
1. Read failure_category first — it tells you which branch below applies
2. If assertion_failed       → read assertions.failures[].expected vs actual
3. If response_shape_mismatch → apply the suggested replacement JSONPath after checking response.body
4. If connection_error       → check request.url for unresolved {{ }} templates
5. If timeout                → check server state / increase step timeout
6. If capture_error          → check previous step's response shape (see "Response-shape drift")
7. If unresolved_template    → check env chain or upstream capture
8. If skipped_due_to_failed_capture/skipped_due_to_fail_fast → IGNORE; fix the root cause grouped by `tarn failures`
9. Fix YAML or application → tarn rerun --failed → tarn diff prev last
```

### Response-shape drift (check this before blaming business logic)

**Symptom.** One step fails with a status-mismatch or capture error. Several downstream steps in the same test are marked `skipped_due_to_failed_capture`. `tarn failures` collapses them into one root-cause group plus `└─ cascades: N skipped`.

**Why it usually happens.** The endpoint's response shape changed. The old capture JSONPath no longer matches. Every downstream step that needed the captured value is now unreachable.

**Worked example — the reopen-request incident.** A create/reopen endpoint used to return:

```json
{ "uuid": "e4f2…", "status": "pending" }
```

and the test captured `user_id: "$.uuid"`. The endpoint now returns an envelope:

```json
{ "request": { "uuid": "e4f2…" }, "stageStatus": "pending" }
```

`$.uuid` no longer matches. The step's own status/body assertions fail (or its capture fails); every downstream `GET /requests/{{ capture.user_id }}` step skips with `skipped_due_to_failed_capture`.

**Recovery loop.**

1. `tarn failures --format json` — read the one root-cause group. Cascade count tells you how many steps will unblock once the root cause is fixed.
2. `tarn inspect last FILE::TEST::STEP --format json` — open the failing step. Read `response.body_excerpt` (or the full `response` block) and identify the new shape.
3. Find the field in the new response. For the example above: `$.request.uuid` instead of `$.uuid`.
4. Patch the capture path **and** add a body-type assertion as a guard rail so the next drift is caught at the failing step, not at the downstream cascade.

Before:

```yaml
capture:
  user_id: "$.uuid"
assert:
  status: 201
```

After:

```yaml
capture:
  user_id: "$.request.uuid"
assert:
  status: 201
  body:
    "$.request": { type: object }
    "$.request.uuid": { type: string, not_empty: true }
```

5. `tarn rerun --failed` — replay only the failing `(file, test)` pair; don't rerun the whole suite.
6. `tarn diff prev last --format json` — confirm the root cause moved to `fixed` and that no `new` failures appeared. `persistent` items need another pass.

### Mutation vs read-response patterns

Shape drift is disproportionately common on mutation endpoints. Use these defaults:

- **Mutation endpoints (`POST`/`PUT`/`PATCH`)** often return an envelope distinct from the read shape — e.g. `{"request": {...}, "meta": {...}}` vs the read endpoint's `{"uuid": "..."}`. Assert the envelope explicitly with a type assertion and capture from the wrapped path (`$.request.uuid`, not `$.uuid`). Do NOT reuse a read-shape capture on a mutation response.
- **Read endpoints (`GET /resource/:id`)** typically return the resource directly and drift less, but paginated/list endpoints wrap in `{"items": [...], "page": N}` and must never be flattened — capture from `$.items[0].id`, not `$[0].id`.
- **When authoring a new capture**, do a one-shot `GET /resource/:id` (or, for mutations, a live `POST` with `--dry-run: false` and `debug: true` on the step) and copy the minimal JSONPath from the *observed* body. Never capture from memory.

## Shell `command:` steps (NAZ-464)

A step can be either a `request:` (HTTP) or a `command:` (shell) — never both. `command:` lets you run fixture-prep / version-stamping helpers inline with your suite without a wrapper script.

```yaml
setup:
  - name: Bump fixture version
    command:
      run: "python3 bin/bump-version.py --version {{ $timestamp }}"
      pass_env: [PATH, PYTHON]
      workdir: "scripts/fixtures"
      capture:
        bumped_version:
          stdout_regex: "version=([^\\s]+)"
        result_code:
          exit_code: true
```

### Security model — `command:` is INERT by default

Without an explicit opt-in, every `command:` step is skipped with `failure_category: skipped_by_policy`. The runner does not spawn the child process at all. To authorize execution:

- CLI: `tarn run --allow-exec ...` (preferred for ad-hoc runs and CI)
- Project config: `allow_exec: true` in `tarn.config.yaml` (for trusted, project-owned repos)
- MCP: pass `"allow_exec": true` in the `tarn_run` / `tarn_run_agent` / `tarn_rerun_failed` tool params

A freshly cloned repo never executes commands until the human opts in. This is intentional — a malicious `.tarn.yaml` cannot exfiltrate secrets just by being cloned and a default `tarn run`.

### Env scrubbing (`pass_env`)

The child process gets a tiny baseline env: `PATH`, `HOME` (`USERPROFILE` on Windows), `TMPDIR`/`TEMP`/`TMP`. Anything else must be allowlisted via `pass_env: [VAR1, VAR2]`. **Tarn's own `{{ env.x }}` and `{{ capture.x }}` chain is NEVER implicitly forwarded** — secrets in `tarn.env.local.yaml` stay scoped to template interpolation. If you need a tarn env variable in the shell, add it to `pass_env` only if the parent process actually exports it; otherwise interpolate it into `command.run` directly (`run: "FOO={{ env.foo }} python3 ..."`).

### Captures from commands

Each entry under `command.capture:` must use exactly one of:

- `stdout_regex: "PATTERN"` — capture group 1 (or full match if no group). A miss fails the step under `failure_category: command_failed` unless `optional: true`.
- `exit_code: true` — capture the literal exit code as an integer.

`assert:` and `poll:` are **only** valid on `request:` steps; the runner rejects them on command steps at parse time.

### When commands fail

`failure_category: command_failed` covers all of: non-zero exit, signal kill (Unix), and required-but-missed stdout regex. The error message identifies which case fired. `skipped_by_policy` is a benign skip (`passed: true`) — the run keeps green when nothing else failed.

## Exit Codes

- **0** — all tests passed
- **1** — one or more tests failed
- **2** — configuration or parse error
- **3** — runtime error (network failure, timeout)

## MCP Integration

Tarn ships with `tarn-mcp`, an MCP server for Claude Code, opencode, Cursor, Windsurf, and Codex. (pi has no native MCP — it uses the `tarn-api-testing` skill plus the `tarn` CLI directly.)

See `references/mcp-integration.md` for setup and tool reference, and `editors/codex/`, `editors/opencode/`, and `editors/pi/` for per-agent integration kits.

## Parallel Execution and Isolation

`--parallel -j N` schedules whole files across N workers. Tests within a file always run sequentially. Two markers let you opt files in or out of the parallel pool:

```yaml
# tests/flows/payments.tarn.yaml
name: Payments
serial_only: true            # file stays on the serial worker under --parallel
group: postgres              # or: bucket with other pg-group files onto one worker

tests:
  refund:
    serial_only: true        # promotes the whole file to serial (file is the isolation unit)
    steps: [...]
```

- `serial_only: true` on a file (or any of its named tests) pins it onto the serial worker after every parallel bucket finishes.
- `group: "name"` runs every file with the same group string on the same worker, serially — useful when tests share an external resource (DB, message broker, S3 bucket) but different resources can still parallelize across buckets.
- Running `--parallel` without `parallel_opt_in: true` in `tarn.config.yaml` prints a one-line stderr warning. Set it once after you've verified your suite is safe to parallelize.

```yaml
# tarn.config.yaml
parallel_opt_in: true
parallel: false              # don't parallelize by default, but allow --parallel
```

## Fixtures (.tarn/fixtures/** and .tarn/state.json)

Every run writes two kinds of artifacts under `.tarn/` (gitignored by default):

- **`.tarn/fixtures/<file-hash>/<test-slug>/<step-index>/`** — per-step request/response/captures (redacted), retaining up to 5 rolling-history files plus a `latest-passed.json` copy. Feeds the LSP's inline JSONPath hover and the `tarn.diffLastPassing` debug command. Disable with `--no-fixtures`; override retention with `--fixture-retention N`.
- **`.tarn/state.json`** — human-readable snapshot of the last run (pass/fail counts, failure list, env metadata, argv). Atomic rewrite. Tail this file from an LLM harness to answer "what just happened?" without re-running.
- **`.tarn/last-run.json`** — always-on machine-readable report of the last run (includes `args`, `env_name`, `working_directory`, `start_time`, `end_time` so "rerun last failures" replays with the same env). Override path with `--report-json PATH`, disable with `--no-last-run-json`.

## Editor Integration via tarn-lsp

Tarn ships `tarn-lsp`, an editor-agnostic LSP 3.17 stdio server delivered as its own binary in the same release pipeline as `tarn` and `tarn-mcp` (and bundled inside the Tarn Docker image). Any LSP client — Claude Code, VS Code, Neovim, Helix, Zed, IntelliJ — can register it against `.tarn.yaml` files to get the same static intelligence the VS Code extension provides.

Agent-relevant capabilities:

- **Schema-aware completion** — top-level and nested keys from the bundled Tarn testfile schema, env keys sorted by resolution priority, captures in scope for the current step, builtin snippets with parameter placeholders.
- **Hover** — provenance for `{{ env.* }}` (full resolution chain), `{{ capture.* }}` (declaring step + JSONPath source), and `{{ $builtin }}` signatures. Hover over a JSONPath literal in `assert.body.*` also evaluates the expression against the step's last recorded response and appends the result inline.
- **Diagnostics** — `publishDiagnostics` driven by `tarn::validation::validate_document`, with ranges matching the JSON report locations.
- **Navigation** — go-to-definition and references for env keys (workspace-scoped) and captures (per-test), plus rename with collision detection.
- **Code lens** — `Run test` and `Run step` lenses on every test and step. Stable command IDs `tarn.runTest` / `tarn.runStep`, selector format `FILE::TEST::STEP_INDEX`. The server emits the lenses but does NOT execute them — the client (Claude Code, VS Code) is responsible for running the selector via `tarn run --select ...`.
- **Formatting** — `textDocument/formatting` shares the `tarn::format::format_document` library surface with the `tarn fmt` CLI, so edits are byte-identical to a CLI reformat.
- **Code actions** — `extract env var` (lift a string literal into an inline `env:` key), `capture this field` (stub a `capture:` from a JSONPath literal under the cursor), `scaffold assert from response` (generate a pre-typed `assert.body` block from a recorded response).
- **Quick fix** — `CodeActionKind::QUICKFIX` shares the `tarn::fix_plan` library with the MCP tool `tarn_fix_plan`. The MCP path is report-driven, the LSP path is diagnostic-driven, the remediation engine is the same.
- **`workspace/executeCommand tarn.evaluateJsonpath`** — evaluates a JSONPath expression against either an inline response payload or a recorded step fixture. Returns a typed result: `{ result: "match", value }`, `{ result: "no_match", available_top_keys }`, or `{ result: "no_fixture", message }`. Agents iterate on a wrong JSONPath path without ever rerunning the test.
- **`tarn.explainFailure`** — given `{ file, test?, step? }`, returns a structured blob: expected vs actual, the last passing step's captures, and a root-cause hint (`5xx → server error`, `401/403 → auth`, `JSONPath no-match → response shape changed`, `unresolved capture → upstream failed`). Paste the blob into a reasoning loop.
- **Run-from-gutter commands** — `tarn.runFile`, `tarn.runTest`, `tarn.runStep`, `tarn.runLastFailures`. Stream `tarn/progress` notifications, return the final `RunOutcome`.
- **Step-through debugger** — `tarn.debugTest` starts a session that pauses between steps and publishes `tarn/captureState` notifications carrying the current captures map and last response. Drive it with `tarn.debugContinue`, `tarn.debugStepOver`, `tarn.debugRerunStep`, `tarn.debugRestart`, `tarn.debugStop`. `tarn.getCaptureState` polls the current snapshot.
- **Response diff vs last passing** — `tarn.diffLastPassing` returns a structured status / headers / body diff against the most recent passing fixture for a step, or `{ error: "no_baseline" }` when no passing run exists.
- **Fixture store access** — `tarn.getFixture` reads the latest fixture for a step (or `{ error: "no-fixture" }`); `tarn.clearFixtures` removes them.

All commands return `{ schema_version: 1, data: ... }` envelopes so the contract can evolve without breaking clients. See `docs/commands.json` for the full manifest and parameter shapes.

Recorded fixtures live under `.tarn/fixtures/<file-hash>/<test-slug>/<step-index>/` — the runner writes them automatically on every run. The LSP reads this directory to drive hover / JSONPath evaluation / diff. No more manual sidecar convention.

See `docs/TARN_LSP.md` for the full spec and `editors/claude-code/tarn-lsp-plugin/README.md` for the Claude Code plugin install flow.

## Advanced Features

### Includes

Reuse shared steps across files:

```yaml
setup:
  - include: ./shared/auth-setup.tarn.yaml
    with:
      role: admin
```

### Polling

Wait for async operations:

```yaml
- name: Wait for processing
  request:
    method: GET
    url: "{{ env.base_url }}/jobs/{{ capture.job_id }}"
  poll:
    until:
      body:
        "$.status": "completed"
    interval: "2s"
    max_attempts: 10
```

### Named Cookie Jars

Multi-user scenarios:

```yaml
- name: Admin login
  cookies: "admin"
  request:
    method: POST
    url: "{{ env.base_url }}/login"
    body: { email: "admin@test.com", password: "pass" }

- name: User login
  cookies: "user"
  request:
    method: POST
    url: "{{ env.base_url }}/login"
    body: { email: "user@test.com", password: "pass" }
```

### Lua Scripts

Escape hatch for complex validation:

```yaml
- name: Custom check
  request:
    method: GET
    url: "{{ env.base_url }}/data"
  script: |
    local body = json.decode(response.body)
    assert(#body.items > 0, "expected items")
    for _, item in ipairs(body.items) do
      assert(item.price > 0, "price must be positive: " .. item.name)
    end
```

### GraphQL

```yaml
- name: Query users
  request:
    method: POST
    url: "{{ env.base_url }}/graphql"
    graphql:
      query: |
        query GetUser($id: ID!) {
          user(id: $id) { name email }
        }
      variables:
        id: "{{ capture.user_id }}"
  assert:
    status: 200
    body:
      "$.data.user.name": { not_empty: true }
```

### Multipart Upload

```yaml
- name: Upload file
  request:
    method: POST
    url: "{{ env.base_url }}/upload"
    multipart:
      fields:
        - name: "title"
          value: "My Photo"
      files:
        - name: "file"
          path: "./fixtures/test.jpg"
          content_type: "image/jpeg"
```

### Auth Helpers

```yaml
defaults:
  auth:
    bearer: "{{ capture.token }}"
    # OR
    basic:
      username: "{{ env.api_user }}"
      password: "{{ env.api_pass }}"
```

### Redirect Assertions

```yaml
assert:
  redirect:
    url: "https://api.example.com/final"
    count: 2
```

## Troubleshooting

Always start at `tarn failures` — it groups by root cause, collapses cascade skips, and points at a single failing step. Only after that does it make sense to inspect a specific step.

**"Unresolved template" in URL** — The `{{ env.x }}` or `{{ capture.x }}` was not resolved. Check that the env var exists in the resolution chain or that the previous capture step passed. If the cascade count is non-zero, fix the upstream capture first, then `tarn rerun --failed`.

**"Connection refused"** — The target server is not running. Start it before running tests.

**"Capture extraction failed"** — The JSONPath does not match the actual response body. See "Response-shape drift" above. Open the failing step with `tarn inspect last FILE::TEST::STEP --format json`, read `response.body_excerpt`, and fix the JSONPath against the *observed* body, not from memory.

**"Parse error"** — Invalid YAML syntax. Run `tarn validate` to see the exact error location.

**Tests pass individually but fail together** — Steps share captures within a test group. Check for capture name collisions or ordering issues.

**A lot of downstream steps are skipped** — That's `skipped_due_to_failed_capture` fanning out from one upstream failure. Do NOT open each skipped step. Run `tarn failures` and fix the single root-cause group at the top.

## File Organization

Recommended project structure:

```
tests/
  health.tarn.yaml          # simple smoke tests
  users.tarn.yaml           # user CRUD flows
  auth.tarn.yaml            # authentication tests
  shared/
    auth-setup.tarn.yaml    # reusable auth steps
tarn.env.yaml               # shared env defaults
tarn.env.local.yaml         # local secrets (git-ignored)
tarn.env.staging.yaml       # staging environment
tarn.config.yaml            # optional project config
```
