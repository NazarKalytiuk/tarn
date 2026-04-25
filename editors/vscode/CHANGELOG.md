# Changelog

## 0.11.2 — Coordinated release with Tarn 0.11.2

Version-only bump to track Tarn 0.11.2. Tarn 0.11.0's publish
pipeline failed before the VS Code extension reached the
Marketplace, so 0.11.2 is the first 0.11.x extension build that
actually ships. (0.11.1 was tagged but produced no artifacts —
the release.yml fix had a YAML indentation bug that made the
workflow file fail to parse, so the tag never resulted in any
publish.)

No extension code changes vs. 0.11.0.

`tarn.minVersion` moves to `0.11.2` so the activation-time
compatibility check matches the CLI version that exists on
crates.io and in the GitHub release.

## 0.11.0 — Coordinated release with Tarn 0.11.0

Version-only bump to track Tarn 0.11.0. `tarn.minVersion` moves to
`0.11.0` because the LSP bridge and `failure_category` parsing now
rely on the expanded failure taxonomy (`response_shape_mismatch`,
`skipped_due_to_fail_fast`, `skipped_by_condition`) shipped in the
0.11.0 CLI. The 0.11.0 extension will surface an activation-time
compatibility warning against an older CLI.

No extension code changes in this release.

## 0.10.0 — Coordinated release with Tarn 0.10.0 (Agent Loop)

Version-only bump to track Tarn 0.10.0, which introduces the Agent
Loop toolchain: immutable per-run artifacts under `.tarn/runs/<run_id>/`,
`summary.json` / `failures.json` / `events.jsonl`, plus the new
subcommands `tarn failures`, `tarn rerun`, `tarn inspect`, `tarn diff`,
`tarn report`, `tarn lint`, `tarn impact`, `tarn scaffold`,
`tarn pack-context`, and `tarn run --agent`. `tarn.minVersion` moves
to `0.10.0` because the CLI now persists archives that the LSP's
"rerun last failures" flow relies on; running the 0.10.0 extension
against a 0.9.0 CLI would surface an activation-time compatibility
warning.

No extension code changes in this release — behavior tracks the
CLI's new artifacts through the existing LSP bridge.

## 0.9.0 — Coordinated release with Tarn 0.9.0 (UUID v4/v7 + faker built-ins)

- Hover and completion surface the new runtime built-ins added in
  Tarn 0.9.0: `$uuid_v4`, `$uuid_v7`, plus the EN-locale faker corpus
  (`$email`, `$first_name`, `$last_name`, `$name`, `$username`,
  `$phone`, `$word`, `$words(n)`, `$sentence`, `$slug`, `$alpha(n)`,
  `$alnum(n)`, `$choice(a, b, …)`, `$bool`, `$ipv4`, `$ipv6`).
  Parameterized forms are emitted as snippets with placeholders.
- `$uuid` is now documented as an alias for `$uuid_v4`.
- Hover hint inside empty `{{ }}` lists the expanded built-in surface.
- Tracks Tarn 0.9.0 minimum version.

## 0.6.1 — Coordinated release with Tarn 0.6.1 (Dockerfile hotfix)

Version-only bump to stay in lockstep with the Tarn 0.6.1 hotfix
release, which ships a Dockerfile fix so the `ghcr.io/nazarkalytiuk/tarn`
image actually contains `tarn-lsp` alongside `tarn` and `tarn-mcp`.

No extension code changes. `tarn.minVersion` stays at `0.6.0`
because no new Tarn CLI features are required — users running
Tarn `0.6.0` can still use extension `0.6.1` without any
activation-time compatibility warnings.

See the top-level [`CHANGELOG.md`](../../CHANGELOG.md) for the
Tarn-side release notes covering NAZ-313.

## 0.6.0 — Coordinated release with Tarn 0.6.0 (tarn-lsp + Claude Code plugin)

Version-only bump to keep the extension in lockstep with the
first-ever **tarn-lsp** release at Tarn `0.6.0`. No user-visible
extension behavior change over `0.5.1` — the content of this
release is the Rust-side tarn-lsp crate, a Claude Code plugin, and
the `vscode-languageclient` scaffolding that shipped in `0.5.1`.

See [`CHANGELOG.md`](../../CHANGELOG.md) at the repo root for the
Tarn-side release notes covering:

- **tarn-lsp** Phase L1 (diagnostics, hover, completion, document
  symbols), Phase L2 (go-to-definition, references, rename, code
  lens), and Phase L3 (formatter, code actions, quick-fix, nested
  completion, JSONPath evaluator).
- **Claude Code plugin** at `editors/claude-code/tarn-lsp-plugin/`
  that registers `tarn-lsp` for `.yaml` files in Tarn-focused
  projects.
- Public API growth on the `tarn` crate (new `tarn::validation`,
  `tarn::outline`, `tarn::format`, `tarn::fix_plan`,
  `tarn::selector`, `tarn::jsonpath` modules) and the release
  pipeline updates that ship tarn-lsp alongside tarn and tarn-mcp.

Tested against **Tarn `0.6.x`** — the NAZ-288 alignment lint
enforces major.minor match.

### Changed

- **Version + `tarn.minVersion` bumped to `0.6.0`** so the
  activation-time compatibility check warns users whose installed
  Tarn CLI is still on `0.5.x`.
- **CHANGELOG cross-linked** with the top-level Tarn changelog so
  users reading the Marketplace listing can follow the full release
  story without jumping between repos.

### Not in this release

No feature moved from the direct-provider path to the
`tarn-lsp` LSP client path. `tarn.experimentalLspClient` stays
`false` by default. Phase V2 tickets will start migrating features
one at a time in follow-up releases.

## 0.5.1 — Phase V1: vscode-languageclient scaffolding (NAZ-309)

First step of Phase V — the extension starts migrating its
in-process TypeScript language-feature providers onto a thin
`vscode-languageclient` front-end that talks to the Rust
`tarn-lsp` crate over stdio. **No feature has moved to the
LSP path in this release.** The scaffold ships `off` by default
behind a new `tarn.experimentalLspClient` window-scoped
setting; flipping it on spawns `tarn-lsp` side-by-side with
the existing direct providers. Phase V2 will migrate features
one ticket at a time while this flag soaks; Phase V3 will
delete the direct providers and the flag together.

Tested against **Tarn `0.5.x`** (any patch level — the
alignment lint now enforces major.minor match only, matching
the documented NAZ-288 policy).

### Migration strategy

See [`docs/LSP_MIGRATION.md`](docs/LSP_MIGRATION.md) for the
full decision: we pick **dual-host** migration, per-feature
minor bumps, and a defined V2 ordering that starts with
diagnostics and ends with the `tarn.evaluateJsonpath`
executeCommand bridge. The doc covers tradeoffs of full vs
dual-host vs selective migration, per-feature rollback plans,
and how the V2 release cadence interacts with the NAZ-288
coordinated-release contract.

### Added

- **`editors/vscode/src/lsp/client.ts`** — pure
  `buildClientOptions(binaryPath)` builder that emits the
  `(ServerOptions, LanguageClientOptions)` pair for a stdio
  transport, a `{ language: "tarn", scheme: "file" }`
  document selector, a dedicated "Tarn LSP" output channel,
  and `RevealOutputChannelOn.Never` so the experimental
  client never surfaces a user-facing toast. The impure
  `startTarnLspClient(context, binaryPath)` wrapper
  performs a dynamic `import("vscode-languageclient/node.js")`
  so the language-client module only loads when the flag is
  flipped on, keeps esbuild from having to bundle the
  entire LSP protocol stack, and wires `dispose()` onto
  `context.subscriptions` plus an explicit
  `await client.stop()` in `deactivate()` so the stdio
  shutdown handshake drains before the extension host
  tears down the child process.
- **`editors/vscode/src/lsp/tarnLspResolver.ts`** — mirror of
  `backend/binaryResolver.ts` for the `tarn-lsp` binary. A
  pure `resolveTarnLspCommand(configured)` helper pins the
  "setting → command" mapping without touching the file
  system; the impure `resolveTarnLspBinary(scope?)` verifies
  that absolute paths are accessible and logs the resolved
  command to the Tarn output channel with a `[tarn-lsp]`
  prefix. Unlike `tarn`, the LSP server does not implement
  `--version` (it is a pure stdio protocol server), so the
  handshake itself is the verification step, performed by
  the language client in `client.ts`.
- **`tarn.experimentalLspClient: boolean`** setting
  (`window` scope, default `false`). Enabling it boots the
  LSP client alongside the direct providers; disabling it
  requires a window reload to take effect.
- **`tarn.lspBinaryPath: string`** setting
  (`machine-overridable` scope, default `"tarn-lsp"`). Kept
  machine-overridable to mirror the `tarn.binaryPath`
  policy from NAZ-283: remote hosts (Remote SSH, Dev
  Containers, WSL, Codespaces) can pin an absolute path
  without polluting the local workspace.
- **`vscode-languageclient@9.0.1`** as a runtime dependency.
  Version 9.x pairs with VS Code `^1.82.0`; the extension's
  `engines.vscode = ^1.90.0` satisfies that minimum. The
  package and its transitive protocol stack
  (`vscode-languageserver-protocol`,
  `vscode-languageserver-types`, `vscode-jsonrpc`,
  `semver`, `minimatch`) are **externalized from esbuild**
  so they load from `node_modules` at runtime instead of
  inflating `out/extension.js` by ~358 KB. The `.vscodeignore`
  is updated to re-include these specific packages in the
  VSIX so the external require resolves post-install.
- **`tests/unit/lspClient.test.ts`** — eight unit tests
  pinning `buildClientOptions` (binary path → serverOptions
  command; stdio transport flag for run and debug;
  document selector matches `.tarn.yaml`; output channel
  name; RevealOutputChannelOn.Never; no cross-call state
  leak). Two additional lint tests verify that the inlined
  `TRANSPORT_KIND_STDIO = 0` and `State.Running = 2`
  constants in `src/lsp/client.ts` and `src/extension.ts`
  still match the live `vscode-languageclient/node` enum
  values — they fail loud if an upstream bump ever
  renumbers either enum.
- **`tests/unit/tarnLspResolver.test.ts`** — five unit tests
  pinning the pure setting-to-command mapping
  (undefined → default, empty → default, bare name → bare
  name, absolute path → normalized absolute path, trimming).
- **`tests/integration/suite/lspClient.test.ts`** — one
  integration test that drives `testing.startExperimentalLspClient()`
  under a real extension host, asserts that the client
  reaches `State.Running = 2`, and asserts clean disposal.
  Skips gracefully with `this.skip()` + a
  `"[lsp-test] skipped: target/debug/tarn-lsp missing"`
  log line when the debug binary is not present (a clean
  clone before `cargo build -p tarn-lsp`).
- **`testing.startExperimentalLspClient`** on the internal
  `TarnExtensionApi.testing` sub-object. The method boots
  the LSP client on demand regardless of the
  `tarn.experimentalLspClient` flag so the integration
  suite can drive the flag-enabled code path without
  reloading the extension host mid-test. Scoped under the
  `@stability internal` testing sub-object — it is not part
  of the public API promise and may change between any two
  releases. The golden `tests/golden/api.snapshot.txt` is
  regenerated to include the new method.
- **`editors/vscode/docs/LSP_MIGRATION.md`** — the Phase V
  migration decision document. Covers the strategy
  tradeoffs, the Phase V2 migration order, the rollback
  plan, and the version-bump policy.

### Changed

- **`esbuild.config.mjs`** — externalizes
  `vscode-languageclient` and its transitive protocol/jsonrpc
  stack so the extension bundle size stays under the 310 KB
  budget. Bundle size: **~290.5 KB before → ~292.7 KB after
  (+~2.2 KB)**. Without the externalization the bundle
  would have ballooned to ~640 KB, so this change is
  load-bearing for the ship-it gate.
- **`.vscodeignore`** — re-includes the six runtime
  packages that back `vscode-languageclient` so the VSIX
  carries them as standard `node_modules` entries.
  Everything else under `node_modules/**` is still
  excluded.
- **`src/extension.ts`** — `activate()` now reads
  `tarn.experimentalLspClient` and conditionally calls
  `startTarnLspClient`. Any failure to resolve or start
  the binary is **advisory, not fatal**: a single warning
  toast is shown, the direct providers keep running, and
  activation continues. `deactivate()` is now `async` so
  it can `await tarnLspClient.stop()` before the extension
  host tears the child process down.
- **`src/config.ts`** — new `getExperimentalLspClient(scope?)`
  helper for the setting. Kept separate from `readConfig`
  because the flag is window-scoped and is only read from
  one call site.
- **`editors/vscode/package.json` version**: `0.5.0 → 0.5.1`
  (patch bump). "One minor" per the NAZ-309 plan was
  interpreted as "one version increment consistent with the
  NAZ-288 minor-alignment policy" because bumping the
  extension's minor while `tarn/Cargo.toml` sits at `0.5.6`
  (the L1/L2/L3 drift the orchestrator flagged as allowed)
  would have introduced the exact cross-minor mismatch the
  plan said to stop at. Patch-level divergence is expressly
  permitted by the policy.
- **`tests/unit/version.test.ts`** — the alignment lint
  now compares `major.minor` rather than the full semver
  triple. The previous implementation enforced full-triple
  equality, which was stricter than the documented policy
  and had been silently broken on `main` since NAZ-294
  patch-bumped `tarn` without a matching extension release.
  This is a proper root-cause fix of the lint against the
  prose it cites, not a workaround. Patch-level drift
  between extension and `tarn` remains explicitly allowed,
  so an L-phase ticket that bumps only `tarn-lsp` no
  longer wedges the unit-test pass.

