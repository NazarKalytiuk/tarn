# Tarn + OpenAI Codex

How to wire [OpenAI Codex](https://developers.openai.com/codex/) (the `codex`
CLI) into a repo so the agent gets the full Tarn surface: the structured
`tarn-mcp` tools and the `tarn-api-testing` skill.

This is the Codex companion to [`../opencode/README.md`](../opencode/README.md)
and [`../claude-code/tarn-lsp-plugin/README.md`](../claude-code/tarn-lsp-plugin/README.md).
Use it alongside [`docs/MCP_WORKFLOW.md`](../../docs/MCP_WORKFLOW.md) and the
repo-root [`README.md`](../../README.md).

## TL;DR

Codex integration is two pieces — one command and one skill directory:

```
your-repo/
├── .codex/config.toml          # or ~/.codex/config.toml — MCP registration
└── .agents/skills/
    └── tarn-api-testing/
        └── SKILL.md             # Agent Skills standard, read by Codex
```

The Tarn repo itself ships the skill at [`.agents/skills/tarn-api-testing/`](../../.agents/skills/tarn-api-testing)
(symlinked to the canonical [`plugin/skills/tarn-api-testing/`](../../plugin/skills/tarn-api-testing/))
and an [`AGENTS.md`](../../AGENTS.md) at the root — clone it, run `codex mcp add tarn -- tarn-mcp`
once, and the agent has Tarn superpowers immediately.

## Prerequisites

1. **Codex CLI** installed (`codex --version`). See [developers.openai.com/codex](https://developers.openai.com/codex/).
2. **`tarn` and `tarn-mcp`** binaries on `$PATH`. From a checkout of [NazarKalytiuk/tarn](https://github.com/NazarKalytiuk/tarn):
   ```bash
   cargo install --path . --bin tarn
   cargo install --path tarn-mcp
   ```
   Or symlink a workspace build into `~/.local/bin`:
   ```bash
   cargo build --release -p tarn -p tarn-mcp
   ln -s "$(pwd)/target/release/tarn"     ~/.local/bin/tarn
   ln -s "$(pwd)/target/release/tarn-mcp" ~/.local/bin/tarn-mcp
   ```

## Wiring it into your own repo

### 1. MCP server

The one-liner registers `tarn-mcp` with Codex and writes the config for you:

```bash
codex mcp add tarn -- tarn-mcp
codex mcp list          # confirm "tarn" is listed
```

Everything after `--` is the stdio launch command. Prefer editing config by
hand, or want it committed to the repo? Copy [`config.example.toml`](./config.example.toml)
into `~/.codex/config.toml` (user scope) or `.codex/config.toml` (project scope):

```toml
[mcp_servers.tarn]
command = "tarn-mcp"
args = []
```

`command` is the bare executable; `args` is a separate array (empty for Tarn).

### 2. Skill

Codex discovers skills via the **Agent Skills standard**: a `SKILL.md` in a
named directory under `.agents/skills/` (repo / current dir / repo root) or
`~/.agents/skills/` (user scope). Drop the Tarn skill at:

```
.agents/skills/tarn-api-testing/SKILL.md
```

Easiest option: vendor the skill directory from this repo into your own, or keep
a git submodule pointing at [`plugin/skills/tarn-api-testing/`](../../plugin/skills/tarn-api-testing/).
`SKILL.md` plus the four files under `references/` are self-contained and need no
source code. Because `.agents/skills/` is a cross-agent standard, the same
directory also lights the skill up for opencode and pi.

Codex activates a skill implicitly when a task matches its `description`, or you
can invoke it explicitly from the `/skills` menu or with `$tarn-api-testing`.

### 3. AGENTS.md (optional but recommended)

Codex reads `AGENTS.md` from the global config, the repo root, and any closer
override. The repo root [`AGENTS.md`](../../AGENTS.md) is already a concise Tarn
agent guide; keeping it committed means even a Codex session without the skill
loaded knows the `validate → run → read failures → fix` loop.

## LSP note

Codex is not an LSP client, so it does **not** consume `tarn-lsp`, and the
`.yaml` compound-extension caveat that applies to opencode and Claude Code does
**not** apply here. Language intelligence in Codex comes from the skill and the
structured MCP tool results, not a language server.

## Verifying it works

Inside your repo after `codex mcp add tarn -- tarn-mcp`:

1. `codex mcp list` shows `tarn`.
2. Start `codex`. Ask: *"list available MCP tools"*. You should see `tarn_run`,
   `tarn_validate`, `tarn_list`, `tarn_fix_plan`.
3. Ask: *"write a Tarn smoke test for GET /health and run it"*. The
   `tarn-api-testing` skill should activate and produce a valid `.tarn.yaml`,
   then call `tarn_validate` / `tarn_run`.

A non-interactive smoke check of the server itself (no Codex required):

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"verify","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
  | tarn-mcp | grep -o '"name":"tarn_[a-z_]*"'
```

## End-to-end example

A reproducible write → run → read-failure → fix loop lives in
[`examples/agent-loop/`](../../examples/agent-loop/) — it hits the public
JSONPlaceholder API, so there is nothing to start. In a Codex session:

> *"Run `examples/agent-loop/api.tarn.yaml` with the tarn tools. If anything
> fails, call `tarn_fix_plan` and patch it."*

Codex will call `tarn_validate` → `tarn_run`, read `summary.status` and
`failure_category` from the structured result, and (on failure) call
`tarn_fix_plan`. See [`examples/agent-loop/README.md`](../../examples/agent-loop/README.md)
for the full loop and a one-line way to make it fail on purpose.

## Troubleshooting

### MCP tools are missing

- `codex mcp list` — is `tarn` registered? If not, rerun `codex mcp add tarn -- tarn-mcp`.
- `which tarn-mcp` — the binary must be on the `$PATH` Codex inherits. A common
  failure is `ENOENT` when `tarn-mcp` is not resolvable from the shell that
  launched Codex.
- If startup times out, raise `startup_timeout_sec` in the `[mcp_servers.tarn]`
  block.

### The skill never activates

- `SKILL.md` must live at `.agents/skills/<name>/SKILL.md` (a bare
  `.agents/skills/<name>.md` is not recognized).
- The directory name should match the `name:` frontmatter (`tarn-api-testing`).
- Implicit activation keys off the `description` — invoke it explicitly via the
  `/skills` menu or `$tarn-api-testing` to confirm it loads.

## References

- [Codex MCP docs](https://developers.openai.com/codex/mcp) — `codex mcp add` and `[mcp_servers.*]`
- [Codex configuration reference](https://developers.openai.com/codex/config-reference) — `~/.codex/config.toml` keys
- [Codex Agent Skills](https://developers.openai.com/codex/skills) — `.agents/skills/<name>/SKILL.md`
- [Codex AGENTS.md guide](https://developers.openai.com/codex/guides/agents-md)
- [Tarn MCP workflow](../../docs/MCP_WORKFLOW.md)
- [opencode companion](../opencode/README.md) · [pi companion](../pi/README.md)
