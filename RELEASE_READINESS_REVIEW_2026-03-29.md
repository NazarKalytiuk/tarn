# Tarn v0.1.0 — Release Readiness Review

Date: 2026-03-29

Scope: public GitHub launch + `Show HN` readiness review for Tarn v0.1.0, with local codebase verification and external competitive research current as of March 2026.

Positioning under review: `API testing that AI agents can write, run, and debug`

Verdict: `CONDITIONAL GO`

Executive summary:
- Tarn is strong enough to show privately or to early design partners.
- Tarn is not ready for a broad public launch today because the first-run path is broken, branding/distribution are inconsistent, and the strongest AI-native promise does not yet hold for runtime failures.
- If the top issues below are fixed, Tarn becomes viable for a narrow public launch without OpenAPI import or first-class auth.

---

## 1. Release Readiness Assessment

### 1a. Feature completeness

Status: `NEEDS WORK`

#### Competitive comparison

| Tool | Format / UX | Protocols | Auth UX | OpenAPI | Reporting | Tarn ahead? | Tarn behind? |
|---|---|---|---|---|---|---|---|
| Hurl 7.1 | plain text DSL | HTTP, GraphQL-over-HTTP, HTML/XML/JSON assertions | mostly manual headers, CLI helpers | limited compared to generators | JSON, HTML, JUnit, rich CLI | MCP, YAML easier for LLMs, named cookie jars, built-in bench | Windows/install maturity, XPath/XML/HTML support |
| StepCI 2.1 | YAML workflow | HTTP/GraphQL focus | declarative auth support | yes, `stepci generate` | CLI + machine-readable output | single Rust binary, MCP, tighter API-test focus | OpenAPI generation, schema-aware checks |
| Bruno 2.x | GUI + file-based collections + CLI | HTTP, GraphQL, gRPC, WebSocket | first-class auth incl. OAuth2 | yes, imports OpenAPI/Postman/Insomnia/WSDL | collection runner + broader ecosystem | smaller single binary concept, more compact failure JSON | protocol breadth, auth, imports, ecosystem |
| Tavern 3.x | YAML + pytest | REST, MQTT, gRPC | manual/Python ecosystem | not core | pytest ecosystem | zero-runtime binary, MCP | plugin ecosystem, protocol breadth |
| runn | YAML runbooks | HTTP, gRPC, DB, SSH, CDP/local | richer scenario primitives | OpenAPI-like + validation | profiling + scenario tooling | simpler API-testing pitch, MCP, bench | broader protocol/workflow surface |

#### Missing features vs competitors

| Missing capability | Seen in | Impact |
|---|---|---|
| OpenAPI import / scaffold generation | StepCI, Bruno, runn | `NEEDS WORK` |
| OpenAPI response validation | StepCI, runn | `NEEDS WORK` |
| First-class auth UX | Bruno, some others | `NEEDS WORK` |
| Windows distribution | Hurl, Bruno | `BLOCKER` for broad CLI launch, not for niche early adopters |
| XML / HTML assertions and captures | Hurl | `NEEDS WORK` |
| gRPC / WebSocket | Bruno, Tavern, runn | `NICE TO HAVE` for Tarn 0.1 |
| DB / SSH / workflow runners | runn | `NICE TO HAVE`, different product scope |
| Migration/import tools from Postman/curl/Hurl | Bruno, StepCI import story | `NEEDS WORK` |

#### Tarn features that are differentiated

| Tarn capability | Competitive value | USP rating |
|---|---|---|
| MCP server purpose-built for API testing | Unique and timely | `REAL USP` |
| Named cookie jars | Good for multi-user API flows | `REAL USP` |
| Type-preserving captures in a YAML DSL | Better chaining, fewer coercion bugs | `GOOD DX`, not headline |
| Built-in benchmarking in same binary | Distinctive for CLI-first workflows | `REAL USP` |
| Compact failure-oriented JSON with request/response attached only on failed steps | Strong for LLM workflows | `REAL USP`, but incomplete today |
| AI-agent-specific docs (`AGENTS.md`, `CLAUDE.md`) | Credibility signal | `SUPPORTING USP`, not moat |

#### Bottom line

Tarn is feature-complete enough for a 0.1 launch if positioned narrowly:
- REST/GraphQL API testing
- YAML-first
- AI-assisted authoring/debugging
- single binary

Tarn is not feature-complete enough to win a broad “Postman alternative” comparison.