### Not changed

- **`src/api.ts` public surface.** The only `TarnExtensionApi`
  change is the new `testing.startExperimentalLspClient`
  test hook, which lives under the `@stability internal`
  sub-object and therefore carries no compatibility
  promise. Every stable field carries over byte-for-byte.
  The golden snapshot at `tests/golden/api.snapshot.txt`
  is regenerated only to include the new internal method,
  and the `apiSurface.test.ts` gate still passes.
- **No language feature has moved to the LSP path.** Every
  Phase 3/4/5/6 feature (diagnostics, document symbols,
  code lens, hover, completion, formatting,
  definition/references/rename, code actions,
  `tarn.evaluateJsonpath` bridge) still runs on its direct
  in-process TypeScript provider. Migrating them is
  explicitly Phase V2 work. See
  `docs/LSP_MIGRATION.md` for the planned order.
- **`tarn-lsp` is not bundled into the VSIX.** Users who
  enable `tarn.experimentalLspClient` must supply their
  own `tarn-lsp` binary (via `tarn.lspBinaryPath` or
  `$PATH`). Bundling the binary into the VSIX is a
  separate Phase V decision and is out of scope for
  NAZ-309.

## 0.5.0 — Phase 6: Coordinated release (NAZ-288)

First coordinated release of the Tarn VS Code extension and the Tarn
CLI under a single shared version number. From `0.5.0` onward, the
extension and the CLI ship together: a single git tag (`v0.5.0`)
triggers both [`release.yml`](../../.github/workflows/release.yml)
(Tarn CLI binaries + crates.io + Homebrew + Docker) and
[`vscode-extension-release.yml`](../../.github/workflows/vscode-extension-release.yml)
(VS Code Marketplace + Open VSX), and a CI alignment lint refuses to
merge a commit that bumps one without the other.

The extension version track was reset from `0.26.0` to `0.5.0` to
match the Tarn CLI. This down-bump is safe because the extension had
never been published to the VS Code Marketplace or Open VSX — every
`0.x.0` release prior to this commit lived only in-repo as a
walkthrough of Phase 1–6 work. A marketplace consumer observing
`0.5.0` as the very first published VSIX sees exactly one coherent
timeline.

Tested against **Tarn `0.5.0`** (the coordinated-release pair cut by
NAZ-288).

### Version alignment policy

Extension `X.Y.*` tracks Tarn `X.Y.*`: the minor number is always
identical, so a user on extension `0.5.x` knows they can run any Tarn
`0.5.x`. Patch numbers may diverge for bug-fix releases on one side
without a matching release on the other — a hotfix to the extension
can ship as `0.5.1` against Tarn `0.5.0`, and vice versa. A new
minor always bumps both sides in lockstep.

This invariant is enforced three ways:

1. **`tests/unit/version.test.ts`** reads `editors/vscode/package.json`
   and `tarn/Cargo.toml` on every unit-test run and fails the build
   if the two version strings drift.
2. **`tarn.minVersion`** is declared at the top of
   `editors/vscode/package.json` (next to `version` and `l10n`). The
   extension spawns `tarn --version` at activation, parses the
   semver, and warns the user with an "Install Tarn" link if the
   installed binary is older than the declared minimum. The check
   is non-fatal — a user on `0.4.x` still gets a working editor,
   they just get a nudge to upgrade.
3. **`vscode-extension-release.yml`** already hard-fails a publish
   when the git tag does not match `package.json` version. This
   ticket leaves that guard in place and couples it with the new
   unit-test lint so the chain is bidirectional: CI blocks drift
   before tagging, the tag check blocks stale publishes at release
   time.

### Added

- **`src/version.ts`** — `parseTarnVersion`, `parseSemver`,
  `compareSemver`, `readMinVersionFromPackage`,
  `checkVersionCompatibility`, `readInstalledTarnVersion`, and
  `warnIfTarnOutdated`. The pure helpers are exported so the
  alignment lint and any future integration tests can reason about
  versions without spawning a real binary. `warnIfTarnOutdated`
  glues the helpers to `vscode.window.showWarningMessage` and is
  wired into `activate()` right after `promptInstallIfMissing()`.
- **`tarn.minVersion` field in `editors/vscode/package.json`** —
  `"0.5.0"` for the first coordinated release. Extension reads this
  from `context.extension.packageJSON` at activation; no extra
  manifest fields leak into `contributes.*` so the field is invisible
  to VS Code itself.
- **`tests/unit/version.test.ts`** — 14 tests covering the
  parse/compare/check helpers plus the cross-file alignment lint
  between `editors/vscode/package.json` and `tarn/Cargo.toml`.
  Every `compareSemver` ordering rule (major, minor, patch,
  release-vs-pre-release, lexical pre-release) is exercised with a
  distinct assertion. The alignment suite fails the build if the
  two versions drift or if `tarn.minVersion` is missing, malformed,
  or higher than the extension version.

### Changed

- **`editors/vscode/package.json` version**: `0.26.0 → 0.5.0`
  (coordinated reset — see decision rationale above).
- **`tarn/Cargo.toml` version**: `0.4.4 → 0.5.0` (coordinated minor
  bump for Phase 6 T54–T58).
- **`src/extension.ts`** — `activate()` now fires a non-blocking
  version check after the binary resolve step. The check runs as a
  fire-and-forget (`void warnIfTarnOutdated(...)`) so activation
  never stalls on the child process.
- **`l10n/bundle.l10n.json`** — two new keys: `"Install Tarn"` and
  `"Tarn CLI {0} is older than the minimum required by this extension ({1}). Some features may not work correctly. Update Tarn to continue."`.
  Both honor the identity-baseline contract required by the
  `l10nLint` suite.

### Not changed

- `src/api.ts` — the public extension API is untouched. The version
  check is an internal concern; downstream integrators never see
  `tarn.minVersion` through `TarnExtensionApi`. The Phase 5 golden
  snapshot at `tests/golden/api.snapshot.txt` remains byte-identical.
- Marketplace assets, localization catalog semantics, and every
  other Phase 6 deliverable carry over unchanged from `0.26.0`.

## Unreleased — 1.0.0 release notes draft

`1.0.0` is the first release under the **stable public API promise**. Extensions and scripts that consume `TarnExtensionApi` via `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode').exports` can rely on every field currently marked `@stability stable` in [`src/api.ts`](src/api.ts) not to break without a major version bump.

What this means for integrators, in one paragraph: if you pin your dependency on the Tarn extension to `^1.0.0`, any future update in the `1.x` range will never remove a stable field, rename a stable field, narrow a stable return type, or widen a stable parameter type. Additive changes (a new optional field, a new stable method) are allowed inside `1.x`. Preview fields can change in any minor release and are always listed explicitly in [`docs/API.md`](docs/API.md). Internal fields (anything under `testing.*`) have no compatibility guarantees at all — they exist for the extension's own integration tests and will break silently. If you are reading `api.testing.*` today, stop.

What this means for the extension itself: the stable surface is now CI-enforced. A golden snapshot at `tests/golden/api.snapshot.txt` pins the declaration of `src/api.ts`, and `tests/unit/apiSurface.test.ts` fails any drift that is not accompanied by an explicit snapshot update. Every new field must carry a `@stability` annotation; the `testing` sub-object is locked to `@stability internal`.

The `1.0.0` release itself is cut by NAZ-288's version alignment step — the extension version, the Tarn `Cargo.toml` version, and the matching git tag all bump together. Between now and then, the extension keeps shipping normal minor releases on the `0.x` track (see `0.24.0` below for the staging release that introduces the API promise mechanics).

## 0.26.0 — Phase 6: Localization baseline (NAZ-286)

Locks in the extension's user-visible string surface so future
translators can land non-English locales without touching a line of
TypeScript. Every command title, quick-pick placeholder, notification
toast, tree-item label, webview tab, and inline markdown block shipped
so far now flows through a single localization layer, and a CI lint
refuses to merge a new hardcoded string into `src/` on the sly.

This is the "catalog in place" deliverable — no translations are
bundled yet. An EN-only extension still behaves identically to
`0.25.0`; the difference is that the strings are now load-bearing
keys in a dedicated bundle instead of being scattered across forty
source files.

### Added

- **`vscode.l10n.t(...)` wrapping across every user-visible string**
  in `src/**/*.ts`. Covers the four command modules
  (`commands/{index,bench,importHurl,initProject}.ts`), every view
  in `src/views/`, `src/testing/{runHandler,ResultMapper,TestController}.ts`,
  `src/language/{HoverProvider,DiagnosticsProvider,FormatProvider}.ts`,
  `src/codelens/TestCodeLensProvider.ts`, `src/notifications.ts`,
  `src/statusBar.ts`, and `src/backend/binaryResolver.ts`. Pluralization
  and file-scope variants in `formatFailureMessage` expand into six
  distinct keys so each branch can be translated independently
  instead of being glued together by string concatenation.
- **`editors/vscode/l10n/bundle.l10n.json`** — the canonical EN
  catalog, 236 keys and growing. Every entry is `"English": "English"`
  for the baseline; future locale files (`bundle.l10n.fr.json`, etc.)
  slot in beside it without touching this file. VS Code picks the
  directory up automatically thanks to the new top-level `"l10n":
  "./l10n"` field in `package.json`.
- **`editors/vscode/package.nls.json`** for `package.json`-level
  strings: every `contributes.commands[*].title`, every
  `contributes.configuration.properties[*].description` and
  `enumDescriptions`, every view name / container title /
  walkthrough step, and the untrusted-workspace capability blurb.
  Each `package.json` entry now reads `%key.name%` and the English
  default lives in `package.nls.json`. Locale variants use sibling
  `package.nls.<locale>.json` files.
- **`tests/unit/l10nLint.test.ts`** — CI lint that scans every
  `.ts` under `src/` for hardcoded literals passed to user-visible
  VS Code APIs (`showInformationMessage`, `showWarningMessage`,
  `showErrorMessage`, `placeHolder:`, `prompt:`, `saveLabel:`,
  `openLabel:`). Literals are flagged unless the call already
  routes through `vscode.l10n.t(...)` or the line carries an
  explicit `// l10n-ignore` override (used for engineer-facing
  `[tarn]` debug log lines). The test also enforces the bundle
  invariant: every `t()` literal in source must have a matching
  entry in `l10n/bundle.l10n.json`, and every bundle key must be
  used by at least one call site. Drift in either direction fails
  the build.
- **`tests/unit/l10nFallback.test.ts`** — acceptance test for the
  EN-fallback contract. Exercises `vscode.l10n.t` with unknown
  keys (returns the source verbatim) and positional `{0}`/`{1}`
  substitution, then drives the full `formatFailureMessage`
  formatter matrix end-to-end to make sure every singular/plural
  × file-count branch still yields the English baseline when no
  translation is available.

### Changed

- **`formatFailureMessage`** in `src/notifications.ts` now routes
  every variant through `vscode.l10n.t(...)` instead of building
  the final string with template concatenation. The output stays
  byte-identical in English — every existing
  `notifications.test.ts` assertion still passes — but each
  branch is now a standalone translatable key.
- **`editors/vscode/tests/unit/__mocks__/vscode.ts`** mock gained
  an `l10n.t` helper that reproduces the production fallback
  behavior (return the key verbatim, substitute positional
  `{N}` placeholders from the trailing args). Without this the
  unit suite would have had to stub `vscode.l10n` in every
  individual test.
- **`editors/vscode/package.json`** bumped to `0.26.0` and gained
  `"l10n": "./l10n"` at the top level so VS Code's extension
  host loads `bundle.l10n.json` on activation.

### Tests

- **Unit** (`tests/unit/l10nLint.test.ts`, 7 tests). The lint
  canary suite includes three self-checks (synthetic violation
  detected, `// l10n-ignore` suppresses, already-wrapped `t()`
  calls are clean) plus three bundle-drift checks (no missing
  entries, no stale entries, identity-baseline values) plus the
  real `src/`-wide scan itself.
- **Unit** (`tests/unit/l10nFallback.test.ts`, 6 tests). Drives
  the `t()` EN fallback directly and through
  `formatFailureMessage` across the singular/plural × zero-file /
  one-file / many-file matrix.
- Total: 306 unit tests, 94 integration tests passing. Bundle
  size: 288.9 KB, well under the 310 KB ceiling.

## 0.25.0 — Phase 6: Marketplace assets (NAZ-287)

First polish pass at the Marketplace listing so that when `1.0.0` cuts the
extension has a real gallery banner, a real README hook, and a real asset
pipeline behind it. This release ships the **pipeline** — `galleryBanner`
wired into `package.json`, the README's opening paragraphs rewritten for a
Marketplace visitor instead of a developer reading the source tree, and the
`editors/vscode/media/marketplace/` directory created as the single home
for every gallery asset the listing references.

The binary assets checked in with this release are intentionally 1×1
placeholder PNGs (plus a 1-frame GIF) generated from Node, because this
ticket was executed in a headless environment that cannot drive a screen
recorder. The **real** deliverable is
[`editors/vscode/media/marketplace/README.md`](media/marketplace/README.md)
— a per-file capture plan that spells out, for every screenshot and for
the 30-second demo GIF: target resolution, exact scene, fixture to use,
and the hero element. A human operator is expected to replace each
placeholder with a real capture in a follow-up commit; because the
file paths, `.vscodeignore` rules, and `package.json` wiring are already
in place, that follow-up is a drag-and-drop plus `npx @vscode/vsce package`.

