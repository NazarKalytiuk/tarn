# Tarn LSP (`tarn-lsp`)

This document is the canonical spec for `tarn-lsp`, the Language Server Protocol implementation for Tarn test files. `tarn-lsp` is the editor-agnostic counterpart to the VS Code extension in `editors/vscode`: it ships as a single stdio binary that any LSP 3.17 client can spawn — Claude Code, Neovim (built-in `vim.lsp`), Helix, Emacs (`eglot` / `lsp-mode`), Zed, Sublime (`LSP` package), and anything else that speaks LSP.

Phase L1 of Epic NAZ-289 delivers the minimum viable server. Nothing in this document is user-facing until the phase checklist below reaches "shipped" on every row — `tarn-lsp` only becomes advertised on the README and install instructions once NAZ-294 lands.

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

Phase L1 is delivered as five tickets under Epic NAZ-289. Each ticket flips on exactly one capability in `tarn-lsp/src/capabilities.rs`.

- [x] **L1.1 — bootstrap (NAZ-290)**: workspace crate, stdio lifecycle (`initialize` / `initialized` / `shutdown` / `exit`), in-memory `DocumentStore`, full text document sync, integration tests over `Connection::memory()`. This ticket ships the skeleton only — no language intelligence yet.
- [x] **L1.2 — diagnostics (NAZ-291)**: parse every open document through `tarn::parser` on `didOpen`/`didChange`/`didSave` and publish YAML + schema diagnostics via `textDocument/publishDiagnostics`. Debounced at 300ms on `didChange`; flushes immediately on open and save; clears on close.
- [ ] **L1.3 — hover (NAZ-292)**: `textDocument/hover` resolves `{{ env.x }}` and `{{ capture.x }}` references, assertion keywords, and step fields to their documentation.
- [ ] **L1.4 — completion (NAZ-293)**: `textDocument/completion` offers snippet expansions, assertion keywords, env/capture identifiers, and HTTP method names with trigger characters `{`, `.`, `"`.
- [ ] **L1.5 — symbols + docs (NAZ-294)**: `textDocument/documentSymbol` returns the test/step tree; README and Claude Code docs are finalised and `tarn-lsp` is added to the release pipeline.

## Running locally

```bash
cargo build -p tarn-lsp              # debug build
cargo build -p tarn-lsp --release    # release build; binary at target/release/tarn-lsp
cargo test  -p tarn-lsp              # run the LSP lifecycle tests
```

The binary reads LSP messages from stdin and writes them to stdout. On `initialize` it also writes a one-line server info banner to stderr — LSP clients surface this in their "Language Server" output pane, so it is the fastest way to confirm the handshake succeeded.

## Claude Code configuration (placeholder)

```text
See NAZ-294 for the finalized Claude Code snippet — pending Phase L1 completion.
```

The configuration block above is intentionally a placeholder. Per the rule in `CLAUDE.md` ("never reference URLs, domains, or external resources without verifying they exist"), the real snippet will only be added once `tarn-lsp` ships in the release pipeline and the full feature surface is live. Until then, do not fabricate a configuration and do not copy-paste the contents of this placeholder block into Claude Code.

## Diagnostics

`tarn-lsp` publishes diagnostics via `textDocument/publishDiagnostics` on three triggers:

- **`didOpen`** — immediately, so opening a `.tarn.yaml` surfaces problems before the first keystroke.
- **`didSave`** — immediately, matching the "save to recheck" muscle memory most clients already teach.
- **`didChange`** — debounced at 300ms. A burst of keystrokes collapses into a single publish once the buffer has been quiet for 300ms. The main loop uses `recv_timeout` against `lsp-server`'s crossbeam channel — no threads, no runtime.

On `didClose` the server publishes a `publishDiagnostics` with an empty `diagnostics` array for the closed URI so stale problems disappear from the client.

Each diagnostic is produced by [`tarn::validation::validate_document`](../tarn/src/validation.rs) — the same parser + schema + semantic validation path `tarn validate` uses from `tarn/src/main.rs`. Nothing is shelled out; Tarn's library surface is called in-process. Every diagnostic carries:

| Field        | Value                                                                                     |
| ------------ | ----------------------------------------------------------------------------------------- |
| `range`      | Derived from NAZ-260 `Location` metadata (1-based line/column → 0-based LSP `Position`). When the underlying error has no location, falls back to a zero-width range at `(0, 0)`. |
| `severity`   | `Error` for YAML-syntax, shape, parse, and cross-field semantic failures. `Warning` is reserved for future soft checks (no checks emit it today). |
| `source`     | Always `"tarn"`.                                                                           |
| `code`       | One of `yaml_syntax`, `tarn_parse`, `tarn_validation`.                                     |
| `message`    | Human-readable text stripped of the `thiserror` prefix and the redundant file path prefix. |

See `tarn-lsp/src/diagnostics.rs` for the conversion and `tarn-lsp/src/debounce.rs` for the pure debounce helper. End-to-end coverage lives in `tarn-lsp/tests/diagnostics_test.rs`.

## Design choices

- **Sync, not async**. The server uses `lsp-server` (from rust-analyzer) plus `lsp-types`. No `tokio`, no `async-std`, no `tower-lsp`. This matches the rest of the Tarn workspace, where only the HTTP client inside `tarn` itself needs a runtime.
- **Full document sync, not incremental**. Tarn's parser operates on whole files; incremental sync would buy nothing and would require re-threading range arithmetic through every feature. Phase L2 may revisit this if profiling shows parse time dominates.
- **Library + binary**. `tarn-lsp` exposes a small library (`src/lib.rs`) so integration tests can drive the lifecycle over `lsp_server::Connection::memory()` without spawning a subprocess. The binary (`src/main.rs`) is a trivial wrapper that calls `tarn_lsp::run()`.
- **`DocumentStore` is in-memory only**. The server never reads from disk. This keeps monorepo behaviour predictable — the server sees exactly what the client has opened, nothing more.

## Links

- Epic: **NAZ-289 — tarn-lsp Language Server for Claude Code and non-VS-Code editors**
- Sibling doc: [`docs/VSCODE_EXTENSION.md`](./VSCODE_EXTENSION.md)
- Crate: [`tarn-lsp/`](../tarn-lsp/)
- Capabilities source of truth: [`tarn-lsp/src/capabilities.rs`](../tarn-lsp/src/capabilities.rs)
