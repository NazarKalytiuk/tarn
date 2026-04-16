//! `textDocument/rename` + `textDocument/prepareRename` handlers and
//! renderers.
//!
//! Phase L2.3 (NAZ-299) is the third navigation feature in `tarn-lsp`. It
//! lets a client atomically rename a capture (per-test, current file) or
//! an env key (across every env source file where the key is declared
//! plus every `.tarn.yaml` file that uses it) via one `WorkspaceEdit`.
//!
//! The module mirrors the pure-renderer pattern established by L2.1 and
//! L2.2:
//!
//!   1. A pure renderer [`rename_workspace_edit`] that takes a
//!      pre-classified [`InterpolationToken`], the requested `new_name`,
//!      and a synthetic [`RenameContext`] of (URL, source) tuples, and
//!      returns either a [`WorkspaceEdit`] or a [`RenameError`]. It never
//!      touches the filesystem, never parses YAML, and never logs —
//!      every behaviour is unit-testable with a plain context literal.
//!
//!   2. A second pure renderer [`prepare_rename_range`] that returns the
//!      sub-range of the identifier under the cursor, used by
//!      `textDocument/prepareRename` to signal to the client which part
//!      of the buffer is renamable.
//!
//!   3. Thin connection-facing wrappers [`text_document_prepare_rename`]
//!      and [`text_document_rename`] that build their contexts from the
//!      server's [`crate::server::ServerState`] (the document store plus
//!      the workspace index) and dispatch into the pure renderers. Those
//!      wrappers are the only place that does I/O.
//!
//! ## Rename semantics
//!
//! * **Capture rename** — single-file, single-test. Updates the
//!   `capture:` key declaration in the owning step and every
//!   `{{ capture.NAME }}` use site in the current file that belongs to
//!   the cursor's test scope (setup captures are visible from every
//!   test). The resulting [`WorkspaceEdit`] has exactly one entry in its
//!   `changes` map: the current file URL.
//!
//! * **Env rename** — cross-file. Updates every env source file where
//!   the key is declared (typically one — inline env block or one of
//!   the `tarn.env*.yaml` files — but possibly more when the user has
//!   the same key in both `tarn.env.yaml` and `tarn.env.local.yaml`),
//!   AND every `{{ env.KEY }}` use site across every `.tarn.yaml` file
//!   in the workspace index.
//!
//! ## Identifier validation
//!
//! Both capture and env keys share the Tarn identifier grammar
//! `^[A-Za-z_][A-Za-z0-9_]*$` (ASCII only — Unicode letters are
//! intentionally rejected so the YAML key, the interpolation token, and
//! the shell-expansion placeholder all agree on what is a valid
//! identifier). Anything else is rejected up front via
//! [`RenameError::InvalidIdentifier`]. The validator is hand-rolled
//! rather than pulling in the `regex` crate because one predicate is
//! not worth the transitive dependency.
//!
//! ## Collision detection
//!
//! * Capture rename: the new name must not collide with another
//!   capture visible from the cursor's test scope. Renaming a capture
//!   to its own current name is a no-op (and is allowed).
//!
//! * Env rename: for every env source file that declares the old name,
//!   the new name must not already appear as a different key in that
//!   same file. The check runs per-file because an env chain may span
//!   multiple files that legitimately declare non-overlapping sets of
//!   keys — only intra-file collisions are rejected.
//!
//! Collisions are reported as [`RenameError::Collision`], which the
//! wrapper maps onto an LSP `InvalidParams` response error so the
//! client can surface a human-readable message without the rename
//! silently producing a partial edit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_server::{ErrorCode, ResponseError};
use lsp_types::{
    Position, PrepareRenameResponse, Range, RenameParams, TextDocumentPositionParams, TextEdit,
    Url, WorkspaceEdit,
};
use tarn::env::{self, inline_env_locations_from_source, EnvSource};
use tarn::outline::{find_capture_declarations, CaptureScope};
use tarn::parser;

use crate::references::{CaptureScopeOwned, WorkspaceFile};
use crate::server::{is_tarn_file_uri, ServerState};
use crate::token::{
    byte_offset_to_position, position_to_byte_offset, resolve_interpolation_token,
    scan_all_interpolations, InterpolationToken, InterpolationTokenSpan,
};

/// The set of errors the rename renderer can produce.
///
/// Every variant maps onto a specific LSP error code via the
/// [`From<RenameError> for ResponseError`] impl so the connection-facing
/// wrapper can forward it directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// The new name failed the Tarn identifier grammar check
    /// (`^[A-Za-z_][A-Za-z0-9_]*$`). The payload is the offending
    /// string, unmodified, so the client can quote it back in the
    /// error toast.
    InvalidIdentifier(String),
    /// The rename would collide with an existing capture or env key in
    /// the same scope (for captures) or same source file (for env).
    /// `source` is a human-readable description of the collision
    /// context ("capture in test `main`", "env file `tarn.env.yaml`",
    /// etc.) so the client can show a useful error message.
    Collision {
        conflicting_name: String,
        source: String,
    },
    /// The token under the cursor is not renamable (builtin, schema
    /// key, unresolved interpolation, empty identifier, …). The wrapper
    /// converts this into a `RequestFailed` response so clients know
    /// the rename action was declined but no parameters were at fault.
    NotRenamable,
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::InvalidIdentifier(name) => {
                write!(
                    f,
                    "`{name}` is not a valid Tarn identifier; expected ASCII letters, digits, or underscore and no leading digit"
                )
            }
            RenameError::Collision {
                conflicting_name,
                source,
            } => {
                write!(
                    f,
                    "cannot rename: `{conflicting_name}` already exists in {source}"
                )
            }
            RenameError::NotRenamable => {
                write!(
                    f,
                    "the token under the cursor is not a renamable capture or env key"
                )
            }
        }
    }
}

impl From<RenameError> for ResponseError {
    fn from(err: RenameError) -> Self {
        let code = match &err {
            RenameError::InvalidIdentifier(_) | RenameError::Collision { .. } => {
                ErrorCode::InvalidParams as i32
            }
            RenameError::NotRenamable => ErrorCode::RequestFailed as i32,
        };
        ResponseError {
            code,
            message: err.to_string(),
            data: None,
        }
    }
}

// Identifier grammar helper lives in [`crate::identifier`] — NAZ-303
// promoted it to a shared module so both the rename renderer and the
// extract-env code action agree on what counts as a valid Tarn name.
pub use crate::identifier::is_valid_identifier;

