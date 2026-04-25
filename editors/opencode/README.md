# Tarn + opencode

How to wire [opencode](https://opencode.ai) into a repo so agents running inside it get the full Tarn surface: structured MCP tools, language-server intelligence on `.tarn.yaml`, and the `tarn-api-testing` skill.

This is the companion to [`../claude-code/tarn-lsp-plugin/README.md`](../claude-code/tarn-lsp-plugin/README.md) for the opencode side. Use it alongside [`docs/TARN_LSP.md`](../../docs/TARN_LSP.md) and the repo-root [`README.md`](../../README.md).

## TL;DR

Opencode has no marketplace or plugin installer for non-JS tools like Tarn. Integration is config-driven: check three files into your repo and you are done.

```
your-repo/
├── opencode.jsonc                          # MCP + LSP registration
└── .opencode/
    └── skills/
        └── tarn-api-testing/
            └── SKILL.md                    # agent-visible skill
```

The Tarn repo itself ships exactly this layout (see [`opencode.jsonc`](../../opencode.jsonc) at the root and [`.opencode/skills/tarn-api-testing/`](../../.opencode/skills/tarn-api-testing/), symlinked to the canonical [`plugin/skills/tarn-api-testing/`](../../plugin/skills/tarn-api-testing/)) — clone, run `opencode` inside, and the agent has Tarn superpowers immediately.

## Prerequisites

1. **opencode** installed. See [opencode.ai/docs](https://opencode.ai/docs/).
2. **`tarn-mcp`** and **`tarn-lsp`** binaries on `$PATH`. From a checkout of [NazarKalytiuk/tarn](https://github.com/NazarKalytiuk/tarn):
   ```bash
   cargo install --path tarn-mcp
   cargo install --path tarn-lsp
   ```
   Or symlink workspace builds into `~/.local/bin`:
   ```bash
   cargo build --release -p tarn-mcp -p tarn-lsp
   ln -s "$(pwd)/target/release/tarn-mcp" ~/.local/bin/tarn-mcp
   ln -s "$(pwd)/target/release/tarn-lsp" ~/.local/bin/tarn-lsp
   ```

## Wiring it into your own repo

### 1. MCP server and LSP config

Copy [`opencode.example.jsonc`](./opencode.example.jsonc) to `opencode.jsonc` at the root of your repo (or merge the `mcp` and `lsp` blocks into your existing config):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "tarn": { "type": "local", "command": ["tarn-mcp"], "enabled": true }
  },
  "lsp": {
    "tarn": { "command": ["tarn-lsp"], "extensions": [".yaml", ".yml"] }
  }
}
```

Project-level (`opencode.jsonc` in repo root) is the right scope for the LSP entry — see the caveat below.

### 2. Skill

Give the agent the `tarn-api-testing` skill. Opencode auto-discovers skills at `.opencode/skills/<name>/SKILL.md` (and also reads `.claude/skills/` / `.agents/skills/` variants for Claude-compatible repos).

Easiest option: vendor the skill directory from this repo into your own, or keep a git submodule pointing at `plugin/skills/tarn-api-testing/`. The skill's `SKILL.md` plus the four files under `references/` are self-contained and require no source code.

## Compound-extension caveat

Tarn test files use the compound extension `.tarn.yaml`, but opencode's LSP matcher uses `path.parse(file).ext`, which only returns the final dotted component — `.yaml`. This plays out the same way it does in Claude Code: the `tarn` LSP entry unavoidably claims every `.yaml` / `.yml` file in the workspace, not just `.tarn.yaml`.

That means:

- **Put the `lsp` entry in project-level `opencode.jsonc`**, committed at the root of a Tarn-focused repo. Do **not** add it to your global `~/.config/opencode/config.json` if you also edit Kubernetes manifests, Compose files, GitHub Actions workflows, or other non-Tarn YAML through opencode.
- If your repo mixes Tarn tests with unrelated YAML, you will want to disable the `tarn` LSP entry per-file or move the non-Tarn YAML into a workspace opencode is not rooted in.

The same limitation is tracked upstream for Claude Code's LSP plugin schema; a suffix-matcher fix on either side resolves it for both. For context see the "Compound-extension caveat" section in [`../claude-code/tarn-lsp-plugin/README.md`](../claude-code/tarn-lsp-plugin/README.md).

## Verifying it works

Inside your repo with `opencode.jsonc` in place:

1. Start opencode. Ask: *"list available MCP tools"*. You should see `tarn_run`, `tarn_validate`, `tarn_list`, `tarn_fix_plan`.
2. Open any `.tarn.yaml` file. Introduce a typo in a schema key — opencode should surface the parser diagnostic with a precise line range.
3. Hover over `{{ env.api_key }}` in a test file — resolved value and source file should appear.
4. Ask the agent: *"write a Tarn smoke test for GET /health"*. The `tarn-api-testing` skill should activate and produce a valid `.tarn.yaml`.

## Troubleshooting

### MCP tools are missing

- Confirm `tarn-mcp` resolves: `which tarn-mcp`.
- Check opencode's MCP log (opencode surfaces MCP stdio errors at startup). A common failure is `ENOENT` when the binary is not on the `$PATH` opencode inherits — launch opencode from a shell where `which tarn-mcp` succeeds.

### LSP not attaching to `.tarn.yaml` buffers

- Confirm `tarn-lsp` resolves: `which tarn-lsp`.
- Confirm the `extensions` array includes `.yaml` (not `.tarn.yaml`) — opencode cannot match compound extensions.
- If another LSP (a generic `yaml-language-server`) is configured for `.yaml` in the same workspace, opencode will route to one and shadow the other. Keep Tarn-focused repos narrow.

### Skill isn't showing up

- `SKILL.md` must live at `.opencode/skills/<name>/SKILL.md` (directory name matches `name:` frontmatter). A bare `.opencode/skills/<name>.md` is **not** recognized.
- Frontmatter `name:` must match `[a-z][a-z0-9-]{0,63}`.

## References

- [opencode MCP servers docs](https://opencode.ai/docs/mcp-servers/)
- [opencode LSP docs](https://opencode.ai/docs/lsp/)
- [opencode agent skills docs](https://opencode.ai/docs/skills/)
- [Tarn LSP spec](../../docs/TARN_LSP.md)
- [Tarn MCP workflow](../../docs/MCP_WORKFLOW.md)
- [Claude Code companion plugin](../claude-code/tarn-lsp-plugin/README.md) — mirrors this setup through Claude Code's plugin/marketplace system.
