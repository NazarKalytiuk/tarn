# ADR 0002 — Tarn Studio: UI design language

- **Status:** Accepted
- **Date:** 2026-05-21
- **Authors:** Nazar Kalytiuk (@nazarkalytiuk)
- **Builds on:** [ADR 0001 — Tarn Studio: native desktop app on Tauri](./0001-tauri-desktop.md)

## Context

ADR 0001 fixed the stack (Tauri 2 + Solid + Tailwind v4 + CodeMirror 6) and the IPC contract, but left the actual user-facing design open. The Phase 1 scaffold was a generic two-column layout with a single-scroll detail panel, written to validate the pipes — not to express a product opinion.

This ADR captures the design opinions decided after a focused discussion that explicitly weighed:

- **The author's own use case** (run + watch, occasionally monitor multi-file runs).
- **The target audience** (QA without YAML fluency, devs working with AI-generated tests, DevRel using Studio for screenshots and demos).
- **Competitive positioning** vs Postman/Insomnia (loud, feature-dense) and vs the terminal (textual, no visual hierarchy).

The tension between "power-user respect" (author) and "discoverability for non-engineers" (QA/DevRel) is resolved by choosing **Raycast/Linear-style minimalism plus discoverable detail behind explicit clicks**, rather than Postman-style information density. Quiet UI + everything-one-click-away beats a busy screen for both audiences.

## Decisions

### 1. Visual identity

- **Dark theme only.** Single mode, brand-consistent (à la Vercel/Linear/Raycast). Light mode is not a Phase 1 cost we accept.
- **Aesthetic reference:** Raycast and Linear — opinionated polish, low information density, generous whitespace, restrained color, system-native typography (`ui-sans-serif`), monospace for payloads (`ui-monospace`).
- **Color usage:** reserved for state. Status colors (`--color-status-pass`, `--color-status-fail`, `--color-status-running`, `--color-status-pending`, `--color-status-skipped`) defined as theme tokens in `styles.css`. Brand accent (`--color-brand-*`) is used only for primary CTAs and active selection — never as decoration.
- **No emojis in the chrome.** Icons are explicit shapes (dots, Lucide glyphs in toolbar), not Unicode emoji.

### 2. Window layout

A single window, two columns, no tabs across the top.

