# Tarn v0.1 — Release Readiness Review

**Date:** 2026-03-29
**Verdict:** CONDITIONAL GO — 3 blockers, ~8 hours of work

---

## 1. Release Readiness Assessment

### 1a. Feature Completeness — READY

**Tarn vs Competitors Feature Matrix:**

| Feature | Tarn | Hurl | StepCI | Bruno CLI | Tavern | Runn |
|---|---|---|---|---|---|---|
| Single binary | ✅ | ✅ | ❌ (npm) | ❌ (npm) | ❌ (pip) | ✅ |
| Cookie jar | ✅ (named jars) | ✅ | ✅ | ✅ | ❌ | ✅ |
| Captures/variables | ✅ (type-preserving) | ✅ | ✅ | ✅ | ✅ | ✅ |
| Setup/teardown | ✅ | ❌ | ✅ | ❌ | ✅ (pytest) | Partial |
| Multipart | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
| GraphQL | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| Polling/retry | ✅ | ✅ | ✅ | ❌ | ❌ | ✅ |
| Scripting | ✅ (Lua) | ❌ | Partial (JS) | ✅ (JS) | ✅ (Python) | ✅ |
| Benchmarking | ✅ | ❌ | Unstable | ❌ | ❌ | Partial |
| MCP server | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Output formats | 5 | 5 | 1 | 3 | pytest | Limited |
| JSON failure taxonomy | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| YAML format | ✅ | Custom | ✅ | Custom (Bru) | ✅ | ✅ |
| OpenAPI import | ❌ | ❌ | ✅ | ✅ | ❌ | ❌ |
| First-class auth | ❌ | `--user` | ❌ | ✅ (GUI) | ❌ | ❌ |
| gRPC/WebSocket | ❌ | ❌ | Partial | ❌ | ✅ (MQTT) | ✅ (gRPC) |
| Windows support | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ |
| GitHub stars | 0 (new) | 18.7K | 1.8K | 42.3K | 1.1K | 617 |

**Features in Tarn that NO competitor has:**

- MCP server for API testing (unique)
- JSON failure taxonomy with `failure_category` (unique)
- Named cookie jars for multi-user scenarios (unique)
- Type-preserving captures (unique)
- Benchmarking + testing in one binary (unique combo)
- 5 output formats (tied with Hurl)
- AGENTS.md / CLAUDE.md as project artifacts (unique)

**Missing features assessment:**

| Missing Feature | Blocker? | Notes |
|---|---|---|
| OpenAPI import | No | Hurl launched without it. Nice-to-have for v0.2 |
| First-class auth | No | `Authorization: "Bearer {{ env.token }}"` is a viable workaround |
| gRPC/WebSocket | No | Niche. Runn is the only competitor with both |
| Windows | NEEDS WORK | Limits audience. Not a blocker for HN (dev audience skews Mac/Linux) |
| Data factories / Faker | No | Built-ins cover basics |

---

### 1b. Quality Signals — READY

| Signal | Tarn | Assessment |
|---|---|---|
| Tests | 365 (352 unit + 13 integration) | Good for ~11.5K LOC. Hurl has more but is 4x the codebase |
| Integration tests | ✅ Real Axum demo server | Strong |
| CI pipeline | ✅ GitHub Actions (fmt + clippy + build + test) | Standard |
| All tests pass | ✅ | Clean |
| No `unsafe` code | ✅ | Clean |
| Error messages | Excellent — actionable hints, location info, typo suggestions | Exceeds most competitors |

**Test coverage by module:**

