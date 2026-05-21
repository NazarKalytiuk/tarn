# dev.to launch article — draft v1

**Target platform:** dev.to (cross-post to Hashnode, personal blog if available)
**Length:** ~1800 words (sweet spot for dev.to: 1500–2500)
**Tags (max 4):** `rust`, `ai`, `testing`, `mcp`
**Canonical URL:** if you have a personal blog, set canonical there and cross-post here. Otherwise dev.to is fine as primary.

**Cover image:** dev.to looks naked without one. Quick options:
- Simple gradient banner with "Tarn" + tagline in monospace font
- A snippet of failure JSON rendered as code on dark background
- Or just skip — dev.to picks a reasonable default. Cover image is the single biggest CTR lever though, worth 10 minutes in Figma/Canva.

**Publish settings:**
- Set `published: false` on first save → preview the rendering before going live
- Once happy, flip to `published: true`
- After publish: share to Twitter/X, link from r/mcp post comment, link from GitHub README "as featured on dev.to" badge later

---

## Title (pick one)

- **A.** `I taught Claude Code to write, run, and fix my API tests` *(Recommended — curiosity-driven, dev.to friendly, hooks the agent-loop angle without buzzword overload)*
- B. `An API testing tool built specifically for AI agent loops`
- C. `How structured JSON failures let AI agents debug API tests on their own`

---

## Article body — paste from `## START` to `## END` into the dev.to editor

## START

I was working on a small API for an internal tool. I wanted my coding agent — Claude Code, in this case, but Cursor or opencode would have done — to take the boring part off my plate: write a happy-path test for each endpoint I added, run it, and fix it when something broke.

The "write" part was great. Claude generated reasonable tests on the first shot. The "run" part was fine.

The "fix" part is where it fell apart.

Here's a typical failure as Jest spits it out:

```
AssertionError: expected 200 to equal 404
    at Object.<anonymous> (/path/to/test.js:14:23)
    at processImmediate (node:internal/timers:483:21)
```

The agent would read this, guess "the URL is wrong" and patch the test. Sometimes that worked. But often the actual problem was different — the server wasn't even running, or the response shape had drifted from `{"uuid": "..."}` to `{"request": {"uuid": "..."}}`, or the body was JSON-shaped fine but the JSONPath in my assertion was wrong, or the timeout was tripping before the response came back.

All of those look identical in stderr. They all collapse into the prose phrase "200 != 404." The agent had no way to tell them apart, so it kept guessing the same fix-shape (URL change) and being right maybe 30% of the time.

I tried a few stopgaps — adding richer error messages, parsing the stack trace heuristically — and they all got me to maybe 50% first-fix correctness. Not enough. Below the bar where you can leave the agent alone and trust the loop to converge.

## The unlock isn't better stderr — it's structure

The model doesn't need a more eloquent error message. It needs *data*.

If the test runner returned, on failure, a JSON shape like this:

```json
{
  "failure_category": "assertion_failed",
  "error_code": "TARN-A-STATUS-MISMATCH",
  "expected": 200,
  "actual": 404,
  "request": { ... },
  "response": { ... },
  "hints": [
    "Status 404 often means the URL or HTTP method is wrong.",
    "Check the endpoint exists and that the path matches your route registration."
  ]
}
```

Then the agent's branching logic becomes obvious:

- `failure_category == "connection_error"` → server isn't reachable. Don't touch the test, check `base_url`, kill-and-restart the dev server.
- `failure_category == "timeout"` → either bump the timeout or look at server perf. Don't change assertions.
- `failure_category == "assertion_failed"` AND `TARN-A-STATUS-MISMATCH` → look at the response body and the URL. Probably the endpoint or method is wrong.
- `failure_category == "assertion_failed"` AND `TARN-A-BODY-SHAPE` → response shape changed. Update the JSONPath, don't touch the URL.
- `failure_category == "capture_error"` → previous step assertion passed but `$.id` couldn't be extracted. The shape of *that* response drifted.

This isn't magic. It's just data instead of prose. The agent can branch on a six-state enum trivially. It cannot reliably branch on a sentence.

So I built that.

## Tarn — what it actually is

Tarn is a CLI-first API testing tool I wrote in Rust. The whole bet is that contract: every failure comes back with a stable category, a stable error code, and a list of remediation hints.

Tests are `.tarn.yaml` files. The minimal one:

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

YAML on purpose. Models already know YAML — there is no DSL to teach. There is no test framework to bootstrap. An LLM writes a `.tarn.yaml` file, you run `tarn run`, it goes.

