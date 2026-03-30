# Tarn AI Workflow Demo

This is a text-first demo you can publish alongside the repo until you record a video.

## Goal

Generate a test from an endpoint description, run Tarn, inspect structured failure JSON, fix the test, rerun green.

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
tarn run health.tarn.yaml --format json
```

## Failure JSON Excerpt

```json
{
  "failure_category": "assertion_failed",
  "assertions": {
    "failures": [
      {
        "assertion": "status",
        "expected": "201",
        "actual": "200",
        "message": "Expected HTTP status 201, got 200"
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

- The request reached the right endpoint.
- The body assertion already passes.
- The only mismatch is the expected status: `201` should be `200`.

## Fix

Change:

```yaml
status: 201
```

to:

```yaml
status: 200
```

## Rerun

```bash
tarn run health.tarn.yaml --format json
```

Expected summary:

```json
{
  "summary": {
    "status": "PASSED"
  }
}
```