| Module | Count | Notes |
|---|---|---|
| `assert::body` | 66 | Extensive: all operators (eq, gt, lt, contains, matches, exists, length, type) |
| `parser` | 24 | YAML parsing, validation, includes (circular, nested, missing) |
| `runner` | 23 | Tags, delay, discovery, jar resolution, retries |
| `interpolation` | 23 | Env, capture, builtin, JSON type preservation |
| `assert::duration` | 18 | Parsing and boundary checks |
| `model` | 18 | Deserialization of all YAML constructs |
| `capture` | 18 | JSONPath, header, regex, type preservation |
| `env` | 17 | Resolution chain, CLI vars, file loading, shell vars |
| `assert::status` | 17 | Shorthand (2xx/4xx/5xx), ranges, sets |
| `report::json` | 14 | Redaction, failure details, schema version |
| `builtin` | 14 | uuid, random_hex, random_int, timestamp, now_iso |
| `bench` | 13 | Aggregation, percentiles, rendering |
| `assert::headers` | 10 | Case-insensitive, contains, matches, missing header |
| `report::junit` | 9 | XML structure, escaping, counts |
| `report::tap` | 8 | TAP v13 format, diagnostics |
| `scripting` | 8 | Lua response access, captures, assertions, syntax errors |
| `error` | 8 | Exit code mapping |
| `report::html` | 7 | Structure, self-contained, data embedding |
| `assert::types` | 6 | Result aggregation |
| `config` | 6 | Defaults, partial, invalid, missing |
| `cookie` | 6 | Capture, overwrite, case-insensitive |
| `report::human` | 4 | Pass/fail rendering, setup/teardown |
| `update` | 4 | Version comparison, target string |
| `http` | 4 | Method validation, connection refused, timeout |
| `assert` (mod) | 3 | Top-level assertion orchestration |
| `report` (mod) | 1 | OutputFormat parsing |
| **Integration** | **13** | Health, failure exit code, JSON/JUnit/TAP output, capture chaining, tags, validate, dry-run, setup/teardown, auth, completions |

**Edge case coverage gaps:**

- ❌ Unicode in body/headers/URLs — untested
- ❌ Large responses (>10MB) — untested
- ❌ SSL certificate errors — untested
- ❌ Malformed JSON responses — handled but untested
- ❌ Watch mode — zero tests
- ❌ Parallel execution — untested
- ❌ GraphQL body construction — untested
- ❌ Multipart success path — untested (only rejection test exists)
- ❌ Poll mode — no unit tests

These are gaps but not blockers. Real-world usage will surface issues.

---

### 1c. Documentation Completeness — READY

| Element | Tarn | Hurl | Bruno |
|---|---|---|---|
| Quick start | ✅ | ✅ | ✅ |
| Installation | ✅ (curl, cargo, binaries) | ✅ (brew, cargo, npm, docker, etc.) | ✅ |
| CLI reference | ✅ (all commands documented) | ✅ | ✅ |
| Example gallery | ✅ (7 examples) | ✅ | ✅ |
| FAQ/Troubleshooting | ❌ | ✅ | ✅ |
| CONTRIBUTING.md | ❌ | ✅ | ✅ |
| CHANGELOG | ❌ | ✅ | ✅ |
| Selling vs documenting | Sells well — "50% fewer tokens" | Documents | Sells ("revolutionizing") |

**README quality:** 900 lines, comprehensive. Opens with clear value prop. The "50% fewer tokens" claim and MCP integration are strong differentiators. Compares favorably to Hurl and Bruno.

**Missing docs (pre-launch):**

- CONTRIBUTING.md — NEEDS WORK (2h effort)
- CHANGELOG.md — NEEDS WORK (1h effort)
- FAQ section — nice-to-have

---

### 1d. Installation Experience — NEEDS WORK

| Method | Status | Notes |
|---|---|---|
| `curl \| sh` | ✅ Works | No checksum verification |
| `cargo install` | ❌ Not published | Crate not on crates.io |
| `brew install` | ❌ No formula | Significant gap vs Hurl |
| Pre-built binaries | ✅ 4 targets | macOS Intel/ARM, Linux x86/ARM |
| Windows | ❌ | No binary, no installer |

**Steps from zero to first `tarn run`:**

- Tarn: `curl -fsSL ... | sh` → `tarn init` → `tarn run` (3 steps)
- Hurl: `brew install hurl` → write file → `hurl file.hurl` (3 steps)

Comparable, but `brew install` is more trusted than `curl | sh`.

**Key gap: crates.io publishing.** Rust users expect `cargo install tarn` to work. This is a **moderate blocker** — Rust community (r/rust, HN Rust users) will immediately try this.

---

### 1e. First-Run Experience — READY

- `tarn init` creates: `tests/` dir, `tarn.env.yaml`, `tarn.config.yaml`, sample test file
- Error messages are excellent — actionable hints, typo suggestions, line:col locations
- 7 example files cover common scenarios
- Time from install to "wow": ~2 minutes (assuming API to test against)

