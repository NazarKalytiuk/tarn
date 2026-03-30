<p align="center">
  <strong>Tarn</strong><br>
  <em>API testing that AI agents can write, run, and debug</em>
</p>

<p align="center">
  <a href="https://github.com/NazarKalytiuk/tarn/actions/workflows/ci.yml"><img src="https://github.com/NazarKalytiuk/tarn/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/NazarKalytiuk/tarn/releases/latest"><img src="https://img.shields.io/github/v/release/NazarKalytiuk/tarn" alt="Release"></a>
  <a href="https://github.com/NazarKalytiuk/tarn/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License"></a>
</p>

---

Tests are `.tarn.yaml` files &mdash; YAML in, structured JSON out. Single binary, zero dependencies. Designed for the AI coding workflow: an LLM writes tests, runs `tarn run --format json`, parses results, iterates.

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

```
$ tarn run
 TARN  Running tests/health.tarn.yaml

 ● Health check

   ✓ GET /health (4ms)

 Results: 1 passed (15ms)
```

## Why Tarn?

- **50% fewer tokens** than equivalent TypeScript/Python tests &mdash; faster LLM generation, lower cost
- **Structured JSON output** with request/response on failures &mdash; machines parse it, not regex
- **Single binary** &mdash; `curl | sh` install, runs in any CI, no runtime needed
- **MCP server** &mdash; direct integration with Claude Code, Cursor, Windsurf
- **Everything you need** &mdash; REST, GraphQL, captures, cookies, multipart, includes, polling, Lua scripting, parallel execution, 5 output formats

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh

# Install to a custom directory
TARN_INSTALL_DIR="$HOME/.local/bin" curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh

# Or build/install from source
cargo install --git https://github.com/NazarKalytiuk/tarn.git --bin tarn
```

Binaries for **macOS** (Intel & Apple Silicon) and **Linux** (amd64 & arm64) on the [releases page](https://github.com/NazarKalytiuk/tarn/releases).
Each release also includes `tarn-checksums.txt` for SHA256 verification.

Installer notes:
- `install.sh` verifies the downloaded archive against `tarn-checksums.txt`
- `TARN_INSTALL_DIR` controls the install destination
- `HIVE_INSTALL_DIR` is still accepted as a backward-compatible alias during the rename transition
- Manual verification also works with `shasum -a 256 -c tarn-checksums.txt`

## Quick Start

```bash
tarn init                              # scaffold tests/health.tarn.yaml + tarn.env.yaml
# Edit tarn.env.yaml to point at your API, or start one on http://localhost:3000
tarn run                               # run all tests
tarn run tests/health.tarn.yaml        # run the scaffolded test directly
tarn run --env staging                 # use staging environment
tarn run --format json                 # structured output for LLM/CI
tarn run --watch                       # re-run on file changes
tarn run --parallel                    # run files in parallel
tarn list --tag smoke                  # inspect matching tests without running them
```

## Hello World

Want a fully local demo path from this repo?

```bash
PORT=3000 cargo run -p demo-server &
cargo run -p tarn -- run examples/demo-server/hello-world.tarn.yaml
```

This exercises a local API with no external network dependency.

## Table of Contents

- [Test File Format](#test-file-format)
- [Assertions](#assertions)
- [Variables](#variables)
- [Cookies](#cookies)
- [Multipart / File Upload](#multipart--file-upload)
- [Includes](#includes)
- [GraphQL](#graphql)
- [Polling](#polling)
- [Lua Scripting](#lua-scripting)
- [CLI Reference](#cli-reference)
- [Output Formats](#output-formats)
- [Performance Testing](#performance-testing)
- [MCP Server](#mcp-server)
- [Troubleshooting](#troubleshooting)
- [GitHub Action](#github-action)
- [Configuration](#configuration)
- [Step Options](#step-options)
- [JSON Schema](#json-schema)
- [Shell Completions](#shell-completions)
- [Development](#development)

## Test File Format

Test files use `.tarn.yaml` and can be organized in any directory structure.

### Minimal Test

```yaml
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "http://localhost:3000/health"
    assert:
      status: 200
```

### Full Format

```yaml
version: "1"
name: "User CRUD Operations"
description: "Tests complete user lifecycle"
tags: [crud, users, smoke]

