# Tarn — CLI API Testing Tool (Rust)

## Overview

Tarn is a CLI-first API testing tool written in Rust. Tests are defined in YAML files optimized for both human readability and LLM generation/analysis. The tool is designed for an AI-assisted workflow: an LLM writes tests, runs `tarn run`, parses structured JSON output, and iterates.

## Core Principles

1. **YAML-first**: every test is a `.tarn.yaml` file — no code, no custom DSL
2. **LLM-friendly**: format is simple enough that an LLM generates valid tests on the first try; CLI output is structured JSON that an LLM can parse and reason about
3. **Single binary**: `cargo build --release` produces one binary with zero runtime dependencies
4. **Incremental complexity**: simple tests are simple (3 lines for a GET + status check), complex tests are possible (chaining, cross-field assertions, setup/teardown)

## Tech Stack

| Purpose | Crate | Why |
|---------|-------|-----|
| CLI framework | `clap` (derive) | Industry standard, great help text |
| YAML parsing | `serde` + `serde_yaml` | Deserialize directly into Rust structs |
| HTTP client | `reqwest` (blocking initially, async later) | Most popular Rust HTTP client |
| JSONPath | `serde_json_path` | RFC 9535 compliant |
| JSON Schema | `jsonschema` | For `tarn validate` command |
| Regex | `regex` | Standard |
| Duration | `std::time` | For response time assertions |
| Colored output | `colored` | TTY-aware |
| Diff | `similar` | For expected-vs-actual diffs |
| Template engine | `handlebars` or manual `{{ }}` interpolation | For variable substitution |

## YAML Test File Format

### File naming convention

```
tests/
  health.tarn.yaml          # single endpoint smoke test
  users/
    crud.tarn.yaml           # full CRUD lifecycle
    validation.tarn.yaml     # negative/error tests
    pagination.tarn.yaml     # list/filter/sort tests
  auth/
    login.tarn.yaml
    permissions.tarn.yaml
tarn.config.yaml             # project-level config (optional)
```

### Minimal test (simplest possible)

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

### Full format specification

