# Tarn — AI Agent Integration Guide

Tarn is a CLI API testing tool. Tests are YAML files (`.tarn.yaml`), results are structured JSON.

## Quick Reference

```bash
tarn run                          # run all tests in tests/
tarn run tests/users.tarn.yaml    # run specific file
tarn run --format json            # structured JSON output (for parsing)
tarn validate                     # check syntax without running
tarn list                         # list all tests
```

## Writing a Test

```yaml
name: User API
steps:
  - name: Create user
    request:
      method: POST
      url: "{{ env.base_url }}/users"
      headers:
        Content-Type: "application/json"
      body:
        name: "Jane Doe"
        email: "jane@example.com"
    capture:
      user_id: "$.id"                  # JSONPath (type-preserving)
      # session:                       # header capture with regex
      #   header: "set-cookie"
      #   regex: "session=([^;]+)"
    assert:
      status: 201                      # also: "2xx", { in: [200,201] }, { gte: 200, lt: 300 }
      body:
        "$.name": "Jane Doe"
        "$.id": { type: string, not_empty: true }

  - name: Get user
    request:
      method: GET
      url: "{{ env.base_url }}/users/{{ capture.user_id }}"
    assert:
      status: 200
      body:
        "$.name": "Jane Doe"
```

## Assertion Operators

| Operator | Example | Description |
|----------|---------|-------------|
| (literal) | `"$.name": "Alice"` | Exact match |
| `eq` | `{ eq: "Alice" }` | Explicit equality |
| `not_eq` | `{ not_eq: "Bob" }` | Not equal |
| `type` | `{ type: string }` | Type check (string/number/boolean/array/object/null) |
| `contains` | `{ contains: "sub" }` | Substring or array element |
| `not_contains` | `{ not_contains: "x" }` | Inverse of contains |
| `starts_with` | `{ starts_with: "usr_" }` | String prefix |
| `ends_with` | `{ ends_with: ".com" }` | String suffix |
| `matches` | `{ matches: "^[a-z]+$" }` | Regex match |
| `not_empty` | `{ not_empty: true }` | Non-empty string/array/object |
| `exists` | `{ exists: true }` | Field exists |
| `length` | `{ length: 5 }` | Exact length |
| `gt`/`gte`/`lt`/`lte` | `{ gt: 0 }` | Numeric comparison |

Multiple operators combine with AND: `{ type: string, contains: "@", not_empty: true }`

## Built-in Functions

- `{{ $uuid }}` — UUID v4
- `{{ $timestamp }}` — Unix timestamp
- `{{ $now_iso }}` — ISO 8601 datetime
- `{{ $random_hex(8) }}` — Random hex string
- `{{ $random_int(1, 100) }}` — Random integer

## Environment Variables

Priority (highest to lowest): `--var` flag > shell env > `tarn.env.local.yaml` > `tarn.env.{name}.yaml` > `tarn.env.yaml` > inline `env:` block.

## JSON Output Structure

`tarn run --format json` returns:

```json
{
  "schema_version": 1,
  "summary": {
    "status": "PASSED|FAILED"
  },
  "files": [{
    "file": "tests/users.tarn.yaml",
    "tests": [{
      "steps": [{
        "name": "Create user",
        "status": "PASSED|FAILED",
        "failure_category": "assertion_failed",
        "assertions": {
          "failures": [{
            "assertion": "status",
            "expected": "201",
            "actual": "400",
            "message": "..."
          }]
        },
        "request": { "method": "POST", "url": "..." },
        "response": { "status": 400, "body": {...} }
      }]
    }]
  }]
}

```

Request/response are only included for failed steps.
`request` is available for failed executed steps; `response` is omitted when no HTTP response exists (for example, connection failure).
Schema files live in `schemas/v1/testfile.json` and `schemas/v1/report.json`.

## MCP Server

Tarn includes an MCP server (`tarn-mcp`) for direct integration with Claude Code, Cursor, and Windsurf.

### Claude Code Setup

Add to your project's `.claude/settings.json`:

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

### Available MCP Tools

- **`tarn_run`** — Run tests, returns structured JSON results
- **`tarn_validate`** — Validate YAML syntax without executing
- **`tarn_list`** — List all available tests and their steps

### Recommended Agent Loop

1. Call `tarn_validate` after generating or editing YAML.
2. Call `tarn_run` and inspect `failure_category` first.
3. If `response` exists, fix assertions/captures against the real payload.
4. If `request.url` still contains `{{ ... }}`, fix env/capture interpolation before retrying.
5. Rerun until summary status is `PASSED`.

### Common Failure Categories

- `assertion_failed` — request succeeded, assertion mismatch
- `connection_error` — DNS/connect/TLS failure before a usable response
- `timeout` — request exceeded allowed time
- `parse_error` — invalid YAML/config/JSONPath surface
- `capture_error` — assertions passed, capture extraction failed

### Non-JSON Responses

- Plain text / HTML responses are preserved as JSON strings in structured output.
- To assert a whole string response body, use `body: { "$": "expected text" }`.

## Cookies

Automatic cookie handling is on by default. Set-Cookie headers are captured and Cookie headers are sent on subsequent requests. Disable per file with `cookies: "off"`.

## Multipart Upload

```yaml
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

## Includes

Reuse shared steps: `- include: ./shared/auth.tarn.yaml` in setup/teardown/steps.

## File Extension

Tests use `.tarn.yaml`. Schema validation: add `# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/hive/main/schemas/v1/testfile.json` at the top of test files for IDE autocompletion.
