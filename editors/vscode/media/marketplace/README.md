# Tarn VS Code — Marketplace assets (capture plan)

This folder holds the artwork referenced by the Marketplace listing
(`galleryBanner`, screenshots inlined in `editors/vscode/README.md`, and the
diagnosis-loop demo GIF). The binary files currently checked in are **1×1
placeholder images** generated from Node so that:

- The VSIX packaging pipeline can already reference them.
- The README renders without broken-image icons.
- `.vscodeignore` and `gallery` paths are verified end-to-end before a human
  ever opens a screen recorder.

A human operator (the project maintainer) is expected to replace each file
listed below with a real capture. This file is the spec for that capture pass.
Do **not** rename files — the README and `package.json` both reference these
exact paths.

## Brand colour

- `galleryBanner.color`: `#1E1B4B` (deep indigo).
- `galleryBanner.theme`: `dark`.
- Rationale: the Tarn activity-bar icon (`media/tarn-icon.svg`) is monochrome
  and inherits `currentColor`, so there is no pre-existing brand palette in
  the repo to pull from. Deep indigo reads cleanly against VS Code's dark
  chrome, gives white foreground text WCAG AA contrast, and avoids colliding
  with the red/green/yellow that VS Code reserves for test-status UI.
- If the project ever ships a proper brand palette, update **both** this
  file and `galleryBanner.color` in `editors/vscode/package.json` in the
  same commit.

## Required captures

All screenshots target **1280×800** (VS Code's recommended Marketplace
screenshot size — anything up to 1920×1080 is accepted, but 1280×800 keeps
the VSIX small and renders crisply in the Marketplace screenshot carousel).
PNG, sRGB, no transparency. File names are **load-bearing** — the README
inlines them by exact path.

### 1. `banner.png`

- **Target size**: 1376×400 (VS Code Marketplace gallery banner size).
- **Scene**: Tarn wordmark on a deep-indigo (`#1E1B4B`) background with a
  one-line tagline underneath: *"Run, debug, and iterate on API tests
  without leaving the editor."*
- **Use**: Marketplace gallery header. Referenced by `galleryBanner.color`
  indirectly — the banner itself is rendered from this PNG in the README
  and acts as the "hero" image at the top of the Marketplace listing.

### 2. `screenshot-test-explorer.png`

- **Target size**: 1280×800.
- **Scene**: VS Code window, Test Explorer panel open on the left, showing
  the hierarchical tree: 1 workspace → 3 `.tarn.yaml` files → each file
  expanded to its tests → one test expanded to its steps. At least one
  passed test (green check), one failed test (red X), and one not-yet-run
  test (grey circle) should be visible so all three states are in-shot.
- **Fixture**: use `editors/vscode/tests/integration/fixtures/workspace/tests/`
  so the tree is populated with real discovered files.
- **Hero element**: the `Run` button in the Test Explorer title bar.

### 3. `screenshot-streaming.png`

- **Scene**: A test run in progress. Test Explorer on the left showing a
  spinning "running" indicator on a step, editor on the right with the
  `.tarn.yaml` file open, and the Tarn output channel visible at the
  bottom streaming structured JSON events. Capture the moment a step has
  just transitioned from "running" to "passed" so the viewer sees both
  the event log and the live UI update.
- **Fixture**: any multi-step test against a responsive endpoint
  (`demo-server` on localhost is ideal).
- **Hero element**: the in-flight progress indicator in the Test tree.

### 4. `screenshot-diff.png`

- **Scene**: A failing assertion surfaced via VS Code's `TestMessage` UI:
  expected-vs-actual peek view anchored to the asserting line in the
  `.tarn.yaml` file, showing a unified diff (green `+`, red `-`) of a
  JSON body assertion. The failure category and error code should be
  visible in the hover.
- **Fixture**: craft a test that asserts `body.user.name == "Alice"`
  against a response that returns `"Alicia"` — small diff, big impact.
- **Hero element**: the inline diff.

### 5. `screenshot-env-picker.png`

