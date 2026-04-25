# Tarn LSP (`tarn-lsp`)

This document is the canonical spec for `tarn-lsp`, the Language Server Protocol implementation for Tarn test files. `tarn-lsp` is the editor-agnostic counterpart to the VS Code extension in `editors/vscode`: it ships as a single stdio binary that any LSP 3.17 client can spawn — Claude Code, Neovim (built-in `vim.lsp`), Helix, Emacs (`eglot` / `lsp-mode`), Zed, Sublime (`LSP` package), and anything else that speaks LSP.

Phase L1 of Epic NAZ-289 is the minimum viable server and is now **fully shipped**. Every feature listed under "Phase L1 status" below is live in the current workspace build.

## Overview

`tarn-lsp` is a thin adapter over the existing `tarn` library (`tarn/src/lib.rs`). It reuses the production parser, interpolation engine, and schema — it does not fork them. The server keeps an in-memory `DocumentStore` populated by `didOpen`/`didChange`, feeds each buffer through `tarn`'s parser on demand, and publishes the results as LSP diagnostics, hovers, completions, and symbols.

The VS Code extension will continue to ship its own direct CLI integration. `tarn-lsp` is for clients that do not have a dedicated Tarn extension and for editors where shipping a full VS Code–style extension is impractical.

## Language identity

- **Language ID**: `tarn`
- **File-match pattern**: `*.tarn.yaml` (and `*.tarn.yml` where a client treats the two as distinct).
- **Binary name**: `tarn-lsp`
- **Transport**: stdio. No TCP, no Unix domain socket, no websocket.
- **LSP protocol version**: 3.17.

The language ID and file pattern intentionally match what the VS Code extension declares in `editors/vscode/package.json`, so any client that already recognises `tarn` files can switch between the extension and `tarn-lsp` without reconfiguration.

## Phase L1 status

Phase L1 is delivered as five tickets under Epic NAZ-289. Each ticket flips on exactly one capability in `tarn-lsp/src/capabilities.rs`. All five are now shipped.

- [x] **L1.1 — bootstrap (NAZ-290)**: workspace crate, stdio lifecycle (`initialize` / `initialized` / `shutdown` / `exit`), in-memory `DocumentStore`, full text document sync, integration tests over `Connection::memory()`.
- [x] **L1.2 — diagnostics (NAZ-291)**: parse every open document through `tarn::parser` on `didOpen`/`didChange`/`didSave` and publish YAML + schema diagnostics via `textDocument/publishDiagnostics`. Debounced at 300ms on `didChange`; flushes immediately on open and save; clears on close.
- [x] **L1.3 — hover (NAZ-292)**: `textDocument/hover` resolves `{{ env.x }}`, `{{ capture.x }}`, `{{ $builtin }}`, and top-level schema keys to Markdown tooltips using the same env resolution chain and parser the runner uses.
- [x] **L1.4 — completion (NAZ-293)**: `textDocument/completion` offers env keys, visible captures, built-in functions, and schema-valid YAML keys with trigger characters `.` and `$`.
- [x] **L1.5 — document symbols + MVP docs (NAZ-294)**: `textDocument/documentSymbol` returns a hierarchical outline — file root (`Namespace`) → named tests (`Module`) → steps (`Function`), with setup/teardown/flat-step siblings. These docs are the MVP release artefact.

**Phase L1 MVP: complete.**

## Phase L2 status

Phase L2 layers navigation features onto the L1 MVP. Each ticket is a thin wrapper around the existing `tarn` crate primitives (`tarn::outline`, `tarn::env`, `tarn::selector`) so jumps stay consistent with what the runner, hover, and diagnostics already see.

- [x] **L2.1 — go-to-definition (NAZ-297)**: `textDocument/definition` jumps from `{{ capture.* }}` / `{{ env.* }}` interpolation tokens to their declaration sites.
- [x] **L2.2 — references (NAZ-298)**: `textDocument/references` lists every use site of a capture (per test, current file) or env key (every `.tarn.yaml` under the workspace root, bounded at 5000 files).
- [x] **L2.3 — rename (NAZ-299)**: `textDocument/rename` + `textDocument/prepareRename` rewrite a capture (per test, current file) or env key (every env source file that declares it, plus every `.tarn.yaml` in the workspace) in a single `WorkspaceEdit`, with identifier validation and per-scope collision detection.
- [x] **L2.4 — code lens (NAZ-300)**: `textDocument/codeLens` emits inline `Run test` and `Run step` actions with stable `tarn.runTest` / `tarn.runStep` command IDs. The server does not execute the commands — clients handle dispatch themselves.

**Phase L2 COMPLETE.** Every navigation and refactor capability listed under Epic NAZ-296 is now shipped.

## Phase L3 status

Phase L3 layers editing features onto the L1/L2 surface. Each ticket is a thin wrapper around an existing `tarn` crate primitive so edits stay consistent with the canonical format, the runner, and the validator.

- [x] **L3.1 — formatting (NAZ-302)**: `textDocument/formatting` reformats the whole document in-process via `tarn::format::format_document` — the same library function the `tarn fmt` CLI calls. Range formatting (`textDocument/rangeFormatting`) is deliberately **not** advertised; the Tarn formatter re-renders the whole buffer so a range-only edit cannot be produced without touching surrounding YAML.
- [x] **L3.2 — code actions + extract env var (NAZ-303)**: `textDocument/codeAction` wires up a pure dispatcher over a flat list of provider functions. The first provider is **extract env var** (`refactor.extract`), which lifts a selected string literal inside a request field into a new env key and rewrites the original site as a `{{ env.<name> }}` interpolation. Collision detection against the full env chain (inline block + `tarn.env.yaml` + `tarn.env.local.yaml` + `tarn.env.{name}.yaml`) suffixes the coined name with a counter (`new_env_key`, `new_env_key_2`, …). Capability advertises `refactor.extract`, `refactor`, and `quickfix` now so later L3 tickets (capture-field refactor, fix-plan quick fix) can plug into the same dispatcher without shipping a capability regression.
- [x] **L3.3 — capture-field + scaffold-assert code actions (NAZ-304)**: two new providers plug into the L3.2 dispatcher. **Capture this field** (`refactor`) lifts a JSONPath literal inside an `assert.body:` entry into a new `capture:` block on the same step, deriving the capture name from the last non-wildcard path segment. **Scaffold assert.body from last response** (`refactor`) walks the top-level fields of a recorded response (pluggable `RecordedResponseSource` trait) and emits an `assert.body` block pre-populated with one `type: …` entry per field. Both actions merge into existing `capture:` / `assert.body:` blocks instead of overwriting them, and collision-suffix any coined capture name (`id`, `id_2`, …) on the way out.
- [x] **L3.5 — nested completion (NAZ-306)**: the completion provider now offers schema-aware child keys for cursors nested below the top-level / step mapping. A YAML walker maps the cursor to a `SchemaPath`, and a schema walker descends through `properties`, `items`, `additionalProperties`, local `$ref`, and `oneOf`/`anyOf`/`allOf` to find valid children at the destination. `request.*` offers `method`/`url`/`headers`/`body`/`form`/`multipart`, `assert.body."$.id".*` offers the `BodyAssertionOperators` grammar (`eq`, `gt`, `matches`, `length`, `type`, `is_uuid`, …), `poll.*` offers `until`/`interval`/`max_attempts`, and `capture.<name>.*` offers the `ExtendedCapture` keys. Descriptions from the schema flow through as `documentation`.
- [x] **L3.6 — JSONPath evaluator (NAZ-307)**: the hover provider grows a fifth token class — **JSONPath literal** — that fires when the cursor sits on a YAML scalar whose text starts with `$.` or `$[` inside an `assert.body` key, a `capture.*.jsonpath` value, or a `poll.until.jsonpath` value. The hover evaluates the expression in place against the step's last recorded response (via the `RecordedResponseSource` trait from L3.3) and appends the result as pretty-printed JSON in the hover markdown, capped at 2000 characters. In parallel, the server now answers `workspace/executeCommand` for a single stable command ID `tarn.evaluateJsonpath`. The command accepts either `{ path, response }` with an inline JSON payload or `{ path, step: { file, test, step } }` that resolves the response through the sidecar convention; returns `{ matches: [...] }` regardless of which lookup path fired. Clients that want to evaluate a JSONPath without re-parsing the `.tarn.yaml` (Claude Code, the upcoming VS Code extension migration, any generic LSP consumer) can invoke the command directly. New shared library surface `tarn::jsonpath::evaluate_path` is the one canonical wrapper over `serde_json_path` everything inside Tarn will consume going forward.

