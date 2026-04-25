# Docs Index

This index lists the canonical repository markdown after the roadmap cleanup.

## Canonical Documents

### Product and Strategy

- `docs/TARN_PRODUCT_STRATEGY.md`
  - product thesis
  - target audience
  - strategic non-goals
  - current risks and next investments

### Competitive Analysis

- `docs/TARN_VS_HURL_COMPARISON.md`
  - current Tarn vs Hurl decision guide
  - what parity is now practical
  - what remains intentionally out of scope
- `docs/HURL_MIGRATION.md`
  - Hurl to Tarn syntax map
  - practical migration notes
  - parity matrix and manual-rewrite boundaries

### Execution Roadmap

- `docs/TARN_COMPETITIVENESS_ROADMAP.md`
  - completed roadmap record
  - historical sequencing
  - intentionally deferred gaps

### Launch Notes

- `docs/LAUNCH_PLAYBOOK.md`

### AI Integration

- `.claude-plugin/plugin.json`
  - Claude Code plugin metadata (name, version, description, repository)
- `.claude-plugin/marketplace.json`
  - marketplace listing with owner info and plugin registry
- `plugin/skills/tarn-api-testing/SKILL.md`
  - Claude Code skill: teaches AI agents Tarn's workflow, commands, file format, and diagnosis loop
- `plugin/skills/tarn-api-testing/references/yaml-format.md`
  - complete `.tarn.yaml` schema reference
- `plugin/skills/tarn-api-testing/references/assertion-reference.md`
  - every assertion operator with examples
- `plugin/skills/tarn-api-testing/references/json-output.md`
  - structured JSON report schema and diagnosis algorithm
- `plugin/skills/tarn-api-testing/references/mcp-integration.md`
  - MCP server setup and tool reference for Claude Code, opencode, Cursor, and Windsurf
- `.mcp.json`
  - project-level MCP server configuration (portable across MCP-compatible tools)
- `docs/TARN_LSP.md`
  - canonical spec for `tarn-lsp`, the LSP 3.17 language server
  - Phase L1 (diagnostics, hover, completion, document symbols)
  - Phase L2 (go-to-definition, references, rename, code lens)
  - Phase L3 (formatting, code actions, quick-fix, nested completion, JSONPath evaluator)
  - editor install paths for Claude Code, Neovim, Helix, Zed, and others
- `editors/claude-code/tarn-lsp-plugin/README.md`
  - Claude Code plugin that wires `tarn-lsp` into Claude Code via its plugin/LSP system
  - shipped from the repo-root marketplace alongside the `tarn` MCP + skill plugin
  - installs via `/plugin marketplace add NazarKalytiuk/tarn` + `/plugin install tarn-lsp@tarn --scope project`
- `editors/opencode/README.md`
  - opencode integration: repo-root `opencode.jsonc` + `.opencode/skills/tarn-api-testing/`
  - config-driven (no marketplace / installer); skill reuses `plugin/skills/tarn-api-testing/` via symlink
- `editors/zed/README.md`
  - Zed extension: grammar, snippets, runnable tasks, and `tarn-lsp` adapter
  - auto-downloads `tarn-lsp` from GitHub releases on first activation
  - published via the [zed-industries/extensions](https://github.com/zed-industries/extensions) registry (source mirrored to `NazarKalytiuk/zed-tarn`)

### Workflow and Operations

- `docs/MCP_WORKFLOW.md`
  - MCP tool surface (`tarn_list`, `tarn_validate`, `tarn_run`, `tarn_fix_plan`) and the failures-first loop for MCP-driven debugging
- `docs/AI_WORKFLOW_DEMO.md`
  - walkthrough of `validate → run → failures → inspect → rerun --failed → diff` with a worked example
- `docs/TROUBLESHOOTING.md`
  - response-shape drift pattern (reopen-request incident) and the `failures` + `inspect` + `rerun --failed` recovery loop
  - route ordering trap and other common failure categories
- `docs/CONFORMANCE.md`
- `docs/RELEASE_VERIFICATION.md`
- `editors/vscode/README.md`
- `docs/VSCODE_EXTENSION.md`
  - Tarn VS Code extension architecture and Phase V migration plan
- `editors/vscode/docs/LSP_MIGRATION.md`
  - Phase V migration plan (direct-provider → `vscode-languageclient` → `tarn-lsp`)
  - dual-host migration, per-feature minor bumps, V2 ordering
  - rationale for `tarn.experimentalLspClient` being off by default in 0.6.x
- `editors/vscode/docs/API.md`
  - `TarnExtensionApi` public surface consumable via `vscode.extensions.getExtension('nazarkalytiuk.tarn-vscode').exports`
  - stability tiers and semver policy
- `README.md`
  - primary user-facing product and CLI guide
- `docs/site/index.html`
  - static docs site entrypoint
  - onboarding-oriented canonical guides

## Superseded Documents

The older pre-release drafts were consolidated or removed to reduce noise. In particular, the historical `spec.md` design draft is no longer canonical and has been deleted.
