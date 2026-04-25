//! Rerun-target resolution from a prior run's `failures.json` (NAZ-403).
//!
//! `tarn rerun --failed` loads a `FailuresDoc` produced by NAZ-401 and
//! derives the minimum selector set needed to rerun just the failing
//! subset of the source run. This module is the single source of truth
//! for that translation: the CLI subcommand is thin glue on top of
//! [`load_selection`].
//!
//! # Granularity
//!
//! Selection is **test-level** by default: each `(file, test)` coordinate
//! pair in the failures doc becomes one `FILE::TEST` selector. Duplicates
//! collapse. Setup/teardown failures (carrying the reserved slugs
//! [`crate::fixtures::SETUP_TEST_SLUG`] / [`crate::fixtures::TEARDOWN_TEST_SLUG`])
//! cannot be rerun in isolation — their fixtures feed every test in the
//! file — so they escalate to a whole-file selector (`FILE`) instead.
//! Step-level granularity is deliberately not produced here; users who
//! want a single step can still pass `--select FILE::TEST::STEP` on the
//! rerun invocation (selectors compose).
//!
//! # JSON surface
//!
//! [`RerunSource`] is the shape injected into the new run's report and
//! summary documents under `rerun_source`. Consumers use it to chain
//! reruns (rerun a rerun) and to audit which historical run seeded the
//! current one.

use crate::fixtures::{SETUP_TEST_SLUG, TEARDOWN_TEST_SLUG};
use crate::report::summary::{FailureEntry, FailuresDoc};
use crate::selector::{format_file_selector, format_test_selector, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Origin of a `tarn rerun` invocation, stamped into the new run's
/// artifacts so automation can chain reruns without re-parsing paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerunSource {
    /// Stable id of the run whose failures seeded this rerun. `None`
    /// when the source `failures.json` did not carry one (legacy
    /// archives or hand-rolled inputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Display-only path to the `failures.json` we loaded. Carried as a
    /// string so serialization is trivially JSON-stable regardless of
    /// platform separator.
    pub source_path: String,
    /// Number of `(file, test)` / `file` targets we derived from the
    /// source doc. Matches the selector count the runner receives.
    pub selected_count: usize,
}

/// A single rerun target. Carries both the selector string the runner
/// consumes and the user-visible `FILE::TEST` / `FILE` label we print
/// on stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunTarget {
    pub file: String,
    /// `None` when the entry targets the whole file (setup/teardown
    /// failure) — produces a `FILE` selector.
    pub test: Option<String>,
}

impl RerunTarget {
    /// Stable human label shown on stderr: `FILE::TEST` or `FILE`.
    pub fn label(&self) -> String {
        match &self.test {
            Some(test) => format!("{}::{}", self.file, test),
            None => self.file.clone(),
        }
    }

    /// Translate into the runner's selector grammar. Infallible — the
    /// strings we pull from `failures.json` are always non-empty
    /// because NAZ-401 populates them.
    pub fn to_selector(&self) -> Selector {
        let raw = match &self.test {
            Some(test) => format_test_selector(&self.file, test),
            None => format_file_selector(&self.file),
        };
        Selector::parse(&raw).expect("rerun target produces a non-empty selector")
    }
}

/// Resolved selection for a rerun.
#[derive(Debug, Clone)]
pub struct RerunSelection {
    pub source: RerunSource,
    pub targets: Vec<RerunTarget>,
}

impl RerunSelection {
    /// Unique set of files the targets touch. Feeds the discovery step
    /// so we don't walk the whole workspace when the rerun only touches
    /// a handful of files.
    pub fn files(&self) -> Vec<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut out = Vec::new();
        for target in &self.targets {
            if seen.insert(target.file.clone()) {
                out.push(target.file.clone());
            }
        }
        out
    }

    pub fn selectors(&self) -> Vec<Selector> {
        self.targets.iter().map(|t| t.to_selector()).collect()
    }
}

/// Where the failures artifact lives.
#[derive(Debug, Clone)]
pub enum RerunSourcePath {
    /// `<workspace>/.tarn/failures.json` — the latest-run pointer.
    LatestPointer(PathBuf),
    /// `<workspace>/.tarn/runs/<id>/failures.json` — a specific run's
    /// archive.
    Archive { run_id: String, path: PathBuf },
}

impl RerunSourcePath {
    /// Path string for both stderr announcements and the `source_path`
    /// field on `RerunSource`. Forward-slash on every platform so JSON
    /// artifacts are byte-identical across Unix and Windows; PowerShell
    /// and CMD both accept `/` paths so the stderr UX still works.
    pub fn display_path(&self) -> String {
        match self {
            RerunSourcePath::LatestPointer(p) => crate::path_util::to_forward_slash(p),
            RerunSourcePath::Archive { path, .. } => crate::path_util::to_forward_slash(path),
        }
    }