**Phase L3 COMPLETE.** Every editing capability listed under Epic NAZ-301 is now shipped. Phase V — migrating `editors/vscode/` off its direct providers onto `tarn-lsp` so there is one implementation of every language feature — is the next coordinated initiative.

## Installation

`tarn-lsp` is published on crates.io, so the recommended install path is a single Cargo command:

```bash
cargo install tarn-lsp
```

The 0.6.2 release was the first Tarn release to publish `tarn-lsp` to crates.io (see `CHANGELOG.md`). After install, `which tarn-lsp` should print a path inside `~/.cargo/bin/`. That binary is what every LSP client below spawns.

If you are working from a local checkout of the tarn repo — for example, hacking on the server itself — you can install directly from the workspace instead:

```bash
# from the root of the tarn repo
cargo install --path tarn-lsp
```

For pure local development with no install step at all:

```bash
cargo build -p tarn-lsp --release
# binary now lives at ./target/release/tarn-lsp — point your LSP client at this path
```

## Features

`tarn-lsp` ships four language features in Phase L1. Each is a full LSP request handler, each reuses the same in-process `tarn` library the CLI uses, and each is covered by both unit and integration tests.

### 1. Diagnostics (`textDocument/publishDiagnostics`)

Every time a `.tarn.yaml` file is opened, changed, or saved, the server reparses the buffer through `tarn::validation::validate_document` — the same code path `tarn validate` uses. Problems surface as LSP diagnostics with:

- **`range`** derived from NAZ-260 `Location` metadata (1-based line/column → 0-based LSP `Position`). Diagnostics without a location fall back to a zero-width range at `(0, 0)` so they are still visible.
- **`severity`** = `Error` for YAML-syntax, shape, parse, and cross-field semantic failures. `Warning` is reserved for future soft checks.
- **`source`** always `"tarn"` so editors can filter on a stable string.
- **`code`** one of `yaml_syntax`, `tarn_parse`, `tarn_validation`.

Example — the following file surfaces a single `tarn_validation` diagnostic pointing at `requestx:`:

```yaml
name: broken example
steps:
  - name: ping
    requestx:              # typo — rejected by the validator
      method: GET
      url: http://example.com
```

`didChange` publishes are debounced 300ms so a burst of keystrokes collapses into one update. `didClose` clears diagnostics for the closed URI by publishing an empty array.

### 2. Hover (`textDocument/hover`)

`tarn-lsp` answers `textDocument/hover` for four token classes. Every hover body is Markdown.

| Token class                | Example                    | Hover body                                                                                                                |
| -------------------------- | -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| **Environment reference**  | `{{ env.base_url }}`       | Effective value (via `tarn::env::resolve_env_with_sources`), the source layer (inline, default file, named, local, CLI), the source file path when applicable, the active environment name, and a `Redacted: yes/no` flag driven by the test file's `redaction.env:` block. |
| **Capture reference**      | `{{ capture.token }}`      | The declaring step (name + index + section — setup / flat steps / named test / teardown), the capture source (JSONPath, header, cookie, status, URL, whole body, or regex), and a distinct "out of scope" branch when the identifier is declared elsewhere in the file but not visible from the cursor. |
| **Built-in function**      | `{{ $uuid }}`              | The canonical call signature and a one-sentence docstring for each Tarn built-in — `$uuid`, `$uuid_v4`, `$uuid_v7`, `$timestamp`, `$now_iso`, `$random_hex(n)`, `$random_int(min, max)`, plus the faker generators added in NAZ-398 (`$email`, `$first_name`, `$last_name`, `$name`, `$username`, `$phone`, `$word`, `$words(n)`, `$sentence`, `$slug`, `$alpha(n)`, `$alnum(n)`, `$choice(a, b, …)`, `$bool`, `$ipv4`, `$ipv6`). Unknown names get a friendly "not a recognized Tarn built-in" hint listing every supported function. |
| **Top-level schema key**   | `status`, `body`, `env`, … | The `description` field from `schemas/v1/testfile.json` (local `$ref` chains resolved), cached in a `OnceLock` so the schema is parsed exactly once per server process. |

Example — hovering over `env.base_url` in the URL below shows the effective value, source, and environment:

```yaml
env:
  base_url: http://localhost:3000
steps:
  - name: read
    request:
      method: GET
      url: "{{ env.base_url }}/items"
```

### 3. Completion (`textDocument/completion`)

`tarn-lsp` answers `textDocument/completion` in four contexts. It advertises `.` and `$` as trigger characters — the two punctuation marks that open a new completion popup inside an interpolation.

| Context                                | Trigger                  | Items                                                                                                                 | Kind        |
| -------------------------------------- | ------------------------ | --------------------------------------------------------------------------------------------------------------------- | ----------- |
| **Inside `{{ env.<prefix> }}`**        | `.` after `env`          | Every key from `tarn::env::resolve_env_with_sources`, each carrying its resolved value as `detail`.                    | `Variable`  |
| **Inside `{{ capture.<prefix> }}`**    | `.` after `capture`      | Every capture declared by a strictly earlier step visible from the cursor.                                             | `Variable`  |
| **Inside `{{ $<prefix> }}`**           | `$` after `{{`           | Every Tarn built-in — the UUID/time/random primitives plus the NAZ-398 faker corpus (`$email`, `$name`, `$slug`, `$choice(...)`, `$ipv4`, …). Parameterized forms (`$random_hex`, `$random_int`, `$words`, `$alpha`, `$alnum`, `$choice`) are emitted as snippets with argument placeholders. | `Function`  |
| **Blank YAML mapping-key line**        | newline / manual trigger | Schema-valid keys for the cursor's scope — root, test group, or step.                                                  | `Property`  |
| **Nested blank line inside a schema-valid parent** (new in L3.5) | newline / manual trigger | Schema-valid child keys for the cursor's YAML path — e.g. `method`/`url`/`headers`/`body` under `request:`, `eq`/`gt`/`matches`/`length` under `assert.body."$.id":`, `until`/`interval`/`max_attempts` under `poll:`, `header`/`cookie`/`jsonpath`/`regex` under `capture.<name>:`. | `Property` |

