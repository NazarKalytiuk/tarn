//! `textDocument/references` handler and renderer.
//!
//! Phase L2.2 (NAZ-298) is the second navigation feature in `tarn-lsp`. It
//! lets a client list every use site of a `{{ capture.NAME }}` or
//! `{{ env.KEY }}` interpolation token.
//!
//! The module is structured the same way [`crate::definition`] is:
//!
//!   1. A pure renderer [`references_for_token`] that takes a pre-classified
//!      token and a synthetic [`ReferencesContext`] of (URL, source, outline)
//!      tuples and returns the matching `Vec<Location>`. It never touches the
//!      filesystem and never parses YAML — every behaviour is unit-testable
//!      with a plain context literal.
//!
//!   2. A thin wrapper [`text_document_references`] that builds a context
//!      from the server's [`crate::server::ServerState`] (the document store
//!      plus the workspace index) and dispatches into the renderer. The
//!      wrapper is the only place that does I/O.
//!
//! ## Reference semantics
//!
//! Captures are scoped per-test in tarn's data model, so capture references
//! intentionally never walk the workspace — they only scan the file the
//! cursor is in. The same scope rules as
//! [`crate::definition::build_definition_context`] apply: setup captures
//! are visible from every test, named-test captures are only visible
//! inside their test.
//!
//! Env references *do* walk the workspace. The walk is bounded by the
//! [`crate::workspace::WORKSPACE_FILE_LIMIT`] safety net so a stray
//! transitive `node_modules` cannot pin a single request. The renderer
//! signature accepts a slice of `(Url, &Outline, &str)` triples to keep it
//! decoupled from the filesystem walker — tests can pass a synthetic slice
//! that includes 0, 1, or N files without spinning up a `WorkspaceIndex`.
//!
//! When `include_declaration` is `true` the response also contains the
//! capture's `capture:` key location (for capture lookups) or the env
//! key's range in whichever env file is the resolved source per the L1.3
//! resolution chain (for env lookups). When `false` only use sites are
//! returned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsp_types::{Location as LspLocation, Position, Range, ReferenceParams, Url};
use tarn::env::{self, EnvEntry, EnvSource};
use tarn::model::Location as TarnLocation;
use tarn::outline::{find_capture_declarations, CaptureScope, Outline};
use tarn::parser;

use crate::server::{is_tarn_file_uri, ServerState};
use crate::token::{resolve_interpolation_token, scan_all_interpolations, InterpolationToken};

/// Context the references renderer consults.
///
/// `current_uri` / `current_source` / `current_outline` describe the file
/// the cursor is in. `workspace` is a slice of every other file the
/// renderer should consider when resolving an env reference — when empty
/// (or when the only entry is `current_uri` itself) the env walk
/// degenerates into a single-file walk, which is exactly the behaviour we
/// want when no workspace root has been configured.
///
/// The renderer never invents new tuples — it only iterates the slice
/// callers hand it. That keeps the renderer signature 100% decoupled
/// from the filesystem walker.
pub struct ReferencesContext<'a> {
    /// URL of the file the cursor is in.
    pub current_uri: Url,
    /// Raw text of the current file.
    pub current_source: &'a str,
    /// Outline (best-effort) of the current file. Used by capture
    /// references to discover declaration sites.
    pub current_outline: Option<&'a Outline>,
    /// Owned name of the test the cursor is in (for capture scoping). The
    /// wrapper computes this once when it builds the context.
    pub current_test_scope: CaptureScopeOwned,
    /// Resolved env, keyed by variable name. Mirrors
    /// [`crate::definition::DefinitionContext::env`].
    pub env: HashMap<String, EnvEntry>,
    /// Every other file in the workspace, paired with its outline (when
    /// the file parses) and raw source text. Used only by env references.
    pub workspace: Vec<WorkspaceFile<'a>>,
}

/// One workspace file the renderer can consult.
pub struct WorkspaceFile<'a> {
    pub uri: Url,
    pub source: &'a str,
    /// Best-effort outline. Today the renderer doesn't actually need the
    /// outline for env walks (token scanning is enough) but the field is
    /// kept so future tickets that need scope information can drop in.
    pub outline: Option<&'a Outline>,
}

