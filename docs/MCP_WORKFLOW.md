# Tarn MCP Workflow

## Claude Code / Cursor Loop

1. The agent calls `tarn_list` to see available tests and steps.
2. The agent calls `tarn_validate` after generating or editing `.tarn.yaml`.
3. The agent calls `tarn_run` to execute tests and receive structured JSON.
4. The agent inspects `failure_category`, failed assertions, and optional request/response payloads.
5. The agent edits the test or the application code.
6. The agent reruns `tarn_run` until the summary status is `PASSED`.

## Why MCP Instead of Shelling Out

- No stdout scraping.
- The agent gets structured data directly.
- The tool surface is smaller: `tarn_run`, `tarn_validate`, `tarn_list`.
- Editors can expose these tools without teaching the model shell quoting.

## When Plain CLI Is Still Fine

Use `tarn run --format json` directly when:

- you do not want to configure MCP;
- you are in CI;
- you want to pipe results into another tool manually.

The JSON contract is the same idea either way: failures are machine-readable, stable, and versioned.