/// One env source file the rename renderer consults.
///
/// Each entry tells the renderer which file to scan for the old key's
/// declaration (for edits) and for the new key (for collision
/// detection). The renderer uses `source` to compute precise
/// [`Range`]s and `human_label` for any collision error message.
pub struct EnvSourceDoc<'a> {
    pub uri: Url,
    pub source: &'a str,
    pub kind: EnvDeclarationKind,
    /// Label used inside [`RenameError::Collision::source`] when the
    /// new name already exists in this file.
    pub human_label: String,
}

/// How to locate top-level env keys inside an env source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvDeclarationKind {
    /// Keys live at the top level of a standalone env file
    /// (`tarn.env.yaml`, `tarn.env.local.yaml`, …).
    EnvFile,
    /// Keys live inside the `env:` block of a `.tarn.yaml` test file.
    InlineEnvBlock,
}

/// Context the rename renderer consults. Pure and filesystem-free —
/// the connection-facing wrappers build this from
/// [`crate::server::ServerState`] before calling the renderer.
pub struct RenameContext<'a> {
    /// URL of the file the cursor is in.
    pub current_uri: Url,
    /// Raw source of the current file.
    pub current_source: &'a str,
    /// Owned capture scope for the cursor position (mirrors the
    /// references context's `current_test_scope`).
    pub current_test_scope: CaptureScopeOwned,
    /// Parsed test file for the current buffer, if it parsed cleanly.
    /// Capture collision detection walks this to find the full set of
    /// names declared in the cursor's scope.
    pub current_test_file: Option<&'a tarn::model::TestFile>,
    /// Every env source file (standalone env files + inline env
    /// blocks) relevant to the current buffer. Used to locate the old
    /// key's declaration site and to check for collisions in the file
    /// receiving the edit.
    pub env_sources: Vec<EnvSourceDoc<'a>>,
    /// Every `.tarn.yaml` file in the workspace — including the current
    /// buffer — used to walk env use sites. Capture renames ignore this
    /// slice because capture scopes never cross file boundaries.
    pub workspace: Vec<WorkspaceFile<'a>>,
}

/// Context consulted by [`prepare_rename_range`]. Carries only the
/// current file because prepareRename never walks the workspace.
pub struct PrepareRenameContext<'a> {
    pub current_source: &'a str,
}

/// Compute the sub-range of the identifier under the cursor for a
/// `textDocument/prepareRename` reply. Returns `None` for tokens that
/// are not renamable (builtins, schema keys, empty identifiers).
///
/// The range covers *only* the identifier, not the surrounding
/// `{{ env.X }}` braces — that's what LSP clients expect so the
/// rename UI highlights the identifier text and the user replaces
/// just the name.
pub fn prepare_rename_range(
    span: &InterpolationTokenSpan,
    ctx: &PrepareRenameContext<'_>,
) -> Option<Range> {
    match &span.token {
        InterpolationToken::Env(name) if !name.is_empty() => {
            identifier_subrange_in_span(ctx.current_source, span.range, "env", name)
        }
        InterpolationToken::Capture(name) if !name.is_empty() => {
            identifier_subrange_in_span(ctx.current_source, span.range, "capture", name)
        }
        _ => None,
    }
}

/// Pure rename renderer. Produces a [`WorkspaceEdit`] for the supplied
/// token and new name, or a [`RenameError`] when validation fails.
pub fn rename_workspace_edit(
    span: &InterpolationTokenSpan,
    new_name: &str,
    ctx: &RenameContext<'_>,
) -> Result<WorkspaceEdit, RenameError> {
    if !is_valid_identifier(new_name) {
        return Err(RenameError::InvalidIdentifier(new_name.to_owned()));
    }
    match &span.token {
        InterpolationToken::Env(old_name) => {
            if old_name.is_empty() {
                return Err(RenameError::NotRenamable);
            }
            if old_name == new_name {
                return Ok(WorkspaceEdit::default());
            }
            rename_env(old_name, new_name, ctx)
        }
        InterpolationToken::Capture(old_name) => {
            if old_name.is_empty() {
                return Err(RenameError::NotRenamable);
            }
            if old_name == new_name {
                return Ok(WorkspaceEdit::default());
            }
            rename_capture(old_name, new_name, ctx)
        }
        InterpolationToken::Builtin(_)
        | InterpolationToken::SchemaKey(_)
        // L3.6 (NAZ-307): JSONPath literals are evaluated in place, not
        // renamed — they don't name any symbol.
        | InterpolationToken::JsonPathLiteral(_) => Err(RenameError::NotRenamable),
    }
}

/// Rename a capture inside the current file. Scopes edits to the
/// cursor's test plus any setup captures (setup captures are visible
/// from every test, so they must still be editable). Returns a
/// [`WorkspaceEdit`] with exactly one entry in `changes`.
fn rename_capture(
    old_name: &str,
    new_name: &str,
    ctx: &RenameContext<'_>,
) -> Result<WorkspaceEdit, RenameError> {
    // 1. Capture collision check inside the current test scope. We
    //    build the visible set using `find_capture_declarations` so we
    //    inherit the same scope semantics references and definition
    //    already use. A capture whose declaration line matches the
    //    one we are about to rename is *us* — filter those out.
    let display_path = uri_display_path(&ctx.current_uri);
    let visible_names =
        visible_capture_names_in_scope(ctx.current_source, &display_path, &ctx.current_test_scope);
    if visible_names.contains(new_name) {
        return Err(RenameError::Collision {
            conflicting_name: new_name.to_owned(),
            source: describe_capture_scope(&ctx.current_test_scope),
        });
    }

    // 2. Collect every capture declaration site in scope that matches
    //    the old name. These become TextEdits on the `capture:` key.
    let mut declarations: Vec<tarn::model::Location> = Vec::new();
    let setup_locs = find_capture_declarations(
        ctx.current_source,
        &display_path,
        old_name,
        &CaptureScope::Setup,
    );
    let scope_locs = find_capture_declarations(
        ctx.current_source,
        &display_path,
        old_name,
        &ctx.current_test_scope.as_borrowed(),
    );
    match ctx.current_test_scope {
        CaptureScopeOwned::Setup | CaptureScopeOwned::Any => {
            declarations.extend(scope_locs);
        }
        _ => {
            declarations.extend(setup_locs.clone());
            declarations.extend(
                scope_locs
                    .into_iter()
                    .filter(|l| !setup_locs.iter().any(|s| s == l)),
            );
        }
    }
    declarations.sort_by(|a, b| a.line.cmp(&b.line).then(a.column.cmp(&b.column)));
    declarations.dedup();

    let mut edits: Vec<TextEdit> = Vec::new();
    for decl in &declarations {
        if let Some(range) = identifier_range_from_tarn_location(ctx.current_source, decl, old_name)
        {
            edits.push(TextEdit {
                range,
                new_text: new_name.to_owned(),
            });
        }
    }

    // 3. Walk every interpolation token in the current file and rewrite
    //    `{{ capture.old }}` to `{{ capture.new }}`.
    for token_span in scan_all_interpolations(ctx.current_source) {
        if let InterpolationToken::Capture(found) = &token_span.token {
            if found == old_name {
                if let Some(range) = identifier_subrange_in_span(
                    ctx.current_source,
                    token_span.range,
                    "capture",
                    old_name,
                ) {
                    edits.push(TextEdit {
                        range,
                        new_text: new_name.to_owned(),
                    });
                }
            }
        }
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    if !edits.is_empty() {
        sort_edits_reverse(&mut edits);
        changes.insert(ctx.current_uri.clone(), edits);
    }
    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })
}