---

### 1b. Quality signals

Status: `NEEDS WORK`

#### What the local repo proves

- CI exists: format, clippy, build, test, release build.
- Release workflow exists for Linux/macOS.
- 352 unit tests passed locally.
- 13 integration tests exist against a real demo server.
- Demo server exists and is useful for real end-to-end checks.

#### What the local repo does not yet prove well enough

- `cargo test --all` under this environment failed 9 integration tests because the harness depends on spawning and binding a local server in a way that the sandbox blocks.
- That is not necessarily a product bug, but it means the release-readiness signal is weaker than “all green everywhere.”

#### Edge-case coverage review

| Edge case | Evidence | Assessment |
|---|---|---|
| Connection refused | explicit test + real runtime check | `READY` |
| Timeout handling | explicit code/tests | `READY` |
| Redirect loops | explicit handling in `http.rs` | `READY` |
| Empty response body | parsed as JSON `null` | `READY` |
| Non-JSON response body | parsed as string | `READY` |
| YAML syntax errors | clear parse output | `READY` |
| Missing JSONPath capture | graceful failure with suggestion | `READY` |
| Unicode / large response / invalid cert | no strong repo evidence | `NEEDS WORK` |
| Long-running watch mode memory behavior | no evidence | `NEEDS WORK` |

#### Important code-level findings

- Response parsing is pragmatic: non-JSON becomes `serde_json::Value::String`.
- Runtime HTTP failures do not become structured step failures in JSON mode; they abort the run.
- Failure taxonomy exists in types and README, but not all categories are actually emitted in normal runtime paths.

#### Bottom line

Quality signals are good for a solo-built Rust CLI at 0.1, but not yet good enough to absorb a big public spike without confidence loss.

---

### 1c. Documentation completeness

Status: `NEEDS WORK`

#### What exists

- README with quick start, examples, CLI reference, output formats, schema, MCP section.
- `spec.md`
- `ROADMAP.md`
- retrospectives
- examples
- `AGENTS.md`
- `CLAUDE.md`

#### Missing or weak

| Doc asset | Status | Impact |
|---|---|---|
| `CONTRIBUTING.md` | missing | `NEEDS WORK` |
| `CHANGELOG.md` | missing | `NEEDS WORK` |
| FAQ / troubleshooting section | missing | `NEEDS WORK` |
| Explicit “known limitations” | weak | `NEEDS WORK` |
| Launch-oriented recorded demo | missing | `NEEDS WORK` |

#### README quality

The README does two things reasonably well:
- explains the product fast
- sells the AI-native angle early

But two trust gaps matter:
- install/branding references still point to `hive`
- first-run instructions imply a smooth `tarn init && tarn run`, which is not true today

#### README vs competitors

| README opening | Impression |
|---|---|
| Tarn | clear and modern, strong positioning, AI-forward |
| Hurl | mature, stable, broad HTTP testing story |
| Bruno | bigger ecosystem / product story |
| StepCI | practical workflow and OpenAPI angle |

Tarn’s opening is competitive. The problem is not copy. The problem is product/doc mismatch.

---

### 1d. Installation experience

Status: `BLOCKER`

#### Current state

| Install path | Status |
|---|---|
| `cargo install --git ...` | source install path documented |
| crates.io package | not evidenced |
| Homebrew formula | not evidenced |
| Prebuilt binaries | Linux/macOS release workflow exists |
| Windows binary | not in workflow |
| One-line install script | yes, macOS/Linux only |

#### Problems

1. Public naming is inconsistent between `hive` and `tarn`.
2. No visible crates.io publishing story.
3. No Homebrew formula.
4. No Windows release assets.
5. Install script is `curl | sh`, which is acceptable for early CLI users, but you need impeccable repo/release consistency if you use that path.

#### Comparison

| Tool | Install maturity vs Tarn |
|---|---|
| Hurl | materially better |
| Bruno | materially better |
| StepCI | better on OpenAPI-generated “first value” |
| Tavern | Python dependency burden, but standard package path |
| runn | broader package/install surface than Tarn |

---

### 1e. First-run experience

Status: `BLOCKER`

#### Verified locally

I created a clean temp directory, ran the built binary:

```bash
tarn init
tarn run
```

Observed result:

```text
Error: HTTP error: Request to {{ env.base_url }}/health failed: builder error
```

This is the single biggest launch blocker.

#### Root cause

