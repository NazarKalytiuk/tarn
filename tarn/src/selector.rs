//! Selective execution filters for `tarn run`.
//!
//! A [`Selector`] narrows the work the runner performs to specific files,
//! tests, or steps. Each selector is parsed from `FILE[::TEST[::STEP]]` on
//! the CLI and combines with the tag filter using AND semantics: the runner
//! executes an item only if it matches the tag filter **and** at least one
//! active selector (or there are no active selectors).
//!
//! The STEP component accepts either a step name (exact match) or a
//! zero-based integer index. File matching is path-suffix based so users
//! can pass either a workspace-relative or absolute path.
//!
//! ## Composing selectors
//!
//! The [`format_file_selector`], [`format_test_selector`], and
//! [`format_step_selector`] helpers build the string representation that
//! `tarn run --select` consumes. They are the **single source of truth**
//! for selector shape: every producer in the tree (the VS Code extension's
//! test controller, the `tarn-lsp` code lens provider, `tarn fix plan`,
//! and anything we add later) should go through these helpers instead of
//! hand-rolling `format!("{}::{}", …)`. Centralising the format here also
//! means a future grammar change only needs to update one place and re-run
//! the unit tests below.

use std::path::Path;

/// A single `--select` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub file: String,
    pub test: Option<String>,
    pub step: Option<StepSelector>,
}

/// How a selector identifies a step within a test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepSelector {
    /// Zero-based index in the step array.
    Index(usize),
    /// Exact step name match.
    Name(String),
}

impl Selector {
    /// Parse a `FILE[::TEST[::STEP]]` selector string.
    ///
    /// Returns an error if the string is empty, contains empty components,
    /// or includes more than three segments.
    pub fn parse(input: &str) -> Result<Self, String> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err("selector is empty".to_string());
        }
        let parts: Vec<&str> = trimmed.splitn(3, "::").collect();
        match parts.as_slice() {
            [file] => {
                ensure_non_empty(file, "file")?;
                Ok(Selector {
                    file: file.to_string(),
                    test: None,
                    step: None,
                })
            }
            [file, test] => {
                ensure_non_empty(file, "file")?;
                ensure_non_empty(test, "test")?;
                Ok(Selector {
                    file: file.to_string(),
                    test: Some(test.to_string()),
                    step: None,
                })
            }
            [file, test, step] => {
                ensure_non_empty(file, "file")?;
                ensure_non_empty(test, "test")?;
                ensure_non_empty(step, "step")?;
                let step_sel = if let Ok(idx) = step.parse::<usize>() {
                    StepSelector::Index(idx)
                } else {
                    StepSelector::Name(step.to_string())
                };
                Ok(Selector {
                    file: file.to_string(),
                    test: Some(test.to_string()),
                    step: Some(step_sel),
                })
            }
            _ => Err(format!("malformed selector: {}", input)),
        }
    }

    /// True if this selector applies to the given file path. Uses suffix
    /// matching so callers can pass either absolute or relative paths.
    pub fn matches_file(&self, file_path: &str) -> bool {
        path_suffix_matches(file_path, &self.file)
    }

    /// True if this selector applies to the given test name. A selector
    /// without a test component matches every test.
    pub fn matches_test(&self, test_name: &str) -> bool {
        match &self.test {
            None => true,
            Some(t) => t == test_name,
        }
    }

    /// True if this selector applies to the given step. A selector without
    /// a step component matches every step in the matched test.
    pub fn matches_step(&self, step_index: usize, step_name: &str) -> bool {
        match &self.step {
            None => true,
            Some(StepSelector::Index(i)) => *i == step_index,
            Some(StepSelector::Name(n)) => n == step_name,
        }
    }
}

