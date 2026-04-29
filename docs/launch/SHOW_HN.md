# Show HN — draft

Status: **draft, not published**.
Target window: **Tuesday or Wednesday, 06:00–08:00 PT** (best HN front-page window).
Author must be online and responsive for **at least 4 hours** after submitting.

---

## Title

Pick one — they trade off engineering credibility vs. AI-zeitgeist pull. **Option A is recommended for HN specifically**; B is better for X/Twitter; C is the safest fallback if "AI" reads as buzzwordy on a given day.

- **A. `Show HN: Tarn – CLI-first API testing with structured JSON failures (Rust)`**
  Leads with the engineering. Rust is HN catnip. "Structured JSON failures" is the actual differentiator. AI angle comes through in the body without taking the front seat.
- B. `Show HN: Tarn – API tests an AI agent can write, run, and debug`
  Punchier, but HN sometimes downvotes anything that looks like an AI launch. Use this only if you've seen recent Show HNs in this space rewarded.
- C. `Show HN: Tarn – YAML API tests, single binary, structured JSON output`
  Neutral and safe. No AI in title. Lower ceiling, lower floor.

URL field: `https://github.com/NazarKalytiuk/tarn` (HN gives the URL the strongest signal — point at the repo, not the docs site).

---

## Body (200–260 words)

> Tarn is a CLI-first API testing tool. Tests are `.tarn.yaml` files; output is structured JSON. Single binary, no runtime, MIT.
>
> The thing I built it to fix: when an API test fails, my LLM agent (Claude Code, Cursor) had to scrape human-readable output to figure out what broke. So Tarn returns failures as structured JSON with a stable `failure_category` (one of `assertion_failed`, `connection_error`, `timeout`, `capture_error`, `parse_error`), an `error_code`, the offending request and response, and a list of remediation hints. The agent reads the taxonomy, picks a fix, and reruns. No regex on log output.
>
> There is also a `tarn-mcp` server that exposes the run loop as MCP tools (`tarn_run`, `tarn_validate`, `tarn_fix_plan`, `tarn_inspect`, `tarn_rerun_failed`, …) so Claude Code / opencode / Cursor / Windsurf can drive it without shelling out.
>
> Some intentional choices:
> - YAML, not a custom DSL — models already know it, no syntax to teach.
> - Failures-first CLI: `tarn failures` collapses cascade-skips, `tarn rerun --failed` replays only broken (file, test) pairs, `tarn diff prev last` buckets fingerprints into new / fixed / persistent.
> - Per-step taxonomy beats a single "test failed" boolean for agent decision-making.
>
> What it does NOT try to be: full Hurl parity (no XPath, no full filter DSL), no OpenAPI-first generation, no GUI. Hurl is still better for handwritten HTTP specs and libcurl features. Bruno has a wider ecosystem and a GUI. Tarn's bet is the agent loop.
>
> Repo: https://github.com/NazarKalytiuk/tarn
> Docs: https://nazarkalytiuk.github.io/tarn/
>
> Feedback wanted especially on the JSON failure schema (`schemas/v1/report.json`), the assertion DSL, and whether the MCP tool surface is the right shape.

---

## First comment (post immediately after submission)

HN best practice: as soon as the post is up, the author posts one substantive comment with backstory and questions. It anchors the discussion and signals you're around.

> Author here. Two pieces of context that didn't fit in the post:
>
> The trigger was watching an LLM agent struggle to fix a flaky API test. It would run the test, get back stderr like `AssertionError: expected 200 got 404`, and the agent would correctly guess "the URL is wrong" — but it would also guess that for `connection_refused` and for `assertion failed because the body changed shape`, because all three look similar in prose. The agent needed structure, not narration. So Tarn's contract is that every failure has a `failure_category` (~5 stable values), an `error_code`, and an array of `hints`. Agents branch on `failure_category` first — that alone removes most bad guesses — then read hints for the specific fix. Full request/response is included only on failure to keep success runs cheap.
>
> Things I'm uncertain about and would love opinions on:
> - Lua scripting is in the runner. I'm worried it's a foot-gun. Should it be removed in favor of capture transforms only?
> - `tarn_fix_plan` (the MCP tool) emits a structured suggestion list. Right level of abstraction, or should the tool just emit raw failures and let the model plan?
> - I haven't done OpenAPI-first generation. People keep asking. Real demand or shiny-object territory?
>
> The repo includes a small `demo-server` (Rust) so you can run the full agent loop locally with no API keys.

