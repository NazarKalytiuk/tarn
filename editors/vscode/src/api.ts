/**
 * Public API surface of the Tarn VS Code extension.
 *
 * This file is the SINGLE SOURCE OF TRUTH for `TarnExtensionApi`, the
 * object returned by `activate()` and exposed to downstream integrators
 * via `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode').exports`.
 *
 * ## Semver policy
 *
 * The extension follows semantic versioning for its public API:
 *
 * - **Breaking changes to `@stability stable` fields** require a major
 *   version bump (e.g. `1.x.y` → `2.0.0`). Removing a field, renaming a
 *   field, narrowing a return type, or widening a parameter type all
 *   count as breaking. Adding a new optional field to a `stable` object
 *   is NOT breaking.
 * - **`@stability preview` fields** may change in any minor release
 *   (`1.1.0` → `1.2.0`). They are shipped so integrators can experiment
 *   and give feedback before the field is promoted to `stable`. Preview
 *   fields are listed explicitly in `docs/VSCODE_EXTENSION.md` in the
 *   "Public API" section.
 * - **`@stability internal` fields** have no compatibility guarantees
 *   whatsoever. They exist to let the extension's own integration tests
 *   poke at internal state that would otherwise require a full VS Code
 *   API roundtrip. Their shape, presence, and behavior can change
 *   between any two releases, including patch releases. Downstream code
 *   that reads internal fields will break silently on upgrade. Do not
 *   use internal fields from production code.
 *
 * ## 1.0.0 gating
 *
 * Until the extension version reaches `1.0.0`, the stable surface is
 * still subject to one last round of pruning. When the extension ships
 * `1.0.0`, every field currently marked `@stability stable` below is
 * frozen under the semver policy above, and the set of stable fields
 * is locked to whatever this file declares at tag time. The roadmap
 * step that performs the `0.x` → `1.0.0` cut is tracked as NAZ-288.
 *
 * A golden snapshot of the declaration in this file is kept at
 * `tests/golden/api.snapshot.txt` and compared in
 * `tests/unit/apiSurface.test.ts`. Any edit to this file that changes
 * the normalized declaration — including whitespace inside the
 * `TarnExtensionApi` interface — must be accompanied by an explicit
 * snapshot update, which is the extension's CI-enforced "did you mean
 * to change the public API?" gate.
 */

import type * as vscode from "vscode";
import type { TarnBackend } from "./backend/TarnBackend";
import type { Report, StepResult } from "./util/schemaGuards";
import type { StepKey } from "./testing/LastRunCache";
import type { FixPlanGroup } from "./views/FixPlanView";
import type { BenchRunContext } from "./views/BenchRunnerPanel";
import type {
  InitProjectOptions,
  InitProjectOutcome,
} from "./commands/initProject";
import type {
  RunHistoryEntry,
  RunHistoryFilter,
} from "./views/RunHistoryView";

/**
 * Opaque, test-only surface. Not part of the public API contract.
 *
 * Every member of this object is implicitly `@stability internal`.
 * The shape may change between any two releases — including patch
 * releases — without a changelog entry. Downstream integrators must
 * not read, call, or branch on anything under `testing`. It exists
 * solely so the extension's own `@vscode/test-electron` integration
 * tests can poke at internal state (workspace index, notifier, fix
 * plan view, run history store, etc.) without having to route every
 * assertion through the VS Code command palette.
 */
