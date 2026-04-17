# Tarn VS Code Extension

**Tarn VS Code extension 0.6.1**, publisher `nazarkalytiuk`, distributed on the [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=nazarkalytiuk.tarn-vscode) and [Open VSX](https://open-vsx.org/extension/nazarkalytiuk/tarn-vscode).

This file is the architecture and contract document for contributors and downstream integrators. It describes how the shipped extension is wired together, which Tarn-side library surfaces and CLI flags it depends on, how its streaming protocol is specified, and what the Phase V migration to `tarn-lsp` looks like.

The **user manual** lives at [`editors/vscode/README.md`](../editors/vscode/README.md): features, screenshots, the full settings matrix, trusted/untrusted workspace behavior, and remote development instructions. This document deliberately does not duplicate that material.

## Goals

- First-class Test Explorer integration: discover, run, cancel, streaming results, per-test and per-step execution, failure diff in peek view.
- Inline authoring: CodeLens, gutter icons, hover, completion, definition, references, rename, schema validation, snippets, `tarn fmt` as a document formatter.
- Failure UX that beats the terminal: diff view, request/response rendering in `TestMessage`, "reveal in editor" on the exact failing line, fix-plan hints.
- Workflow glue: environment picker, tag picker, status bar, run history, redaction-aware output.
- Zero-config for standard repos; explicit settings for monorepos, custom binaries, Remote SSH, Dev Containers, WSL, Codespaces.
- Tarn-side contracts stay small, additive, and backwards compatible. No forks.

## Non-Goals

- No reimplementation of the runner in TypeScript. The extension is a thin stateful shell around the `tarn` CLI, with an experimental `tarn-lsp` front-end behind a flag.
- No proprietary file format. `.tarn.yaml` stays canonical.
- No web extension (`vscode.dev`) support. The extension spawns native binaries.
- No cloud sync, team dashboards, or any network-side features.

## Current Architecture

```
┌──────────────────── VS Code Extension Host ────────────────────┐
│                                                                │
│  ┌─ Test Controller ─┐     ┌─── CodeLens Providers ───┐        │
│  │  discovery        │     │  per-test / per-step     │        │
│  │  run / cancel     │     │  Run | Dry Run | Run step│        │
│  │  streaming        │     └──────────────┬───────────┘        │
│  └────────┬──────────┘                    │                    │
│           │                               │                    │
│  ┌────────▼─────────── Core Services ─────▼──────┐             │
│  │  WorkspaceIndex   YamlAst   EnvService         │             │
│  │  ResultMapper     RunHistoryStore              │             │
│  │  StatusBar        OutputChannel  Diagnostics   │             │
│  │  Notifications    FixPlanView    SecretStorage │             │
│  └────────┬───────────────────────────┬──────────┘             │
│           │                           │                        │
│  ┌────────▼────────┐        ┌─────────▼────────────────────┐   │
│  │ TarnProcessRunner│       │ LspClient (experimental)     │   │
│  │ spawns tarn CLI │        │ vscode-languageclient ⇆      │   │
│  │ run/validate/   │        │   tarn-lsp stdio             │   │
│  │ list/fmt/env    │        │ behind tarn.experimental-    │   │
│  └────────┬────────┘        │   LspClient flag (default off)│   │
│           │                 └─────────┬────────────────────┘   │
│           ▼                           ▼                        │
│   ┌─────────────┐                ┌─────────────┐               │
│   │  tarn CLI   │                │  tarn-lsp   │               │
│   └─────────────┘                └─────────────┘               │
└────────────────────────────────────────────────────────────────┘
```

Two stacks, one host. `TarnProcessRunner` (`src/backend/`) is the default backend: it spawns the `tarn` CLI for every run, validation, list, format, and environment query. `LspClient` (`src/lsp/`, added in 0.5.1 / 0.6.0) is the experimental language-client front-end that spawns `tarn-lsp` behind the `tarn.experimentalLspClient` setting; as of 0.6.1 it boots side by side with the direct providers but **no language feature has been routed through it yet**. See [Phase V — LSP migration plan](#phase-v--lsp-migration-plan) below.

Since NAZ-279 the extension also ships a second, opt-in backend that talks to `tarn-mcp` over JSON-RPC. See [Backends](#backends) for the full contract.

## Backends

The extension picks one of two backends at activation based on the `tarn.backend` setting:

| Setting | Backend | Implementation | Notes |
|---|---|---|---|
| `tarn.backend: "cli"` (default) | `TarnProcessRunner` | Spawn `tarn` per command. | Full feature parity; NDJSON streaming, `--select`, `--cookie-jar-per-test`, bench, HTML reports, curl export, `tarn fmt`, `tarn env --json`. |
| `tarn.backend: "mcp"` | `TarnMcpClient` | Keep one long-lived `tarn-mcp` process per workspace; dispatch JSON-RPC requests over stdio. | Lower per-command overhead (no process spawn); fewer features. |

The MCP backend implements the same `TarnBackend` interface as the CLI backend, so `RunHandler`, `WorkspaceIndex`, `ResultMapper`, and every other consumer stays backend-agnostic.

### JSON-RPC surface

`TarnMcpClient` performs the standard MCP `initialize` handshake once per workspace, then dispatches each operation as a `tools/call` request:

| Extension method | MCP tool | Notes |
|---|---|---|
| `run()` | `tarn_run` | `cwd` is threaded through the `arguments` object so the server can resolve relative paths against the workspace root (NAZ-248). |
| `validateStructured()` / `validate()` | `tarn_validate` | The MCP tool emits a single `error: string` per invalid file; the client maps that into the CLI's `errors: [{ message }]` shape so diagnostics still render. |
| `listFile()` | delegated to CLI | MCP's `tarn_list` tool does not emit the scoped `{setup, steps, tests, teardown}` envelope required by scoped discovery; falls back to `tarn list --file <path> --format json`. |
| `runBench`, `runHtmlReport`, `exportCurl`, `initProject`, `importHurl`, `formatDocument`, `envStructured` | delegated to CLI | No MCP tool equivalent exists; `TarnMcpClient` composes over the CLI runner so the user never loses a feature when switching backends. |

Every request includes a `cwd` argument so the server can resolve relative paths against the user's workspace root. `tarn-mcp` accepts the field since NAZ-248; older servers that predate it will simply ignore the extra key.

### NDJSON limitation

MCP's `tools/call` is a single-reply JSON-RPC request — there is no server-side streaming of intermediate events. When the caller asks for `streamNdjson: true` against the MCP backend, `TarnMcpClient` degrades in one of two ways:

1. If the request also uses a feature the MCP tool does not support (`dryRun`, `selectors`, `parallel`, or multi-file runs), the client falls back to `tarn run --ndjson` via the CLI runner so streaming stays live.
2. Otherwise, the client issues a one-shot `tarn_run` tool call and, once the final report lands, **synthesizes** `file_started` / `step_finished` / `test_finished` / `file_finished` / `done` events from the report before returning. All events fire at once at the end of the run rather than live.

The Test Explorer UI contract is unchanged — all events still arrive through the `onEvent` callback — but the user experience trades live progress for a single final update. Users who care about progress should stay on `tarn.backend: "cli"`.

### Fallback to CLI on failure

`tarn.backend: "mcp"` falls back to the CLI backend (and shows a one-shot `vscode.window.showInformationMessage`) whenever:

- The `tarn-mcp` binary cannot be resolved from `tarn.mcpPath`, `PATH`, or a bundled copy.
- Spawning the child process fails.
- The MCP `initialize` handshake rejects or times out.

The fallback notification is latched for the extension-host session so the user is not spammed on every command. `deactivate()` disposes the `tarn-mcp` child process (SIGTERM then SIGKILL after a 2s grace window) in both clean and failure shutdowns.

### Core services

- `WorkspaceIndex` globs `**/*.tarn.yaml`, owns `Map<fileUri, ParsedFile>`, and invalidates on filesystem events. Incremental refresh goes through `tarn list --file <path> --format json` with an AST fallback; see [Discovery precedence](#discovery-precedence) below.
- `YamlAst` parses every test file with the `yaml` library in CST mode and keeps node → `Range` maps. This is what anchors CodeLens, document symbols, completion scope, and rename targets to exact lines.
- `EnvService` reads `tarn.env*.yaml` via `tarn env --json`, exposes the active selection via `workspaceState`, and feeds the environment picker / status bar.
- `ResultMapper` joins a Tarn NDJSON or final JSON report to the workspace index and produces `TestMessage` objects with correct locations. It prefers Tarn's own report `location: { file, line, column }` fields (added by T55 / NAZ-260) and falls back to the AST when they are missing. See [Mapping results to editor ranges](#mapping-results-to-editor-ranges).
- `RunHistoryStore` keeps the last `tarn.history.max` runs for the Run History view.

## What ships in 0.6.1

Feature summary grouped by area. See [`editors/vscode/README.md`](../editors/vscode/README.md) and [`editors/vscode/CHANGELOG.md`](../editors/vscode/CHANGELOG.md) for screenshots and per-release notes.

**Test Explorer.** Hierarchical discovery (workspace → file → test → step), `Run` and `Dry Run` profiles, streaming updates from `tarn run --ndjson`, cancellation via `SIGINT`, per-test and per-step execution via `tarn run --select`, failure peek with expected/actual diff, request, response, failure category, error code, remediation hints. Setup and teardown render as virtual collapsible nodes under each file.

**Editor.** CodeLens above every test and step (`Run`, `Dry Run`, `Run step`), schema validation for `*.tarn.yaml` via `redhat.vscode-yaml` and the bundled schema, `tarn-*` snippet library, Tarn-aware syntax highlighting for interpolation and JSONPath, document symbols, gutter icons, diagnostics anchored to the failing assertion line.

**Environments.** Auto-discovers every `tarn.env*.yaml`, surfaces them via `Tarn: Select Environment…` and the status bar entry, persists the active environment per workspace, honors `tarn.defaultEnvironment`.

**Status bar.** Active environment on the left (click → env picker), last-run summary on the right (click → output channel), live progress during a run.

**Commands.** `Tarn: Run All Tests`, `Tarn: Run Current File`, `Tarn: Dry Run Current File`, `Tarn: Validate Current File`, `Tarn: Rerun Last Run`, `Tarn: Select Environment…`, `Tarn: Set Tag Filter…`, `Tarn: Show Output`, `Tarn: Install / Update Tarn`. The canonical list is the `commands` field on `TarnExtensionApi` (see [Public API](#public-api)).

**Trusted / untrusted workspaces.** Activation is gated by `workspaceTrust`. Untrusted workspaces get read-only features only (grammar, snippets, schema validation); spawning the CLI, running tests, and `tarn validate` are all disabled until trust is granted. The public `TarnExtensionApi` returns `undefined` from `activate()` in untrusted workspaces.

**Remote Development.** Dev Container, GitHub Codespaces, WSL, and Remote SSH are all audited as first-class targets. The extension always runs on the same side as the files and the Tarn binary. `tarn.binaryPath`, `tarn.lspBinaryPath`, and `tarn.requestTimeoutMs` are declared `machine-overridable` so remote hosts can pin them without leaking into local workspace settings. Full audit writeup at [`docs/VSCODE_REMOTE.md`](VSCODE_REMOTE.md); per-target setup at [`editors/vscode/README.md`](../editors/vscode/README.md#remote-setups).

**Public API.** `TarnExtensionApi` returned from `activate()` exposes `testControllerId`, `indexedFileCount`, `commands`, and an opaque internal `testing` sub-object for the extension's own integration tests. See [Public API](#public-api).

**Localization.** String surface runs through `vscode.l10n` with a `bundle.l10n.json` catalog and a `package.nls.json` file. English is the only bundled locale; the infrastructure lets translators land new locales without touching TypeScript.

## Tarn-side contract

Every feature above is implemented against a small set of CLI flags and library surfaces on the `tarn` crate. This section is the contract the extension relies on — adding or changing anything here is a cross-repo concern.

### CLI surface

| Invocation | Used by |
|---|---|
| `tarn run --format json` | Batch run fallback when streaming is off. |
| `tarn run --ndjson` | **Primary runtime path.** Streams `file_started`, `step_finished`, `test_finished`, `file_finished`, and `done` events on stdout, one JSON object per line. Composable with `--format json=<path>` so the final report is still written to disk. Parallel mode buffers each file and emits it atomically on `file_finished` so events never interleave across files. |
| `tarn run --select FILE[::TEST[::STEP_INDEX]]` | Repeatable. Drives "Run test at cursor", "Run step", and per-item Test Explorer execution. ANDs with `--tag`. `STEP_INDEX` is zero-based and matches the `tarn::selector` parser exactly. |
| `tarn run --only-failed` | Rerun-failed workflows (Test Explorer "Run failed only"). |
| `tarn validate --format json` | On-save validation and the `Tarn: Validate Current File` command. Emits `{files: [{file, valid, errors: [{message, line, column}]}]}`; fills diagnostics in the Problems view. |
| `tarn list --file <path> --format json` | Incremental discovery refresh (see [Discovery precedence](#discovery-precedence)). Only path that sees `include:`-expanded steps with their real names — client-side AST sees only the raw `{include: ...}` entry. |
| `tarn env --json` | Environment picker; returns every configured environment plus its `source_file`, inline vars, and resolved values (with redaction). |
| `tarn fmt` | `Tarn: Format File` command and the `DocumentFormattingEditProvider` implementation. |
| `tarn --version` | Activation-time compatibility check against `tarn.minVersion` in the extension's `package.json`. |

The `--ndjson`, `--select`, `--only-failed`, `tarn validate --format json`, `tarn env --json`, `tarn list --file`, and `tarn fmt` flags all landed across the 0.5.x series as the "T51–T58" extension-contract tickets, bundled into the coordinated 0.5.0 cut by NAZ-288. Location metadata on step and assertion results (the field `ResultMapper` prefers over AST ranges) landed as NAZ-260 / T55.

### Library surface

When Phase V moves features onto `tarn-lsp`, the LSP server consumes the `tarn` crate in-process. Both the CLI and the LSP already share these public library surfaces as of 0.6.0:

- `tarn::validation::validate_document` — document-level parse + schema validation, returns ranged diagnostics.
- `tarn::outline::outline_document`, `find_capture_declarations`, `find_scalar_at_position`, `CaptureScope`, `PathSegment`, `ScalarAtPosition`.
- `tarn::env::resolve_env_with_sources`, `EnvEntry.declaration_range`, `inline_env_locations_from_source`, `scan_top_level_key_locations`.
- `tarn::selector::format_*` — the canonical selector composer for `FILE::TEST::STEP_INDEX`. Used by `tarn run --select`, `tarn-lsp` code lens (`tarn.runTest` / `tarn.runStep`), and any extension code that needs to round-trip a selector.
- `tarn::format::format_document` — the formatter shared by `tarn fmt` and `tarn-lsp`.
- `tarn::fix_plan::generate_fix_plan` — shared fix-plan engine; the MCP tool `tarn_fix_plan` and the LSP quick-fix provider both call this.
- `tarn::jsonpath::evaluate_path` — single wrapper around `serde_json_path` that everything Tarn-side uses to evaluate JSONPath expressions.

The extension relies on these indirectly today (via the CLI) and directly once a feature migrates to the LSP path in Phase V2.

### Schemas

Two schema files in [`schemas/v1/`](../schemas/v1/) are the structural contract:

- [`schemas/v1/testfile.json`](../schemas/v1/testfile.json) — `.tarn.yaml` test files. Wired through `redhat.vscode-yaml` via the `contributes.yamlValidation` block in `editors/vscode/package.json`, so every Tarn file in the editor gets YAML schema validation with zero extra configuration.
- [`schemas/v1/report.json`](../schemas/v1/report.json) — the JSON report emitted by `tarn run --format json` and the per-file summary included in `tarn run --ndjson` events. `ResultMapper` parses report payloads against this schema and uses `zod` (or equivalent) runtime guards so a drift between extension and CLI is caught at parse time instead of flowing into a mis-mapped `TestMessage`.

The `tarn-lsp` crate carries its own copy of `testfile.json` at `tarn-lsp/schemas/v1/testfile.json` (NAZ-314 resolution) so `cargo publish -p tarn-lsp` can read the schema from inside the crate directory; a sync-verification test enforces parity with the workspace copy.

### Streaming contract (`--ndjson`)

`tarn run --ndjson` is the primary runtime path. Event shape:

```jsonl
{"event":"file_started","file":"...","timestamp":"..."}
{"event":"step_finished","file":"...","test":"...","step":"...","phase":"setup|test|teardown","status":"PASSED","duration_ms":12}
{"event":"test_finished","file":"...","test":"...","status":"FAILED"}
{"event":"file_finished","file":"...","summary":{...}}
{"event":"done","summary":{...}}
```

Failing `step_finished` events carry `failure_category`, `error_code`, and `assertion_failures`. Parallel-mode runs buffer each file and emit all its events atomically on `file_finished`, so the client never sees interleaved events from concurrent files. When stdout is bound to a structured format (`json`, `junit`, `tap`), streaming progress is routed to stderr so stdout stays parseable.

The extension's `RunHandler` pipes the child process's stdout into an NDJSON parser and forwards each event straight to the Test Controller. A run that dies mid-stream still surfaces its completed `step_finished` events as partial results, with the remainder marked errored; this is how cancellation and crash-mid-run are rendered cleanly.

### Selector format

The extension composes selectors via the shared `tarn::selector` module so the VS Code extension, `tarn-lsp` code lens, and the CLI parser all produce identical strings. The format is `FILE::TEST[::STEP_INDEX]`, with `STEP_INDEX` a zero-based integer and all three components joined by `::`. `TEST` is the test `name:` as written in the YAML; `STEP_INDEX` is deliberately positional rather than named so rename-in-place doesn't silently invalidate a pinned selector.

### Discovery precedence

`WorkspaceIndex` ships a four-level discovery strategy:

1. **Startup** — `WorkspaceIndex.initialize()` globs `**/*.tarn.yaml` and parses every match with the client-side YAML AST. Scoped `tarn list` is deliberately not used here because activation latency dominates on workspaces with dozens of test files.
2. **Incremental refresh** — on `onDidChange` / `onDidCreate`, `refreshSingleFile(uri)` calls `tarn list --file <path> --format json`. If the outcome is `{ok: true, file}`, the extension merges Tarn's authoritative tests/steps with the AST's ranges and notifies the TestController only when the structure actually changed (`rangesStructurallyEqual`).
3. **Per-file fallback** — if Tarn returns `{ok: false, reason: "file_error"}` (the YAML parses in the editor but Tarn rejects it at load time), the refresh path falls back to the AST for that one file and leaves scoped discovery enabled for the rest of the session.
4. **Session-wide fallback** — if Tarn returns `{ok: false, reason: "unsupported"}` (missing binary, spawn error, watchdog, older Tarn without `--file`), the extension flips a session-local capability flag and stays on the AST path until the next explicit refresh.

Because Tarn resolves `include:` directives at parse time, the scoped path is the only way Test Explorer sees `include:`-expanded steps with their real names.

## Mapping results to editor ranges

Tarn's JSON report carries an optional `location: {file, line, column}` on every `StepResult`, `AssertionDetail`, and `AssertionFailure`. Fields are 1-based to match the human / error output Tarn already prints, and they are captured inside the parser before any HTTP work runs — so the location is pinned to the exact bytes Tarn saw, not to whatever the editor holds when the report arrives.

`ResultMapper.buildFailureMessages` resolves the source anchor for each failure in this order:

1. **`failure.location`** (per-assertion) — preferred. Lands on the exact operator node (`status:`, `body $.path:`, `headers:`, etc.) the user authored.
2. **`step.location`** (step-level) — fallback for assertion failures that lack their own location, and anchor for non-assertion failures (connection errors, capture failures).
3. **AST range** — fallback for reports that omit `location` entirely. Covers pre-T55 Tarn versions and `include:`-expanded steps where Tarn emits `location: None` because the step was synthesized from an include directive.

The 1-based line and column are decremented by one before they become a `vscode.Position`. A Tarn location is a single point; the mapper builds a zero-width `vscode.Range` at that point and lets VS Code expand it to the enclosing token for rendering.

### Drift-free by construction

The AST layer is rebuilt every time the file changes on disk, so it reflects the *current* file, not the one Tarn executed. If the user edits the file between a run starting and its report rendering, or if parallel tests keep auto-formatting the buffer, the AST range can drift. The JSON-reported `location` is pinned to the exact file the CLI saw and survives every subsequent edit. Integration tests in `resultMapperLocation.test.ts` enforce this by inserting blank lines at the top of the fixture between run start and report parse, then asserting the diagnostic still lands on the original assertion node.

The AST path is never removed — it is the source of truth for authoring features (CodeLens, document symbols, hover, completion, rename) and the fallback for reports that don't carry `location`. Both paths coexist permanently.

## Phase V — LSP migration plan

This is the active roadmap. The goal is to move the extension's in-process TypeScript language-feature providers onto a thin [`vscode-languageclient`](https://github.com/microsoft/vscode-languageserver-node) front-end that talks to the Rust [`tarn-lsp`](TARN_LSP.md) crate over stdio, so there is **one implementation of every language feature** shared across the VS Code extension, Claude Code, Neovim, Helix, Zed, and any other LSP 3.17 client.

The full migration decision — strategy tradeoffs, per-feature ordering, rollback plan, and version-bump policy — is in [`editors/vscode/docs/LSP_MIGRATION.md`](../editors/vscode/docs/LSP_MIGRATION.md). This section is the high-level summary.

**Strategy: dual-host.** Both stacks run side by side behind the `tarn.experimentalLspClient` flag (window-scoped, default `false`). Each Phase V2 ticket migrates exactly one feature at a time; the direct provider for that feature is deleted in the same ticket. Phase V3 deletes the flag and the last of the direct providers once every feature has soaked.

**Phase sequence.**

| Phase | Ticket | Outcome |
|---|---|---|
| V1 | NAZ-309 | Scaffolding: `vscode-languageclient@9.0.1` runtime dep, `src/lsp/client.ts`, `src/lsp/tarnLspResolver.ts`, `tarn.experimentalLspClient` + `tarn.lspBinaryPath` settings, dynamic-import so the protocol stack only loads when the flag is on. Shipped in **0.5.1**, re-tagged as part of **0.6.0**. |
| V2.1 | TBD | Migrate diagnostics |
| V2.2 | TBD | Migrate document symbols |
| V2.3 | TBD | Migrate code lens |
| V2.4 | TBD | Migrate hover |
| V2.5 | TBD | Migrate completion |
| V2.6 | TBD | Migrate formatting |
| V2.7 | TBD | Migrate definition / references / rename |
| V2.8 | TBD | Migrate code actions |
| V2.9 | TBD | Bridge `tarn.evaluateJsonpath` executeCommand |
| V3 | TBD | Delete direct providers + experimental flag |

Epic: **NAZ-308**. V1 shipped as **NAZ-309**.

**Current status (0.6.1).** The scaffold is live. `tarn.experimentalLspClient = true` spawns `tarn-lsp` side by side with the direct providers; failures to resolve or start the binary are advisory, not fatal. **No feature has moved to the LSP path yet.** Phase V2 tickets will start migrating features one at a time once the scaffold has soaked across a release.

**Why dual-host and not a single rip-and-replace.** A full migration in one ticket moves every language feature at once, so any LSP regression (say, hover formatting differing from the in-process provider) wedges every other feature until it is fixed, and the rollback scope is an entire PR of integration-test diffs. Dual-host keeps each feature ticket small, bounds the blast radius to one provider at a time, and lets us dog-food the LSP path per feature before committing. Selective migration (some features LSP, some direct, forever) is explicitly rejected — maintaining two codebases for the same language surface is the worst possible steady state, and it undermines the reason `tarn-lsp` exists (one canonical language surface for Neovim / Helix / Claude Code / etc.).

## Public API

The extension exposes a structured object to other extensions via `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode').exports`. That object conforms to `TarnExtensionApi`, defined in [`editors/vscode/src/api.ts`](../editors/vscode/src/api.ts). `api.ts` is the single source of truth — `extension.ts` re-exports the type but does not redeclare its shape.

```ts
import type { TarnExtensionApi } from "nazarkalytiuk.tarn-vscode";
import * as vscode from "vscode";

const ext = vscode.extensions.getExtension<TarnExtensionApi>(
  "nazarkalytiuk.tarn-vscode",
);
if (!ext) return;              // extension not installed
const api = await ext.activate();
if (!api) return;              // untrusted workspace — no API exposed
```

`activate()` returns `undefined` in untrusted workspaces. Downstream integrators must handle that branch: the extension deliberately does not spawn Tarn, index files, or expose any surface until the user grants trust.

The full field-by-field reference, stability tiers, semver policy, `1.0.0` gate, and the golden-snapshot test that enforces API drift are documented in [`editors/vscode/docs/API.md`](../editors/vscode/docs/API.md). Summary:

- `testControllerId`, `indexedFileCount`, `commands` — **stable**. Breaking changes require a major version bump.
- `testing` — **internal**. Opaque sub-object for the extension's own integration tests. No compatibility guarantees whatsoever, including across patch releases. Do not use from production code.

The stable surface is CI-enforced via `editors/vscode/tests/unit/apiSurface.test.ts`, which compares a normalized `api.ts` against `editors/vscode/tests/golden/api.snapshot.txt`. Any edit to the interface — adding, removing, renaming a field, changing a `@stability` annotation, changing the semver policy prose — fails the test unless the golden is updated in the same commit. Three extra invariants: every `readonly` field must carry a `@stability` annotation, the file-level block comment must mention every stability tier, and the `testing` sub-object must be annotated `@stability internal`.

The `0.x` → `1.0.0` cut is tracked as part of NAZ-288's version alignment work. Until `1.0.0`, the stable surface is still subject to one last round of pruning; integrators should pin to a minor range rather than a caret range.

## Release and version alignment

**NAZ-288 policy.** From 0.5.0 onward, `tarn`, `tarn-mcp`, `tarn-lsp`, and the VS Code extension all share `major.minor`. Patch numbers may diverge for a hotfix on one side only, but a new minor always bumps every side in lockstep.

**Coordinated release.** A single git tag `v<version>` triggers two workflows:

- [`.github/workflows/release.yml`](../.github/workflows/release.yml) — `tarn`, `tarn-mcp`, `tarn-lsp` binaries, crates.io publish, Homebrew formula, Docker image. Runs on `push: tags: ["v*"]`.
- [`.github/workflows/vscode-extension-release.yml`](../.github/workflows/vscode-extension-release.yml) — VSIX build, VS Code Marketplace and Open VSX publish. Same tag trigger; waits for the CLI release to land on GitHub so the ordering is CLI → extension.

**Enforcement.** Three layers:

1. `editors/vscode/tests/unit/version.test.ts` cross-reads `editors/vscode/package.json` and `tarn/Cargo.toml` on every `npm run test:unit` pass. The lint compares `major.minor` (not the full semver triple) so a patch-only release on one side doesn't wedge the other.
2. `editors/vscode/package.json` declares `tarn.minVersion` alongside `version`. `src/version.ts` runs `tarn --version` at activation, parses the semver, and warns non-fatally if the installed CLI is below the minimum.
3. `vscode-extension-release.yml` hard-fails a publish when the git tag does not match `package.json` version.

**Per-release notes.** [`CHANGELOG.md`](../CHANGELOG.md) at the repo root carries the Tarn-side notes; [`editors/vscode/CHANGELOG.md`](../editors/vscode/CHANGELOG.md) carries the extension notes. Both cross-link so a reader of the Marketplace listing can follow the full release story without jumping repos.

## References

- [`editors/vscode/README.md`](../editors/vscode/README.md) — user manual, screenshots, settings, remote setups.
- [`editors/vscode/CHANGELOG.md`](../editors/vscode/CHANGELOG.md) — per-release notes `0.1.0` → `0.6.1`.
- [`editors/vscode/docs/API.md`](../editors/vscode/docs/API.md) — `TarnExtensionApi` quick reference.
- [`editors/vscode/docs/LSP_MIGRATION.md`](../editors/vscode/docs/LSP_MIGRATION.md) — Phase V migration decision.
- [`docs/TARN_LSP.md`](TARN_LSP.md) — `tarn-lsp` language server spec.
- [`docs/VSCODE_REMOTE.md`](VSCODE_REMOTE.md) — Remote Development audit.
- [`schemas/v1/testfile.json`](../schemas/v1/testfile.json) — test file schema.
- [`schemas/v1/report.json`](../schemas/v1/report.json) — JSON report schema.
- [`CHANGELOG.md`](../CHANGELOG.md) — Tarn root changelog.
