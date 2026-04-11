# Install Tarn

Tarn is a single static binary. Pick one:

```bash
# Homebrew
brew install NazarKalytiuk/tarn/tarn

# Cargo
cargo install tarn

# Install script
curl -fsSL https://raw.githubusercontent.com/NazarKalytiuk/hive/main/install.sh | sh
```

Verify with:

```bash
tarn --version
```

If the binary is not on your `PATH`, set `tarn.binaryPath` in the VS Code Settings UI to the absolute path.

## Required Tarn version

- **Minimum**: any Tarn with `--format json` (0.4.0+). The extension
  works against older Tarn and silently degrades the features listed
  below.
- **Recommended**: Tarn **0.6.0** or newer. This unlocks:
  - `--ndjson` streaming progress in the Test Explorer (Tarn T53)
  - `--select FILE::TEST::STEP` subset runs (Tarn T51)
  - Scoped `tarn list --file` discovery (Tarn T57)
  - Drift-free runtime result anchoring via the new `location`
    metadata on steps and assertions (Tarn T55 / NAZ-260). With this
    in place, the red squiggle on a failing assertion stays glued to
    the exact operator key even if you edit the file after the run
    started. Older Tarn versions still get anchoring, but via the
    editor's current YAML AST, which can drift under concurrent
    edits.

The extension never hard-gates on a Tarn version — it detects missing
features at run time and falls back gracefully.