#### Nested completion (new in L3.5)

L3.5 layers a schema-tree walker on top of the L1.4 top-level completion. [`completion::resolve_schema_path`] maps a blank-line cursor to a `SchemaPath` — a dot-path into the JSON Schema — and [`schema::children_at_schema_path`] walks that path through `properties`, `items`, `additionalProperties`, `$ref`, and the `oneOf` / `anyOf` / `allOf` combinators to find every valid child at the destination. Completion items carry the schema's `description` field as their `documentation` where available, so hovering a suggestion shows the same text Tarn's schema docs render.

The YAML walker is deliberately line-based and permissive — it works off raw lines rather than a parsed tree, so half-finished buffers mid-edit still produce a usable path. The schema walker supports the JSON Schema constructs the bundled `schemas/v1/testfile.json` actually uses (`properties`, `items`, `additionalProperties`, local `$ref`, `oneOf`/`anyOf`/`allOf`); `patternProperties`, `if`/`then`/`else`, and external refs are out of scope because the Tarn schema does not use them.

Example — typing `{{ env.` in a request URL shows every env key resolved from the active environment chain:

```yaml
env:
  base_url: http://localhost:3000
  api_key: secret
tests:
  main:
    steps:
      - name: list
        request:
          method: GET
          url: "{{ env. }}/items"   # completion offers base_url, api_key
```

### 4. Document symbols (`textDocument/documentSymbol`) — new in L1.5

`tarn-lsp` answers `textDocument/documentSymbol` with a hierarchical outline editors render in their go-to-symbol UI, outline pane, or breadcrumb trail. The tree is:

- **File root** (`SymbolKind::Namespace`) — the file-level `name:` value (or the URI basename if `name:` is absent).
- **Named tests** (`SymbolKind::Module`) — every key under the top-level `tests:` mapping, in source order.
- **Steps** (`SymbolKind::Function`) — every entry under `setup:`, `teardown:`, top-level `steps:`, and each test's `steps:` block. Steps without a `name:` (e.g. `include:` entries) get a synthetic `<step N>` placeholder so the outline still reflects source ordering.

Each symbol's `range` covers the full YAML node (so clicking the symbol selects the whole step), and `selection_range` covers just the `name:` value (so go-to-symbol lands on the name). The ranges come from the same `yaml-rust2` second-pass scanner as NAZ-260 runtime locations, so the outline stays in lockstep with diagnostics on every line.

Example — the outline of this fixture is `symbols example → [login (setup)] → [main → [list, create]] → [cleanup (teardown)]`:

```yaml
name: symbols example
setup:
  - name: login
    request: { method: POST, url: http://localhost/auth }
tests:
  main:
    steps:
      - name: list
        request: { method: GET, url: http://localhost/items }
      - name: create
        request: { method: POST, url: http://localhost/items }
teardown:
  - name: cleanup
    request: { method: POST, url: http://localhost/cleanup }
```

### 5. Go-to-definition (`textDocument/definition`) — new in L2.1

Invoking "Go to definition" on a `{{ capture.NAME }}` token jumps to the declaring `capture:` entry inside the same test (setup captures are always visible; named-test captures are only visible from steps in that test). If several steps declare the same capture name, every declaration is returned so the client can show a picker. If no step declares the name, the handler returns no result and the client suppresses its UI.

Invoking "Go to definition" on a `{{ env.KEY }}` token walks the env resolution chain (`--var` > `tarn.env.local.yaml` > named env file > `tarn.env.yaml` > inline `env:` block) and jumps to the winning layer's declaration site. Layers that do not live in a YAML file we can point at — `--var` overrides, shell-variable expansion, and named profile vars from `tarn.config.yaml` — intentionally return no result. Built-in functions (`$uuid`, `$random_hex`, …) and top-level schema keys (`status`, `body`, `request`, …) are explicitly non-navigable; L2.1 only covers interpolation tokens.

Ranges come from the same `yaml-rust2` second-pass scanner that powers `documentSymbol`, so jump targets stay in lockstep with the outline pane and the diagnostics gutter.

### 6. References (`textDocument/references`) — new in L2.2

Invoking "Find references" on a `{{ capture.NAME }}` token lists every interpolation in the current file that references the same capture, scoped to the cursor's enclosing test (setup captures are always visible from inside any test). Capture references intentionally never walk the workspace — Tarn captures are scoped per-test in the data model, and surfacing cross-test matches would be misleading. When the request asks for `include_declaration: true`, the response also includes the `capture:` key location for each declaration in scope.

Invoking "Find references" on a `{{ env.KEY }}` token walks every `.tarn.yaml` file under the workspace root and lists every interpolation that references the same env key. The walk is populated lazily on the first reference query and cached in a `WorkspaceIndex` keyed by file URL; the server's `didChange` / `didSave` / `didClose` notification handlers invalidate the affected URL so subsequent queries see fresh content. The walk is bounded at **5000 files** as a safety net for pathological monorepos — when the cap is reached the server logs a warning to stderr and serves the partial result rather than erroring out. When the request asks for `include_declaration: true`, the response also includes the env key's source location in whichever file (inline `env:` block, default env file, named env file, local env file) supplied the winning value per the L1.3 resolution chain. Layers that do not live in a YAML file we can point at — `--var` overrides and named profile vars — emit only the in-source use sites.

Built-in functions and top-level schema keys are non-navigable, the same way they are for go-to-definition.

### 7. Rename (`textDocument/rename` + `textDocument/prepareRename`) — new in L2.3

Invoking "Rename symbol" on a `{{ capture.NAME }}` or `{{ env.KEY }}` token — or on the corresponding declaration — rewrites every declaration and every use site in a single atomic `WorkspaceEdit`. Before the rename fires, the server answers a `textDocument/prepareRename` round-trip that returns the sub-range of the identifier under the cursor so the client can highlight exactly the text the user is about to replace. `prepareRename` returns `null` for tokens that are not renamable (built-in functions, top-level schema keys, and tokens whose identifier is empty or still being typed).

**Capture rename is single-file, single-test.** Capture scopes never cross file boundaries in Tarn's data model, so the resulting edit touches only the current file. Setup captures are visible from every test, so a rename that starts on a capture declared in `setup:` updates every test's use sites; a rename that starts on a capture declared inside a named `tests:` group only updates that test. If the new name collides with another capture already visible from the cursor scope, the server rejects the rename with an `InvalidParams` response error naming the conflicting key.

