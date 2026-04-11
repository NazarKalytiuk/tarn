# VS Code Extension → tarn-lsp Migration (Phase V)

This document records the migration strategy that Phase V of the
VS Code extension roadmap (Epic NAZ-308) commits to for moving
the extension's in-process TypeScript language-feature providers
onto a thin [`vscode-languageclient`](https://github.com/microsoft/vscode-languageserver-node)
front-end that talks to the Rust [`tarn-lsp`](../../../tarn-lsp)
crate over stdio.

It is the source of truth for:

1. Which migration shape we picked and why.
2. The order in which Phase V2 migrates individual features.
3. The rollback plan if any V2 feature regresses on an
   integration test.
4. The version-bump policy that governs Phase V2 releases.

The scaffold itself is landed by ticket **NAZ-309** (Phase V1):
`vscode-languageclient@9.0.1` is a runtime dependency, the
experimental `tarn.experimentalLspClient` setting is wired, and
the LSP client boots side-by-side with the direct providers
behind that flag. No language feature has moved onto the LSP
path yet; that is the Phase V2 work this document plans.

## Status

| Phase | Ticket | Outcome |
| --- | --- | --- |
| V1 | NAZ-309 | Scaffold + docs (THIS ticket) |
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

## 1. Strategy decision

We evaluated three shapes for the migration. The summary up
front: **we pick dual-host for Phase V2, convert one feature at
a time per ticket, and delete the direct providers in a final
Phase V3 cleanup once every feature has shipped and soaked.**

### Option A — Full migration in one ticket

Rip out every direct provider in a single commit and replace
them with LSP-driven equivalents.

Pros:

- Minimal transitional complexity. There is exactly one
  producer per language feature, so no coordination glue
  between the two stacks.
- Smallest bundle: once `src/language/*` is deleted, the
  extension carries only the `vscode-languageclient`
  dependency plus the thin client wrapper.
- Forces the `tarn-lsp` surface to match the direct
  providers exactly before the migration can proceed — a
  useful stress test.

Cons:

- **Any LSP regression blocks every language feature at
  once.** Diagnostics, hover, completion, formatting, and
  go-to-definition all move together; if the server misbehaves
  on a single feature (say, hover formatting differs from the
  in-process provider), all nine features are wedged until
  the one bug is fixed.
- The scope of an integration-test diff is huge. A single
  PR would touch every language-related test in
  `tests/integration/suite/*.test.ts`, which makes review
  extremely hard and makes the blast radius of a bad merge
  impossible to bound.
- Rollback is a revert of a PR that may have taken weeks to
  review. The further a migration sits in `main` before it
  surfaces a regression, the more expensive it is to
  unwind.
- No way to dog-food the LSP path for a single feature
  before committing to the full flip. Any bug in `tarn-lsp`
  that only manifests under a specific document-selector or
  filesystem watcher pattern shows up first in production.

### Option B — Selective migration (some features LSP, some direct, forever)

Move the features that are easier to migrate (e.g. diagnostics,
document symbols) to the LSP path and leave the rest on the
direct providers permanently. This was the pattern some of
Microsoft's first-party extensions adopted briefly before
committing to language-client migration.

Pros:

- Upfront cost is lower: only migrate features that benefit
  clearly.
- No risk for features that stay on the direct path.

Cons:

- **Maintaining two codebases in parallel forever is the
  worst possible steady state.** Every bug fix to Tarn's
  language semantics now has to land in the Rust LSP AND
  in the TypeScript direct provider, or the two drift.
- The extension loses the benefit of a single canonical
  language surface for other editors (Neovim, Helix, Emacs,
  Claude Code) because the feature set they see via
  `tarn-lsp` is a strict subset of the VS Code experience.
  Selective migration directly undermines the reason we
  built `tarn-lsp` in the first place.
- No path to Phase V3 cleanup. "Finish the migration later"
  becomes "never".

### Option C — Dual-host migration (CHOSEN)

Run both stacks side by side. The LSP client boots behind the
`tarn.experimentalLspClient` flag. Each Phase V2 ticket
migrates **exactly one feature**: it deletes the direct
provider registration for that feature while flipping the LSP
client on for the integration tests that exercise that
feature. The two stacks coexist for the duration of Phase V2
and only the fully-validated LSP path is kept when Phase V3
lands.

Pros:

- **Lowest-regression-risk path.** The direct providers keep
  working for every feature that has not yet been migrated,
  so a Phase V2 ticket that regresses only blocks its own
  feature. Diagnostics is validated in isolation before
  hover moves, hover before completion, and so on.
- **Per-feature rollback is a single revert.** Each V2
  ticket is one commit touching one feature + one or two
  tests; reverting it restores the direct provider for that
  feature with no cascade.
- **Gradual soak time.** Users who enable the experimental
  flag get the LSP path early for feedback while the
  default experience stays on direct providers. A concern
  that surfaces only in real usage (e.g. an edge case in
  `serde_yaml` recovery that the integration tests miss)
  shows up while the direct fallback is still one setting
  flip away.
- **No forever cost.** Phase V3 is trivial: delete the
  direct-provider registrations and the experimental flag.
  The dual-host state is explicitly temporary.
- Coexistence is a first-class VS Code concept. Both the
  direct `vscode.languages.register*Provider` APIs and the
  `vscode-languageclient`-registered providers feed into
  the same provider lists, VS Code queries all providers
  for a given request, and results are merged. There is no
  exclusivity constraint for a given language id. We
  verified this empirically by running the scaffold with
  the flag enabled — both sets of providers live, and the
  integration suite still passes.

Cons:

- Transitional complexity: two producers for a feature mean
  potentially duplicate results. We address this by
  requiring each V2 ticket to **unregister the direct
  provider for the feature it is migrating in the same
  commit** — not just "add LSP, hope direct is skipped".
  The commit is tightly bounded: one `Disposable.dispose()`
  call + one new `documentSelector` assignment + one test
  update.
- Small bundle overhead: `vscode-languageclient` is added
  as a dependency even though nothing uses it until V2
  starts. The overhead is ~2 KB in `out/extension.js`
  (the language-client package itself is externalized via
  esbuild and loaded from `node_modules` at runtime, so
  it does not inflate the parsed-JS footprint on every
  activation). See NAZ-309 commit body for exact numbers.

### Why dual-host is load-bearing for this migration

The thing that makes dual-host the right choice specifically
for Tarn is that `tarn-lsp` is a comparatively new component
compared to the direct providers. The direct providers have
six months of production soak time; `tarn-lsp` has a few weeks.
Running them side by side for the duration of Phase V2 means
every single V2 ticket is its own controlled experiment with
a safe fallback, and the decision to adopt `tarn-lsp`
permanently is made feature-by-feature on the basis of
integration-test evidence, not trust.

## 2. Phase V2 migration order

The order is chosen so the lowest-risk, highest-confidence
features migrate first, with each subsequent ticket adding
surface area that benefits from lessons learned in the
previous one.

| # | Ticket | Feature | Rationale for position in the sequence |
| --- | --- | --- | --- |
| V2.1 | TBD | **Diagnostics** | Safest. `publishDiagnostics` is the most mature LSP message; `tarn-lsp` already emits diagnostics identical to the direct provider's output (NAZ-294 wired this through `tarn validate`). Zero user-facing surface for the server to get wrong beyond text, severity, and range. |
| V2.2 | TBD | **Document symbols** | Read-only, feeds the outline view. No user input, no latency concerns, failure modes are purely cosmetic ("outline is empty"). Good canary for the JSON-RPC round-trip with a non-diagnostic message. |
| V2.3 | TBD | **Code lens** | Still read-only, still driven by the parse tree, but introduces command bindings (`tarn.runTestFromCodeLens` / `tarn.dryRunTestFromCodeLens`). Exercises the `commands` negotiation in the client options. |
| V2.4 | TBD | **Hover** | First user-interactive feature. Latency-sensitive but the server's hover surface (NAZ-307: env/capture/builtin/JSONPath tokens) is already exercised by `cargo test` and matches the direct provider's MarkdownString output. |
| V2.5 | TBD | **Completion** | Completion is more latency-sensitive than hover and cares about incremental document sync. Migrated after hover because by V2.5 the client→server sync loop has proven itself under two read-oriented features. |
| V2.6 | TBD | **Formatting** | Touches document contents. Uses `tarn fmt` through the server (NAZ-302). Migrated here because formatting failures are loud (nothing happens or a parse error surfaces) and the direct provider is a one-shot call that's easy to cut over. |
| V2.7 | TBD | **Go-to-definition / references / rename (as 3 sub-tickets)** | These are all position-to-range navigation features and share the same resolver in `tarn-lsp` (NAZ-297). Split into three tickets because rename is the only one that issues a `WorkspaceEdit`, and rolling back a bad rename migration should not also roll back definition. |
| V2.8 | TBD | **Code actions** | Introduces the `codeAction/resolve` and `workspace/applyEdit` round-trips. Migrated late because it ships the Quick Fix surface (NAZ-305) and an LSP regression here would silently break fix plans. The direct provider stays registered until this ticket is green. |
| V2.9 | TBD | **`tarn.evaluateJsonpath` executeCommand bridge** | Last on the list because it is the only feature that requires `workspace/executeCommand` plumbing. The bridge cannot be validated until every other feature the command depends on (hover inline-response, JSONPath evaluator) has shipped via the LSP path. |

### Ticket shape for each V2.x

Each V2.x ticket is expected to:

1. Add a per-feature toggle in `tarn.experimentalLspClient`'s
   client options (so the migration commit can cut over
   atomically) **OR**, more commonly, simply unregister the
   direct provider for that feature in `extension.ts` and
   flip the Phase V1 flag to `true` by default in workspace
   settings for the test suites that cover the feature.
2. Update the existing integration test (e.g.
   `tests/integration/suite/diagnostics.test.ts`) so that it
   asserts against the LSP path. The assertions should be
   byte-for-byte identical where possible; if not, the
   ticket must document the intentional drift.
3. Add a CHANGELOG entry describing the migration and a
   minor version bump.
4. Pass `cargo clippy -D warnings`, `cargo test`,
   `npm run test:unit`, `npm run test:integration`, and
   `npm run build` (bundle must not regress by more than
   ~10 KB).

## 3. Rollback plan

Rollback happens at two levels.

### Per-feature (expected rollback)

If a V2.x ticket's LSP path regresses on an integration test
after landing on `main`, the recovery is:

1. `git revert <commit-sha>` for the offending V2.x commit.
2. This restores the direct provider registration for that
   feature and its integration-test assertions.
3. Open a follow-up ticket (e.g. NAZ-3XXa "re-migrate
   feature X after fixing root cause") that blocks on the
   root cause being addressed in `tarn-lsp`.

Because every V2.x ticket is scoped to **one commit, one
feature, and no shared state with other V2.x tickets**, the
revert is always clean and does not touch the features that
have already migrated successfully. This is the key property
that motivated dual-host in the first place: a Phase V2
regression is never a Phase V2 stall.

### Full rollback (nuclear option)

If the dual-host approach itself proves untenable (e.g.
VS Code starts enforcing exclusivity on language-id providers
in a future release), the recovery is:

1. `git revert` every V2.x commit in reverse order (newest
   first) so each direct provider comes back on the branch
   that last had it working.
2. `git revert` the NAZ-309 scaffold commit. This removes
   the `vscode-languageclient` dependency, the experimental
   flag, and the `src/lsp/` directory.
3. Ship the reverted state as a minor release with a
   CHANGELOG entry that notes Phase V is on hold and refers
   to a new Phase V' ticket that rethinks the shape.

The scaffold commit (NAZ-309) is deliberately structured so
that a `git revert` of it alone is sufficient to remove every
Phase V1 artifact — nothing that the V1 ticket adds is
entangled with the existing direct-provider code (the only
`extension.ts` edit is a guarded `if (getExperimentalLspClient()) { ... }`
block). This makes the nuclear option genuinely cheap.

### What does NOT count as a rollback trigger

- An LSP regression in **tracing or logging** that does not
  change observable behavior. Log the issue and open a
  bug — do not revert.
- A bundle-size regression under ~10 KB. Open a follow-up
  to investigate; do not revert.
- A transient CI flake that does not reproduce locally.
  Investigate the flake — do not revert until the
  regression is confirmed.
- **Any regression fixable in `tarn-lsp`'s next patch
  release.** If the fix is faster than the revert +
  re-migrate cycle, fix the root cause instead. Rollback is
  the escape hatch, not the first response.

## 4. Version-bump policy for Phase V2

Two options were considered and we pick per-feature minor
bumps.

### Option 4A — One coordinated V2 release

Ship all of Phase V2 as a single version bump at the end of
the migration (e.g. `0.6.0 → 0.7.0`, with the LSP client
enabled by default).

Pros:

- One "big news" moment for end users ("VS Code extension
  now fully speaks LSP").

Cons:

- Users on `main` between V2.1 and V2.9 would have no
  packaged release reflecting the in-progress state. If a
  regression is reported against a marketplace build during
  that window, there is no signed VSIX that includes the
  fix.
- Release-candidate coordination is expensive. A single
  `0.6.0 → 0.7.0` release bundles nine features' worth of
  changelog and nine features' worth of marketplace
  release notes, which is hard to review and hard to
  rollback.

### Option 4B — Per-feature minor bumps (CHOSEN)

Each V2.x ticket ships as its own minor release: `0.6.0 →
0.6.1` is V1 (NAZ-309 scaffold), `0.6.1 → 0.6.2` is V2.1
(diagnostics), `0.6.2 → 0.6.3` is V2.2, and so on.

Pros:

- **Rollback granularity matches migration granularity.**
  The version number tells marketplace users exactly which
  feature moved to LSP and when. If a user files a bug
  against `0.6.3` the reporter can pin down the specific
  V2.x ticket that introduced it.
- Each release's changelog is small, focused, and easy to
  review.
- A marketplace release exists at every step, so if a user
  needs to pin a pre-migration version for one feature
  they can say "install `0.6.2` which has diagnostics on
  LSP but hover on direct".
- The alignment-with-Tarn policy from NAZ-288 already
  expects per-minor coordination; per-feature minor bumps
  dovetail with that contract.

Cons:

- More marketplace noise (nine releases over Phase V2
  instead of one).
- Each release cuts a `vsix` and pushes to both marketplace
  and Open VSX. The release pipeline already handles this
  automatically (NAZ-284), so the marginal cost is near
  zero.

### Interaction with the NAZ-288 alignment contract

The NAZ-288 coordinated-release policy says extension `X.Y.*`
tracks Tarn `X.Y.*` on the **minor number** and allows patch
numbers to diverge. Phase V2 moves on minor number in
lockstep where a feature migration is coupled to a
`tarn-lsp` capability change (e.g. V2.9 which exercises a
workspace/executeCommand bridge that may need a corresponding
`tarn-lsp` release). Feature migrations that do not require a
tarn-side change (e.g. V2.1 diagnostics, which uses already-shipped
`tarn-lsp` capabilities) ship as patch-level VS Code-only
releases: `0.6.0 → 0.6.1`, with `tarn` staying on whatever
patch level matches the current minor.

The alignment lint in
[`tests/unit/version.test.ts`](../tests/unit/version.test.ts)
enforces minor-level alignment on every test run; the
per-feature bumps above never violate it by construction.

## Appendix: Phase V1 scaffold surface

For the curious, here is what NAZ-309 actually landed. The
Phase V1 surface is intentionally tiny — everything else the
LSP client touches is bolted on by V2.x tickets.

- `editors/vscode/package.json`:
  - `"vscode-languageclient": "^9.0.1"` under `dependencies`.
  - `tarn.experimentalLspClient: boolean` (default `false`,
    `scope: "window"`).
  - `tarn.lspBinaryPath: string` (default `"tarn-lsp"`,
    `scope: "machine-overridable"`).
- `editors/vscode/src/lsp/tarnLspResolver.ts`:
  - `resolveTarnLspCommand(configured)` — pure
    setting-to-command mapping, unit-tested.
  - `resolveTarnLspBinary(scope?)` — impure wrapper that
    verifies absolute paths are accessible.
- `editors/vscode/src/lsp/client.ts`:
  - `buildClientOptions(binaryPath)` — pure builder for
    `ServerOptions` + `LanguageClientOptions`. Stdio
    transport, `{ language: "tarn", scheme: "file" }`
    document selector, dedicated "Tarn LSP" output
    channel, `RevealOutputChannelOn.Never`.
  - `startTarnLspClient(context, binaryPath)` — dynamic
    `import("vscode-languageclient/node.js")` so the
    language-client module is only loaded when the flag is
    on. Registers `dispose()` on `context.subscriptions` +
    explicit `await client.stop()` in `deactivate()`.
- `editors/vscode/src/extension.ts`:
  - `activate()` reads `tarn.experimentalLspClient`; if
    true, resolves the binary and starts the client.
    Any failure is non-fatal — the user sees one warning
    toast and the direct providers keep running.
  - `deactivate()` awaits `client.stop()` so the stdio
    handshake drains before the extension host tears
    down the child process.
  - Test hook: `testing.startExperimentalLspClient()` is
    the integration-test entry point (scoped under the
    internal `testing` sub-object of `TarnExtensionApi`).
- `editors/vscode/tests/unit/lspClient.test.ts` +
  `editors/vscode/tests/unit/tarnLspResolver.test.ts` —
  unit tests for the pure builders + constant pinning
  against the real language-client enums.
- `editors/vscode/tests/integration/suite/lspClient.test.ts`
  — integration test that boots the client, asserts
  `State.Running = 2`, and disposes cleanly. Skips (not
  fails) if `target/debug/tarn-lsp` is missing.

Everything else — the nine features, the `registerHoverProvider`
disposal, the completion trigger characters, the codeAction
provider cutover — lands in Phase V2 and is out of scope for
this document beyond the order documented above.