```
┌─ header ────────────────────────────────────────────────────────────┐
│ logo · /project-path                                                │
├─ toolbar ───────────────────────────────────────────────────────────┤
│ [▶ Run all]/[⏹]   env ▾   tags: [a×][b×]+   history ▾               │
├─ sidebar ────────────────┬─ detail ───────────────────────────────┤
│ ▾ smoke.tarn.yaml   3/3  │  step header (name · method · status)   │
│   • Health check    12ms │  ─────────────────────────────────────  │
│   • Login           48ms │  failure summary (one line, if failed)  │
│ ▾ crud.tarn.yaml   ⚡1/3  │  ─────────────────────────────────────  │
│   • Create user     91ms │  [Assertions] [Request] [Response] [Curl]│
│   ⚡ Update user      …   │  ─────────────────────────────────────  │
│                          │  selected tab content                    │
├─ footer ─────────────────┴──────────────────────────────────────────┤
│ ✓ N  ✗ N  ◷ Ns                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

**Sidebar width:** 320 px default, resizable, persisted per project. No collapse-to-iconbar mode — if the user wants more room for detail, they shrink the sidebar.

**No right-side panel, no bottom panel, no tab strip.** Everything that isn't a step lives in the toolbar or footer.

### 3. Toolbar

Left to right:

1. **Run all / Cancel.** Single primary action. While a run is in flight, the button becomes Cancel and the original Run all label is suppressed (no shape shifting; the same slot toggles label and color).
2. **Env picker.** Dropdown listing the project's `tarn.env*.yaml` files. Default is the implicit base env (`tarn.env.yaml`). Selection becomes `tarn run --env <name>`.
3. **Tag chips.** Tags discovered from `tarn list --format json` (`files[].tags[]`). Clicking a chip adds it to the active filter (visible as `[name ×]`). Multiple chips AND together — same semantics as `tarn run --tag a --tag b` (comma-separated).
4. **Run history dropdown.** A dropdown labeled `now ▾` while a run is fresh, switching to a past timestamp once one is selected. Picking a past run repaints the tree with the statuses from that run (read from `.tarn/last-run.json` and per-step fixtures). Only the most recent N (Phase 1: 10) are listed; "see all" opens a sheet in Phase 3.

No Cmd+K palette in Phase 1. Search arrives only when the test count makes scrolling painful — which won't happen for typical projects.

### 4. Sidebar tree

- **Hierarchy:** file → test → step. Files always shown expanded by default; tests collapsible. Tag-filtered tree hides files with zero matching tests.
- **Status indicator:** a 6 px filled circle to the left of the row. Five states: `pending` (slate-600), `running` (amber-500 with subtle 1 Hz opacity pulse), `passed` (green-500), `failed` (red-500), `skipped` (slate-400). No icon glyphs (✓/✗) — circles only. Color carries the meaning; for accessibility, status also appears as a small text label on the right when the user navigates by keyboard (Phase 2).
- **Right-side rail per row:** duration in ms when known, faint. No glyphs, no hover-only Run buttons crowding the row (the previous scaffold had per-row Run buttons revealed on hover — removed in this design as too loud; selecting a step and pressing `R` re-runs it, or the user uses the toolbar).
- **Selection:** clicking a row selects it. Selection is visual (background) only — no auto-running, no auto-jumping.

### 5. Detail panel

The detail panel is the right column. Structure top-to-bottom:

1. **Step header** — step name, file path (small/dimmed), method + URL when known, status pill, duration.
2. **Failure summary** — single line, only present when step failed. Reads the first assertion failure's `message`. The user can dismiss/expand details via the tabs below; the summary itself never expands.
3. **Tabs** — `Assertions` (default), `Request`, `Response`, `Curl`.
   - **Assertions** is auto-active for any step, with the failure-first ordering when failed.
   - **Request**/**Response** read from the same `step_finished` event (run with `--verbose-responses` so the data is always there, see decision 7).
   - **Curl** is a generated, copy-paste-ready `curl` command, equivalent to `tarn run --format curl` for that step.

Tabs are **horizontal pills**, not bordered tabs — subtle hover/active state via background. No tab content height jitter: the tab strip is fixed-height; only the body scrolls.

**Passed step minimalism:** Assertions tab for a passed step is a single line per assertion (`✓ status == 200`) with no expanded request/response. The user must click `Request` or `Response` to see payloads. This is the "minimal default for passed" decision; the data is one click away, not behind a separate command.

### 6. Failure presentation

Three explicit rules, chosen against the more dramatic alternatives:

1. **Single-line failure summary** at the top of the detail panel — _not_ a colored box with expected/actual diff. The diff lives inside the Assertions tab, expanded by default for failed steps. Reason: a louder hero would make Studio feel like an error tracker; the design intent is quiet competence.
2. **No auto-jump** when a run finishes with failures. The footer counter updates (`✗ 2`), and the failed steps glow red in the tree, but the selected step does not change. The user navigates explicitly. Reason: the author's own workflow ("watch it run, then decide") trumps the QA-friendly auto-jump. Power-user respect won the trade-off.
3. **No toast notifications.** Run completion is silent. State is read from the tree (red dots) and footer (counter). Reason: notifications break the "quiet by default" promise; counters and color carry the same information without stealing focus.

For QA discoverability the implication is: tag-filter and tree colors must be visually unambiguous, because they are the only signal that something failed. Tested by ensuring red is distinguishable from amber for the most common color-blind types (deuteranopia/protanopia) — verified via WebAIM contrast checker once design tokens land.

### 7. Streaming verbosity and detail source

`tarn run --ndjson` carries only progress and assertion data per `step_finished` event — request/response/captures are **not** in the stream regardless of `--verbose-responses` (verified against `tarn 0.13.1`). Adding them upstream would inflate the stream for the live-progress use case where most consumers only need status.

Instead, Studio uses the two-source model the CLI already provides:

1. **Live stream (`tarn run --ndjson --verbose-responses`)** — drives the tree (status, duration) and the Assertions tab as events arrive.
2. **Always-on JSON artifact (`.tarn/last-run.json`)** — emitted at the end of every run, contains `request`, `response`, `captures`, `location` (file/line/column), and `remediation_hints` for every step. Studio reads it lazily after `test:run-done` and serves Request/Response/Curl tabs from there.

`--verbose-responses` is still passed so the JSON artifact contains full request and response bodies for **every** step, not just failed ones. Without it, only failed steps get bodies in the JSON.

During a run, the Request/Response/Curl tabs show a quiet "Waiting for run to finish" placeholder rather than a spinner. Once `run-done` fires, the backend reads `.tarn/last-run.json`, parses it once, and emits a `test:run-report-ready` event with the parsed report. The frontend stores the report and resolves tab content from it.

The trade-off is that during a running run the user can't yet inspect the request/response of an already-completed step. For Phase 1 this is acceptable — most runs finish in under 10 seconds and the deferred load is invisible. Phase 2 may pre-emptively read per-step fixtures from `.tarn/fixtures/` as they land if user feedback shows the delay is felt.

Body size is capped by the CLI default `--max-body 8192`. If a step's body in the report shows `…<truncated: N bytes>`, the Request/Response tab surfaces a "Re-run with full body" action (Phase 2) that re-runs just that step with `--max-body 0`.

### 8. Empty state

Centered card, vertical:

```
                          Tarn Studio
                       ─────────────────
                Quiet, fast API testing.

                  ┌─────────────────────┐
                  │  Open project…   ⌘O │
                  └─────────────────────┘

              Recent: ~/work/my-api  ·  ~/play/sandbox
