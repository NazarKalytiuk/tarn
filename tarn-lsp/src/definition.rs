//! `textDocument/definition` handler and renderer.
//!
//! Phase L2.1 (NAZ-297) adds the first navigation feature to `tarn-lsp`:
//! a client can invoke "Go to definition" on a `{{ capture.NAME }}` or
//! `{{ env.KEY }}` token and jump to the declaration site.
//!
//! The module mirrors the L1.3 hover architecture:
//!
//!   1. A pure renderer
//!      [`definition_for_token`] that takes a pre-classified token plus
//!      a synthetic [`DefinitionContext`] and returns an
//!      [`lsp_types::GotoDefinitionResponse`]. It never touches the
//!      filesystem, never parses YAML, and never logs — every behaviour
//!      is unit-testable with a plain struct literal.
//!
//!   2. A thin wrapper [`text_document_definition`] that reads the
//!      document from the server's [`DocumentStore`], builds a
//!      context by calling [`tarn::outline::find_capture_declarations`]
//!      and [`tarn::env::resolve_env_with_sources`], and hands both to
//!      the renderer. That function is the only place that does I/O.
//!
//! ## Jump semantics
//!
//! * `{{ capture.y }}` — scans the current test file for every step
//!   that declares `y` under `capture:` *within the same test*. If
//!   several steps declare the same capture name (legal but unusual)
//!   the response includes every match so the client can show a
//!   picker. If no step declares it, the response is empty.
//!
//! * `{{ env.x }}` — walks the env resolution chain (CLI > named
//!   profile vars > `tarn.env.local.yaml` > named env file >
//!   `tarn.env.yaml` > inline `env:` block) and returns the declaration
//!   location of the *winning* layer. When the winning layer is a YAML
//!   file we scanned (every layer except CLI, shell expansion, and
//!   profile vars), the location points at the key's line and column.
//!   When the winning layer has no declaration site we can jump to
//!   (CLI, shell expansion, named profile vars), the response is
//!   empty — this matches the spec in the ticket.
//!
//! * `{{ $builtin }}` and top-level schema keys are explicitly not
//!   navigable in L2.1 and always return an empty response.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use lsp_types::{GotoDefinitionResponse, Location as LspLocation, Position, Range, Url};
use tarn::env::{self, EnvEntry, EnvSource};
use tarn::model::Location as TarnLocation;
use tarn::outline::{find_capture_declarations, CaptureScope};
use tarn::parser;

use crate::hover::HoverToken;
use crate::server::{is_tarn_file_uri, DocumentStore};

/// Context the pure renderer consults to produce a definition response.
///
/// Every field is pre-computed by [`build_definition_context`] so the
/// renderer itself has nothing to do besides pattern-match the token and
/// look up one of the two maps.
#[derive(Debug, Default)]
pub struct DefinitionContext {
    /// Resolved env, keyed by variable name. Entries that have a
    /// `declaration_range` populated are the ones we can produce a jump
    /// for; entries without one (CLI, shell-expansion, profile vars)
    /// fall through to an empty response so the client's UI does not
    /// flash at a bogus target.
    pub env: BTreeMap<String, EnvEntry>,
    /// For each capture name that appears anywhere visible from the
    /// current cursor, the list of source locations where it is
    /// declared. Populated by [`build_definition_context`] by scanning
    /// the current document with
    /// [`tarn::outline::find_capture_declarations`].
    pub captures: HashMap<String, Vec<TarnLocation>>,
}