- `tarn init` creates:
  - `tests/health.tarn.yaml`
  - `tarn.env.yaml`
  - `tarn.config.yaml`
- env resolution loads `tarn.env.yaml` relative to the test file’s parent directory.
- generated env file lives at project root, not under `tests/`.
- `tarn.config.yaml` exists, but the runtime path resolution does not appear to use it for env file lookup or test root behavior.

#### UX implication

A user who follows the Quick Start exactly gets a broken project immediately.

That is a GitHub launch trust-killer and a `Show HN` killer.

---

## 2. AI-Native Claim Validation

### 2a. JSON output for diagnosis

Status: `NEEDS WORK`

#### What works well

I generated a real failing JSON report against the local demo server:
- one step
- expected `404`
- actual `200`
- request and response included
- compact structure
- `failure_category: assertion_failed`

This is very good LLM input. It is narrower and more diagnosis-friendly than general-purpose test JSON.

#### What does not work

For runtime failures such as connection refused, `tarn run --format json` currently prints a plain stderr error and exits `3`. No JSON document is produced.

That breaks the AI-native story in the exact cases where agents most need structure.

#### Tarn vs pytest JSON vs Playwright JSON

| Format | Diagnostic quality for API failure |
|---|---|
| Tarn failure JSON | best when it exists |
| pytest-json-report | richer but noisier; optimized for test framework semantics, not API steps |
| Playwright JSON | powerful but too broad/noisy for CLI API-only diagnosis |

#### Conclusion

Tarn has the best AI-oriented failure shape of the set, but only for step failures that complete into a `StepResult`.

---

### 2b. Generation test

Status: `READY`

For LLM generation, Tarn’s DSL is easier than pytest or Playwright because:
- fewer structural tokens
- no host language boilerplate
- native captures and assertions
- obvious request-response mental model

Likely one-shot model mistakes:
- using `assertions:` instead of `assert:`
- inventing auth-specific shorthand that Tarn does not have
- wrong indentation under `body`
- using unsupported output assumptions for runtime errors

For simple CRUD endpoint prompts, Tarn should usually beat pytest and Playwright in first-pass validity.

---

### 2c. MCP integration

Status: `NEEDS WORK`

What exists:
- separate `tarn-mcp` binary
- direct tool surface: `tarn_run`, `tarn_validate`, `tarn_list`
- README and `AGENTS.md` mention Claude Code/Cursor/Windsurf

What is missing:
- public recorded workflow demo
- explicit proof story: prompt -> generated test -> failing run -> agent diagnosis -> fixed test/code

#### Strategic note

MCP is a real differentiator. It is not fluff. But it is only launch leverage if you show it working.

---

### 2d. `CLAUDE.md` and `AGENTS.md`

Status: `NEEDS WORK`

Strong points:
- compact syntax reference
- examples
- capture/assertion coverage
- MCP setup

Missing points:
- explicit troubleshooting recipes
- guidance for runtime network failures
- guidance for non-JSON responses
- guidance for retries/polling when async APIs are flaky
- example prompts for typical agent workflows

Recommended addition:

```text
If tarn exits with code 3 and no JSON is produced:
1. Check URL interpolation
2. Check server availability
3. Retry with --verbose
4. If response is HTML/plain text, treat body assertions accordingly
```

---

## 3. Competitive Positioning Review

### 3a. New players / market change

Status: `NEEDS WORK`

#### What changed materially

- Postman Agent Mode is now real enough to be considered a threat to Tarn’s AI-native messaging.
- MCP has become a recognized integration surface, which helps Tarn’s story.
- I did not find a clearly dominant direct MCP-native API testing competitor among Hurl/Bruno/StepCI/Tavern/runn docs.

#### Threat assessment

| Competitor / trend | Threat level |
|---|---|
| Postman Agent Mode | high messaging threat |
| Bruno broadening into gRPC/WebSocket/import/auth | high product threat |
| Hurl continuing maturity/install leadership | medium-high |
| bespoke MCP API-testing utilities | low currently |

---

### 3b. Positioning test

Status: `READY`

The phrase:

`API testing that AI agents can write, run, and debug`

works.

It is understandable in under 30 seconds and is more specific than generic “AI-native” copy.

What needs tightening is not the slogan but the proof:
- working first-run path
- fully structured runtime failures
- one visible MCP demo

---

### 3c. Unique selling points review

Status: `NEEDS WORK`