```

- **Recent projects** — populated from `tauri-plugin-store` (`~/Library/Application Support/Tarn Studio/store.json` on macOS). Click on a recent entry → opens the project (same path as `Open project…`).
- **No quick-start cards.** No "import from OpenAPI", no "import from curl", no examples carousel. Decided against the third option in the design discussion — it would clutter the first impression. The same actions live inside the project menu once a project is open (Phase 3).
- **No tutorial / docs link.** A small `?` lives in the corner (Phase 3) linking to the website; Phase 1 ships without it.

### 9. Animations and motion

- **Running pulse:** 1 Hz opacity oscillation on the amber dot (`0.55 → 1.0 → 0.55`). Used only for running, never for pending. Implemented in CSS, not JS.
- **State transitions:** dots animate via 150 ms color transitions (pending → running → passed/failed), not via icon swaps.
- **Tab switching:** no animation. Instant.
- **Tree expand/collapse:** 100 ms height transition.
- **Dialog/dropdown:** native Tauri/system; no custom transitions.

The implicit budget: any animation that doesn't actively communicate state change is removed.

### 10. Keyboard

Phase 1 ships these shortcuts. They are documented in a Help sheet (Phase 3) but discoverable in tooltips.

| Action | Shortcut |
|---|---|
| Open project | ⌘O |
| Run all | ⌘R |
| Cancel run | ⌘. |
| Re-run selected step | ⌘⇧R |
| Focus sidebar | ⌘1 |
| Focus detail | ⌘2 |
| Switch tab → | ⌘] |
| Switch tab ← | ⌘[ |
| Toggle current file | ←/→ on file row |

No Cmd+K, no Cmd+P, no quick-switcher. Adding one is justified once test count exceeds ~50 per project on average; that is not the Phase 1 bar.

## Out of scope

The following are intentionally **not** part of the Phase 1 UI and not addressed by this ADR:

- Inline YAML editor (Phase 2 — separate ADR will cover CodeMirror + LSP integration).
- Latency graphs and per-step trend charts (Phase 3).
- Curl / OpenAPI / Hurl import wizards in-app (Phase 3).
- Multi-window (Phase 4+).
- Light theme.
- Mobile/tablet layouts. Studio is desktop-only.

## Consequences

### Positive

- Coherent product opinion: every Phase 1 decision points the same direction (quiet, polished, minimal). Future contributors have a clear rubric for "does this fit Tarn Studio."
- Cheaper to ship: no Cmd+K palette, no light theme, no auto-jump heuristics, no toast system. Each of those would have been a multi-day investment.
- Distinguishes Tarn from Postman/Insomnia, which are saturated and feature-dense. There is room for a quiet-and-fast brand in the API-testing space.

### Negative

- First-time QA users may need a small onboarding nudge — the design assumes they will explore by clicking, not be guided. Mitigation: empty-state copy and tooltips are written for non-engineers (Phase 1 copy review before release).
- DevRel screenshots of a "quiet" UI are less viral than screenshots of dramatic failure dashboards. The marketing implication is to lean on _animated_ demos (GIFs of a run streaming in) rather than static stills, which favor density.
- The `--verbose-responses` default increases NDJSON volume. If real users hit performance issues, falling back to fixture-on-click is the planned escape hatch.

### Risks

| Risk | Mitigation |
|---|---|
| "Quiet" UI mistaken for "underbaked" | Polish the small details (typography, spacing, motion) hard. The product feels intentional, not unfinished. |
| Color-only status fails accessibility | Audit with WebAIM contrast + simulate protanopia/deuteranopia before release. Add status text labels in Phase 2 keyboard nav. |
| Truncated bodies confuse users | Always show the truncation marker. Phase 2 adds a one-click "re-run with full body". |

## How this overrides ADR 0001

ADR 0001 left UI undecided. This ADR is purely additive — no architectural decisions in ADR 0001 are reversed.
