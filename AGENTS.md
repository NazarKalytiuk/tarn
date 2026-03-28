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
      user_id: "$.id"
    assert:
      status: 201
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
  "status": "PASSED|FAILED",
  "files": [{
    "file": "tests/users.tarn.yaml",
    "tests": [{
      "steps": [{
        "name": "Create user",
        "status": "PASSED|FAILED",
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

## File Extension

Tests use `.tarn.yaml`. Schema validation: add `# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/hive/main/schemas/v1/testfile.json` at the top of test files for IDE autocompletion.