export interface TarnExtensionTestingApi {
  readonly backend: TarnBackend;
  readonly validateDocument: (uri: vscode.Uri) => Promise<void>;
  readonly reloadEnvironments: () => Promise<void>;
  readonly listEnvironments: () => Promise<
    ReadonlyArray<{
      name: string;
      source_file: string;
      vars: Readonly<Record<string, string>>;
    }>
  >;
  readonly getActiveEnvironment: () => string | null;
  readonly formatDocument: (uri: vscode.Uri) => Promise<vscode.TextEdit[]>;
  readonly lastRunCacheSize: () => number;
  readonly loadLastRunFromReport: (report: Report) => void;
  readonly showStepDetails: (key: StepKey) => boolean;
  readonly loadCapturesFromReport: (report: Report) => void;
  readonly capturesTotalCount: () => number;
  readonly isCaptureKeyRedacted: (key: string) => boolean;
  readonly isHidingAllCaptures: () => boolean;
  readonly toggleHideCaptures: () => void;
  readonly loadFixPlanFromReport: (report: Report) => void;
  readonly fixPlanSnapshot: () => ReadonlyArray<FixPlanGroup>;
  readonly showReportHtml: (html: string) => void;
  readonly sendReportMessage: (message: unknown) => Promise<boolean>;
  readonly showBenchResult: (context: BenchRunContext) => void;
  readonly lastBenchContext: () => BenchRunContext | undefined;
  readonly importHurl: (
    source: string,
    dest: string,
    cwd: string,
  ) => Promise<{ success: boolean; exitCode: number | null; stderr: string }>;
  readonly initProject: (
    options: InitProjectOptions,
  ) => Promise<InitProjectOutcome>;
  readonly history: {
    readonly add: (entry: RunHistoryEntry) => Promise<void>;
    readonly all: () => ReadonlyArray<RunHistoryEntry>;
    readonly clear: () => Promise<void>;
    readonly setFilter: (filter: RunHistoryFilter) => void;
    readonly getFilter: () => RunHistoryFilter;
  };
  readonly notifier: {
    readonly isTarnViewFocused: () => boolean;
    readonly wouldNotify: (
      report: Report,
      options: { dryRun: boolean },
    ) => boolean;
    readonly maybeNotify: (
      report: Report,
      options: { dryRun: boolean; files: string[] },
    ) => Promise<boolean>;
  };
  /**
   * Build VS Code `TestMessage`s for a failing step, honoring the
   * JSON-reported `location` metadata (Tarn T55) with an AST range
   * fallback. Exposed for integration tests that want to verify the
   * location resolution pipeline end-to-end against a real Tarn run.
   *
   * The `astFallback` parameter simulates the AST-derived
   * `stepItem.range` from discovery. Pass `null` to simulate a step
   * with no AST anchor at all.
   */
  readonly buildFailureMessagesForStep: (
    step: StepResult,
    fileUri: vscode.Uri,
    astFallback: vscode.Range | null,
  ) => vscode.TestMessage[];
  /**
   * Live snapshot of the workspace index. Exposed for the scoped
   * discovery integration test (NAZ-282) which needs to observe
   * individual file updates post-activation. The shape is an
   * array of one entry per indexed file with its resolved URI,
   * the file's display name, the test names, and whether the
   * entry was built from `tarn list --file` (`fromScopedList:
   * true`) or the AST fallback (`fromScopedList: false`).
   */
  readonly workspaceIndexSnapshot: () => ReadonlyArray<{
    readonly uri: string;
    readonly fileName: string;
    readonly tests: ReadonlyArray<{
      readonly name: string;
      readonly stepCount: number;
    }>;
    readonly fromScopedList: boolean;
  }>;
  /**
   * Force a scoped refresh of a single file, awaiting completion.
   * Exposed so integration tests can deterministically observe
   * the incremental discovery path instead of racing the native
   * `FileSystemWatcher`, which is unreliable for files created
   * via `fs.writeFile` during a test run.
   */
  readonly refreshSingleFile: (uri: vscode.Uri) => Promise<void>;
  /**
   * Phase V1 (NAZ-309) test hook: boots the experimental
   * `tarn-lsp` language client on demand and returns a tiny probe
   * into its lifecycle. Exposed so the integration test can
   * verify that the `tarn.experimentalLspClient = true` code path
   * reaches `State.Running` and disposes cleanly without having
   * to reload the extension host mid-test.
   *
   * Resolves to `undefined` if the experimental feature is not
   * enabled or if the `tarn-lsp` binary cannot be spawned — the
   * integration test skips gracefully in those cases. Calling
   * `dispose()` on the returned probe stops the client and
   * removes it from the module-scoped handle so `deactivate()`
   * is a no-op afterwards.
   */
  readonly startExperimentalLspClient: () => Promise<
    | {
        readonly running: boolean;
        readonly state: number;
        readonly dispose: () => Promise<void>;
      }
    | undefined
  >;
}

/**
 * Public API returned by `activate()`.
 *
 * Downstream integrators can obtain this object via:
 *
 * ```ts
 * const ext = vscode.extensions.getExtension<TarnExtensionApi>(
 *   "nazarkalytiuk.tarn-vscode",
 * );
 * const api = await ext?.activate();
 * ```
 *
 * Every field below carries a `@stability` annotation that tells you
 * how much you can depend on it. See the file-level block comment
 * above for the semver policy those annotations point at.
 */
export interface TarnExtensionApi {
  /**
   * The `vscode.TestController` id used by the extension's Test
   * Explorer integration. Stable across releases so other extensions
   * can reference runs via the Testing API.
   *
   * @stability stable
   */
  readonly testControllerId: string;

  /**
   * Number of `.tarn.yaml` files currently tracked by the workspace
   * index. Reflects the state at the time `activate()` resolved; use
   * the Testing API for live updates.
   *
   * @stability stable
   */
  readonly indexedFileCount: number;

  /**
   * The full list of command ids the extension contributes. Useful
   * for downstream extensions that want to build a palette or wire
   * their own UI to Tarn actions without hard-coding command ids.
   * The order of this array is not guaranteed.
   *
   * @stability stable
   */
  readonly commands: readonly string[];

  /**
   * Opaque, test-only surface. This whole sub-object is marked
   * internal — its shape may change between any two releases,
   * including patch releases, with no changelog entry. Do not use
   * from production code.
   *
   * @stability internal
   */
  readonly testing: TarnExtensionTestingApi;
}
