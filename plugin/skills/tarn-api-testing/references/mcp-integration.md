# Tarn MCP Integration Reference

Tarn ships with `tarn-mcp`, an MCP (Model Context Protocol) server that lets AI agents run, validate, and inspect API tests directly.

## Setup

### Project-level `.mcp.json` (preferred)

The portable way to wire `tarn-mcp` into any MCP-compatible tool is a single `.mcp.json` at the repo root:

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

Claude Code, Cursor, Windsurf, and any other MCP-compatible client pick this file up out of the box — no editor-specific settings file needed. Commit it to the repo and every contributor gets Tarn tooling on clone.

If you prefer an editor-specific file, the per-editor alternatives below still work.

### Claude Code

Add to `.claude/settings.json` in the project root:

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

### opencode

Opencode does **not** read `.mcp.json`. Use its native config — `opencode.jsonc` at the repo root:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "tarn": {
      "type": "local",
      "command": ["tarn-mcp"],
      "enabled": true
    }
  }
}
```

Note the different schema shape: opencode uses `mcp` (not `mcpServers`), `type: "local"` for stdio, and a `command` array that includes both the executable and its args. See `editors/opencode/README.md` for the full setup (MCP + LSP + skill).

### Cursor

Add to `.cursor/mcp.json`:

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

### Windsurf

Add to `.windsurf/mcp.json`:

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

**Prerequisite:** `tarn-mcp` binary must be in `$PATH`. Build with `cargo build --release -p tarn-mcp`.

## Resolving the working directory (`cwd`)

Every tool accepts an optional `cwd` absolute path that drives config discovery (`tarn.config.yaml`, `tarn.env*.yaml`), include-path resolution, and multipart file paths. The server uses it in this priority order:

1. `cwd` argument on the tool call, if the caller passed one. Must be absolute and point at an existing directory; a missing `tarn.config.yaml` there now fails fast with the resolved path in the error — no silent fallback.
2. The workspace root the MCP client announced during `initialize` (`workspaceFolders[0].uri`, `rootUri`, or `rootPath`).
3. The server process's current directory.

This fixes the long-standing "MCP can't resolve `{{ env.base_url }}`" issue — if you previously had to shell out to `tarn` from the terminal because the MCP tool picked up the wrong working dir, pass `cwd` explicitly (or rely on the client's workspace hint) and it will work.

## Available Tools

All tools accept `cwd` in addition to the parameters below.

### tarn_run

Run API tests and return structured JSON results.

**Parameters:**
- `path` (optional) — path to a specific `.tarn.yaml` file or directory; relative paths resolve against `cwd`, defaults to `"tests"`
- `env` (optional) — environment name (maps to `tarn.env.{name}.yaml`)
- `tag` (optional) — run only tests matching this tag
- `vars` (optional) — object of key/value string overrides
- `cwd` (optional) — absolute working directory (see above)
- `allow_exec` (optional, boolean) — authorize shell `command:` steps (NAZ-464) to spawn child processes. Off by default; without it, every `command:` step is reported as `failure_category: skipped_by_policy`. Mirrors the CLI `--allow-exec` flag.

**Returns:** Full JSON report matching `schemas/v1/report.json`.

### tarn_validate

Validate YAML syntax without executing HTTP requests.

**Parameters:**
- `path` (optional) — path to validate; relative against `cwd`
- `cwd` (optional) — absolute working directory

**Returns:** Validation result with any parse errors and their locations.

### tarn_list

List all available test files, test groups, and steps.

**Parameters:**
- `path` (optional) — list steps for a specific file or directory
- `cwd` (optional) — absolute working directory

**Returns:** Structured listing of tests and steps.

### tarn_fix_plan

Generate a fix plan for failed test results.

**Parameters:**
- `report` — JSON report from a failed `tarn_run`
- `cwd` (optional) — absolute working directory used for any follow-up `tarn_run` invocations

**Returns:** Structured remediation plan with suggested fixes per failed step.

The same `tarn::fix_plan` library that backs this MCP tool also powers the `tarn-lsp` quick-fix code action (`CodeActionKind::QUICKFIX`). Agents working inside Claude Code (with the `tarn-lsp` plugin installed) or opencode (with the `lsp.tarn` entry in `opencode.jsonc`) will see the same remediation surfaced as a one-click code action on the diagnostic — the MCP tool is the report-driven path, the LSP quick-fix is the diagnostic-driven path, and the engine is the same. See `docs/MCP_WORKFLOW.md` for the cross-reference.

## Recommended Agent Loop

```
1. After generating or editing .tarn.yaml → call tarn_validate
2. If validation passes → call tarn_run
3. Read summary.status
4. If FAILED:
   a. Find failed steps → read failure_category
   b. Read assertions.failures[] for expected vs actual
   c. If request.url contains unresolved {{ }} → fix env/capture
   d. Optionally call tarn_fix_plan for structured remediation
   e. Fix YAML or application code
   f. Go to step 1
5. If PASSED → done
```

## When to Use MCP vs CLI

**Use MCP (tarn_run tool)** when:
- Working inside Claude Code, opencode, Cursor, or Windsurf
- You want structured JSON returned directly to the agent context
- Iterating on test failures in an agent loop

**Use CLI directly** when:
- Running in CI/CD pipelines
- You need specific output formats (junit, tap, html)
- Running benchmarks or using advanced CLI flags
- Human is reading the output directly

## Key Fields to Focus On

When processing `tarn_run` results, prioritize these fields:

1. `summary.status` — overall pass/fail
2. `files[].tests[].steps[].failure_category` — why a step failed
3. `files[].tests[].steps[].assertions.failures[]` — what exactly was wrong
4. `files[].tests[].steps[].request.url` — check for unresolved templates
5. `files[].tests[].steps[].response.body` — actual server response
6. `files[].tests[].steps[].remediation_hints` — suggested fixes

## JSONPath Evaluation via tarn-lsp

`tarn-lsp` exposes a `workspace/executeCommand` handler for command `tarn.evaluateJsonpath`. It evaluates a JSONPath expression against either an inline response payload or a recorded step response resolved via the sidecar convention. An agent can use it to verify an `assert.body.*` expression against a real response before committing it to the YAML, without re-parsing the `.tarn.yaml` or round-tripping through `tarn run`.

Two call shapes are supported.

Inline response:

```json
{
  "path": "$.data[0].id",
  "response": { "data": [{ "id": "u_123", "name": "Jane" }] }
}
```

Recorded step reference:

```json
{
  "path": "$.data[0].id",
  "step": {
    "file": "tests/users.tarn.yaml",
    "test": "create-user",
    "step": 2
  }
}
```

Both shapes return:

```json
{ "matches": ["u_123"] }
```

See `docs/TARN_LSP.md` for the full handler spec.

## Recorded Response Sidecar

Several `tarn-lsp` features (hover JSONPath evaluation, `scaffold assert from response`, step-reference `tarn.evaluateJsonpath` calls) read the last recorded response for a given step from a sidecar directory next to the test file:

```
tests/users.tarn.yaml.last-run/<test-slug>/<step-slug>.response.json
```

- Each step's last response is stored independently, keyed by slugified test name and step name.
- The server only reads this directory. Writing it is the client's responsibility — typically the editor integration that runs `tarn run` captures the response per step and drops it here.
- Agents that want hover-driven JSONPath results and the scaffold-assert code action to light up should make sure the client is populating the sidecar after each run.