**Gap:** No "hello world" that works without external API. The `tarn init` template targets `{{ env.base_url }}/health` which requires a running server. A self-contained example (hitting httpbin.org or jsonplaceholder.typicode.com) in the init template would improve this.

---

## 2. AI-Native Claim Validation

### 2a. JSON Output for LLM Diagnosis — READY (Strong USP)

Tarn's JSON output is specifically designed for LLM consumption:

- `schema_version: 1` for forward compatibility
- `failure_category` enum (assertion_failed, connection_error, timeout, parse_error, capture_error) — no other tool has this
- Full request/response **only for failed steps** (compact output, fewer tokens)
- Secret redaction built-in
- Structured assertion details with expected/actual/message

This is genuinely better than pytest --json-report or Playwright --reporter=json for LLM diagnosis. The failure taxonomy allows an LLM to immediately branch its debugging strategy.

### 2b. Test Generation by LLMs — READY

YAML is the most LLM-friendly test format because:

- Every LLM has extensive YAML training data
- Structure is self-documenting
- JSON Schema exists for validation
- Hurl's custom format requires learning; YAML is known

### 2c. MCP Integration — READY (Unique)

tarn-mcp is built and functional (430 LOC, 3 tools). No other API testing tool has an MCP server. This is a genuine first-mover advantage.

**Gap:** No recorded demo of the Claude Code → tarn_run → analyze → fix workflow. A GIF or video would be powerful for launch.

### 2d. AGENTS.md — READY

100+ lines of structured guidance for AI agents. Quick reference, assertion operators, JSON output format, testing tips. Combined with CLAUDE.md, this provides comprehensive AI-agent documentation that no competitor offers.

---

## 3. Competitive Positioning

### 3a. New Players

| Tool | Threat Level | Notes |
|---|---|---|
| Postman Agent Mode | Medium | Pushes users away with pricing, but AI features set expectations |
| Keploy | Low | Different approach (traffic capture), 15.2K stars |
| Octrafic | Low | Very early (v0.4.0), natural language approach |
| Kusho AI | Low | Commercial, YC-backed, different segment |
| EvoMaster | Low | AI fuzzing, different approach (evolutionary algorithms) |
| Jikken | Low | Rust CLI, 131 stars, very small community |

No direct competitor combines CLI-first + YAML + AI-native + MCP. The window is open.

### 3b. Positioning

"API testing that AI agents can write, run, and debug" — this is clear, differentiated, and testable. No competitor uses AI messaging. The first 3 paragraphs of Tarn's README are stronger than Hurl's (which is purely descriptive) and comparable to Bruno's (which focuses on Postman replacement).

### 3c. USP Assessment

| Claimed USP | Real USP or Marketing? |
|---|---|
| MCP server | **Real** — genuinely unique in the space |
| Lua + YAML in one tool | **Real** — Hurl has no scripting at all |
| Named cookie jars | **Niche but real** — useful for multi-user testing |
| Type-preserving captures | **Real** — avoids string coercion bugs |
| JSON failure taxonomy | **Real** — unique, LLM-optimized |
| Bench + test in one binary | **Real** — no competitor combines both |
| 5 output formats | **Tied** with Hurl |
| AGENTS.md / CLAUDE.md | **Real** — signals AI-native intent |

---

## 4. Launch Strategy

### 4a. Naming — NEEDS WORK

| Registry | Status |
|---|---|
| crates.io | ✅ Available |
| npm | ❌ Taken (4.7M weekly downloads — Knex connection pool) |
| Google "tarn API testing" | Zero results (no presence yet) |
| GitHub | ✅ Available (current repo name is hive-api-test) |

The npm conflict is not blocking (different ecosystem) but will cause SEO confusion. "tarn" is a mountain lake — memorable, short, but not self-explanatory. Comparable to "hurl" which also had naming conflicts.

**Action needed:** Rename GitHub repo from `hive-api-test` to `tarn` before launch.

### 4b. GitHub Repository Readiness — NEEDS WORK

