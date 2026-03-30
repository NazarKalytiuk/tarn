# Show HN Draft

## Title

Show HN: Tarn, a single-binary API test runner built for Claude/Cursor workflows

## Body

Tarn is a CLI-first API testing tool written in Rust.

- Tests are YAML (`.tarn.yaml`)
- Output is structured JSON for agents and CI
- Single binary, no runtime dependencies
- Includes an MCP server for Claude Code / Cursor / Windsurf

The loop we optimized for is:

1. agent writes a test
2. Tarn runs it
3. JSON failure output points to the exact mismatch
4. agent fixes the test or app code
5. rerun until green

Repo checklist before posting:

- release binaries uploaded
- `cargo install tarn` path decided and documented
- README quick start verified
- smoke CI green

Questions to expect:

- why not Hurl / Bruno / Postman?
- why no OpenAPI import yet?
- why no Windows yet?
- how safe is Lua scripting?
- what does MCP buy over `tarn run --format json`?
