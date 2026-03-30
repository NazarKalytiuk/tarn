# Changelog

## 0.1.0

- initial public Tarn release
- YAML-based API tests in `.tarn.yaml`
- structured JSON, JUnit, TAP, HTML, and human output
- setup/teardown, captures, cookies, includes, polling, retries, Lua scripting
- GraphQL support
- MCP server (`tarn-mcp`)
- benchmark mode (`tarn bench`)

## Unreleased

- runtime failures now emit structured JSON step results
- project-root env/config resolution fixed for scaffolded and nested test files
- Lua sandbox restricted with memory and instruction limits
- checksum verification added to installers
- release smoke test added to CI
