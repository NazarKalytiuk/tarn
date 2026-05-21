# ADR 0001 — Tarn Studio: native desktop app on Tauri

- **Status:** Accepted
- **Date:** 2026-05-21
- **Authors:** Nazar Kalytiuk (@nazarkalytiuk)
- **Supersedes:** —

## Context

Tarn currently ships three user-facing surfaces:

1. The `tarn` CLI (canonical runner).
2. The `tarn-vscode` extension (v0.12.0 on VS Code Marketplace and Open VSX), a thin TypeScript shell around the CLI with Test Explorer integration, NDJSON streaming, CodeLens, MCP backend, and an experimental LSP client.
3. The `tarn-lsp` and `tarn-mcp` daemons, consumed by editors and AI clients.

The remaining gap is users who do not live inside VS Code (JetBrains, Vim, Sublime, Zed, designers, manual QA, demo presenters). For them the only option today is the terminal. We need a native, standalone IDE-like experience that is not coupled to any editor.

This ADR fixes the architecture for that desktop app, named **Tarn Studio**.

## Decision

### 1. Stack

| Layer | Choice | Rationale |
|---|---|---|
| Shell | **Tauri 2.x** | Rust backend matches existing workspace; small bundles vs Electron; native WebView; first-class sidecar support; mature signing/notarization story. |
| Frontend framework | **Solid.js 1.x** | Smallest bundle and lowest runtime overhead of the reactive options; fine-grained reactivity is a good fit for live NDJSON streaming into a large test tree. |
| Styling | **Tailwind v4** | Same conventions already used in `docs/site/`; zero-config dark theme. |
| Editor | **CodeMirror 6** | Lighter than Monaco (~80 KB vs ~3 MB), composable extensions, ergonomic Solid integration, LSP via `@open-rpc/client-js` over Tauri events. |
| Charts | **uPlot** | Lowest CPU cost for per-step latency graphs in Phase 3. |
| Build (frontend) | **Vite** | Tauri-default. |
| Package manager | **pnpm** | Already installed locally (`10.32.1`); fastest installs. |

### 2. Repository layout

Tarn Studio ships as a **workspace member** of the existing repo:

```
hive-api-test/
├── tarn/                # CLI core (existing)
├── tarn-lsp/            # LSP daemon (existing)
├── tarn-mcp/            # MCP daemon (existing)
├── demo-server/         # Existing
├── editors/vscode/      # VS Code extension (existing)
└── tarn-desktop/        # NEW — Tarn Studio
    ├── package.json     # Solid + Tailwind + CodeMirror + @tauri-apps/cli
    ├── vite.config.ts
    ├── tsconfig.json
    ├── index.html
    ├── src/             # Solid frontend
    │   ├── main.tsx
    │   ├── App.tsx
    │   ├── components/
    │   ├── ipc/         # typed wrappers around Tauri invoke/listen
    │   └── stores/      # Solid stores
    └── src-tauri/       # Tauri Rust backend
        ├── Cargo.toml
        ├── tauri.conf.json
        ├── build.rs
        └── src/
            ├── main.rs
            ├── lib.rs
            ├── commands/      # invoke handlers
            └── sidecar/       # tarn + tarn-lsp process management
```

`tarn-desktop/src-tauri/Cargo.toml` declares its own minimal `[workspace]` table so the desktop crate is its own Cargo workspace, decoupled from the top-level CLI workspace. Studio versions independently of the CLI — see section 7 — so its `target/` and `Cargo.lock` stay out of the CLI's release pipeline and vice versa. Current Studio version: `0.1.0`.

### 3. Integration with Tarn core

**Sidecar bundling**, not in-process library. Both `tarn` and `tarn-lsp` are bundled as external binaries via `tauri.conf.json > bundle > externalBin` and shipped inside the app bundle.

Rationale (vs linking `tarn` as a Rust crate):

