# Tarn Test File Format Reference

Complete reference for the `.tarn.yaml` test file structure.

## Top-Level Properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | string | Yes | Human-readable name for this test file |
| `description` | string | No | What this test file covers |
| `version` | string | No | Schema version (always `"1"`) |
| `tags` | string[] | No | Tags for filtering with `--tag` |
| `env` | object | No | Inline env vars (lowest priority) |
| `cookies` | `"auto"`, `"off"`, or `"per-test"` | No | Cookie handling mode (default: `"auto"`) |
| `redaction` | object | No | Header/value redaction policy for reports |
| `defaults` | object | No | Default settings for all requests |
| `setup` | step[] | No | Steps run once before all tests |
| `teardown` | step[] | No | Steps run after all tests (even on failure) |
| `tests` | object | One required | Named test groups (grouped format) |
| `steps` | step[] | One required | Flat step list (simple format) |
| `serial_only` | boolean | No | When true, file is pinned onto the serial worker under `--parallel` |
| `group` | string | No | Resource-group name — files sharing this string run on the same parallel worker |

**Either `steps` or `tests` is required, but not both.**

## Two Formats

### Simple (flat steps)

```yaml
name: Health checks
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 200
```

### Grouped (named tests)

```yaml
name: User API
tests:
  create-user:
    description: "Creates a new user"
    tags: [smoke]
    steps:
      - name: POST /users
        request:
          method: POST
          url: "{{ env.base_url }}/users"
          body:
            name: "Jane"
        assert:
          status: 201
```

## Defaults Block

Applied to every request in the file. Step-level values override defaults.

```yaml
defaults:
  headers:
    Content-Type: "application/json"
    Accept: "application/json"
  auth:
    bearer: "{{ capture.token }}"
  timeout: 5000               # ms
  connect_timeout: 3000       # ms
  follow_redirects: true
  max_redirs: 10
  retries: 0
  delay: "0ms"                # e.g., "100ms", "2s"
```

## Step Properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `name` | string | Yes | Human-readable step name |
| `description` | string | No | Optional multi-line description, rendered under the step name in human output |
| `request` | object | Yes | HTTP request definition |
| `capture` | object | No | Extract values from response |
| `assert` | object | No | Assertions on response |
| `retries` | integer | No | Retry count on failure |
| `timeout` | integer | No | Step timeout in ms |
| `connect_timeout` | integer | No | Connect timeout in ms |
| `follow_redirects` | boolean | No | Follow HTTP redirects |
| `max_redirs` | integer | No | Max redirects to follow |
| `delay` | string | No | Delay before step (`"100ms"`, `"2s"`) |
| `poll` | object | No | Polling configuration |
| `script` | string | No | Lua script for custom validation |
| `cookies` | bool/string | No | Cookie jar control |
| `if` | string | No | Run step only when interpolated expression is truthy (mutually exclusive with `unless`) |
| `unless` | string | No | Run step only when interpolated expression is falsy (mutually exclusive with `if`) |
| `debug` | boolean | No | Embed request/response in the report for this step even when it passes (opts out of the default `only-on-failure` shape) |

## Test Group Properties (under `tests:`)

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `description` | string | No | Description of the test group |
| `tags` | string[] | No | Group-level tags |
| `steps` | step[] | Yes | Steps inside the group |
| `serial_only` | boolean | No | When true, promotes the entire enclosing file to serial (file is the parallel isolation unit) |

## Truthy / falsy rules for `if:` and `unless:`

Expressions go through normal `{{ ... }}` interpolation, then the resolved string is classified:

- Falsy: empty string, whitespace-only, `"false"` / `"FALSE"`, `"0"`, `"null"` / `"Null"`, an unresolved `{{ capture.X }}` placeholder (i.e. an optional-unset capture).
- Truthy: every other non-empty value, including `"true"`, `"1"`, `"ok"`, `"false-but-not"`.

A skipped step reports as `failure_category: skipped_by_condition` with `passed: true` — it never flips the exit code and downstream steps see the previous capture state unchanged.

## Request Properties

| Property | Type | Required | Description |
|----------|------|----------|-------------|
| `method` | string | Yes | HTTP method (GET, POST, PUT, DELETE, PATCH, etc.) |
| `url` | string | Yes | Request URL (supports interpolation) |
| `headers` | object | No | Request headers |
| `auth` | object | No | Auth helper (bearer or basic) |
| `body` | any | No | JSON request body |
| `form` | object | No | URL-encoded form body |
| `graphql` | object | No | GraphQL query/mutation |
| `multipart` | object | No | Multipart form data |

**Only one of `body`, `form`, `graphql`, `multipart` should be used per request.**

## Auth Config

