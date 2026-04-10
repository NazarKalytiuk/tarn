# Tarn AI Workflow Demo

This is the shortest text-first demo of Tarn's core promise: an agent writes a test, Tarn returns structured failures, and the agent fixes the exact mismatch instead of guessing from stdout.

## Goal

Generate a test from an endpoint description, run Tarn, inspect structured JSON, fix the test, rerun green.

## Example Prompt

```text
Write a Tarn test for GET /health on http://127.0.0.1:3000.
It should expect status 200 and body.status == "ok".
```

## Generated Test

```yaml
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "http://127.0.0.1:3000/health"
    assert:
      status: 201
      body:
        "$.status": "ok"
```

## Run Tarn

```bash
tarn run health.tarn.yaml --format json --json-mode compact
```

## Failure JSON Excerpt

```json
{
  "failure_category": "assertion_failed",
  "error_code": "STATUS_MISMATCH",
  "remediation_hints": [
    "Compare the expected status with the actual response status."
  ],
  "assertions": {
    "failures": [
      {
        "assertion": "status",
        "expected": "201",
        "actual": "200"
      }
    ]
  },
  "request": {
    "method": "GET",
    "url": "http://127.0.0.1:3000/health"
  },
  "response": {
    "status": 200,
    "body": {
      "status": "ok"
    }
  }
}
```

## Agent Diagnosis

- the request reached the correct endpoint
- the body assertion already passes
- the only mismatch is the expected status

At this point the agent can either patch the file directly or call `tarn_fix_plan` over the latest report.

## Fix

```yaml
status: 200
```

## Rerun

```bash
tarn run health.tarn.yaml --format json --json-mode compact
```

Expected summary:

```json
{
  "summary": {
    "status": "PASSED"
  }
}
```

## Tips for Large Suites

When the agent is iterating on a suite with hundreds of tests, two flags keep the feedback loop tight:

- `--only-failed` prunes passing files, tests, and steps from both human and JSON output. Summary counts still reflect the full run, so CI reports stay accurate, but the agent only has to read the failures it needs to fix.
- Progress streaming is on by default: with `--format json` the structured report goes to stdout and per-test progress lines go to stderr, so the agent can tail stderr for liveness while still parsing stdout at the end. Use `--no-progress` if a CI harness already timestamps every stdout line and you prefer the classic batch dump.

```bash
# CI-friendly: show only failures in JSON, no stderr noise
tarn run --only-failed --no-progress --format json

# Interactive debugging: stream progress to stderr, final JSON to stdout
tarn run --only-failed --format json
```