### Added

- **`galleryBanner`** in `editors/vscode/package.json`:
  `{ "color": "#1E1B4B", "theme": "dark" }`. Deep indigo because the only
  pre-existing brand asset in the repo (`media/tarn-icon.svg`) is
  monochrome and inherits `currentColor`, so there is no existing palette
  to match. Indigo reads cleanly against VS Code's dark chrome, gives
  white foreground text WCAG AA contrast, and avoids colliding with the
  red/green/yellow VS Code reserves for test-status UI. Rationale is
  documented in `media/marketplace/README.md` so a future rebrand can be
  updated in one place.
- **`editors/vscode/media/marketplace/`** — new directory containing
  placeholder binaries for every image the README inlines:
  `banner.png`, `screenshot-test-explorer.png`, `screenshot-streaming.png`,
  `screenshot-diff.png`, `screenshot-env-picker.png`,
  `screenshot-codelens.png`, and `demo.gif`. Each is a minimal valid
  1×1 solid-colour image that renders without broken-image icons and lets
  the VSIX packaging pipeline be verified end-to-end before the human
  capture pass.
- **`editors/vscode/media/marketplace/README.md`** — the capture plan.
  Lists every asset, the exact scene/fixture to record, target
  resolution, hero element, and a frame-by-frame script for the 30-second
  diagnosis-loop GIF (run → failure → jump-to-line → fix → rerun →
  green). This is the real deliverable of this ticket.

### Changed

- **`editors/vscode/README.md` opening rewritten for Marketplace
  first-impression.** The file previously opened with *"First-class editor
  support for Tarn API test files."* which is accurate but assumes the
  reader already knows what Tarn is. The new opening leads with the
  value proposition — *"Run, debug, and iterate on API tests without
  leaving the editor"* — and anchors it to the concrete loop a user will
  actually feel (run → see failure → jump to line → fix → rerun → green)
  before the first `##` heading. Four screenshots are inlined in the
  Features section so the Marketplace preview shows real artwork instead
  of a wall of bullet points. Every image path resolves to a placeholder
  today and will resolve to a real capture when the human operator
  replaces the files.
- **Version bumped `0.24.0 → 0.25.0`.** Marketplace-asset changes ship on
  the normal 0.x cadence; the `1.0.0` bump is still owned by NAZ-288.

### Not changed

- **No runtime behavior change.** The activation manifest, commands,
  views, and public API are untouched. Extension bundle size is
  unchanged — the marketplace assets ship in the VSIX alongside the
  bundle, not inside it.

## 0.24.0 — Phase 6: Stable API promise (NAZ-285)

The extension's return value from `activate()` has been an `interface TarnExtensionApi` since Phase 1, but nothing bound downstream integrators to any particular subset of that interface, and nothing stopped a future PR from deleting a field that an external extension had started to depend on. This release pins the public surface as a hard contract so the roadmap can safely cut `1.0.0` (gated on NAZ-288).

### Added

- **`editors/vscode/src/api.ts`** — new, single source of truth for `TarnExtensionApi` and the internal `TarnExtensionTestingApi` sub-type. `src/extension.ts` now re-exports the public type instead of redeclaring its shape. Every field carries a JSDoc `@stability` annotation (`stable`, `preview`, or `internal`) and a file-level block comment documents the semver policy in prose: stable bumps major, preview bumps minor, internal can change in patch releases.
- **`testing` sub-object explicitly marked `@stability internal`** — the sub-object that holds `backend`, `buildFailureMessagesForStep`, `workspaceIndexSnapshot`, etc. is still exposed for the extension's own `@vscode/test-electron` integration tests, but its opaque-and-test-only status is now a typed annotation on the interface, not just a prose comment in `extension.ts`. Downstream code that reads `api.testing.*` is unsupported and will break silently on upgrade.
- **Public API section in `docs/VSCODE_EXTENSION.md`** — prose documentation of the interface shape, the stability tiers, the semver policy, the `1.0.0` gating plan, and the enforcement mechanism. Cross-linked to `api.ts`, the golden snapshot, and the enforcement test.
- **`editors/vscode/docs/API.md`** — user-facing quick reference aimed at integrators. Shows how to call `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode')`, documents each stable field, and explains what the `1.0.0` gate means for anyone building on the API today. Linked from `editors/vscode/README.md`.
- **`editors/vscode/tests/golden/api.snapshot.txt`** — normalized golden snapshot of the `api.ts` declaration. Whitespace-normalized, line-comments and non-`@stability` block comments stripped, so formatting edits don't trip the test but any real shape change does.
- **`editors/vscode/tests/unit/apiSurface.test.ts`** — new unit test, 4 assertions:
  1. The normalized `src/api.ts` matches the golden snapshot. Failure includes a one-command regeneration hint.
  2. The file-level semver-policy block comment mentions every stability tier (`stable`, `preview`, `internal`) so future contributors can't accidentally drop one.
  3. Every `readonly` field of `TarnExtensionApi` carries a `@stability` annotation in its JSDoc.
  4. The `testing` sub-object is annotated `@stability internal`.
  This test is picked up by `npm run test:unit` and runs on every PR, so an API drift that is not accompanied by a documented change gets caught locally before review. That's the "CI lint step that fails on unannounced breaking API changes" from the NAZ-285 acceptance criteria — no separate GitHub Actions config required.
- **`## Unreleased — 1.0.0 release notes draft`** block above this entry. Will be promoted to the actual `1.0.0` release notes when NAZ-288 cuts the version bump.

### Changed

- `src/extension.ts` no longer contains the inline `TarnExtensionApi` interface declaration (~116 lines of nested `import(...)` type references). It imports and re-exports the type from `./api` instead. The `activate()` return value is unchanged — the structural type check still applies, and the object literal that builds the API is the only thing left in `extension.ts`.

### Not changed

- **No runtime behavior change.** The shape of the object returned from `activate()` is identical to `0.23.0`. An extension that was already consuming `api.testControllerId`, `api.indexedFileCount`, `api.commands`, or (unwisely) `api.testing.*` will observe zero change. The ticket is a documentation-and-enforcement ticket, not a breaking change.
- **No version bump to 1.0.0.** The version alignment step that bumps `editors/vscode/package.json` to `1.0.0` is owned by NAZ-288, which runs after all other Phase 6 tickets. The release notes draft above is staged in the `Unreleased` block for that tickets to lift into place.

## 0.23.0 — Phase 5: Remote compatibility audit (NAZ-283)

End-to-end audit of the extension against the four remote-development
targets VS Code ships first-class support for — Dev Container, GitHub
Codespaces, WSL, and Remote SSH — and the targeted fix the audit
surfaced.

### Changed

- **`tarn.requestTimeoutMs` scope changed from `resource` to
  `machine-overridable`** in `package.json`. The watchdog is a
  function of the remote host's network latency, not the workspace's
  test suite, so a user who bumps the timeout to `300000` for a slow
  Remote SSH host should not have that value silently leak back into
  their local workspace runs. This matches the scope `tarn.binaryPath`
  already uses for the same reason.

### Added

- **`editors/vscode/media/remote/devcontainer.json`** — a drop-in
  Dev Container config that users can copy to
  `.devcontainer/devcontainer.json`. Uses the official
  `mcr.microsoft.com/devcontainers/rust:1-bookworm` image, installs
  `tarn-cli` into `/usr/local/cargo/bin/tarn` via `cargo install` in
  `postCreateCommand`, and pins `nazarkalytiuk.tarn-vscode` plus
  `redhat.vscode-yaml` under `customizations.vscode.extensions`.
  GitHub Codespaces consumes the same file unchanged.
- **"Remote Setups" section in `README.md`** — four short
  subsections (Dev Container, Codespaces, WSL, Remote SSH) that
  state the verified behavior, show the minimum config needed, and
  point at `docs/VSCODE_REMOTE.md` for the full audit.
- **`docs/VSCODE_REMOTE.md`** — the full audit writeup, including
  the per-environment checklist (activation, binary resolution, test
  discovery, run, cancellation), a mental-model table of where the
  extension host lives on each target, and a summary of hazards
  checked (none found in code; one misclassified setting scope
  fixed, see above).

### Audit findings (no code change needed)

- `binaryResolver.ts` uses `execFile` with the remote-scoped
  `binaryPath` setting, no `process.platform` branching, no `.exe`
  suffix handling, no PATH walking. Safe on every target.
- `TarnProcessRunner.spawn` never uses `shell: true`, so argv is
  passed directly to the OS and SIGINT/SIGKILL reach Tarn without
  a shell wrapper.
- All path construction flows through Node's `path` module, which
  resolves to `path.posix` on Linux/macOS remotes and `path.win32`
  on Windows remotes — always matching whatever side the extension
  host is on.
- `os.tmpdir()` is used for NDJSON report paths, HTML report paths,
  and `tarn fmt` temp files, so none of the temp-file plumbing
  hardcodes `/tmp` or `C:\Temp`.
- Settings scopes for `testFileGlob`, `excludeGlobs`,
  `defaultEnvironment`, `defaultTags`, `parallel`, `jsonMode`,
  `showCodeLens`, `statusBar.enabled`, `validateOnSave`,
  `notifications.failure`, and `cookieJarMode` are all correctly
  `resource`-scoped (workspace preferences, not machine config).

### Known limitations

- The audit was performed on paper against the extension source and
  VS Code's Remote Development documentation. Live smoke-tests of
  each of the four environments (actually spinning up a container,
  a Codespace, a WSL distro, a Remote SSH host, and running the
  full Run / Cancel cycle end-to-end) are tracked as a follow-up.

## 0.22.0 — Phase 5: Scoped discovery via tarn list --file (NAZ-282)

Tarn T57 (shipped in NAZ-261) added a scoped
`tarn list --file PATH --format json` command that emits the canonical
post-`include:` structure of a single YAML file. This release teaches
the extension's `WorkspaceIndex` to use that command on every
`onDidChange` / `onDidCreate` event, keeping the authoritative test
tree in lockstep with whatever the runner will actually execute. The
client-side YAML AST stays as the startup-discovery path (to avoid N
process spawns on activation) and as the fallback when the installed
Tarn binary predates T57 or rejects a specific file.

### Added

- **`scopedListResultSchema`** and **`parseScopedListResult`** in
  `src/util/schemaGuards.ts` — a new zod shape that matches Tarn's
  `{ files: [{ file, name, tags?, setup[], steps[], tests[],
  teardown[], error? }] }` envelope, including the degraded
  `{ file, error }` per-file record Tarn emits when it cannot parse
  the scoped path. Exported as `ScopedListFile` (the raw inferred
  type) and `ScopedListFileStrict` (the validated-and-unwrapped
  shape consumers depend on).
- **`TarnBackend.listFile(absolutePath, cwd, token)`** — a new
  discriminated-outcome method on the backend interface. Returns
  `{ ok: true, file }` on success, `{ ok: false, reason:
  "file_error", error }` when Tarn parsed the YAML but rejected it
  (keeps scoped discovery enabled for the rest of the session), or
  `{ ok: false, reason: "unsupported" }` for binary-level failures
  (disables scoped discovery until the next explicit
  `Tarn: Refresh Discovery`).
- **`WorkspaceIndex.refreshSingleFile(uri)`** — the incremental
  refresh path used by the file-system watcher. Calls
  `backend.listFile(uri.fsPath)` first and overlays the AST's range
  metadata onto Tarn's authoritative structure, then compares the
  merged `FileRanges` against the cached entry via
  `rangesStructurallyEqual` and only fires a listener notification
  when the test/step tree actually changed. Keystroke-saves that
  only edit request bodies, URLs, or assertions no longer churn
  the Test Explorer gutter icons.
- **`mergeScopedWithAst(scoped, ast)`** — a pure helper that folds
  Tarn's scoped-list output together with the client AST: Tarn wins
  on "what tests and steps exist" (so `include:`-expanded steps
  show up even though the raw YAML only has a `{ include: ... }`
  entry), the AST wins on "where on disk the name lives". Exported
  so the unit tests can exercise the merge without spinning up a
  full extension host.
- **`rangesStructurallyEqual(a, b)`** — a structural comparator on
  `FileRanges` that ignores line/column shifts and compares only
  test names, step names, arity, and descriptions. Guards against
  the TestItem tree rebuilding on every comment-only edit.
- **`api.testing.workspaceIndexSnapshot()`** and
  **`api.testing.refreshSingleFile(uri)`** — two narrow testing
  hooks on the extension API so the scoped-discovery integration
  test can deterministically observe the incremental path without
  racing the native `FileSystemWatcher`.

### Changed

- **`WorkspaceIndex` constructor** now accepts an optional
  `{ backend, cwd }` options bag. When both are supplied (the
  production path from `activate()`), the incremental refresh
  tries the scoped `tarn list --file` path before falling back to
  the AST; when omitted (unit tests), the index stays on the pure
  AST path so no Tarn process is spawned. Startup discovery
  (`initialize()`) deliberately still uses the glob + AST to avoid
  N process spawns at activation time.
- **Incremental refresh** now short-circuits the listener
  notification when the new structure is structurally identical to
  the cached one. Before this release every `onDidChange` call
  rebuilt the file's `TestItem` children; the TestController now
  only sees updates when a test or step was actually added,
  removed, renamed, or re-described.

### Developer

