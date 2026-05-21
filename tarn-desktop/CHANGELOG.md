# Tarn Studio — changelog

Tarn Studio is a native desktop app for running and investigating Tarn API tests. It versions independently of the [Tarn CLI](../CHANGELOG.md) and uses its own git tag namespace `studio-vX.Y.Z`.

## Unreleased

## 0.1.0 — First public scaffold

The first release of Tarn Studio. Ships the foundation that turns the Tarn CLI into a calm, dark, native API-testing surface: discover tests, run them, watch their status stream live, drill into failures.

### Design language

Codified across two ADRs in `docs/adr/`:

- **ADR 0001** — Tauri 2 shell, Solid frontend, Tailwind v4 styling, CodeMirror 6 (reserved for Phase 2's YAML editor), sidecar packaging of the `tarn` CLI, NDJSON-streamed IPC.
- **ADR 0002** — visual language: dark-only, restrained colour (status colours are the only saturated hues), shape-distinct status indicators so colour is never the only signal, no toast notifications, no progress bars (a per-step completion strip replaces them).

The brief that guided the design lives at `docs/design/tarn-studio-design-brief.md`.

### Shell and chrome

- **Custom titlebar** with traffic-light placeholders, project path crumb, env picker, run-history affordance, and a settings dropdown.
- **Toolbar** with tag chips, Run all / Cancel, plus two bonus controls that the CLI cannot offer: a `Show only N failures` focus toggle and a `Re-run failures` button that re-runs just the previously-red selectors.
- **Statusbar** with at-a-glance counters per status, elapsed/duration, and the active env.

### Catalogue tree

- File → test → step hierarchy, indented and connected by a single column of status dots.
- File rows show a `passed/total` count; test rows the same scoped to their steps.
- Step rows render the HTTP method as a coloured monospace tag (GET/POST/PATCH/PUT/DELETE), the path in mono, and the duration or `RUNNING` marker on the right.
- **Hover Run button** on every row: file, test, or step. Subtle, only visible on the row the cursor is over. Stops propagation so it never fights with row selection.
- Tag filter narrows the visible tree to files carrying every selected tag (AND semantics, matching `tarn run --tag a --tag b`).

### Run dashboard

When no step is selected, the right pane shows a calm dashboard:

- Hero counters per status (36 px monospace), with `running` quietly pulsing.
- A **per-step completion strip** — one coloured block per real step, in run order. Conveys "where we are" without lying about total duration the way a progress bar would.
- A `Currently running` card with the step's method, path, and "waiting on response…" indicator.
- A `Failures` list pinned beneath, each row clickable to jump into that step's detail.
- An `All N steps passed` satisfaction card on green runs.
- A cancelled-run card that explains exactly which steps kept their status and which returned to pending.
- Idle fact tiles (files / tests / steps / tags / target env / last run) when no run has been started yet.

### Step detail

Two surfaces sharing one header (status dot, label, duration, location, `Open YAML` and `Re-run` actions):

- **Passed step.** Quiet success card listing every assertion that passed. Request and response are reachable but collapsed by default — minimal noise.
- **Failed step.** The marquee surface, optimised for Sasha and Mira:
  - An `Assertion failed` card with the message, side-by-side `Expected` / `Actual` blocks, and a list of the other assertions that passed in the same step.
  - A `Request` section with method + URL header and a headers table; copy-as-curl action.
  - A `Response` section with the status pill, headers, and a syntax-coloured JSON body (truncated past 8 KB).
  - A `Hints` block at the bottom carrying `tarn fix-plan`'s remediation suggestions.

### Empty / error states

- **First-launch empty state** with a monogram logo, `Open a project` primary + `New from template` secondary, and an unobtrusive `Recent` list once the user has opened anything.
- **`tarn` binary not found** state with a `brew install tarn-tools/tap/tarn` code block and an explicit `Locate binary…` action.

### Backend (Rust / Tauri)

- `discover_tests`, `run_tests`, `cancel_run`, `get_run_report`, `list_environments` Tauri commands.
- NDJSON streaming pipeline: spawns `tarn run --ndjson --verbose-responses`, parses each line into a typed `TarnEvent`, emits to the frontend as `test:file-started` / `test:step-finished` / `test:test-finished` / `test:file-finished` / `test:run-done` events tagged with `run_id`.
- After the CLI exits, the backend reads `.tarn/last-run.json` and emits a single `test:run-report-ready` event carrying the full report, so the detail panel can show real request/response bodies.
- Binary resolution looks at `$TARN_BIN`, then `PATH`, then the workspace `target/{release,debug}` — bundled-sidecar resolution comes later (Phase 4).

### Scope-aware run reset

A subtle but important behaviour: when running a subset (a single file, test, step, or the `Re-run failures` shortcut), only the steps in that subset are reset to `pending`. The green dots of every other step from the previous run are preserved. Running `Run all` is the only path that resets everything — each full run remains a clean snapshot.

### Example project

`examples/studio-demo/` ships alongside this release: 5 `.tarn.yaml` files (smoke, auth, user CRUD, posts, intentional failures) wired to `jsonplaceholder.typicode.com` and `httpbin.org` so the UI can be exercised end-to-end without a local server. 16 passing / 5 failing / 21 total across 13 tests.

### Known gaps (deferred)

- Run history viewer (Phase 3) — the affordance is in the titlebar but the dropdown is a placeholder.
- Inline YAML editor with `tarn-lsp` (Phase 2).
- Binary signing + auto-updater + DMG / MSI / AppImage release artifacts (Phase 4).
- Light theme — explicitly not planned.