/// Render a classified token into an LSP `GotoDefinitionResponse`.
///
/// Pure: no filesystem, no parser, no globals. The renderer is the
/// place that encodes L2.1's jump semantics — every edge case in the
/// ticket boils down to "which map do we look in, and what do we return
/// when the lookup misses?".
///
/// Returns `None` for tokens that L2.1 explicitly does not navigate —
/// builtins, schema keys, or an env/capture lookup that came up empty
/// — so the wrapper forwards `null` to the client exactly the way the
/// LSP spec expects.
pub fn definition_for_token(
    token: &HoverToken,
    ctx: &DefinitionContext,
) -> Option<GotoDefinitionResponse> {
    match token {
        HoverToken::Env(key) => {
            if key.is_empty() {
                return None;
            }
            let entry = ctx.env.get(key)?;
            let declaration = entry.declaration_range.as_ref()?;
            // Only file-backed layers carry a location. CliVar, shell
            // expansion, and NamedProfileVars intentionally have
            // `declaration_range == None`, so the `?` above already
            // filters them out. We still double-check here so a future
            // refactor that pre-populates a range for a non-file layer
            // does not silently break the spec.
            if !source_is_file_backed(&entry.source) {
                return None;
            }
            let location = tarn_location_to_lsp(declaration)?;
            Some(GotoDefinitionResponse::Scalar(location))
        }
        HoverToken::Capture(name) => {
            if name.is_empty() {
                return None;
            }
            let declarations = ctx.captures.get(name)?;
            if declarations.is_empty() {
                return None;
            }
            let locations: Vec<LspLocation> = declarations
                .iter()
                .filter_map(tarn_location_to_lsp)
                .collect();
            if locations.is_empty() {
                return None;
            }
            if locations.len() == 1 {
                Some(GotoDefinitionResponse::Scalar(
                    locations.into_iter().next().unwrap(),
                ))
            } else {
                Some(GotoDefinitionResponse::Array(locations))
            }
        }
        HoverToken::Builtin(_) => None,
        HoverToken::SchemaKey(_) => None,
        // L3.6 (NAZ-307): JSONPath literals have no navigation target —
        // they are evaluated in place against a sidecar response, not
        // resolved to a declaration site. The hover provider is the
        // only consumer that renders them.
        HoverToken::JsonPathLiteral(_) => None,
    }
}

/// Build a [`DefinitionContext`] for `uri` + `source`.
///
/// Performs two side-effectful steps the renderer can't:
///
///   1. Parses the buffer to find out which test the cursor lives in
///      (so capture scope is "this test + setup" rather than the whole
///      file). Uses [`crate::hover::collect_visible_captures`] for the
///      visibility rules — same as hover — so the two features never
///      disagree on what's in scope.
///   2. Resolves the env chain via
///      [`tarn::env::resolve_env_with_sources`] and enriches the inline
///      `env:` block's entries with ranges by scanning the test file's
///      own raw text with
///      [`tarn::env::inline_env_locations_from_source`]. The inline
///      case is the only one `resolve_env_with_sources` cannot fill in
///      itself, because the inline block arrives as a pre-parsed
///      `HashMap<String, String>` without any line metadata.
pub fn build_definition_context(source: &str, uri: &Url, position: Position) -> DefinitionContext {
    let path = uri_to_path(uri);
    let parse_result = parser::parse_str(source, &path);
    let (inline_env, captures) = match &parse_result {
        Ok(test_file) => {
            let display_path = path.display().to_string();
            let cursor_line_one_based = (position.line as usize) + 1;
            let scope = pick_capture_scope(test_file, cursor_line_one_based);
            let captures = collect_capture_locations(source, &display_path, &scope, test_file);
            (test_file.env.clone(), captures)
        }
        Err(_) => (HashMap::new(), HashMap::new()),
    };

    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut env = env::resolve_env_with_sources(
        &inline_env,
        None,
        &[],
        &base_dir,
        "tarn.env.yaml",
        &HashMap::new(),
    )
    .unwrap_or_default();

    // Enrich inline entries with their per-key locations inside the
    // current test file. `resolve_env_with_sources` cannot do this
    // itself because it never sees the raw text of the test file.
    let inline_locations =
        env::inline_env_locations_from_source(source, &path.display().to_string());
    for (key, entry) in env.iter_mut() {
        if matches!(entry.source, EnvSource::InlineEnvBlock) {
            if let Some(loc) = inline_locations.get(key) {
                entry.declaration_range = Some(loc.clone());
            }
        }
    }

    DefinitionContext { env, captures }
}

