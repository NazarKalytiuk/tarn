# Tarn MCP Workflow

Tarn's MCP server exists to keep the agent on a structured tool surface instead of asking it to parse shell output.

## Setup

The recommended setup is a project-level `.mcp.json` in the repo root:

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

This is portable across Claude Code, Cursor, Windsurf, and any other client that reads `.mcp.json`. **opencode is the exception** — it uses its own `opencode.jsonc` schema (`mcp.tarn` with `type: "local"` and a `command` array). See `plugin/skills/tarn-api-testing/references/mcp-integration.md` for the opencode snippet and the other editor-specific alternatives (`.claude/settings.json`, `.cursor/mcp.json`, `.windsurf/mcp.json`).

## Agent Skill

The `plugin/skills/tarn-api-testing/` directory is the canonical home for the Tarn agent skill — structured knowledge about Tarn's workflow, commands, test file format, assertions, captures, and failure diagnosis. Two agents pick it up today:

- **Claude Code** loads it from the `tarn` plugin (the plugin ships `plugin/skills/tarn-api-testing/` as its skill directory).
- **opencode** loads it from `.opencode/skills/tarn-api-testing/`, which this repo provides as a relative symlink to the canonical path above — no content duplication.

## Available Tools

- `tarn_list`
- `tarn_validate`
- `tarn_run`
- `tarn_fix_plan`

### `cwd` parameter

Every tool accepts an optional `cwd` parameter (absolute path string). When set, it is used as the project root for:

- discovery of `tarn.config.yaml`, `tarn.env.yaml`, `tarn.env.{name}.yaml`, and `tarn.env.local.yaml`
- resolving any relative `path`, `include:` directive, and multipart file reference
- resolving CLI-style named environments configured under `environments:` in `tarn.config.yaml`

Defaulting rules (when `cwd` is omitted):

1. the workspace root the MCP client announced during `initialize` — either `workspaceFolders[0].uri` or the legacy `rootUri` / `rootPath` fields (with `file://` stripped), **is used when available**;
2. otherwise the MCP server process's current directory is used.

Failure modes:

- A relative `cwd` is rejected with `Parameter cwd must be an absolute path`.
- A non-existent or non-directory `cwd` is rejected with `cwd does not exist` / `cwd is not a directory`.
- When `cwd` is explicitly set but the directory does **not** contain `tarn.config.yaml`, the tool fails fast with an error that names the full resolved path. There is no silent fallback to the process cwd — the assumption is that an agent that set `cwd` meant it.

When `cwd` is not set, a missing `tarn.config.yaml` is still tolerated (the server walks up for an ancestor project or runs with library defaults) so legacy single-file flows keep working.

## Recommended Loop

1. Call `tarn_list` to discover tests and steps.
2. Call `tarn_validate` after generating or editing `.tarn.yaml`.
3. Call `tarn_run` and inspect the structured report.
4. Branch first on `failure_category` and `error_code`.
5. Use `tarn_fix_plan` when you want prioritized next actions from the latest report.
6. Edit the test or the application code.
7. Rerun until `summary.status` is `PASSED`.

### Failures-first loop over MCP

The same failures-first discipline documented for the CLI applies when driving Tarn through MCP. The MCP tool surface (`tarn_run` + `tarn_fix_plan`) hands back the same per-run artifacts (`summary.json`, `failures.json`, `report.json`) under `.tarn/runs/<run_id>/`, so after an MCP-driven `tarn_run` you should:

1. Read `failures.json` (or call `tarn_fix_plan` which already operates on the failing subset) — **do not parse the full `report.json` unless the failures-level artifacts are insufficient.**
2. For one specific failing step, drop to the CLI equivalent (`tarn inspect last FILE::TEST::STEP --format json`) or read the step's record straight out of `report.json` — there is no dedicated MCP tool for drill-down yet, so the CLI is the canonical path.
3. Patch the YAML (via the MCP client's file edit path) or the application.
4. Replay only the failing subset with `tarn rerun --failed` at the CLI (again, no MCP wrapper yet — the MCP surface is intentionally minimal).
5. Call `tarn diff prev last` at the CLI to confirm the root cause is `fixed` with no `new` failures.

See [`docs/AI_WORKFLOW_DEMO.md`](./AI_WORKFLOW_DEMO.md) and the Tarn skill's **Failures-First Loop** section ([`plugin/skills/tarn-api-testing/SKILL.md`](../plugin/skills/tarn-api-testing/SKILL.md)) for the canonical CLI sequence.

> **Editor consumers:** `tarn_fix_plan` is backed by the same `tarn::fix_plan` library surface that powers `tarn-lsp`'s `CodeActionKind::QUICKFIX` **Apply fix** code action (NAZ-305, L3.4). The MCP tool uses the report-driven path for prioritised advice; the LSP uses the diagnostic-driven path for structured edits that clients apply with one click. See [`docs/TARN_LSP.md`](./TARN_LSP.md#apply-fix-quickfix--new-in-l34) for the LSP-side contract.

## Shell `command:` steps

Tarn 0.12 introduces shell `command:` steps (NAZ-464). Files containing these steps run by default as no-ops over MCP — the runner reports `failure_category: skipped_by_policy` and never spawns the child process. Pass `"allow_exec": true` in the `tarn_run` / `tarn_run_agent` / `tarn_rerun_failed` tool params to authorize execution. The same gate is exposed as `tarn run --allow-exec` on the CLI and `allow_exec: true` in `tarn.config.yaml` for trusted, project-owned repositories.

Treat `allow_exec` as a per-run consent: an agent should never set it implicitly. Surface the `command:` block to the human, confirm trust, and pass it explicitly. The child process gets a minimal env (`PATH`, `HOME`/`USERPROFILE`, `TMPDIR`/`TEMP`/`TMP`); other parent-process variables must be allowlisted via `pass_env:` in the YAML. Tarn's own `{{ env.x }}` chain never leaks into the child without an explicit interpolation in `command.run`.

Failures land in two new categories:
- `skipped_by_policy` — benign skip; the step has `passed: true` and the run stays green if nothing else failed.
- `command_failed` — non-zero exit, signal kill, or a non-optional `stdout_regex` miss; treated like a regular step failure for exit-code purposes.

## Fields That Matter Most

Focus on these first:

- `failure_category`
- `error_code`
- `remediation_hints`
- `assertions.failures`
- optional failed-step `request`
- optional failed-step `response`

That ordering is deliberate. It keeps the agent from patching assertions before it understands whether the failure is parse, connection, timeout, capture, or a plain mismatch.

## Why MCP Instead of Shelling Out

- no stdout scraping
- fewer quoting and path-resolution mistakes
- smaller tool surface for the model
- direct structured results instead of ad hoc parsing

## When Plain CLI Is Still Fine

Use the CLI directly when:

- you are in CI
- you do not want MCP setup overhead
- you want report files such as `json`, `html`, `junit`, or `curl`

The equivalent fallback is:

```bash
tarn run --format json --json-mode compact
```

The report contract is the same idea either way: machine-readable, versioned, and intentionally stable.