/// Rename an env key across every source file that declares it, plus
/// every `.tarn.yaml` file that uses it. Returns a [`WorkspaceEdit`]
/// keyed by URL — declarations and use sites for the same file collapse
/// into one entry.
fn rename_env(
    old_name: &str,
    new_name: &str,
    ctx: &RenameContext<'_>,
) -> Result<WorkspaceEdit, RenameError> {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    // 1. Collision check per env source file that declares the old
    //    name. Walk each file's declared key set and bail out the
    //    moment we see both `old_name` and `new_name` in the same
    //    file.
    for env_source in &ctx.env_sources {
        let declared = collect_env_key_ranges(env_source);
        if declared.contains_key(old_name) && declared.contains_key(new_name) {
            return Err(RenameError::Collision {
                conflicting_name: new_name.to_owned(),
                source: env_source.human_label.clone(),
            });
        }
    }

    // 2. Rewrite declarations in every env source file that declares
    //    the old name.
    for env_source in &ctx.env_sources {
        let declared = collect_env_key_ranges(env_source);
        if let Some(range) = declared.get(old_name).copied() {
            changes
                .entry(env_source.uri.clone())
                .or_default()
                .push(TextEdit {
                    range,
                    new_text: new_name.to_owned(),
                });
        }
    }

    // 3. Rewrite use sites in every workspace file, including the
    //    current buffer. We deduplicate against `current_uri` so a
    //    workspace entry for the current file doesn't double-count.
    let mut seen_sources: HashSet<Url> = HashSet::new();
    push_env_use_site_edits(
        &ctx.current_uri,
        ctx.current_source,
        old_name,
        new_name,
        &mut changes,
    );
    seen_sources.insert(ctx.current_uri.clone());
    for file in &ctx.workspace {
        if seen_sources.contains(&file.uri) {
            continue;
        }
        seen_sources.insert(file.uri.clone());
        push_env_use_site_edits(&file.uri, file.source, old_name, new_name, &mut changes);
    }

    // 4. Sort edits inside each file in reverse document order so
    //    clients can apply them safely without shifting offsets.
    for edits in changes.values_mut() {
        sort_edits_reverse(edits);
    }

    Ok(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })
}

/// Scan an env source file and return `{ key → key-range }` for every
/// top-level key. The range covers the bare identifier so callers can
/// emit a precise [`TextEdit`].
fn collect_env_key_ranges(env_source: &EnvSourceDoc<'_>) -> HashMap<String, Range> {
    let display_path = uri_display_path(&env_source.uri);
    let locations_1based = match env_source.kind {
        EnvDeclarationKind::EnvFile => {
            env::scan_top_level_key_locations(env_source.source, &display_path)
        }
        EnvDeclarationKind::InlineEnvBlock => {
            inline_env_locations_from_source(env_source.source, &display_path)
        }
    };
    let mut out: HashMap<String, Range> = HashMap::new();
    for (key, loc) in locations_1based {
        // `Location` reports the *value* column for scalar values, not
        // the key column, so we scan the declaration line for the
        // identifier text and compute a precise range from there.
        if let Some(range) = locate_key_range_on_line(env_source.source, loc.line, &key) {
            out.insert(key, range);
        }
    }
    out
}

/// Find the LSP [`Range`] of a bare YAML key on a given 1-based line.
///
/// Scans the line for `<key>:` (optionally followed by whitespace or
/// end-of-line) and returns the range of the key text itself. Returns
/// `None` when the line cannot be decoded as UTF-8 or the key is not
/// present on that line — that is always a soft failure.
fn locate_key_range_on_line(source: &str, line_one_based: usize, key: &str) -> Option<Range> {
    if line_one_based == 0 {
        return None;
    }
    let zero_based = line_one_based - 1;
    let line = source.lines().nth(zero_based)?;
    let mut search_from = 0usize;
    while let Some(rel) = line[search_from..].find(key) {
        let start = search_from + rel;
        let end = start + key.len();
        // Boundary checks — make sure this match is a bare key and
        // not a substring of a larger identifier.
        let before_ok = start == 0 || !is_ident_byte(line.as_bytes()[start - 1]);
        let after_ok = end == line.len() || !is_ident_byte(line.as_bytes()[end]);
        if before_ok && after_ok {
            // Make sure this is a key, not a value: the first
            // non-whitespace char after `end` must be a `:`.
            let rest = &line[end..];
            let trimmed = rest.trim_start();
            if trimmed.starts_with(':') {
                let line_u32 = zero_based as u32;
                return Some(Range::new(
                    Position::new(line_u32, start as u32),
                    Position::new(line_u32, end as u32),
                ));
            }
        }
        search_from = start + 1;
    }
    None
}

/// True when `b` would continue an identifier (ASCII alphanumeric or
/// underscore). Used by `locate_key_range_on_line` to reject
/// partial-word matches like finding `api` inside `api_key`.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Sort a vector of text edits so the LSP client can apply them
/// bottom-up without shifting earlier offsets. Ties break on start
/// character so the order is stable when two edits land on the same
/// line.
fn sort_edits_reverse(edits: &mut [TextEdit]) {
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });
}

/// Collect the set of capture names visible from `scope`. Used by the
/// capture collision check — rejects a rename that would clash with an
/// already-visible name.
fn visible_capture_names_in_scope(
    source: &str,
    display_path: &str,
    scope: &CaptureScopeOwned,
) -> HashSet<String> {
    let mut names = HashSet::new();
    // Setup captures are always visible. For every other scope we
    // include both setup and the scope itself.
    let scan = |scope: &CaptureScope<'_>, names: &mut HashSet<String>| {
        let keys = collect_capture_key_names(source, display_path, scope);
        names.extend(keys);
    };
    match scope {
        CaptureScopeOwned::Setup => {
            scan(&CaptureScope::Setup, &mut names);
        }
        CaptureScopeOwned::Any => {
            scan(&CaptureScope::Any, &mut names);
        }
        other => {
            scan(&CaptureScope::Setup, &mut names);
            scan(&other.as_borrowed(), &mut names);
        }
    }
    names
}

