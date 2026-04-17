# Troubleshooting

Guide to diagnosing and fixing the most common Tarn failure modes.

## Route ordering (NestJS and similar)

### What's happening

Many web frameworks match HTTP routes in registration order. When a
dynamic (parameterized) route is registered *before* a specific,
sibling route, the dynamic route **swallows** calls that were meant
for the specific route.

Classic shape:

```ts
// NestJS controller — order matters.
@Get(':id')          // registered first: /foo/:id
findOne(...) { ... }

@Get('approve')      // registered second: /foo/approve
approve(...) { ... }
```

A request to `POST /foo/approve` is matched by `/foo/:id` with
`id = "approve"`. The handler then tries to parse `"approve"` as a
UUID (or integer, or whatever the parameter type is), fails
validation, and returns an opaque 4xx such as:

```json
{
  "statusCode": 400,
  "message": "Validation failed (uuid is expected)",
  "error": "Bad Request"
}
```

The caller sees a 400/404 and assumes their payload is wrong. The
real problem is server-side route registration order.

Express, Fastify, FastAPI, ASP.NET, and many other frameworks have
the same trap under different names.

### How Tarn flags it

When a test expects a 2xx status but receives a 4xx, and the response
body contains a strong textual signal of parameter-validation failure
(for example `"invalid uuid"`, `"cannot parse"`, `"validation failed"`,
`"route not found"`, or a framework-style error that names a URL
segment), Tarn prints a diagnostic note under the failure:

```
 ✗ POST /orders/approve (12ms)
   ├─ Expected HTTP status 201, got 400
   └─ note: the server may have matched this path to a dynamic
      route (e.g. /foo/:id); check for route ordering conflicts
      (see docs/TROUBLESHOOTING.md#route-ordering).
```

In JSON output the same hint appears on the failing `status`
assertion:

```json
{
  "assertion": "status",
  "expected": "201",
  "actual": "400",
  "message": "Expected HTTP status 201, got 400",
  "hints": [
    "note: the server may have matched this path to a dynamic route (e.g. /foo/:id); check for route ordering conflicts (see docs/TROUBLESHOOTING.md#route-ordering)."
  ]
}
```

The hint is intentionally conservative — it only fires on a clear
textual signal. Absence of the hint is **not** evidence that route
ordering is fine; it means the body didn't give a reliable clue.

### How to confirm

1. **Dump the server's route table** in registration order. In NestJS
   this is most easily done by temporarily logging in `main.ts`:

   ```ts
   const server = app.getHttpServer();
   const router = server._events.request._router;
   console.log(router.stack
     .filter(l => l.route)
     .map(l => `${Object.keys(l.route.methods)[0].toUpperCase()} ${l.route.path}`));
   ```

   For Express/Fastify/FastAPI, use the framework's equivalent
   introspection.

2. **Look for a specific route registered after a sibling dynamic
   route** on the same path prefix. Any `/foo/:something` that
   appears before `/foo/<literal>` is a trap.

3. **Try the call directly** with the literal path and a syntactically
   valid param value. If the handler accepts the UUID-shaped value
   but rejects the literal, you've confirmed the collision.

### How to fix

Reorder the route registrations so that **specific routes come
before dynamic routes**:

```ts
// Specific first.
@Get('approve')
approve(...) { ... }

// Dynamic last.
@Get(':id')
findOne(...) { ... }
```

In frameworks where controllers are composed from multiple modules,
check the module import order as well — a later-imported module's
dynamic route can still shadow an earlier-imported specific one if
the path prefixes line up.

When specific-before-dynamic isn't enough (for example, `approve`
really is a legal `:id` value in your domain), disambiguate with a
verb prefix on one side:

```ts
@Post(':id/approve')   // /foo/<uuid>/approve — unambiguous
```

## Common failure categories

| `failure_category` | Typical root cause |
|--------------------|--------------------|
| `connection_error` | Server is down, wrong host/port, DNS issue, TLS/connect failure |
| `timeout` | Step timed out before receiving a complete response |
| `assertion_failed` | Request succeeded, but a status/header/body/duration check failed |
| `capture_error` | The step passed assertions, but extraction failed afterward |
| `parse_error` | Invalid YAML, invalid JSONPath, or invalid config surface |

## Agent diagnosis loop

1. `tarn validate` first — catches syntax and config surface errors.
2. `tarn run --format json --json-mode compact`.
3. Read `failure_category` and `error_code` before the free-text
   message.
4. If a failed `status` assertion carries `hints`, follow the first
   hint before second-guessing the test.
5. If `response` exists, inspect it before editing assertions or
   payloads.
6. If `request.url` still contains `{{ ... }}`, fix env/capture
   interpolation before retrying.

## Non-JSON bodies

- Tarn preserves plain text / HTML responses as JSON strings in the
  structured report.
- Use `body: { "$": "plain text response" }` to assert the whole root
  string when needed.