/// Owned counterpart to [`tarn::outline::CaptureScope`]. Mirrors the same
/// type used by `crate::definition` so future refactors can collapse the
/// two into a shared helper if both modules end up needing it.
#[derive(Debug, Clone)]
pub enum CaptureScopeOwned {
    Setup,
    Teardown,
    FlatSteps,
    Test(String),
    Any,
}

impl CaptureScopeOwned {
    pub fn as_borrowed(&self) -> CaptureScope<'_> {
        match self {
            CaptureScopeOwned::Setup => CaptureScope::Setup,
            CaptureScopeOwned::Teardown => CaptureScope::Teardown,
            CaptureScopeOwned::FlatSteps => CaptureScope::FlatSteps,
            CaptureScopeOwned::Test(name) => CaptureScope::Test(name.as_str()),
            CaptureScopeOwned::Any => CaptureScope::Any,
        }
    }
}

/// Render references for a classified token.
///
/// Pure: no filesystem, no parser, no globals. Builtins, schema keys,
/// empty identifiers, and unknown env / capture lookups all collapse to
/// an empty `Vec` rather than `None` so the LSP wrapper can hand the
/// result straight back to the client.
pub fn references_for_token(
    token: &InterpolationToken,
    ctx: &ReferencesContext<'_>,
    include_declaration: bool,
) -> Vec<LspLocation> {
    match token {
        InterpolationToken::Env(key) => {
            if key.is_empty() {
                return Vec::new();
            }
            collect_env_references(key, ctx, include_declaration)
        }
        InterpolationToken::Capture(name) => {
            if name.is_empty() {
                return Vec::new();
            }
            collect_capture_references(name, ctx, include_declaration)
        }
        InterpolationToken::Builtin(_) | InterpolationToken::SchemaKey(_) => Vec::new(),
        // L3.6 (NAZ-307): JSONPath literals have no reference graph —
        // they live inline inside one step's assertion/capture/poll
        // block and never reference a symbol elsewhere in the file.
        InterpolationToken::JsonPathLiteral(_) => Vec::new(),
    }
}

/// Walk every workspace file in `ctx.workspace` (plus the current file)
/// and collect every interpolation token that references `env.key`.
fn collect_env_references(
    key: &str,
    ctx: &ReferencesContext<'_>,
    include_declaration: bool,
) -> Vec<LspLocation> {
    let mut out: Vec<LspLocation> = Vec::new();

    // 1. Use sites in the current file.
    push_env_uses(&ctx.current_uri, ctx.current_source, key, &mut out);

    // 2. Use sites in every other workspace file. We deduplicate against
    // the current URI so callers that pass the current file inside the
    // workspace slice (a real `WorkspaceIndex` does) don't double-count.
    for file in &ctx.workspace {
        if file.uri == ctx.current_uri {
            continue;
        }
        push_env_uses(&file.uri, file.source, key, &mut out);
    }

    // 3. Optionally include the env declaration. Mirrors
    // `definition::definition_for_token`'s file-backed-source check.
    if include_declaration {
        if let Some(entry) = ctx.env.get(key) {
            if let Some(loc) = entry.declaration_range.as_ref() {
                if source_is_file_backed(&entry.source) {
                    if let Some(location) = tarn_location_to_lsp(loc) {
                        // Avoid emitting the declaration twice if a use
                        // site happens to land on the same line+column.
                        if !out.iter().any(|existing| {
                            existing.uri == location.uri && existing.range == location.range
                        }) {
                            out.push(location);
                        }
                    }
                }
            }
        }
    }

    out
}

/// Scan a single file's source text for every `{{ env.<key> }}`
/// interpolation and append matches to `out`.
fn push_env_uses(uri: &Url, source: &str, key: &str, out: &mut Vec<LspLocation>) {
    for span in scan_all_interpolations(source) {
        if let InterpolationToken::Env(found) = &span.token {
            if found == key {
                out.push(LspLocation {
                    uri: uri.clone(),
                    range: span.range,
                });
            }
        }
    }
}

