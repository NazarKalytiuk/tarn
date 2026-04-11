<p align="center">
  <strong>Tarn</strong><br>
  <em>API testing that AI agents can write, run, and debug</em>
</p>

<p align="center">
  <a href="https://github.com/NazarKalytiuk/hive/actions/workflows/ci.yml"><img src="https://github.com/NazarKalytiuk/hive/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/NazarKalytiuk/hive/blob/main/docs/CONFORMANCE.md"><img src="https://img.shields.io/github/actions/workflow/status/NazarKalytiuk/hive/ci.yml?branch=main&label=conformance" alt="Conformance"></a>
  <a href="https://github.com/NazarKalytiuk/hive/releases/latest"><img src="https://img.shields.io/github/v/release/NazarKalytiuk/hive" alt="Release"></a>
  <a href="https://github.com/NazarKalytiuk/hive/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="License"></a>
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
- **Everything you need** &mdash; REST, GraphQL, captures, cookies, multipart, includes, polling, Lua scripting, parallel execution, 7 output formats

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/hive/main/install.sh | sh

# Install to a custom directory
TARN_INSTALL_DIR="$HOME/.local/bin" curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/hive/main/install.sh | sh

# Or build/install from source
cargo install --git https://github.com/NazarKalytiuk/hive.git --bin tarn
```

Binaries for **macOS** (Intel & Apple Silicon), **Linux** (amd64 & arm64), and **Windows** (amd64 zip) are published on the [releases page](https://github.com/NazarKalytiuk/hive/releases).
Each release also includes `hive-checksums.txt` for SHA256 verification and a generated `tarn.rb` Homebrew formula artifact.

Container path:
- `ghcr.io/<owner>/tarn:<tag>` from the release workflow

Installer notes:
- `install.sh` verifies the downloaded archive against `hive-checksums.txt`
- `TARN_INSTALL_DIR` controls the install destination
- `HIVE_INSTALL_DIR` is still accepted as a backward-compatible alias during the rename transition
- Manual verification also works with `shasum -a 256 -c hive-checksums.txt`

## Quick Start

```bash
tarn init                              # scaffold tests/health + advanced examples/ templates
# Edit tarn.env.yaml to point at your API, or start one on http://localhost:3000
tarn run                               # run all tests
tarn run tests/health.tarn.yaml        # run the scaffolded test directly
tarn fmt --check                       # verify canonical YAML formatting
tarn run --env staging                 # use staging environment
tarn run --format json                 # structured output for LLM/CI
tarn run --only-failed                 # show only failing tests in the output
tarn run --no-progress                 # disable streaming progress (batch dump at end)
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
There are more local scenarios in `examples/demo-server/` for redirects, cookies, forms, error responses, and authenticated CRUD flows.

## Table of Contents

