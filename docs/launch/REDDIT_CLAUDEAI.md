# r/ClaudeAI — beta launch draft

**Purpose:** beta run before HN. Test how the agent-first framing lands, gather feedback, iterate copy for Tuesday HN submit.

**Submit URL:** https://www.reddit.com/r/ClaudeAI/submit

**Post type:** Text (self) post. Pick the "Text" tab when the submit page loads. Do **not** use the "Link" type — we want the body to drive the discussion.

**Flair:** `Showcase` (this is the rule-7 category — "Showcase your project").

**Rule-7 compliance** (r/ClaudeAI requires all of these for showcase posts; this draft already covers them):

1. Clear that the project was built **with Claude Code** AND **for Claude** by you — covered in paragraphs 1, 3, and 4 below.
2. Clear description of **what was built**, **how Claude helped**, and **what it does** — covered.
3. **Free to try and explicitly says so** — "Free, MIT-licensed" appears twice (top + next to repo link).
4. **Promotional language minimal** — no superlatives, names limitations, defers to Hurl/Bruno where they're stronger.
5. **No referral links** — only direct GitHub + docs site links.

---

## Title (113 chars — Reddit limit is 300, fits comfortably)

```
I built Tarn — API tests Claude Code can write, run, and debug end-to-end (open source, MCP server included)
```

---

## Body — CORRECTED VERSION (paste as-is into the body field, Markdown mode on)

> **Note:** the first version of this body that got published had two formatting issues — the "What it does NOT try to be" paragraph got absorbed into the previous bullet list, and the Docs bullet absorbed "Looking for honest feedback:" so questions 2 and 3 disappeared. The version below is restructured to be Reddit-markdown-bulletproof: every paragraph after a list is preceded by a blank line AND uses bold-as-heading rather than starting with a list item.

Hey r/ClaudeAI — I just open-sourced **Tarn**, a CLI-first API testing tool I built collaboratively with Claude Code over the last few months. It's free, MIT-licensed, and a single static binary you install with `curl | sh` or `cargo install`.

**Why it exists.** Claude Code, Cursor, opencode, and Windsurf couldn't reliably write and fix API tests for me when failures came back as human-readable stderr. The agent could correctly guess "the URL is wrong" for a 404, but it would also guess that for `connection refused` and for `assertion failed because the body shape changed`, because in prose all three look similar. The fix needed structure, not narration.

**What Tarn actually does.** Every test failure is structured JSON with a stable `failure_category` (`assertion_failed`, `connection_error`, `timeout`, `capture_error`, `parse_error`), an `error_code`, the offending request and response, and a list of remediation hints. When the agent runs `tarn run --format json` and a test breaks, it gets structured data it can branch on — not stderr it has to guess from.

**Built for Claude (and other MCP-capable agents).** A companion `tarn-mcp` server exposes the run loop as MCP tools — `tarn_run`, `tarn_validate`, `tarn_fix_plan`, `tarn_inspect`, `tarn_rerun_failed`, and a few more. Claude Code drives the whole write-run-debug loop through tools instead of shelling out and parsing output.

**How Claude Code actually helped me build it.** The failure-taxonomy schema, the assertion DSL design, and the MCP tool surface were all iterated through paired Claude Code sessions — I'd describe the goal, watch Claude propose a few options, push back on the parts that felt off, and converge. The `.tarn.yaml` format itself was refined based on what Claude Code could actually generate without errors. There's a `CLAUDE.md` in the repo with the project rules I built up over the process — including the things I had to teach Claude *not* to do (never suppress clippy warnings with `#[allow(...)]`, always verify install commands from the production URL, never reference URLs without checking they exist, etc.). It's basically the project's institutional memory written for an LLM.

Tests are `.tarn.yaml` files. A minimal one:

    name: Health check
    steps:
      - name: GET /health
        request:
          method: GET
          url: "{{ env.base_url }}/health"
        assert:
          status: 200

**Some intentional choices:**