- Stable, already-tested process boundary (NDJSON, exit codes, structured JSON).
- Zero risk of UI panics or memory pressure taking down the runner.
- One artifact pipeline can release CLI, MCP, LSP, and Studio together.
- VS Code extension already uses the same contract — we keep one source of truth.

The frontend never spawns processes directly. All process management is done by the Rust backend in `src-tauri/src/sidecar/`, which exposes typed Tauri commands and events to the WebView.

### 4. IPC contract

Frontend ↔ Backend uses **Tauri commands** (request/response) and **Tauri events** (push). Wire format is JSON, and event payloads mirror the existing `tarn run --ndjson` schema 1:1 so the schema stays canonical.

#### Commands (Frontend → Backend)

```ts
open_project(path: string) → ProjectMeta
discover_tests(project: string) → { files: TestFile[] }              // wraps `tarn list --format json`
run_tests(selector: RunSelector) → { run_id: string }                // spawns `tarn run --ndjson`
cancel_run(run_id: string) → { ok: true }                            // sends SIGTERM
validate(project: string, file?: string) → Diagnostic[]              // wraps `tarn validate --format json`
format_file(path: string) → { ok: true }                             // wraps `tarn fmt`
list_environments(project: string) → Env[]                           // wraps `tarn env --json`
get_run_history(project: string, limit: number) → RunSummary[]       // reads `.tarn/last-run.json` + fixtures
read_fixture(project: string, step_path: string) → Fixture            // reads `.tarn/fixtures/<...>.json`
import_curl(text: string) → { yaml: string }                         // wraps `tarn import-curl`
import_openapi(path: string) → { yaml: string }                      // wraps `tarn init --openapi`
import_hurl(path: string) → { yaml: string }                         // wraps `tarn import-hurl`
fix_plan(run_id: string) → FixPlan                                   // wraps `tarn fix-plan`
```

`RunSelector` mirrors `--select FILE[::TEST[::STEP]]` plus `tag`, `env`, `vars`. Single source of truth for the selector grammar stays in the CLI.

#### Events (Backend → Frontend)

```ts
"test:run-started"     { run_id, started_at }
"test:file-started"    { run_id, file, file_name }
"test:step-finished"   { run_id, file, test, step, step_index, status, duration_ms, phase,
                         progress, assertion_failures?, error_code?, failure_category? }
"test:test-finished"   { run_id, file, test, status, duration_ms, steps: {failed, passed, total} }
"test:file-finished"   { run_id, file, file_name, status, duration_ms, summary }
"test:run-done"        { run_id, duration_ms, summary }
"lsp:diagnostic"       { file, diagnostics }
"sidecar:log"          { source: "tarn" | "tarn-lsp", level, message }
```

The Rust backend reads NDJSON line-by-line from the sidecar `stdout`, deserializes into one strongly-typed enum (`TarnEvent`), tags it with the originating `run_id`, and emits via `app.emit_to("main", topic, payload)`. The frontend subscribes through a typed wrapper in `src/ipc/`.

There is **no `step_started` event** in the current CLI stream (verified against `tarn 0.13.1` output). Phase 1 infers the "running" UI state from the absence of `step_finished` for upcoming steps within an already-`file_started` file. We may add a `step_started` NDJSON event upstream if the inferred UX is insufficient — that is a separate ADR.

### 5. LSP integration

In Phase 1, `tarn-lsp` is **not** wired in — diagnostics come from polling `tarn validate --format json` on save. This avoids the complexity of an LSP-in-WebView bridge for the MVP.

In Phase 2, the Rust backend spawns one `tarn-lsp` per project, owns the stdio pipes, and proxies LSP JSON-RPC messages to the WebView through a single Tauri event channel (`"lsp:message"`). CodeMirror connects through a thin client that speaks LSP over those events. We deliberately reuse one daemon per project rather than per-document for performance.

### 6. Distribution