- **Scene**: The `Tarn: Select Environment…` command palette quick-pick
  open, listing discovered environments (`dev`, `staging`, `local`,
  `production`, `<none>`) with the current selection highlighted. The
  status-bar entry showing the active environment should also be visible
  at the bottom of the window.
- **Fixture**: a workspace with `tarn.env.yaml`, `tarn.env.staging.yaml`,
  `tarn.env.local.yaml` so the picker has >1 real choice.
- **Hero element**: the environment quick-pick list.

### 6. `screenshot-codelens.png`

- **Scene**: A `.tarn.yaml` file open in the editor with CodeLens actions
  rendered above at least two `tests:` entries and one `steps:` entry:
  `Run | Dry Run | Run step`. The file should be zoomed enough that the
  CodeLens labels are legible (VS Code default zoom + font size 14).
- **Fixture**: any real test file; a 3-test / 6-step fixture is ideal
  because it demonstrates that CodeLens scales to multiple targets.
- **Hero element**: the CodeLens row directly above a `name:` key.

### 7. `demo.gif`

- **Target length**: 30 seconds, ≤8 MB (Marketplace caps animated images
  at ~10 MB; keep headroom). 15 fps is plenty for an editor recording.
- **Target size**: 1280×720 or 1024×640. Downscale aggressively if the
  file blows past 8 MB — the GIF is in the VSIX and counts against the
  published extension size.
- **Scene (the "diagnosis loop")**: the single most important animated
  asset in the listing. Show the full tight loop a first-time user will
  recognise within 5 seconds:
  1. (0–4 s) User clicks **Run All** from the Test Explorer. Progress
     indicators spin, output pane streams events.
  2. (4–9 s) A step fails. Red X appears in the tree, a failure toast
     slides in, the `TestMessage` peek opens inline.
  3. (9–14 s) User clicks **"Jump to failure"** in the peek. Cursor
     lands on the exact asserting line in the `.tarn.yaml`.
  4. (14–21 s) User edits the YAML (e.g., fixes an expected status code
     or an interpolation).
  5. (21–26 s) User clicks **Rerun** (`Tarn: Rerun Last Run` from the
     status bar).
  6. (26–30 s) All tests pass. Tree turns green, status-bar entry shows
     a green check, and the final frame holds on "all passing" for 1 s
     so the GIF loop reads cleanly.
- **Tool suggestion**: record with macOS QuickTime or Kap, then convert
  to GIF with `gifski --fps 15 --width 1280 -o demo.gif capture.mov`.
- **Hero frame**: the green "all pass" state — this is the frame that
  will be shown when the GIF is paused in the Marketplace listing.

## Packaging verification

After replacing the placeholders with real captures, confirm **every** path
below ships in the VSIX:

```bash
cd editors/vscode
npx @vscode/vsce ls | grep '^media/marketplace/'
```

Expected output:

```
media/marketplace/README.md
media/marketplace/banner.png
media/marketplace/demo.gif
media/marketplace/screenshot-codelens.png
media/marketplace/screenshot-diff.png
media/marketplace/screenshot-env-picker.png
media/marketplace/screenshot-streaming.png
media/marketplace/screenshot-test-explorer.png
```

If any file is missing from that list, check `editors/vscode/.vscodeignore`
— the `!media/**` rule must remain after the `node_modules/**` exclude.

## Why placeholders ship

Cutting the pipeline and the capture plan in one commit lets us:

1. Bump the extension version and wire up `galleryBanner` in `package.json`
   without waiting on a separate art pass.
2. Verify end-to-end packaging now, so the human who replaces the PNGs
   only has to drag-and-drop the real files into this directory and rerun
   `npx @vscode/vsce package`.
3. Keep the Marketplace README honest: every image it references already
   exists on disk, so the doc renders locally during development even
   though the content is stub art.

When each file below is replaced with a real capture, delete the
corresponding "Required captures" entry above and move it into a short
`## Shipped` section so future contributors can see the live state of
the asset pack at a glance.
