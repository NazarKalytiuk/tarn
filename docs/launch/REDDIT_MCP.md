# r/mcp — beta launch draft (v2)

**Purpose:** cross-post after r/ClaudeAI silent-fail. r/mcp is a smaller (~105k), more focused sub where the `tarn-mcp` server is on-topic by definition. Different title and body from r/ClaudeAI to avoid spam-detector flags.

**Submit URL:** https://www.reddit.com/r/mcp/submit

**Post type:** Text (self) post. r/mcp accepts both, but Text gives us body for the tool list and example.

**Flair:** r/mcp uses `Servers` / `News` / `Resource` / `Question` / `Showcase` style flairs (varies). Pick `Servers` if it exists (this IS a server), otherwise `Showcase` or `Resource`.

**Format principles** (lessons from r/ClaudeAI v1 silent-fail):

- No three-question feedback list — reads as homework
- One open question max, only if natural to the post topic
- Visual rhythm: short paragraphs alternating with short lists, no wall-of-bold
- Lead with the **tool list** since that's what r/mcp readers scan for
- Always blank line between paragraph and list (Reddit eats single newlines)

---

## Title (110 chars — Reddit allows 300)

```
Tarn-mcp: an MCP server for API testing — agents write, run, and fix .yaml tests via tools (open source, Rust)
```

---

## Body (paste as-is, Markdown mode on)

Hey r/mcp — sharing **tarn-mcp**, an MCP server I built so AI agents (Claude Code, Cursor, opencode, Windsurf) can drive the full API-testing loop through structured tools instead of shelling out and parsing stderr.

The tools it exposes:

- `tarn_run` — execute a `.tarn.yaml` test or directory, return structured JSON
- `tarn_validate` — syntax/config check before running
- `tarn_fix_plan` — consume a failure report, emit structured fix suggestions
- `tarn_inspect` — drill into a specific failure (`file::test::step`) without parsing the full report
- `tarn_rerun_failed` — replay only failing (file, test) pairs from the last run
- `tarn_list`, `tarn_diff`, `tarn_scaffold`, `tarn_pack_context`, and a few more

The underlying CLI (`tarn`) is a single static Rust binary. Tests are `.tarn.yaml` files. Output is structured JSON with stable failure categories (`assertion_failed`, `connection_error`, `timeout`, `capture_error`, `parse_error`), error codes, and remediation hints. So when the agent runs a test and it breaks, it gets data it can branch on — not stderr it has to guess from.

A minimal test file:

    name: Health check
    steps:
      - name: GET /health
        request:
          method: GET
          url: "{{ env.base_url }}/health"
        assert:
          status: 200

The `tarn_fix_plan` tool is the design choice I'm least sure about. Right now it consumes a failure report and emits structured fix suggestions — the alternative would be to just emit raw failure data and let the model plan its own fix. Open to opinions on that tradeoff if anyone here has built similar tools.

Free, MIT, single binary. `curl | sh` or `cargo install` to install both `tarn` and `tarn-mcp`.

- Repo: https://github.com/NazarKalytiuk/tarn
- MCP setup docs: https://nazarkalytiuk.github.io/tarn/mcp.html
- Full docs: https://nazarkalytiuk.github.io/tarn/

---

## Pre-submit micro-checklist

- [ ] Open submit page: https://www.reddit.com/r/mcp/submit
- [ ] Click **Text** tab (NOT Link)
- [ ] Paste title (above)
- [ ] Switch body editor to **Markdown** mode (toggle is usually in the editor toolbar — must be ON or formatting will break like in r/ClaudeAI)
- [ ] Paste body (above)
- [ ] Pick flair (`Servers` or `Showcase`)
- [ ] **Preview** before submitting — verify:
  - Tool list renders as bullets, not as one paragraph
  - YAML example renders as a code block (4-space indent should preserve it)
  - Two paragraph breaks between body sections (no absorption like r/ClaudeAI v1)
- [ ] Submit

## After you post

- Stay around for ~1 hour. r/mcp is smaller, conversation moves slower. Reply to every comment.
- If anyone says "what's the difference from `tarn run --format json` if I just shell out?" — your line: "MCP gives the agent typed tool params + structured returns. With `tarn run --format json` the agent has to know to pass `--format json`, parse stdout, handle non-zero exit codes. With `tarn-mcp` it's `tools/call` and a typed response."
- If anyone asks about the JSON failure schema — point to `schemas/v1/report.json` in the repo.
- Track every "wish it had X" comment — that's input for the HN post tomorrow.

## What we're checking with this beta

- Does the **MCP-first framing** (instead of agent-first or AI-first) land better?
- Does the **tool list at the top** drive engagement vs. the wall-of-bold-paragraphs in r/ClaudeAI v1?
- Specifically: does anyone bite on the **`tarn_fix_plan` design tradeoff** at the end? That's a single embedded question, not a quiz.

If r/mcp gives positive signal (>10 upvotes, >2 substantive comments in 1 hour), the HN post tomorrow leans more MCP-first too. If it's flat, we already know that issue isn't framing — it's account/visibility, and HN starts fresh.

## Why r/mcp not r/AI_Agents

- r/AI_Agents is bigger (350k vs 105k) but broader — agents-as-a-concept, not MCP specifically
- r/mcp readers are *exactly* the people who care about a typed tool surface for API testing
- Smaller sub = AutoMod usually less aggressive on new accounts
- Topical match means flair is obvious (`Servers`)