| Element | Status |
|---|---|
| LICENSE | ✅ MIT |
| .gitignore | ✅ |
| Topics/tags | ❌ Not set |
| Description | ❌ Not set |
| Social preview image | ❌ Missing |
| Releases with binaries | ✅ (via release.yml) |
| Issue templates | ❌ Missing |
| Discussions | ❌ Not enabled |
| Badges | ❌ Not in README |
| CONTRIBUTING.md | ❌ Missing |
| CHANGELOG.md | ❌ Missing |

**Recommended GitHub topics:** `rust`, `cli`, `api-testing`, `http-client`, `testing`, `testing-tools`, `developer-tools`, `yaml`, `integration-testing`, `automation`, `rest-api`, `api-client`, `http`, `ci-cd`, `ai-native`, `llm`, `mcp`

### 4c. Hacker News Launch

**Optimal title:** `Show HN: Tarn – CLI API testing tool designed for AI agents (Rust, single binary)`

**Optimal timing:** Sunday 12:00-14:00 UTC (highest breakout rate: 11.75-15.7%)

**Expected traction:** 50-200 stars week 1 (realistic for solo Rust CLI tool). Getting to Hurl-level (461 points) would require exceptional positioning.

**Reference HN posts:**

| Title | Points | Comments | Date |
|---|---|---|---|
| Hurl 4.0.0 | 592 | 102 | June 2023 |
| Hurl: Run and test HTTP requests with plain text | 461 | 112 | June 2025 |
| Bruno: Fast and Git-friendly open-source API client (Postman alternative) | 1,538 | 400 | — |
| StepCI: auto-generates API tests | 108 | 52 | Oct 2022 |

**Expected criticism and responses:**

| Criticism | Response |
|---|---|
| "Why not just use Hurl?" | "Hurl is great for humans. Tarn is designed for AI agents — structured JSON with failure taxonomy, MCP server, YAML format LLMs already know" |
| "Yet another YAML format" | "YAML is deliberately chosen — it's the most LLM-tokenizable format. No custom syntax to learn" |
| "No OpenAPI import?" | "Planned for v0.2. You can start testing any API right now without a spec" |
| "No Windows?" | "Tracking in roadmap. PRs welcome" |
| "AI-native is just marketing" | "Here's the MCP server, here's the JSON failure taxonomy, here's the AGENTS.md. Try it with Claude Code" |

### 4d. Multi-Channel Strategy

| Channel | Priority | Timing | Notes |
|---|---|---|---|
| Hacker News (Show HN) | P0 | Day 0 (Sunday) | Link directly to GitHub |
| r/rust | P0 | Day 0-1 | Needs blog post first |
| This Week in Rust newsletter | P1 | Submit PR week of launch | Requires blog post |
| awesome-mcp-servers | P1 | Day 1 | Unique angle, high-traffic list |
| awesome-rust | P1 | After 50+ stars | Has minimum star requirement |
| Dev.to / Hashnode article | P1 | Day 0 (cross-post) | "I built X in Rust" format |
| Twitter/X | P2 | Day 0 | Tag @rustlang, @thisweekinrust |
| r/programming | P2 | After traction | Strict self-promo rules |
| Console.dev newsletter | P2 | Email submission | Reviews 2-3 dev tools weekly |
| Product Hunt | P3 | Week 2 | Medium impact tier |
| Lobste.rs | P3 | Invite-only | Strong systems programming audience |

**Key influencers to engage:**

| Handle | Name | Relevance |
|---|---|---|
| @rustlang | Rust Language (official) | Core community hub |
| @ThePrimeagen | ThePrimeagen | Massive reach, covers Rust tools |
| @fasterthanlime | Amos Wenger | Deep Rust content creator |
| @burntsushi5 | Andrew Gallant | Creator of ripgrep, Rust team |
| @consoledotdev | Console newsletter | Reviews devtools weekly |
| @thisweekinrust | This Week in Rust | Crucial for Rust visibility |

**Relevant awesome-lists:**

| List | Stars | Submission Criteria |
|---|---|---|
| awesome-rust (rust-unofficial) | High | 50+ GitHub stars OR 2,000+ crates.io downloads |
| awesome-mcp-servers (wong2) | Very high | MCP server listing |
| awesome-devops-mcp-servers | Medium | DevOps-focused MCP tools |
| awesome-rust-testing | Smaller | Less strict, curated list |
| awesome-http-clients | Medium | HTTP clients and API tools |
| awesome-api-tools | Medium | API testing tools collection |
| awesome-testing (TheJambo) | Medium | General testing resources |