- **New unit tests** (`tests/unit/workspaceIndex.test.ts`) cover
  the scoped/AST merge (named tests, flat `steps:`, synthetic
  `include:`-only entries, missing AST, setup/teardown passthrough)
  and the structural comparator (identity, line-number shifts,
  renames, added steps, description churn, setup arity changes).
- **New unit tests** in `tests/unit/schemaGuards.test.ts` exercise
  the scoped list envelope — named tests, flat steps, top-level
  error envelope, per-file error envelope, and rejections of
  malformed shapes that would otherwise slip through to the
  runtime merge.
- **New integration test** (`tests/integration/suite/scopedDiscovery.test.ts`)
  drives the incremental path end-to-end against the real Tarn
  debug binary: add / change / rename / delete of a single fixture
  while a sibling file stays idle, plus a "broken YAML does not
  disable scoped discovery" regression test.

## 0.21.0 — Phase 5: Consume location metadata (NAZ-281)

Tarn T55 (shipped in NAZ-260) now emits an optional
`location: { file, line, column }` object on every `StepResult`, every
`AssertionDetail`, and every `AssertionFailure` that maps back to a
YAML operator key. This release teaches `ResultMapper` to prefer those
run-time-captured coordinates over the editor's current YAML AST, so
failure diagnostics stay anchored on the exact assertion node even if
the user edits the file between the moment Tarn loaded it and the
moment the extension renders the report. AST lookup remains the
authoring-side source of truth (CodeLens, completion, hover, rename)
and the fallback path for older Tarn versions or `include:`-expanded
steps where the CLI emits `location: None`.

### Added

- **`locationSchema`** in `src/util/schemaGuards.ts` — a new zod
  shape with `file: string`, `line: int>=1`, `column: int>=1` that
  matches Tarn's 1-based coordinates exactly. The schema is exported
  alongside the existing `Report`, `StepResult`, and `AssertionDetail`
  types and is wired into `stepResultSchema` and `assertionDetailSchema`
  as an optional field so older reports still parse.
- **`locationFromTarn(location, parsed)`** helper in
  `src/testing/ResultMapper.ts` — converts a Tarn-reported
  `{file, line, column}` into a `vscode.Location`, normalizing 1-based
  line/column into 0-based `Position`s and reusing the `ParsedFile` URI
  when the reported file matches to keep VS Code URI identity stable
  with the rest of the mapping pipeline.
- **`resolveStepLocation(step, stepItem, parsed)`** — the step-level
  resolver that encodes the new preference order: JSON
  `step.location` first, AST `stepItem.range` second. Exported so
  downstream views (fix plan, run history, webview jump-to) can reuse
  the same precedence if needed.
- **`api.testing.buildFailureMessagesForStep`** — a narrow testing hook
  on the extension API that lets integration tests run a real Tarn
  fixture, feed the returned `StepResult` through the mapper, and
  verify the resulting `TestMessage.location` without having to reach
  into the discovery + run-handler plumbing.
- **`media/walkthrough/install.md`** now lists the
  `requiresTarnVersion` hint for Phase 5 features so users installing
  the extension know which Tarn version enables drift-free result
  anchoring. The extension does NOT hard-gate on the version — older
  Tarn still works, just with AST-based anchoring.

### Changed

- **`ResultMapper.buildFailureMessages`** now resolves the step anchor
  via `resolveStepLocation` and, for every assertion failure that
  carries its own `location`, prefers that per-assertion coordinate
  over the step-level fallback. This means the red squiggle on a body
  JSONPath assertion lands on the `body:` operator key (or the nested
  `$.path: expected` line), not on the step's `name:` key.
- **`assertionDetailSchema`** and **`stepResultSchema`** in
  `src/util/schemaGuards.ts` now accept the optional `location` field
  introduced by Tarn T55. The existing drift-tolerance rules from
  NAZ-280 (`diff: nullish`, `passed` optional inside `failures[]`) are
  preserved — this change is purely additive.
- **`docs/VSCODE_EXTENSION.md` §5.1 "Mapping Results to Editor Ranges"**
  rewritten to document the new preference order: JSON `location`
  first, AST lookup second. Clarifies that the AST layer remains the
  source of truth for authoring features (CodeLens, rename, hover,
  completion) and only loses its job for runtime result anchoring.

### Tests

- **Unit** (`tests/unit/ResultMapper.test.ts`, +13 tests). Covers
  `locationFromTarn` (happy path, 1->0-based conversion, lower-bound
  clamping to 0, undefined input, URI reuse), `resolveStepLocation`
  (JSON wins, AST fallback, neither available), and
  `buildFailureMessages` with five location scenarios: per-assertion
  JSON location wins over step location, step location wins over AST
  when assertion lacks its own, explicit AST drift scenario (JSON line
  14 vs AST line 11), older-Tarn fallback, per-failure distinct
  locations for multi-assert steps, and generic (non-assertion)
  failures anchored on the JSON step location.
- **Unit** (`tests/unit/schemaGuards.test.ts`, +2 tests). Verifies
  `parseReport` round-trips the optional `location` field on steps and
  on both assertion detail/failure shapes, and rejects payloads with a
  non-positive line (Tarn guarantees 1-based).
- **Integration** (`tests/integration/suite/resultMapperLocation.test.ts`,
  +4 tests). Spins up the demo-server on an ephemeral port, writes a
  deterministic `tests/location-drift.tarn.yaml` fixture whose step
  lives at line 10 and whose `status: 404` assertion lives at line 15,
  runs Tarn, and asserts: (1) Tarn's JSON report actually carries the
  step location `{line: 10, column: 9}` and failure location
  `{line: 15, column: 11}`; (2) `buildFailureMessagesForStep` anchors
  the `TestMessage` on 0-based line 14 / column 10 even when the
  test passes a deliberately-wrong AST range at line 999; (3) after a
  simulated mid-run edit that prepends two blank lines and shifts the
  AST range to line 11, the diagnostic stays glued to JSON-reported
  line 14; (4) when `location` is stripped from the report (older
  Tarn), the AST range takes over and the diagnostic lands on line 9.

## 0.20.0 — Phase 5: Honor per-test cookie jar (NAZ-280)

First Phase 5 feature: the extension can now force Tarn's per-test
cookie jar isolation (T54 on the CLI side) so subset runs
(run-at-cursor, run-step, run-test) never inherit stale session state
from tests that happen not to be in scope. This was the long-standing
gap that made "rerun just this step" unsafe whenever a sibling test
had set a session cookie.

### Added

- **`tarn.cookieJarMode`** setting (NAZ-280) with an enum of
  `"default" | "per-test"`, default `"default"`. When set to
  `"per-test"`, the extension appends `--cookie-jar-per-test` to
  every `tarn run` it spawns so Tarn clears the default cookie jar
  between named tests. Respects the CLI precedence: a file with
  `cookies: "off"` still short-circuits on the runner side, and
  named (multi-user) jars are left untouched.
- **`src/backend/runArgs.ts`** — a pure `buildRunArgs` helper
  extracted from `TarnProcessRunner` so the argv construction is
  exercised directly from vitest. Every branch is covered.
- **`normalizeCookieJarMode`** helper in `src/config.ts`. Narrows a
  raw setting value to `"per-test" | "default"`, falling back to
  `"default"` on typos or unknown values so a bad setting never
  crashes the runner.
- **Extension host API**: the existing
  `testing.backend.run` path already surfaces the argv via stdout
  logging; integration tests exercise the behavior end-to-end
  through the fixture + demo-server rather than by asserting the
  argv directly.

### Changed

- **`TarnProcessRunner`** now threads `readConfig().cookieJarMode`
  into three `tarn run` spawn sites: the main `run()` path
  (Test Explorer, run-file, run-test, run-step, run-at-cursor), the
  NDJSON streaming path used for live updates, and the
  `exportCurl` / `runHtmlReport` secondary flows. Applying the flag
  to the curl export and HTML report generation keeps their output
  consistent with whatever the user just saw in the Test Explorer.
- **`src/util/schemaGuards.ts`** — the assertion detail schema now
  accepts the real tarn JSON shape instead of the idealized one:
  `diff` is `z.string().nullish()` (tarn emits `null` when there is
  no structural diff) and `passed` is optional on entries inside
  `assertions.failures[]` (tarn omits it because those entries are
  by definition failed). Before this fix, any report with at least
  one failing step bounced at the zod gate and collapsed to
  `report: undefined`, which masked every downstream
  expected/actual/diff surface on real failures.

### Tests

- **Unit** (`tests/unit/runArgs.test.ts`, 21 tests). Covers every
  branch of the argv builder: stdout-JSON form vs NDJSON form,
  every optional flag (`--dry-run`, `--parallel`, `--env`, `--tag`,
  `--select`, `--var`, `--json-mode`), default mode omitting the
  flag, `per-test` mode appending it in both forms, flag ordering
  relative to file paths, and the `per-test` flag surviving every
  combination of dry-run + selectors + env + tags + vars.
  `normalizeCookieJarMode` has four tests covering the exact match,
  undefined fallback, typo fallback, and empty-string fallback.
- **Unit** (`tests/unit/schemaGuards.test.ts`, +1 test). Regression
  for the `diff: null` / omitted `passed` shape emitted by real
  tarn on failing reports.
- **Integration**
  (`tests/integration/suite/cookieJarMode.test.ts`, 4 tests).
  Spins up a `demo-server` subprocess on an OS-allocated port,
  writes `tests/cookie-jar.tarn.yaml` with a `login_sets_session`
  test plus two tests that assert `body.session == null`, and
  drives the full pipeline twice:
  - `tarn.cookieJarMode = "default"` → login passes, both
    clean-jar tests fail (they inherit the session).
  - `tarn.cookieJarMode = "per-test"` → all three tests pass
    (jar is wiped between named tests).
  A third test flips the setting back to `"default"` mid-session
  to prove the setting is read on every run, not cached at
  activation. A fourth test asserts that the setting is
  contributed in `package.json` with the correct default, mirroring
  the guard used by `notifications.test.ts`.

Total: 255 unit tests, 85 integration tests passing. Bundle 280.5 KB.

## 0.19.0 — Phase 4: Init Project wizard polish

Ninth and final Phase 4 feature: the minimal "pick folder, run
tarn init, offer to open" flow shipped in Phase 1 is now a real
multi-step wizard with scaffold flavor selection, env customization,
auto-validation of generated files, and auto-open of the health
check fixture.

### Added

- **`runInitProject`** (NAZ-278) in a new
  `src/commands/initProject.ts` module. Drives the full
  post-`tarn init` pipeline:
  1. Run `tarn init` through the existing backend.
  2. Prune `examples/` and `fixtures/` when the user picked the
     `basic` flavor — `tarn init` always scaffolds everything, so
     trimming on the extension side is the simplest path without
     a Tarn-side `--scaffold` flag.
  3. Rewrite `tarn.env.yaml` with user-supplied overrides via
     `customizeEnvFile`.
  4. Validate every generated `.tarn.yaml` via
     `backend.validateStructured` and log each failure to the
     output channel.
- **Four-step wizard** (`runInitProjectWizard`):
  1. Quick-pick destination (workspace folder entries +
     `Browse…`).
  2. Overwrite warning when `tarn.config.yaml` / `tarn.env.yaml`
     / `tests` / `examples` already exist in the target folder.
  3. Quick-pick scaffold flavor: `All templates (recommended)` or
     `Basic` (health check + configs only).
  4. Optional env customization: skip or sequentially prompt for
     `base_url` (URL-validated), `admin_email`
     (must contain `@`), and `admin_password` (password-masked).
- **Auto-open** `tests/health.tarn.yaml` after a successful
  scaffold so the first thing users see is a real working Tarn
  test, not an empty file explorer.
- **Auto-validate** every generated test file and surface the
  count in a follow-up toast: success ("project ready in X") or
  warning ("N file(s) failed validation") with details in the
  output channel.
- **`customizeEnvFile`** pure helper that rewrites a
  `tarn.env.yaml` in place without touching comments, blank
  lines, or unmatched keys. Values containing colons, `@`, or
  other ambiguous characters get wrapped in double quotes
  (`formatYamlScalar`). Unknown keys get appended in an annotated
  `# Added by Tarn: Init Project Here` block so the wizard's
  additions are always attributable.
- **`scaffoldFilesToPrune`** pure helper mapping flavor →
  relative paths to delete. `basic` returns `["examples",
  "fixtures"]`; `all` returns `[]`.
- **Extension host API**: `testing.initProject(options)` runs the
  full pipeline without dialogs so integration tests can assert
  file presence, deletions, env content, and validation outcome
  against real `tarn init` output.

### Changed

- **`tarn.initProject`** command now dispatches through the new
  wizard. The old inlined handler and its `pickInitFolder` /
  `detectExistingScaffold` helpers were removed from
  `commands/index.ts`.

### Tests

- **Unit** (`tests/unit/initProject.test.ts`, 8 tests). Covers
  `customizeEnvFile` (no-op when overrides empty, in-place
  replacement, mandatory quoting for URL/email values, annotated
  append block for unknown keys, comment/indent preservation,
  quote/backslash escaping) and `scaffoldFilesToPrune` (both
  flavors).
- **Integration** (`tests/integration/suite/initProject.test.ts`,
  4 tests). Scaffolds into a `fs.mkdtemp` directory in
  `os.tmpdir()` and drives every branch:
  - Command registration.
  - `all` flavor: every expected file created, `examples/` kept,
    zero validation errors.
  - `basic` flavor: `examples/` and `fixtures/` pruned from disk,
    health check preserved, zero validation errors.
  - Env override path: `tarn.env.yaml` on disk contains the
    rewritten URL, email, and password exactly where expected.