- [Docs Index](#docs-index)
- [Test File Format](#test-file-format)
- [Assertions](#assertions)
- [Variables](#variables)
- [Cookies](#cookies)
- [Form URL-Encoding](#form-url-encoding)
- [Multipart / File Upload](#multipart--file-upload)
- [Includes](#includes)
- [GraphQL](#graphql)
- [Polling](#polling)
- [Lua Scripting](#lua-scripting)
- [CLI Reference](#cli-reference)
- [Output Formats](#output-formats)
- [Performance Testing](#performance-testing)
- [MCP Server](#mcp-server)
- [Claude Code Plugin](#claude-code-plugin)
- [Claude Code Skill](#claude-code-skill)
- [Troubleshooting](#troubleshooting)
- [GitHub Action](#github-action)
- [Configuration](#configuration)
- [Step Options](#step-options)
- [JSON Schema](#json-schema)
- [VS Code Extension](#vs-code-extension)
- [Shell Completions](#shell-completions)
- [Development](#development)

## Docs Index

Canonical project docs live in [`docs/INDEX.md`](./docs/INDEX.md).

If you are looking for product direction or comparisons, start with:

- [`docs/TARN_PRODUCT_STRATEGY.md`](./docs/TARN_PRODUCT_STRATEGY.md)
- [`docs/TARN_VS_HURL_COMPARISON.md`](./docs/TARN_VS_HURL_COMPARISON.md)
- [`docs/HURL_MIGRATION.md`](./docs/HURL_MIGRATION.md)
- [`docs/TARN_COMPETITIVENESS_ROADMAP.md`](./docs/TARN_COMPETITIVENESS_ROADMAP.md)

For AI-assisted workflows, see also:
- [Claude Code Plugin](#claude-code-plugin) &mdash; install Tarn as a Claude Code plugin
- [Claude Code Skill](#claude-code-skill) &mdash; structured knowledge for AI agents
- [`docs/MCP_WORKFLOW.md`](./docs/MCP_WORKFLOW.md) &mdash; MCP server usage patterns

For editor integrations:
- [`docs/TARN_LSP.md`](./docs/TARN_LSP.md) &mdash; `tarn-lsp` Language Server for Claude Code, Neovim, Helix, Zed, and other LSP 3.17 clients. Delivers diagnostics, hover, completion, and document symbols for `.tarn.yaml` files.
- [`docs/VSCODE_EXTENSION.md`](./docs/VSCODE_EXTENSION.md) &mdash; the VS Code extension in [`editors/vscode`](./editors/vscode).

A lightweight static docs site now lives in [`docs/site/index.html`](./docs/site/index.html) and is deployable via GitHub Pages from `.github/workflows/docs-site.yml`.

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

`request.method` accepts standard verbs and custom tokens such as `PURGE` or `PROPFIND`.

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
  "$.notes": { empty: true }
  "$.code": { length: 6 }
  "$.msg": { not_contains: "error" }
```

**Format assertions:**

```yaml
body:
  "$.request_id": { is_uuid: true }
  "$.created_at": { is_date: true }
  "$.client_ip": { is_ipv4: true }
  "$.server_ip": { is_ipv6: true }
```

**Integrity assertions:**

```yaml
body:
  "$": { bytes: 15 }                                # raw response body length
  "$.payload": { sha256: "2cf24dba5fb0a30e..." }    # matched value digest
  "$.legacy": { md5: "5d41402abc4b2a76..." }
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

### Redirects

```yaml
assert:
  redirect:
    url: "https://api.example.com/health"
    count: 2
```

`redirect.url` checks the final response URL after following redirects. `redirect.count` checks how many redirects were actually followed.

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

**Cookie capture** &mdash; capture a response cookie by name from `Set-Cookie`:

```yaml
capture:
  session_cookie:
    cookie: "session"
```

**Status capture** &mdash; capture the HTTP status code as a number:

```yaml
capture:
  status_code:
    status: true
```

**Final URL capture** &mdash; capture the final response URL after redirects:

```yaml
capture:
  final_url:
    url: true
```

**JSONPath with regex** &mdash; extract a sub-match from a body field:

```yaml
capture:
  user_id:
    jsonpath: "$.message"
    regex: "ID: (\\w+)"
```

**Whole-body regex** &mdash; extract from the full response body string:

```yaml
capture:
  body_word:
    body: true
    regex: "plain (text)"
```

**Transform-lite in interpolation** &mdash; reshape captured arrays and collections without dropping into Lua:

```yaml
request:
  form:
    first_tag: "{{ capture.tags | first }}"
    last_tag: "{{ capture.tags | last }}"
    tag_count: "{{ capture.tags | count }}"
    joined_tags: "{{ capture.tags | join('|') }}"
    words: "{{ capture.message | split(' ') | count }}"
    normalized: "{{ capture.message | replace(' response', '') }}"
    status_code: "{{ capture.status_text | to_int }}"
    payload: "{{ capture.user | to_string }}"
```

`first` and `last` expect arrays. `count` works on arrays, objects, and strings. `join(...)` joins array items after converting each item to its string form. `split(...)` and `replace(..., ...)` operate on strings. `to_int` parses integer strings, and `to_string` stringifies any captured value.

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

Or reset the default jar between named tests in a file so IDE subset runs and flaky suites never see session state from a prior test. Setup and teardown still share the file-level jar. Named jars (multi-user scenarios) are untouched.

```yaml
cookies: "per-test"
```

The `--cookie-jar-per-test` CLI flag forces per-test isolation regardless of the file's declared mode (except when the file sets `cookies: "off"`, which always wins).

## Auth

Tarn supports first-class `bearer` and `basic` auth helpers, while keeping explicit `Authorization` headers as the escape hatch:

```yaml
request:
  auth:
    bearer: "{{ env.token }}"
  headers:
    X-API-Key: "{{ env.api_key }}"
```

Basic auth:

```yaml
request:
  auth:
    basic:
      username: "{{ env.username }}"
      password: "{{ env.password }}"
```

You can also set `defaults.auth` once per file. If `headers.Authorization` is already present, Tarn leaves it unchanged.

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

### Persist Cookie Jars

Use `tarn run --cookie-jar .tarn-cookies.json` to preload jars from disk and write back the updated state after the run. The file stores named jars too, so multi-user sessions can survive across runs.

`--cookie-jar` currently works only with sequential execution. Combine it with `--parallel` only after jar sharing becomes deterministic.

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

## Form URL-Encoding

Send `application/x-www-form-urlencoded` payloads with `form:`:

```yaml
steps:
  - name: Login form
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      form:
        email: "user@example.com"
        password: "{{ env.password }}"
    assert:
      status: 200
```

Tarn URL-encodes the fields and auto-sets `Content-Type: application/x-www-form-urlencoded` unless you override it explicitly.

> Note: `form` cannot be combined with `body`, `graphql`, or `multipart` on the same step.

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

> Note: `multipart` cannot be combined with `body`, `form`, or `graphql` on the same step.

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

Includes also support lightweight parametrization and deep overrides for reusable step packs:

```yaml
steps:
  - include: ./shared/user-pack.tarn.yaml
    with:
      tenant: "acme"
      user_id: 42
    override:
      request:
        headers:
          X-Tenant: "acme"
```

Inside the included file, use `{{ params.tenant }}` and `{{ params.user_id }}` placeholders.

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
- `response.body` &mdash; response body as Lua table (auto-parsed from JSON)
- `captures` &mdash; read/write captures table
- `assert(condition, message)` &mdash; assertion (collected, not thrown)
- `json.decode(string)` &mdash; parse a JSON string into a Lua table
- `json.encode(value)` &mdash; serialize a Lua value to a JSON string

## CLI Reference

```
tarn run [PATH] [OPTIONS]          Run test files
tarn bench <PATH> [OPTIONS]        Benchmark a step
tarn validate [PATH] [--format]    Validate YAML (--format human|json)
tarn fmt [PATH] [--check]          Normalize Tarn YAML
tarn list                          List all tests
tarn import-hurl <PATH>            Convert common-case Hurl files to Tarn
tarn init                          Scaffold a new project
tarn update                        Update to the latest version
tarn update --check                Check for updates without installing
tarn completions <SHELL>           Generate shell completions
```

### `tarn run` Options

| Flag | Description |
|------|-------------|
| `--format <FORMAT>` | Repeatable. Supports `human`, `json`, `junit`, `tap`, `html`, `curl`, `curl-all`, or `FORMAT=PATH` |
| `--json-mode <MODE>` | For JSON outputs: `verbose` (default) or `compact` |
| `--tag <TAGS>` | Filter by tag (comma-separated, AND logic) |
| `--select <FILE[::TEST[::STEP]]>` | Narrow execution to specific files, tests, or steps (repeatable; ANDs with `--tag`) |
| `--var <KEY=VALUE>` | Override env variables (repeatable) |
| `--env <NAME>` | Load `tarn.env.{name}.yaml` |
| `-v, --verbose` | Print full request/response for every step |
| `--only-failed` | Show only failed tests and steps (summary counts stay accurate) |
| `--no-progress` | Disable streaming progress output; print the final report in one batch |
| `--ndjson` | Stream machine-readable NDJSON events to stdout (for editor integrations, MCP, structured CI) |
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
tarn run --format json --json-mode compact     # smaller JSON for automation loops
tarn run --format html                          # HTML dashboard
tarn run --format curl                          # failed requests as curl
tarn run --format curl-all=reports/replay.sh    # full suite replay script
tarn run --format human --format json=reports/run.json --format junit=reports/junit.xml
tarn run --format human,json=reports/run.json   # comma-separated also works
tarn run --watch                                # re-run on changes
tarn run --parallel --jobs 4                    # parallel execution
tarn run -v                                     # verbose
tarn run --dry-run                              # preview only
tarn run --only-failed                          # hide passing tests, show failures only
tarn run --no-progress                          # disable streaming, batch dump at end
tarn run --only-failed --format json            # CI-friendly: only failed items in JSON
tarn run --select tests/users.tarn.yaml::login   # run just the "login" test in one file
tarn run --select tests/users.tarn.yaml::login::2   # run just step index 2 of login
tarn run --select "a.tarn.yaml::login" --select "b.tarn.yaml::checkout"  # union across files
tarn fmt tests/                                  # rewrite a directory in place
tarn fmt tests/auth.tarn.yaml --check            # CI-style formatting check
```

### Structured Validation (`tarn validate --format json`)

`tarn validate --format json` emits a machine-readable report so editors and CI can surface parse errors inline. The schema:

```json
{
  "files": [
    {
      "file": "tests/users.tarn.yaml",
      "valid": false,
      "errors": [
        { "message": "found unexpected end of stream ...", "line": 14, "column": 7 }
      ]
    }
  ]
}
```

- `line` and `column` are populated for YAML syntax errors (derived from `serde_yaml`'s error location).
- Parser semantic errors (unknown fields, shape mismatches) surface `message` only when the underlying error does not carry a location.
- Exit code is `0` when every file is valid, `2` otherwise. The human format (`--format human`, the default) is unchanged.

### Environment Discovery (`tarn env --json`)

`tarn env --json` prints the project's named environments in a stable schema so editors can populate pickers and previews:

```json
{
  "project_root": "/path/to/project",
  "default_env_file": "tarn.env.yaml",
  "environments": [
    {
      "name": "staging",
      "source_file": "tarn.env.staging.yaml",
      "vars": {
        "base_url": "https://staging.example.com",
        "api_token": "***"
      }
    }
  ]
}
```

Inline `vars` from `tarn.config.yaml` are redacted when the key matches `redaction.env` (case-insensitive), so `tarn env --json` never prints literal secrets. Environments are sorted alphabetically by name.

### Streaming Progress

By default `tarn run` streams per-test output as each test finishes instead of dumping everything at the end. The behaviour adapts to how stdout is used:

- **Sequential (default)** &mdash; each test is printed the moment it completes. You see progress live as the suite runs.
- **Parallel (`--parallel`)** &mdash; each file is printed atomically when it completes, so output from concurrently running files never interleaves.
- **Stdout is `human`** &mdash; streaming writes directly to stdout and the final emit prints only the summary line (no duplication).
- **Stdout is a structured format** (`json`, `junit`, `tap`, `html`, `curl`) &mdash; progress streams to stderr so stdout stays pure and parseable.

Pass `--no-progress` to disable streaming entirely and restore the old "batch at end" behaviour (useful for CI logs that already capture per-line timestamps).

### NDJSON Streaming (`--ndjson`)

`tarn run --ndjson` streams machine-readable events to stdout, one JSON object per line. Designed for editor integrations (live Test Explorer updates), MCP clients, and CI pipelines that want structured progress without post-processing the final report.

Event types, in order:

- `file_started` &mdash; a test file has begun running
- `step_finished` &mdash; one step finished (with `phase: "setup" | "test" | "teardown"`). On failure, also carries `failure_category`, `error_code`, and `assertion_failures[]`
- `test_finished` &mdash; a named test finished, with per-step counts
- `file_finished` &mdash; a file finished, with its own summary
- `done` &mdash; emitted once at the very end, carrying the aggregated summary for the whole run

`--ndjson` composes with file-bound `--format` targets, so you can stream live progress **and** write a final report at the same time:

```bash
# Stream NDJSON to stdout, final JSON report to disk
tarn run --ndjson --format json=reports/run.json | jq '.event'

# Pure NDJSON (default human output is silently dropped on stdout)
tarn run --ndjson
```

`--ndjson` collides with any other structured format writing to stdout (e.g. `--format json`). Route the other format to a file, or pick one of the two streams.

In parallel mode (`--parallel`), each file's event stream is emitted atomically on `file_finished` so events from concurrently running files never interleave.

`--only-failed` works with both streaming and batch modes: passing tests and steps are omitted everywhere, but the final summary still reports total passed/failed counts.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | All tests passed |
| `1` | One or more tests failed |
| `2` | Configuration/parse error |
| `3` | Runtime error (network, timeout, script) |

## Output Formats

You can emit multiple formats in one run. Keep at most one bare non-HTML format for stdout and send the rest to files:

```bash
tarn run \
  --format human \
  --format json=reports/run.json \
  --format junit=reports/junit.xml \
  --format html=reports/run.html \
  --format curl=reports/failures.sh \
  --format curl-all=reports/replay.sh
```

### JSON (`--format json`)

Structured JSON with versioned schema. Key design:
- `schema_version: 1` for forward compatibility
- Full request/response included **only for failed steps**
- `failure_category` on failures: `assertion_failed`, `connection_error`, `timeout`, `parse_error`, `capture_error`, `unresolved_template`
- Stable `error_code` and `remediation_hints` are included on failed steps for automation-friendly diagnostics
- `response_status` and `response_summary` on all executed steps (passed and failed) &mdash; AI agents can see what a passed step returned
- `captures_set` on steps listing which capture variables were set; `captures` map on test groups showing all resolved values
- `--json-mode compact` keeps the same top-level schema but drops passed assertion details and truncates response bodies to ~200 chars
- Sensitive headers are redacted by default and can be customized per file with top-level `redaction:`
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
      "captures": { "user_id": "usr_123", "token": "abc" },
      "steps": [{
        "name": "Create user",
        "status": "PASSED",
        "response_status": 201,
        "response_summary": "201 Created: Object{3 keys}",
        "captures_set": ["user_id"]
      }, {
        "name": "Update user",
        "status": "FAILED",
        "response_status": 400,
        "response_summary": "400 Bad Request: name required",
        "failure_category": "assertion_failed",
        "error_code": "assertion_mismatch",
        "remediation_hints": ["..."],
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

### Curl (`--format curl`, `--format curl-all`)

`curl` exports only failed executed requests. `curl-all` exports every executed request in run order, including setup and teardown.

```bash
tarn run --format human --format curl=reports/failures.sh
tarn run --format curl-all=reports/replay.sh
```

Also supports: **Human** (colored terminal), **JUnit XML**, **TAP**, **HTML** (self-contained dashboard).

Example:

```yaml
redaction:
  headers:
    - authorization
    - x-session-token
  env:
    - api_token
  captures:
    - session_token
  replacement: "[redacted]"
```

## Performance Testing

Reuses your existing test files for benchmarking.

```bash
tarn bench tests/health.tarn.yaml -n 1000 -c 50
tarn bench tests/health.tarn.yaml -n 500 -c 25 --ramp-up 5s
tarn bench tests/health.tarn.yaml --format json     # for CI thresholds
tarn bench tests/health.tarn.yaml --format csv --export json=reports/bench.json
tarn bench tests/health.tarn.yaml --fail-under-rps 200 --fail-above-p95-ms 80
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

The simplest approach is a project-level `.mcp.json` in the repo root (works with Claude Code and other MCP-compatible tools):

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

Alternatively, add to your Claude Code project settings (`.claude/settings.json`):

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
| `tarn_fix_plan` | Analyze a Tarn JSON report and return prioritized next actions |

The MCP server lets your AI agent write `.tarn.yaml` tests, execute them, parse structured results, and iterate &mdash; all without leaving the editor.

Typical agent loop:

1. `tarn_list` to discover tests and steps
2. `tarn_validate` after generating YAML
3. `tarn_run` to get structured failures
4. `tarn_fix_plan` to turn the latest report into machine-friendly next steps
5. inspect `failure_category`, `error_code`, `assertions.failures`, and optional `request`/`response`
6. patch the test or application code
7. rerun until summary status is `PASSED`

See [docs/MCP_WORKFLOW.md](./docs/MCP_WORKFLOW.md), [docs/AI_WORKFLOW_DEMO.md](./docs/AI_WORKFLOW_DEMO.md), and [docs/CONFORMANCE.md](./docs/CONFORMANCE.md).

## Claude Code Plugin

Tarn is available as a Claude Code plugin. The plugin bundles the MCP server and the Tarn skill, so installing it gives your agent structured API testing capabilities out of the box.

### Install the plugin

Tarn is published as a Claude Code **marketplace** (a registry that can contain multiple plugins). Installation is two steps:

```bash
# 1. Register the marketplace
claude plugin marketplace add NazarKalytiuk/hive

# 2. Install the Tarn plugin from it
claude plugin install tarn@tarn
```

After installing, Claude Code can write, run, and debug `.tarn.yaml` tests directly via the bundled MCP server and skill.

### Manual setup

If you prefer manual configuration, add the MCP server to a project-level `.mcp.json` in the repo root:

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

This is equivalent to configuring the MCP server in `.claude/settings.json` but is portable across editors and tools that support MCP.

### Plugin metadata

The plugin configuration lives in `.claude-plugin/`:

- **`plugin.json`** &mdash; name, version, description, author, and repository URL
- **`marketplace.json`** &mdash; marketplace listing with owner info and plugin registry

## Claude Code Skill

The `skills/tarn-api-testing/` directory contains a Claude Code skill that teaches AI agents how to write, run, debug, and iterate on Tarn tests. The skill is automatically loaded when an agent encounters API testing tasks.

**What the skill provides:**

- Core workflow (write &rarr; validate &rarr; run &rarr; inspect &rarr; fix &rarr; rerun)
- Complete command reference with all CLI flags
- Test file format with minimal and full-featured examples
- Environment variable resolution chain
- Capture formats (JSONPath, headers, cookies, URL, status, body)
- Assertion operator quick reference
- JSON output schema and failure category taxonomy
- Diagnosis loop for structured failure triage
- MCP integration setup for Claude Code, Cursor, and Windsurf

**Reference docs** in `skills/tarn-api-testing/references/`:

| File | Contents |
|------|----------|
| `yaml-format.md` | Complete `.tarn.yaml` schema with all properties |
| `assertion-reference.md` | Every assertion operator with examples |
| `json-output.md` | Structured JSON report schema and diagnosis algorithm |
| `mcp-integration.md` | MCP server setup and tool reference |

The skill triggers on keywords like "API test", "tarn", ".tarn.yaml", "test this endpoint", "smoke test", and "integration test for API".

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

## Intentional Gaps

Tarn does not aim for full Hurl parity. The main intentionally unclosed gaps are:

- XPath / HTML assertions and captures
- full Hurl-style filter DSL
- exotic auth and libcurl-specific transport features
- OpenAPI-first generation workflows

## GitHub Action

```yaml
- uses: NazarKalytiuk/hive@v1
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
defaults:
  connect_timeout: 1000
  follow_redirects: true
redaction:
  headers: ["authorization", "cookie"]
environments:
  staging:
    env_file: "env/staging.yaml"
    vars:
      base_url: "https://staging.example.com"
```

Behavior:
- `test_dir` sets the default discovery directory for `tarn run`, `tarn validate`, and `tarn list`
- `env_file` changes the root env file name; Tarn also checks `.{name}` and `.local` variants
- `defaults` acts as project-wide request policy for headers/auth/timeouts/retries/redirects/delay
- `redaction` provides a project-wide default report sanitization policy
- `environments` makes named `--env` profiles first-class and powers `tarn env`
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
# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/hive/main/schemas/v1/testfile.json
name: My test
steps: ...
```

The schema is bundled at `schemas/v1/testfile.json` in the repository.
The structured report schema is bundled at `schemas/v1/report.json`.

## VS Code Extension

An editor extension now lives in [`editors/vscode`](./editors/vscode). It provides:

- Tarn file association for `*.tarn.yaml` and `*.tarn.yml`
- schema defaults for Tarn test files and `tarn-report.json`
- snippets for test skeletons, polling, multipart, GraphQL, form requests, and includes

Local install is documented in [`editors/vscode/README.md`](./editors/vscode/README.md).

## Shell Completions

```bash
tarn completions bash > /etc/bash_completion.d/tarn
tarn completions zsh > ~/.zsh/completions/_tarn
tarn completions fish > ~/.config/fish/completions/tarn.fish
```

## Development

```bash
git clone https://github.com/NazarKalytiuk/hive.git
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
