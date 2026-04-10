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
- `skills/tarn-api-testing/SKILL.md`
  - Claude Code skill: teaches AI agents Tarn's workflow, commands, file format, and diagnosis loop
- `skills/tarn-api-testing/references/yaml-format.md`
  - complete `.tarn.yaml` schema reference
- `skills/tarn-api-testing/references/assertion-reference.md`
  - every assertion operator with examples
- `skills/tarn-api-testing/references/json-output.md`
  - structured JSON report schema and diagnosis algorithm
- `skills/tarn-api-testing/references/mcp-integration.md`
  - MCP server setup and tool reference for Claude Code, Cursor, and Windsurf
- `.mcp.json`
  - project-level MCP server configuration (portable across MCP-compatible tools)

### Workflow and Operations

- `docs/MCP_WORKFLOW.md`
- `docs/AI_WORKFLOW_DEMO.md`
- `docs/CONFORMANCE.md`
- `docs/RELEASE_VERIFICATION.md`
- `editors/vscode/README.md`
- `docs/VSCODE_EXTENSION.md`
  - canonical spec for the Tarn VS Code extension
  - architecture, features, Tarn-side contract (§6.1–§6.7), phased roadmap
- `README.md`
  - primary user-facing product and CLI guide
- `docs/site/index.html`
  - static docs site entrypoint
  - onboarding-oriented canonical guides

## Superseded Documents

The older pre-release drafts were consolidated or removed to reduce noise. In particular, the historical `spec.md` design draft is no longer canonical and has been deleted.