env:
  base_url: "http://localhost:3000/api/v1"

defaults:
  headers:
    Content-Type: "application/json"
  timeout: 5000
  retries: 1

tests:
  create_and_verify:
    description: "Create a user, then verify it exists"
    tags: [smoke]
    steps:
      - name: Create user
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          body:
            name: "Jane Doe"
            email: "jane.{{ $random_hex(6) }}@example.com"
        capture:
          user_id: "$.id"
        assert:
          status: 201
          body:
            "$.name": "Jane Doe"
            "$.id": { type: string, not_empty: true }

      - name: Verify user
        request:
          method: GET
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
        assert:
          status: 200
          body:
            "$.id": "{{ capture.user_id }}"
```

### Setup and Teardown

`setup` runs once before all tests. `teardown` runs after all tests **even if tests fail**.

```yaml
name: "CRUD with auth"

setup:
  - name: Login
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "{{ env.admin_email }}"
        password: "{{ env.admin_password }}"
    capture:
      auth_token: "$.token"

teardown:
  - name: Cleanup
    request:
      method: POST
      url: "{{ env.base_url }}/test/cleanup"

tests:
  my_test:
    steps:
      - name: Authenticated request
        request:
          method: GET
          url: "{{ env.base_url }}/users"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        assert:
          status: 200
```

## Assertions

### Status

```yaml
assert:
  status: 200              # exact match
  status: "2xx"            # any 2xx status
  status:                  # set of allowed codes
    in: [200, 201, 204]
  status:                  # range
    gte: 400
    lt: 500
```

### Body (JSONPath)

All body assertions use [JSONPath](https://www.rfc-editor.org/rfc/rfc9535) expressions.

**Equality:**

```yaml
body:
  "$.name": "Alice"              # string
  "$.age": 30                    # number
  "$.active": true               # boolean
  "$.deletedAt": null            # null
  "$.field": { eq: "value" }     # explicit
  "$.field": { not_eq: "bad" }   # inequality
```

**Numeric comparisons:**

```yaml
body:
  "$.age": { gt: 18, lt: 100 }
  "$.count": { gte: 1, lte: 50 }
```

**String assertions:**

```yaml
body:
  "$.email": { contains: "@example.com" }
  "$.id": { starts_with: "usr_", matches: "^usr_[a-z0-9]+$" }
  "$.name": { not_empty: true }
  "$.code": { length: 6 }
  "$.msg": { not_contains: "error" }
```

**Type checks:**

```yaml
body:
  "$.name": { type: string }
  "$.tags": { type: array, length_gt: 0 }
  "$.meta": { type: object }
```

**Existence:**

```yaml
body:
  "$.id": { exists: true }
  "$.internal": { exists: false }
```

**Combined (AND logic):**

```yaml
body:
  "$.id": { type: string, not_empty: true, starts_with: "usr_" }
```

### Headers

```yaml
assert:
  headers:
    content-type: "application/json"                    # exact match
    content-type: contains "application/json"           # substring
    x-request-id: matches "^[a-f0-9-]{36}$"            # regex
```

Header names are case-insensitive.

### Duration

```yaml
assert:
  duration: "< 500ms"
  duration: "<= 1s"
```

## Variables

### Environment Variables

| Priority | Source | Example |
|----------|--------|---------|
| 1 (highest) | CLI `--var` | `--var base_url=http://staging` |
| 2 | Shell env `${VAR}` | `password: "${ADMIN_PASSWORD}"` |
| 3 | `tarn.env.local.yaml` | (gitignored, for secrets) |
| 4 | `tarn.env.{name}.yaml` | `--env staging` loads this |
| 5 | `tarn.env.yaml` | default env file |
| 6 (lowest) | Inline `env:` block | in the test file itself |

### Captures (Chaining)

Capture values from responses to use in subsequent steps. Captured values preserve their original JSON types (numbers stay numbers, booleans stay booleans).

```yaml
steps:
  - name: Login
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "admin@example.com"
        password: "password123"
    capture:
      token: "$.token"              # JSONPath capture from body
      user_id: "$.user.id"          # nested path

  - name: Use token
    request:
      method: GET
      url: "{{ env.base_url }}/users"
      headers:
        Authorization: "Bearer {{ capture.token }}"
```