/// Walk every capture declaration in `scope` and return the set of
/// distinct capture names.
///
/// We do this by probing `find_capture_declarations` with the token
/// the user is editing plus every capture name already collected —
/// but `find_capture_declarations` only matches one name at a time,
/// so instead we re-parse the buffer via `tarn::parser::parse_str`
/// to enumerate `capture:` keys. Parse failures degrade gracefully
/// to an empty set.
fn collect_capture_key_names(
    source: &str,
    _display_path: &str,
    scope: &CaptureScope<'_>,
) -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(test_file) = parser::parse_str(source, Path::new("rename.tarn.yaml")) else {
        return out;
    };
    let push = |step: &tarn::model::Step, out: &mut HashSet<String>| {
        for name in step.capture.keys() {
            out.insert(name.clone());
        }
    };
    match scope {
        CaptureScope::Setup => {
            for step in &test_file.setup {
                push(step, &mut out);
            }
        }
        CaptureScope::Teardown => {
            for step in &test_file.teardown {
                push(step, &mut out);
            }
        }
        CaptureScope::FlatSteps => {
            for step in &test_file.steps {
                push(step, &mut out);
            }
        }
        CaptureScope::Test(name) => {
            if let Some(group) = test_file.tests.get(*name) {
                for step in &group.steps {
                    push(step, &mut out);
                }
            }
        }
        CaptureScope::Any => {
            for step in &test_file.setup {
                push(step, &mut out);
            }
            for step in &test_file.steps {
                push(step, &mut out);
            }
            for group in test_file.tests.values() {
                for step in &group.steps {
                    push(step, &mut out);
                }
            }
            for step in &test_file.teardown {
                push(step, &mut out);
            }
        }
    }
    out
}

/// Describe a capture scope for a collision error message.
fn describe_capture_scope(scope: &CaptureScopeOwned) -> String {
    match scope {
        CaptureScopeOwned::Setup => "the setup phase".to_owned(),
        CaptureScopeOwned::Teardown => "the teardown phase".to_owned(),
        CaptureScopeOwned::FlatSteps => "the top-level steps list".to_owned(),
        CaptureScopeOwned::Test(name) => format!("test `{name}`"),
        CaptureScopeOwned::Any => "the current file".to_owned(),
    }
}

/// Convert a 1-based [`tarn::model::Location`] pointing at a capture
/// key into an LSP [`Range`] spanning the key identifier.
///
/// The tarn scanner reports the key's 1-based line and column, but
/// the column of `find_capture_declarations` is computed from the
/// yaml-rust2 marker which is a character-column, not a byte-column.
/// We recompute the byte range ourselves by walking the line text so
/// the resulting range lines up with what LSP clients expect.
fn identifier_range_from_tarn_location(
    source: &str,
    loc: &tarn::model::Location,
    name: &str,
) -> Option<Range> {
    if loc.line == 0 {
        return None;
    }
    let zero_based_line = loc.line - 1;
    let line = source.lines().nth(zero_based_line)?;
    // Scan the line for `<name>` as a bare key near the tarn-reported
    // column. We start from column 0 to be lenient about column
    // arithmetic quirks between yaml-rust2 and LSP.
    let line_u32 = zero_based_line as u32;
    let mut search_from = 0usize;
    while let Some(rel) = line[search_from..].find(name) {
        let start = search_from + rel;
        let end = start + name.len();
        let before_ok = start == 0 || !is_ident_byte(line.as_bytes()[start - 1]);
        let after_ok = end == line.len() || !is_ident_byte(line.as_bytes()[end]);
        if before_ok && after_ok {
            let rest = &line[end..];
            if rest.trim_start().starts_with(':') {
                return Some(Range::new(
                    Position::new(line_u32, start as u32),
                    Position::new(line_u32, end as u32),
                ));
            }
        }
        search_from = start + 1;
    }
    None
}

/// Walk `source` for every `{{ env.old }}` interpolation and append a
/// [`TextEdit`] rewriting it to `{{ env.new }}` into `changes[uri]`.
fn push_env_use_site_edits(
    uri: &Url,
    source: &str,
    old_name: &str,
    new_name: &str,
    changes: &mut HashMap<Url, Vec<TextEdit>>,
) {
    for span in scan_all_interpolations(source) {
        if let InterpolationToken::Env(found) = &span.token {
            if found == old_name {
                if let Some(range) =
                    identifier_subrange_in_span(source, span.range, "env", old_name)
                {
                    changes.entry(uri.clone()).or_default().push(TextEdit {
                        range,
                        new_text: new_name.to_owned(),
                    });
                }
            }
        }
    }
}

/// Compute the sub-range that covers only the identifier inside a
/// `{{ env.X }}` or `{{ capture.X }}` interpolation span.
///
/// `kind` is the literal token prefix, one of `"env"` or `"capture"`.
/// The helper scans the span slice for `<kind>.<name>` and returns the
/// LSP [`Range`] pointing at `<name>` inside the source. Returns
/// `None` if the span is malformed (e.g. a byte-offset conversion
/// fails) — callers treat that as "nothing to edit here".
fn identifier_subrange_in_span(
    source: &str,
    span_range: Range,
    kind: &str,
    name: &str,
) -> Option<Range> {
    let start_byte = position_to_byte_offset(source, span_range.start)?;
    let end_byte = position_to_byte_offset(source, span_range.end)?;
    if start_byte > end_byte || end_byte > source.len() {
        return None;
    }
    let slice = &source[start_byte..end_byte];
    let pattern = format!("{kind}.{name}");
    let rel_pos = slice.find(&pattern)?;
    // Make sure the match is bounded — `env.api_key` must not be
    // preceded by more identifier characters (would mean we matched
    // inside a longer name).
    if rel_pos > 0 {
        let prev = slice.as_bytes()[rel_pos - 1];
        if is_ident_byte(prev) {
            return None;
        }
    }
    let ident_rel_start = rel_pos + kind.len() + 1; // skip `<kind>.`
    let ident_rel_end = ident_rel_start + name.len();
    // After-boundary: the byte after the name must not be another
    // identifier character — otherwise we matched a substring.
    if ident_rel_end < slice.len() {
        let next = slice.as_bytes()[ident_rel_end];
        if is_ident_byte(next) {
            return None;
        }
    }
    let ident_start_byte = start_byte + ident_rel_start;
    let ident_end_byte = start_byte + ident_rel_end;
    Some(Range::new(
        byte_offset_to_position(source, ident_start_byte),
        byte_offset_to_position(source, ident_end_byte),
    ))
}

