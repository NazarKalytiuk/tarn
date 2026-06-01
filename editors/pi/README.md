# Tarn + pi

How to wire the [pi coding agent](https://github.com/badlogic/pi-mono/tree/main/packages/coding-agent)
into a repo so the agent can write, run, and debug Tarn tests.

This is the pi companion to [`../opencode/README.md`](../opencode/README.md) and
[`../codex/README.md`](../codex/README.md). Use it alongside
[`docs/MCP_WORKFLOW.md`](../../docs/MCP_WORKFLOW.md) and the repo-root
[`README.md`](../../README.md).

## How pi integrates (read this first)

pi is deliberately minimal and has **no built-in MCP support**. From the pi docs:

> **No MCP.** Build CLI tools with READMEs (see Skills), or build an extension
> that adds MCP support.
> — [packages/coding-agent/README.md](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/README.md)

That is a perfect fit for Tarn: `tarn` is a single static binary with structured
JSON output, and pi already ships a `bash` tool. So the canonical pi integration
is the **Agent Skills standard plus the `tarn` CLI** — the skill teaches pi the
failures-first loop, and pi runs `tarn` directly. No MCP server is required.

If you specifically want the structured `tarn-mcp` *tool* surface inside pi, use
a community MCP adapter — see [Optional: tarn-mcp via an adapter](#optional-tarn-mcp-via-an-adapter)
below. We do not fabricate a native MCP config because pi has none.

## TL;DR

```
your-repo/
├── AGENTS.md                    # loaded by pi at startup (the repo ships one)
└── .agents/skills/
    └── tarn-api-testing/
        └── SKILL.md             # Agent Skills standard, read by pi
```

The Tarn repo ships the skill at [`.agents/skills/tarn-api-testing/`](../../.agents/skills/tarn-api-testing)
(symlinked to the canonical [`plugin/skills/tarn-api-testing/`](../../plugin/skills/tarn-api-testing/))
and an [`AGENTS.md`](../../AGENTS.md) at the root — clone it, run `pi` inside, and
the agent already knows Tarn.

## Prerequisites

1. **pi** installed (`pi --version`). See the
   [pi coding agent README](https://github.com/badlogic/pi-mono/tree/main/packages/coding-agent).
2. **`tarn`** binary on `$PATH`. From a checkout of [NazarKalytiuk/tarn](https://github.com/NazarKalytiuk/tarn):
   ```bash
   cargo install --path . --bin tarn
   ```
   Or symlink a workspace build:
   ```bash
   cargo build --release -p tarn
   ln -s "$(pwd)/target/release/tarn" ~/.local/bin/tarn
   ```
   (`tarn-mcp` is only needed for the optional adapter path below.)

## Wiring it into your own repo

### 1. Skill

pi discovers skills from `~/.pi/agent/skills/`, `~/.agents/skills/`, `.pi/skills/`,
and `.agents/skills/` (searched from the working directory up through parents),
plus pi packages and the `skills` settings array. Drop the Tarn skill at:

```
.agents/skills/tarn-api-testing/SKILL.md
```

Vendor the directory from this repo, or keep a git submodule pointing at
[`plugin/skills/tarn-api-testing/`](../../plugin/skills/tarn-api-testing/). The
skill's `name` (`tarn-api-testing`) already satisfies pi's naming rules
(lowercase, digits, single hyphens). Because `.agents/skills/` is a cross-agent
standard, this one directory also lights the skill up for Codex and opencode.

pi loads a skill automatically when the task matches, or you can invoke it
explicitly:

```
/skill:tarn-api-testing write a smoke test for GET /health and run it
```

### 2. AGENTS.md (recommended)

pi loads `AGENTS.md` (or `CLAUDE.md`) at startup from `~/.pi/agent/AGENTS.md`,
parent directories, and the current directory, and concatenates all matches. The
repo root [`AGENTS.md`](../../AGENTS.md) is a concise Tarn agent guide, so a pi
session in the repo knows the `validate → run → failures → fix` loop even before
the skill is loaded.

### Optional: tarn-mcp via an adapter

pi has no native MCP, but the community [`pi-mcp-adapter`](https://github.com/nicobailon/pi-mcp-adapter)
bridges standard MCP servers into pi as tools, reading the conventional
`.mcp.json` (or `.pi/mcp.json`) — the **same** file Tarn already ships at the
repo root:

```bash
pi install npm:pi-mcp-adapter   # then restart pi
```

```jsonc
// .mcp.json (repo root — already present in this repo)
{
  "mcpServers": {
    "tarn": { "command": "tarn-mcp", "args": [] }
  }
}
```

A copy lives at [`mcp.example.json`](./mcp.example.json). This requires
`tarn-mcp` on `$PATH`. The adapter is third-party — check its README for the
current install command and config precedence; the native skill + CLI path above
needs none of this.

## LSP note

pi is not an LSP client, so it does **not** consume `tarn-lsp`, and the `.yaml`
compound-extension caveat that applies to opencode and Claude Code does **not**
apply here. pi gets its Tarn intelligence from the skill and the structured
`tarn` CLI output.

## Verifying it works

In your repo with the skill in place:

1. Start `pi`. Run `/skill:tarn-api-testing` — it should load without error.
2. Ask: *"write a Tarn smoke test for GET /health and run it"*. pi should create
   a valid `.tarn.yaml` and run `tarn validate` / `tarn run` through its `bash`
   tool.
3. Confirm the CLI directly: `tarn --version` and `tarn list` resolve.

If you took the adapter path, ask pi to *"list available MCP tools"* — you
should see `tarn_run`, `tarn_validate`, `tarn_list`, `tarn_fix_plan`.

## End-to-end example

A reproducible write → run → read-failure → fix loop lives in
[`examples/agent-loop/`](../../examples/agent-loop/) — it hits the public
JSONPlaceholder API, so there is nothing to start. In a pi session:

> *"Run `examples/agent-loop/api.tarn.yaml`. Use `tarn run ... --format llm`,
> read the verdict line, and if anything fails run `tarn failures` and fix it."*

pi runs the commands through `bash`, reads the structured `llm`/`json` output,
and iterates. See [`examples/agent-loop/README.md`](../../examples/agent-loop/README.md)
for the full loop and a one-line way to make it fail on purpose.

## Troubleshooting

### The skill never loads

- `SKILL.md` must live at `.agents/skills/<name>/SKILL.md`; the directory name
  should match the `name:` frontmatter (`tarn-api-testing`).
- pi searches from the working directory upward — start pi at or below the repo
  root so `.agents/skills/` is on the path.
- Invoke it explicitly with `/skill:tarn-api-testing` to rule out implicit-match
  issues.

### `tarn: command not found`

- pi runs `tarn` through `bash`; the binary must be on the `$PATH` pi inherits.
  `which tarn` from the shell that launched pi should succeed.

### MCP tools missing (adapter path)

- Confirm the adapter is installed and pi was restarted.
- Confirm `which tarn-mcp` resolves and `.mcp.json` is discoverable from the
  working directory.

## References

- [pi coding agent README](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/README.md) — "No MCP" and the CLI-tools-with-READMEs philosophy
- [pi skills docs](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md) — skill discovery paths, `SKILL.md` frontmatter, naming rules
- [pi extensions docs](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/extensions.md)
- [pi-mcp-adapter](https://github.com/nicobailon/pi-mcp-adapter) — optional MCP bridge for pi
- [Tarn MCP workflow](../../docs/MCP_WORKFLOW.md)
- [Codex companion](../codex/README.md) · [opencode companion](../opencode/README.md)