**Header capture** &mdash; capture values from response headers with optional regex:

```yaml
capture:
  session_token:
    header: "set-cookie"
    regex: "session_token=([^;]+)"
  request_id:
    header: "x-request-id"
```

**JSONPath with regex** &mdash; extract a sub-match from a body field:

```yaml
capture:
  user_id:
    jsonpath: "$.message"
    regex: "ID: (\\w+)"
```

### Built-in Functions

```yaml
"{{ $uuid }}"                    # UUID v4
"{{ $random_hex(8) }}"           # 8-char hex string
"{{ $random_int(1, 100) }}"      # random integer in range
"{{ $timestamp }}"               # unix timestamp
"{{ $now_iso }}"                 # ISO 8601 datetime
```

## Cookies

Tarn automatically captures `Set-Cookie` headers and sends stored cookies on subsequent requests. This is enabled by default.

```yaml
name: Auth flow
steps:
  - name: Login
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "admin@test.com"
        password: "secret"
    # Set-Cookie from response is automatically stored
    assert:
      status: 200

  - name: Access protected resource
    request:
      method: GET
      url: "{{ env.base_url }}/profile"
    # Cookie header is automatically sent
    assert:
      status: 200
```

Disable automatic cookies per file:

```yaml
cookies: "off"
```

## Auth Today

Tarn does not ship first-class auth blocks yet. Today the intended path is explicit headers plus env or captured values:

```yaml
request:
  headers:
    Authorization: "Bearer {{ env.token }}"
    X-API-Key: "{{ env.api_key }}"
```

Basic auth works through a precomputed header:

```yaml
request:
  headers:
    Authorization: "Basic {{ env.basic_auth_b64 }}"
```

OAuth2 client-credentials is not built in yet. Fetch the token in a setup step, capture it, then reuse it in later requests.

### Step-Level Cookie Control

Use `cookies: false` on a step to bypass the cookie jar entirely. No cookies are sent and no `Set-Cookie` headers are captured:

```yaml
steps:
  - name: Login
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "admin@test.com"
        password: "secret"
    assert:
      status: 200

  - name: Test unauthenticated access
    cookies: false
    request:
      method: GET
      url: "{{ env.base_url }}/profile"
    assert:
      status: 401
```

### Named Cookie Jars

For multi-user scenarios, use named jars to maintain separate cookie sessions:

```yaml
steps:
  - name: Login as admin
    cookies: "admin"
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "admin@test.com"
        password: "secret"

  - name: Login as viewer
    cookies: "viewer"
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "viewer@test.com"
        password: "secret"

  - name: Admin can manage users
    cookies: "admin"
    request:
      method: GET
      url: "{{ env.base_url }}/admin/users"
    assert:
      status: 200

  - name: Viewer cannot manage users
    cookies: "viewer"
    request:
      method: GET
      url: "{{ env.base_url }}/admin/users"
    assert:
      status: 403
```

Each named jar is independent &mdash; cookies captured in `"admin"` are never sent with `"viewer"` requests. Steps without a `cookies:` field (or with `cookies: true`) use the default jar.

### CSRF Protection

When the cookie jar sends cookies automatically, frameworks with CSRF protection (e.g., Better Auth) may reject requests that lack an `Origin` header. Add it to `defaults` to fix:

```yaml
defaults:
  headers:
    Content-Type: "application/json"
    Origin: "http://localhost:3000"
```

If your app derives the expected origin from the request URL, set `Origin` to match `env.base_url`:

```yaml
defaults:
  headers:
    Origin: "{{ env.base_url }}"
```

## Multipart / File Upload

Send multipart form data for file uploads using the `multipart:` field:

```yaml
steps:
  - name: Upload photo
    request:
      method: POST
      url: "{{ env.base_url }}/api/photos"
      headers:
        Authorization: "Bearer {{ capture.token }}"
      multipart:
        fields:
          - name: "title"
            value: "My Photo"
          - name: "description"
            value: "A test upload"
        files:
          - name: "photo"
            path: "./fixtures/test.jpg"
            content_type: "image/jpeg"
          - name: "thumbnail"
            path: "./fixtures/thumb.png"
            filename: "custom-name.png"
    assert:
      status: 201
```

