# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tarn is a CLI-first API testing tool written in Rust. Tests are defined in `.tarn.yaml` files. Designed for AI-assisted workflows: an LLM writes tests, runs `tarn run`, parses structured JSON output, and iterates.

## Build & Run Commands

```bash
cargo build                    # dev build
cargo build --release          # release build (single binary, zero runtime deps)
cargo run -- run               # run all tests
cargo run -- run tests/x.tarn.yaml  # run specific test file
cargo run -- validate          # validate YAML without running
cargo run -- list              # dry run listing
cargo test                     # run all Rust tests
cargo test parser_test         # run a single test module
cargo clippy                   # lint
cargo fmt                      # format
```

## Architecture

The codebase follows a pipeline architecture: **parse YAML -> resolve env/variables -> execute HTTP -> assert responses -> report results**.

Key modules in `src/`:

- **model.rs** - Serde-derived Rust structs mirroring the YAML test format (TestFile, Step, Assertion, etc.)
- **parser.rs** - Loads `.tarn.yaml` files into `TestFile` structs
- **env.rs** - Environment variable resolution with priority chain: CLI `--var` > shell env > `tarn.env.local.yaml` > `tarn.env.{name}.yaml` > `tarn.env.yaml` > inline `env:` block
- **interpolation.rs** - `{{ env.x }}` and `{{ capture.x }}` template resolution across all string fields
- **runner.rs** - Orchestrator: load file -> resolve env -> run setup -> run tests -> run teardown
- **http.rs** - Request execution via reqwest (blocking initially)
- **capture.rs** - JSONPath extraction from responses for variable chaining between steps
- **assert/** - Assertion modules: status, headers, body (JSONPath), duration, types
- **report/** - Output formatters behind a Reporter trait: human (colored), json, junit, tap
- **builtin.rs** - Built-in functions: `$uuid`, `$random_hex(n)`, `$random_int(min,max)`, `$timestamp`, `$now_iso`
- **config.rs** - Optional `tarn.config.yaml` parsing
- **main.rs** - CLI entry point using clap (derive)

## Key Crates

| Purpose | Crate |
|---------|-------|
| CLI | `clap` (derive) |
| YAML | `serde` + `serde_yaml` |
| HTTP | `reqwest` (blocking) |
| JSONPath | `serde_json_path` |
| JSON Schema | `jsonschema` |
| Regex | `regex` |
| Colored output | `colored` |
| Diff | `similar` |
| Templates | `handlebars` or manual `{{ }}` |

## Test File Format

Files use `.tarn.yaml` extension. Minimal test:
```yaml
name: Health check
steps:
  - name: GET /health
    request:
      method: GET
      url: "{{ env.base_url }}/health"
    assert:
      status: 200
```

Full format supports: `env`, `defaults`, `setup`, `teardown`, `tests` (with `steps`), `capture`, and rich assertions (see spec.md for the complete assertion reference).

## Exit Codes

- 0: all tests passed
- 1: one or more tests failed
- 2: configuration/parse error
- 3: runtime error (network failure, timeout)

## Implementation Phases

The spec defines 6 ordered phases. Check `spec.md` "Implementation Plan" section for current phase targets. In summary:
1. Foundation (CLI skeleton, basic HTTP, status assertions)
2. Core assertions (body/JSONPath, headers, duration)
3. Variables and chaining (capture, env resolution, interpolation, built-ins)
4. Setup/teardown and orchestration (runner lifecycle, tag filtering, file discovery)
5. Reporters (JSON, JUnit, TAP output formats)
6. Polish (init, list, validate commands, JSON Schema, error messages)

## Design Decisions

- JSON output includes full request/response ONLY for failed steps (keeps output compact)
- Secrets in headers are redacted to `***` in output
- Assertions on the same JSONPath use AND logic (all must pass)
- Tests within a file run sequentially; steps within a test are sequential
- Each test is independent but steps within a test share captured variables
- Setup runs once before all tests; teardown runs even if tests fail


# Testing Strategy

You are acting as a senior QA engineer AND developer on this project. I am a solo developer — I write code, I test code, I ship code. There is no separate QA team. Tests are my only safety net.

## Core Philosophy
- Every piece of code must be tested before it's considered done
- Tests must catch real bugs, not just satisfy coverage metrics
- A test that can't fail is worthless — every assertion must be meaningful
- Test behavior, not implementation details

## What to Test (Priority Order)

### 1. Critical Path (MUST have)
- All public API endpoints: valid input, invalid input, auth/unauth, edge cases
- All service methods: happy path + every error branch
- All database operations: create, read, update, delete + constraint violations
- All business logic: calculations, state transitions, validations

### 2. Edge Cases (MUST have)
- Empty inputs, null, undefined
- Boundary values (0, -1, MAX_INT, empty string, very long string)
- Concurrent operations where applicable
- Malformed data, unexpected types

### 3. Error Handling (MUST have)
- Every catch block must be triggered by a test
- External service failures (DB down, API timeout, network error)
- Validation errors — test every validation rule
- Auth failures: expired token, wrong role, missing token

### 4. Integration Points (SHOULD have)
- Service-to-service communication
- Database queries with realistic data
- Message queue producers/consumers

## How to Write Tests

### Structure
- Use AAA pattern: Arrange → Act → Assert
- One logical assertion per test (multiple expect() is OK if testing one behavior)
- Test name must describe the scenario: `should return 404 when user does not exist`
- Group tests logically with describe blocks by method/feature

### Mocking Rules
- Mock external dependencies (DB, HTTP, message queues), NOT the unit under test
- Never mock what you're testing
- Use realistic mock data, not `{ foo: 'bar' }`
- Verify mock interactions (was the DB called with correct params?)

### Quality Checks
- Every test must fail if the corresponding code is removed/broken (mutation-resistant)
- No test should depend on another test's state (isolated)
- No hardcoded dates/times — use relative or frozen time
- No flaky patterns: no `setTimeout`, no reliance on execution order

## Coverage Requirements
- Aim for >90% line coverage on business logic / services / controllers
- 100% coverage on validators, guards, interceptors, pipes
- Every public method must have at least: 1 happy path + 1 error path test
- Every `if` branch must be covered
- Every `catch` block must be covered

## When Writing New Code
After implementing any feature or fixing a bug:
1. Write tests for the happy path first
2. Write tests for every error/edge case
3. Run all tests to make sure nothing is broken
4. If coverage for the changed file is <90%, add more tests

## When Writing Tests for Existing Code
When I ask you to "cover X with tests" or "add tests for this module":
1. First, READ the entire file and understand all branches/paths
2. List all scenarios that need testing (show me the plan)
3. Write ALL the tests — do not skip scenarios, do not say "similar tests can be added"
4. Run the tests. Fix any failures.
5. Report final coverage for the file.

## Anti-Patterns to AVOID
- ❌ `expect(result).toBeDefined()` alone — too weak, assert the actual value
- ❌ Testing private methods directly — test through public API
- ❌ Copy-paste tests with minor variations — use `test.each` / parameterized tests
- ❌ Snapshot tests for logic (OK for UI components only)
- ❌ Testing framework/library code — only test YOUR code
- ❌ Writing `// TODO: add more tests` — write them NOW or never
