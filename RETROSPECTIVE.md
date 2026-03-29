# Tarn CLI Retrospective — SWD2 API Testing

## Context

Used tarn v0.1.0 to write black-box API tests for the SWD2 Elysia API (29 files, 614 test steps, covering auth, profiles, discover, photos, albums, chat, social, settings, admin endpoints). Tests run against a live dev server with PostgreSQL, Redis, and Mailpit.

---

## What Worked Well

### YAML Format
- Readable and easy to reason about — the 3-line minimal test is genuinely simple
- LLM-friendly as advertised — Claude generated valid test files on first try (modulo the body assertion format issue, see below)
- `env:` blocks + interpolation (`{{ env.base_url }}`) make tests portable across environments
- `$uuid` and other built-in functions prevent test collision without manual setup

### CLI Experience
- `tarn validate` is excellent — catches parse errors before running, fast feedback loop
- `tarn list` gives a clear overview of test structure
- Human output with ✓/✗ is clean and easy to scan
- Single binary install, zero runtime dependencies — worked immediately on macOS
- Exit codes (0/1/2/3) map cleanly to CI pass/fail/config-error/runtime-error

### Lua Scripting
- The escape hatch that saved us repeatedly — extracted cookies from Set-Cookie headers, parsed email tokens from Mailpit, compared number types
- Access to `response.headers`, `response.body`, `response.status` and writable `captures` table is well-designed
- `assert()` function inside Lua for custom assertions is a nice touch

### Setup/Teardown Lifecycle
- Setup steps running once before all tests and sharing captures is the right model
- Captured variables flowing from setup → tests → across steps within a test works correctly

### Assertion System
- 20+ operators cover most needs (eq, contains, type, length, exists, regex matches, etc.)
- JSONPath (RFC 9535) is the right choice — works well for nested JSON responses
- AND logic for multiple assertions on the same path is intuitive

### MCP Server
- Clean integration concept — `tarn-mcp` exposes run/validate/list as MCP tools
- Enables AI-assisted test writing → run → iterate loop

---

## Issues & Bugs Found

### P0 — Critical: Capture Failure Aborts Entire Run (exit code 3)

When a JSONPath capture matches nothing (e.g., response has `{ "error": "..." }` instead of the expected shape), tarn exits immediately with code 3. This means:
- One bad setup step kills ALL remaining test files
- No partial results are reported
- The `--format json` output is empty/missing

**Expected behavior:** Mark the step as failed, skip dependent steps, continue to next test/file.

### P1 — No Header Capture

Captures only work on response body via JSONPath. There's no way to capture from response headers without Lua scripting. This forced us to write Lua scripts for every sign-in step to extract the session token from `Set-Cookie`.

**Impact:** Every authenticated test file needed a 5-line Lua block instead of a one-line capture.

Example of what we needed:
```yaml
# What we wanted
capture:
  session_token:
    header: "set-cookie"
    regex: "better-auth\\.session_token=([^;]+)"

# What we had to write instead
script: |
  local cookie = response.headers["set-cookie"]
  if cookie then
    local token = cookie:match("better%-auth%.session_token=([^;]+)")
    if token then captures["session_token"] = token end
  end
```

### P1 — No Cookie Jar / Automatic Cookie Handling

Most real APIs use Set-Cookie for authentication. Tarn has no concept of a cookie jar that automatically:
1. Captures Set-Cookie from responses
2. Sends stored cookies on subsequent requests

This is table-stakes for API testing tools. Without it, every auth flow requires manual Lua extraction + manual Cookie header construction.

### P1 — No Multipart/Form-Data Support

Cannot test file upload endpoints. The `body:` field only supports JSON. For SWD2's photo upload endpoint (`POST /api/photos`), we couldn't write meaningful upload tests at all — had to test error paths only.

### P2 — Body Assertion Format Mismatch with Documentation

The spec.md shows body assertions as a list:
```yaml
body:
  - path: "$.name"
    eq: "Alice"
```

But the actual code expects a map:
```yaml
body:
  "$.name": "Alice"
```

The error message `invalid type: sequence, expected a map` is correct but unhelpful for someone who read the docs. This caused 27/29 files to fail validation on first attempt.

### P2 — Status Assertion Only Accepts Exact Numbers