---

## 5. Critical Defects and Risks

### 5a. Showstopper Bugs

| Scenario | Status | Severity |
|---|---|---|
| Non-JSON response (XML, HTML) | ✅ Handled — wraps in String | OK |
| Response > 10MB | ❌ Untested | Low risk |
| Invalid SSL cert | ❌ Untested (uses rustls defaults — will reject) | OK behavior, undocumented |
| Connection refused | ✅ Excellent error message | OK |
| YAML syntax error | ✅ Line:col + hints + typo suggestions | Excellent |
| JSONPath not found | ✅ Suggestions (case mismatch, available keys) | Excellent |
| Capture returns null | ✅ Suggestions | OK |

### 5b. Performance

| Concern | Assessment |
|---|---|
| Regex recompilation | Every `interpolate()` call recompiles. Measurable but not blocking |
| HTTP client per-request | No connection reuse. Performance hit for large suites |
| Watch mode memory | No leak risk — re-runs from scratch each time |
| 1000 test files | Should work (glob + rayon parallelism) |

### 5c. Security — BLOCKER

| Issue | Severity | Details |
|---|---|---|
| **Lua sandbox escape** | **CRITICAL** | `Lua::new()` in `scripting.rs:22` loads full stdlib: `os.execute()`, `io.open()`, `io.popen()`, `loadfile()`, `dofile()`, `require()`, `debug` library. Any `.tarn.yaml` with `script:` can execute arbitrary commands |
| Secret redaction gaps | OK | Covers all 5 output formats |
| install.sh no checksums | Medium | No SHA256 verification of downloaded binaries |
| No unsafe Rust | ✅ | Clean |

**The Lua sandbox is a release blocker.** If someone downloads a malicious `.tarn.yaml` file and runs `tarn run`, the `script:` block can execute `os.execute("rm -rf /")`. This will be the first thing security-conscious HN commenters will find.

**Fix:** Replace `Lua::new()` with `Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::MATH, LuaOptions::default())`. Estimated effort: **1-2 hours**.

**Secret redaction details:**

- **JSON reporter** (`report/json.rs:124-144`): Redacts `Authorization`, `Cookie`, `Set-Cookie`, `X-Api-Key`, `X-Auth-Token`. Case-insensitive.
- **Human reporter**: Does NOT include request/response details — no secrets leak.
- **JUnit reporter**: Only shows failure messages, no raw headers.
- **TAP reporter**: Same as JUnit — only failure diagnostics.
- **HTML reporter**: Delegates to `json::render()` which applies redaction.

### 5d. Platform Coverage — NEEDS WORK

| Platform | Build | Test (CI) | Binary |
|---|---|---|---|
| Linux x86_64 | ✅ | ✅ | ✅ |
| Linux aarch64 | ✅ | ❌ | ✅ |
| macOS Intel | ✅ | ❌ | ✅ |
| macOS Apple Silicon | ✅ | ❌ | ✅ |
| Windows | ❌ | ❌ | ❌ |
| Alpine/musl | ❌ | ❌ | ❌ |

CI only runs on Ubuntu. macOS binaries are released but never tested in CI.

### 5e. Missing "Table Stakes"

| Element | Status |
|---|---|
| `--help` for all commands | ✅ |
| `--version` | ✅ (`tarn 0.1.0`) |
| Exit codes documented | ✅ (in README + spec) |
| Shell completions | ✅ (bash/zsh/fish/powershell/elvish) |
| Man page | ❌ (nice-to-have) |

**Bug found:** `tarn list --tag` flag is silently ignored (captured as `tag: _` in `main.rs:178`). Minor but should be fixed.

---

## 6. OpenAPI as Pre-Launch Feature — NOT NEEDED

- Hurl launched without OpenAPI and reached 18.7K stars
- StepCI had it from day 1 and only reached 1.8K stars
- OpenAPI import is Tier 1 roadmap but not a launch blocker
- Should be clearly listed in ROADMAP.md as "coming in v0.2"
- The `tarn init --from openapi.yaml` command can be a compelling follow-up HN post

---