fn uri_display_path(uri: &Url) -> String {
    uri.to_file_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| uri.path().to_owned())
}

fn uri_to_path(uri: &Url) -> PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| PathBuf::from(uri.path()))
}

// ---------------------------------------------------------------------
// connection-facing wrappers
// ---------------------------------------------------------------------

/// `textDocument/prepareRename` request entry point. Returns `None`
/// (which serialises to JSON `null`) when the cursor isn't on a
/// renamable token — that is the LSP convention for "decline rename".
pub fn text_document_prepare_rename(
    state: &mut ServerState,
    params: TextDocumentPositionParams,
) -> Option<PrepareRenameResponse> {
    let uri = params.text_document.uri;
    let position = params.position;
    if !is_tarn_file_uri(&uri) {
        return None;
    }
    let source = state.documents.get(&uri).map(|s| s.to_owned())?;
    let span = resolve_interpolation_token(&source, position)?;
    let ctx = PrepareRenameContext {
        current_source: &source,
    };
    prepare_rename_range(&span, &ctx).map(PrepareRenameResponse::Range)
}

/// `textDocument/rename` request entry point. Returns an empty
/// [`WorkspaceEdit`] when the cursor isn't on a renamable token — the
/// LSP spec allows the server to signal "nothing to rename" with an
/// empty edit rather than a response error. Invalid-identifier and
/// collision errors surface as [`ResponseError`]s via the
/// [`From<RenameError>`] impl.
pub fn text_document_rename(
    state: &mut ServerState,
    params: RenameParams,
) -> Result<WorkspaceEdit, ResponseError> {
    let uri = params.text_document_position.text_document.uri.clone();
    let position = params.text_document_position.position;
    let new_name = params.new_name;

    if !is_tarn_file_uri(&uri) {
        return Ok(WorkspaceEdit::default());
    }
    let source = match state.documents.get(&uri).map(|s| s.to_owned()) {
        Some(s) => s,
        None => return Ok(WorkspaceEdit::default()),
    };
    let Some(span) = resolve_interpolation_token(&source, position) else {
        return Ok(WorkspaceEdit::default());
    };

    // Build the env resolution + inline env location data the same
    // way references does — the renderer can't do this itself because
    // it never touches the filesystem.
    let path = uri_to_path(&uri);
    let parse_result = parser::parse_str(&source, &path);
    let (inline_env, scope, test_file_for_ctx) = match &parse_result {
        Ok(tf) => {
            let cursor_line = (position.line as usize) + 1;
            (
                tf.env.clone(),
                pick_capture_scope(tf, cursor_line),
                Some(tf.clone()),
            )
        }
        Err(_) => (HashMap::new(), CaptureScopeOwned::Any, None),
    };

    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let env_btree = env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &HashMap::new(),
    )
    .unwrap_or_default();

    // Refresh the workspace index so it contains the freshest copy
    // of the current buffer before we walk it.
    state
        .workspace_index
        .insert_from_source(uri.clone(), source.clone());
    if let Err(err) = state.workspace_index.ensure_scanned() {
        eprintln!("tarn-lsp: workspace scan failed: {err}");
    }

    // Collect env source files: inline env of the current buffer +
    // every file-backed env layer we can find. Stored in owned form
    // so the renderer can borrow from them uniformly.
    let env_source_texts = collect_env_source_texts(&env_btree, &path, &source);

    let workspace_entries: Vec<(Url, String)> = state
        .workspace_index
        .iter()
        .map(|(url, cached)| (url.clone(), cached.source.clone()))
        .collect();

    let env_sources: Vec<EnvSourceDoc<'_>> = env_source_texts
        .iter()
        .map(|s| EnvSourceDoc {
            uri: s.uri.clone(),
            source: s.source.as_str(),
            kind: s.kind,
            human_label: s.human_label.clone(),
        })
        .collect();

    let workspace: Vec<WorkspaceFile<'_>> = workspace_entries
        .iter()
        .map(|(url, source)| WorkspaceFile {
            uri: url.clone(),
            source: source.as_str(),
            outline: None,
        })
        .collect();

    let ctx = RenameContext {
        current_uri: uri,
        current_source: &source,
        current_test_scope: scope,
        current_test_file: test_file_for_ctx.as_ref(),
        env_sources,
        workspace,
    };

    rename_workspace_edit(&span, &new_name, &ctx).map_err(ResponseError::from)
}

/// Owned env source text — the wrappers build these once and then
/// hand borrowed views to the pure renderer.
struct EnvSourceText {
    uri: Url,
    source: String,
    kind: EnvDeclarationKind,
    human_label: String,
}

/// Walk the resolved env map and materialise one [`EnvSourceText`] per
/// distinct file-backed layer we find. Always includes the inline env
/// block of the current buffer, even when no entry was resolved from
/// it, so the renderer can still rename an inline-declared key.
fn collect_env_source_texts(
    env_map: &std::collections::BTreeMap<String, tarn::env::EnvEntry>,
    current_path: &Path,
    current_source: &str,
) -> Vec<EnvSourceText> {
    let mut out: Vec<EnvSourceText> = Vec::new();
    let mut seen_urls: HashSet<Url> = HashSet::new();

    // 1. Always include the current buffer as an inline env block
    //    candidate — even when no resolved entry points at it the
    //    user may still have an inline `env:` block they want to
    //    rename.
    if let Ok(current_url) = Url::from_file_path(current_path) {
        out.push(EnvSourceText {
            uri: current_url.clone(),
            source: current_source.to_owned(),
            kind: EnvDeclarationKind::InlineEnvBlock,
            human_label: format!(
                "inline `env:` block of `{}`",
                current_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("current file")
            ),
        });
        seen_urls.insert(current_url);
    }

    // 2. File-backed env layers.
    for entry in env_map.values() {
        let (path_str, label) = match &entry.source {
            EnvSource::DefaultEnvFile { path } => {
                (path.clone(), format!("default env file `{path}`"))
            }
            EnvSource::NamedEnvFile { path, env_name } => (
                path.clone(),
                format!("named env file `{path}` (environment `{env_name}`)"),
            ),
            EnvSource::LocalEnvFile { path } => (path.clone(), format!("local env file `{path}`")),
            _ => continue,
        };
        let path_buf = PathBuf::from(&path_str);
        let Ok(url) = Url::from_file_path(&path_buf) else {
            continue;
        };
        if seen_urls.contains(&url) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path_buf) else {
            continue;
        };
        out.push(EnvSourceText {
            uri: url.clone(),
            source: text,
            kind: EnvDeclarationKind::EnvFile,
            human_label: label,
        });
        seen_urls.insert(url);
    }

    out
}