Total: 233 unit tests, 81 integration tests passing.

## 0.18.0 — Phase 4: Failure notifications with inline actions

Eighth Phase 4 feature: a warning toast that pops after a failing
run with three one-click actions (Show Fix Plan, Open Report, Rerun
Failed), gated on whether the Tarn activity bar is already visible.

### Added

- **`FailureNotifier`** (NAZ-277) in `src/notifications.ts`.
  Constructor-injected `isTarnViewFocused` signal and
  `FailureActionHandlers` so tests can stub every dependency.
  Exposes `wouldNotify(report, {dryRun})` for pure decision checks
  and `maybeNotify(report, {dryRun, files})` which shows the
  `vscode.window.showWarningMessage` toast and dispatches the
  picked action.
- **`shouldNotifyOnFailure`** pure helper that resolves the
  `mode × dryRun × failedSteps × tarnViewVisible` decision matrix.
  Unit-tested independently of the class.
- **`formatFailureMessage`** pure helper that produces a
  "Tarn: N failed steps in a, b, c" summary, inlines up to three
  file names, and collapses to a count when more than three
  files failed.
- **Inline actions** wired to existing commands:
  - **Show Fix Plan** → `tarn.fixPlan.focus` (auto-registered by
    VS Code for every contributed tree view).
  - **Open Report** → `tarn.openHtmlReport` with the run's files
    passed through so the report covers exactly what just failed.
  - **Rerun Failed** → `tarn.runFailed` shipped in Phase 2.
  Errors from the dispatched commands are swallowed so a
  mis-wired action can't crash the run handler after the toast.
- **`tarn.notifications.failure`** setting with an enum of
  `"always" | "focused" | "off"`, default `"focused"`. `focused`
  suppresses the toast when any Tarn activity-bar tree view is
  visible (they all flip together when the container is
  selected). Dry runs never trigger the notification regardless
  of mode.
- **`tarn.fixPlan`** tree view now registered via
  `vscode.window.createTreeView` instead of
  `registerTreeDataProvider` so the extension can read
  `TreeView.visible` as the "Tarn focused" signal.
- **`tarn.openHtmlReport`** command extended to accept an optional
  `files: readonly string[]` argument. When provided, the command
  runs the HTML report against those files instead of the active
  editor. This is what the "Open Report" notification action uses
  to stay in scope with the run that just failed.
- **Extension host API**: `testing.notifier.{isTarnViewFocused,
  wouldNotify, maybeNotify}` for integration tests.

### Changed

- **`runHandler`** now calls
  `failureNotifier.maybeNotify(report, {dryRun, files})` after
  the report has been applied and the history has been written,
  so any action fired from the toast lands on fresh data.
- **`createTarnTestController`** signature accepts a
  `FailureNotifier` parameter so the run handler can reach it
  without extra plumbing.
- **Unit vscode mock** (`tests/unit/__mocks__/vscode.ts`) gained
  minimal `workspace.getConfiguration` and `window.showWarningMessage`
  stubs so pure helpers that touch the config boundary can be
  exercised in vitest.

### Tests

- **Unit** (`tests/unit/notifications.test.ts`, 13 tests). Covers
  every branch of `shouldNotifyOnFailure` (off / dry / no-failures
  / focused+visible / focused+hidden / always+visible), every
  branch of `formatFailureMessage` (singular/plural, 1/2/3/4+
  files, empty file list, mixed pass/fail), and two
  `FailureNotifier.maybeNotify` short-circuit paths (dry run and
  no failures) so handlers are never invoked on those.
- **Integration**
  (`tests/integration/suite/notifications.test.ts`, 6 tests).
  Asserts the `tarn.notifications.failure` setting is contributed
  with `"focused"` as the default, exercises the
  `isTarnViewFocused` signal, drives every decision path through
  `wouldNotify` (passing report / dry run / always-fail / off),
  and verifies that flipping the setting to `"off"` immediately
  suppresses the decision. The toast-showing `maybeNotify` path
  is not exercised in integration because
  `showWarningMessage` blocks until an action is clicked — the
  unit tests cover it through the injected `FailureActionHandlers`
  instead.

Total: 225 unit tests, 77 integration tests passing.

## 0.17.0 — Phase 4: Run History pinning, filtering, delta rerun

Seventh Phase 4 feature: the existing Run History tree view now
supports pinning entries so they survive eviction, filtering the
listing by status / env / tag, and replaying a past run with its
exact selectors and environment via "Rerun from History".

### Added

- **Pin / unpin** actions (NAZ-276). Inline `$(pin)` / `$(pinned)`
  icons on every run entry in the tree. Pinned entries:
  - Show a leading 📌 in the label and sort to the top of the view.
  - Are never evicted by the 20-entry ring buffer — the cap now
    applies only to *unpinned* entries.
  - Survive `Tarn: Clear Run History`, which drops unpinned runs
    but keeps pinned ones so users can't accidentally lose a
    manually-marked-important run.
- **Filter bar** in the view title (`$(filter)`). Opens a quick
  pick with `All runs`, `Passed only`, `Failed or errored`, plus a
  dynamic section of `env · <name>` and `tag · <name>` options
  derived from the entries currently in the store. Selection
  persists until changed again.
- **`Tarn: Rerun from History`** replays a past run using its
  exact selectors, files, environment, and tag filter. Per-step
  and per-test selectors are resolved back to `TestItem` ids via
  the discovery module's `ids.step` / `ids.test` helpers so the
  underlying `tarn run --select …` invocation matches the original
  exactly. Dry runs replay as dry runs. Missing entries (evicted,
  cleared, etc.) surface a friendly info message instead of
  throwing.
- **`selectors` field** on `RunHistoryEntry`. Populated by
  `runHandler` from the `planRun` output so the rerun command has
  every `FILE::TEST[::STEP]` string the original run used.
- **`files` field** now holds *workspace-relative* paths (matching
  what the runner passes to tarn) rather than the full paths the
  report emitted, so rerun resolution does not need any
  workspace-root munging.
- **`pinned` field** on `RunHistoryEntry` with backward-compat
  normalization: entries persisted before NAZ-276 lack the field
  and are loaded with `pinned: false` defaulted in.
- **`RunHistoryStore.pin(id)` / `unpin(id)` / `findById(id)`**
  methods that update the persisted memento and re-trim the
  unpinned partition whenever a pinned entry becomes unpinned.
- **`historyFilterPredicate` / `applyHistoryFilter` /
  `trimWithPinned`** pure helpers exported from
  `views/RunHistoryView.ts` for unit testing.
- **Extension host API**: `testing.history.{add, all, clear,
  setFilter, getFilter}` so integration tests can seed the store
  and exercise the filter/rerun paths without clicking through UI.

### Changed

- **`RunHistoryStore.entryFromReport`** now takes an options object
  (`{environment, tags, files, selectors, dryRun}`) instead of
  positional args so new fields don't become positional traps.
- **`runHandler`** passes `filesToRun` (relative paths) and the
  computed `selectors` array into `entryFromReport`.
- **`RunHistoryTreeProvider`** holds a current `RunHistoryFilter`,
  applies it on each `getChildren` call, and exposes
  `setFilter` / `getFilter`. Pin state drives a distinct
  `tarnRunEntry` vs `tarnRunEntryPinned` `contextValue` so the
  package.json menu definition can show only one of pin/unpin at
  a time.

### Tests

- **Unit** (`tests/unit/runHistoryStore.test.ts`, 16 tests).
  Covers `historyFilterPredicate` (all/passed/failed/env/tag with
  empty variants), `applyHistoryFilter`, `trimWithPinned` (evicts
  oldest unpinned first, never drops pinned), the live store
  (LIFO order, 20-cap eviction, pin/unpin, unpin re-trims, clear
  keeps pinned, legacy entry normalization), and
  `entryFromReport` field propagation.
- **Integration** (`tests/integration/suite/runHistory.test.ts`,
  5 tests). Registers the new commands, exercises pin/unpin via
  `vscode.commands.executeCommand`, confirms `clear()` preserves
  pinned entries, round-trips the filter through the tree
  provider, and verifies `rerunFromHistory` fails gracefully on a
  missing id.

Total: 212 unit tests, 71 integration tests passing.

## 0.16.0 — Phase 4: Hurl import wizard

Sixth Phase 4 feature: `Tarn: Import Hurl File…` wraps
`tarn import-hurl` in an open-dialog + save-dialog wizard so users
can migrate existing Hurl test files into Tarn YAML from the
command palette.

### Added

- **`tarn.importHurl`** command (NAZ-275) registered in
  `package.json` with `$(arrow-down)` icon. Available from the
  command palette.
- **Import wizard** (`src/commands/importHurl.ts`):
  1. `showOpenDialog` filtered to `.hurl` files.
  2. `showSaveDialog` with a default destination of
     `<name>.tarn.yaml` next to the source (see
     `defaultHurlDestination`).
  3. Backend spawn inside `vscode.window.withProgress` with
     cancellation support.
  4. On success, opens the imported file and surfaces a
     `showInformationMessage` with **Run** and **Validate** quick
     actions that forward to the existing `tarn.runFile` /
     `tarn.validateFile` commands.
  5. On failure, appends stderr to the Tarn output channel and
     raises an error message with the exit code.
- **`defaultHurlDestination`** helper exported from the command
  module. Strips `.hurl` case-insensitively (preserving any dotted
  stem like `foo.bar.hurl` → `foo.bar.tarn.yaml`) and falls back to
  appending `.tarn.yaml` if the source has no `.hurl` suffix.
- **`runImportHurl`** internal helper extracted from the wizard.
  Accepts explicit `source`, `dest`, and `cwd` so the integration
  test can drive the spawn-and-return path without invoking the
  VS Code dialogs.
- **`TarnBackend.importHurl`** method on the backend interface and
  `TarnProcessRunner`. Shells out `tarn import-hurl <src> -o <dest>`
  and returns `{ exitCode, stdout, stderr }`.
- **Extension host API**: `testing.importHurl` forwards to
  `runImportHurl` so integration tests can import a real fixture
  without clicking through native dialogs.

### Tests

- **Unit** (`tests/unit/importHurl.test.ts`, 5 tests). Covers
  `defaultHurlDestination`: `.hurl` stripping, dotted stems,
  non-`.hurl` sources, case-insensitive suffix match, deeply
  nested paths.
- **Integration** (`tests/integration/suite/importHurl.test.ts`,
  3 tests). Creates a temp directory, writes a minimal `.hurl`
  fixture, drives the backend through `testing.importHurl`, and
  asserts the resulting `.tarn.yaml` contains the expected method,
  URL, and status assertion. Also asserts command registration
  and graceful failure when the source file is missing.

Total: 196 unit tests, 66 integration tests passing.

## 0.15.0 — Phase 4: Bench runner wizard

Fifth Phase 4 feature: `Tarn: Benchmark Step…` wraps the existing
`tarn bench` subcommand in an interactive quick-pick wizard and
renders the resulting JSON summary in a webview panel.

### Added

- **`tarn.benchStep`** command (NAZ-274) wired into the command
  palette and the `resourceLangId == tarn` editor title / context
  menu. Uses `$(dashboard)` codicon for visibility.
- **Bench wizard** (`src/commands/bench.ts`). Drives the user
  through four prompts:
  1. Quick-pick of every benchmarkable step in the active file
     (setup / test / teardown sections, labelled
     `"test_name / step_name"`). The last-selected step is hoisted
     to the top for one-click re-runs.
  2. Input box for request count (default 100, validated as a
     positive integer).
  3. Input box for concurrency (default 10, validated as a
     positive integer).
  4. Input box for optional ramp-up duration, validated against
     `^\d+(ms|s|m)?$` so only Tarn-recognized durations are
     accepted.
  The wizard persists every setting per-file in
  `context.workspaceState` under the `tarn.benchSettings:<path>`
  key, so repeat benchmarks require pressing Enter four times.
- **`TarnBackend.runBench`** and `TarnProcessRunner` implementation.
  Builds `tarn bench <file> -n N -c C --step IDX --format json
  [--ramp-up X] [--env E] [--var k=v …]`, parses stdout through a
  new `benchResultSchema` zod guard, and surfaces parse failures
  via the output channel rather than crashing the wizard.
- **`benchResultSchema`** in `src/util/schemaGuards.ts`. Mirrors
  the shape of `tarn bench --format json`: `step_name`, `method`,
  `url`, `concurrency`, `ramp_up_ms`, `total_requests`,
  `successful`, `failed`, `error_rate`, `total_duration_ms`,
  `throughput_rps`, per-phase `latency` and `timings` stats,
  `status_codes`, `errors`, `gates`, and `passed_gates`.
- **`BenchRunnerPanel`** webview (`src/views/BenchRunnerPanel.ts`).
  Singleton side-column panel with four sections:
  - *Summary grid* — throughput, totals, error rate, wall-clock,
    concurrency, and a gate outcome chip tinted green/red.
  - *Latency bars* — CSS-width bars for min / mean / p50 / p95 /
    p99 / max with a shared horizontal scale so relative spread is
    obvious at a glance. Std-dev surfaces below the bars.
  - *Status codes + errors + gates* — table and bullet list forms.
  - *Raw JSON* — pretty-printed, HTML-escaped `<pre>` block for
    copy-paste.
- **Extension host API**: `testing.showBenchResult` and
  `testing.lastBenchContext` so integration tests can drive the
  panel without spawning `tarn bench`.

### Changed

