# Tarn: Features & Improvements Roadmap

Based on synthesis of 4 market research documents (2026-03-28).

---

## Tier 1 — Must-have before public launch

These features close the gap between "working tool" and "credible product". Without them, Tarn risks being ignored as "yet another YAML tester."

### 1. Rename the project

**Why:** "Hive" (old name) collided with Apache Hive, Hive AI ($2B), Hive Blockchain, GraphQL Hive, Hive ransomware. The crate on crates.io is taken. SEO for "hive API testing" was unwinnable. Renamed to Tarn..
**What:** Pick a short, unique, Googleable name. Good patterns: `hurl`, `bruno`, `rg`, `bat`, `k6`.
**Effort:** Low (naming) + Medium (rename everything)
**Impact:** Critical for discoverability and brand

### 2. Stable JSON output schema (versioned)

**Why:** This is core to the AI-native claim. LLM agents need a reliable contract to parse results. Currently JSON output may change between versions.
**What:**
- Version the JSON schema (v1)
- Publish JSON Schema file
- Include full request/response context ONLY for failed steps
- Structured failure taxonomy: `assertion_failed`, `connection_error`, `timeout`, `parse_error`
- Redact secrets in output (`Authorization: Bearer ***`)
**Effort:** Medium
**Impact:** High — without this, "AI-native" is an empty claim

### 3. OpenAPI import / test scaffold generation

**Why:** Biggest onboarding friction reducer. Both research reports rank this as Tier 1. LLM workflow starts here: give it an OpenAPI spec -> generate tests -> run -> iterate.
**What:**
- `tarn init --from openapi.yaml` — generates `.tarn.yaml` files for each endpoint
- Cover all paths/methods with basic status checks
- Generate request bodies from schema examples
- Include negative tests (missing required fields -> 4xx)
**Effort:** High
**Impact:** Very High — the bridge between "I have an API" and "I have tests"

### 4. Auth & secrets fundamentals

**Why:** Almost every real API requires auth. Without this, Tarn only works for public endpoints.
**What:**
- Bearer token (already exists via headers, but needs first-class support)
- API key (header or query param)
- Basic auth
- OAuth2 client_credentials flow (most common for service-to-service)
- Secret masking in all outputs (human + JSON)
- Support for `.env` files and `${ENV_VAR}` expansion
**Effort:** Medium
**Impact:** High — table stakes for real-world use

### 5. Excellent error surfaces

**Why:** The quality of error messages is the product experience. LLM agents parse these. Developers read these. This is where Tarn wins or loses.
**What:**
- Assertion mismatch with expected vs actual + diff
- JSONPath resolution trace (show which part of the path failed)
- Request/response excerpts in failure context
- YAML parse errors with file:line:column + suggestion
- Network errors with actionable messages ("Connection refused — is the server running on port 3000?")
- Timeout with actual vs allowed duration
**Effort:** Medium
**Impact:** Very High — this IS the agent-loop UX

---

## Tier 2 — Highest leverage after launch

These make Tarn competitive and enable real adoption at scale.

### 6. MCP server

**Why:** No API testing tool has an MCP server. This puts Tarn directly inside Claude Code, Cursor, Windsurf. First-mover advantage is real. Both reports flag this as the strongest distribution bet.
**What:**
- JSON-RPC over stdio
- Tools: `tarn_run`, `tarn_validate`, `tarn_list`, `tarn_init`
- Returns structured JSON that LLM can act on directly
- Publish to awesome-mcp-servers and MCP registry
**Effort:** Medium (500-1000 lines, core already exists)
**Impact:** Very High — distribution channel + product differentiation

### 7. Parallel test execution

**Why:** Sequential-only doesn't scale. Every competitor supports this. CI pipelines with 50+ test files need parallelism.
**What:**
- `tarn run --parallel` or `tarn run --jobs 4`
- File-level parallelism (steps within a test stay sequential)
- Thread-safe capture/env isolation per file
- Summary output aggregates results from all threads
**Effort:** Medium-High
**Impact:** High — credibility for CI/production use