| Candidate USP | Assessment |
|---|---|
| MCP server for API testing | `REAL USP` |
| Lua scripting + YAML assertions in one tool | useful, but Lua safety concerns weaken marketing value |
| Named cookie jars | `REAL USP`, narrow but credible |
| Type-preserving captures | good DX, not headline |
| JSON failure taxonomy | good idea, currently underimplemented in runtime flow |
| Benchmarking + testing in one binary | `REAL USP` |
| 5 output formats | good parity feature, not USP by itself |
| `AGENTS.md` / `CLAUDE.md` in repo | supporting proof, not moat |

---

## 4. Launch Strategy Review

### 4a. Naming

Status: `BLOCKER`

Problems:
- repo/docs still mix `hive` and `tarn`
- `tarn` already exists as an npm package name
- `Tarn` also appears as a separate product/brand in other software contexts

This does not force a rename, but it does force a decision:
- either fully commit to `tarn` everywhere before launch
- or rename before launch

Do not launch with the current split.

---

### 4b. GitHub repository readiness

Status: `NEEDS WORK`

Present:
- LICENSE
- CI workflow
- release workflow
- action metadata
- examples

Missing or not evidenced in repo contents:
- issue templates
- PR template
- `CONTRIBUTING.md`
- `CHANGELOG.md`
- social preview image
- repo topics/tags
- discussions configuration

---

### 4c. Hacker News launch

Status: `NEEDS WORK`

Suggested title:

`Show HN: Tarn, a single-binary API test runner built for Claude/Cursor workflows`

Good backup title:

`Show HN: Tarn, YAML API tests with machine-readable failures for AI agents`

Likely comment objections:
- why not Hurl?
- why not Bruno?
- why no OpenAPI import?
- why no first-class auth?
- why no Windows?
- is Lua safe?
- does JSON really cover all failure modes?

Best response style:
- narrow scope
- admit intentional omissions
- show real differentiation
- avoid “Postman alternative” framing

Timing guidance:
- recent HN analysis suggests weekend posts outperform weekdays for breakout odds
- best overall windows include Sunday `11:00-16:00 UTC`, with `12:00 UTC` especially strong
- weekday best is roughly `11:00-13:00 UTC`

---

### 4d. Other channels

Status: `READY`

Recommended sequence:

1. GitHub release
2. short demo clip
3. `Show HN`
4. focused posts to:
   - `r/rust`
   - `r/programming` only if framing is genuinely novel
   - `r/webdev` if highlighting API workflow and local-first nature
   - `r/devops` if emphasizing CI/single-binary/JUnit/TAP
5. dev.to / Hashnode article:
   - “Why AI agents need machine-readable API test failures”
   - not “I built another Postman alternative”

---

## 5. Critical flaws and risks

### 5a. Showstopper bugs / product correctness

Status: `BLOCKER`

| Scenario | Current behavior | Assessment |
|---|---|---|
| Non-JSON response | stored as string body | `READY` |
| Response >10MB | no hard evidence | `NEEDS WORK` |
| Invalid SSL cert | likely plain runtime error, not JSON structured | `NEEDS WORK` |
| Connection refused | actionable stderr, no JSON document | `BLOCKER` for AI-native claim |
| YAML syntax error | clear parse message | `READY` |
| JSONPath missing | graceful capture failure with helpful hints | `READY` |
| Capture returns `null` | type-preserved, likely okay | `READY` |
| `tarn init` first run | broken | `BLOCKER` |

### 5b. Performance concerns

Status: `READY`

What I measured locally:
- validating 100 tiny files: about `0.01s`
- validating 1000 tiny files: about `0.05s`
- validating one 150-step YAML file: effectively instant at this scale

This is only a smoke signal, not a benchmark suite. But there is no obvious parsing scalability alarm from local validation behavior.

Unknowns:
- memory behavior in long-running watch mode
- behavior under many parallel live HTTP runs
- huge response bodies

### 5c. Security concerns

Status: `NEEDS WORK`

| Concern | Assessment |
|---|---|
| Secrets redacted in JSON | yes |
| Secrets redacted in HTML | not clearly yes; likely no |
| Lua sandboxing | weak; plain `Lua::new()` raises concern |
| `curl | sh` installer | acceptable only if repo/release naming is consistent and release integrity story is clear |

### 5d. Platform coverage

Status: `NEEDS WORK`