/// Capture references stay in the current file and respect the cursor's
/// per-test scope. Setup captures are visible from every test, so the
/// renderer searches both the cursor's scope and setup.
fn collect_capture_references(
    name: &str,
    ctx: &ReferencesContext<'_>,
    include_declaration: bool,
) -> Vec<LspLocation> {
    let mut out: Vec<LspLocation> = Vec::new();

    // 1. Use sites — every interpolation token in the current file that
    // references `capture.<name>`. Tarn's data model scopes captures
    // per-test, but at the source-text level we still want to point at
    // every same-file occurrence; cross-test resolution is the user's
    // problem to interpret.
    for span in scan_all_interpolations(ctx.current_source) {
        if let InterpolationToken::Capture(found) = &span.token {
            if found == name {
                out.push(LspLocation {
                    uri: ctx.current_uri.clone(),
                    range: span.range,
                });
            }
        }
    }

    // 2. Optionally include the declaration site(s) in the cursor's
    // scope (plus setup, which is always visible).
    if include_declaration {
        let display_path = uri_display_path(&ctx.current_uri);
        let mut declarations: Vec<TarnLocation> = Vec::new();
        let setup_locs = find_capture_declarations(
            ctx.current_source,
            &display_path,
            name,
            &CaptureScope::Setup,
        );
        let scope_locs = find_capture_declarations(
            ctx.current_source,
            &display_path,
            name,
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
        for decl in declarations {
            if let Some(location) = tarn_location_to_lsp(&decl) {
                if !out.iter().any(|existing| {
                    existing.uri == location.uri && existing.range == location.range
                }) {
                    out.push(location);
                }
            }
        }
    }

    out
}

/// Same predicate `definition::source_is_file_backed` uses. Kept private
/// to this module so the two go-to features can evolve independently
/// without one accidentally narrowing the other's behaviour.
fn source_is_file_backed(source: &EnvSource) -> bool {
    matches!(
        source,
        EnvSource::InlineEnvBlock
            | EnvSource::DefaultEnvFile { .. }
            | EnvSource::NamedEnvFile { .. }
            | EnvSource::LocalEnvFile { .. }
    )
}

/// Convert a 1-based [`TarnLocation`] into a 0-based LSP [`LspLocation`].
/// Same body as `definition::tarn_location_to_lsp` — duplicated rather
/// than re-exported to keep the modules independently testable.
fn tarn_location_to_lsp(loc: &TarnLocation) -> Option<LspLocation> {
    let uri = location_file_to_url(&loc.file)?;
    let line = loc.line.saturating_sub(1) as u32;
    let column = loc.column.saturating_sub(1) as u32;
    let start = Position::new(line, column);
    let end = Position::new(line, column);
    Some(LspLocation {
        uri,
        range: Range::new(start, end),
    })
}

fn location_file_to_url(path: &str) -> Option<Url> {
    let path_buf = PathBuf::from(path);
    let absolute = if path_buf.is_absolute() {
        path_buf
    } else {
        std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(&path_buf))
            .unwrap_or(path_buf)
    };
    Url::from_file_path(&absolute).ok()
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

/// `textDocument/references` request entry point.
///
/// Returns an empty `Vec` (rather than `None`) when the cursor doesn't
/// resolve to a token or when no references are found. Returning the
/// empty vector matches what every other LSP server (rust-analyzer,
/// gopls, etc.) does — clients display "no references found" rather than
/// suppressing the UI.
pub fn text_document_references(
    state: &mut ServerState,
    params: ReferenceParams,
) -> Vec<LspLocation> {
    let uri = params.text_document_position.text_document.uri.clone();
    let position = params.text_document_position.position;

    if !is_tarn_file_uri(&uri) {
        return Vec::new();
    }
    let Some(source) = state.documents.get(&uri).map(|s| s.to_owned()) else {
        return Vec::new();
    };
    let Some(span) = resolve_interpolation_token(&source, position) else {
        return Vec::new();
    };

    // Build the env + capture context the same way definition does. We
    // can't reuse `definition::build_definition_context` directly
    // because the references renderer needs an owned `HashMap` plus the
    // owned scope, but the steps are identical. Future cleanup ticket
    // could lift this into a shared helper.
    let path = uri_to_path(&uri);
    let parse_result = parser::parse_str(&source, &path);
    let (inline_env, scope) = match &parse_result {
        Ok(test_file) => {
            let cursor_line = (position.line as usize) + 1;
            (
                test_file.env.clone(),
                pick_capture_scope(test_file, cursor_line),
            )
        }
        Err(_) => (HashMap::new(), CaptureScopeOwned::Any),
    };

    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut env_btree = env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &HashMap::new(),
    )
    .unwrap_or_default();
    let inline_locations =
        env::inline_env_locations_from_source(&source, &path.display().to_string());
    for (key, entry) in env_btree.iter_mut() {
        if matches!(entry.source, EnvSource::InlineEnvBlock) {
            if let Some(loc) = inline_locations.get(key) {
                entry.declaration_range = Some(loc.clone());
            }
        }
    }
    let env: HashMap<String, EnvEntry> = env_btree.into_iter().collect();

    // Make sure the workspace cache reflects the freshest copy of the
    // current buffer (the document store wins over whatever was on disk
    // at scan time) and is populated for every other file we know
    // about. Best-effort — failures degrade to a single-file walk.
    state
        .workspace_index
        .insert_from_source(uri.clone(), source.clone());
    if let Err(err) = state.workspace_index.ensure_scanned() {
        eprintln!("tarn-lsp: workspace scan failed: {err}");
    }

    let workspace_files: Vec<WorkspaceFile<'_>> = state
        .workspace_index
        .iter()
        .map(|(url, cached)| WorkspaceFile {
            uri: url.clone(),
            source: cached.source.as_str(),
            outline: cached.outline.as_ref(),
        })
        .collect();

    // The current file's outline lives in the workspace index too,
    // because we just inserted it. Pull it back out by URL so the
    // renderer always sees a consistent view.
    let current_outline = state
        .workspace_index
        .get(&uri)
        .and_then(|c| c.outline.as_ref());
    let current_source = state
        .workspace_index
        .get(&uri)
        .map(|c| c.source.as_str())
        .unwrap_or(&source);

    let ctx = ReferencesContext {
        current_uri: uri.clone(),
        current_source,
        current_outline,
        current_test_scope: scope,
        env,
        workspace: workspace_files,
    };

    references_for_token(&span.token, &ctx, params.context.include_declaration)
}