- **`CommandDeps`** now carries `benchRunnerPanel` and
  `workspaceState` so the bench command can open the panel and
  persist per-file settings through the standard command wiring.

### Deviations from the ticket

- **No chart.js dependency.** `tarn bench --format json` only emits
  aggregate percentiles (not histogram buckets), so there's no
  chartable data beyond p50/p95/p99. Four CSS-width bars render
  that data more honestly and skip a ~200 KB external asset, a
  CSP/`localResourceRoots` wiring round-trip, and a dead
  `media/webview/chart.js` path.
- **No live streaming progress.** `tarn bench` doesn't emit NDJSON
  while running; the panel opens once the final JSON summary is in
  hand. Users see a `withProgress` notification during the run.

### Tests

- **Unit** (`tests/unit/benchRunnerPanel.test.ts`, 13 tests).
  Covers `benchResultSchema` (real tarn output, minimal shape,
  rejection of malformed latency), `percentWidth` (scaling, zero/
  negative max, clamping, tiny-value floor), formatters
  (`formatNumber`, `formatPercent`, `formatDuration`), `renderBar`
  (label/value emission, HTML escaping), and `buildSettingsKey`
  (per-file namespacing).
- **Integration** (`tests/integration/suite/benchRunner.test.ts`,
  3 tests). Registers the command, primes the panel via
  `testing.showBenchResult` with a synthetic `BenchRunContext`,
  and asserts that repeat `show` calls replace the previous
  context.

Total: 191 unit tests, 63 integration tests passing.

## 0.14.0 — Phase 4: HTML report webview

Fourth Phase 4 feature: `Tarn: Open HTML Report` now generates a
Tarn HTML dashboard for the active `.tarn.yaml` and opens it in a
side-by-side webview. Clicking a failed step inside the report
jumps to the exact YAML line in the source file.

### Added

- **`ReportWebview`** (NAZ-273). New
  `src/views/ReportWebview.ts` singleton webview. Tarn's HTML report
  is self-contained (inline CSS/JS/data), so we load it into
  `webview.html` as a string — no `localResourceRoots` wiring
  required. The panel is reused across invocations via `reveal`.
- **`injectReportBridge`** pure helper that splices a small script
  into the generated HTML just before `</body>`. The injected bridge
  walks `.file-card[data-file]` and `.test-group[data-test]` nodes
  (already emitted by the Tarn HTML reporter), derives the
  `stepIndex` of each failed `.step` row, and attaches click
  handlers that post `{type: "jumpTo", file, test, stepIndex}`
  messages back to the extension host. Idempotent via a
  `BRIDGE_MARKER` comment, so re-rendering never double-injects.
- **Click-to-jump handler** in `ReportWebview.handleMessage`. Uses
  the `WorkspaceIndex` to resolve the file path (exact / suffix /
  basename match) and the YAML AST range for
  `(testName, stepIndex)`. Opens the document in `ViewColumn.One`
  so the report stays visible in the side column.
- **`TarnBackend.runHtmlReport`** and `TarnProcessRunner`
  implementation. Spawns `tarn run --format html=<tmpPath>
  --no-progress` with the current env / tag filter / selectors,
  returns `{ htmlPath, exitCode, stderr }`, and confirms the file
  landed on disk before handing the path back.
- **`tarn.openHtmlReport`** command. Resolves the active editor's
  `.tarn.yaml` file, runs the report through
  `vscode.window.withProgress`, reads the HTML, hands it to
  `ReportWebview`, and deletes the tmp file immediately (no need to
  keep it around since the content is already in the webview).
- **Editor title menu + command palette gating**: the command is
  available both in the palette and the `resourceLangId == tarn`
  context menu so users can invoke it directly from a test file.
- **Extension host API**: `testing.showReportHtml` and
  `testing.sendReportMessage` so integration tests can drive the
  panel and exercise the `jumpTo` handler without spawning tarn.

### Tests

- **Unit** (`tests/unit/reportWebview.test.ts`, 6 tests).
  Covers bridge placement before `</body>`, selector presence for
  the walker, idempotence, fallback when `</body>` is missing, and
  the swallow-list for inner expand/collapse controls.
- **Integration** (`tests/integration/suite/reportWebview.test.ts`,
  5 tests). Registers the command, drives `showReportHtml` with a
  minimal Tarn-style HTML fixture, sends synthetic `jumpTo` messages
  and asserts the active editor and cursor line, and verifies
  malformed messages are ignored.

Total: 178 unit tests, 60 integration tests passing.

## 0.13.0 — Phase 4: Fix Plan view

Third Phase 4 feature: a ranked remediation-hint tree for the most
recent failing run. Every failed step from the final JSON report
contributes one entry per `remediation_hint`, grouped by
`failure_category`, with click-to-jump that lands the cursor on the
offending step's line.

### Added

- **`FixPlanView`** (NAZ-271). New
  `src/views/FixPlanView.ts` tree data provider. Walks
  `files[].tests[].steps[]` on every completed run, filters to
  `status: "FAILED"`, and creates one leaf per `remediation_hint`.
  Failures that report a category but no hints surface as a
  placeholder entry so they still appear in the view (rather than
  silently disappearing).
- **Category grouping** with a stable sort order: `assertion_failed`,
  `capture_error`, `unresolved_template`, `parse_error`,
  `connection_error`, `timeout`, then anything unknown.
  `humanizeCategory` renders friendly labels (`"Assertion failed"`,
  `"Connection error"`, etc.) and `iconForCategory` assigns a
  distinct codicon per group.
- **Click-to-jump**. Each entry's `TreeItem.command` fires
  `tarn.jumpToFailure` with the fixture URI and a serialized range,
  opening the file and placing the cursor on the failing step's
  line. The command registration lives in `src/commands/index.ts`
  and is hidden from the command palette (it requires arguments).
- **`tarn.fixPlan`** view declared under the `tarn` activity bar
  container with the `lightbulb` codicon. Ships with a placeholder
  node ("No fix plan available…") until the first failing run
  populates it.
- **Extension host API**: `testing.loadFixPlanFromReport` and
  `testing.fixPlanSnapshot` so integration tests can drive the view
  without spawning a real run.

### Changed

- **`runHandler`** now calls `fixPlanView.loadFromReport` after the
  existing `lastRunCache` / `capturesInspector` hooks. A passing run
  empties the view; a failing run repopulates it.
- **`createTarnTestController`** signature accepts a `FixPlanView`
  parameter so the run handler can forward reports to it.

### Tests

- **Unit** (`tests/unit/fixPlanView.test.ts`, 9 tests).
  Covers `flattenReportToPlan` (empty reports, per-category
  grouping, no-hint placeholder, default category fallback,
  location attachment, category ordering) plus `categoryOrder`,
  `humanizeCategory`, and `deserializeRange`.
- **Integration** (`tests/integration/suite/fixPlan.test.ts`, 6
  tests). Primes the view via `loadFixPlanFromReport` with a
  synthetic report pointing at the existing fixture
  `tests/health.tarn.yaml`, asserts category grouping, verifies
  that the resolved step range lands on the real line from the
  WorkspaceIndex, and exercises `tarn.jumpToFailure` end-to-end by
  asserting the active editor and cursor position after the command
  runs.

Total: 172 unit tests, 55 integration tests passing.

## 0.12.0 — Phase 4: Captures Inspector view

Second Phase 4 feature: a debugger-style "locals" tree for captured
variables. Every completed run now populates a `tarn.captures` view
under the Tarn activity bar, grouped `file > test > key = value` and
scoped to the most recent run.

### Added

- **`CapturesInspector`** (NAZ-270). New
  `src/views/CapturesInspector.ts` tree data provider. Walks the
  final JSON report's `files[].tests[].captures` map after every
  run and renders one node per captured value. Scalar values (string,
  number, boolean, null) render as leaves; objects and arrays expand
  into child nodes so users can drill into nested captures without
  leaving the sidebar.
- **Redaction awareness**. Reads `redaction.captures` from
  `tarn.config.yaml` and masks matching top-level capture keys as
  `***`. A `vscode.workspace.createFileSystemWatcher` pointed at
  `tarn.config.yaml` keeps the list fresh when users edit the config.
  Redacted nodes drop their children so users cannot drill past the
  mask.
- **"Hide all capture values" toggle**. New
  `tarn.toggleHideCaptures` command, wired into the view title bar
  as an `$(eye-closed)` action. Flips a demo-mode flag that redacts
  every capture regardless of the redaction list — useful for screen
  sharing and recordings.
- **Click-to-copy**. Clicking a capture row fires
  `tarn.copyCaptureValue`, which writes the redaction-aware rendered
  value (raw strings, JSON for arrays/objects, `***` when redacted)
  to the clipboard and shows a brief status bar confirmation. The
  command never leaks a real value from a redacted node.
- **`tarn.captures` view** declared under the `tarn` activity bar
  container with the `list-tree` codicon. Ships with a placeholder
  node ("No captures from the last run…") until the first run
  completes.
- **Extension host API**: `testing.loadCapturesFromReport`,
  `testing.capturesTotalCount`, `testing.isCaptureKeyRedacted`,
  `testing.isHidingAllCaptures`, `testing.toggleHideCaptures`. Used
  by the integration suite to drive the view without spawning a
  real run.

### Changed

- **`schemaGuards.testResultSchema`** now includes an optional
  `captures: Record<string, unknown>` field, matching what Tarn
  already emits in `tests[].captures`. Previous versions silently
  stripped this field via zod's object mode.
- **`runHandler`** now calls `capturesInspector.loadFromReport`
  alongside `lastRunCache.loadFromReport` after every successful
  run, replacing the previous view state with the latest captures.
- **`createTarnTestController`** signature accepts a
  `CapturesInspector` parameter so the run handler can forward
  reports to it without additional plumbing.

### Tests

- **Unit** (`tests/unit/capturesInspector.test.ts`, 12 tests).
  Covers `extractRedactionCaptures` (missing config, malformed
  config, string filtering), `isExpandable` (arrays, objects,
  scalars, null/undefined), and `renderRawValue` (strings with
  truncation, numbers, booleans, null, arrays, objects).
- **Integration**
  (`tests/integration/suite/captures.test.ts`, 7 tests). Primes the
  view via `loadCapturesFromReport` with a synthetic report that
  includes the `auth_token` key listed in the fixture workspace's
  `tarn.config.yaml` redaction list. Asserts total count, per-key
  redaction, toggle behavior, command wiring, and empty-report
  handling.
- The fixture `tarn.config.yaml` gained `redaction.captures:
  [auth_token]` to exercise the redaction code path end-to-end.

Total: 163 unit tests, 49 integration tests passing.

## 0.11.0 — Phase 4: Request/Response Inspector webview

First Phase 4 feature: rich step-detail inspector that beats the
markdown-in-a-popover `TestMessage` the extension shipped in Phase 1.
When a step fails, users can now open a dedicated webview panel with
Request / Response / Assertions tabs, pretty-printed headers, JSON
body pretty-printing, a 10 KB truncation guard with an "Open full
in new editor" action, and a per-assertion pass/fail breakdown with
diff rendering.

### Added

- **`RequestResponsePanel`** (NAZ-272). Singleton webview panel that
  opens beside the editor, reuses the existing panel on repeat
  invocations, and preserves context when hidden. No framework —
  the HTML is rendered from a plain template string with CSP-locked
  scripts and VS Code theme CSS variables.
- **Tabbed UI**:
  - *Request* — method + URL, headers table, body with JSON
    pretty-printing.
  - *Response* — status, headers table, body. When the step passed,
    Tarn only records the status code (bodies are failure-only), so
    the panel shows a helpful "Response bodies are only included
    for failed steps" empty state.
  - *Assertions* — one row per assertion with expected, actual,
    message, and colored diff (if present). Prefers the full
    `details` array when available, falls back to `failures` only.
- **Body truncation** at 10 KB with a `data-open-full` button that
  posts a message to the extension host, which opens the full body
  in a new editor with auto-detected language (JSON / XML / plain).
- **`LastRunCache`** (`src/testing/LastRunCache.ts`). Tiny
  in-memory map keyed by `file::test::index` that stores every
  step result from the most recent run. Populated by `runHandler`
  after `applyReport` so the panel can look up any step on demand
  without re-running tests.
- **`tarn.showStepDetails`** command (hidden from the command
  palette because it requires an argument). Accepts either a
  `StepKey` `{ file, test, stepIndex }` or a wrapper
  `{ encodedKey }` string, looks the step up in the cache, and
  opens the panel.
- **Extension API test hooks**:
  `testing.lastRunCacheSize()`, `testing.loadLastRunFromReport()`,
  and `testing.showStepDetails(key)`. The second lets integration
  tests prime the cache from a synthetic report without running
  real tests; the third drives the panel deterministically and
  returns a boolean so tests can assert hit/miss.

### Scope note

v1 ships the panel plus the command. Auto-triggering on Test
Explorer selection, CodeLens buttons on failed steps, and a
`Copy as curl` action inside the panel are intentional follow-ups
(NAZ-272 comments in Linear) — the infrastructure is now in place
and those integrations are small additions later.

### Tests

- **19 new unit tests** across two files:
  - `lastRunCache.test.ts` (7 tests) — empty state, indexing
    setup/test/teardown, setup lookup via `test: "setup"`, test
    lookup by name + index, replace-on-reload semantics, `clear()`,
    and `encode`/`decode` round-trip plus malformed handling.
  - `requestResponsePanel.test.ts` (12 tests) — `stringifyBody`
    for strings/objects/arrays/circular refs, `detectLanguage`
    for JSON / XML / plaintext, and `truncateBody` above/below/at
    the 10 KB threshold.