```yaml
# yaml-language-server: $schema=https://tarn-api.dev/schemas/v1/testfile.json
version: "1"

name: "User CRUD Operations"
description: "Tests complete user lifecycle: create, read, update, delete"
tags: [crud, users, smoke]

# Environment variables — values come from:
# 1. Inline defaults (below)
# 2. env files (tarn.env.yaml, tarn.env.staging.yaml)
# 3. CLI --var key=value (highest priority)
# 4. Shell environment variables via ${VAR_NAME}
env:
  base_url: "http://localhost:3000/api/v1"
  admin_email: "admin@example.com"
  admin_password: "${ADMIN_PASSWORD}"  # from shell env

# Default headers/settings applied to every request in this file
defaults:
  headers:
    Content-Type: "application/json"
    Accept: "application/json"
  timeout: 5000  # ms

# Setup runs once before all tests in this file
# Uses the same step format as tests
setup:
  - name: Authenticate
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "{{ env.admin_email }}"
        password: "{{ env.admin_password }}"
    capture:
      auth_token: "$.token"
      # capture uses JSONPath expressions
      # captured values are available as {{ capture.auth_token }}
    assert:
      status: 200

# Teardown runs once after all tests (even if tests fail)
teardown:
  - name: Clean up test data
    request:
      method: POST
      url: "{{ env.base_url }}/test/cleanup"
      headers:
        Authorization: "Bearer {{ capture.auth_token }}"

# Tests — the main content
# Each test is independent (but steps within a test are sequential)
# Tests within a file run sequentially by default
tests:
  create_and_verify_user:
    description: "Create a user, then verify it exists"
    tags: [smoke]
    steps:
      - name: Create user
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
          body:
            name: "Jane Doe"
            email: "jane.doe.{{ $random_hex(6) }}@example.com"
            role: "editor"
            tags: ["content", "marketing"]
        capture:
          user_id: "$.id"
          created_at: "$.createdAt"
        assert:
          # --- Status ---
          status: 201

          # --- Response time ---
          duration: "< 500ms"

          # --- Headers ---
          headers:
            content-type: contains "application/json"
            x-request-id: matches "^[a-f0-9-]{36}$"

          # --- Body assertions via JSONPath ---
          body:
            # Simple equality
            "$.name": "Jane Doe"
            "$.role": "editor"
            "$.deletedAt": null

            # Type checks
            "$.id": { type: string, matches: "^usr_[a-z0-9]+$" }
            "$.email": { type: string, contains: "@example.com" }
            "$.tags": { type: array, length: 2, contains: "content" }
            "$.createdAt": { type: string, not_empty: true }

            # Numeric checks
            # "$.age": { type: number, gte: 0, lte: 150 }

            # Existence / absence
            "$.id": { exists: true }
            "$.deletedAt": { exists: true }  # field exists but value is null
            "$.internal_field": { exists: false }  # field should not be present

      - name: Verify user via GET
        request:
          method: GET
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        assert:
          status: 200
          body:
            "$.id": "{{ capture.user_id }}"
            "$.name": "Jane Doe"
            "$.createdAt": "{{ capture.created_at }}"

      - name: Update user role
        request:
          method: PATCH
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
          body:
            role: "admin"
        assert:
          status: 200
          body:
            "$.role": "admin"

      - name: Delete user
        request:
          method: DELETE
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        assert:
          status: 204

      - name: Confirm deletion
        request:
          method: GET
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        assert:
          status: 404
          body:
            "$.error": "not_found"

  invalid_inputs:
    description: "Verify proper error handling for bad requests"
    steps:
      - name: Missing required field
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
          body:
            name: "No Email User"
        assert:
          status: 422
          body:
            "$.error": "validation_error"
            "$.details": { type: array, length_gte: 1 }
            "$.details[0].field": "email"
            "$.details[0].message": { contains: "required" }

      - name: Invalid email format
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
          body:
            name: "Bad Email"
            email: "not-an-email"
        assert:
          status: 422
          body:
            "$.details[0].field": "email"

      - name: Unauthorized without token
        request:
          method: GET
          url: "{{ env.base_url }}/users"
        assert:
          status: 401

  pagination:
    description: "Verify list endpoint pagination and sorting"
    steps:
      - name: First page
        request:
          method: GET
          url: "{{ env.base_url }}/users?page=1&limit=5&sort=name:asc"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        capture:
          total: "$.meta.totalCount"
          first_name: "$.data[0].name"
        assert:
          status: 200
          body:
            "$.data": { type: array, length_lte: 5 }
            "$.meta.page": 1
            "$.meta.limit": 5
            "$.meta.totalCount": { type: number, gte: 0 }

      - name: Second page returns different results
        request:
          method: GET
          url: "{{ env.base_url }}/users?page=2&limit=5&sort=name:asc"
          headers:
            Authorization: "Bearer {{ capture.auth_token }}"
        assert:
          status: 200
          body:
            "$.data[0].name": { not_eq: "{{ capture.first_name }}" }
```

### Assertion Reference

All assertions available in `assert.body` via JSONPath:

```yaml
# Equality
"$.field": "exact value"           # string equality
"$.field": 42                      # number equality
"$.field": true                    # boolean equality
"$.field": null                    # null check
"$.field": { eq: "value" }         # explicit equality

# Comparison (numbers)
"$.field": { gt: 10 }             # greater than
"$.field": { gte: 10 }            # greater than or equal
"$.field": { lt: 100 }            # less than
"$.field": { lte: 100 }           # less than or equal

# String assertions
"$.field": { contains: "substr" }
"$.field": { starts_with: "prefix" }
"$.field": { ends_with: "suffix" }
"$.field": { matches: "^regex$" }  # regex match
"$.field": { not_empty: true }
"$.field": { length: 10 }         # exact string length
"$.field": { length_gt: 5 }

# Inequality
"$.field": { not_eq: "bad" }
"$.field": { not_contains: "error" }

# Type checks
"$.field": { type: string }
"$.field": { type: number }
"$.field": { type: boolean }
"$.field": { type: array }
"$.field": { type: object }
"$.field": { type: "null" }

# Array assertions
"$.array": { length: 5 }
"$.array": { length_gt: 0 }
"$.array": { length_gte: 1 }
"$.array": { length_lte: 100 }
"$.array": { contains: "value" }     # array contains element
"$.array": { not_contains: "bad" }

# Existence
"$.field": { exists: true }         # field is present (value can be null)
"$.field": { exists: false }        # field is absent

# Combining assertions (AND logic — all must pass)
"$.field": { type: string, not_empty: true, matches: "^usr_" }
```

