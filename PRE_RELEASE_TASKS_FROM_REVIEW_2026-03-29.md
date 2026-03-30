# Tarn — Pre-Release Tasks From Release Readiness Review

Source of truth for this file:
- [RELEASE_READINESS_REVIEW_2026-03-29.md](/Users/nazarkalituk/Documents/hive-api-test/RELEASE_READINESS_REVIEW_2026-03-29.md)
- [RELEASE-READINESS.md](/Users/nazarkalituk/Documents/hive-api-test/RELEASE-READINESS.md)

Scope:
- only tasks that come directly from the current release-readiness review
- no extra market-research backlog
- no broad roadmap items unless they were explicitly called out in the review as pre-release work

Priority legend:
- `P0` = must fix before public release / `Show HN`
- `P1` = should fix before release if possible
- `P2` = can be deferred, but was explicitly noted in the review

## Progress Update (2026-03-30)

Completed in this pass:
- `P0-01` Fix `tarn init` first-run flow
- `P0-02` Fix project env/config path behavior
- `P0-03` Make JSON output work for runtime failures
- `P0-04` Make JSON contract truthful and stable
- `P0-05` Lock down the Lua sandbox
- `P0-06` Normalize all `hive` vs `tarn` references and rename repo surface
- `P0-09` Add release-blocking smoke tests to CI
- `P0-10` Verify and fix README quick start
- `P1-01` Fix `list --tag` behavior
- `P1-02` Fully wire `tarn.config.yaml` defaults and parallel behavior
- `P1-03` Add coverage for critical edge cases
- `P1-04` Add a self-contained hello-world path
- `P1-05` Add troubleshooting guidance for AI agents
- `P1-06` Record one publishable AI workflow demo transcript
- `P1-07` Strengthen MCP documentation with a real workflow
- `P1-08` Add release hygiene docs
- `P1-11` Add macOS release verification in CI
- `P1-10` Add installer checksum/integrity verification
- `P2-01` Document OpenAPI as explicitly deferred
- `P2-02` Document auth workarounds and scope boundaries
- `P2-03` Add watch-mode verification guidance
- `P2-04` Add larger-scale suite validation smoke coverage
- `P2-05` Improve installer safety messaging
- `P2-06` Cache/reuse compiled regexes in hot paths
- `P2-07` Reuse HTTP client across steps
- `P2-09` Prepare channel-specific launch submission drafts

Partially completed, but still needs live release work:
- `P0-07` Fix install/update/release pipeline end-to-end
- `P0-08` Publish and verify the Rust install path
- `P0-11` Prepare a coherent release package
- `P1-09` Add issue templates / PR template / repo metadata
- `P2-08` Prepare launch messaging assets

Verified locally after these changes:
- `cargo test -q -p tarn --lib --bins` passes (`367` lib tests, `5` bin tests)
- `cargo test -p tarn --test integration_test` passes (`21` integration tests)
- `bash scripts/ci/smoke.sh` passes end to end

Still open from the review:
- live crates.io publish and `cargo install tarn` verification
- real GitHub release asset validation (`install.sh`, `tarn update`, checksums, release notes)
- GitHub repo settings that cannot be changed from files alone (description, topics, social preview, Discussions)
- recording/publishing the actual launch demo clip

---

## P0 — Must Fix Before Public Release

| ID | Task | Why | Acceptance Criteria |
|---|---|---|---|
| P0-01 | Fix `tarn init` first-run flow | Current scaffold does not run cleanly out of the box; this is the biggest launch blocker | In a clean temp dir, `tarn init && tarn run` works as documented, with env interpolation resolving correctly |
| P0-02 | Fix project env/config path behavior | `tarn.env.yaml` / `tarn.config.yaml` contract is currently broken or misleading for generated projects | Root-level and nested test files resolve env/config exactly as docs describe; covered by tests |
| P0-03 | Make JSON output work for runtime failures | `--format json` currently does not emit structured JSON for connection/timeout/runtime failures | Connection refused, timeout, SSL/runtime errors emit structured JSON with failure category instead of plain stderr only |
| P0-04 | Make JSON contract truthful and stable | The AI-native claim depends on a reliable machine-readable result contract | JSON schema/shape is stable, versioned, and matches actual runtime behavior |
| P0-05 | Lock down the Lua sandbox | `RELEASE-READINESS.md` flags current `Lua::new()` behavior as a release blocker because `script:` can execute arbitrary commands | Lua runtime is restricted to the intended safe stdlib surface, documented, and covered by tests proving dangerous stdlib paths are unavailable |
| P0-06 | Normalize all `hive` vs `tarn` references and rename repo surface | Branding/install/update/docs are inconsistent and undermine trust; `RELEASE-READINESS.md` explicitly calls out repo naming as a blocker | README, badges, schema URLs, install script, updater, GitHub Action, release names, examples, and GitHub repo naming all use one final name and one repo path |
| P0-07 | Fix install/update/release pipeline end-to-end | Current release/install surface is inconsistent and may not work cleanly for users | `install.sh`, `tarn update`, release artifacts, and GitHub Action install path all work against the real published repo/releases |
| P0-08 | Publish and verify the Rust install path | `RELEASE-READINESS.md` explicitly treats missing `cargo install tarn` as a launch blocker for Rust users | crates.io publishing is live if chosen, `cargo install tarn` works, version matches release, and README reflects the real install matrix |
| P0-09 | Add release-blocking smoke tests to CI | Public launch should not rely on manually verified first-run/install/output behavior | CI covers generated scaffold run, JSON failure output, runtime JSON failure output, and at least one demo-server end-to-end run |
| P0-10 | Verify and fix README quick start | README currently overpromises and includes stale/misaligned install references | Every quick-start/install/example command in README is tested against the real binaries and repo layout |
| P0-11 | Prepare a coherent release package | Public release needs a trustworthy install and artifact story | GitHub release contains the binaries documented in README, with matching names, working install path, and release notes |