A more realistic test:

```yaml
name: User CRUD
env:
  base_url: "http://localhost:3000/api/v1"

tests:
  create_and_verify:
    steps:
      - name: Create user
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          body:
            name: "Jane"
            email: "jane.{{ $random_hex(6) }}@example.com"
        capture:
          user_id: "$.id"
        assert:
          status: 201
          body:
            "$.id": { type: string, not_empty: true }

      - name: Verify user
        request:
          method: GET
          url: "{{ env.base_url }}/users/{{ capture.user_id }}"
        assert:
          status: 200
          body:
            "$.id": "{{ capture.user_id }}"
```

`{{ $random_hex(6) }}` is a built-in faker so each run gets a unique email. `capture` plucks `$.id` from the create response and types it (string stays string, number stays number — important for downstream JSONPath assertions). The second step interpolates `{{ capture.user_id }}` into both the URL and the assertion.

Default human output:

```
$ tarn run tests/users.tarn.yaml
 ● User CRUD / create_and_verify
   ✓ Create user (123ms)
   ✓ Verify user (45ms)
 Results: 1 test passed (180ms)
```

For agents and CI, ask for JSON:

```
$ tarn run tests/users.tarn.yaml --format json --json-mode compact
```

You get a complete machine-readable run report with `failure_category`, `error_code`, request, response, captures, durations, all of it. Successful steps are summarized; failed steps include the full request and response so the agent has every byte it needs to diagnose the issue without re-running anything.

## The agent loop in practice

Here is what a realistic loop looks like when I'm pairing with Claude Code on a new endpoint.

Me: "I just added `POST /users/:id/avatar` for multipart avatar uploads. Write a test for it."

Claude Code writes `tests/avatar.tarn.yaml` with a multipart upload step. Runs `tarn run tests/avatar.tarn.yaml --format json`.

Output (failure):

```json
{
  "tests": [{
    "name": "Upload avatar",
    "status": "failed",
    "steps": [{
      "name": "POST avatar",
      "failure_category": "assertion_failed",
      "error_code": "TARN-A-STATUS-MISMATCH",
      "request": {
        "method": "POST",
        "url": "http://localhost:3000/api/v1/users/abc-123/avatar",
        "multipart": [{"name": "file", "filename": "avatar.png", "size": 4321}]
      },
      "response": {
        "status": 400,
        "body": {"error": "missing field 'avatar'"}
      },
      "hints": [
        "Server returned 400 with body containing 'missing field'. Check that the multipart field name matches the server expectation."
      ]
    }]
  }]
}
```

Claude reads `failure_category: "assertion_failed"`, sees the hint about a missing field, looks at the response body — `missing field 'avatar'` — and the request — `name: "file"`. Patches the YAML to use `name: "avatar"`. Re-runs. Green.

Total round trip: maybe 30 seconds. No human in the middle. The interesting part is that the agent didn't have to *guess* — it had a `failure_category` to branch on, a hint to read first, and the request/response to confirm.

## MCP — making it tool-native

The next thing I built was `tarn-mcp`, a server that implements the Model Context Protocol. Instead of Claude Code shelling out to `tarn run --format json` and parsing stdout, it can call typed MCP tools directly.

The tools:

- `tarn_run` — execute a test or directory, structured JSON return
- `tarn_validate` — syntax check before running
- `tarn_fix_plan` — consume a failure report, emit structured fix suggestions
- `tarn_inspect` — drill into a specific failure (`file::test::step`) without parsing the full report
- `tarn_rerun_failed` — replay only failing `(file, test)` pairs
- `tarn_diff` — compare two run reports, bucket failures into `new` / `fixed` / `persistent`
- a few more

Configure it in your `.mcp.json`:

```json
{
  "mcpServers": {
    "tarn": {
      "command": "tarn-mcp"
    }
  }
}
```

Now Claude Code, opencode, Cursor, Windsurf — anything that speaks MCP — can call these as tools. Faster, less brittle, and the agent doesn't waste tokens re-parsing the same stdout format on every call.

## What surprised me

A few things from real use I didn't expect when I started:

**1. YAML mattered more than the structured failures.** I expected the structured-JSON-failure thing to be the headline win. It is — but the bigger jump in agent first-shot correctness came from switching the test format from a Jest-style DSL to plain YAML. From maybe 60% correct first tests to maybe 90%. Models generate cleaner YAML than they generate any test-framework DSL, full stop. That was bigger than I gave it credit for.