/// Parse a collection of selector strings, returning all parse errors.
pub fn parse_all(inputs: &[String]) -> Result<Vec<Selector>, Vec<String>> {
    let mut out = Vec::with_capacity(inputs.len());
    let mut errs = Vec::new();
    for input in inputs {
        match Selector::parse(input) {
            Ok(sel) => out.push(sel),
            Err(e) => errs.push(format!("invalid --select {}: {}", input, e)),
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

/// True if any selector matches the given file (or if the list is empty).
pub fn any_matches_file(selectors: &[Selector], file_path: &str) -> bool {
    if selectors.is_empty() {
        return true;
    }
    selectors.iter().any(|s| s.matches_file(file_path))
}

/// True if any selector matches the given file + test (or if the list is
/// empty). Selectors without a test component match every test in their
/// file.
pub fn any_matches_test(selectors: &[Selector], file_path: &str, test_name: &str) -> bool {
    if selectors.is_empty() {
        return true;
    }
    selectors
        .iter()
        .any(|s| s.matches_file(file_path) && s.matches_test(test_name))
}

/// True if any selector matches the given file + test + step (or if the
/// list is empty). Selectors without a step component match every step in
/// their matched test.
pub fn any_matches_step(
    selectors: &[Selector],
    file_path: &str,
    test_name: &str,
    step_index: usize,
    step_name: &str,
) -> bool {
    if selectors.is_empty() {
        return true;
    }
    selectors.iter().any(|s| {
        s.matches_file(file_path)
            && s.matches_test(test_name)
            && s.matches_step(step_index, step_name)
    })
}

/// True if any active selector restricts which steps run. If no selector
/// targets this test/file or they all match without step constraints, the
/// runner can skip the per-step filtering loop.
pub fn has_step_level_filter(selectors: &[Selector], file_path: &str, test_name: &str) -> bool {
    selectors
        .iter()
        .any(|s| s.matches_file(file_path) && s.matches_test(test_name) && s.step.is_some())
}

/// Compose a file-only selector (`FILE`).
///
/// This is the shape `tarn run --select` expects for "run every test in
/// this file". Whitespace is trimmed from the file path so callers can
/// feed it directly from a `Url` or display path without pre-cleaning.
///
/// The returned string always round-trips through [`Selector::parse`] —
/// the unit tests below cover this invariant explicitly.
pub fn format_file_selector(file: &str) -> String {
    file.trim().to_owned()
}

/// Compose a test selector (`FILE::TEST`).
///
/// Use this for "run every step in the given test". The test name is
/// written literally — Tarn's selector grammar treats `::` in the step
/// component as part of the name (see
/// [`Selector::parse`]'s `preserves_double_colon_inside_step_name`
/// case), so name components do not need to be escaped.
pub fn format_test_selector(file: &str, test: &str) -> String {
    format!("{}::{}", file.trim(), test)
}

/// Compose a step selector (`FILE::TEST::STEP_INDEX`).
///
/// Uses the zero-based step index rather than the step's `name:` value,
/// matching what the VS Code extension's test controller emits today
/// (`editors/vscode/src/testing/runHandler.ts`). Indices are unique per
/// test and never require escaping, which makes them the robust default
/// for programmatic selector builders.
pub fn format_step_selector(file: &str, test: &str, step_index: usize) -> String {
    format!("{}::{}::{}", file.trim(), test, step_index)
}

fn ensure_non_empty(part: &str, name: &str) -> Result<(), String> {
    if part.is_empty() {
        Err(format!("{} component is empty", name))
    } else {
        Ok(())
    }
}

fn path_suffix_matches(actual: &str, selector_file: &str) -> bool {
    let actual_norm = normalize_path(actual);
    let selector_norm = normalize_path(selector_file);
    if actual_norm == selector_norm {
        return true;
    }
    // Allow selector to be a suffix of actual (e.g. "users.tarn.yaml" matches
    // "tests/users.tarn.yaml"), as long as the match aligns on a path
    // separator so "users.tarn.yaml" does not match "other-users.tarn.yaml".
    if !actual_norm.ends_with(&selector_norm) {
        return false;
    }
    let prefix_len = actual_norm.len() - selector_norm.len();
    if prefix_len == 0 {
        return true;
    }
    actual_norm.as_bytes()[prefix_len - 1] == b'/'
}

fn normalize_path(raw: &str) -> String {
    Path::new(raw)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_only() {
        let s = Selector::parse("tests/users.tarn.yaml").unwrap();
        assert_eq!(s.file, "tests/users.tarn.yaml");
        assert!(s.test.is_none());
        assert!(s.step.is_none());
    }

    #[test]
    fn parses_file_and_test() {
        let s = Selector::parse("tests/users.tarn.yaml::create_user").unwrap();
        assert_eq!(s.file, "tests/users.tarn.yaml");
        assert_eq!(s.test.as_deref(), Some("create_user"));
        assert!(s.step.is_none());
    }

    #[test]
    fn parses_file_test_and_named_step() {
        let s = Selector::parse("tests/users.tarn.yaml::create_user::Create user").unwrap();
        assert_eq!(s.step, Some(StepSelector::Name("Create user".into())));
    }

    #[test]
    fn parses_numeric_step_as_index() {
        let s = Selector::parse("tests/users.tarn.yaml::create_user::2").unwrap();
        assert_eq!(s.step, Some(StepSelector::Index(2)));
    }

    #[test]
    fn preserves_double_colon_inside_step_name() {
        let s = Selector::parse("f.tarn.yaml::t::ns::deep").unwrap();
        assert_eq!(s.step, Some(StepSelector::Name("ns::deep".into())));
    }

    #[test]
    fn rejects_empty_selector() {
        assert!(Selector::parse("").is_err());
        assert!(Selector::parse("   ").is_err());
    }

    #[test]
    fn rejects_empty_components() {
        assert!(Selector::parse("::create").is_err());
        assert!(Selector::parse("f.tarn.yaml::").is_err());
        assert!(Selector::parse("f.tarn.yaml::t::").is_err());
    }

    #[test]
    fn file_match_is_exact_when_equal() {
        let s = Selector::parse("tests/users.tarn.yaml").unwrap();
        assert!(s.matches_file("tests/users.tarn.yaml"));
    }

    #[test]
    fn file_match_works_on_path_suffix() {
        let s = Selector::parse("users.tarn.yaml").unwrap();
        assert!(s.matches_file("tests/users.tarn.yaml"));
        assert!(s.matches_file("/abs/path/tests/users.tarn.yaml"));
    }

    #[test]
    fn file_match_does_not_confuse_partial_filename() {
        let s = Selector::parse("users.tarn.yaml").unwrap();
        assert!(!s.matches_file("tests/other-users.tarn.yaml"));
    }

    #[test]
    fn any_matches_file_returns_true_when_empty_list() {
        assert!(any_matches_file(&[], "tests/any.tarn.yaml"));
    }

    #[test]
    fn any_matches_test_respects_file_boundary() {
        let selectors = vec![Selector::parse("a.tarn.yaml::login").unwrap()];
        assert!(any_matches_test(&selectors, "a.tarn.yaml", "login"));
        assert!(!any_matches_test(&selectors, "b.tarn.yaml", "login"));
        assert!(!any_matches_test(&selectors, "a.tarn.yaml", "logout"));
    }

    #[test]
    fn any_matches_step_with_no_step_selector_runs_all_steps() {
        let selectors = vec![Selector::parse("a.tarn.yaml::login").unwrap()];
        assert!(any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            0,
            "step1"
        ));
        assert!(any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            3,
            "step4"
        ));
    }

    #[test]
    fn any_matches_step_with_index_only_matches_that_index() {
        let selectors = vec![Selector::parse("a.tarn.yaml::login::1").unwrap()];
        assert!(!any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            0,
            "step1"
        ));
        assert!(any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            1,
            "step2"
        ));
    }

    #[test]
    fn any_matches_step_with_name_only_matches_exact_name() {
        let selectors = vec![Selector::parse("a.tarn.yaml::login::Create user").unwrap()];
        assert!(any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            0,
            "Create user"
        ));
        assert!(!any_matches_step(
            &selectors,
            "a.tarn.yaml",
            "login",
            0,
            "Delete user"
        ));
    }

    #[test]
    fn union_semantics_across_selectors() {
        let selectors = vec![
            Selector::parse("a.tarn.yaml::login").unwrap(),
            Selector::parse("b.tarn.yaml::logout").unwrap(),
        ];
        assert!(any_matches_test(&selectors, "a.tarn.yaml", "login"));
        assert!(any_matches_test(&selectors, "b.tarn.yaml", "logout"));
        assert!(!any_matches_test(&selectors, "a.tarn.yaml", "logout"));
    }

    #[test]
    fn has_step_level_filter_detects_step_constraints() {
        let selectors = vec![Selector::parse("a.tarn.yaml::login::1").unwrap()];
        assert!(has_step_level_filter(&selectors, "a.tarn.yaml", "login"));
        let selectors = vec![Selector::parse("a.tarn.yaml::login").unwrap()];
        assert!(!has_step_level_filter(&selectors, "a.tarn.yaml", "login"));
    }

    #[test]
    fn parse_all_collects_errors() {
        let inputs = vec!["good.tarn.yaml".into(), "::bad".into(), "".into()];
        let err = parse_all(&inputs).unwrap_err();
        assert_eq!(err.len(), 2);
    }

    // ---- format_*_selector helpers ----

    #[test]
    fn format_file_selector_trims_whitespace() {
        assert_eq!(
            format_file_selector("  tests/users.tarn.yaml  "),
            "tests/users.tarn.yaml"
        );
    }

    #[test]
    fn format_test_selector_joins_with_double_colon() {
        assert_eq!(
            format_test_selector("tests/users.tarn.yaml", "create_user"),
            "tests/users.tarn.yaml::create_user"
        );
    }

    #[test]
    fn format_step_selector_uses_zero_based_index() {
        assert_eq!(
            format_step_selector("tests/users.tarn.yaml", "create_user", 0),
            "tests/users.tarn.yaml::create_user::0"
        );
        assert_eq!(
            format_step_selector("tests/users.tarn.yaml", "create_user", 3),
            "tests/users.tarn.yaml::create_user::3"
        );
    }

    #[test]
    fn format_test_selector_roundtrips_through_parse() {
        let composed = format_test_selector("tests/users.tarn.yaml", "create user");
        let parsed = Selector::parse(&composed).unwrap();
        assert_eq!(parsed.file, "tests/users.tarn.yaml");
        assert_eq!(parsed.test.as_deref(), Some("create user"));
        assert!(parsed.step.is_none());
    }

    #[test]
    fn format_step_selector_roundtrips_to_index_step() {
        let composed = format_step_selector("tests/users.tarn.yaml", "create_user", 2);
        let parsed = Selector::parse(&composed).unwrap();
        assert_eq!(parsed.step, Some(StepSelector::Index(2)));
    }

    #[test]
    fn format_helpers_handle_names_with_special_characters() {
        // Test names may carry hyphens, spaces, punctuation. The grammar
        // does not require escaping — the test name is written verbatim
        // and the parser splits on the first `::` boundaries only.
        let composed = format_test_selector("f.tarn.yaml", "GET /users/{id} flow");
        assert_eq!(composed, "f.tarn.yaml::GET /users/{id} flow");
        let parsed = Selector::parse(&composed).unwrap();
        assert_eq!(parsed.test.as_deref(), Some("GET /users/{id} flow"));
    }
}
