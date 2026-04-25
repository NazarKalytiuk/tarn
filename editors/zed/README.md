# Tarn for Zed

[Tarn](https://github.com/NazarKalytiuk/tarn) is a CLI-first API testing tool. Tests live in `.tarn.yaml` files, run as a single `tarn run`, and produce structured JSON output designed for AI-assisted workflows.

This extension brings Tarn into [Zed](https://zed.dev):

- **Language server** — diagnostics, completion, hover, code actions, code lens, formatting, symbols, rename, references (powered by `tarn-lsp`).
- **Syntax highlighting** — `.tarn.yaml` / `.tarn.yml` files recognized as a distinct YAML dialect.
- **Snippets** — `tarn-test`, `tarn-step`, `tarn-capture`, `tarn-poll`, `tarn-form`, `tarn-graphql`, `tarn-multipart`, `tarn-lifecycle`, `tarn-include`.
- **Runnable tasks** — run / dry-run / validate the current file, run all tests, list all tests, directly from the command palette or task runner.

## Install

From Zed: open the Extensions panel (`cmd+shift+x` / `ctrl+shift+x`), search for `Tarn`, click Install.

The extension will auto-download the matching `tarn-lsp` binary from the [Tarn GitHub releases](https://github.com/NazarKalytiuk/tarn/releases) on first activation. No manual install required.

If you prefer to manage the binary yourself:

```sh
cargo install tarn-lsp
```

The extension uses this order to locate the server:

1. `lsp.tarn-lsp.binary.path` from your Zed `settings.json`
2. `tarn-lsp` on your `$PATH`
3. A cached download under Zed's extension work directory

## Running tests

You also need the `tarn` CLI on your `$PATH` to execute tests:

```sh
cargo install tarn
# or: curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/tarn/main/install.sh | sh
```

Open the task picker (`cmd+shift+p` → `task: spawn`) and pick a Tarn task:

- `tarn: run file` — run every test in the current file
- `tarn: dry-run file` — parse and plan without sending requests
- `tarn: validate file` — schema + referential validation
- `tarn: run all` / `tarn: list all` / `tarn: validate all` — whole-workspace variants

## Configuration

Put overrides in your Zed `settings.json`:

```json
{
  "lsp": {
    "tarn-lsp": {
      "binary": {
        "path": "/usr/local/bin/tarn-lsp",
        "arguments": []
      },
      "settings": {
        "trace": { "server": "off" }
      }
    }
  }
}
```

Anything inside `settings` is forwarded to `tarn-lsp` as `workspace/configuration` under the `tarn` namespace.

## Contributing

Source lives at [github.com/NazarKalytiuk/tarn/tree/main/editors/zed](https://github.com/NazarKalytiuk/tarn/tree/main/editors/zed). The public release repo is mirrored to [github.com/NazarKalytiuk/zed-tarn](https://github.com/NazarKalytiuk/zed-tarn) and submitted to the [zed-industries/extensions](https://github.com/zed-industries/extensions) registry.

### Local dev

```sh
cd editors/zed
cargo build --release --target wasm32-unknown-unknown
```

Then in Zed: `cmd+shift+p` → `zed: install dev extension` → pick `editors/zed/`.

### Release flow

Versioning is independent from the Tarn CLI. Bump `editors/zed/extension.toml` and `editors/zed/Cargo.toml` in lockstep, then tag:

```sh
git tag zed-v0.1.0
git push --tags
```

The tag triggers `.github/workflows/zed-mirror-release.yml`, which:

1. Verifies the tag matches both manifest versions.
2. Builds the WASM as a sanity check.
3. Mirrors the `editors/zed/` tree to [`NazarKalytiuk/zed-tarn`](https://github.com/NazarKalytiuk/zed-tarn) with a plain `vX.Y.Z` tag.

The mirror requires a `ZED_TARN_DEPLOY_TOKEN` secret (a PAT with `contents: write` on the mirror repo).

After the mirror runs, submit to the registry:

1. Fork [`zed-industries/extensions`](https://github.com/zed-industries/extensions).
2. Add the mirror as a submodule: `git submodule add https://github.com/NazarKalytiuk/zed-tarn.git extensions/tarn` and check out the release tag.
3. Add the `[tarn]` entry to `extensions.toml` with `submodule = "extensions/tarn"` and the matching version.
4. Open a PR; merge publishes the extension to Zed's marketplace.

## License

MIT — see [LICENSE](./LICENSE).
