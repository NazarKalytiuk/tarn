# Tarn — Pre-Release Task List

Date: 2026-03-29

Purpose: one consolidated release backlog created from:
- [RELEASE_READINESS_REVIEW_2026-03-29.md](/Users/nazarkalituk/Documents/hive-api-test/RELEASE_READINESS_REVIEW_2026-03-29.md)
- [MARKET_RESEARCH.md](/Users/nazarkalituk/Documents/hive-api-test/MARKET_RESEARCH.md)
- [MARKET_RESEARCH_BRIEF_2026-03-28.md](/Users/nazarkalituk/Documents/hive-api-test/MARKET_RESEARCH_BRIEF_2026-03-28.md)
- [MARKET_RESEARCH_FULL_2026-03-28.md](/Users/nazarkalituk/Documents/hive-api-test/MARKET_RESEARCH_FULL_2026-03-28.md)
- [SYNTHESIS.md](/Users/nazarkalituk/Documents/hive-api-test/SYNTHESIS.md)
- [ROADMAP.md](/Users/nazarkalituk/Documents/hive-api-test/ROADMAP.md)
- [RETROSPECTIVE-v2.md](/Users/nazarkalituk/Documents/hive-api-test/RETROSPECTIVE-v2.md)
- [RETROSPECTIVE-v3.md](/Users/nazarkalituk/Documents/hive-api-test/RETROSPECTIVE-v3.md)

Release target assumed by this file:
- public GitHub launch
- release assets for end users
- launch post on Hacker News

Priority legend:
- `P0` = must complete before public release
- `P1` = strongly recommended before release
- `P2` = optional before release, okay to defer if schedule is tight

Definition of ready to launch:
- all `P0` tasks done
- at least the highest-value `P1` tasks done
- launch artifacts prepared

---

## P0 — Must Complete Before Public Release

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P0-01 | Finalize naming strategy and normalize all public surfaces | Decide whether product/repo/package stays `tarn` or is renamed again; remove all `hive`/mixed naming from README, install/update scripts, Action, schema URLs, badges, release URLs, examples | release review, synthesis, roadmap |
| P0-02 | Fix `tarn init` so a fresh project actually runs | In a clean temp directory: `tarn init && tarn run` succeeds or fails only because the sample server is intentionally absent, not because interpolation/config is broken | release review |
| P0-03 | Wire project config/env resolution correctly | `tarn.config.yaml`, `tarn.env.yaml`, `tarn.env.{name}.yaml`, and `tarn.env.local.yaml` behave predictably from project root and nested test directories; add tests covering root vs nested files | release review |
| P0-04 | Make `--format json` work for runtime failures, not only assertion failures | Connection refused, timeout, SSL failure, script error, and parse/runtime execution failures produce structured JSON in JSON mode, with stable failure categories | release review, roadmap, synthesis |
| P0-05 | Freeze and document the JSON result contract | JSON shape is explicitly versioned, documented, and covered by contract tests; publish machine-readable schema for output, not just input test files | roadmap, synthesis, release review |
| P0-06 | Redact secrets in every output surface | Authorization, Cookie, Set-Cookie, API keys, tokens, and similar headers are masked in JSON, HTML, JUnit, TAP, verbose output, and any future AI-facing payloads | release review, roadmap |
| P0-07 | Decide the Lua safety story | Either restrict/sandbox Lua meaningfully or document it as an unsafe escape hatch with explicit wording in README/spec/docs and examples; no ambiguous security claims | release review |
| P0-08 | Align install/update/release pipeline with the chosen name and release channel | `install.sh`, `tarn update`, GitHub Action install script, release asset names, badges, and README install commands all point to the same repo/releases and actually work | release review |
| P0-09 | Add launch-blocking smoke tests to CI | CI must cover: generated project from `tarn init`, JSON assertion failure path, JSON connection-failure path, install/update path smoke, and at least one end-to-end demo-server run | release review |
| P0-10 | Fix the CLI credibility gaps already visible in code | `list --tag` actually filters; `tarn.config.yaml` is either fully wired or removed from user-facing docs; no dead/demo-only surface remains in core commands | release review |
| P0-11 | Correct README quick-start and examples so they match shipped behavior | Every install command, URL, badge, schema link, and first-run example is verified against the real binaries/scripts | release review |
| P0-12 | Prepare a working public release package | GitHub release contains the documented binaries, install instructions, checksums or verification guidance, and changelog/release notes | release review, roadmap |