---

## P1 — Strongly Recommended Before Release

| ID | Task | Why | Acceptance Criteria |
|---|---|---|---|
| P1-01 | Fix `list --tag` behavior | Review found the CLI accepts `list --tag` but main dispatch ignores it | `tarn list --tag ...` actually filters output and has tests |
| P1-02 | Either fully wire `tarn.config.yaml` or reduce its public prominence | Right now it looks partly decorative, which damages credibility | Config is either fully used in runtime behavior or docs are changed to reflect the real support level |
| P1-03 | Add/expand tests for critical edge cases | Review found weak evidence for Unicode, invalid SSL, large responses, and similar boundaries | Integration or focused tests exist for Unicode, non-JSON body, empty response, redirects, invalid SSL, and large-response handling |
| P1-04 | Add a self-contained “hello world” path | `RELEASE-READINESS.md` notes that first-run still lacks a wow moment without an external API/server | `tarn init` template or examples include at least one self-contained/runnable hello-world path that works without requiring users to invent their own API target |
| P1-05 | Add troubleshooting guidance for AI agents | `AGENTS.md` / `CLAUDE.md` are useful but incomplete for real failure handling | Docs include guidance for runtime errors, non-JSON responses, capture misses, retries, and diagnosis loops |
| P1-06 | Record one real AI workflow demo | The review called out the need to prove the AI-native claim with workflow, not just features | Public demo/transcript shows: generate test -> run Tarn -> inspect failure JSON -> fix -> rerun green |
| P1-07 | Strengthen MCP documentation with a real workflow | MCP exists, but proof and workflow clarity are still weak | README and agent docs show actual Claude Code/Cursor flow using `tarn_run`, not just setup snippets |
| P1-08 | Add release hygiene docs | Both review docs call out missing contribution/release docs | `CONTRIBUTING.md` and `CHANGELOG.md` exist and match current release process |
| P1-09 | Add issue templates / PR template / repo metadata | Launch readiness includes repository hygiene, not only binary functionality | `.github` templates exist; repo description, topics, badges, social preview, and Discussions setting are ready |
| P1-10 | Add installer checksum/integrity verification | `RELEASE-READINESS.md` flags missing checksum verification on `install.sh` | Release assets publish SHA256 checksums and installer or docs verify/download them clearly |
| P1-11 | Add macOS release verification in CI | `RELEASE-READINESS.md` notes that release binaries are built for macOS but not tested in CI | At least one macOS CI job validates build or smoke-runs the release artifact |

---

## P2 — Explicitly Mentioned In The Review But Can Be Deferred

| ID | Task | Why | Acceptance Criteria |
|---|---|---|---|
| P2-01 | OpenAPI import/scaffold generation | Review says it is important, but not a blocker for a narrow 0.1 launch | If not shipped, roadmap/docs explicitly say it is coming soon |
| P2-02 | First-class auth ergonomics | Manual headers are workable today, but users will ask for a better auth UX | If not shipped, docs clearly show Bearer/API key/Basic workarounds and scope boundaries |
| P2-03 | Add watch-mode reliability checks | Review flagged missing evidence around long-running watch behavior | At least one soak/smoke test or manual verification note exists |
| P2-04 | Add larger-scale suite validation smoke tests | Review found no obvious parsing scalability alarm, but stronger proof would help | Optional CI/nightly validation of larger file counts exists |
| P2-05 | Improve installer safety messaging | `curl | sh` is acceptable only if transparency is high | Installer docs explain what is downloaded, where it installs, and how to verify a release |
| P2-06 | Cache/reuse compiled regexes in hot paths | `RELEASE-READINESS.md` calls out regex recompilation as measurable overhead | Hot-path regexes use lazy/static initialization instead of recompiling on every call |
| P2-07 | Reuse HTTP client across steps where appropriate | `RELEASE-READINESS.md` calls out per-request client creation as avoidable overhead | HTTP client lifecycle is improved to reuse connections within a run/file without changing behavior |
| P2-08 | Prepare launch messaging assets | Review recommended HN/reddit/X/dev.to prep, but this is secondary to fixing product blockers | Draft launch copy exists for HN + short demo clip + one comparison page |
| P2-09 | Prepare channel-specific launch submissions | `RELEASE-READINESS.md` includes explicit launch channels beyond HN | Drafts/checklists exist for HN, `r/rust`, dev.to/Hashnode, and awesome-mcp-servers submission |

---

## Recommended Release Sequence

### Phase 1 — Remove hard blockers

- P0-01 through P0-11

### Phase 2 — Strengthen credibility

- P1-01 through P1-11

### Phase 3 — Nice-to-have before launch

- P2 items as schedule allows

---

## Release Gate

- [x] `tarn init` works from zero
- [x] env/config behavior matches docs
- [x] JSON mode works for assertion failures and runtime failures
- [x] Lua sandbox is restricted to the intended safe surface
- [ ] `cargo install tarn` works, or docs explicitly use a different supported install path
- [x] naming/repo/install/update/release paths are fully consistent in the repo
- [x] CI covers first-run and structured-failure smoke tests
- [x] README quick start is verified against real binaries
- [ ] release artifacts exist and match install docs
