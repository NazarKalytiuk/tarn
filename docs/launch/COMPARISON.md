# Tarn Positioning Notes

Use this page when answering launch questions or writing comparison copy.

## Tarn vs Hurl

- Hurl is excellent for humans writing request files by hand.
- Tarn is optimized for AI-assisted loops: YAML the model already knows, structured failure JSON, and an MCP server.
- Tarn also combines API testing and lightweight benchmarking in one binary.

## Tarn vs Bruno CLI

- Bruno has a broader ecosystem, GUI workflows, and richer auth/import surfaces today.
- Tarn is smaller, single-binary, easier to drop into CI, and more focused on machine-readable execution output.
- Tarn's MCP story is materially stronger for Claude Code / Cursor workflows.

## Tarn vs StepCI

- StepCI is strong on OpenAPI-driven flows and schema-aware generation.
- Tarn is stronger when the starting point is "describe an endpoint and let the agent iterate".
- Tarn keeps the whole loop local and binary-first instead of Node-first.

## Talking Points

- "Hurl is great for handwritten HTTP specs; Tarn is for write-run-debug loops with AI agents."
- "Bruno is a broader API client platform; Tarn is a narrower CLI runner with a stronger machine-readable contract."
- "StepCI starts from specs; Tarn starts from executable tests the model can edit directly."