---

## P1 — Strongly Recommended Before Release

### Product / Core UX

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P1-01 | Implement OpenAPI scaffold/import, or explicitly cut scope | Preferably ship `tarn init --from openapi.yaml`; if not shipping, state clearly in README/roadmap/launch post that it is next on the roadmap | roadmap, synthesis, market research, release review |
| P1-02 | Ship first-class auth fundamentals | Support or document ergonomic flows for Bearer token, API key, Basic auth, and OAuth2 client_credentials; examples exist for all supported modes | roadmap, synthesis, release review |
| P1-03 | Improve error surfaces to “agent-loop quality” | Failure output consistently includes expected vs actual, request/response excerpts, path/field context, and actionable hints | roadmap, release review |
| P1-04 | Add `.env` file support or clearly document the env strategy | Users can load secrets without hacks, or docs explicitly say shell env + YAML env files are the supported path | roadmap |
| P1-05 | Add native regex body capture for text/HTML responses | Mailpit-like token extraction works without Lua for common cases; docs show how to capture from plain text/HTML bodies | retrospective v3 |
| P1-06 | Improve root-cause attribution for failed setup chains | When setup fails and downstream steps/captures break, error output points to the originating setup failure instead of a confusing secondary JSONPath miss | retrospective v3 |
| P1-07 | Decide Windows support policy | Either ship Windows binaries and test them or state explicitly that v0.1 is macOS/Linux-first | release review, roadmap |
| P1-08 | Decide package-manager strategy | If crates.io/Homebrew are feasible, ship them; otherwise make the public install story explicit and coherent | release review, market research |

### AI-Native Workflow / MCP

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P1-09 | Record one polished AI workflow demo | Demo shows: generate tests -> run Tarn -> inspect structured failure -> fix -> rerun green; can be video, transcript, or both | synthesis, market research, release review |
| P1-10 | Strengthen MCP story in docs | README, `AGENTS.md`, and `CLAUDE.md` show concrete Claude Code/Cursor workflow, not just setup JSON snippets | release review, market research |
| P1-11 | Add troubleshooting playbooks for agents | Document what agents should do on parse errors, runtime errors, non-JSON responses, retries, auth failures, and capture misses | release review |
| P1-12 | Consider exposing `tarn_init` / scaffold functionality in MCP | If OpenAPI scaffold exists, MCP should expose it; if not, keep the MCP surface minimal and clearly documented | roadmap, market research |

### Quality / Security / Reliability

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P1-13 | Expand integration coverage for real-world edge cases | Add tests for Unicode bodies, invalid SSL, redirects, empty responses, non-JSON bodies, and large-response behavior | release review |
| P1-14 | Add a watch-mode soak or reliability check | No obvious memory/cpu leak or runaway watcher behavior in a longer-running watch test | release review |
| P1-15 | Add large-suite scale smoke tests | Validate and/or run hundreds of files in CI or nightly to catch regression in file discovery, parallelism, and reporting | release review |
| P1-16 | Review HTML report for safe disclosure | Ensure HTML report does not leak sensitive headers/body fragments by default | release review |
| P1-17 | Review installer safety and messaging | `curl | sh` path explains what it downloads, where it installs, and how to verify a release | release review |

### Docs / Repository Hygiene

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P1-18 | Add `CONTRIBUTING.md` | Basic dev setup, coding conventions, test expectations, and release workflow are documented | release review |
| P1-19 | Add `CHANGELOG.md` | Release notes are cumulative and visible to users outside GitHub release pages | release review |
| P1-20 | Add issue templates and PR template | Bug report, feature request, and regression report templates exist in `.github/` | release review |
| P1-21 | Add/verify repo metadata | Description, topics, social preview image, and badges are ready for launch | release review, market research |
| P1-22 | Curate 3 launch-grade examples | At minimum: health check, CRUD with captures, CI/auth example; all examples are runnable and documented | synthesis |
| P1-23 | Add migration/onboarding docs | At minimum document “from curl” and “from OpenAPI” paths; if automation is not ready, provide manual recipes | synthesis, market research |
| P1-24 | Document platform and scope boundaries | Be explicit about what Tarn is not: not a Postman replacement, not a full QA platform, not all-protocol coverage | synthesis, market research |