- **5 new integration tests** in `stepDetails.test.ts` against a
  real VS Code instance: command registration, cache sizing after
  loading a synthetic report, panel-open success for a known key,
  panel-open miss for an unknown key, and a full
  `vscode.commands.executeCommand` round-trip.
- Extension unit tests: **131 → 151 passing**.
- Extension integration tests: **37 → 42 passing**.

## 0.10.0 — Phase 3 complete: YAML grammar injection for interpolation

Seventh and final Phase 3 feature: the TextMate grammar shipped in
`syntaxes/tarn.tmLanguage.json` now assigns distinct scope names to
every part of a `{{ ... }}` interpolation so themes and the language
providers can discriminate between env keys, capture names, and
built-in functions.

### Changed

- **`meta.template.tarn`** is the wrapper scope on the entire
  `{{ ... }}` range (previously `meta.interpolation.tarn`).
- **`keyword.control.template.begin.tarn`** and
  **`keyword.control.template.end.tarn`** scopes are added to the
  `{{` / `}}` delimiters alongside the standard
  `punctuation.definition.template.begin/end.tarn` names so both
  theme families highlight the braces.
- **`variable.other.env.tarn`** now scopes `env.KEY` references
  (was grouped with captures under `variable.other.readwrite.tarn`).
  The rule splits into two capture groups — the `env` namespace
  token and the key identifier — so themes that distinguish
  namespace from identifier render correctly.
- **`variable.other.capture.tarn`** does the same for
  `capture.NAME` references.
- **`support.function.builtin.tarn`** (unchanged) matches every
  Tarn runtime built-in: `$uuid`, `$timestamp`, `$now_iso`,
  `$random_hex`, `$random_int`.
- `support.function.transform.tarn` and
  `keyword.operator.pipeline.tarn` are preserved from the
  previous grammar for the transform-lite pipeline.

### Tests

- **10 new unit tests** in `grammar.test.ts` load the grammar JSON,
  walk every pattern + capture entry, flatten space-separated scope
  names, and assert the expected scopes are declared. The tests
  also exercise the actual regexes against realistic tokens
  (`env.base_url`, `capture.auth_token`, every `$builtin` name) so
  regressions on the underlying matching rules surface immediately.
- Extension unit tests: **121 → 131 passing**.
- Integration tests unchanged: 37/37 passing.

### Manual verification

The grammar's theme rendering is intentionally not covered by
automated tests (VS Code does not expose tokenization via a public
API). Visual verification on the Default Dark+ and Default Light+
themes stays the responsibility of the release smoke-test.

### Phase 3 status

Phase 3 is now complete (NAZ-263 through NAZ-269). The extension
has a full authoring surface: diagnostics on save, environments
tree view, completion, hover, go-to-definition / references /
rename, `tarn fmt` format provider, and distinct grammar scopes.
Next up: Phase 4 rich-UX features (NAZ-270 through NAZ-278), with
NAZ-272 Request/Response Inspector as the highest priority.

## 0.9.0 — Phase 3: `tarn fmt` format provider

Sixth Phase 3 feature: VS Code's Format Document action now
routes `.tarn.yaml` files through `tarn fmt`, so Shift+Alt+F
(and `editor.formatOnSave: true`) normalizes Tarn YAML to the
canonical form without leaving the editor.

### Added

- **`TarnFormatProvider`** (NAZ-268). Implements
  `vscode.DocumentFormattingEditProvider` on the `tarn` language
  and returns a single full-document `TextEdit` so undo is one
  step. Parse errors in the source file are surfaced via the
  output channel plus a one-shot warning notification, and the
  buffer is left untouched — formatting an invalid file never
  corrupts it.
- **`backend.formatDocument(content, cwd, token)`**. The Tarn CLI
  has no `--stdout` or stdin mode, so the backend writes the
  document content to a tmp `.tarn.yaml` file, runs
  `tarn fmt <tmp>`, reads the formatted result back, and cleans
  up the tmp file. Errors and cancellation are propagated so the
  provider can decide whether to emit any edits at all.
- **`TarnExtensionApi.testing.formatDocument(uri)`** test hook.
  Calls the provider directly so integration tests do not have
  to fight VS Code's formatter selection when another extension
  (e.g., `redhat.vscode-yaml`) also registers a document
  formatter for the same file.

### Behavior

- Already-canonical files produce an empty edit list so
  `formatOnSave` is a no-op and the document is never dirty-marked
  unnecessarily.
- Files with YAML parse errors log to the Tarn output channel and
  return no edits.
- Cancellation honors the provided `CancellationToken` and leaves
  the document untouched.

### Tests

- **3 new integration tests** in `format.test.ts` against a real
  `tarn` binary: a messy fixture with non-canonical indentation
  and quoting produces exactly one full-document edit with
  canonical output; an already-canonical fixture produces zero
  edits; a fixture with an unterminated quoted string produces
  zero edits and leaves the on-disk content untouched.
- Extension integration tests: **34 → 37 passing**.
- Unit tests unchanged: 121/121 passing.

## 0.8.0 — Phase 3: Symbol navigation (definition, references, rename)

Fifth Phase 3 feature: Tarn interpolation tokens now participate in
VS Code's standard symbol navigation surface. Right-click a capture
or an env key inside `{{ ... }}` and the usual "Go to Definition",
"Find All References", and "Rename Symbol" actions Just Work.

### Added

- **`TarnDefinitionProvider`** (NAZ-267). Jumps from a
  `{{ capture.x }}` reference to the step that declares
  `capture: { x: ... }`, respecting the capture's file scope.
  Jumps from `{{ env.key }}` to the file(s) listed by the
  `EnvironmentsView` cache, with an in-file line lookup that
  grep-matches `^<key>:` in each source file so the cursor lands
  on the actual declaration line instead of line 0.
- **`TarnReferencesProvider`**. Returns every
  `{{ capture.NAME }}` usage in the current file, plus the
  declaration if `includeDeclaration` is true. Works equally well
  when the cursor is on a reference or on the declaration key
  inside a `capture:` block. Env references are explicitly
  out-of-scope for v1 (they span multiple files); the provider
  returns an empty list rather than a misleading partial answer.
- **`TarnRenameProvider`**. Renames a capture across its
  declaration and every in-file reference as a single
  WorkspaceEdit. `prepareRename` narrows the edit range to just
  the identifier (so renaming from a reference doesn't accidentally
  rewrite the `{{ }}` punctuation). Rejects renames when:
  - the cursor is on an env token (edit the source file directly);
  - the file has YAML parse errors (fix first);
  - the capture is not declared in the current file (probably came
    from an `include:` directive — edit the included file);
  - the new name isn't a valid identifier
    (`^[A-Za-z_][A-Za-z0-9_]*$`).
- **`buildCaptureIndex(source)`** helper in
  `src/language/completion/captures.ts`. Walks the YAML CST once
  and returns a searchable index of every capture declaration with
  its phase, test name, step info, and byte-offset key range.
  Shared by definition / references / rename so they stay
  consistent and cheap.
- **`findCaptureReferences(source, nameFilter?)`** helper.
  Regex-scans the document for `{{ capture.NAME }}` tokens and
  returns the identifier-only byte ranges (not the `{{`/`}}`
  punctuation) so rename edits are precise.
- **`cursorSymbol(document, position)`** helper in
  `src/language/SymbolProviders.ts`. Wraps `findHoverToken` and
  falls back to the capture index to detect clicks on declaration
  keys inside `capture:` blocks. Returns a tagged union
  (`env` / `capture-ref` / `capture-decl`) the three providers
  consume.

### Tests

- **10 new unit tests** in `captureSymbols.test.ts` exercise
  `buildCaptureIndex` and `findCaptureReferences` against a
  multi-test fixture: capture collection from setup / tests /
  teardown, per-declaration phase and test-name recording,
  `findDeclarationAt` offset hit-testing, `findByName` lookup,
  reference scanning over mixed env/capture/builtin interpolations,
  name filtering, whitespace tolerance inside `{{ ... }}`, and
  empty-source handling.
- **6 new integration tests** in `symbols.test.ts` drive the
  providers through VS Code's public commands
  (`vscode.executeDefinitionProvider`,
  `vscode.executeReferenceProvider`,
  `vscode.executeDocumentRenameProvider`, `vscode.prepareRename`)
  against a real extension host: go-to-definition on a capture
  reference lands on the declaring key line, go-to-definition on
  `{{ env.base_url }}` produces locations in at least one env
  source file, find-all-references on a capture returns all 3
  expected locations (2 refs + 1 decl), renaming a capture
  replaces every occurrence via a single WorkspaceEdit, invalid
  new names are rejected, and `prepareRename` on a reference
  returns only the identifier range (not the whole `{{ }}` token).
- Extension unit tests: **111 → 121 passing**.
- Extension integration tests: **28 → 34 passing**.

## 0.7.0 — Phase 3: Hover provider for interpolation

Fourth Phase 3 feature: hovering over any `{{ env.x }}`,
`{{ capture.y }}`, or `{{ $builtin }}` token now shows a
context-aware markdown tooltip with resolved values, source files,
capturing step, or built-in signature + docs.

### Added

- **`TarnHoverProvider`** (NAZ-266). Registered on the `tarn`
  language and reuses the same env cache, capture walker, and
  builtin list as `TarnCompletionProvider`.
- **Hover on `{{ env.KEY }}`** lists every configured environment
  that declares the key with its source file and (already-redacted)
  value. Missing keys get a "not declared in any configured
  environment — will resolve at runtime" hint so the user isn't
  misled into thinking the key is broken.
- **Hover on `{{ capture.NAME }}`** shows the step that captured
  it, whether the step lives in setup or a named test, the step
  index, and the phase. Missing names get a "not in scope" hint
  with a reminder about the capture scoping rules.
- **Hover on `{{ $builtin }}`** shows the signature and doc.
  Parameterized builtins like `$random_hex(n)` are matched by
  stripping the arg list before the lookup.
- **Hover on the bare `{{ }}`** (empty interpolation or just the
  keyword `env` / `capture` / `$`) shows a quick help card
  describing every available form.
- **`findHoverToken(line, column)`** — a new token finder in
  `src/language/completion/hoverToken.ts` that does a single-pass
  scan for the enclosing `{{ ... }}` pair, classifies its contents,
  and returns the token range for VS Code to highlight. Handles
  multi-interpolation lines, cursor-on-boundary edge cases, and
  unclosed expressions.

### Out of scope (follow-up)

- **Dry-run URL preview** on hover over a `url:` field (spawning
  `tarn run --dry-run --select ...` with a per-hover cache). Still
  valuable but too much machinery for this ticket. Tracked as a
  follow-up to NAZ-266 in `docs/VSCODE_EXTENSION.md`.

### Tests

- **13 new unit tests** in `hoverToken.test.ts` exercising the
  finder against every expected shape: cursor outside/inside/on
  boundary of a `{{ ... }}`, env with cursor on key vs on `env`
  itself, capture, `$uuid` bare and `$random_hex(n)` with args,
  empty interpolation, multi-interpolation lines, unknown
  expressions, and unclosed `{{`.
- **6 new integration tests** in `hover.test.ts` against a real
  VS Code instance: env hover lists both staging and production
  environments and their values, capture hover shows the
  capturing step name, missing capture shows a "not in scope"
  warning, `$uuid` hover shows the doc text, `$random_hex(8)`
  hover shows the `$random_hex(n)` signature (not the literal
  argument), and hovering outside any interpolation never fires
  our provider (verified by checking for stable substrings our
  provider emits).
- Extension unit tests: **98 → 111 passing**.
- Extension integration tests: **22 → 28 passing**.

### Dependencies

- NAZ-264 Environments tree view (env cache) — shipped in `990182e`.
- NAZ-265 Completion provider (shares `collectVisibleCaptures` and
  `BUILTIN_FUNCTIONS`) — shipped in `946bec9`.

## 0.6.0 — Phase 3: Interpolation completion provider

Third Phase 3 feature: VS Code now offers IntelliSense for Tarn's
`{{ ... }}` template expressions. Type `{{ env.`, `{{ capture.`, or
`{{ $` inside any string in a `.tarn.yaml` and the completion widget
fills in the matching names from the merged env resolution chain,
visible captures, or the built-in function list.

### Added

- **`TarnCompletionProvider`** (NAZ-265). Registered on the `tarn`
  language with trigger characters `{`, `.`, `$`, and space. The
  provider itself is string-based (grammar scope detection is a
  separate ticket, NAZ-269); on each invocation it inspects the line
  prefix to decide whether the cursor sits inside an open `{{ ... }}`
  expression and what kind.
- **Env key completions** come from the `EnvironmentsView` cache
  (NAZ-264), so the first Phase 3 feature is now delivering value
  beyond its tree. Every env key is labeled with the list of
  environments that declare it.
- **Capture completions** use a new `collectVisibleCaptures` helper
  that walks the YAML CST and returns only captures visible at the
  cursor: setup captures are always in scope, test captures only when
  the cursor is in that same test and only from strictly earlier
  steps, teardown sees everything. Captures from other tests are
  never offered.
- **Built-in function completions** for the full Tarn interpolation
  runtime list: `$uuid`, `$timestamp`, `$now_iso`, `$random_hex(n)`,
  `$random_int(min, max)`. Parameterized builtins insert a snippet
  with argument placeholders.
- **Top-level completions** offer `env`, `capture`, and `$uuid` when
  the user just typed `{{ ` with nothing after it. Picking `env` or
  `capture` re-triggers the suggest widget automatically.