### Variable Interpolation

```yaml
# Available in url, headers, body, assert values:
"{{ env.var_name }}"          # from env block or env files
"{{ capture.var_name }}"      # from capture blocks (previous steps)
"{{ $random_hex(8) }}"        # built-in function: random hex string
"{{ $random_int(1, 100) }}"   # built-in function: random integer
"{{ $timestamp }}"            # built-in: current unix timestamp
"{{ $uuid }}"                 # built-in: UUID v4
"{{ $now_iso }}"              # built-in: ISO 8601 datetime
```

### Environment Files

```yaml
# tarn.env.yaml (default, committed to git — no secrets)
base_url: "http://localhost:3000/api/v1"
admin_email: "admin@example.com"

# tarn.env.staging.yaml (per-environment overrides)
base_url: "https://staging-api.example.com/v1"

# tarn.env.local.yaml (gitignored — secrets go here)
admin_password: "s3cret"
```

Priority (highest wins): CLI `--var` > shell env `${VAR}` > `tarn.env.local.yaml` > `tarn.env.{name}.yaml` > `tarn.env.yaml` > inline `env:` block in test file.

### Project Config (optional)

```yaml
# tarn.config.yaml
test_dir: "tests"           # where to find .tarn.yaml files
env_file: "tarn.env.yaml"   # default env file
timeout: 10000              # global default timeout (ms)
retries: 0                  # retry failed requests
parallel: false             # run test files in parallel (future)
```

## CLI Interface

### Commands

```bash
# Run all tests
tarn run

# Run specific file
tarn run tests/users/crud.tarn.yaml

# Run tests matching tags
tarn run --tag smoke
tarn run --tag "crud,users"  # AND logic

# Run with specific environment
tarn run --env staging
# loads tarn.env.staging.yaml

# Override variables
tarn run --var base_url=http://localhost:8080 --var admin_password=test

# Output formats
tarn run --format human    # default: colored terminal output
tarn run --format json     # structured JSON (for LLM consumption)
tarn run --format junit    # JUnit XML (for CI/CD)
tarn run --format tap      # TAP (Test Anything Protocol)

# Validate test files without running
tarn validate
tarn validate tests/users/crud.tarn.yaml

# List all tests (dry run)
tarn list
tarn list --tag smoke

# Init new project
tarn init

# Version
tarn --version
```

### Exit Codes

```
0 — all tests passed
1 — one or more tests failed
2 — configuration/parse error (invalid YAML, missing env, etc.)
3 — runtime error (network failure, timeout, etc.)
```

## CLI Output Formats

### Human-readable (default)

```
 TARN  Running tests/users/crud.tarn.yaml

 ● User CRUD Operations

   Setup
   ✓ Authenticate (145ms)

   create_and_verify_user — Create a user, then verify it exists
   ✓ Create user (234ms)
   ✓ Verify user via GET (89ms)
   ✓ Update user role (112ms)
   ✓ Delete user (67ms)
   ✓ Confirm deletion (54ms)

   invalid_inputs — Verify proper error handling for bad requests
   ✓ Missing required field (43ms)
   ✓ Invalid email format (38ms)
   ✗ Unauthorized without token (51ms)
     ├─ status: expected 401, got 403
     └─ body $.error: expected "unauthorized", got "forbidden"

   Teardown
   ✓ Clean up test data (78ms)

 Results: 7 passed, 1 failed (911ms)
```

