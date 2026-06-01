# Agent-loop example

A self-contained, reproducible example of the Tarn write → run → read failure →
fix-plan → fix loop. [`api.tarn.yaml`](./api.tarn.yaml) hits the public
[JSONPlaceholder](https://jsonplaceholder.typicode.com) API, so it runs with
**zero local setup** — no server, no secrets.

This is the example referenced by the per-agent integration guides:

- [`editors/codex/README.md`](../../editors/codex/README.md)
- [`editors/opencode/README.md`](../../editors/opencode/README.md)
- [`editors/pi/README.md`](../../editors/pi/README.md)

## Run it

```bash
tarn validate examples/agent-loop/api.tarn.yaml
tarn run      examples/agent-loop/api.tarn.yaml --format llm
```

Expected: `tarn: PASS 4/4 steps, 0 failed, 1 file`.

## The loop

The same loop works whether the agent drives Tarn through the `tarn-mcp` tools
(Codex, opencode, Claude Code, Cursor, Windsurf) or the `tarn` CLI directly
(pi, or any agent with a shell tool):

| Step | MCP tool | CLI equivalent |
|------|----------|----------------|
| 1. Write / edit `.tarn.yaml` | — | — |
| 2. Check syntax | `tarn_validate` | `tarn validate <path>` |
| 3. Run | `tarn_run` | `tarn run <path> --format llm` |
| 4. Read root-cause failures | `tarn_last_failures` | `tarn failures` |
| 5. Inspect one failing step | `tarn_inspect` | `tarn inspect last FILE::TEST::STEP` |
| 6. Get a remediation plan | `tarn_fix_plan` | (use the failures + inspect output) |
| 7. Patch YAML or app, rerun the failing subset | `tarn_rerun_failed` | `tarn rerun --failed` |
| 8. Confirm | `tarn_run` | `tarn diff prev last` |

## See the loop fire

Make the suite fail on purpose, then recover:

1. In `api.tarn.yaml`, change `length_gte: 1` to `length_gte: 1000`.
2. `tarn run examples/agent-loop/api.tarn.yaml --format llm` → step 1 fails with
   `failure_category: assertion_failed`.
3. `tarn failures` → one root-cause group, expected vs actual side by side.
4. Revert the value and `tarn rerun --failed` → green again.

An agent does the same thing through the MCP tools without ever scraping stdout.