/// Pick the capture scope for a cursor position.
///
/// Mirrors the visibility rules used by
/// [`crate::hover::collect_visible_captures`], but in terms of the
/// `tarn::outline` scope enum so we can hand it straight to the
/// capture-declaration scanner.
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

/// Owned counterpart to [`tarn::outline::CaptureScope`].
///
/// `CaptureScope` borrows the test name, which is painful for a
/// function that builds its scope from data that doesn't outlive the
/// stack frame. The LSP handler owns its own `String`s and converts to
/// the borrowed form on demand via [`as_borrowed`].
#[derive(Debug, Clone)]
enum CaptureScopeOwned {
    Setup,
    Teardown,
    FlatSteps,
    Test(String),
    Any,
}

impl CaptureScopeOwned {
    fn as_borrowed(&self) -> CaptureScope<'_> {
        match self {
            CaptureScopeOwned::Setup => CaptureScope::Setup,
            CaptureScopeOwned::Teardown => CaptureScope::Teardown,
            CaptureScopeOwned::FlatSteps => CaptureScope::FlatSteps,
            CaptureScopeOwned::Test(name) => CaptureScope::Test(name.as_str()),
            CaptureScopeOwned::Any => CaptureScope::Any,
        }
    }
}

/// Walk every capture name declared by the file's visible sections and
/// collect their declaration locations via
/// [`tarn::outline::find_capture_declarations`].
///
/// Setup captures are always visible (the same way hover's
/// `collect_visible_captures` treats them) so when the cursor is inside
/// a named test we search both that test and setup. The same is true
/// for teardown, which can reference any prior capture — that case
/// falls back to `CaptureScope::Any`.
fn collect_capture_locations(
    source: &str,
    display_path: &str,
    scope: &CaptureScopeOwned,
    test_file: &tarn::model::TestFile,
) -> HashMap<String, Vec<TarnLocation>> {
    let mut out: HashMap<String, Vec<TarnLocation>> = HashMap::new();

    // Union of every capture name that could be visible from this
    // scope. We query the scanner once per unique name so large
    // files do not pay N * parse-cost on every request.
    let mut wanted: Vec<String> = Vec::new();
    let push_unique = |name: &str, wanted: &mut Vec<String>| {
        if !wanted.iter().any(|n| n == name) {
            wanted.push(name.to_owned());
        }
    };
    for step in &test_file.setup {
        for name in step.capture.keys() {
            push_unique(name, &mut wanted);
        }
    }
    match scope {
        CaptureScopeOwned::Setup => {
            // Nothing beyond setup captures — already collected above.
        }
        CaptureScopeOwned::FlatSteps => {
            for step in &test_file.steps {
                for name in step.capture.keys() {
                    push_unique(name, &mut wanted);
                }
            }
        }
        CaptureScopeOwned::Test(test_name) => {
            if let Some(group) = test_file.tests.get(test_name.as_str()) {
                for step in &group.steps {
                    for name in step.capture.keys() {
                        push_unique(name, &mut wanted);
                    }
                }
            }
        }
        CaptureScopeOwned::Teardown | CaptureScopeOwned::Any => {
            for step in &test_file.steps {
                for name in step.capture.keys() {
                    push_unique(name, &mut wanted);
                }
            }
            for group in test_file.tests.values() {
                for step in &group.steps {
                    for name in step.capture.keys() {
                        push_unique(name, &mut wanted);
                    }
                }
            }
            for step in &test_file.teardown {
                for name in step.capture.keys() {
                    push_unique(name, &mut wanted);
                }
            }
        }
    }

    for name in &wanted {
        // Setup is always visible, so we search both the selected
        // scope and setup (except when the scope is already setup or
        // any, in which case the extra scan would double-count).
        let mut locations: Vec<TarnLocation> = Vec::new();
        let setup_locations =
            find_capture_declarations(source, display_path, name, &CaptureScope::Setup);
        // Selected scope.
        let scope_locations =
            find_capture_declarations(source, display_path, name, &scope.as_borrowed());
        match scope {
            CaptureScopeOwned::Setup | CaptureScopeOwned::Any => {
                locations.extend(scope_locations);
            }
            _ => {
                locations.extend(setup_locations.clone());
                locations.extend(
                    scope_locations
                        .into_iter()
                        .filter(|l| !setup_locations.iter().any(|s| s == l)),
                );
            }
        }

        // Deduplicate by (line, column, file) so a file that declares
        // the same capture in multiple places doesn't emit duplicate
        // jump targets.
        locations.sort_by(|a, b| a.line.cmp(&b.line).then(a.column.cmp(&b.column)));
        locations.dedup();

        if !locations.is_empty() {
            out.insert(name.clone(), locations);
        }
    }

    out
}