/// Pick the capture scope for a cursor position. Mirrors the
/// `definition::pick_capture_scope` heuristic.
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
    use tarn::env::EnvEntry;

    fn env_entry(value: &str, source: EnvSource, range: Option<TarnLocation>) -> EnvEntry {
        EnvEntry {
            value: value.to_owned(),
            source,
            declaration_range: range,
        }
    }

    fn tarn_loc(file: &str, line: usize, column: usize) -> TarnLocation {
        TarnLocation {
            file: file.to_owned(),
            line,
            column,
        }
    }

    fn make_ctx<'a>(
        current_uri: Url,
        current_source: &'a str,
        env: HashMap<String, EnvEntry>,
        workspace: Vec<WorkspaceFile<'a>>,
        scope: CaptureScopeOwned,
    ) -> ReferencesContext<'a> {
        ReferencesContext {
            current_uri,
            current_source,
            current_outline: None,
            current_test_scope: scope,
            env,
            workspace,
        }
    }

    // ---------- captures ----------

    #[test]
    fn capture_with_multiple_uses_in_same_test_returns_every_use_site() {
        let source = "name: c\ntests:\n  main:\n    steps:\n      - name: login\n        request:\n          method: POST\n          url: \"http://x/auth\"\n        capture:\n          token: $.id\n      - name: a\n        request:\n          method: GET\n          url: \"http://x/{{ capture.token }}\"\n      - name: b\n        request:\n          method: GET\n          url: \"http://x/items?key={{ capture.token }}\"\n";
        let uri = Url::parse("file:///tmp/cap.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri.clone(),
            source,
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Test("main".into()),
        );
        let refs = references_for_token(&InterpolationToken::Capture("token".into()), &ctx, false);
        assert_eq!(refs.len(), 2, "two interpolation use sites");
        assert!(refs.iter().all(|r| r.uri == uri));
    }

    #[test]
    fn capture_with_no_uses_returns_only_declaration_when_requested() {
        // Capture is declared but never referenced. include_declaration=true
        // should still return the declaration site.
        let source = "name: c\ntests:\n  main:\n    steps:\n      - name: login\n        request:\n          method: POST\n          url: \"http://x/auth\"\n        capture:\n          token: $.id\n";
        let uri = Url::parse("file:///tmp/cap-decl.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri.clone(),
            source,
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Test("main".into()),
        );
        let with_decl =
            references_for_token(&InterpolationToken::Capture("token".into()), &ctx, true);
        assert_eq!(with_decl.len(), 1, "declaration only");

        let without_decl =
            references_for_token(&InterpolationToken::Capture("token".into()), &ctx, false);
        assert!(
            without_decl.is_empty(),
            "no use sites and include_declaration=false"
        );
    }

    #[test]
    fn capture_not_declared_returns_empty() {
        let source = "name: empty\nsteps: []\n";
        let uri = Url::parse("file:///tmp/none.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            source,
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Capture("missing".into()), &ctx, true);
        assert!(refs.is_empty());
    }

    #[test]
    fn capture_with_empty_name_returns_empty() {
        let uri = Url::parse("file:///tmp/x.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            "name: x\n",
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Capture(String::new()), &ctx, true);
        assert!(refs.is_empty());
    }

    // ---------- env ----------

    #[test]
    fn env_used_in_current_file_only_returns_local_uses() {
        let source = "name: e\nenv:\n  base_url: http://localhost\nsteps:\n  - name: a\n    request:\n      method: GET\n      url: \"{{ env.base_url }}/x\"\n";
        let uri = Url::parse("file:///tmp/env-local.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri.clone(),
            source,
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::FlatSteps,
        );
        let refs = references_for_token(&InterpolationToken::Env("base_url".into()), &ctx, false);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].uri, uri);
    }

    #[test]
    fn env_used_in_two_files_returns_uses_from_both() {
        let source_a =
            "name: a\nsteps:\n  - name: x\n    request: { method: GET, url: \"{{ env.k }}/a\" }\n";
        let source_b = "name: b\nsteps:\n  - name: y\n    request: { method: GET, url: \"{{ env.k }}/b\" }\n  - name: z\n    request: { method: GET, url: \"{{ env.k }}/z\" }\n";
        let uri_a = Url::parse("file:///tmp/a.tarn.yaml").unwrap();
        let uri_b = Url::parse("file:///tmp/b.tarn.yaml").unwrap();
        let workspace = vec![WorkspaceFile {
            uri: uri_b.clone(),
            source: source_b,
            outline: None,
        }];
        let ctx = make_ctx(
            uri_a.clone(),
            source_a,
            HashMap::new(),
            workspace,
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Env("k".into()), &ctx, false);
        assert_eq!(refs.len(), 3, "1 from a + 2 from b");
        let count_a = refs.iter().filter(|r| r.uri == uri_a).count();
        let count_b = refs.iter().filter(|r| r.uri == uri_b).count();
        assert_eq!(count_a, 1);
        assert_eq!(count_b, 2);
    }

    #[test]
    fn env_with_include_declaration_returns_env_source_location() {
        let source = "name: e\nenv:\n  base_url: http://localhost\nsteps: []\n";
        let uri = Url::parse("file:///tmp/env-decl.tarn.yaml").unwrap();
        let mut env = HashMap::new();
        env.insert(
            "base_url".to_owned(),
            env_entry(
                "http://localhost",
                EnvSource::InlineEnvBlock,
                Some(tarn_loc("/tmp/env-decl.tarn.yaml", 3, 5)),
            ),
        );
        let ctx = make_ctx(uri, source, env, Vec::new(), CaptureScopeOwned::Any);
        let refs = references_for_token(&InterpolationToken::Env("base_url".into()), &ctx, true);
        assert_eq!(refs.len(), 1, "declaration only");
        assert_eq!(refs[0].range.start.line, 2);
        assert_eq!(refs[0].range.start.character, 4);
    }

    #[test]
    fn env_with_include_declaration_false_returns_only_use_sites() {
        // Same source as the previous test, but the buffer also includes
        // a use site so we have something concrete to check.
        let source = "name: e\nenv:\n  base_url: http://localhost\nsteps:\n  - name: a\n    request: { method: GET, url: \"{{ env.base_url }}\" }\n";
        let uri = Url::parse("file:///tmp/env-uses.tarn.yaml").unwrap();
        let mut env = HashMap::new();
        env.insert(
            "base_url".to_owned(),
            env_entry(
                "http://localhost",
                EnvSource::InlineEnvBlock,
                Some(tarn_loc("/tmp/env-uses.tarn.yaml", 3, 5)),
            ),
        );
        let ctx = make_ctx(uri, source, env, Vec::new(), CaptureScopeOwned::FlatSteps);
        let refs = references_for_token(&InterpolationToken::Env("base_url".into()), &ctx, false);
        assert_eq!(refs.len(), 1, "single use site, no declaration");
        // The use site is on line index 5 (0-based) — line 6 of the source.
        assert_eq!(refs[0].range.start.line, 5);
    }

    #[test]
    fn env_not_found_in_resolved_chain_still_returns_use_sites() {
        // env.foo is referenced in the source but missing from the
        // resolved env map (e.g. the user has a typo). The renderer
        // still surfaces the in-source use site so the client can show
        // them — references is not the same as definition.
        let source = "url: \"{{ env.foo }}\"\n";
        let uri = Url::parse("file:///tmp/env-missing.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            source,
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Env("foo".into()), &ctx, true);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn env_with_empty_key_returns_empty() {
        let uri = Url::parse("file:///tmp/x.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            "name: x\n",
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Env(String::new()), &ctx, true);
        assert!(refs.is_empty());
    }

    #[test]
    fn env_resolved_from_cli_var_does_not_emit_declaration() {
        // CLI vars have no declaration_range, so include_declaration=true
        // should still produce just the use site (if any).
        let source = "url: \"{{ env.token }}\"\n";
        let uri = Url::parse("file:///tmp/env-cli.tarn.yaml").unwrap();
        let mut env = HashMap::new();
        env.insert(
            "token".to_owned(),
            env_entry("from-cli", EnvSource::CliVar, None),
        );
        let ctx = make_ctx(uri, source, env, Vec::new(), CaptureScopeOwned::Any);
        let refs = references_for_token(&InterpolationToken::Env("token".into()), &ctx, true);
        assert_eq!(refs.len(), 1, "use site only, no declaration");
    }

    // ---------- non-navigable tokens ----------

    #[test]
    fn builtin_token_returns_empty() {
        let uri = Url::parse("file:///tmp/x.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            "url: \"{{ $uuid }}\"\n",
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Builtin("uuid".into()), &ctx, true);
        assert!(refs.is_empty());
    }

    #[test]
    fn schema_key_token_returns_empty() {
        let uri = Url::parse("file:///tmp/x.tarn.yaml").unwrap();
        let ctx = make_ctx(
            uri,
            "name: x\n",
            HashMap::new(),
            Vec::new(),
            CaptureScopeOwned::Any,
        );
        let refs =
            references_for_token(&InterpolationToken::SchemaKey("status".into()), &ctx, true);
        assert!(refs.is_empty());
    }

    // ---------- workspace dedup ----------

    #[test]
    fn workspace_entry_for_current_uri_does_not_double_count() {
        // The renderer's workspace slice is allowed to include the
        // current file (a real WorkspaceIndex always does). The renderer
        // must skip that entry when iterating so the use sites in the
        // current file are not counted twice.
        let source = "url: \"{{ env.k }}\"\n";
        let uri = Url::parse("file:///tmp/dup.tarn.yaml").unwrap();
        let workspace = vec![WorkspaceFile {
            uri: uri.clone(),
            source,
            outline: None,
        }];
        let ctx = make_ctx(
            uri,
            source,
            HashMap::new(),
            workspace,
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Env("k".into()), &ctx, false);
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn env_used_in_three_files_returns_uses_from_all_three() {
        let source_a = "url: \"{{ env.k }}/a\"\n";
        let source_b = "url: \"{{ env.k }}/b\"\n";
        let source_c = "url: \"{{ env.k }}/c\"\n";
        let uri_a = Url::parse("file:///tmp/a.tarn.yaml").unwrap();
        let uri_b = Url::parse("file:///tmp/b.tarn.yaml").unwrap();
        let uri_c = Url::parse("file:///tmp/c.tarn.yaml").unwrap();
        let workspace = vec![
            WorkspaceFile {
                uri: uri_b.clone(),
                source: source_b,
                outline: None,
            },
            WorkspaceFile {
                uri: uri_c.clone(),
                source: source_c,
                outline: None,
            },
        ];
        let ctx = make_ctx(
            uri_a.clone(),
            source_a,
            HashMap::new(),
            workspace,
            CaptureScopeOwned::Any,
        );
        let refs = references_for_token(&InterpolationToken::Env("k".into()), &ctx, false);
        assert_eq!(refs.len(), 3);
        assert!(refs.iter().any(|r| r.uri == uri_a));
        assert!(refs.iter().any(|r| r.uri == uri_b));
        assert!(refs.iter().any(|r| r.uri == uri_c));
    }
}