### JSON (--format json)

```json
{
  "version": "1",
  "timestamp": "2026-03-28T12:00:00Z",
  "duration_ms": 911,
  "files": [
    {
      "file": "tests/users/crud.tarn.yaml",
      "name": "User CRUD Operations",
      "status": "FAILED",
      "duration_ms": 911,
      "summary": { "total": 8, "passed": 7, "failed": 1 },
      "setup": [
        {
          "name": "Authenticate",
          "status": "PASSED",
          "duration_ms": 145
        }
      ],
      "tests": {
        "create_and_verify_user": {
          "description": "Create a user, then verify it exists",
          "status": "PASSED",
          "duration_ms": 556,
          "steps": [
            {
              "name": "Create user",
              "status": "PASSED",
              "duration_ms": 234,
              "assertions": { "total": 12, "passed": 12, "failed": 0 }
            }
          ]
        },
        "invalid_inputs": {
          "description": "Verify proper error handling for bad requests",
          "status": "FAILED",
          "duration_ms": 132,
          "steps": [
            {
              "name": "Missing required field",
              "status": "PASSED",
              "duration_ms": 43,
              "assertions": { "total": 4, "passed": 4, "failed": 0 }
            },
            {
              "name": "Unauthorized without token",
              "status": "FAILED",
              "duration_ms": 51,
              "request": {
                "method": "GET",
                "url": "http://localhost:3000/api/v1/users",
                "headers": { "Content-Type": "application/json" }
              },
              "response": {
                "status": 403,
                "headers": { "content-type": "application/json" },
                "body": { "error": "forbidden", "message": "Access denied" }
              },
              "assertions": {
                "total": 2,
                "passed": 0,
                "failed": 2,
                "failures": [
                  {
                    "assertion": "status",
                    "expected": 401,
                    "actual": 403,
                    "message": "Expected HTTP status 401, got 403"
                  },
                  {
                    "assertion": "body $.error",
                    "expected": "unauthorized",
                    "actual": "forbidden",
                    "message": "JSONPath $.error: expected \"unauthorized\", got \"forbidden\""
                  }
                ]
              }
            }
          ]
        }
      },
      "teardown": [
        {
          "name": "Clean up test data",
          "status": "PASSED",
          "duration_ms": 78
        }
      ]
    }
  ],
  "summary": {
    "files": 1,
    "tests": 3,
    "steps": { "total": 8, "passed": 7, "failed": 1 },
    "status": "FAILED"
  }
}
```

Key design decisions for JSON output:
- Full request/response is included ONLY for failed steps (keeps output compact)
- Every failed assertion has `expected`, `actual`, and `message` fields
- Secrets in headers are redacted to `***`
- The `summary` at the top level gives instant pass/fail for CI exit decisions

## Project Structure

```
tarn/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point (clap)
│   ├── lib.rs               # Public API for library usage
│   ├── config.rs            # tarn.config.yaml parsing
│   ├── model.rs             # Rust structs for YAML test files (serde)
│   ├── parser.rs            # Load and validate .tarn.yaml files
│   ├── env.rs               # Environment variable resolution
│   ├── interpolation.rs     # {{ variable }} template engine
│   ├── runner.rs            # Orchestrator: setup → tests → teardown
│   ├── http.rs              # HTTP request execution (reqwest)
│   ├── capture.rs           # JSONPath extraction from responses
│   ├── assert/
│   │   ├── mod.rs           # Assertion dispatcher
│   │   ├── status.rs        # Status code assertions
│   │   ├── headers.rs       # Header assertions
│   │   ├── body.rs          # Body/JSONPath assertions
│   │   ├── duration.rs      # Response time assertions
│   │   └── types.rs         # AssertionResult, Expected/Actual types
│   ├── report/
│   │   ├── mod.rs           # Reporter trait
│   │   ├── human.rs         # Colored terminal output
│   │   ├── json.rs          # Structured JSON output
│   │   ├── junit.rs         # JUnit XML
│   │   └── tap.rs           # TAP format
│   └── builtin.rs           # Built-in functions ($uuid, $random_hex, etc.)
├── schemas/
│   └── v1/
│       └── testfile.json    # JSON Schema for .tarn.yaml files
├── examples/
│   ├── minimal.tarn.yaml
│   ├── crud.tarn.yaml
│   ├── auth.tarn.yaml
│   └── tarn.env.yaml
└── tests/
    ├── parser_test.rs
    ├── assert_test.rs
    ├── interpolation_test.rs
    └── integration_test.rs
```