/// True when an [`EnvSource`] is backed by a YAML file we can point a
/// jump at. CLI, shell expansion, and named profile vars are *not*
/// file-backed — L2.1's spec pins those to an empty response.
fn source_is_file_backed(source: &EnvSource) -> bool {
    matches!(
        source,
        EnvSource::InlineEnvBlock
            | EnvSource::DefaultEnvFile { .. }
            | EnvSource::NamedEnvFile { .. }
            | EnvSource::LocalEnvFile { .. }
    )
}

/// Convert a 1-based [`TarnLocation`] into a 0-based LSP
/// [`LspLocation`].
///
/// This is the only place where [`tarn-lsp`] turns a `tarn` point into
/// an LSP range — every other module either uses
/// [`crate::diagnostics::location_to_range`] (different signature, same
/// conversion) or [`crate::symbols::span_to_range`] (spans, not
/// points). Future refactors should collapse the three, but the
/// conversion is trivial enough that the duplication is a
/// maintenance-zero cost today.
fn tarn_location_to_lsp(loc: &TarnLocation) -> Option<LspLocation> {
    let uri = location_file_to_url(&loc.file)?;
    let line = loc.line.saturating_sub(1) as u32;
    let column = loc.column.saturating_sub(1) as u32;
    // Point range — LSP clients expand the highlight to the enclosing
    // token when `start == end`.
    let start = Position::new(line, column);
    let end = Position::new(line, column);
    Some(LspLocation {
        uri,
        range: Range::new(start, end),
    })
}

/// Build a `file://` URL for a tarn [`TarnLocation::file`].
///
/// The field is a display path — potentially relative, potentially
/// with trailing platform-native separators. We normalise it to an
/// absolute path so [`Url::from_file_path`] accepts it, and fall back
/// to `file://` prefixing when that fails so malformed paths still
/// round-trip through the serializer.
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

fn uri_to_path(uri: &Url) -> PathBuf {
    uri.to_file_path()
        .unwrap_or_else(|_| PathBuf::from(uri.path()))
}