| Platform | State |
|---|---|
| Linux x86_64 | release workflow present |
| Linux arm64 | release workflow present |
| macOS Intel | release workflow present |
| macOS Apple Silicon | release workflow present |
| Windows | absent |
| Alpine / musl | absent |

### 5e. Missing table stakes

Status: `READY`

Present:
- `--help`
- `--version`
- shell completions
- exit codes documented in README

Weak spots:
- `list --tag` CLI exposes a tag option in clap, but main dispatch currently ignores it
- `tarn.config.yaml` exists, but project-level config does not appear meaningfully wired into runtime path/env behavior

---

## 6. OpenAPI as a pre-launch feature

Status: `NEEDS WORK`

Conclusion:
- not required for a 0.1 launch
- required soon after launch if Tarn wants serious comparison wins against StepCI/Bruno/runn

Messaging recommendation:
- do not pretend it exists
- put it visibly in roadmap
- mention it proactively in launch discussions

---

## 7. Auth as a pre-launch feature

Status: `NEEDS WORK`

Manual headers are enough for:
- Bearer token
- API key header
- Basic auth if user precomputes header or uses server support patterns

What users will miss:
- ergonomic OAuth2
- standardized auth blocks
- easier onboarding from existing tool mental models

Most important auth flows to prioritize:
1. Bearer token
2. API key
3. Basic auth
4. OAuth2 client credentials

---

## 8. Recommendations

### 8a. Go / No-Go

Status: `CONDITIONAL GO`

#### Today

`No-Go` for a broad public launch or `Show HN` today.

#### After a short fix cycle

`Go` for a narrowly framed 0.1 launch if the top launch blockers are fixed.

---

### 8b. Top 5 actions before launch

| Action | Why | Effort |
|---|---|---|
| Fix `tarn init` to create a genuinely runnable project | first impression | `4-6h` |
| Emit JSON for runtime errors in `--format json` | core AI-native promise | `6-10h` |
| Unify public naming and release/update/install references | trust + install | `4-8h` |
| Add secret redaction to HTML or remove raw headers there | security credibility | `2-4h` |
| Add `CONTRIBUTING.md`, `CHANGELOG.md`, issue templates, and one recorded demo | repo readiness | `4-8h` |

### 8c. Top 5 things that can wait

| Item | Reason |
|---|---|
| OpenAPI import | important, but not required for first public release |
| First-class auth UX | manual headers are enough for early adopters |
| Faker/data factories | useful, not launch-critical |
| gRPC/WebSocket | outside core 0.1 scope |
| Migration tools | post-launch growth feature |

### 8d. Risk mitigation

| Risk | Mitigation |
|---|---|
| Broken first run | fix scaffold, add CI smoke test that runs generated project |
| Runtime failures not in JSON | catch HTTP/script/runtime failures into structured `StepResult` or top-level failure envelope |
| Branding confusion | choose `tarn` or rename, then update README/install/action/updater/release paths together |
| Secret leakage in reports | centralize report redaction and test every format |
| Lua safety criticism | document clearly, consider opt-in flag or safer mode |
| Windows criticism | either add Windows assets or explicitly state “macOS/Linux first” in launch copy |

### 8e. Success metrics

| Horizon | Strong result | Weak result |
|---|---|---|
| 1 day | `50-150` stars | under `20` stars |
| 1 week | `200-500` stars | under `75` stars |
| 1 month | `500-1500` stars | under `200` stars |

Community health:
- week 1: `5-15` substantive issues
- month 1: `1-3` external PRs
- visible “I used Tarn with Claude/Cursor” mentions

Failure modes:
- launch discussion fixates on broken install
- first-run issues dominate issues list
- AI-native claim is challenged by non-JSON runtime failures

---

## Local code review findings behind the verdict

### 1. `tarn init` creates a broken project

Files:
- [tarn/src/main.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/main.rs)
- [tarn/src/env.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/env.rs)

Problem:
- scaffold writes root `tarn.env.yaml`
- runtime resolves env relative to each test file’s directory
- fresh `tarn run` leaves `{{ env.base_url }}` unresolved

Severity: `BLOCKER`

### 2. JSON output does not cover runtime network failures

Files:
- [tarn/src/runner.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/runner.rs)
- [tarn/src/http.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/http.rs)
- [README.md](/Users/nazarkalituk/Documents/hive-api-test/README.md)

Problem:
- HTTP errors bubble out before report rendering
- `--format json` prints stderr text instead of JSON

Severity: `BLOCKER`

### 3. Public naming is inconsistent across docs/scripts/updater