### 8. GraphQL support

**Why:** 34% of teams test multiple protocols. REST-only is increasingly incomplete. Postman, Hurl, StepCI all support GraphQL.
**What:**
- `method: GRAPHQL` or `graphql:` block in step
- Query/mutation/subscription as string in body
- Variables block
- JSONPath assertions on `$.data` and `$.errors`
- Introspection query helper
**Effort:** Medium
**Impact:** Medium-High — expands addressable market

### 9. Watch mode

**Why:** Essential for iterative development with AI agents. The agent writes a test -> saves -> Tarn reruns automatically -> agent reads output.
**What:**
- `tarn run --watch` or `tarn watch`
- File system watcher on `.tarn.yaml` files
- Re-runs changed file + dependents
- Clear screen between runs
- Works with `--format json` for agent consumption
**Effort:** Low-Medium
**Impact:** Medium — improves developer experience loop

### 10. Retry & polling

**Why:** Real APIs have eventual consistency. Without retry, tests become flaky.
**What:**
- Per-step: `retry: { count: 3, delay: 1000, backoff: exponential }`
- Poll mode: `poll: { until: "$.status == 'completed'", max: 10, interval: 2000 }`
- Retry only on specific conditions (status 5xx, timeout)
- Report retries in output (attempt 1/3, 2/3, 3/3)
**Effort:** Medium
**Impact:** Medium-High — prevents false negatives in CI

---

## Tier 3 — Competitive features (3-6 months)

### 11. OpenAPI response schema validation

**Why:** Automatically verify responses match the spec, not just custom assertions. Schemathesis, Postman do this.
**What:**
- `assert: { schema: "openapi.yaml#/paths/~1users/get/responses/200" }`
- Or auto-validate if OpenAPI spec is referenced in config
- Report schema violations as structured assertion failures
**Effort:** Medium-High
**Impact:** Medium

### 12. ~~Includes / reusable blocks~~ ✅ DONE

**Shipped.** `- include: ./shared/auth-setup.tarn.yaml` in setup/teardown/steps/tests.*.steps. Resolved at parse time with circular include detection.

### 13. Test data factories / richer built-ins

**Why:** Current built-ins ($uuid, $random_hex) are primitive. Real tests need realistic data.
**What:**
- `$faker.email`, `$faker.name`, `$faker.phone` (built-in fake data)
- `$file("path/to/payload.json")` — load body from external file
- `$env("VAR", "default")` — env with fallback
- `$timestamp_offset("+1h")` — relative timestamps
- `$base64_encode(value)`, `$base64_decode(value)`
**Effort:** Medium
**Impact:** Medium — convenience for real-world tests

### 14. GitHub Action

**Why:** CI is the second-best audience. A ready-made GitHub Action reduces friction to near zero.
**What:**
```yaml
- uses: [new-name]/action@v1
  with:
    tests: tests/
    env: staging
    format: junit
```
- Auto-installs binary
- Publishes JUnit results to GitHub PR checks
- Annotation on failures
**Effort:** Low
**Impact:** Medium-High for DevOps segment

### 15. Migration tools

**Why:** Users have existing tests/collections. Migration reduces switching cost.
**What:**
- `tarn convert --from curl "curl -X POST ..."` -> generates .tarn.yaml
- `tarn convert --from postman collection.json` -> generates test files
- `tarn convert --from hurl file.hurl` -> generates .tarn.yaml
- `tarn convert --from openapi spec.yaml` -> generates test scaffolds (same as init)
**Effort:** Medium per format
**Impact:** Medium — reduces switching friction

---

## Tier 4 — Future / if traction warrants

### 16. Mock/stub server

