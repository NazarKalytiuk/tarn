# Hurl to Tarn Migration Guide

**Date**: 2026-04-01

This guide reflects Tarn after the completed competitiveness roadmap, not the earlier pre-release surface.

This guide is for teams that already have `.hurl` files and want a practical path into Tarn.

It is not a claim that Tarn should become a Hurl clone.

The goal is narrower:

- port the common request/assert/capture cases cleanly;
- use Tarn features that reduce test orchestration pain;
- identify the cases that still need manual rewriting or should stay in Hurl.

For the deeper product comparison, see [docs/TARN_VS_HURL_COMPARISON.md](./TARN_VS_HURL_COMPARISON.md).

## Short Version

Migrate to Tarn when you want:

- YAML that LLMs generate reliably;
- structured JSON failures with `failure_category`, `error_code`, and remediation hints;
- setup/teardown lifecycle;
- named cookie jars for multi-user scenarios;
- polling, benchmark mode, watch mode, and MCP tooling.

Stay on Hurl for the parts that depend on:

- XPath / HTML assertions and captures;
- certificate inspection queries;
- Hurl's full filter DSL;
- libcurl-specific transport features or exotic auth paths.

## Concept Mapping

| Hurl concept | Tarn concept | Migration note |
|---|---|---|
| `.hurl` file | `.tarn.yaml` file | Tarn is YAML, with schema support and formatter |
| Entry | Step | One Hurl request/response pair usually becomes one Tarn step |
| Variables | `env` / `capture` / builtins | Tarn uses explicit namespaces: `{{ env.x }}`, `{{ capture.x }}`, `{{ $uuid }}` |
| `[Captures]` | `capture:` | Tarn supports JSONPath, header, cookie, body regex, status, final URL |
| Implicit response checks | `assert:` | Status/body/headers/duration/redirect assertions are explicit |
| Hurl retry / timing flags | Step options + CLI | Use `retries`, `timeout`, `connect_timeout`, `follow_redirects`, `max_redirs` |
| Auth header boilerplate | `auth.bearer` / `auth.basic` | Prefer first-class helpers unless the suite needs a custom scheme |
| Multiple report flags | repeatable `--format` | Tarn can emit `human`, `json`, `junit`, `tap`, `html`, `curl`, `curl-all` in one run |
| `hurlfmt` | `tarn fmt` | Tarn normalizes aliases and field ordering in-place |

## Syntax Map

### Basic request

Hurl:

```hurl
GET https://api.example.com/health
HTTP 200
[Asserts]
jsonpath "$.status" == "ok"
```

Tarn:

```yaml
name: Health
steps:
  - name: GET /health
    request:
      method: GET
      url: "https://api.example.com/health"
    assert:
      status: 200
      body:
        "$.status": "ok"
```

### Capture then reuse

Hurl:

```hurl
POST https://api.example.com/users
{
  "name": "Jane"
}
HTTP 201
[Captures]
user_id: jsonpath "$.id"

GET https://api.example.com/users/{{user_id}}
HTTP 200
```

Tarn:

```yaml
name: Users
steps:
  - name: Create user
    request:
      method: POST
      url: "https://api.example.com/users"
      body:
        name: "Jane"
    capture:
      user_id: "$.id"
    assert:
      status: 201

  - name: Read user
    request:
      method: GET
      url: "https://api.example.com/users/{{ capture.user_id }}"
    assert:
      status: 200
```

### JSON body from a file

Hurl can send a file as the request body:

```hurl
POST https://api.example.com/users
file,create-user.json;
HTTP 201
```

Tarn's `body_file` is the direct equivalent for **JSON** payloads. The path resolves relative to the test file, the file is parsed as JSON, and it is interpolated just like an inline `body` (so `{{ env }}` / `{{ capture }}` / builtins work inside it):

```yaml
name: Users
steps:
  - name: Create user
    request:
      method: POST
      url: "https://api.example.com/users"
      body_file: "create-user.json"
    assert:
      status: 201
```

For non-JSON file bodies (binary uploads, raw text), use `multipart` for file uploads or inline the content as a string `body`; `body_file` itself is JSON-only.

### Shared auth

Hurl usually keeps auth setup inline in earlier entries.

In Tarn, move that into `setup` when later tests depend on it:

```yaml
setup:
  - name: Login
    request:
      method: POST
      url: "{{ env.base_url }}/auth/login"
      body:
        email: "{{ env.admin_email }}"
        password: "{{ env.admin_password }}"
    capture:
      token: "$.token"
```

That is one of the biggest migration wins: once the suite stops being flat, Tarn gets cleaner than Hurl instead of more verbose.

## Parity Matrix