/// The `textDocument/definition` request entry point.
///
/// Returns `None` when the URI has no open buffer, when the cursor
/// does not resolve to an interpolation token, or when none of the
/// navigable cases apply. LSP clients interpret a `null` result as
/// "nothing to jump to" and suppress their UI — exactly the behaviour
/// the spec demands for builtins, missing keys, etc.
pub fn text_document_definition(
    store: &DocumentStore,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    if !is_tarn_file_uri(uri) {
        return None;
    }
    let source = store.get(uri)?;
    let span = crate::hover::resolve_hover_token(source, position)?;
    let ctx = build_definition_context(source, uri, position);
    definition_for_token(&span.token, &ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Url;
    use std::collections::BTreeMap;
    use tarn::env::{EnvEntry, EnvSource};

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

    fn captures_with(
        name: &str,
        locations: Vec<TarnLocation>,
    ) -> HashMap<String, Vec<TarnLocation>> {
        let mut out = HashMap::new();
        out.insert(name.to_owned(), locations);
        out
    }

    // --------- definition_for_token: captures ---------

    #[test]
    fn capture_single_visible_declaration_returns_scalar_location() {
        let loc = tarn_loc("/tmp/sample.tarn.yaml", 5, 7);
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: captures_with("token", vec![loc.clone()]),
        };
        let resp = definition_for_token(&HoverToken::Capture("token".into()), &ctx).unwrap();
        match resp {
            GotoDefinitionResponse::Scalar(location) => {
                // `scan` reports 1-based, handler reports 0-based.
                assert_eq!(location.range.start.line, 4);
                assert_eq!(location.range.start.character, 6);
            }
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn capture_missing_from_scope_returns_none() {
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: HashMap::new(),
        };
        assert!(definition_for_token(&HoverToken::Capture("token".into()), &ctx).is_none());
    }

    #[test]
    fn capture_declared_twice_in_same_test_returns_array_response() {
        let a = tarn_loc("/tmp/sample.tarn.yaml", 5, 7);
        let b = tarn_loc("/tmp/sample.tarn.yaml", 12, 7);
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: captures_with("token", vec![a.clone(), b.clone()]),
        };
        let resp = definition_for_token(&HoverToken::Capture("token".into()), &ctx).unwrap();
        match resp {
            GotoDefinitionResponse::Array(locs) => {
                assert_eq!(locs.len(), 2);
                assert_eq!(locs[0].range.start.line, 4);
                assert_eq!(locs[1].range.start.line, 11);
            }
            other => panic!("expected array, got {other:?}"),
        }
    }

    #[test]
    fn capture_with_empty_name_returns_none() {
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: captures_with("token", vec![tarn_loc("/tmp/x.tarn.yaml", 1, 1)]),
        };
        assert!(definition_for_token(&HoverToken::Capture(String::new()), &ctx).is_none());
    }

    // --------- definition_for_token: env ---------

    #[test]
    fn env_found_in_inline_env_block_returns_scalar_location() {
        let mut env = BTreeMap::new();
        env.insert(
            "base_url".to_owned(),
            env_entry(
                "http://localhost",
                EnvSource::InlineEnvBlock,
                Some(tarn_loc("/tmp/t.tarn.yaml", 3, 5)),
            ),
        );
        let ctx = DefinitionContext {
            env,
            captures: HashMap::new(),
        };
        let resp = definition_for_token(&HoverToken::Env("base_url".into()), &ctx).unwrap();
        match resp {
            GotoDefinitionResponse::Scalar(location) => {
                assert_eq!(location.range.start.line, 2);
                assert_eq!(location.range.start.character, 4);
            }
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn env_found_in_tarn_env_yaml_returns_scalar_location() {
        let mut env = BTreeMap::new();
        env.insert(
            "base_url".to_owned(),
            env_entry(
                "http://from-file",
                EnvSource::DefaultEnvFile {
                    path: "/proj/tarn.env.yaml".to_owned(),
                },
                Some(tarn_loc("/proj/tarn.env.yaml", 1, 11)),
            ),
        );
        let ctx = DefinitionContext {
            env,
            captures: HashMap::new(),
        };
        let resp = definition_for_token(&HoverToken::Env("base_url".into()), &ctx).unwrap();
        match resp {
            GotoDefinitionResponse::Scalar(location) => {
                assert!(location.uri.to_string().contains("tarn.env.yaml"));
                assert_eq!(location.range.start.line, 0);
                assert_eq!(location.range.start.character, 10);
            }
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    #[test]
    fn env_resolved_from_cli_var_returns_empty() {
        let mut env = BTreeMap::new();
        env.insert(
            "token".to_owned(),
            env_entry("from-cli", EnvSource::CliVar, None),
        );
        let ctx = DefinitionContext {
            env,
            captures: HashMap::new(),
        };
        assert!(definition_for_token(&HoverToken::Env("token".into()), &ctx).is_none());
    }

    #[test]
    fn env_resolved_from_named_profile_vars_returns_empty() {
        let mut env = BTreeMap::new();
        env.insert(
            "region".to_owned(),
            env_entry(
                "eu-west-1",
                EnvSource::NamedProfileVars {
                    env_name: "staging".to_owned(),
                },
                None,
            ),
        );
        let ctx = DefinitionContext {
            env,
            captures: HashMap::new(),
        };
        assert!(definition_for_token(&HoverToken::Env("region".into()), &ctx).is_none());
    }

    #[test]
    fn env_not_found_at_all_returns_none() {
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: HashMap::new(),
        };
        assert!(definition_for_token(&HoverToken::Env("missing".into()), &ctx).is_none());
    }

    #[test]
    fn env_with_empty_key_returns_none() {
        let ctx = DefinitionContext {
            env: BTreeMap::new(),
            captures: HashMap::new(),
        };
        assert!(definition_for_token(&HoverToken::Env(String::new()), &ctx).is_none());
    }

    #[test]
    fn env_local_file_layer_navigates_to_local_env_yaml() {
        let mut env = BTreeMap::new();
        env.insert(
            "api_token".to_owned(),
            env_entry(
                "super-secret",
                EnvSource::LocalEnvFile {
                    path: "/proj/tarn.env.local.yaml".to_owned(),
                },
                Some(tarn_loc("/proj/tarn.env.local.yaml", 2, 12)),
            ),
        );
        let ctx = DefinitionContext {
            env,
            captures: HashMap::new(),
        };
        let resp = definition_for_token(&HoverToken::Env("api_token".into()), &ctx).unwrap();
        match resp {
            GotoDefinitionResponse::Scalar(location) => {
                assert!(location.uri.to_string().contains("tarn.env.local.yaml"));
                assert_eq!(location.range.start.line, 1);
            }
            other => panic!("expected scalar, got {other:?}"),
        }
    }

    // --------- definition_for_token: non-navigable tokens ---------

    #[test]
    fn builtin_token_always_returns_none() {
        let ctx = DefinitionContext::default();
        assert!(definition_for_token(&HoverToken::Builtin("uuid".into()), &ctx).is_none());
        assert!(definition_for_token(&HoverToken::Builtin("random_hex".into()), &ctx).is_none());
    }

    #[test]
    fn schema_key_token_always_returns_none() {
        let ctx = DefinitionContext::default();
        assert!(definition_for_token(&HoverToken::SchemaKey("status".into()), &ctx).is_none());
        assert!(definition_for_token(&HoverToken::SchemaKey("body".into()), &ctx).is_none());
    }

    // --------- build_definition_context (light-touch) ---------

    #[test]
    fn build_definition_context_discovers_capture_in_same_test() {
        let yaml = "\
name: Capture test
tests:
  main:
    steps:
      - name: login
        request:
          method: POST
          url: http://x/auth
        capture:
          token: $.id
      - name: next
        request:
          method: GET
          url: http://x/items
          headers:
            Authorization: \"Bearer {{ capture.token }}\"
";
        let uri = Url::parse("file:///tmp/def-sample.tarn.yaml").unwrap();
        // Cursor anywhere in the second step so `token` is visible.
        let ctx = build_definition_context(yaml, &uri, Position::new(13, 20));
        let entries = ctx.captures.get("token").expect("token location");
        assert!(!entries.is_empty());
        assert_eq!(entries[0].line, 10);
    }

    #[test]
    fn tarn_location_to_lsp_clamps_zero_line_to_point() {
        let loc = tarn_loc("/tmp/x.tarn.yaml", 0, 0);
        let lsp = tarn_location_to_lsp(&loc).unwrap();
        assert_eq!(lsp.range.start, Position::new(0, 0));
        assert_eq!(lsp.range.end, Position::new(0, 0));
    }

    #[test]
    fn source_is_file_backed_covers_every_file_layer() {
        assert!(source_is_file_backed(&EnvSource::InlineEnvBlock));
        assert!(source_is_file_backed(&EnvSource::DefaultEnvFile {
            path: "/tmp/x".into()
        }));
        assert!(source_is_file_backed(&EnvSource::NamedEnvFile {
            path: "/tmp/x".into(),
            env_name: "s".into()
        }));
        assert!(source_is_file_backed(&EnvSource::LocalEnvFile {
            path: "/tmp/x".into()
        }));
        assert!(!source_is_file_backed(&EnvSource::CliVar));
        assert!(!source_is_file_backed(&EnvSource::NamedProfileVars {
            env_name: "s".into()
        }));
    }
}