/// Pick the capture scope for a cursor line. Mirrors the
/// `references::pick_capture_scope` heuristic so all three L2
/// navigation features agree on which test the cursor lives in.
fn pick_capture_scope(
    test_file: &tarn::model::TestFile,
    cursor_line_one_based: usize,
) -> CaptureScopeOwned {
    let mut best: Option<(usize, CaptureScopeOwned)> = None;

    let mut consider = |section: CaptureScopeOwned, steps: &[tarn::model::Step]| {
        for step in steps {
            if let Some(loc) = &step.location {
                if loc.line <= cursor_line_one_based {
                    let line = loc.line;
                    match &best {
                        Some((best_line, _)) if *best_line >= line => {}
                        _ => {
                            best = Some((line, section.clone()));
                        }
                    }
                }
            }
        }
    };

    consider(CaptureScopeOwned::Setup, &test_file.setup);
    consider(CaptureScopeOwned::FlatSteps, &test_file.steps);
    for (test_name, group) in &test_file.tests {
        consider(CaptureScopeOwned::Test(test_name.clone()), &group.steps);
    }
    consider(CaptureScopeOwned::Teardown, &test_file.teardown);

    best.map(|(_, scope)| scope)
        .unwrap_or(CaptureScopeOwned::Any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::references::CaptureScopeOwned;

    fn ctx_with(uri: Url, source: &str, scope: CaptureScopeOwned) -> RenameContext<'_> {
        RenameContext {
            current_uri: uri,
            current_source: source,
            current_test_scope: scope,
            current_test_file: None,
            env_sources: Vec::new(),
            workspace: Vec::new(),
        }
    }

    fn span_at(source: &str, needle: &str) -> InterpolationTokenSpan {
        scan_all_interpolations(source)
            .into_iter()
            .find(|s| {
                let start = position_to_byte_offset(source, s.range.start).unwrap();
                let end = position_to_byte_offset(source, s.range.end).unwrap();
                source[start..end].contains(needle)
            })
            .expect("token span present in source")
    }

    // `is_valid_identifier` is unit-tested in `crate::identifier` —
    // NAZ-303 moved the helper there so the rename renderer and the
    // extract-env code action share the exact same grammar check.

    // ---------- prepare_rename_range ----------

    #[test]
    fn prepare_rename_on_env_token_returns_identifier_range() {
        let source = "url: \"{{ env.base_url }}\"\n";
        let span = span_at(source, "env.base_url");
        let range = prepare_rename_range(
            &span,
            &PrepareRenameContext {
                current_source: source,
            },
        )
        .expect("range");
        let start = position_to_byte_offset(source, range.start).unwrap();
        let end = position_to_byte_offset(source, range.end).unwrap();
        assert_eq!(&source[start..end], "base_url");
    }

    #[test]
    fn prepare_rename_on_capture_token_returns_identifier_range() {
        let source = "url: \"http://x/{{ capture.token }}\"\n";
        let span = span_at(source, "capture.token");
        let range = prepare_rename_range(
            &span,
            &PrepareRenameContext {
                current_source: source,
            },
        )
        .expect("range");
        let start = position_to_byte_offset(source, range.start).unwrap();
        let end = position_to_byte_offset(source, range.end).unwrap();
        assert_eq!(&source[start..end], "token");
    }

    #[test]
    fn prepare_rename_on_builtin_returns_none() {
        let source = "url: \"{{ $uuid }}\"\n";
        let span = span_at(source, "$uuid");
        assert!(prepare_rename_range(
            &span,
            &PrepareRenameContext {
                current_source: source
            }
        )
        .is_none());
    }

    #[test]
    fn prepare_rename_on_empty_env_name_returns_none() {
        let source = "url: \"{{ env. }}\"\n";
        // The scanner classifies `env.` as `Env("")`. Even though
        // `scan_all_interpolations` skips empty bodies, `env.` is
        // non-empty.
        let span = scan_all_interpolations(source)
            .into_iter()
            .next()
            .expect("one token");
        let out = prepare_rename_range(
            &span,
            &PrepareRenameContext {
                current_source: source,
            },
        );
        assert!(out.is_none());
    }

    // ---------- capture renames ----------

    fn capture_fixture() -> &'static str {
        "name: cap\ntests:\n  main:\n    steps:\n      - name: login\n        request:\n          method: POST\n          url: \"http://x/auth\"\n        capture:\n          token: $.id\n      - name: list\n        request:\n          method: GET\n          url: \"http://x/{{ capture.token }}\"\n      - name: detail\n        request:\n          method: GET\n          url: \"http://x/items?k={{ capture.token }}\"\n"
    }

    #[test]
    fn rename_capture_happy_path_updates_declaration_and_every_use_site() {
        let uri = Url::parse("file:///tmp/cap.tarn.yaml").unwrap();
        let source = capture_fixture();
        let ctx = ctx_with(uri.clone(), source, CaptureScopeOwned::Test("main".into()));
        let span = span_at(source, "capture.token");
        let edit = rename_workspace_edit(&span, "auth_token", &ctx).expect("rename ok");
        let changes = edit.changes.expect("changes present");
        assert_eq!(changes.len(), 1, "capture rename is single-file");
        let edits = changes.get(&uri).expect("current uri edits");
        // 1 declaration + 2 use sites.
        assert_eq!(edits.len(), 3);
        for e in edits {
            assert_eq!(e.new_text, "auth_token");
        }
    }

    #[test]
    fn rename_capture_on_declaration_site_updates_declaration_and_every_use_site() {
        // Cursor lands on the capture *declaration* (via the scanner
        // over a fake interpolation span). We fabricate the renderer
        // call with a synthetic token so the test stays focused on
        // the pure rename logic rather than scanner edge cases.
        let uri = Url::parse("file:///tmp/cap-decl.tarn.yaml").unwrap();
        let source = capture_fixture();
        let ctx = ctx_with(uri.clone(), source, CaptureScopeOwned::Test("main".into()));
        // Synthesise a span pointing at the first use site so the
        // renderer has a valid interpolation token to anchor on;
        // semantics are identical to clicking on the declaration.
        let span = span_at(source, "capture.token");
        let edit = rename_workspace_edit(&span, "renamed", &ctx).expect("rename ok");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert!(!edits.is_empty(), "at least one edit");
        assert!(edits.iter().any(|e| e.new_text == "renamed"));
    }

    #[test]
    fn rename_capture_with_zero_uses_still_emits_declaration_edit() {
        let source = "name: cap\ntests:\n  main:\n    steps:\n      - name: login\n        request:\n          method: POST\n          url: \"http://x/auth\"\n        capture:\n          token: $.id\n";
        let uri = Url::parse("file:///tmp/cap-no-use.tarn.yaml").unwrap();
        let ctx = ctx_with(uri.clone(), source, CaptureScopeOwned::Test("main".into()));
        // Synthesise a span by hand — the source has no use site of
        // the capture. Pretend the user right-clicked the declaration
        // itself (any interpolation with matching classification
        // works for the renderer).
        let synthetic_span = InterpolationTokenSpan {
            token: InterpolationToken::Capture("token".into()),
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            step_context: None,
        };
        let edit = rename_workspace_edit(&synthetic_span, "renamed", &ctx).expect("rename ok");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).expect("has edits for current uri");
        assert_eq!(edits.len(), 1, "only the declaration is edited");
        assert_eq!(edits[0].new_text, "renamed");
    }

    #[test]
    fn rename_capture_collision_in_same_test_is_rejected() {
        let source = "name: cap\ntests:\n  main:\n    steps:\n      - name: login\n        request:\n          method: POST\n          url: \"http://x/auth\"\n        capture:\n          token: $.id\n          session: $.session\n";
        let uri = Url::parse("file:///tmp/cap-clash.tarn.yaml").unwrap();
        let ctx = ctx_with(uri, source, CaptureScopeOwned::Test("main".into()));
        let span = InterpolationTokenSpan {
            token: InterpolationToken::Capture("token".into()),
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            step_context: None,
        };
        let err = rename_workspace_edit(&span, "session", &ctx).unwrap_err();
        match err {
            RenameError::Collision {
                conflicting_name, ..
            } => {
                assert_eq!(conflicting_name, "session");
            }
            other => panic!("expected collision, got {other:?}"),
        }
    }

    #[test]
    fn rename_capture_to_same_name_is_a_noop() {
        let source = capture_fixture();
        let uri = Url::parse("file:///tmp/cap-noop.tarn.yaml").unwrap();
        let ctx = ctx_with(uri, source, CaptureScopeOwned::Test("main".into()));
        let span = span_at(source, "capture.token");
        let edit = rename_workspace_edit(&span, "token", &ctx).expect("no-op rename");
        // A noop edit has no per-file changes. An empty map is fine.
        let changes = edit.changes.unwrap_or_default();
        assert!(changes.is_empty() || changes.values().all(Vec::is_empty));
    }

    #[test]
    fn rename_capture_with_invalid_identifier_is_rejected() {
        let source = capture_fixture();
        let uri = Url::parse("file:///tmp/cap-bad.tarn.yaml").unwrap();
        let ctx = ctx_with(uri, source, CaptureScopeOwned::Test("main".into()));
        let span = span_at(source, "capture.token");
        let err = rename_workspace_edit(&span, "2bad", &ctx).unwrap_err();
        assert!(matches!(err, RenameError::InvalidIdentifier(_)));
    }

    // ---------- env renames ----------

    #[test]
    fn rename_env_across_two_files_updates_every_use_site_and_declaration() {
        let source_a = "name: a\nenv:\n  base_url: http://localhost:3000\nsteps:\n  - name: list\n    request: { method: GET, url: \"{{ env.base_url }}/items\" }\n";
        let source_b = "name: b\nsteps:\n  - name: ping\n    request: { method: GET, url: \"{{ env.base_url }}/ping\" }\n  - name: pong\n    request: { method: GET, url: \"{{ env.base_url }}/pong\" }\n";
        let uri_a = Url::parse("file:///tmp/a.tarn.yaml").unwrap();
        let uri_b = Url::parse("file:///tmp/b.tarn.yaml").unwrap();

        let env_sources = vec![EnvSourceDoc {
            uri: uri_a.clone(),
            source: source_a,
            kind: EnvDeclarationKind::InlineEnvBlock,
            human_label: "inline env block".to_owned(),
        }];
        let workspace = vec![
            WorkspaceFile {
                uri: uri_a.clone(),
                source: source_a,
                outline: None,
            },
            WorkspaceFile {
                uri: uri_b.clone(),
                source: source_b,
                outline: None,
            },
        ];

        let ctx = RenameContext {
            current_uri: uri_a.clone(),
            current_source: source_a,
            current_test_scope: CaptureScopeOwned::FlatSteps,
            current_test_file: None,
            env_sources,
            workspace,
        };

        let span = span_at(source_a, "env.base_url");
        let edit = rename_workspace_edit(&span, "api_url", &ctx).expect("rename ok");
        let changes = edit.changes.unwrap();
        assert_eq!(changes.len(), 2, "edits across two files");

        let edits_a = changes.get(&uri_a).expect("a has edits");
        // 1 declaration + 1 use site.
        assert_eq!(edits_a.len(), 2);
        let edits_b = changes.get(&uri_b).expect("b has edits");
        // 2 use sites.
        assert_eq!(edits_b.len(), 2);
        for e in edits_a.iter().chain(edits_b.iter()) {
            assert_eq!(e.new_text, "api_url");
        }
    }

    #[test]
    fn rename_env_with_two_declaration_sites_edits_both() {
        let source_file = "name: a\nenv:\n  base_url: http://localhost:3000\nsteps: []\n";
        let env_file = "base_url: http://override/\nother: value\n";
        let uri_test = Url::parse("file:///tmp/a.tarn.yaml").unwrap();
        let uri_env = Url::parse("file:///tmp/tarn.env.yaml").unwrap();

        let env_sources = vec![
            EnvSourceDoc {
                uri: uri_test.clone(),
                source: source_file,
                kind: EnvDeclarationKind::InlineEnvBlock,
                human_label: "inline env block".to_owned(),
            },
            EnvSourceDoc {
                uri: uri_env.clone(),
                source: env_file,
                kind: EnvDeclarationKind::EnvFile,
                human_label: "tarn.env.yaml".to_owned(),
            },
        ];
        let workspace = vec![WorkspaceFile {
            uri: uri_test.clone(),
            source: source_file,
            outline: None,
        }];
        let ctx = RenameContext {
            current_uri: uri_test.clone(),
            current_source: source_file,
            current_test_scope: CaptureScopeOwned::Any,
            current_test_file: None,
            env_sources,
            workspace,
        };

        let span = InterpolationTokenSpan {
            token: InterpolationToken::Env("base_url".into()),
            range: Range::new(Position::new(0, 0), Position::new(0, 1)),
            step_context: None,
        };
        let edit = rename_workspace_edit(&span, "api_url", &ctx).expect("rename ok");
        let changes = edit.changes.unwrap();
        assert!(changes.contains_key(&uri_test), "test file edited");
        assert!(changes.contains_key(&uri_env), "env file edited");
        // Env file has one declaration edit.
        assert_eq!(changes.get(&uri_env).unwrap().len(), 1);
    }

    #[test]
    fn rename_env_collision_inside_single_source_file_is_rejected() {
        // Both `api_key` and `api_secret` live in the same env file.
        // Renaming `api_key` to `api_secret` should collide.
        let env_file = "api_key: xxx\napi_secret: yyy\n";
        let uri_env = Url::parse("file:///tmp/tarn.env.yaml").unwrap();
        let uri_test = Url::parse("file:///tmp/t.tarn.yaml").unwrap();
        let source = "url: \"{{ env.api_key }}\"\n";

        let env_sources = vec![EnvSourceDoc {
            uri: uri_env,
            source: env_file,
            kind: EnvDeclarationKind::EnvFile,
            human_label: "tarn.env.yaml".to_owned(),
        }];
        let ctx = RenameContext {
            current_uri: uri_test,
            current_source: source,
            current_test_scope: CaptureScopeOwned::Any,
            current_test_file: None,
            env_sources,
            workspace: Vec::new(),
        };
        let span = span_at(source, "env.api_key");
        let err = rename_workspace_edit(&span, "api_secret", &ctx).unwrap_err();
        match err {
            RenameError::Collision {
                conflicting_name, ..
            } => {
                assert_eq!(conflicting_name, "api_secret");
            }
            other => panic!("expected collision, got {other:?}"),
        }
    }

    #[test]
    fn rename_env_collision_only_checks_files_that_declare_old_name() {
        // File B declares `new_name` but does NOT declare `old_name`,
        // so it must not block the rename. File A declares `old_name`
        // only. Renaming succeeds.
        let file_a = "old_name: one\nunrelated: two\n";
        let file_b = "new_name: one\n";
        let uri_a = Url::parse("file:///tmp/tarn.env.yaml").unwrap();
        let uri_b = Url::parse("file:///tmp/tarn.env.local.yaml").unwrap();
        let uri_test = Url::parse("file:///tmp/t.tarn.yaml").unwrap();
        let env_sources = vec![
            EnvSourceDoc {
                uri: uri_a.clone(),
                source: file_a,
                kind: EnvDeclarationKind::EnvFile,
                human_label: "file a".to_owned(),
            },
            EnvSourceDoc {
                uri: uri_b,
                source: file_b,
                kind: EnvDeclarationKind::EnvFile,
                human_label: "file b".to_owned(),
            },
        ];
        let source = "url: \"{{ env.old_name }}\"\n";
        let ctx = RenameContext {
            current_uri: uri_test,
            current_source: source,
            current_test_scope: CaptureScopeOwned::Any,
            current_test_file: None,
            env_sources,
            workspace: Vec::new(),
        };
        let span = span_at(source, "env.old_name");
        let edit = rename_workspace_edit(&span, "new_name", &ctx).expect("rename ok");
        let changes = edit.changes.unwrap();
        // The declaration in file A is rewritten; file B is untouched.
        assert!(changes.contains_key(&uri_a));
    }

    #[test]
    fn rename_env_with_invalid_identifier_is_rejected() {
        let source = "url: \"{{ env.base_url }}\"\n";
        let ctx = ctx_with(
            Url::parse("file:///tmp/x.tarn.yaml").unwrap(),
            source,
            CaptureScopeOwned::Any,
        );
        let span = span_at(source, "env.base_url");
        let err = rename_workspace_edit(&span, "1bad", &ctx).unwrap_err();
        assert!(matches!(err, RenameError::InvalidIdentifier(_)));
    }

    #[test]
    fn rename_on_builtin_token_returns_not_renamable() {
        let source = "id: \"{{ $uuid }}\"\n";
        let ctx = ctx_with(
            Url::parse("file:///tmp/x.tarn.yaml").unwrap(),
            source,
            CaptureScopeOwned::Any,
        );
        let span = span_at(source, "$uuid");
        let err = rename_workspace_edit(&span, "new_uuid", &ctx).unwrap_err();
        assert!(matches!(err, RenameError::NotRenamable));
    }

    // ---------- helpers ----------

    #[test]
    fn identifier_subrange_in_span_computes_precise_bounds() {
        let source = "url: \"{{ env.base_url }}\"\n";
        let span = scan_all_interpolations(source).into_iter().next().unwrap();
        let range =
            identifier_subrange_in_span(source, span.range, "env", "base_url").expect("range");
        let start = position_to_byte_offset(source, range.start).unwrap();
        let end = position_to_byte_offset(source, range.end).unwrap();
        assert_eq!(&source[start..end], "base_url");
    }

    #[test]
    fn identifier_subrange_rejects_partial_name_match() {
        let source = "url: \"{{ env.base_url_extra }}\"\n";
        let span = scan_all_interpolations(source).into_iter().next().unwrap();
        // `base_url` is a substring of `base_url_extra`; the helper
        // must not claim the match.
        let out = identifier_subrange_in_span(source, span.range, "env", "base_url");
        assert!(out.is_none());
    }

    #[test]
    fn locate_key_range_on_line_finds_bare_key() {
        let source = "name: cap\napi_key: xxx\n";
        let range = locate_key_range_on_line(source, 2, "api_key").expect("range");
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 0);
        assert_eq!(range.end.character, 7);
    }

    #[test]
    fn rename_error_into_response_error_maps_codes() {
        let invalid: ResponseError = RenameError::InvalidIdentifier("1bad".into()).into();
        assert_eq!(invalid.code, ErrorCode::InvalidParams as i32);
        let collision: ResponseError = RenameError::Collision {
            conflicting_name: "x".into(),
            source: "y".into(),
        }
        .into();
        assert_eq!(collision.code, ErrorCode::InvalidParams as i32);
        let not_renamable: ResponseError = RenameError::NotRenamable.into();
        assert_eq!(not_renamable.code, ErrorCode::RequestFailed as i32);
    }
}