**Why:** WireMock has 6M+ downloads/month. Useful for frontend teams testing against API contracts.
**What:** `tarn mock --from openapi.yaml` starts a local server returning example responses.
**Effort:** High
**Impact:** Medium — different use case, expands market

### 17. gRPC support

**Why:** Growing protocol, Postman/Hoppscotch/StepCI support it.
**What:** `method: GRPC`, proto file reference, message body, response assertions.
**Effort:** High
**Impact:** Medium

### 18. WebSocket testing

**Why:** Postman, Hoppscotch, Artillery support it.
**What:** Connect, send messages, assert on received messages with timeout.
**Effort:** High
**Impact:** Low-Medium

### 19. Snapshot testing / VCR

**Why:** Record real responses, replay in tests. Useful for regression testing.
**What:** `tarn record` saves responses, `tarn run --replay` uses recorded data.
**Effort:** High
**Impact:** Low-Medium

### 20. Lua / custom DSL escape hatch

**Why:** The 20% of tests that need cross-field comparison, array aggregation, complex logic.
**What:** `assert: { lua: "response.body.total == #response.body.items" }` or custom DSL.
**Effort:** Medium-High (Lua via mlua) or Medium (custom DSL)
**Impact:** Medium — prevents YAML ceiling problem

### 21. HTML report

**Why:** Managers and stakeholders want visual reports. Playwright has this.
**What:** `tarn run --format html` generates a single-file HTML report with pass/fail, timings, failure details.
**Effort:** Medium
**Impact:** Low-Medium

### 22. Cloud offering (if PMF)

**Why:** Monetization path. Scheduled runs, team dashboards, hosted test execution.
**What:** SaaS product around the open-source core.
**Effort:** Very High
**Impact:** Revenue enabler

---

## Non-features (what to deliberately NOT build)

Based on both research reports, these are anti-patterns to avoid:

1. **GUI** — don't build one. Tarn is CLI-first by design. A GUI dilutes focus.
2. **Embedded scripting language** — don't turn YAML into a programming language. If needed, add Lua as escape hatch, not inline JS/Python.
3. **Enterprise features early** — no RBAC, audit logs, SSO before proving the wedge.
4. **Too many protocols before HTTP is excellent** — nail REST first, then GraphQL, then gRPC.
5. **"Written in Rust" marketing** — frame as "zero-dependency install", not implementation language.
6. **tarn bench as priority** — benchmarking is nice-to-have, not a differentiator.

---

## Summary matrix

| # | Feature | Effort | Impact | Tier |
|---|---------|--------|--------|------|
| 1 | Rename project | Low-Med | Critical | 1 |
| 2 | Stable JSON schema | Medium | High | 1 |
| 3 | OpenAPI import/scaffold | High | Very High | 1 |
| 4 | Auth & secrets | Medium | High | 1 |
| 5 | Error surface quality | Medium | Very High | 1 |
| 6 | MCP server | Medium | Very High | 2 |
| 7 | Parallel execution | Med-High | High | 2 |
| 8 | GraphQL support | Medium | Med-High | 2 |
| 9 | Watch mode | Low-Med | Medium | 2 |
| 10 | Retry & polling | Medium | Med-High | 2 |
| 11 | OpenAPI schema validation | Med-High | Medium | 3 |
| 12 | Includes / reusable blocks | Medium | Medium | 3 |
| 13 | Data factories / built-ins | Medium | Medium | 3 |
| 14 | GitHub Action | Low | Med-High | 3 |
| 15 | Migration tools | Medium | Medium | 3 |
| 16 | Mock server | High | Medium | 4 |
| 17 | gRPC | High | Medium | 4 |
| 18 | WebSocket | High | Low-Med | 4 |
| 19 | Snapshot/VCR | High | Low-Med | 4 |
| 20 | Lua/DSL escape hatch | Med-High | Medium | 4 |
| 21 | HTML report | Medium | Low-Med | 4 |
| 22 | Cloud offering | Very High | Revenue | 4 |