| Area | Hurl | Tarn | Migration status |
|---|---|---|---|
| Basic HTTP requests | Yes | Yes | Straightforward |
| JSON body assertions | Yes | Yes | Straightforward |
| Whole-body text / JSON diff | Yes | Yes | Straightforward |
| Header assertions | Yes | Yes | Straightforward |
| Duration assertions | Yes | Yes | Straightforward |
| Redirect assertions | Yes | Yes | Straightforward |
| JSONPath capture | Yes | Yes | Straightforward |
| Header regex capture | Yes | Yes | Straightforward |
| Cookie capture | Yes | Yes | Straightforward |
| Status / final URL capture | Yes | Yes | Straightforward |
| Form requests | Yes | Yes | Straightforward |
| Multipart requests | Yes | Yes | Straightforward |
| Cookie jar persistence | Yes | Yes | Straightforward |
| Multiple output formats in one run | Yes | Yes | Straightforward |
| Compact machine JSON | Limited | Yes | Tarn-native upgrade |
| MCP tool surface | No | Yes | Tarn-native upgrade |
| Setup / teardown lifecycle | No | Yes | Tarn-native upgrade |
| Named cookie jars | No | Yes | Tarn-native upgrade |
| Polling | Limited | Yes | Tarn-native upgrade |
| XPath / HTML assertions | Yes | No | Manual rewrite or keep in Hurl |
| Certificate inspection queries | Yes | No | Keep in Hurl for now |
| Full Hurl filter chain | Yes | Partial | Manual rewrite when beyond Tarn transform-lite |
| Exotic auth / libcurl-only transport | Yes | Partial | Manual review |

## Common Migration Rewrites

### 1. Add namespaces to reused values

Hurl:

```text
{{token}}
{{user_id}}
```

Tarn:

```text
{{ capture.token }}
{{ capture.user_id }}
```

Environment variables become `{{ env.base_url }}` rather than a flat variable namespace.

### 2. Collapse repeated headers into `defaults`

If many Hurl entries repeat the same `Authorization` or `Content-Type` headers, move them into:

```yaml
defaults:
  headers:
    Content-Type: "application/json"
```

### 3. Merge related Hurl files into one lifecycle-oriented Tarn file

When a Hurl suite uses one file for setup, one for CRUD, and one for cleanup, Tarn often reads better as:

- `setup`
- `tests`
- `teardown`

instead of three separate scripts.

### 4. Replace Hurl filters with Tarn transform-lite where possible

Often this:

- `first`
- `last`
- `count`
- `join`
- `split`
- `replace`
- `to_int`
- `to_string`

is enough without dropping into Lua.

If the Hurl flow depends on a richer filter chain, rewrite that part manually.

## Recommended Migration Order

1. Port the simple request/assert files first.
2. Replace flat Hurl variables with explicit `env` and `capture` namespaces.
3. Move repeated login/bootstrap calls into `setup`.
4. Move repeated cleanup into `teardown`.
5. Convert cookie-heavy multi-user flows to named jars instead of manual cookie plumbing.
6. Replace unsupported XPath / certificate queries manually or keep those checks in Hurl.
7. Run `tarn fmt`, then `tarn validate`, then `tarn run --format json --json-mode compact`.

## Built-in Converter

Tarn now ships a conservative MVP converter for common Hurl files:

```bash
tarn import-hurl path/to/test.hurl
tarn import-hurl path/to/test.hurl --output converted/test.tarn.yaml
```

The converter is intentionally narrow. It is meant to accelerate the boring 70-80%, not to guess through unsupported Hurl surface area.

Supported common cases:

- request line (`METHOD URL`)
- request headers
- JSON request body
- `HTTP ...` status line
- `[Captures]` with `jsonpath`, `header`, `cookie`, `status`, `url`, `body`
- `[Asserts]` with `jsonpath`, `header`, `body`, `url`, `redirects`

When the converter sees unsupported Hurl syntax, it fails fast with a parse error so you know the file needs manual migration.

## What Usually Breaks First

### Variable interpolation

The most common migration mistake is forgetting Tarn namespaces:

- wrong: `{{user_id}}`
- right: `{{ capture.user_id }}`

### Duplicate YAML keys

Hurl allows repeating assertions on the same conceptual field as separate lines.

In YAML, duplicate map keys are invalid. Combine operators on one path:

```yaml
"$.id": { is_uuid: true, not_empty: true }
```

not:

```yaml
"$.id": { is_uuid: true }
"$.id": { not_empty: true }
```

### Over-porting flat structure

Do not mechanically map every Hurl file to one Tarn file if the suite has a shared login or shared cleanup flow. Tarn’s lifecycle model is where the migration starts paying off.

## Suggested Automation Boundary

The upcoming converter should handle the common 70-80%:

- request method / URL / headers / body
- basic status and JSONPath assertions
- straightforward captures
- flat variable rewrites

Manual review should still be required for:

- XPath
- certificate queries
- advanced filters
- Hurl-specific auth or transport features
- places where Tarn lifecycle should replace flat entry ordering

## Final Recommendation

Do not migrate because Tarn is “like Hurl but YAML”.

Migrate when the suite benefits from Tarn’s actual differentiators:

- agent-friendly JSON
- MCP
- setup/teardown
- named cookie jars
- polling
- formatter + schema + editor support

If a Hurl file mainly exists to validate protocol details that Tarn does not model yet, keep that file in Hurl and migrate the workflow-oriented parts first.