| Platform | Format | Signing | Updater |
|---|---|---|---|
| macOS | `.dmg` (universal: aarch64 + x86_64) | Apple Developer ID + notarization | Tauri Updater plugin, GitHub Releases as endpoint |
| Windows | `.msi` | Self-signed initially, Authenticode later | Tauri Updater plugin |
| Linux | `.AppImage` + `.deb` | Unsigned (community standard) | Tauri Updater plugin |

Release pipeline is added to the existing GitHub Actions workflow that already builds `tarn` per platform. Studio reuses those artifacts as its sidecar inputs.

Bundle size budget: ≤ 25 MB compressed per platform DMG/MSI (tarn ~8 MB + tarn-lsp ~6 MB + WebView assets + Tauri shell). Above 25 MB triggers a review.

### 7. Versioning and release coordination

- **Tarn Studio versions independently of the CLI.** Studio carries its own semver in `tarn-desktop/src-tauri/Cargo.toml`, its own changelog at `tarn-desktop/CHANGELOG.md`, and its own git tag namespace (`studio-vX.Y.Z`, distinct from the CLI's `vX.Y.Z`). First Studio release: `studio-v0.1.0`. The earlier draft of this ADR called for workspace lockstep; that decision was reversed once the desktop crate was pulled out of the top-level workspace.
- The `package.json` field `"tarn": { "minVersion": "X.Y.Z" }` is **not** used (unlike the VS Code extension), because the sidecar is bundled. The bundled CLI version is pinned in `tauri.conf.json > bundle > externalBin` at build time.
- A Studio release does not imply a CLI release, and vice versa. The bundled CLI version can lag the latest CLI by minor versions as long as the IPC contract holds.

### 8. Out of scope (Phase 1)

The following are deferred to later phases and not architected here:
- Latency graphs and per-step history charts (Phase 3).
- Curl / OpenAPI / Hurl import wizards (Phase 3).
- Run history view across past runs (Phase 3).
- Multi-window (Phase 4+).
- Cloud sync, team dashboards, account systems — explicitly **never** in scope. Stays consistent with the VS Code extension's non-goals.

## Consequences

### Positive

- Reaches users outside VS Code without compromising the CLI's primary surface.
- Reuses the existing `--ndjson` / `--format json` contracts → no new schemas to maintain.
- Sidecar packaging keeps the runner identical to CI, eliminating "works in IDE, fails in CI" divergence.
- Solid + CodeMirror keeps bundle and runtime cost low, important for an always-resident desktop app.

### Negative

- A second UI codebase to maintain alongside `editors/vscode/`. Mitigated by sharing the IPC schema (NDJSON) and keeping both as thin shells around the CLI.
- Native distribution overhead: code signing, notarization, multi-platform CI matrix.
- macOS Apple Developer membership cost (~$99/year) becomes a fixed expense.

### Risks

| Risk | Mitigation |
|---|---|
| LSP-in-WebView bridge proves fragile | Phase 1 uses validate-polling; Phase 2 introduces LSP only after editor UX is otherwise solid. |
| WebView differences across platforms (WebKit on macOS, WebView2 on Windows, WebKitGTK on Linux) | Lock targets to evergreen baselines; pin CSS to Tailwind v4 defaults; CI smoke-tests on all three. |
| User confusion: CLI vs Studio vs VS Code extension | Docs split: `docs/INDEX.md` keeps CLI primary; Studio gets its own landing in `docs/site/`. |

## Phasing

- **Phase 1 — Foundation (~3 weeks).** Project picker, test tree with live status, step detail panel, NDJSON streaming pipeline, packaging skeleton.
- **Phase 2 — Authoring (~4 weeks).** CodeMirror editor, `tarn-lsp` integration, env management, tag/env pickers, `tarn fmt`.
- **Phase 3 — Power features (~4 weeks).** Run history, latency graphs, curl/OpenAPI/Hurl import, fix-plan view.
- **Phase 4 — Distribution (~1-2 weeks).** Signing, notarization, auto-updater, store/landing-page push.

Detailed task breakdown for Phase 1 lives in the session task list, not in this ADR.
