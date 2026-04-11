//! `tarn-lsp` — Language Server Protocol implementation for Tarn test files.
//!
//! This binary speaks LSP over stdio. It is the editor-agnostic counterpart to
//! the VS Code extension in `editors/vscode`, and targets LSP clients such as
//! Claude Code, Neovim, Helix, and Emacs. See `docs/TARN_LSP.md`.

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    tarn_lsp::run()
}