## 7. Auth as Pre-Launch Feature — NOT NEEDED

- `Authorization: "Bearer {{ env.token }}"` is a well-understood pattern
- Hurl's only auth convenience is `--user` for Basic auth
- No competitor besides Bruno (GUI) has comprehensive first-class auth
- Bearer token + API key via headers covers 90%+ of real-world usage
- OAuth2 client_credentials would be a strong v0.2 feature

---

## 8. Recommendations

### 8a. Go/No-Go: CONDITIONAL GO

Tarn is 90% ready for "Show HN". Three items must be addressed first.

### 8b. Top 5 Actions Before Launch

| # | Action | Effort | Priority |
|---|---|---|---|
| 1 | **Fix Lua sandbox** — replace `Lua::new()` with restricted stdlib (TABLE + STRING + MATH only) | 1-2h | **BLOCKER** |
| 2 | **Publish to crates.io** — `cargo publish` | 1h | **BLOCKER** |
| 3 | **Rename GitHub repo** from `hive-api-test` to `tarn` | 30min | **BLOCKER** |
| 4 | **GitHub polish** — add topics, description, social preview, badges, issue templates | 2-3h | HIGH |
| 5 | **Add CONTRIBUTING.md + CHANGELOG.md** | 2h | HIGH |

Total: **~8 hours** to reach launch-ready state.

### 8c. Top 5 Things That Can Wait

| # | Item | Why It Can Wait |
|---|---|---|
| 1 | OpenAPI import | Hurl proved it's not needed at launch |
| 2 | First-class auth | Headers workaround is sufficient |
| 3 | Windows support | HN/dev audience skews Mac/Linux |
| 4 | Homebrew formula | Can add after initial traction |
| 5 | Additional tests for edge cases | Real-world usage will surface issues faster |

### 8d. Risk Mitigation

| Risk | Mitigation | Effort |
|---|---|---|
| Lua sandbox escape | Fix before launch — restricted stdlib | 1-2h |
| install.sh no checksums | Add SHA256 verification | 2h |
| `list --tag` bug | Fix the ignored flag | 30min |
| CI only on Ubuntu | Add macOS CI job | 1h |
| npm naming conflict | Not actionable. SEO will build over time with "tarn API testing" | — |
| Regex recompilation | Use `lazy_static!` or `once_cell::sync::Lazy` | 1h |
| HTTP client per-request | Create client once per test file, pass through steps | 2h |

### 8e. Success Metrics

| Metric | Failure | OK | Good | Great |
|---|---|---|---|---|
| HN points | <10 | 10-50 | 50-200 | 200+ |
| Stars day 1 | <5 | 5-30 | 30-100 | 100+ |
| Stars week 1 | <20 | 20-100 | 100-300 | 300+ |
| Stars month 1 | <50 | 50-300 | 300-1,000 | 1,000+ |
| External issues week 1 | 0 | 1-3 | 3-10 | 10+ |
| External PRs month 1 | 0 | 1 | 2-5 | 5+ |
| MCP installs | 0 | 1-5 | 5-20 | 20+ |

**Reference benchmarks:**
- Average HN launch: 121 stars in 24h, 289 in a week
- 1 day on GitHub Trending: 500-2,000 new stars
- Credibility threshold: 100-1,000 stars

---

## Executive Summary

### CONDITIONAL GO — 3 blockers, ~8 hours of work

**Must fix before launch:**

1. **Lua sandbox is wide open** — `os.execute()` available via `script:` blocks. Critical security vulnerability that HN will find immediately
2. **Not on crates.io** — Rust users expect `cargo install tarn` to work
3. **GitHub repo is named `hive-api-test`** — must be `tarn`

**Strengths that justify launching now:**

- Feature set exceeds Hurl in several areas (setup/teardown, scripting, benchmarking, MCP)
- AI-native positioning is genuinely unique and defensible
- MCP server is a first-mover advantage that competitors will copy
- JSON failure taxonomy is a real innovation for LLM workflows
- Error messages are best-in-class
- 365 tests, clean codebase, MIT license

**Market timing is favorable:**

- Postman's March 2026 pricing changes are pushing developers to open-source tools
- No competitor has AI-native features
- The MCP ecosystem is exploding and tarn-mcp would be the first API testing MCP server
- Weekend HN posting (Sunday 12:00 UTC) gives 20-30% better odds