```yaml
# Bearer token
auth:
  bearer: "{{ capture.token }}"

# Basic auth
auth:
  basic:
    username: "{{ env.api_user }}"
    password: "{{ env.api_pass }}"
```

## Capture Formats

### JSONPath shorthand

```yaml
capture:
  user_id: "$.id"
  token: "$.auth.token"
```

### Extended capture

```yaml
capture:
  session:                       # from header
    header: "set-cookie"
    regex: "session=([^;]+)"     # optional regex
  csrf:                          # from cookie
    cookie: "csrf_token"
  final_url:                     # final URL after redirects
    url: true
  status_code:                   # HTTP status code
    status: true
  raw_body:                      # whole response body
    body: true
  explicit_jsonpath:             # explicit JSONPath form
    jsonpath: "$.data.id"
  first_admin:                   # identity-based array matching
    jsonpath: "$.users[*]"
    where:                       # pick the first element matching
      role: "admin"
```

### Optional / conditional captures

| Property | Type | Description |
|----------|------|-------------|
| `optional` | boolean | Missing path → variable unset, not an error. Downstream unresolved references produce a distinct "declared optional and not set" message. |
| `default` | any (number/string/bool/null) | Value to use when the path is missing. Implies `optional`. `null` is preserved (not treated as unset). |
| `when` | object | Only attempt capture when the response matches. Only `when.status` is supported; grammar matches the `status:` assertion (exact, `in: [...]`, ranges). |

```yaml
capture:
  maybe_id:
    jsonpath: "$.id"
    optional: true

  total:
    jsonpath: "$.count"
    default: 0

  created_id:
    jsonpath: "$.id"
    when:
      status: 201

  error_code:
    jsonpath: "$.error.code"
    when:
      status:
        gte: 400
        lt: 500
```

Invalid combinations (e.g. `optional: false` with `default:`) fail validation at parse time.

## Include Directive

Reuse steps from another file:

```yaml
setup:
  - include: ./shared/auth-setup.tarn.yaml
    with:                          # inject parameters
      role: admin
    override:                      # deep-merge into each imported step
      timeout: 10000
```

Included file receives `with` values as `{{ params.name }}`.

## Polling Config

```yaml
poll:
  until:                           # assertions that must pass
    body:
      "$.status": "completed"
  interval: "2s"                   # time between attempts
  max_attempts: 10                 # max tries
```

## Redaction Config

```yaml
redaction:
  headers:                         # header names to redact (case-insensitive)
    - authorization
    - cookie
    - set-cookie
    - x-api-key
  replacement: "***"               # replacement string
  env:                             # env var values to redact
    - api_key
    - secret
  captures:                        # capture values to redact
    - token
```

## Multipart Config

```yaml
multipart:
  fields:
    - name: "title"
      value: "My Document"
  files:
    - name: "file"
      path: "./fixtures/test.pdf"
      content_type: "application/pdf"
      filename: "renamed.pdf"      # optional override
```

## GraphQL Config

```yaml
graphql:
  query: |
    query GetUser($id: ID!) {
      user(id: $id) { name email }
    }
  variables:
    id: "{{ capture.user_id }}"
  operation_name: "GetUser"        # optional, for multi-operation queries
```

## Interpolation

All string values support template interpolation:

- `{{ env.name }}` — environment variable
- `{{ capture.name }}` — captured value from previous step
- `{{ params.name }}` — parameter from include `with:` block
- `{{ $uuid }}` — UUID v4 (alias for `$uuid_v4`)
- `{{ $uuid_v4 }}` — random UUID v4
- `{{ $uuid_v7 }}` — time-ordered UUID v7 (Unix-ms prefix)
- `{{ $timestamp }}` — Unix epoch seconds
- `{{ $now_iso }}` — ISO 8601 datetime
- `{{ $random_hex(N) }}` — random hex string
- `{{ $random_int(min, max) }}` — random integer
- Faker (EN): `{{ $email }}`, `{{ $first_name }}`, `{{ $last_name }}`, `{{ $name }}`, `{{ $username }}`, `{{ $phone }}`, `{{ $word }}`, `{{ $words(n) }}`, `{{ $sentence }}`, `{{ $slug }}`, `{{ $alpha(n) }}`, `{{ $alnum(n) }}`, `{{ $choice(a, b, ...) }}`, `{{ $bool }}`, `{{ $ipv4 }}`, `{{ $ipv6 }}`
- Seed for reproducible runs: `TARN_FAKER_SEED=<u64>` (env wins) or `tarn.config.yaml: faker.seed: <u64>`. Wall-clock values stay real-time.

## Schema Validation

Add to the top of test files for IDE autocompletion:

```yaml
# yaml-language-server: $schema=https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/schemas/v1/testfile.json
```
