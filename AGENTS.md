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
- **`tarn_fix_plan`** — Takes a failed `tarn_run` report and returns a structured remediation plan (prioritized next actions per failed step). Backed by the shared `tarn::fix_plan` library that also powers `tarn-lsp`'s quick-fix code action.

### Recommended Agent Loop

1. Call `tarn_validate` after generating or editing YAML.
2. Call `tarn_run` and inspect `failure_category` first.
3. Optionally call `tarn_fix_plan` with the failed report for structured remediation (prioritized next actions per failed step — useful when multiple steps failed and you need to order the fixes).
4. If `response` exists, fix assertions/captures against the real payload.
5. If `request.url` still contains `{{ ... }}`, fix env/capture interpolation before retrying.
6. Rerun until summary status is `PASSED`.

### Common Failure Categories

- `assertion_failed` — request succeeded, assertion mismatch
- `connection_error` — DNS/connect/TLS failure before a usable response
- `timeout` — request exceeded allowed time
- `parse_error` — invalid YAML/config/JSONPath surface
- `capture_error` — assertions passed, capture extraction failed

### Non-JSON Responses

- Plain text / HTML responses are preserved as JSON strings in structured output.
- To assert a whole string response body, use `body: { "$": "expected text" }`.

## Editor Integration (tarn-lsp)

`tarn-lsp` is a standalone LSP 3.17 stdio binary shipped alongside `tarn` and `tarn-mcp` in the same releases (every Tarn release since 0.6.0). It is designed for agents running inside an editor client — Claude Code, VS Code, Neovim, Helix, Zed — so language features come from the same Tarn core the CLI uses, with no feature drift.

### Agent-Relevant Capabilities

- Diagnostics via `textDocument/publishDiagnostics` on open / save / debounced change — YAML and schema errors with precise ranges.
- Hover with inline JSONPath evaluation against the step's last recorded response.
- Schema-aware completion: env keys, captures in scope, builtins, nested schema keys.
- Go-to-definition / references / rename for env keys (workspace-wide) and captures (per-test).
- Code lens above every test and step emitting stable `tarn.runTest` / `tarn.runStep` commands. The server emits the commands; the client is responsible for dispatching `tarn run --select FILE::TEST::STEP_INDEX`.
- Whole-document formatting, shared with the `tarn fmt` CLI via `tarn::format::format_document`.
- Code actions: **extract env var**, **capture this field**, **scaffold assert from recorded response**.
- **Quick fix** (`CodeActionKind::QUICKFIX`) backed by the same `tarn::fix_plan` engine the MCP `tarn_fix_plan` tool uses.
- `workspace/executeCommand` handler for `tarn.evaluateJsonpath` — agents can evaluate a JSONPath against either an inline response payload or a step reference without re-parsing the `.tarn.yaml`.

### Recorded-Response Sidecar Convention

`tarn-lsp` reads last responses from `<file>.tarn.yaml.last-run/<test-slug>/<step-slug>.response.json`. Writes are the client's responsibility — if an agent wants hover-JSONPath and scaffold-assert to light up, drop recorded responses at that path.

### Claude Code Install

```
/plugin marketplace add /absolute/path/to/hive-api-test/editors/claude-code
/plugin install tarn-lsp@tarn-lsp --scope project
/reload-plugins
```

Compound-extension caveat: the plugin claims all `.yaml` / `.yml` files, so install `--scope project` in Tarn-focused repos only. See `editors/claude-code/tarn-lsp-plugin/README.md` for the full spec.

### Other Clients

For Neovim / Helix / Zed / any LSP 3.17 client, see `docs/TARN_LSP.md` for client-specific setup.

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