**Env rename is workspace-wide.** The edit updates every env source file that declares the old name — inline `env:` block of the current test file, `tarn.env.yaml`, `tarn.env.{name}.yaml`, `tarn.env.local.yaml` — and every `{{ env.KEY }}` use site across every `.tarn.yaml` file in the workspace index. Layers that do not live in a YAML file (`--var` overrides, shell expansion, named profile vars) are left untouched because there is nothing on disk to edit; the use sites are still rewritten. Collision detection runs per env source file that declares the old name: if any such file also already declares the new name, the server rejects the rename so the user does not end up with two keys of the same name in one file.

**Identifier validation.** Both capture and env keys must match the Tarn identifier grammar `^[A-Za-z_][A-Za-z0-9_]*$`. The validator is ASCII only — Unicode letters are intentionally rejected so the YAML key, the interpolation token, and the `${VAR}` shell-expansion placeholder all agree on what is a valid identifier. An invalid new name surfaces as an `InvalidParams` response error with a human-readable message the client can show in a toast. Built-ins and schema keys surface as `RequestFailed` so clients can tell the difference between "bad name" and "this token is not renamable".

### 8. Code lens (`textDocument/codeLens`) — new in L2.4

Every named test in a `.tarn.yaml` file gets a **`Run test`** code lens anchored on its `name:` line, and every step inside a named test gets a **`Run step`** lens on its own `name:` line. The lenses are only emitted for named-test groups; setup, teardown, and top-level flat `steps:` intentionally do not receive lenses — they match the behavioural scope of the VS Code extension's `TestCodeLensProvider.ts` so switching between the extension and plain LSP shows the same affordances.

Each lens carries a `Command` whose `command` field is one of two stable, well-known constants:

- `tarn.runTest` — emitted for test-level lenses
- `tarn.runStep` — emitted for step-level lenses

These strings are part of the server's public contract and must not change. The `arguments` field carries a single JSON object with the fields the client needs to spawn `tarn run --select <selector>` itself:

```jsonc
// Run test
{
  "file": "file:///abs/path/tests/users.tarn.yaml",
  "test": "create_user",
  "selector": "/abs/path/tests/users.tarn.yaml::create_user"
}

// Run step
{
  "file": "file:///abs/path/tests/users.tarn.yaml",
  "test": "create_user",
  "step": "POST /users",
  "selector": "/abs/path/tests/users.tarn.yaml::create_user::0"
}
```

The `selector` string is the exact argument to pass to `tarn run --select`. It is composed by the shared [`tarn::selector::format_test_selector`] / [`tarn::selector::format_step_selector`] helpers — the same source of truth the VS Code extension uses via `editors/vscode/src/testing/runHandler.ts`, so both producers emit byte-identical strings. The step component is the **zero-based step index**, not the step name, because indices are unique per test and never require escaping.

`codeLens/resolve` is **not** implemented — every lens is fully populated in the initial response. Clients that send a resolve request will get a JSON-RPC `MethodNotFound` error; well-behaved clients will never send one because the server advertises `resolveProvider: false` in its capabilities.

**The server does not execute `tarn.runTest` / `tarn.runStep`.** These commands are handled by the client, which is expected to shell out to `tarn run --select <selector>` on its own side. Streaming NDJSON progress back through LSP notifications is deliberately deferred — a Phase L3 follow-up can revisit it if we decide the server should own execution as well.

### 9. Formatting (`textDocument/formatting`) — new in L3.1

