# Tarn for VS Code

![Tarn banner](media/marketplace/banner.png)

**Run, debug, and iterate on API tests without leaving the editor.** Tarn turns your `.tarn.yaml` files into first-class VS Code tests: discovered in the Test Explorer, runnable from CodeLens, streamed live with structured events, and — when something fails — jumped straight to the asserting line with a unified diff of expected vs actual right in the peek view.

Aimed squarely at the tight diagnosis loop that API debugging lives inside: **run → see the failure → fix the YAML or the server → rerun → green**. No terminal juggling, no re-parsing JSON by eye, no guessing which step blew up. The extension drives the [Tarn CLI](https://github.com/NazarKalytiuk/tarn) so everything that runs in your editor runs identically in CI — the same binary, the same report, the same exit code.

![Test Explorer tree](media/marketplace/screenshot-test-explorer.png)

## Features

### Test Explorer

- Hierarchical discovery of `*.tarn.yaml` across the workspace: file → test → step.
- Run and Dry Run profiles.
- Cancellable runs with live progress streamed into the Tarn output channel.
- Rich failure messages: expected vs actual, unified diff, request, response, remediation hints, failure category, error code.

![Live streaming](media/marketplace/screenshot-streaming.png)
![Failure diff in peek view](media/marketplace/screenshot-diff.png)

### Editor

- CodeLens above every test and step: `Run`, `Dry Run`, `Run step`.
- File-level schema validation for `*.tarn.yaml` via `redhat.vscode-yaml` and the Tarn JSON schema.
- Snippet library for common test patterns (`tarn-test`, `tarn-step`, `tarn-capture`, `tarn-poll`, `tarn-form`, `tarn-graphql`, `tarn-multipart`, `tarn-lifecycle`, `tarn-include`).
- Tarn-aware syntax highlighting for interpolation, JSONPath, and assertion operators.

![CodeLens actions](media/marketplace/screenshot-codelens.png)

### Environments

- Auto-discovers every `tarn.env*.yaml` in the workspace and exposes them via
  the `Tarn: Select Environment…` quick-pick and the status-bar entry.
- Persists the active environment per workspace so reruns stay scoped.
- `tarn.defaultEnvironment` lets you pin a default in workspace settings.

![Environment picker](media/marketplace/screenshot-env-picker.png)

### Commands

| Command | Description |
|---|---|
| `Tarn: Run All Tests` | Runs every discovered test file. |
| `Tarn: Run Current File` | Runs only the active `.tarn.yaml`. |
| `Tarn: Dry Run Current File` | Interpolates but does not send requests. |
| `Tarn: Validate Current File` | Invokes `tarn validate`. |
| `Tarn: Rerun Last Run` | Reuses the last run request. |
| `Tarn: Select Environment…` | Picks an environment from discovered `tarn.env.*.yaml`. |
| `Tarn: Set Tag Filter…` | Applies a comma-separated tag filter. |
| `Tarn: Show Output` | Focuses the Tarn output channel. |
| `Tarn: Install / Update Tarn` | Opens install instructions. |

### Status bar

- Left: active environment (click to pick).
- Right: last run summary (click to open output).

## Settings

All settings live under the `tarn.*` namespace. The most useful are:

- `tarn.binaryPath` — path to the Tarn CLI binary. Defaults to `tarn`.
- `tarn.testFileGlob` — discovery glob. Defaults to `**/*.tarn.yaml`.
- `tarn.excludeGlobs` — excluded globs. Defaults to `["**/target/**","**/node_modules/**","**/.git/**"]`.
- `tarn.defaultEnvironment` — environment passed as `--env` when nothing is picked.
- `tarn.defaultTags` — default tag filter.
- `tarn.parallel` — toggle `--parallel`.
- `tarn.jsonMode` — `verbose` or `compact`.
- `tarn.showCodeLens` — toggle CodeLens actions.
- `tarn.statusBar.enabled` — toggle the status bar entries.

See the full list in the VS Code Settings UI under `Extensions → Tarn`.

## Requirements

- Tarn CLI (`tarn`) on `PATH`, or a custom path configured via `tarn.binaryPath`.
- [`redhat.vscode-yaml`](https://marketplace.visualstudio.com/items?itemName=redhat.vscode-yaml) (declared as an extension dependency; installed automatically).

## Install Locally

1. From the `editors/vscode` folder, run `npm install && npm run build`.
2. `Developer: Install Extension from Location…` in VS Code and pick the `editors/vscode` folder.
3. Or run `npm run package` to build a VSIX, then `Extensions: Install from VSIX…`.

## Trusted vs Untrusted Workspaces

In untrusted workspaces the extension provides read-only features only (grammar, snippets, schema validation). Running tests, validating files, and spawning the Tarn binary are disabled until the workspace is trusted.

## Remote Setups

The extension is a UI-less backend that shells out to the `tarn` binary and reads file paths via VS Code's workspace API, so it follows VS Code's standard Remote Development model: **the extension always runs on the same side as the files and the binary** (remote host for Remote SSH, inside the container for Dev Containers/Codespaces, inside the Linux distro for WSL). There are no "local bridge" paths to worry about.

All four supported remote setups have been audited. The full audit — what was checked, what was fixed, and the per-environment checklist — lives in [`docs/VSCODE_REMOTE.md`](../../docs/VSCODE_REMOTE.md). The short version:

### Dev Container

Copy [`media/remote/devcontainer.json`](media/remote/devcontainer.json) to `.devcontainer/devcontainer.json`. It uses the official Rust dev container image (`mcr.microsoft.com/devcontainers/rust:1-bookworm`), installs `tarn` into `/usr/local/cargo/bin/tarn` via `cargo install tarn-cli --locked` in `postCreateCommand`, and auto-installs both `nazarkalytiuk.tarn-vscode` and `redhat.vscode-yaml` inside the container. Because the base image already puts `/usr/local/cargo/bin` on PATH for the `vscode` user, the extension's default `tarn.binaryPath = "tarn"` resolves without any per-container override.

### GitHub Codespaces

Codespaces consumes the exact same `.devcontainer/devcontainer.json` — no second config file is required. The extension is a good candidate for a **Codespaces prebuild**: add the file, then enable prebuilds on the repository so `cargo install tarn-cli` runs ahead of time and new Codespaces start with `tarn` already compiled. The Tarn extension installs automatically via the `customizations.vscode.extensions` list.

### WSL

With the extension installed into the WSL distro (VS Code's "Install in WSL" button, or the `Remote-WSL` kind under Extensions), `tarn.binaryPath` defaults to the Linux-side `tarn` on `$PATH` — **not** the Windows-side `tarn.exe`. The extension never branches on `process.platform`, never uses `\\` separators, and builds argv paths via Node's `path` module which runs inside the Linux server and therefore always produces POSIX paths. If you keep the Tarn CLI on PATH inside WSL (`cargo install tarn-cli` or a prebuilt release in `~/.local/bin`), no extra configuration is needed.

### Remote SSH

Install the extension on the remote host (VS Code's "Install in SSH: host" action). Binary resolution uses the remote host's `$PATH`, **not** the local machine's. If `tarn` is not on the remote PATH, set `tarn.binaryPath` to an **absolute remote path** (e.g. `/home/you/.cargo/bin/tarn` or `/usr/local/bin/tarn`). The setting is declared `machine-overridable` so you can pin it per remote without polluting your local workspace settings. The same policy applies to `tarn.requestTimeoutMs`: slow-network remotes can override the watchdog without affecting your local runs.

## What Gets Wired

- `*.tarn.yaml`, `*.tarn.yml` → language id `tarn`.
- Tarn test schema → `schemas/v1/testfile.json`.
- JSON report schema → `schemas/v1/report.json` for `tarn-report.json` and `*.tarn-report.json`.

## Release

The extension publishes to both the [VS Code Marketplace](https://marketplace.visualstudio.com/) and [Open VSX](https://open-vsx.org/) from tagged releases via `.github/workflows/vscode-extension-release.yml`.

### One-time setup

1. **Verify the publisher** on both marketplaces for `nazarkalytiuk` (manual, one-time on each site).
2. **Create marketplace PATs** and add them to the repo under **Settings → Secrets and variables → Actions**:
   - `VSCE_PAT` — from [dev.azure.com/<publisher>/_usersSettings/tokens](https://dev.azure.com/) with the `Marketplace › Manage` scope.
   - `OVSX_PAT` — from [open-vsx.org/user-settings/tokens](https://open-vsx.org/user-settings/tokens).
3. **Create the Open VSX namespace** (once): `npx ovsx create-namespace nazarkalytiuk -p "$OVSX_PAT"`.

### Cutting a release

1. Bump `editors/vscode/package.json` to the new version. The workflow will fail if `package.json` disagrees with the tag.
2. Add a `## <version>` section to `editors/vscode/CHANGELOG.md`.
3. Commit and tag: `git tag v<version> && git push origin v<version>`.
4. The `Release` workflow (Rust binaries) runs first. The `VS Code Extension Release` workflow runs in parallel, waits for the binary release to appear on GitHub, then packages and publishes the VSIX to both marketplaces.
5. Re-runs: if either publish fails, re-invoke via **Actions → VS Code Extension Release → Run workflow** with the failed tag as the `tag` input.

### Pre-releases

Tags with a hyphen suffix (e.g. `v0.19.0-rc.1`, `v0.20.0-beta`) are published with the `--pre-release` flag on both marketplaces. Stable tags (`v0.19.0`) publish as regular releases.

### Local dry-run

To verify the VSIX builds cleanly without publishing:

```bash
cd editors/vscode
npm ci
npm run lint
npm run test:unit
npm run build
npx vsce package --no-dependencies --out tarn-vscode.vsix
```

## Public API

Other extensions can consume `TarnExtensionApi` via `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode').exports`. See [`docs/API.md`](docs/API.md) for the stable surface, stability tiers, and semver policy.

## Roadmap

See `docs/VSCODE_EXTENSION.md` for the full phased plan and Tarn-side dependencies (`T51`–`T57`).
