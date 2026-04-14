---
name: tarn-api-testing
description: |
  Write, run, debug, and iterate on API tests using Tarn — a CLI-first API testing tool with .tarn.yaml files and structured JSON output. Use when: writing API tests, running tarn commands, debugging test failures, generating tests from OpenAPI specs or curl commands, setting up CI smoke tests, or integrating with tarn-mcp. Triggers: "API test", "tarn", ".tarn.yaml", "test this endpoint", "smoke test", "integration test for API".
---

# Tarn API Testing Skill

Tarn is a CLI-first API testing tool. Tests are declarative YAML files (`.tarn.yaml`). Results are structured JSON designed for AI agent consumption. Single binary, zero runtime dependencies.

## Core Workflow

The primary loop when working with Tarn:

1. **Write** a `.tarn.yaml` test file
2. **Validate** syntax: `tarn validate tests/file.tarn.yaml`
3. **Run** tests: `tarn run tests/file.tarn.yaml --format json`
4. **Inspect** failures: check `failure_category`, then `assertions.failures`, then `request`/`response`
5. **Fix** the YAML or the application code
6. **Rerun** until `summary.status` is `"PASSED"`

Always validate before running. Always use `--format json` when parsing results programmatically.

## Commands

```bash
tarn run                              # run all .tarn.yaml files in tests/
tarn run tests/users.tarn.yaml        # run specific file
tarn run --format json                # structured JSON output
tarn run --format json --json-mode compact  # compact JSON (no pretty-print)
tarn run --tag smoke                  # run only tests tagged "smoke"
tarn run --env staging                # use tarn.env.staging.yaml
tarn run --only-failed                # hide passing tests, show failures only
tarn run --only-failed --format json  # CI-friendly JSON filtered to failures
tarn run --no-progress                # disable streaming progress (batch dump)
tarn run --select tests/users.tarn.yaml::create-user        # narrow to a single test
tarn run --select tests/users.tarn.yaml::create-user::0     # narrow to a single step (0-based)
tarn run --ndjson                     # stream one JSON event per line on stdout
tarn run --ndjson --format json=out.json  # NDJSON stream + final JSON report to file
tarn run --redact-header x-custom-secret  # ad-hoc redaction for this run
tarn validate                         # check syntax without executing
tarn validate tests/users.tarn.yaml   # validate specific file
tarn validate --format json           # structured validation output (line/column errors)
tarn env                              # print resolved env chain (human)
tarn env --json                       # structured env dump with provenance
tarn fmt                              # reformat all .tarn.yaml files in place
tarn fmt tests/users.tarn.yaml        # reformat a specific file
tarn fmt --check                      # CI mode: exit 1 if any file needs formatting
tarn list                             # list all tests and steps (dry run)
```

### Streaming and filtering

- `tarn run` streams per-test output as each test finishes (per-file in `--parallel` mode). With `--format human` the stream writes to stdout; with structured formats (`json`, `junit`, `tap`, `html`, `curl`) it writes to stderr so stdout stays parseable.
- `--only-failed` drops passing files, tests, and steps from both human and JSON output. Summary counts still reflect the full run.
- `--no-progress` disables streaming and prints the final report in one batch at the end — use it when a CI harness already timestamps every line.

### Streaming and selective execution

- `--select FILE[::TEST[::STEP]]` narrows a run to specific files, tests, or steps. Repeatable — multiple selectors union. ANDs with `--tag`. `STEP` accepts either a step name or a 0-based integer index. Step-level selection runs ONLY that step with no prior steps, so any captures produced by earlier steps will be unset — prefer test-level selectors for chained flows and reserve step-level selection for isolated smoke checks.
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

- `{{ $uuid }}` — UUID v4
- `{{ $timestamp }}` — Unix epoch seconds
- `{{ $now_iso }}` — ISO 8601 datetime
- `{{ $random_hex(8) }}` — random hex string of length N
- `{{ $random_int(1, 100) }}` — random integer in range

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
| `connection_error` | DNS/connect/TLS failure | Check URL, server status, network |
| `timeout` | Request exceeded allowed time | Increase timeout or fix server |
| `parse_error` | Invalid YAML, JSONPath, or config | Fix syntax |
| `capture_error` | Capture extraction failed | Fix JSONPath or response shape |

### Diagnosis Loop

```
1. Check summary.status — if PASSED, done
2. Find failed step → read failure_category
3. If assertion_failed → read assertions.failures[].expected vs actual
4. If connection_error → check request.url for unresolved {{ }} templates
5. If capture_error → check previous step's response shape
6. Fix YAML or application → rerun
```

## Exit Codes

- **0** — all tests passed
- **1** — one or more tests failed
- **2** — configuration or parse error
- **3** — runtime error (network failure, timeout)

## MCP Integration

Tarn ships with `tarn-mcp`, an MCP server for Claude Code, Cursor, and Windsurf.

See `references/mcp-integration.md` for setup and tool reference.

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
- **`workspace/executeCommand tarn.evaluateJsonpath`** — evaluates a JSONPath expression against either an inline response payload or a recorded step response, so agents can verify an assertion body expression against a real response without round-tripping through `tarn run`.

Recorded responses that unlock hover JSONPath evaluation and `scaffold assert from response` follow a sidecar convention: `<file>.tarn.yaml.last-run/<test-slug>/<step-slug>.response.json`. The LSP server only reads this directory — writing it is the client's job. Agents that want hover/JSONPath affordances to light up should drop last-run responses there.

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

**"Unresolved template" in URL** — The `{{ env.x }}` or `{{ capture.x }}` was not resolved. Check that the env var exists in the resolution chain or that the previous capture step passed.

**"Connection refused"** — The target server is not running. Start it before running tests.

**"Capture extraction failed"** — The JSONPath does not match the actual response body. Run the step with `--format json`, inspect `response.body`, and fix the JSONPath.

**"Parse error"** — Invalid YAML syntax. Run `tarn validate` to see the exact error location.

**Tests pass individually but fail together** — Steps share captures within a test group. Check for capture name collisions or ordering issues.

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