**2. Failure cascades are the real problem, not single failures.** If step 3 fails because step 2 couldn't `capture: $.id`, then steps 4, 5, 6 all show as failed for unrelated reasons (they were trying to use a `user_id` that doesn't exist). The naive agent tries to fix step 6 first — and step 6 looks fine. Confusion compounds. Tarn collapses these cascades into a single root-cause entry — `cascades: 5` rather than five individual failures. That single change made loops noticeably more efficient.

**3. The `tarn_fix_plan` tool is the most uncertain piece.** I built it as an MCP tool that consumes a failure report and emits structured fix *suggestions*. But I'm honestly not sure that's the right level of abstraction. Maybe the model should just see the raw failure report and plan its own fix, and `tarn_fix_plan` is over-engineering. I haven't decided yet. If you've built similar agent tools, I'd love to hear which side of this you've landed on.

## What it deliberately doesn't do

I want to be transparent about scope:

- **No XPath / HTML assertions.** Hurl is better for HTML scraping.
- **No full Hurl-style filter DSL.** Hurl wins on filter depth.
- **No OpenAPI-first test generation.** People keep asking; I'm not yet convinced this is the right fit for the agent loop, where the model generates tests from informal specs anyway.
- **No GUI.** Bruno has an excellent one. If you want a GUI, use Bruno; Tarn is for CI and agent loops.
- **No record-replay.** Trace-based testing tools exist for that.

Tarn's bet is specifically the write-run-fix slice that an AI coding agent drives. If you're hand-writing tests as a human, Hurl or Bruno will probably make you happier.

## Try it

If you're driving an agent loop where API tests are part of the picture, Tarn might fit. The install is one line:

```bash
curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh
tarn init
tarn run
```

Single static binary, musl-linked so it runs on any Linux from Alpine to RHEL, plus macOS (Intel + Apple Silicon) and Windows. MIT-licensed. The `install.sh` also lays down `tarn-mcp` and `tarn-lsp` (a Language Server for in-editor diagnostics on `.tarn.yaml` files) when those are available in the release archive.

- Repo: <https://github.com/NazarKalytiuk/tarn>
- Docs: <https://nazarkalytiuk.github.io/tarn/>
- MCP setup: <https://nazarkalytiuk.github.io/tarn/mcp.html>

I'm specifically interested in feedback on three things:

1. The JSON failure schema (`schemas/v1/report.json`) — does the failure-category taxonomy feel complete, too coarse, or too fine?
2. Whether `tarn_fix_plan` (the MCP fix-suggestion tool) is the right abstraction, or whether it should just emit raw failures and let the model plan the fix itself.
3. What's missing for *your* specific agent loop — what would make you switch from your current setup, if you have one?

If you build something with it, drop a note in the GitHub issues or come find me. I'm reachable.

## END

---

## Post-publish checklist

- [ ] Article goes live with `published: true`
- [ ] Tags: `rust`, `ai`, `testing`, `mcp` (4-tag max enforced)
- [ ] Cover image set (or accept default)
- [ ] Add the dev.to URL to `README.md` "as featured on" section if it gets traction (>500 reads)
- [ ] Drop the URL as a comment under your r/mcp post: "I wrote up the longer story behind this on dev.to" — no overt promo, just link
- [ ] Cross-post to Hashnode in 1-2 weeks (don't same-day; gives dev.to its SEO fair shot)
- [ ] Save the URL — you'll link to this from every future Tarn launch piece (HN when you can post, Twitter, awesome-list PRs, etc.)

## Why this article matters more than the launch tweets

A dev.to article is a permanent, indexable artifact. Three months from now when someone searches "AI agent API testing" or "MCP server API tests" or "structured failure JSON Rust", this piece can still be working. A tweet has a 6-hour half-life. A Reddit post has maybe 24 hours. dev.to articles routinely get 50% of their lifetime traffic in months 2–6, not week 1.

Don't optimize for first-day numbers. Optimize for the article being good enough that someone deep into building agent tooling six months from now actually reads it through and shows up at the GitHub repo.

## What to do if engagement is low

- Don't repost it — that hurts SEO via canonical-URL dilution
- Do leave thoughtful comments on related dev.to articles in the AI/testing/MCP space; that drives some traffic and signals you're a real human
- Do link to it from every future channel — your HN post (eventually), Twitter, awesome-list PRs
- After 30 days, look at dev.to analytics: which sections did people read, where did they bounce. That tells you what to write next.
