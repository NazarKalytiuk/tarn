# Tarn JSON Output Reference

Structured output from `tarn run --format json`. Designed for programmatic consumption by AI agents and CI systems.

## Top-Level Structure

```json
{
  "schema_version": 1,
  "version": "1",
  "timestamp": "2026-04-01T12:00:00Z",
  "duration_ms": 1523,
  "files": [ ... ],
  "summary": {
    "files": 2,
    "tests": 5,
    "status": "PASSED",
    "steps": {
      "total": 12,
      "passed": 12,
      "failed": 0
    }
  }
}
```

## File Result

```json
{
  "file": "tests/users.tarn.yaml",
  "name": "User API",
  "status": "PASSED",
  "duration_ms": 850,
  "summary": {
    "total": 3,
    "passed": 3,
    "failed": 0
  },
  "setup": [ ... ],
  "tests": [ ... ],
  "teardown": [ ... ]
}
```

## Test Result

```json
{
  "name": "create-and-fetch",
  "description": "Create a user then fetch it",
  "status": "FAILED",
  "duration_ms": 420,
  "captures": {
    "user_id": "usr_123",
    "token": null
  },
  "steps": [ ... ]
}
```

`captures` shows all captured values at the end of the test group. Missing or failed captures appear as `null`.

## Step Result

### Passed Step

```json
{
  "name": "Create user",
  "status": "PASSED",
  "duration_ms": 85,
  "response_status": 201,
  "response_summary": "201 Created: Object{3 keys}",
  "captures_set": ["user_id"],
  "assertions": {
    "total": 3,
    "passed": 3,
    "failed": 0,
    "details": [
      {
        "assertion": "status == 201",
        "passed": true,
        "expected": "201",
        "actual": "201",
        "message": "Status code matches",
        "diff": null
      }
    ],
    "failures": []
  }
}
```

**Note:** `request` and `response` are omitted for passed steps (keeps output compact). `response_status`, `response_summary`, and `captures_set` provide enough context for AI agents to understand step outcomes.

### Failed Step

```json
{
  "name": "Fetch user",
  "status": "FAILED",
  "duration_ms": 42,
  "failure_category": "assertion_failed",
  "error_code": "assertion_mismatch",
  "remediation_hints": [
    "Expected status 200 but got 404. Verify the resource exists."
  ],
  "assertions": {
    "total": 2,
    "passed": 0,
    "failed": 2,
    "details": [ ... ],
    "failures": [
      {
        "assertion": "status == 200",
        "expected": "200",
        "actual": "404",
        "message": "Status code mismatch: expected 200, got 404",
        "diff": null
      },
      {
        "assertion": "body $.name == \"Jane Doe\"",
        "expected": "\"Jane Doe\"",
        "actual": "{\"error\":\"not found\"}",
        "message": "JSONPath $.name: path not found in response body",
        "diff": "- \"Jane Doe\"\n+ (path not found)"
      }
    ]
  },
  "request": {
    "method": "GET",
    "url": "http://localhost:3000/users/abc123",
    "headers": {
      "authorization": "***"
    },
    "body": null
  },
  "response": {
    "status": 404,
    "headers": {
      "content-type": "application/json"
    },
    "body": {
      "error": "not found"
    }
  }
}
```

## Failure Categories

| Category | Error Codes | Description |
|----------|-------------|-------------|
| `assertion_failed` | `assertion_mismatch`, `poll_condition_not_met` | HTTP succeeded, assertion mismatch |
| `connection_error` | `connection_refused`, `dns_resolution_failed`, `tls_verification_failed`, `network_error` | Could not reach server |
| `timeout` | `request_timed_out` | Request exceeded timeout |
| `parse_error` | `parse_error`, `validation_failed`, `configuration_error` | Invalid YAML, JSONPath, or config |
| `capture_error` | `capture_extraction_failed` | JSONPath extraction failed |
| `unresolved_template` | `interpolation_failed` | `{{ capture.x }}` or `{{ env.x }}` not resolved |

## Error Codes

| Code | Category | Meaning |
|------|----------|---------|
| `assertion_mismatch` | assertion_failed | Expected vs actual value differs |
| `poll_condition_not_met` | assertion_failed | Polling exhausted max_attempts |
| `capture_extraction_failed` | capture_error | JSONPath matched nothing |
| `request_timed_out` | timeout | Exceeded step or default timeout |
| `connection_refused` | connection_error | Server not accepting connections |
| `dns_resolution_failed` | connection_error | Hostname could not be resolved |
| `tls_verification_failed` | connection_error | TLS/SSL handshake failed |
| `redirect_limit_exceeded` | connection_error | Too many redirects |
| `network_error` | connection_error | Other network failure |
| `interpolation_failed` | parse_error | `{{ }}` template could not resolve |
| `validation_failed` | parse_error | YAML structure invalid |
| `configuration_error` | parse_error | Config file issue |
| `parse_error` | parse_error | YAML/JSON syntax error |

## Diagnosis Algorithm

```
FOR each file in report.files:
  IF file.status == "FAILED":
    FOR each test in file.tests:
      IF test.status == "FAILED":
        FOR each step in test.steps:
          IF step.status == "FAILED":
            MATCH step.failure_category:
              "assertion_failed":
                READ step.assertions.failures[]
                COMPARE .expected vs .actual
                CHECK step.response.body for actual API response
                FIX assertion values or application code

              "connection_error":
                CHECK step.request.url for unresolved {{ }} templates
                IF templates found: fix env or capture references
                ELSE: verify server is running at that URL

              "timeout":
                INCREASE step timeout or defaults.timeout
                OR investigate slow server response

              "parse_error":
                RUN tarn validate to get detailed syntax error
                FIX YAML structure

              "capture_error":
                CHECK previous step passed
                CHECK step.request and response shape
                FIX JSONPath expression in capture

              "unresolved_template":
                READ step.assertions.failures[].message for variable names
                CHECK that prior capture steps passed
                CHECK that env vars are set in tarn.env.yaml
                FIX missing captures or env configuration
```

## Notes

- Secrets in headers are redacted to `"***"` in output
- `request` is present for failed executed steps; absent if step was skipped
- `response` is absent when no HTTP response exists (connection failure)
- Non-JSON response bodies are preserved as JSON strings
- `remediation_hints` provides actionable suggestions when available
- Full JSON schema: `schemas/v1/report.json`

## Filtering Output

- `--only-failed` drops passing files, tests, and steps from the `files` array so you can focus on what needs fixing. Top-level `summary` counts still reflect the full run, so PASSED/FAILED totals stay accurate even when passing entries are hidden.
- Streaming progress: by default `tarn run --format json` prints per-test progress lines to **stderr** as tests finish; the structured JSON report is written to stdout once all tests complete. stdout remains pure JSON and is always safe to pipe into `jq` or parse directly. Use `--no-progress` to silence the stderr stream.