> Note: `multipart` cannot be combined with `body` or `graphql` on the same step.

## Includes

Reuse shared step sequences across test files with `include:` directives:

```yaml
name: User tests
setup:
  - include: ./shared/auth-setup.tarn.yaml
steps:
  - name: Get users
    request:
      method: GET
      url: "{{ env.base_url }}/users"
    assert:
      status: 200
```

The included file's `setup` and `steps` are inlined at the include point. Includes work in `setup`, `teardown`, `steps`, and `tests.*.steps`. Circular includes are detected and rejected.

## GraphQL

Native GraphQL support with the `graphql:` block. Automatically sets `Content-Type: application/json` and constructs the standard GraphQL JSON body.

```yaml
steps:
  - name: Get user
    request:
      method: POST
      url: "{{ env.base_url }}/graphql"
      graphql:
        query: |
          query GetUser($id: ID!) {
            user(id: $id) {
              id
              name
              email
            }
          }
        variables:
          id: "{{ capture.user_id }}"
        operation_name: "GetUser"
    assert:
      status: 200
      body:
        "$.data.user.name": "Alice"
        "$.errors": { exists: false }
```

## Polling

Re-execute a step until a condition is met. Useful for async workflows where you need to wait for a state change.

```yaml
steps:
  - name: Create export
    request:
      method: POST
      url: "{{ env.base_url }}/exports"
    capture:
      export_id: "$.id"
    assert:
      status: 202

  - name: Wait for completion
    request:
      method: GET
      url: "{{ env.base_url }}/exports/{{ capture.export_id }}"
    poll:
      until:
        body:
          "$.status": "completed"
      interval: "2s"
      max_attempts: 15
    assert:
      status: 200
      body:
        "$.status": "completed"
```

`poll.until` uses the same assertion syntax. The step re-executes every `interval` until the `until` condition passes or `max_attempts` is reached.

## Lua Scripting

For logic that goes beyond declarative assertions, use inline Lua scripts. Scripts run after the HTTP response is received and have access to `response` and `captures`.

```yaml
steps:
  - name: Validate complex logic
    request:
      method: GET
      url: "{{ env.base_url }}/users"
    script: |
      -- Access response
      assert(response.status == 200, "Expected 200")

      -- Work with the response body (Lua table)
      local users = response.body.users
      assert(#users > 0, "Expected at least one user")

      -- Cross-field validation
      for _, user in ipairs(users) do
        assert(user.email:find("@"), "Invalid email for " .. user.name)
      end

      -- Set captures for subsequent steps
      captures.first_user_id = users[1].id
    assert:
      status: 200
```

**Available in Lua:**
- `response.status` &mdash; HTTP status code
- `response.headers` &mdash; response headers table
- `response.body` &mdash; response body as Lua table
- `captures` &mdash; read/write captures table
- `assert(condition, message)` &mdash; assertion (collected, not thrown)

## CLI Reference

```
tarn run [PATH] [OPTIONS]          Run test files
tarn bench <PATH> [OPTIONS]        Benchmark a step
tarn validate [PATH]               Validate YAML without running
tarn list                          List all tests
tarn init                          Scaffold a new project
tarn update                        Update to the latest version
tarn update --check                Check for updates without installing
tarn completions <SHELL>           Generate shell completions
```

### `tarn run` Options

| Flag | Description |
|------|-------------|
| `--format <FORMAT>` | `human` (default), `json`, `junit`, `tap`, `html` |
| `--tag <TAGS>` | Filter by tag (comma-separated, AND logic) |
| `--var <KEY=VALUE>` | Override env variables (repeatable) |
| `--env <NAME>` | Load `tarn.env.{name}.yaml` |
| `-v, --verbose` | Print full request/response for every step |
| `--dry-run` | Show interpolated requests without sending |
| `-w, --watch` | Re-run on file changes |
| `--parallel` | Run test files in parallel |
| `-j, --jobs <N>` | Number of parallel workers (default: CPU count) |

### Examples