- YAML, not a custom DSL — the model already knows the syntax, no foot-gun training.
- Failures-first CLI: `tarn failures` collapses cascade-skips, `tarn rerun --failed` replays only broken (file, test) pairs, `tarn diff prev last` buckets failure fingerprints into new / fixed / persistent.
- Per-step failure taxonomy, not a single "test failed" boolean.
- Full request/response embedded only on failure (success runs stay cheap).

&nbsp;

**What it does NOT try to be.** Full Hurl parity (no XPath, no full filter DSL), no OpenAPI-first generation, no GUI. Hurl is still better for handwritten HTTP specs. Bruno has a wider ecosystem and a GUI. Tarn's focus is the agent loop.

**Free, MIT, open source.** Repo: https://github.com/NazarKalytiuk/tarn — Docs: https://nazarkalytiuk.github.io/tarn/

&nbsp;

**Looking for honest feedback on three specific things:**

1. Does the MCP tool surface (`tarn_run` / `tarn_validate` / `tarn_fix_plan` / `tarn_inspect` / `tarn_rerun_failed`) feel right — too granular, not granular enough, or about right?
2. Is `tarn_fix_plan` the right level of abstraction? Or should the tool just emit raw failures and let the model plan its own fix?
3. Anyone here actually drive an API testing loop from Claude Code today? What does the missing piece look like for you?

---

### What changed from v1 → v2

- `**What it does NOT try to be**` is its own paragraph (was being eaten by the previous bullet list).
- "Free, MIT" is its own bold paragraph with both URLs inline (was a 2-bullet list that absorbed the "Looking for feedback" line).
- All three feedback questions are present (v1 had only the third, somewhere between paste and post).
- Two `&nbsp;` HTML-entity spacers force visible paragraph breaks Reddit otherwise collapses around list-to-paragraph transitions.

---

## Pre-submit micro-checklist (5 minutes)

- [ ] Open the submit page: https://www.reddit.com/r/ClaudeAI/submit
- [ ] Click **Text** tab (NOT Link)
- [ ] Paste title (above)
- [ ] Switch body editor to **Markdown** mode (look for "Markdown Mode" toggle, usually bottom-right of the editor)
- [ ] Paste body (above)
- [ ] Pick a flair if the sub requires one (`Project` / `Showcase` / `Open Source` is the safe pick)
- [ ] **Preview** before submitting — verify code blocks render, em-dashes (—) survive, bullet list renders
- [ ] Verify GitHub link and Docs link are clickable in the preview
- [ ] Hit Post

## After you post

1. **Stay online for ~2 hours.** Reddit's algorithm watches early engagement closely. Reply to every comment in the first hour, even short ones. "Good question — the reason is X" beats "thanks!".
2. **Don't argue if someone says "yet another API tester".** Concede the parts that are similar; redirect to what's actually different (structured failure taxonomy + MCP). The phrase "Tarn's bet is the agent loop specifically" usually defuses it.
3. **If someone asks about Hurl/Bruno/Postman**, use the lines from `docs/LAUNCH_PLAYBOOK.md` → Comparison Talking Points. Verbatim is fine.
4. **Track what people ask for that you don't have.** OpenAPI-first generation, XPath assertions, etc. — that becomes raw input for Tuesday's HN post and the roadmap.

## What you're testing in this beta

- Does the **agent-first framing** land or feel buzzwordy? → Watch ratio of "this is interesting" vs. "another AI thing" comments.
- Is the **`tarn_fix_plan` abstraction** the right one? → Q1+Q2 in the post are designed to surface this. Read replies carefully.
- Are there **concrete missing features** that would block adoption? → Q3 surfaces this.

If r/ClaudeAI gives you positive directional signal (>20 upvotes, >5 substantive comments in 2 hours), HN on Tuesday with the same framing is safer. If it's tepid, we recalibrate the HN title towards CLI-first/engineering-first lead before submitting.

## Do NOT cross-post yet

- r/rust, r/LocalLLaMA, r/AI_Agents — save for after HN. Each is a separate beachhead.
- HN itself — Tuesday 06:00 PT as planned.

A Sunday Reddit beta in r/ClaudeAI does not burn any of these, because Reddit/HN don't share spam fingerprints and ClaudeAI's audience overlap with HN/r/rust is small.