**Recommended launch date:** Next Sunday after fixing the 3 blockers.

---

## Appendix A: Competitor Deep Profiles

### Hurl (hurl.dev)

- **Stars:** 18,686 | **Contributors:** 87 | **License:** Apache-2.0
- **Latest:** v7.1.0 (Nov 2025) | **Language:** Rust (libcurl)
- **Install:** Homebrew, Cargo, npm, Docker, conda-forge, apt, pacman, Chocolatey, Scoop
- **Formats:** Text, JSON, JUnit, TAP, HTML
- **AI features:** None
- **HN best:** 592 points (June 2023)
- **Positioning:** "Command line tool that runs HTTP requests defined in simple plain text format"
- **Strengths:** Plain-text format, libcurl (HTTP/3, IPv6), massive community, wide distribution
- **Weaknesses:** No scripting, no setup/teardown, no benchmarking, no MCP, no AI features

### StepCI (stepci.com)

- **Stars:** 1,844 | **Contributors:** 18 | **License:** MPL-2.0
- **Latest:** v2.8.2 (June 2024 — 9 months stale)
- **Language:** TypeScript | **Install:** npm, Homebrew
- **AI features:** None (OpenAPI auto-generation is rule-based)
- **Concerns:** Stagnating — last release 9 months ago. Restrictive license.

### Bruno (usebruno.com)

- **Stars:** 42,368 | **Contributors:** 406 | **License:** MIT
- **Latest:** v3.2.0 (March 2026) | **Language:** JavaScript
- **Positioning:** Postman replacement (GUI-first, CLI secondary)
- **AI features:** None (privacy-first, deliberately no cloud/AI)
- **Context:** Massive star growth driven by Postman's pricing changes

### Tavern (taverntesting.github.io)

- **Stars:** 1,131 | **Contributors:** 59 | **License:** MIT
- **Language:** Python (pytest plugin) | **Install:** pip
- **Strengths:** Deep pytest integration, Python ecosystem
- **Weaknesses:** Python-only, has hit its ceiling at 1.1K stars

### Runn (github.com/k1LoW/runn)

- **Stars:** 617 | **Contributors:** 32 | **License:** MIT
- **Latest:** v1.6.2 (March 27, 2026 — very active) | **Language:** Go
- **Strengths:** Multi-protocol (HTTP + gRPC + SQL + CDP + SSH), usable as Go library
- **Weaknesses:** Small community, Japanese-centric docs

### New AI-Native Tools

- **Kusho AI** — Commercial, YC-backed, generates tests from OpenAPI via AI
- **Keploy** — 15.2K stars, eBPF traffic capture, AI-powered test generation
- **Octrafic** — v0.4.0, natural language API testing, multi-LLM support
- **EvoMaster** — 657 stars, evolutionary algorithm API fuzzing

## Appendix B: MCP Servers for API Testing

| Server | Description |
|---|---|
| openapi-hurl-mcp | Bridges OpenAPI specs with Hurl; generates Hurl test files |
| mcp-rest-api (dkmaker) | TypeScript MCP for REST APIs via Cline |
| mcp-http-client | REST API client with Swagger discovery |
| APIAgent (Agoda) | Converts any REST/GraphQL API into MCP server |
| Apollo MCP Server | Turns GraphQL operations into MCP tools |

No "tarn-mcp" found in any public registry. The space is nascent but growing.

## Appendix C: Codebase Statistics

| Metric | Value |
|---|---|
| Crates | 3 (tarn, demo-server, tarn-mcp) |
| Total Rust LOC | ~11,500 |
| Core (tarn/src) | ~9,700 |
| MCP Server | ~430 |
| Modules (tarn) | 17 primary + 12 nested (assert + report) |
| Dependencies (tarn) | 18 runtime + 5 dev |
| Test count | 365 (352 unit + 13 integration) |
| Example files | 8 YAML files (~573 lines) |
| Output formats | 5 (human, json, junit, tap, html) |
| Assertion operators | 20+ |
| Built-in functions | 5 |
| Supported platforms | macOS (Intel + ARM), Linux (x86_64 + ARM64) |
| License | MIT |