### Launch Assets / Go-To-Market

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P1-25 | Draft launch narrative and positioning copy | Use one consistent line such as `API testing that AI agents can write, run, and debug`; avoid YAML-first or generic “Postman alternative” copy | synthesis, release review |
| P1-26 | Draft the `Show HN` post | Include title, short intro, real demo, and direct answers to “why not Hurl/Bruno/Postman?” | release review, market research |
| P1-27 | Prepare a short demo clip/GIF | 30-60 second visual asset for GitHub, HN comments, X, and blog posts | market research, synthesis |
| P1-28 | Prepare comparison content | Publish “Why not Hurl?”, “Why not Bruno?”, “Why not Playwright?”, or a similar migration/comparison page | market research |
| P1-29 | Prepare channel-specific posts | Have tailored launch text for GitHub, HN, Reddit (`r/rust`, `r/webdev`, `r/devops`), and dev.to/Hashnode | release review, market research |
| P1-30 | Prepare MCP ecosystem distribution | If MCP story is a launch pillar, queue submissions to relevant MCP directories/awesome lists after release | market research |

---

## P2 — Optional Before Release, Safe To Defer If Schedule Is Tight

### Product Features

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P2-01 | Add OpenAPI response schema validation | Responses can be validated against an OpenAPI spec, or a clear design exists for it | roadmap, release review |
| P2-02 | Add richer test data helpers / faker-like built-ins | Users can generate realistic emails/names/timestamps without custom scripts | roadmap |
| P2-03 | Add migration tools | Convert from curl/Postman/Hurl/OpenAPI where feasible | roadmap, market research |
| P2-04 | Add Windows release assets if not already done under P1 | Official binaries exist and are tested on CI | release review |
| P2-05 | Add Homebrew formula if not already done under P1 | `brew install` path is real and documented | release review, market research |
| P2-06 | Add Alpine/musl build if targeting containers heavily | musl binary exists and is smoke-tested | release review |

### Quality / DX

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P2-07 | Skip or ignore shared include files during `tarn validate` | Include-only files no longer show up as confusing validation failures, or `.tarnignore` exists | retrospective v3 |
| P2-08 | Add `.tarnignore` or shared-file convention | Users can cleanly exclude helper files from file discovery/validation | retrospective v3 |
| P2-09 | Add optional between-file delay / flush hook | Useful for rate-limited APIs or shared-test-environment cleanup between files | retrospective v3 |
| P2-10 | Improve multi-user cookie workflow ergonomics | Reduce verbosity for two-user flows without breaking current named-jar design | retrospective v3 |

### Launch / Ecosystem

| ID | Task | Acceptance Criteria | Sources |
|---|---|---|---|
| P2-11 | Publish GitHub Action improvements | Action is launch-ready, documented, and consistent with final naming/env conventions | release review, roadmap |
| P2-12 | Prepare package-registry discoverability work | crates.io story, Homebrew tap, MCP registry listing, awesome-lists submissions are queued | market research |

---

## Recommended Execution Order

### Phase 1 — Remove launch blockers

- P0-01 through P0-12

### Phase 2 — Strengthen product credibility

- P1-01 through P1-08
- P1-13 through P1-17

### Phase 3 — Strengthen the AI-native launch story

- P1-09 through P1-12
- P1-25 through P1-30

### Phase 4 — Nice-to-have polish

- all `P2` items as schedule allows

---

## Release Gate Checklist

Use this right before publishing:

- [ ] All `P0` tasks complete
- [ ] README verified against real binaries/scripts
- [ ] `tarn init` smoke test passes
- [ ] JSON mode verified for assertion failures and runtime failures
- [ ] Secret redaction verified across all output formats
- [ ] Release artifacts uploaded and install script tested
- [ ] AI demo recorded and linked
- [ ] Launch copy drafted
- [ ] Issue templates / changelog / contributing docs added
- [ ] Scope statement is explicit: what Tarn is for, and what it is not for

---

## Notes

- If schedule is tight, do not silently skip `P0` work. Narrow the launch instead.
- If OpenAPI import/auth are not ready, say so openly and keep the launch story focused on the working wedge.
- Do not broaden scope to “general API platform” for the first public release.