## Implementation Plan (ordered phases)

### Phase 1: Foundation (skeleton that runs)

1. Set up Cargo.toml with all dependencies
2. Define `model.rs` — all Rust structs matching YAML format above, with `#[derive(Deserialize)]`
3. Implement `parser.rs` — load YAML file → `TestFile` struct
4. Implement `main.rs` — `clap` CLI with `run` and `validate` subcommands
5. Implement basic `http.rs` — send a request, return status + headers + body + duration
6. Implement `assert/status.rs` — just `status: 200` works
7. Implement `report/human.rs` — minimal colored pass/fail output
8. **Milestone**: `tarn run minimal.tarn.yaml` sends GET, checks status, prints result

### Phase 2: Core assertions

1. Implement `assert/body.rs` — all JSONPath assertions (eq, contains, matches, type, length, gt/lt, exists)
2. Implement `assert/headers.rs` — header assertions (contains, matches, eq)
3. Implement `assert/duration.rs` — `duration: "< 500ms"`
4. Implement assertion combining (multiple checks on same JSONPath = AND logic)
5. **Milestone**: full assertion suite works against real API

### Phase 3: Variables and chaining

1. Implement `capture.rs` — extract values from response via JSONPath
2. Implement `env.rs` — load env files with priority chain
3. Implement `interpolation.rs` — resolve `{{ env.x }}`, `{{ capture.x }}` in all string fields
4. Implement `builtin.rs` — `$uuid`, `$random_hex`, `$timestamp`, etc.
5. Implement `--var key=value` CLI override
6. **Milestone**: multi-step tests with chaining work (create → get → verify)

### Phase 4: Setup/teardown and orchestration

1. Implement `runner.rs` — full lifecycle: load file → resolve env → run setup → run tests → run teardown
2. Implement defaults merging (file-level headers applied to each request)
3. Implement `--tag` filtering
4. Implement test file discovery (scan `test_dir` for `*.tarn.yaml`)
5. Implement `config.rs` — optional `tarn.config.yaml`
6. **Milestone**: `tarn run` discovers and runs all test files with setup/teardown

### Phase 5: Reporters

1. Implement `report/json.rs` — full structured JSON output as specified above
2. Implement `report/junit.rs` — JUnit XML for CI/CD
3. Implement `report/tap.rs` — TAP format
4. Implement `--format` flag
5. Improve `report/human.rs` — request/response diff for failures, colored JSONPath
6. **Milestone**: all 4 output formats work, `--format json` output parseable by LLM

### Phase 6: Polish and DX

1. Implement `tarn init` — scaffold project structure
2. Implement `tarn list` — dry run listing
3. Implement `tarn validate` — check YAML against JSON Schema without running
4. Create JSON Schema file for `.tarn.yaml` format
5. Write example test files for `examples/`
6. Error messages: every error must be actionable (file:line:column where possible)
7. **Milestone**: tool is ready for daily use

### Future (post-MVP)

- Parallel test file execution (`--parallel`)
- Retry logic (`retries: 2` per step)
- Before/after hooks per test (not just file-level setup/teardown)
- `tarn generate --from-openapi spec.yaml` — scaffold tests from OpenAPI
- `tarn explain tests/crud.tarn.yaml` — render test steps as plain English
- Lua scripting for custom assertions (escape hatch for the 20%)
- GraphQL support
- gRPC support
- WebSocket support
- Watch mode (`tarn run --watch`)
- HTML report