---

## Twitter/X / LinkedIn cross-post (~280 chars)

> Just shipped Tarn — a CLI-first API testing tool in Rust where every failure comes back as structured JSON your agent can branch on (`failure_category`, `error_code`, `hints`). YAML tests, single binary, MCP server for Claude Code / Cursor. https://github.com/NazarKalytiuk/tarn

---

## Pre-publish checklist

Run this **same day** before submitting. Each item that fails kills the launch — pause, fix, retry next day.

### Repo
- [ ] `README.md` hero matches the post tagline (currently: "API tests an AI agent can write, run, and debug")
- [ ] `https://github.com/NazarKalytiuk/tarn` loads, latest release is recent
- [ ] `LICENSE` is present and correct
- [ ] `CONTRIBUTING.md` exists and is honest (don't promise PR turnaround you can't keep)
- [ ] No "TODO: ship X" or `unimplemented!()` in code paths that the README references
- [ ] `cargo install --git https://github.com/NazarKalytiuk/tarn.git --bin tarn` succeeds on a clean machine

### Release pipeline
- [ ] Latest release at `https://github.com/NazarKalytiuk/tarn/releases/latest` has all 6 archives (macOS Intel, macOS Apple Silicon, Linux amd64, Linux arm64, Windows amd64, plus checksums)
- [ ] `tarn-mcp` and `tarn-lsp` binaries are bundled in each archive (the post mentions them)
- [ ] `tarn.rb` Homebrew formula artifact is uploaded
- [ ] `curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh` works on a fresh macOS and a fresh Linux container — **install from the live URL, not the local copy** (project rule)

### Docs site
- [ ] `https://nazarkalytiuk.github.io/tarn/` loads in <2 s
- [ ] `getting-started.html`, `mcp.html`, `cli-reference.html`, `examples.html` all 200
- [ ] No 404s in the nav
- [ ] Hero on the docs site matches the README hero

### Demo readiness
- [ ] Local demo path works end to end: `PORT=3000 cargo run -p demo-server &` then `cargo run -p tarn -- run examples/demo-server/hello-world.tarn.yaml`
- [ ] `tarn run --format json --json-mode compact` example output is in a tab, ready to paste if someone asks "what does the JSON actually look like"
- [ ] Have `schemas/v1/report.json` open — people will ask for the schema

### Author readiness
- [ ] You can stay online and reply for **at least 4 hours** after submission
- [ ] You've drafted answers to the [Questions To Expect](../LAUNCH_PLAYBOOK.md#questions-to-expect) ahead of time
- [ ] You have a coffee. (Seriously. The first hour is decisive.)

---

## Day-of timing

| Time (PT) | Action |
|-----------|--------|
| 05:50 | Final pass on repo + docs site |
| 06:00 | Submit Show HN |
| 06:01 | Post first comment (the backstory above) |
| 06:05 | Cross-post the Twitter/X variant — link to the HN thread, not the repo |
| 06:30 | Check `/newest` placement; reply to first 1–2 comments |
| 09:00 | Reply to every comment received so far |
| 12:00 | If on the front page: reply individually to every substantive comment; do NOT thank for upvotes |
| 18:00 | Wind-down; thank the active commenters |
| Next day | r/rust post (separate draft, different lead) |

---

## Things to avoid in the thread

- Do **not** reply with "thanks!" alone. HN treats it as noise. If a comment is positive, engage with the technical content even briefly.
- Do **not** argue about Hurl/Bruno positioning — concede the parts where they're stronger. The Launch Playbook already has the calibrated lines under [Comparison Talking Points](../LAUNCH_PLAYBOOK.md#comparison-talking-points). Use them verbatim.
- Do **not** edit the post body after submission unless there's a factual error. Edits are visible and look defensive.
- Do **not** ask people to upvote anywhere. HN auto-detects vote rings and will rank-penalize the post.
