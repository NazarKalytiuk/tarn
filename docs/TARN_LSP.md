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

**Phase L1 MVP: complete.** The roadmap footer at the bottom of this document lists Phase L2 and L3 work that is deliberately out of scope for this release.

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

Save the file — some clients only publish diagnostics on save regardless of the server advertising change events. If diagnostics still do not appear, open the client's "language server" output channel and look for a `tarn-lsp 0.5.4 initialized` banner. If that banner is missing, the client never successfully spawned the binary; see the "binary not found" section above.

If the banner is present but diagnostics are empty, run `tarn validate path/to/file.tarn.yaml` from a terminal in the same directory. If the CLI reports errors but the LSP does not, file an issue with the file path and expected diagnostics — that is a real bug, not a configuration problem.

### Document symbols pane is empty

Some clients only populate the outline view after the first successful parse. Trigger a change (even an inconsequential whitespace edit) and save. The outline should repopulate within 300ms.

If the pane is still empty, the file may not parse as YAML at all — the scanner returns an empty outline when `yaml-rust2` cannot load the document. Check the diagnostics view for a `yaml_syntax` error.

## Roadmap

Phase L1 is the MVP. Phase L2 and L3 pick up the long tail of LSP features and are deliberately out of scope for this release. They will land as new Linear tickets under Epic NAZ-289 (or a successor epic if L2 grows large enough to warrant its own).

### Phase L2 — navigation and refactor (future)

- **`textDocument/definition`** — jump from `{{ env.x }}` / `{{ capture.y }}` to where the variable is declared.
- **`textDocument/references`** — find every use of a capture or env key from its declaration site.
- **`textDocument/rename`** — rename a capture or env key across the file safely.
- **`textDocument/codeLens`** — inline "Run this test" / "Run this step" affordances that invoke the CLI.
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