Cannot express "4xx" or "400 or 422" — must pick one exact status code. Many API error responses legitimately return either 400 or 422 depending on the validation layer that catches the error first.

```yaml
# Not supported
status: { in: [400, 422] }
status: { gte: 400, lt: 500 }

# Must pick one
status: 422
```

### P2 — No Shared Setup / Includes

Every test file that needs authentication duplicates the full 6-step auth setup (signup → Mailpit → verify → login → onboard). With 25 authenticated test files, that's 150 duplicated setup steps.

Roadmap mentions `include: ./shared/auth-setup.tarn.yaml` — this is badly needed.

### P3 — No Built-in Delay/Throttle Between Requests

When hitting rate-limited APIs, there's no way to add automatic delays between requests (per-step `delay:` exists but must be added to every step manually). We had to flush Redis between test files externally.

### P3 — Captured Values Are Always Strings

JSONPath captures convert all values to strings. When the response has `{ "count": 42 }` and you capture it, then assert `"$.count": "{{ capture.prev_count }}"`, it fails because the response has number `42` but the capture has string `"42"`. Had to use Lua with `tonumber()` to work around this.

---

## Feature Requests (Priority Order)

### Must Have (Before Production Use)

1. **Header capture** — `capture: { token: { header: "set-cookie", regex: "..." } }`
2. **Cookie jar** — automatic Set-Cookie handling with opt-in/opt-out per file
3. **Shared setup / includes** — `include: ./shared/auth.tarn.yaml` in setup block
4. **Graceful capture failure** — mark step as failed, continue run, don't exit code 3
5. **Multipart/form-data support** — `body: { type: multipart, fields: [...], files: [...] }`

### Should Have

6. **Status code ranges** — `status: { gte: 400, lt: 500 }` or `status: { in: [400, 422] }`
7. **Type-aware capture comparison** — numbers stay numbers, booleans stay booleans
8. **Global setup** — run once across ALL files (not per-file), useful for creating test users
9. **OpenAPI import** — `tarn init --from openapi.yaml` to scaffold test files (on roadmap)
10. **Retry with backoff** — `retries: 3, backoff: exponential` for flaky/rate-limited endpoints

### Nice to Have

11. **Test tagging in output** — filter results by tag in summary
12. **Watch mode per-file** — `tarn run --watch auth/` re-runs on file change (exists but untested)
13. **Diff output for body assertion failures** — show expected vs actual JSON side-by-side
14. **Environment-specific overrides** — `tarn run --env staging` with `tarn.env.staging.yaml`
15. **Request logging** — `--verbose` shows headers but not request body

---

## Quantitative Assessment

| Metric | Value |
|--------|-------|
| Files written | 29 |
| Test steps | 614 |
| Lines of YAML | ~26,000 |
| Time to write all tests | ~30 min (LLM-generated) |
| Time to fix validation errors | ~20 min |
| Time to fix runtime failures | ~60 min |
| Final pass rate | 100% (614/614) |
| Bugs found in SWD2 API | 3 (dateOfBirth hook, duplicate enum, dead code) |
| Bugs found in tarn | 1 (body assertion format docs mismatch) |
| Workarounds needed | 4 (Lua for cookies, Lua for headers, Redis flush for rate limits, no file upload tests) |

---

## Verdict

**Is it useful?** Yes. Found 3 real bugs in SWD2 that unit tests missed. The black-box approach catches integration issues (like the dateOfBirth string-vs-Date bug) that mocked tests can't.

**Is it easy to use?** Mostly. The YAML format is intuitive, validation is fast, and Lua scripting is a powerful escape hatch. But the lack of cookie/header capture support means every auth-based test requires boilerplate Lua, which undermines the "simple YAML" promise.

**Is it helpful for LLM workflows?** Very. The YAML format is trivially generatable by LLMs. The JSON output format enables automated iteration. The MCP server closes the loop. This is tarn's strongest differentiator.

**Would I use it again?** Yes, for API smoke tests and integration verification. Not yet for comprehensive test suites that need file uploads, WebSocket testing, or complex auth flows — too much Lua glue required. Once header capture and cookie jar land, it becomes a strong choice.

**Overall rating: 7/10** — Solid foundation, needs cookie/header/multipart support to be production-ready for real-world APIs.