### Tests

- **22 new unit tests** across two new files:
  - `interpolationContext.test.ts` (13 tests) exercises
    `detectInterpolationContext` against every expected shape: empty
    interpolation, env context with and without a prefix, capture
    context, builtin context, closed interpolations, nested braces,
    and unknown prefixes.
  - `visibleCaptures.test.ts` (9 tests) covers the capture visibility
    rules end-to-end: setup captures from the outside, scope
    narrowing inside earlier vs later test steps, sibling test
    isolation, teardown seeing everything, and graceful handling of
    malformed YAML.
- **5 new integration tests** in `completion.test.ts` against a real
  VS Code instance: env completions come from `tarn.config.yaml`,
  capture completions respect step ordering, same-step and sibling-
  test captures are excluded, builtin completions contain every name
  from the Tarn runtime, and typing outside any interpolation never
  fires our provider (verified by filtering results by our
  provider's stable `detail` prefix to ignore the built-in word
  completer's noise).
- Extension unit tests: **76 → 98 passing**.
- Extension integration tests: **17 → 22 passing**.

### Dependencies

- Tarn T56 (`tarn env --json`), shipped in `cfffb69` — provides the
  env data the completion reads from.
- NAZ-264 Environments tree view, shipped in `990182e` — owns the
  env cache that NAZ-265 reuses.

## 0.5.0 — Phase 3: Environments tree view

Second Phase 3 feature: a first-class Environments treeview under the
Tarn activity bar container, backed by `tarn env --json` (Tarn T56).
Replaces the glob-based discovery the extension used in Phase 1, and
becomes the shared env cache that upcoming authoring features
(completion, hover, go-to-definition) will read from.

### Added

- **`EnvironmentsView`** (NAZ-264). Implements `TreeDataProvider`,
  loads from `backend.envStructured` on activation and on
  `tarn.config.yaml` file changes (via `FileSystemWatcher`). Caches
  the last successful report so repeated queries are free.
- **Tree rendering**: one node per environment showing the active
  check mark, name, source file relative path, and inline var count.
  Tooltip lists every var with its redacted value. A click sets the
  environment as active and toggles off on a second click.
- **Per-node context menu**: `Open source file`, `Copy as --env flag`.
  Inline action buttons on each tree item.
- **`Tarn: Reload Environments`** command (also in the view title
  bar) to force a fresh `tarn env --json` spawn.
- **`Tarn: Set Active (from tree)`**, **`Tarn: Open Environment Source
  File`**, **`Tarn: Copy as --env Flag`** commands — hidden from the
  command palette (they require a context argument).
- **`backend.envStructured(cwd, token)`** on the `TarnBackend`
  interface. Returns `EnvReport | undefined`, parsing the output with
  a new `parseEnvReport` zod schema.
- **`envReportSchema`**, `EnvReport`, `EnvEntry` exported from
  `schemaGuards.ts`.
- **`TarnExtensionApi.testing.reloadEnvironments`**,
  **`listEnvironments`**, **`getActiveEnvironment`** test hooks so
  integration tests can drive the view without TreeDataProvider
  plumbing.

### Changed

- **`Tarn: Select Environment…`** command now reads from the view's
  cache instead of globbing `tarn.env.*.yaml` from the workspace
  root. Picks include source file and var count in the quick-pick
  item description.
- Removed the dead `collectEnvironments` helper in `commands/index.ts`
  that did the old glob-based discovery.
- Integration test fixture workspace (`tests/integration/fixtures/workspace/`)
  now ships a real `tarn.config.yaml` declaring staging + production
  environments plus a redaction rule for `api_token`, so every
  environments-related test runs against a realistic project layout.

### Tests

- Extension integration tests: **12 → 17 passing**. New
  `environments.test.ts` covers: loading two environments in
  alphabetical order, redaction of `api_token` via the project's
  `redaction.env` list, command registration for all four new
  commands, toggling the active environment via
  `tarn.setEnvironmentFromTree`, and clipboard output of
  `tarn.copyEnvironmentAsFlag`.
- Extension unit tests unchanged: 76/76 passing.

### Dependencies

- Tarn T56 (`tarn env --json` schema polish + redaction), shipped
  in `cfffb69`.

## 0.4.0 — Phase 3 kick-off: diagnostics on save

First Phase 3 feature: the extension now surfaces Tarn parse errors as
inline diagnostics on every save, powered by `tarn validate --format json`
(Tarn T52). This turns VS Code into a real linter for `.tarn.yaml` files.

### Added

- **`TarnDiagnosticsProvider`** (NAZ-263). On every save of a `.tarn.yaml`,
  spawns `tarn validate --format json <file>` via the backend, parses the
  structured output with a new `parseValidateReport` zod schema, and
  publishes `vscode.Diagnostic` entries into a dedicated
  `DiagnosticCollection('tarn')`.
  - YAML syntax errors anchor on the exact line/column reported by
    `serde_yaml` (converted from 1-based to 0-based and clamped to the
    document bounds).
  - Parser semantic errors without location fall back to a line-0
    marker so the Problems panel still shows them.
  - Diagnostics clear the moment the file becomes valid again.
  - Closing a document clears its diagnostics.
  - In-flight validations for a document are cancelled and replaced when
    a new save races ahead of the previous one — no stale diagnostics.
- **`tarn.validateOnSave`** setting (default `true`). Flip it to `false`
  to disable the behavior without uninstalling.
- **`backend.validateStructured`** method on the `TarnBackend` interface.
  Returns `ValidateReport | undefined` so callers don't have to parse
  stdout themselves. `TarnProcessRunner` implements it by invoking
  `tarn validate --format json`; returns `undefined` on stale Tarn
  versions so the fallback path is graceful.
- **`TarnExtensionApi.testing.validateDocument(uri)`** — test-only hook
  that awaits a validate run synchronously so integration tests can
  assert on diagnostics deterministically without polling.

### Tests

- Extension integration tests: **7 → 12 passing** against a real `tarn`
  binary. New `diagnostics.test.ts` covers valid file (no diagnostics),
  YAML syntax error (line/column preserved, source set to `tarn`),
  unknown-field semantic error, fix cycle (diagnostic clears when
  content becomes valid), and the `tarn.validateOnSave: false` toggle.
- Extension unit tests: still 76/76 passing (no regressions).

### Dependencies

- Tarn T52 (`tarn validate --format json`), shipped in `cfffb69`.

## 0.3.0 — Phase 2: live streaming and selective execution

Cashes in the Tarn-side `T51` (`--select`) and `T53` (NDJSON reporter) so
the Test Explorer updates live as each step finishes and editor-driven
"run test at cursor" / "rerun failed" workflows use precise selectors.

### Added

- **Live Test Explorer updates via NDJSON**. Runs now spawn `tarn run
  --ndjson --format json=<tmp>` and parse `step_finished` /
  `test_finished` / `file_finished` / `done` events from stdout as they
  arrive. Passing steps turn green the moment they complete instead of
  waiting for the final JSON report. Failures still use the final JSON
  report so the `TestMessage` keeps its rich expected/actual/diff, full
  request, full response, and remediation hints.
- **Selective execution**. When a user clicks Run on a specific test or
  step (from Test Explorer, CodeLens, or `Tarn: Run Current File`), the
  backend derives `--select FILE::TEST[::STEP]` selectors from the
  `TestRunRequest.include` items and forwards them to the CLI. Running
  a single file still uses positional args; running a test adds
  `FILE::TEST`; running a single step adds `FILE::TEST::index`.
- **`Tarn: Run Failed Tests` command**. Tracks the set of failed item IDs
  from the last completed run and reruns only those via selectors.
- **Per-item metadata map** (`discovery.ts`). Every discovered
  `TestItem` carries its structured kind (file / test / step), uri,
  test name, and step index via a `WeakMap`, so the run handler never
  has to parse item IDs.
- Backend interface: `RunOptions` gains `selectors`, `streamNdjson`,
  and `onEvent`. New `NdjsonEvent` union type for consumers that want
  to observe the raw stream.
- Extension API: `TarnExtensionApi` now exposes `testing.backend` so
  integration tests can exercise the backend directly.

### Changed

- `runHandler.ts` rewritten to plan selectors up front, stream events
  via `onEvent`, then apply the final JSON report with rich failure
  `TestMessage`s. Files resolved from NDJSON events using a suffix
  match against `parsedByPath`.
- `TarnProcessRunner` splits the run path: NDJSON mode uses a
  `readline` interface on stdout and writes the final report to a
  tmp file, while the legacy path still supports polling consumers.
  The tmp file is cleaned up on the way out.
- `Tarn: Rerun Last Run` unchanged in behavior but now also remembers
  whether the run was dry.

### Tests

- Extension unit tests: still 76/76 passing (no regressions).
- Extension integration tests: 4 → **7 passing** against a real `tarn`
  binary. New backend suite covers NDJSON streaming end-to-end, single
  test selection, and single step selection. The integration runner
  now writes `tarn.binaryPath` into the fixture workspace pointing at
  `target/debug/tarn` so the test always exercises the source-built
  CLI rather than whatever is on `PATH`.

## 0.2.0 — Phase 1 foundation

Adds extension host integration on top of the existing declarative package.

### Added

- Test Explorer integration via the VS Code Testing API.
  - Hierarchical discovery: workspace → file → test → step.
  - `Run` and `Dry Run` test run profiles.
  - Cancellation via SIGINT with SIGKILL fallback after 2 s, plus a configurable watchdog timeout.
  - Result mapping from `tarn run --format json --json-mode verbose` into `TestRun.passed / failed`.
  - Rich failure `TestMessage` with expected/actual, unified diff, request, response, remediation hints, and failure category/error code.
- CodeLens above each test and step: `Run`, `Dry Run`, `Run step`.
- Document symbol provider: outline view of tests, steps, setup, teardown with scope-aware hierarchy.
- Tarn activity bar container with a **Run History** tree view persisting the last 20 runs (status, env, tags, duration, files).
- Status bar entries: active environment (click to pick) and last run summary (click to open output).
- Commands:
  - `Tarn: Run All Tests`
  - `Tarn: Run Current File`
  - `Tarn: Dry Run Current File`
  - `Tarn: Validate Current File`
  - `Tarn: Rerun Last Run`
  - `Tarn: Select Environment…`
  - `Tarn: Set Tag Filter…`, `Tarn: Clear Tag Filter`
  - `Tarn: Export Current File as curl` (all or failed-only via `--format curl-all` / `--format curl`)
  - `Tarn: Clear Run History`
  - `Tarn: Open Getting Started`
  - `Tarn: Show Output`
  - `Tarn: Install / Update Tarn`
- **Getting Started walkthrough** with five steps: install, open example, run, select env, inspect failure.
- Workspace indexing with on-change reparsing via `FileSystemWatcher`, idempotent initialization.
- YAML AST with range maps for tests, steps, setup, and teardown — foundation for CodeLens, document symbols, result anchoring, and future authoring features.
- Settings namespace `tarn.*` with 13 keys covering binary path, discovery globs, parallelism, JSON mode, timeouts, redaction passthrough, and UI toggles.
- Workspace trust gating: untrusted workspaces keep grammar, snippets, and schema wiring but do not spawn the Tarn binary.
- Shell-free process spawning via Node's built-in `child_process.spawn` with an argv array, plus a log formatter for copyable command lines in the output channel.
- Zod-validated parsing of Tarn JSON reports.

### Tests

- **Unit tests** (vitest, 76 tests across 5 files):
  - `shellEscape` — safe identifier passthrough, space/quote/dollar/backtick escaping.
  - `schemaGuards` — passing report, failing report with full rich detail, enum rejection, missing-field rejection.
  - `YamlAst` — file name, tests, steps, setup, teardown, flat `steps`, malformed input.
  - `YamlAstSweep` — parses every `.tarn.yaml` fixture in `examples/` and verifies non-empty names plus non-negative ranges (55 dynamic tests).
  - `ResultMapper.buildFailureMessages` — rich assertion failure, multi-assertion, generic fallback, and every `failure_category` enum value.
- **Integration tests** (`@vscode/test-electron` + mocha): smoke suite covering activation, test controller registration, discovery of a fixture workspace, document symbols, and command registration. Runs via `npm run test:integration`.

### CI

- GitHub Actions workflow `.github/workflows/vscode-extension.yml` running typecheck, unit tests, and build across `ubuntu-latest`, `macos-latest`, `windows-latest`; Ubuntu job also packages a VSIX artifact.

### Preserved from 0.1.0

- Language id `tarn` for `*.tarn.yaml` / `*.tarn.yml`.
- Grammar at `syntaxes/tarn.tmLanguage.json`.
- Snippets (`tarn-test`, `tarn-step`, `tarn-capture`, `tarn-poll`, `tarn-form`, `tarn-graphql`, `tarn-multipart`, `tarn-lifecycle`, `tarn-include`).
- Schema wiring for test files and report files via `redhat.vscode-yaml`.

### Known gaps (tracked in `docs/VSCODE_EXTENSION.md` and `T51`–`T57`)

- Streaming progress requires Tarn NDJSON reporter (`T53`); Phase 1 uses the final JSON report.
- Run-at-cursor and run-failed-only require selective execution (`T51`).
- Structured validation diagnostics require `tarn validate --format json` (`T52`).
- Runtime result ranges are AST-inferred until Tarn exposes location metadata (`T55`).

## 0.1.0

Initial declarative package: language id, grammar, snippets, schema wiring.