Invoking "Format Document" (or any equivalent client-side shortcut — VS Code's `Shift+Alt+F`, Neovim's `vim.lsp.buf.format`, Helix's `:format`) reformats the whole `.tarn.yaml` buffer into the canonical Tarn layout by routing through `tarn::format::format_document`. That is the **same library function** the `tarn fmt` CLI calls — there is exactly one implementation, so a buffer formatted via LSP is byte-identical to the result of running `tarn fmt` on it from the terminal.

The server always responds with one of two shapes:

- An **empty array** when the buffer is already canonical, unknown to the server (never `didOpen`-ed), **unparseable** (broken YAML mid-edit), or schema-invalid. In the unparseable and schema-invalid cases the server logs a `tarn-lsp:` / `tarn::format:` prefixed warning to stderr so the "Language Server" output pane still tells the user why nothing changed. Formatting a broken document is a **no-op, never a failure** — the client never sees an error pop-up while the user is still typing.
- A **single whole-document `TextEdit`** whose range starts at `(0, 0)` and ends past the last character of the old buffer. Clients merge the edit as one undo step, so a single Ctrl+Z reverts the entire format. There is no range-level diffing because the Tarn formatter re-renders the whole buffer — computing a minimal line diff would buy nothing the user can see, and would risk drift between CLI and LSP output.

**Range formatting is not supported.** `textDocument/rangeFormatting` is deliberately left off the server capabilities, and the corresponding dispatch handler is absent. The Tarn formatter's contract is "normalise the whole document to canonical field order", which has no sensible subset-of-range interpretation — the `serde_yaml` round-trip produces bytes that differ from the input at arbitrary offsets, not just inside the selection. Clients that ask for range formatting will get a `MethodNotFound` response; well-behaved clients will never ask because the capability is not advertised.

**CLI parity.** `tarn fmt file.tarn.yaml` and "Format Document" in the editor produce byte-identical output for the same input, because both paths call `tarn::format::format_document`. If a formatted buffer surprises you, run `tarn fmt --check file.tarn.yaml` from the terminal — the same canonical layout will surface there.

### 10. Code actions (`textDocument/codeAction`) — new in L3.2

`tarn-lsp` answers `textDocument/codeAction` with a **pure dispatcher** that walks a flat list of provider functions and concatenates their results. Each provider returns a fully-resolved `CodeAction` with the `edit: WorkspaceEdit` already populated, so there is no `codeAction/resolve` round trip — clients apply the edit in one step.

The capability advertises three stable action kinds from this ticket forward:

| Kind                 | Provider                              | Status                    |
| -------------------- | ------------------------------------- | ------------------------- |
| `refactor.extract`   | **Extract env var**                   | shipped in L3.2 (NAZ-303) |
| `refactor`           | **Capture this field**, **Scaffold assert.body from last response** | shipped in L3.3 (NAZ-304) |
| `quickfix`           | **Apply fix** from shared fix-plan library | shipped in L3.4 (NAZ-305) |

Both `refactor` providers plug into the same `code_actions_for_range` dispatcher that `extract_env` uses — new providers are just a function call and a provider name away. `resolve_provider: false` is pinned for the whole dispatcher — every provider must return a fully resolved action.

#### Extract env var (`refactor.extract`)

**Trigger.** Place the cursor (or make a selection inside) a string literal that lives in a request field — `request.url`, `request.headers.*`, a `request.body.*` field value, `request.query.*`, `request.form.*`, `request.multipart.*`, or the step-level `url:` alias — **and** the literal is not already an interpolation. Clients see the action as `"Extract to env var…"` in their refactor menu.

**Edit shape.** A single `WorkspaceEdit` on the current file with two `TextEdit`s:

1. The literal is replaced with `"{{ env.<chosen_name> }}"` (quoted so the YAML parse shape stays a string).
2. The original value is inserted into the file's inline `env:` block as a new key named `<chosen_name>`. If the file has no inline `env:` block, a fresh one is created at the top of the file (above any `setup:` / `tests:` / `steps:`). The original value is escaped via a small `yaml_scalar_literal` helper so simple words stay bare scalars (`new_env_key: hello`) while anything containing quotes, special YAML characters, newlines, or that would parse as a bool / number / null gets a double-quoted form (`new_env_key: "hello \"world\""`).

**Placeholder naming.** The coined env key defaults to `new_env_key`. The name is then checked against the **full env chain** the file sees (inline `env:` block ∪ `tarn.env.yaml` ∪ `tarn.env.local.yaml` ∪ `tarn.env.{name}.yaml`). When the default name is already taken, the server suffixes with a counter until a free slot is found: `new_env_key`, `new_env_key_2`, `new_env_key_3`, … The chosen name is logged to stderr (`tarn-lsp: extract env var chose name …`) so users can see what was picked.

**Excluded scalars.** The action declines on:

- scalars already classified as interpolations (`{{ env.foo }}`, `{{ capture.bar }}`, `{{ $uuid }}`),
- YAML bool / null / numeric literals (`true`, `3`, `3.14`, `null`, `~`),
- scalars that are not inside a request field (step `name:`, `tags:`, `capture:`, `assert:`, `defaults:`),
- selections that span more than one YAML node.

#### Capture this field (`refactor`) — new in L3.3

**Trigger.** Place the cursor on the JSONPath literal that serves as the key of an `assert.body:` entry — e.g. `"$.data[0].id"` inside:

```yaml
assert:
  body:
    "$.data[0].id":
      eq: 5
```

Clients see the action as `"Capture as capture variable…"` in the refactor menu. The URL-id heuristic variant sketched in NAZ-304's description is deferred to a follow-up — the body-assertion trigger is the shipped MVP.

**Edit shape.** A single `WorkspaceEdit` on the current file with one `TextEdit` that inserts a `capture:` entry into the enclosing step. The generated entry uses the extended `{ jsonpath: … }` form:

```yaml
capture:
  id:
    jsonpath: "$.data[0].id"
```

If the step already has a `capture:` block, the new entry is **appended** to it — duplicates by name are skipped and existing entries are never overwritten. If the step has no `capture:` block yet, a fresh one is inserted at the end of the step mapping at the same indent as `name:`, `request:`, `assert:`, and its other top-level siblings.

**Leaf-name derivation.** The capture key is the last non-wildcard segment of the JSONPath:

| Path                  | Coined name |
| --------------------- | ----------- |
| `$.id`                | `id`        |
| `$.data[0]`           | `data_0`    |
| `$.data[0].id`        | `id`        |
| `$.user.email`        | `email`     |
| `$.tags[*]`           | `tags`      |
| `$["weird-key"]`      | `weird_key` |

Empty, wildcard-only, and otherwise unrepresentable paths fall back to `field`. Names are then sanitised to the Tarn identifier grammar (`[A-Za-z_][A-Za-z0-9_]*`) — non-identifier characters become `_`, leading digits are prefixed with `_`, and collisions against existing captures in the same step are resolved by counter-suffix (`id`, `id_2`, …), exactly the same shape the env-key collision resolver uses.

**Excluded positions.** The action declines on:

- scalars whose value is not a JSONPath literal (does not start with `$.` or `$[`),
- positions that are not inside an `assert.body:` key,
- buffers the outline walker cannot locate a step for.

#### Scaffold assert.body from last response (`refactor`) — new in L3.3

**Trigger.** Place the cursor anywhere inside a `request:` block of a named step — e.g. on the `url:`, `method:`, or a header value — **and** the LSP must have a recorded response for that step on disk. Clients see the action as `"Scaffold assert.body from last response"` in the refactor menu.

**Edit shape.** A single `WorkspaceEdit` on the current file with one `TextEdit` that inserts an `assert.body:` block pre-populated with one entry per top-level field of the recorded response, keyed on the JSONPath and bound to the inferred type:

```yaml
assert:
  body:
    "$.id":
      type: number
    "$.name":
      type: string
    "$.tags":
      type: array
```

Inferred types follow the JSON-to-Tarn mapping `number | string | boolean | array | object | null`. Integers and floats both fold to `number`. Only top-level fields are in scope — the user can drill into nested paths manually.

**Merge behavior.** If the step already has an `assert.body:` block, new entries are **appended**. Paths that are already declared (by key string) are skipped, so re-running the action never duplicates an existing assertion. If every top-level field of the response is already asserted, the action declines — nothing to do. If the step has `assert:` but no `body:` child, a fresh `body:` sub-block is appended to the existing `assert:` mapping. If the step has neither, a brand new `assert:\n  body: …` block is inserted at the end of the step.

**Recorded-response sidecar convention.** The LSP reads recorded responses from a pluggable [`RecordedResponseSource`](../tarn-lsp/src/code_actions/response_source.rs) trait whose default disk implementation expects sidecar files at:

```text
<file>.tarn.yaml
<file>.tarn.yaml.last-run/
  <test-slug>/
    <step-slug>.response.json
```

`<test-slug>` and `<step-slug>` are URL-safe versions of the respective names (lowercase, whitespace replaced with `-`, everything outside `[a-z0-9_-]` stripped). Top-level setup and teardown steps use the sentinel test slugs `setup` / `teardown`; top-level flat `steps:` use the sentinel `flat`. Example for a step named `POST /users` inside a test named `create_user`:

```text
users.tarn.yaml.last-run/create_user/post-users.response.json
```

The VS Code extension keeps its own last-run cache **in memory only** (see `editors/vscode/src/testing/LastRunCache.ts`) and does not yet write these sidecars. Until the writer lands as a separate ticket, the disk reader always returns `None` and the action simply does not trigger — a documented no-op. The trait seam means unit and integration tests substitute an `InMemoryResponseSource` so they never depend on the writer.

**Excluded positions.** The action declines when:

- `CodeActionContext.recorded_response_reader` is `None` (server wiring omitted),
- the reader returns `None` (no recording available),
- the recorded response is not a JSON object (arrays, scalars, `null` have no top-level fields),
- every top-level field is already present in the step's `assert.body:`,
- the cursor is not inside a `request:` block,
- the enclosing step has no `name:` (synthetic `<step N>` placeholder names have no deterministic sidecar slug).

#### Apply fix (`quickfix`) — new in L3.4

**Trigger.** Invoke `textDocument/codeAction` at a range where the client has one or more `source: "tarn"` diagnostics. The provider asks the shared [`tarn::fix_plan::generate_fix_plan`](../tarn/src/fix_plan.rs) library whether any of those diagnostics carry a mechanically-applicable fix. Today the library recognises the `Unknown field 'X' at <context>. Did you mean 'Y'?` pattern — the typo messages the parser already emits for unknown mapping keys — and returns a `FixPlan` with a single-key replacement edit. Other validation messages flow through the LSP as ordinary diagnostics with no Quick Fix offered (declining to offer a fix is an explicit allowed state, not an error).

**Backend sharing.** `tarn-mcp`'s `tarn_fix_plan` tool and `tarn-lsp`'s Quick Fix provider both call into `tarn::fix_plan`, so the two surfaces share one source of truth. The MCP tool uses the report-driven path (`generate_fix_plan_from_report`) — advice plus prioritised remediation hints — and the LSP uses the diagnostic-driven path (`generate_fix_plan`) — structured `WorkspaceEdit`s. Both paths emit the same `FixPlan` struct; only the `edits` vector populates differently.

**Action shape.** Every Quick Fix carries:

- `kind: CodeActionKind::QUICKFIX`
- `title: "Apply fix: <plan title>"` (e.g. `"Apply fix: Change 'step' to 'steps'"`)
- `diagnostics: Some(vec![diagnostic])` — pins the action to the squiggle it resolves so clients render it under the matching error
- `edit: WorkspaceEdit` — the full set of pre-computed text edits on the current buffer
- `is_preferred: Some(true)` — library-produced plans are unambiguous by construction, so clients that auto-apply the preferred action can do so without prompting

**Safety gates.** The provider skips any diagnostic whose `source` is not `"tarn"`, any diagnostic with a numeric `code`, and any diagnostic whose message does not match a fix-plan pattern. Foreign diagnostics produced by other LSPs or extensions never flow into the library.

### 11. JSONPath evaluator — new in L3.6 (NAZ-307)

L3.6 layers two complementary affordances on top of the existing hover + code-action surface so LLM clients (including Claude Code) and humans alike can evaluate a JSONPath against the step's last recorded response without re-parsing the `.tarn.yaml` buffer themselves.

Both affordances share the same library primitive: `tarn::jsonpath::evaluate_path(path, value) -> Result<Vec<serde_json::Value>, JsonPathError>`. This is a thin, canonical wrapper over `serde_json_path` (already a workspace dependency used by `tarn::assert::body` and `tarn::capture`). Ship as `tarn/src/jsonpath.rs`, re-exported from `tarn/src/lib.rs`. One source of truth — the assertion, capture, hover, and command paths all end up in the same parser / query plumbing.

#### JSONPath hover

**Trigger.** Place the cursor on a YAML scalar whose unquoted text starts with `$.` or `$[` (or is the bare `$`). The detector fires when the scalar sits in one of three expected shapes:

- a key in an `assert.body` mapping (e.g. `"$.data[0].id"`)
- the value of a `capture.<name>.jsonpath: "…"` entry or the shorthand `capture.<name>: "$.foo"`
- the value of a `poll.until.jsonpath: "…"` entry (when the shape exists)

**Lookup.** The hover locates the enclosing step via the `tarn::outline` walker, derives a `(test, step)` identifier pair, and asks the same `RecordedResponseSource` trait the scaffold-assert code action uses (NAZ-304). The default disk reader consults the `<file>.last-run/<test-slug>/<step-slug>.response.json` sidecar convention. When no response is available the hover still renders a heading (`JSONPath literal: \`$.data[0].id\``) plus a graceful "no recorded response" footer — it never errors.

**Output.** When a response is available, the hover evaluates the path and appends the result as pretty-printed JSON in a fenced ` ```json ` block. Results longer than 2000 characters are truncated with an explicit marker so clients can still invoke the `tarn.evaluateJsonpath` command for the full payload (see below).

#### `workspace/executeCommand` for `tarn.evaluateJsonpath`

**Capability.** Advertised via `execute_command_provider: ExecuteCommandOptions { commands: vec!["tarn.evaluateJsonpath".into()], .. }`. L3.6 is the first Phase L3 ticket to flip `workspace/executeCommand` on — earlier tickets (L2.4 `tarn.runTest` / `tarn.runStep`) used stable command IDs but the server deliberately did not handle them server-side; clients dispatched them by shelling out.

**Arguments.** The command accepts exactly one argument object in either of two shapes (dispatched via a `serde` untagged enum):

```jsonc
// Shape 1: inline response.
{
  "path": "$.data[0].id",
  "response": { "data": [{"id": 42}] }
}

// Shape 2: step reference — the server reads the sidecar response.
{
  "path": "$.data[0].id",
  "step": { "file": "/abs/fixture.tarn.yaml", "test": "main", "step": "list items" }
}
```

The `file` field accepts either a bare filesystem path or a `file://` URI — the server normalises through `Url::to_file_path` so clients can forward whatever they have. Setup / teardown / flat-step sentinels (`"setup"`, `"teardown"`, `"<flat>"`) are accepted for the `test` field, mirroring the sidecar writer's slug rules.

**Return envelope.** `{ "matches": [ ... ] }` — an explicit object wrapping the JSONPath matches in document order. Single-match paths return a one-element array, not an unwrapped value; multi-match paths return every match; not-found paths return `[]` (not an error). Future expansions (match locations inside the response document) can add sibling fields without a source-breaking change.

**Errors.** Every soft failure returns an `InvalidParams` (`-32602`) `ResponseError`:

- missing or malformed argument object
- JSONPath parse failure (with the underlying message)
- step reference with no sidecar file on disk
- step reference with a corrupt / non-JSON sidecar

Unknown command IDs return `MethodNotFound` (`-32601`) with a helpful message listing `tarn.evaluateJsonpath`. Return code is always a standard JSON-RPC error — the command never hangs or silently swallows a bad argument.

**Phase L3 COMPLETE.** With L3.6 shipped every editing capability under Epic NAZ-301 is now live. The next coordinated initiative is **Phase V** — migrating `editors/vscode/` off its direct providers onto `tarn-lsp` so there is one implementation of every language feature.

## Client configuration

The binary speaks stdio LSP 3.17 — any client that can spawn an LSP server and speak JSON-RPC over stdio will work. Below are the three most common configurations.

### Generic LSP client (reference)

Every major LSP client ends up feeding the same three facts to its launcher: a language identifier, a file pattern, and a command. In pseudo-config JSON:

```json
{
  "languageId": "tarn",
  "filePattern": "*.tarn.yaml",
  "command": ["tarn-lsp"],
  "transport": "stdio"
}
```

Adapt the field names to whatever your client's config actually calls them. Nothing about `tarn-lsp` is specific to any one client — it is a plain LSP 3.17 stdio server.

### Neovim (`nvim-lspconfig`)

`nvim-lspconfig` does not ship a built-in entry for Tarn yet, so register the server manually. Drop this into your Neovim config (`init.lua` or a filetype plugin):

```lua
local configs = require("lspconfig.configs")
local lspconfig = require("lspconfig")

if not configs.tarn_lsp then
  configs.tarn_lsp = {
    default_config = {
      cmd = { "tarn-lsp" },
      filetypes = { "tarn" },
      root_dir = lspconfig.util.root_pattern("tarn.config.yaml", ".git"),
      settings = {},
    },
  }
end

vim.filetype.add({
  pattern = { [".*%.tarn%.yaml"] = "tarn", [".*%.tarn%.yml"] = "tarn" },
})

lspconfig.tarn_lsp.setup({})
```

The two pieces are (a) the `configs.tarn_lsp` registration — Neovim needs to know the command and filetype — and (b) the `vim.filetype.add` call so `.tarn.yaml` buffers actually get the `tarn` filetype. Adapt to your LSP client framework if you use something other than `nvim-lspconfig`.

### VS Code

VS Code is **not** wired up to `tarn-lsp` today. The existing VS Code extension in `editors/vscode/` uses direct providers (hover, completion, documentSymbol) that call the `tarn` library in-process, rather than going through an LSP client. Migrating the VS Code extension onto `tarn-lsp` is deliberately deferred to Phase L2 so the MVP ships on a stable, well-tested surface. If you want to use `tarn-lsp` from VS Code today, install a generic LSP client extension (e.g. [`langserver-generic`](https://marketplace.visualstudio.com/search?term=generic%20language%20client)) and point it at the generic snippet above.

### Claude Code

Claude Code registers LSP servers through its plugin system — specifically via a `.lsp.json` file inside a Claude Code plugin. This repo ships a ready-to-use plugin at `editors/claude-code/tarn-lsp-plugin/`, surfaced from the repo-root marketplace alongside the `tarn` MCP + skill plugin, so you can install both from one place.

**Prerequisites**

1. Claude Code **2.0.74 or newer** (`claude --version` to check; `npm update -g @anthropic-ai/claude-code` or `brew upgrade claude-code` to update).
2. `tarn-lsp` on your `$PATH`. From a checkout of this repo: `cargo install --path tarn-lsp`.

**Install the plugin** (inside a Claude Code session):

```shell
/plugin marketplace add NazarKalytiuk/tarn
/plugin install tarn-lsp@tarn --scope project
/reload-plugins
```

The `--scope project` flag matters — see the **Compound-extension caveat** below.

If you prefer a local checkout, substitute `/absolute/path/to/tarn` for `NazarKalytiuk/tarn` in the `marketplace add` call. For a one-off session without persistent installation, use `claude --plugin-dir` instead:

```shell
claude --plugin-dir /absolute/path/to/tarn/editors/claude-code/tarn-lsp-plugin
```

**What the plugin registers** (from `editors/claude-code/tarn-lsp-plugin/.lsp.json`):

```json
{
  "tarn": {
    "command": "tarn-lsp",
    "args": [],
    "transport": "stdio",
    "extensionToLanguage": {
      ".yaml": "tarn",
      ".yml": "tarn"
    },
    "restartOnCrash": true,
    "maxRestarts": 3,
    "startupTimeout": 5000,
    "shutdownTimeout": 2000
  }
}
```

**Compound-extension caveat**

Tarn test files use the compound extension `.tarn.yaml`, but Claude Code's current LSP plugin format registers language servers by **simple file extension** (`.yaml`). This plugin necessarily claims every `.yaml` and `.yml` file in any project where it's installed — any other YAML language server you had running (Kubernetes, Compose, CI configs) will be shadowed for those files while this plugin is active.

Install at `--scope project` in Tarn-focused repos only. **Do not install at user scope.** If you need side-by-side Tarn + generic YAML intelligence in the same repo, this plugin is not the right fit yet; the gap is tracked as a follow-up to request either compound-extension support or a glob-based file-pattern matcher in Claude Code's LSP plugin schema.

**Verify it works**

Inside Claude Code after install, run `/plugin`, switch to the **Installed** tab, and confirm `tarn-lsp@tarn` is listed with no errors. Then open any `.tarn.yaml` file and:

1. Introduce a typo in a schema key — Claude's diagnostics indicator (press **Ctrl+O**) shows the parser error with a precise line range.
2. Hover over `{{ env.api_key }}` — resolved value and source file appear.
3. Start typing `{{ capture.` — captures from earlier steps in the current test autocomplete.

**Troubleshooting**

- `Executable not found in $PATH`: `tarn-lsp` isn't on your `$PATH`. Run `which tarn-lsp`. If empty, install via `cargo install --path tarn-lsp` or symlink the debug binary into `~/.local/bin`.
- LSP not attaching to `.tarn.yaml` buffers: confirm the plugin is **enabled** (`/plugin` → **Installed** tab) and run `/reload-plugins`. If another plugin is also claiming `.yaml`, there's an ordering conflict — disable the other plugin in this project.
- Diagnostics silent on malformed YAML: `tarn-lsp` degrades broken-input formatting to a no-op by design. The parser diagnostics still fire — check the diagnostics panel, not hover.

Full plugin README with extra context: [`editors/claude-code/tarn-lsp-plugin/README.md`](../editors/claude-code/tarn-lsp-plugin/README.md).

### opencode

[opencode](https://opencode.ai) registers LSP servers directly from its config file — no plugin wrapper. This repo commits an [`opencode.jsonc`](../opencode.jsonc) at the root so agents running inside it get `tarn-lsp` automatically.

**Prerequisites**

1. opencode installed (see [opencode.ai/docs](https://opencode.ai/docs/)).
2. `tarn-lsp` on `$PATH`: `cargo install --path tarn-lsp` from a checkout of this repo.

**Config** (`opencode.jsonc` at your repo root):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "lsp": {
    "tarn": {
      "command": ["tarn-lsp"],
      "extensions": [".yaml", ".yml"]
    }
  }
}
```

**Compound-extension caveat:** opencode matches LSP servers by final-component extension (`.yaml` / `.yml`), not by full suffix, so this entry claims every YAML file in the workspace — not just `.tarn.yaml`. Commit it at project level in Tarn-focused repos only; do not add it to your global `~/.config/opencode/config.json` if you also edit unrelated YAML through opencode. Same limitation as Claude Code, tracked upstream on both sides.

Full opencode install flow (including the MCP server and `tarn-api-testing` skill): [`editors/opencode/README.md`](../editors/opencode/README.md).

## Smoke test

Once `tarn-lsp` is wired into your editor, this four-step smoke test exercises every Phase L1 feature. Save this as `smoke.tarn.yaml`:

```yaml
name: lsp smoke test
env:
  base_url: http://localhost:3000
  api_key: secret
tests:
  main:
    steps:
      - name: list
        request:
          method: GET
          url: "{{ env.base_url }}/items"
          headers:
            Authorization: "Bearer {{ env.api_key }}"
        capture:
          first_id: $.data[0].id
      - name: fetch
        request:
          method: GET
          url: "{{ env.base_url }}/items/{{ capture.first_id }}"
```

Then:

1. **Diagnostics** — change `request:` (line 9) to `requestx:` and save. The editor should show a red squiggle under `requestx:` with source `tarn` and code `tarn_validation`. Undo the typo; the squiggle disappears on the next publish.
2. **Hover** — hover over `env.base_url` in the `url:` line (line 11). The tooltip shows the resolved value (`http://localhost:3000`), the source layer (`inline`), and the active environment name. Then, on the `fetch` step's URL line, hover over `capture.first_id` — the tooltip shows which step declared the capture and its source JSONPath.
3. **Completion** — delete the text between `{{ ` and ` }}` on line 11 so the line reads `          url: "{{ env. }}/items"`, put the cursor immediately after the `.`, and trigger completion. You see `base_url` and `api_key`, each with the resolved value in the `detail` field.
4. **Document symbols** — open the editor's outline / go-to-symbol view. You see `lsp smoke test → main → [list, fetch]`. Clicking `fetch` jumps to `- name: fetch` with the range covering the whole step body.

If any of the four steps behaves differently, see the troubleshooting section at the bottom of this document.

## Design choices

- **Sync, not async**. The server uses `lsp-server` (from rust-analyzer) plus `lsp-types`. No `tokio`, no `async-std`, no `tower-lsp`. This matches the rest of the Tarn workspace, where only the HTTP client inside `tarn` itself needs a runtime.
- **Full document sync, not incremental**. Tarn's parser operates on whole files; incremental sync would buy nothing and would require re-threading range arithmetic through every feature. Phase L2 may revisit this if profiling shows parse time dominates.
- **Library + binary**. `tarn-lsp` exposes a small library (`src/lib.rs`) so integration tests can drive the lifecycle over `lsp_server::Connection::memory()` without spawning a subprocess. The binary (`src/main.rs`) is a trivial wrapper that calls `tarn_lsp::run()`.
- **`DocumentStore` is in-memory only**. The server never reads from disk. This keeps monorepo behaviour predictable — the server sees exactly what the client has opened, nothing more.
- **Single yaml-rust2 second pass for ranges**. Diagnostic ranges and document-symbol ranges come from the same scanner family (`tarn::parser_locations` and `tarn::outline`), so the outline is guaranteed to point at the same lines the diagnostics do.

## Troubleshooting

### `tarn-lsp` binary not found

The client will report "language server binary not found" or "failed to spawn". Check:

- `which tarn-lsp` — should print a path. If empty, `cargo install --path tarn-lsp` did not run or your shell has not picked up `~/.cargo/bin`.
- Absolute-path fallback: most clients accept an absolute path (e.g. `/Users/you/.cargo/bin/tarn-lsp` or a `target/release/tarn-lsp` from this repo). Use that if your client cannot resolve `tarn-lsp` via `$PATH`.

### LSP client does not attach to `.tarn.yaml` files

The most common cause is the file type: your client needs a filetype mapping from `.tarn.yaml` → `tarn`. Neovim users: see the `vim.filetype.add` snippet above. Other clients usually have a similar "file association" or "language assignment" setting.

The second-most common cause: the client only starts the server once a matching document is opened. Open a `.tarn.yaml` file and check the client's "server status" view.

### Diagnostics do not show up

Save the file — some clients only publish diagnostics on save regardless of the server advertising change events. If diagnostics still do not appear, open the client's "language server" output channel and look for a `tarn-lsp 0.5.5 initialized` banner. If that banner is missing, the client never successfully spawned the binary; see the "binary not found" section above.

If the banner is present but diagnostics are empty, run `tarn validate path/to/file.tarn.yaml` from a terminal in the same directory. If the CLI reports errors but the LSP does not, file an issue with the file path and expected diagnostics — that is a real bug, not a configuration problem.

The banner string updates with every release — at the time of writing it is `tarn-lsp 0.5.7 initialized`.

### Document symbols pane is empty

Some clients only populate the outline view after the first successful parse. Trigger a change (even an inconsequential whitespace edit) and save. The outline should repopulate within 300ms.

If the pane is still empty, the file may not parse as YAML at all — the scanner returns an empty outline when `yaml-rust2` cannot load the document. Check the diagnostics view for a `yaml_syntax` error.

## Roadmap

Phase L1 is the MVP. Phase L2 and L3 pick up the long tail of LSP features and are deliberately out of scope for this release. They will land as new Linear tickets under Epic NAZ-289 (or a successor epic if L2 grows large enough to warrant its own).

### Phase L2 — navigation and refactor (complete)

- [x] **`textDocument/definition`** (NAZ-297) — jump from `{{ env.x }}` / `{{ capture.y }}` to where the variable is declared.
- [x] **`textDocument/references`** (NAZ-298) — find every use of a capture (per-test, current file) or env key (every `.tarn.yaml` under the workspace root, bounded at 5000 files).
- [x] **`textDocument/rename`** (NAZ-299) — rename a capture (per-test, current file) or env key (every env source file that declares it + every workspace use site) in a single `WorkspaceEdit`, with identifier validation and per-scope collision detection.
- [x] **`textDocument/codeLens`** (NAZ-300) — `Run test` / `Run step` inline actions with stable `tarn.runTest` / `tarn.runStep` command IDs; clients dispatch the commands themselves by shelling out to `tarn run --select <selector>`.

**Phase L2 COMPLETE.** Epic NAZ-296 is closed. The follow-ups below were previously bundled with L2 and remain open as general housekeeping:

- **Claude Code config integration** — shipped as NAZ-310: `editors/claude-code/tarn-lsp-plugin/`, surfaced from the repo-root marketplace at `.claude-plugin/marketplace.json` alongside the `tarn` MCP + skill plugin. See the Claude Code section above for install instructions. The remaining gap is Claude Code's lack of compound-extension (`.tarn.yaml`) support in its LSP plugin schema; tracked as an upstream feedback request.
- **VS Code extension migration** — migrate `editors/vscode/` off its direct providers onto `tarn-lsp`, so there is one implementation of every language feature. Tracked under Phase V (Epic NAZ-308, scaffolding shipped as NAZ-309).

### Phase L3 — editing polish (complete)

- [x] **`textDocument/formatting`** (NAZ-302) — whole-document formatting via `tarn::format::format_document`; identical output to `tarn fmt` from the terminal. Range formatting is deliberately **not** supported.
- [x] **`textDocument/codeAction`** (NAZ-303, NAZ-304, NAZ-305) — extract env var, capture this field, scaffold assert.body from last response, and fix-plan quick fix via shared `tarn::fix_plan` library.
- [x] **Nested schema completion** (NAZ-306) — schema-aware child keys for cursors nested below the top-level / step mapping.
- [x] **JSONPath evaluator** (NAZ-307) — hover evaluation against the step's last recorded response + `workspace/executeCommand` for `tarn.evaluateJsonpath` with `{ matches: [...] }` return envelope.

**Phase L3 COMPLETE.** Epic NAZ-301 is closed. **Phase V** (VS Code extension migration onto `tarn-lsp`) is the next coordinated initiative.

## Links

- Epic: **NAZ-289 — tarn-lsp Language Server for Claude Code and non-VS-Code editors**
- Sibling doc: [`docs/VSCODE_EXTENSION.md`](./VSCODE_EXTENSION.md)
- Crate: [`tarn-lsp/`](../tarn-lsp/)
- Capabilities source of truth: [`tarn-lsp/src/capabilities.rs`](../tarn-lsp/src/capabilities.rs)
- Outline extractor (shared with diagnostics): [`tarn/src/outline.rs`](../tarn/src/outline.rs)
