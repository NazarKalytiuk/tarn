# tarn-lsp — Claude Code plugin

A Claude Code plugin that wires [`tarn-lsp`](../../../docs/TARN_LSP.md) into the Claude Code CLI so `.tarn.yaml` files get full LSP intelligence — diagnostics, hover, completion, document symbols, go-to-definition, references, rename, code lens, formatting, code actions, quick-fix, and JSONPath evaluation — while you work with Claude.

## Prerequisites

1. **Claude Code 2.0.74 or newer** — the LSP plugin system landed in late 2025. Run `claude --version` to check, then `npm update -g @anthropic-ai/claude-code` or `brew upgrade claude-code` as needed.
2. **`tarn-lsp` binary in your `$PATH`.** From a checkout of this repo:
   ```bash
   cargo install --path tarn-lsp
   ```
   Or build locally without installing and symlink the binary:
   ```bash
   cargo build --release -p tarn-lsp
   ln -s "$(pwd)/target/release/tarn-lsp" ~/.local/bin/tarn-lsp
   ```

## Install the plugin

Claude Code's plugin system works through **marketplaces**. This plugin is listed in the repo-root marketplace at `.claude-plugin/marketplace.json`, alongside the `tarn` MCP + skill plugin. Register the marketplace once, then install:

```bash
# From inside Claude Code
/plugin marketplace add NazarKalytiuk/tarn
/plugin install tarn-lsp@tarn --scope project
/reload-plugins
```

Prefer installing from a local checkout? Substitute the absolute path to this repo for `NazarKalytiuk/tarn` in the `marketplace add` call.

`--scope project` is important — see the [Compound-extension caveat](#compound-extension-caveat) below for why you should not install this plugin at user scope.

Alternatively, for a one-off session without installing anything persistently, use `claude --plugin-dir`:

```bash
claude --plugin-dir /absolute/path/to/tarn/editors/claude-code/tarn-lsp-plugin
```

## Verify it works

Inside Claude Code, with the plugin installed:

```
/plugin
```

Switch to the **Installed** tab and confirm `tarn-lsp` is listed with no errors. Then open any `.tarn.yaml` file in the project and:

1. Introduce a typo in a schema key. Claude Code's diagnostics indicator (press **Ctrl+O**) should show the parser error with a precise line range.
2. Hover over `{{ env.api_key }}` — you should see the resolved value and source file.
3. Start typing `{{ capture.` — captures from earlier steps in the current test should autocomplete.

If any of that fails, see **Troubleshooting** below.

## Compound-extension caveat

Tarn test files use the compound extension `.tarn.yaml`. Claude Code's current LSP plugin format registers language servers by **simple file extension** (`.yaml`), so this plugin necessarily claims **all** `.yaml` (and `.yml`) files in any project where it's installed. That means any *other* YAML language server you had running — `yaml-language-server` for Kubernetes manifests, Compose files, CI configs — will be shadowed for the same files while this plugin is active.

**Recommendation**: install this plugin at `--scope project` in repos that are Tarn-focused (all `.yaml` is Tarn test content, or any non-Tarn YAML is fine going unchecked for the session). Do **not** install at user scope.

If you need side-by-side Tarn + generic YAML intelligence in the same repo, this plugin is not the right fit yet. The gap is tracked as a Phase L2 follow-up; we will file feedback with Claude Code requesting either compound-extension support (`.tarn.yaml`) or a glob-based file-pattern matcher.

## Troubleshooting

### `Executable not found in $PATH`

The plugin is installed but Claude Code can't find the `tarn-lsp` binary. Check:
- `which tarn-lsp` — does the binary resolve on your `$PATH`?
- If you installed via `cargo install --path tarn-lsp`, confirm `~/.cargo/bin` is on `$PATH`.
- If you symlinked into `~/.local/bin`, confirm that directory is on `$PATH` in the shell where you launch Claude Code.

### LSP server not responding

Run Claude Code with `claude --debug` and look for `tarn-lsp` loading errors in the log. The server writes a startup banner (`tarn-lsp 0.6.0 initialized`) to stderr on successful `initialize`, visible in the debug log.

### Diagnostics / hover / completion missing on some `.yaml` files

Those files aren't Tarn test files — the `.yaml` extension claim is too broad (see **Compound-extension caveat**). Move Tarn tests under a predictable directory and either exclude non-Tarn YAML from the project or use `--scope local` so the plugin only activates when you're actively editing Tarn tests.

## What this plugin does NOT do

- **No code execution.** `tarn run --select` is still a shell command — Claude Code calls it directly, the LSP does not embed the runner. Run-test / run-step code lenses emit the selector; the client (Claude Code) dispatches execution itself.
- **No bundled `tarn-lsp` binary.** You install the binary separately. A future revision may bundle the binary inside the plugin directory.
- **No Marketplace publication.** This plugin lives in the `tarn` repo at `editors/claude-code/tarn-lsp-plugin/` and is served from the repo-root marketplace (`.claude-plugin/marketplace.json`). It is not published to the official Anthropic marketplace yet; that's a soak-test-and-then-publish follow-up.

## References

- Full `tarn-lsp` spec: [`docs/TARN_LSP.md`](../../../docs/TARN_LSP.md).
- Claude Code plugin system: [plugins reference](https://code.claude.com/docs/en/plugins-reference).
- Claude Code LSP servers section: [plugins-reference#lsp-servers](https://code.claude.com/docs/en/plugins-reference#lsp-servers).
- Related Linear ticket: NAZ-310.