    fn filesystem_path(&self) -> &Path {
        match self {
            RerunSourcePath::LatestPointer(p) => p,
            RerunSourcePath::Archive { path, .. } => path,
        }
    }
}

/// Errors `load_selection` can surface. The CLI maps every variant to
/// exit 2 (configuration/parse error) — `AllPassing` is signaled
/// separately because it is not an error from the user's point of view.
#[derive(Debug)]
pub enum RerunError {
    /// The failures.json path does not exist.
    NotFound(PathBuf),
    /// Read or UTF-8 decode failed.
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The file was present but not a valid `FailuresDoc`.
    Parse {
        path: PathBuf,
        error: serde_json::Error,
    },
}

impl std::fmt::Display for RerunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RerunError::NotFound(path) => write!(
                f,
                "no failures.json at {}. Run `tarn run` first.",
                path.display()
            ),
            RerunError::Io { path, error } => {
                write!(f, "failed to read {}: {}", path.display(), error)
            }
            RerunError::Parse { path, error } => {
                write!(f, "failed to parse {}: {}", path.display(), error)
            }
        }
    }
}

impl std::error::Error for RerunError {}

/// Load a `FailuresDoc` from disk and derive the rerun selection.
pub fn load_selection(source: &RerunSourcePath) -> Result<RerunSelection, RerunError> {
    let path = source.filesystem_path();
    if !path.is_file() {
        return Err(RerunError::NotFound(path.to_path_buf()));
    }
    let raw = std::fs::read(path).map_err(|error| RerunError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let doc: FailuresDoc = serde_json::from_slice(&raw).map_err(|error| RerunError::Parse {
        path: path.to_path_buf(),
        error,
    })?;
    Ok(build_selection(&doc, source))
}

/// Pure derivation from a parsed doc — exposed so unit tests can drive
/// it without touching the filesystem.
pub fn build_selection(doc: &FailuresDoc, source: &RerunSourcePath) -> RerunSelection {
    let targets = derive_targets(&doc.failures);
    let source = RerunSource {
        run_id: doc.run_id.clone(),
        source_path: source.display_path(),
        selected_count: targets.len(),
    };
    RerunSelection { source, targets }
}

fn derive_targets(failures: &[FailureEntry]) -> Vec<RerunTarget> {
    // De-dupe on (file, test) coordinates while preserving the order of
    // first appearance so output is stable for the same failures doc.
    let mut seen: BTreeSet<(String, Option<String>)> = BTreeSet::new();
    let mut out: Vec<RerunTarget> = Vec::new();
    for entry in failures {
        let test = if entry.test == SETUP_TEST_SLUG || entry.test == TEARDOWN_TEST_SLUG {
            None
        } else {
            Some(entry.test.clone())
        };
        let key = (entry.file.clone(), test.clone());
        if seen.insert(key.clone()) {
            out.push(RerunTarget {
                file: key.0,
                test: key.1,
            });
        }
    }
    out
}

/// Compose the selection with any user-provided selectors: union
/// semantics per NAZ-403 acceptance criterion ("if the user passes
/// both `--failed` AND `--select`, both apply").
pub fn union_with_user_selectors(
    selection_selectors: Vec<Selector>,
    user_selectors: Vec<Selector>,
) -> Vec<Selector> {
    let mut out = selection_selectors;
    out.extend(user_selectors);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::FailureCategory;
    use crate::report::summary::{FailuresDoc, SUMMARY_SCHEMA_VERSION};
    use tempfile::TempDir;

    fn entry(file: &str, test: &str) -> FailureEntry {
        FailureEntry {
            file: file.into(),
            test: test.into(),
            step: "some step".into(),
            failure_category: Some(FailureCategory::AssertionFailed),
            message: "boom".into(),
            request: None,
            response: None,
            root_cause: None,
            response_shape_mismatch: None,
        }
    }

    fn doc(failures: Vec<FailureEntry>) -> FailuresDoc {
        FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("r-123".into()),
            failures,
        }
    }

    fn pointer(path: PathBuf) -> RerunSourcePath {
        RerunSourcePath::LatestPointer(path)
    }

    #[test]
    fn failing_test_produces_one_file_test_selector() {
        let d = doc(vec![entry("a.tarn.yaml", "login")]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.targets.len(), 1);
        assert_eq!(sel.targets[0].file, "a.tarn.yaml");
        assert_eq!(sel.targets[0].test.as_deref(), Some("login"));
        assert_eq!(sel.targets[0].label(), "a.tarn.yaml::login");
        let selectors = sel.selectors();
        assert_eq!(selectors.len(), 1);
        assert_eq!(selectors[0].file, "a.tarn.yaml");
        assert_eq!(selectors[0].test.as_deref(), Some("login"));
        assert_eq!(sel.source.run_id.as_deref(), Some("r-123"));
        assert_eq!(sel.source.selected_count, 1);
    }

    #[test]
    fn setup_failure_escalates_to_whole_file() {
        let d = doc(vec![entry("a.tarn.yaml", SETUP_TEST_SLUG)]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.targets.len(), 1);
        assert_eq!(sel.targets[0].test, None);
        assert_eq!(sel.targets[0].label(), "a.tarn.yaml");
        let selectors = sel.selectors();
        assert!(selectors[0].test.is_none());
    }

    #[test]
    fn teardown_failure_escalates_to_whole_file() {
        let d = doc(vec![entry("a.tarn.yaml", TEARDOWN_TEST_SLUG)]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.targets.len(), 1);
        assert_eq!(sel.targets[0].test, None);
    }

    #[test]
    fn duplicate_entries_collapse_to_one_target() {
        let d = doc(vec![
            entry("a.tarn.yaml", "login"),
            entry("a.tarn.yaml", "login"),
        ]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.targets.len(), 1);
    }

    #[test]
    fn multiple_tests_in_same_file_produce_multiple_targets() {
        let d = doc(vec![
            entry("a.tarn.yaml", "login"),
            entry("a.tarn.yaml", "logout"),
        ]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.targets.len(), 2);
        let labels: Vec<String> = sel.targets.iter().map(|t| t.label()).collect();
        assert_eq!(
            labels,
            vec![
                "a.tarn.yaml::login".to_string(),
                "a.tarn.yaml::logout".to_string()
            ]
        );
    }

    #[test]
    fn empty_failures_yields_empty_selection() {
        let d = doc(vec![]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert!(sel.targets.is_empty());
        assert_eq!(sel.source.selected_count, 0);
    }

    #[test]
    fn files_returns_unique_file_list_preserving_order() {
        let d = doc(vec![
            entry("b.tarn.yaml", "t"),
            entry("a.tarn.yaml", "t"),
            entry("b.tarn.yaml", "u"),
        ]);
        let sel = build_selection(&d, &pointer(PathBuf::from(".tarn/failures.json")));
        assert_eq!(sel.files(), vec!["b.tarn.yaml", "a.tarn.yaml"]);
    }

    #[test]
    fn missing_file_returns_not_found() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("does-not-exist.json");
        let err = load_selection(&pointer(missing.clone())).unwrap_err();
        match err {
            RerunError::NotFound(p) => assert_eq!(p, missing),
            other => panic!("expected NotFound, got {:?}", other),
        }
    }

    #[test]
    fn malformed_json_returns_parse_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("failures.json");
        std::fs::write(&path, b"{not valid json").unwrap();
        let err = load_selection(&pointer(path.clone())).unwrap_err();
        assert!(matches!(err, RerunError::Parse { .. }));
    }

    #[test]
    fn roundtrip_via_filesystem_produces_expected_targets() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("failures.json");
        let d = doc(vec![
            entry("tests/users.tarn.yaml", "create_user"),
            entry("tests/users.tarn.yaml", "create_user"),
            entry("tests/auth.tarn.yaml", SETUP_TEST_SLUG),
        ]);
        std::fs::write(&path, serde_json::to_vec_pretty(&d).unwrap()).unwrap();
        let sel = load_selection(&pointer(path.clone())).unwrap();
        assert_eq!(sel.targets.len(), 2);
        assert_eq!(
            sel.source.source_path,
            crate::path_util::to_forward_slash(&path)
        );
    }

    #[test]
    fn archive_source_path_uses_archive_display_path() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("failures.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&doc(vec![])).unwrap()).unwrap();
        let src = RerunSourcePath::Archive {
            run_id: "r-1".into(),
            path: path.clone(),
        };
        let sel = load_selection(&src).unwrap();
        assert_eq!(
            sel.source.source_path,
            crate::path_util::to_forward_slash(&path)
        );
    }

    #[test]
    fn union_with_user_selectors_preserves_both_sets() {
        let rerun_sel = vec![Selector::parse("a.tarn.yaml::t").unwrap()];
        let user_sel = vec![Selector::parse("b.tarn.yaml").unwrap()];
        let union = union_with_user_selectors(rerun_sel, user_sel);
        assert_eq!(union.len(), 2);
    }
}