Files:
- [README.md](/Users/nazarkalituk/Documents/hive-api-test/README.md)
- [install.sh](/Users/nazarkalituk/Documents/hive-api-test/install.sh)
- [tarn/src/update.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/update.rs)
- [action.yml](/Users/nazarkalituk/Documents/hive-api-test/action.yml)

Problem:
- `hive` and `tarn` are both live in public-facing surfaces

Severity: `BLOCKER`

### 4. HTML report likely leaks secrets

Files:
- [tarn/src/report/json.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/report/json.rs)
- [tarn/src/report/html.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/report/html.rs)

Problem:
- JSON path redacts request headers
- HTML renderer appears to print headers directly

Severity: `NEEDS WORK`

### 5. `tarn.config.yaml` is mostly decorative today

Files:
- [tarn/src/main.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/main.rs)
- [tarn/src/config.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/config.rs)

Problem:
- config parsing exists
- runtime path/env behavior does not appear to rely on it meaningfully

Severity: `NEEDS WORK`

### 6. `list --tag` option is parsed but ignored in dispatch

File:
- [tarn/src/main.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/main.rs)

Problem:
- clap defines a tag option for `list`
- main dispatch drops it with `Commands::List { tag: _ } => list_command()`

Severity: `NEEDS WORK`

### 7. Lua scripting is powerful but not safely bounded

File:
- [tarn/src/scripting.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/scripting.rs)

Problem:
- security objections are likely at launch

Severity: `NEEDS WORK`

---

## Final recommendation

Decision: `CONDITIONAL GO`

Top 3 reasons:
1. First-run path is currently broken.
2. Distribution and naming are inconsistent enough to undermine launch trust.
3. Tarn’s strongest AI-native claim is only partially true because runtime failures do not produce structured JSON.

If those 3 issues are fixed, Tarn is good enough for an intentionally narrow 0.1 launch.

Recommended launch framing:

`Tarn is a single-binary YAML API test runner for AI-assisted workflows. It is strongest today for REST/GraphQL teams who want machine-readable failures, local-first execution, and a direct path into Claude/Cursor via MCP.`

Avoid this framing for now:

`Postman alternative`

---

## Sources

Primary external sources used for competitive verification:

- Hurl home: https://hurl.dev/index.html
- Hurl installation: https://hurl.dev/docs/installation.html
- Hurl captures: https://hurl.dev/docs/capturing-response.html
- Hurl GraphQL request docs: https://hurl.dev/docs/request.html
- StepCI CLI docs: https://docs.stepci.com/reference/cli.html
- StepCI matchers: https://docs.stepci.com/reference/matchers.html
- Bruno auth docs: https://docs.usebruno.com/auth/oauth2/authorization-code
- Bruno import docs: https://docs.usebruno.com/get-started/import-export-data/import-collections
- Tavern docs: https://tavern.readthedocs.io/en/latest/
- runn README: https://github.com/k1LoW/runn
- Postman Agent Mode: https://www.postman.com/templates/agent-mode/
- pytest-json-report: https://github.com/numirias/pytest-json-report
- HN timing analysis: https://www.myriade.ai/blogs/when-is-it-the-best-time-to-post-on-show-hn/
- npm package name conflict (`tarn`): https://www.npmjs.com/package/tarn/v/0.1.4?activeTab=dependents
- Product Hunt name collision context: https://www.producthunt.com/products/tarn

Local verification sources:

- [README.md](/Users/nazarkalituk/Documents/hive-api-test/README.md)
- [tarn/src/main.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/main.rs)
- [tarn/src/env.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/env.rs)
- [tarn/src/http.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/http.rs)
- [tarn/src/runner.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/runner.rs)
- [tarn/src/report/json.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/report/json.rs)
- [tarn/src/report/html.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/report/html.rs)
- [tarn/src/scripting.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/scripting.rs)
- [tarn/src/update.rs](/Users/nazarkalituk/Documents/hive-api-test/tarn/src/update.rs)
- [install.sh](/Users/nazarkalituk/Documents/hive-api-test/install.sh)
- [action.yml](/Users/nazarkalituk/Documents/hive-api-test/action.yml)
- [.github/workflows/ci.yml](/Users/nazarkalituk/Documents/hive-api-test/.github/workflows/ci.yml)
- [.github/workflows/release.yml](/Users/nazarkalituk/Documents/hive-api-test/.github/workflows/release.yml)