```bash
tarn run                                        # all tests
tarn run tests/auth.tarn.yaml                   # specific file
tarn run --tag smoke                            # filter by tag
tarn run --env staging                          # staging env
tarn run --var base_url=http://localhost:8080    # override var
tarn run --format json                          # JSON for LLM/CI
tarn run --format html                          # HTML dashboard
tarn run --watch                                # re-run on changes
tarn run --parallel --jobs 4                    # parallel execution
tarn run -v                                     # verbose
tarn run --dry-run                              # preview only
```

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All tests passed |
| `1` | One or more tests failed |
| `2` | Configuration/parse error |
| `3` | Runtime error (network, timeout, script) |

## Output Formats

### JSON (`--format json`)

Structured JSON with versioned schema. Key design:
- `schema_version: 1` for forward compatibility
- Full request/response included **only for failed steps**
- `failure_category` on failures: `assertion_failed`, `connection_error`, `timeout`, `parse_error`, `capture_error`
- Secrets redacted to `***`
- `request` is present for failed executed steps; `response` is omitted for connection/setup failures where no response exists

Schema files:
- test files: `schemas/v1/testfile.json`
- JSON report output: `schemas/v1/report.json`

```json
{
  "schema_version": 1,
  "summary": { "status": "FAILED", "steps": { "total": 5, "passed": 4, "failed": 1 } },
  "files": [{
    "tests": [{
      "steps": [{
        "name": "Create user",
        "status": "FAILED",
        "failure_category": "assertion_failed",
        "assertions": {
          "failures": [{ "assertion": "status", "expected": "201", "actual": "400", "message": "..." }]
        },
        "request": { "method": "POST", "url": "..." },
        "response": { "status": 400, "body": { "error": "name required" } }
      }]
    }]
  }]
}
```

Also supports: **Human** (colored terminal), **JUnit XML**, **TAP**, **HTML** (self-contained dashboard).

## Performance Testing

Reuses your existing test files for benchmarking.

```bash
tarn bench tests/health.tarn.yaml -n 1000 -c 50
tarn bench tests/health.tarn.yaml -n 500 -c 25 --ramp-up 5s
tarn bench tests/health.tarn.yaml --format json     # for CI thresholds
```

```
 TARN BENCH  GET http://localhost:3000/health — 200 requests, 20 concurrent

  Requests:      200 total, 200 ok, 0 failed (0.0%)
  Throughput:    3125.0 req/s

  Latency:
    min        1ms
    p50        2ms
    p95        43ms
    p99        45ms
    max        45ms
```

## MCP Server

Tarn includes an MCP (Model Context Protocol) server for direct integration with AI coding tools.

### Setup

Add to your Claude Code project settings (`.claude/settings.json`):

```json
{
  "mcpServers": {
    "tarn": {
      "command": "tarn-mcp",
      "args": []
    }
  }
}
```

For Cursor, add to `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "tarn": {
      "command": "tarn-mcp"
    }
  }
}
```

### Available Tools

| Tool | Description |
|------|-------------|
| `tarn_run` | Run tests, returns structured JSON results |
| `tarn_validate` | Validate YAML syntax without executing |
| `tarn_list` | List all tests and their steps |

The MCP server lets your AI agent write `.tarn.yaml` tests, execute them, parse structured results, and iterate &mdash; all without leaving the editor.

Typical agent loop:

1. `tarn_list` to discover tests and steps
2. `tarn_validate` after generating YAML
3. `tarn_run` to get structured failures
4. inspect `failure_category`, `assertions.failures`, and optional `request`/`response`
5. patch the test or application code
6. rerun until summary status is `PASSED`

See [docs/MCP_WORKFLOW.md](/Users/nazarkalituk/Documents/hive-api-test/docs/MCP_WORKFLOW.md) and [docs/AI_WORKFLOW_DEMO.md](/Users/nazarkalituk/Documents/hive-api-test/docs/AI_WORKFLOW_DEMO.md).

## Troubleshooting

Common cases:

- `connection_error`: server is down, wrong host/port, DNS issue, TLS/connect failure
- `timeout`: step timed out before receiving a complete response
- `assertion_failed`: request succeeded, but status/header/body/duration check failed
- `capture_error`: the step passed assertions, but extraction failed afterward
- `parse_error`: invalid YAML, invalid JSONPath, or invalid config surface

Agent diagnosis loop:

1. run `tarn validate` first for syntax/config errors
2. run `tarn run --format json`
3. read `failure_category` before reading the message text
4. if `response` exists, inspect it before editing assertions
5. if `request.url` still contains `{{ ... }}`, fix env/capture interpolation before retrying

