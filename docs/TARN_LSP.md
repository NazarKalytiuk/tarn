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

## Installation

`tarn-lsp` is a Cargo workspace crate in this repository. Until the crate is published to crates.io, the only supported install path is building from the workspace:

```bash
# from the root of the hive repo
cargo install --path tarn-lsp
```

After install, `which tarn-lsp` should print a path inside `~/.cargo/bin/`. That binary is what every LSP client below spawns.

For local development (no install step, useful if you are hacking on the server):

```bash
cargo build -p tarn-lsp --release
# binary now lives at ./target/release/tarn-lsp — point your LSP client at this path
```

The published-crate install path (`cargo install tarn-lsp`) becomes available when the crate is pushed to crates.io; that is tracked as a Phase L2 follow-up. Until then, please use `cargo install --path`.

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
| **Built-in function**      | `{{ $uuid }}`              | The canonical call signature and a one-sentence docstring for each of `$uuid`, `$timestamp`, `$now_iso`, `$random_hex(n)`, and `$random_int(min, max)`. Unknown names get a friendly "not a recognized Tarn built-in" hint listing every supported function. |
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
| **Inside `{{ $<prefix> }}`**           | `$` after `{{`           | The five Tarn built-ins (`$uuid`, `$timestamp`, `$now_iso`, `$random_hex`, `$random_int`), the last two as snippets.   | `Function`  |
| **Blank YAML mapping-key line**        | newline / manual trigger | Schema-valid keys for the cursor's scope — root, test group, or step.                                                  | `Property`  |

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

Claude Code's LSP configuration path is still evolving and is not yet pinned down in public documentation. Rather than fabricate a `claude-code.lsp` config key that may not exist, here is what we can commit to today:

- `tarn-lsp` is a **standard** LSP 3.17 stdio server — it does not require any Claude-specific bridging.
- **Identifying the exact Claude Code config file and schema is tracked as a Phase L2 follow-up.** When the schema is stable we will drop a concrete JSON block into this section.
- If you are wiring `tarn-lsp` into Claude Code today and the official docs do not yet cover it, open an issue on the `hive` repo and we will add a tested snippet.

Please do **not** copy-paste a config key inferred from other LSP clients into Claude Code — the Claude Code harness reads its settings from a different layout, and a wrong key is silently ignored, which is worse than a missing section.

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

- **Claude Code config integration** — finalise the Claude Code LSP config snippet once the harness schema is public.
- **VS Code extension migration** — migrate `editors/vscode/` off its direct providers onto `tarn-lsp`, so there is one implementation of every language feature.

### Phase L3 — polish and advanced refactor (future)

- **`textDocument/formatting`** — canonicalise `.tarn.yaml` indentation and key order.
- **`textDocument/codeAction`** — quick-fix squiggle hints, including integration with `tarn_fix_plan`.
- **Inline JSONPath hover/completion** — resolve `$.foo.bar` against a cached response body for step-level assertions.
- **Workspace-wide symbol search** — `workspace/symbol` across every open `.tarn.yaml` in the project.

Phase L2 will begin when Phase L1 is proven with real users. If you hit a rough edge with the MVP or want one of the L2 items to move earlier, please open an issue — usage data drives the order.

## Links

- Epic: **NAZ-289 — tarn-lsp Language Server for Claude Code and non-VS-Code editors**
- Sibling doc: [`docs/VSCODE_EXTENSION.md`](./VSCODE_EXTENSION.md)
- Crate: [`tarn-lsp/`](../tarn-lsp/)
- Capabilities source of truth: [`tarn-lsp/src/capabilities.rs`](../tarn-lsp/src/capabilities.rs)
- Outline extractor (shared with diagnostics): [`tarn/src/outline.rs`](../tarn/src/outline.rs)