Non-JSON bodies:

- Tarn preserves plain text / HTML responses as JSON strings in the structured report
- use `body: { "$": "plain text response" }` to assert the whole root string when needed

## Not Yet Shipped

Tracked, but intentionally deferred from the first public release:

- OpenAPI import / scaffold generation
- first-class auth helpers beyond manual headers and env vars
- Windows release artifacts

## GitHub Action

```yaml
- uses: NazarKalytiuk/tarn@v1
  with:
    path: tests/
    format: junit
    env: staging
```

**Inputs:**

| Input | Default | Description |
|-------|---------|-------------|
| `path` | `tests` | Test file or directory |
| `format` | `human` | Output format |
| `env` | &mdash; | Environment name |
| `tag` | &mdash; | Tag filter |
| `version` | `latest` | Tarn version |
| `vars` | &mdash; | Variables (newline-separated `KEY=VALUE`) |

## Configuration

### `tarn.config.yaml` (optional)

```yaml
test_dir: "tests"
env_file: "tarn.env.yaml"
timeout: 10000
retries: 0
parallel: false
```

Behavior:
- `test_dir` sets the default discovery directory for `tarn run`, `tarn validate`, and `tarn list`
- `env_file` changes the root env file name; Tarn also checks `.{name}` and `.local` variants
- `timeout` and `retries` act as project defaults when a test file does not set `defaults.timeout` or `defaults.retries`
- `parallel: true` makes parallel file execution the default for `tarn run`

### File-level defaults

```yaml
defaults:
  headers:
    Content-Type: "application/json"
  timeout: 5000
  retries: 1
  delay: "100ms"    # default delay before each request
```

## Step Options

### Retries

```yaml
retries: 3    # retry up to 3 times on failure (exponential backoff)
```

### Timeout

```yaml
timeout: 30000    # 30 seconds for this step
```

### Delay

```yaml
delay: "2s"    # wait before executing
```

## JSON Schema

Add to the top of your `.tarn.yaml` files for IDE autocompletion:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/schemas/v1/testfile.json
name: My test
steps: ...
```

The schema is bundled at `schemas/v1/testfile.json` in the repository.
The structured report schema is bundled at `schemas/v1/report.json`.

## Shell Completions

```bash
tarn completions bash > /etc/bash_completion.d/tarn
tarn completions zsh > ~/.zsh/completions/_tarn
tarn completions fish > ~/.config/fish/completions/tarn.fish
```

## Development

```bash
git clone https://github.com/NazarKalytiuk/tarn.git
cd tarn

cargo build                    # build
cargo test --all               # test suite
cargo clippy                   # lint
cargo fmt                      # format
bash scripts/ci/smoke.sh       # release-path smoke test

# Run demo server + examples
PORT=3333 cargo run -p demo-server &
cargo run -p tarn -- run examples/ --var base_url=http://localhost:3333
```

See [docs/RELEASE_VERIFICATION.md](docs/RELEASE_VERIFICATION.md) for the broader release-candidate checklist, including watch-mode and installer verification.

### Architecture

Pipeline: **parse YAML &rarr; resolve env &rarr; interpolate &rarr; execute HTTP &rarr; assert &rarr; report**

| Module | Role |
|--------|------|
| `model.rs` | Serde structs for `.tarn.yaml` |
| `parser.rs` | YAML loading + validation |
| `env.rs` | 6-layer env resolution |
| `interpolation.rs` | `{{ }}` template engine |
| `runner.rs` | Orchestrator (setup &rarr; tests &rarr; teardown) |
| `http.rs` | HTTP client (reqwest) |
| `capture.rs` | JSONPath + header extraction |
| `cookie.rs` | Automatic cookie jar |
| `config.rs` | `tarn.config.yaml` parsing |
| `builtin.rs` | Built-in functions (`$uuid`, `$timestamp`, etc.) |
| `update.rs` | Self-update mechanism |
| `assert/` | Status, body, headers, duration |
| `report/` | Human, JSON, JUnit, TAP, HTML |
| `scripting.rs` | Lua scripting engine (mlua) |
| `watch.rs` | File watcher (notify) |
| `bench.rs` | Performance testing (async) |

## License

MIT
