//! `tarn diff` — compare two runs' summary + failure sets (NAZ-405).
//!
//! Given two run ids, `tarn diff` loads `summary.json` and
//! `failures.json` for each run and emits:
//!
//! - a **totals delta** (passed/failed counts and duration) and
//! - a **failure set delta** classifying each failure fingerprint as
//!   `new` (in `to` but not `from`), `fixed` (in `from` but not `to`),
//!   or `persistent` (in both).
//!
//! Fingerprinting reuses `report::failures_command::fingerprint_for`
//! so diff output collapses the same cascade/sibling noise that the
//! `tarn failures` view already deduplicates.
//!
//! # JSON schema (stable)
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "from": { "run_id": "…", "path": "…/summary.json" },
//!   "to":   { "run_id": "…", "path": "…/summary.json" },
//!   "totals_delta": {
//!     "files": 0, "tests": 0, "steps": 0,
//!     "failed_files": -1, "failed_tests": -2, "failed_steps": -3,
//!     "duration_ms": -120
//!   },
//!   "new":        [ { "fingerprint": "…", "count": N, "exemplar": {…} } ],
//!   "fixed":      [ … ],
//!   "persistent": [ … ]
//! }
//! ```
//!
//! Each bucket entry carries a `count` of matching failure entries in
//! the source bucket and an `exemplar` with the concrete `file/test/
//! step/category/message` of the first such entry — enough to decide
//! "what broke" without loading the full failures doc.

use crate::report::failures_command::fingerprint_for;
use crate::report::run_dir::{resolve_run_id, run_directory};
use crate::report::summary::{FailureEntry, FailuresDoc, SummaryDoc};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DIFF_SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub enum DiffError {
    NotFound(PathBuf),
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    Parse {
        path: PathBuf,
        error: serde_json::Error,
    },
    UnknownRunId {
        alias: String,
        error: std::io::Error,
    },
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NotFound(p) => write!(f, "missing artifact at {}", p.display()),
            DiffError::Io { path, error } => {
                write!(f, "failed to read {}: {}", path.display(), error)
            }
            DiffError::Parse { path, error } => {
                write!(f, "failed to parse {}: {}", path.display(), error)
            }
            DiffError::UnknownRunId { alias, error } => {
                write!(f, "unknown run id '{}': {}", alias, error)
            }
        }
    }
}

impl std::error::Error for DiffError {}

/// Inputs for the two sides of a diff.
#[derive(Debug, Clone)]
pub struct DiffSide {
    pub run_id: String,
    pub summary_path: PathBuf,
    pub failures_path: PathBuf,
}

/// Filter knobs applied after fingerprints are computed. All three
/// filters AND together.
#[derive(Debug, Clone, Default)]
pub struct DiffFilters<'a> {
    pub file: Option<&'a str>,
    pub test: Option<&'a str>,
    pub category: Option<&'a str>,
}

impl<'a> DiffFilters<'a> {
    fn matches(&self, entry: &FailureEntry) -> bool {
        if let Some(f) = self.file {
            // Filter on the root_cause file when present so cascade
            // fallout stays grouped with its origin; fall back to the
            // entry's own file when the root cause is unresolved.
            let candidate = entry
                .root_cause
                .as_ref()
                .map(|rc| rc.file.as_str())
                .unwrap_or(entry.file.as_str());
            if candidate != f {
                return false;
            }
        }
        if let Some(t) = self.test {
            let candidate = entry
                .root_cause
                .as_ref()
                .map(|rc| rc.test.as_str())
                .unwrap_or(entry.test.as_str());
            if candidate != t {
                return false;
            }
        }
        if let Some(cat) = self.category {
            let entry_cat = entry
                .failure_category
                .map(failure_category_as_str)
                .unwrap_or("");
            if !entry_cat.eq_ignore_ascii_case(cat) {
                return false;
            }
        }
        true
    }
}

/// Resolve `last` / `prev` / bare run id to a concrete `DiffSide`.
/// Mirrors [`crate::report::inspect::resolve_source`] but reads the
/// summary and failures pair (no need for the heavier `report.json`).
pub fn resolve_side(workspace_root: &Path, alias: &str) -> Result<DiffSide, DiffError> {
    let run_id =
        resolve_run_id(workspace_root, alias).map_err(|error| DiffError::UnknownRunId {
            alias: alias.to_string(),
            error,
        })?;
    let dir = run_directory(workspace_root, &run_id);
    let summary_path = dir.join("summary.json");
    let failures_path = dir.join("failures.json");
    if !summary_path.is_file() {
        return Err(DiffError::NotFound(summary_path));
    }
    if !failures_path.is_file() {
        return Err(DiffError::NotFound(failures_path));
    }
    Ok(DiffSide {
        run_id,
        summary_path,
        failures_path,
    })
}

/// Load the summary+failures pair for a side from disk.
pub fn load_side(side: &DiffSide) -> Result<(SummaryDoc, FailuresDoc), DiffError> {
    let summary: SummaryDoc = read_json(&side.summary_path)?;
    let failures: FailuresDoc = read_json(&side.failures_path)?;
    Ok((summary, failures))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, DiffError> {
    let bytes = std::fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DiffError::NotFound(path.to_path_buf())
        } else {
            DiffError::Io {
                path: path.to_path_buf(),
                error,
            }
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|error| DiffError::Parse {
        path: path.to_path_buf(),
        error,
    })
}

/// Pure derivation: given both sides' artifacts, produce the diff
/// envelope. Unit-testable without touching the filesystem.
pub fn build_diff(
    from_side: &DiffSide,
    from_summary: &SummaryDoc,
    from_failures: &FailuresDoc,
    to_side: &DiffSide,
    to_summary: &SummaryDoc,
    to_failures: &FailuresDoc,
    filters: &DiffFilters<'_>,
) -> Value {
    let from_map = index_by_fingerprint(&from_failures.failures, filters);
    let to_map = index_by_fingerprint(&to_failures.failures, filters);

    let mut new_bucket: Vec<FingerprintEntry> = Vec::new();
    let mut fixed_bucket: Vec<FingerprintEntry> = Vec::new();
    let mut persistent_bucket: Vec<FingerprintEntry> = Vec::new();

    // `to`-side entries are either new (absent in `from`) or persistent.
    for (fp, entries) in &to_map {
        if from_map.contains_key(fp) {
            persistent_bucket.push(FingerprintEntry::from(fp.clone(), entries));
        } else {
            new_bucket.push(FingerprintEntry::from(fp.clone(), entries));
        }
    }
    // Anything in `from` that isn't in `to` got fixed between runs.
    for (fp, entries) in &from_map {
        if !to_map.contains_key(fp) {
            fixed_bucket.push(FingerprintEntry::from(fp.clone(), entries));
        }
    }

    json!({
        "schema_version": DIFF_SCHEMA_VERSION,
        "from": side_json(from_side, from_summary),
        "to":   side_json(to_side, to_summary),
        "totals_delta": totals_delta_json(from_summary, to_summary),
        "new":        new_bucket.iter().map(FingerprintEntry::to_json).collect::<Vec<_>>(),
        "fixed":      fixed_bucket.iter().map(FingerprintEntry::to_json).collect::<Vec<_>>(),
        "persistent": persistent_bucket.iter().map(FingerprintEntry::to_json).collect::<Vec<_>>(),
        "filters": json!({
            "file": filters.file,
            "test": filters.test,
            "category": filters.category,
        }),
    })
}

fn index_by_fingerprint<'a>(
    failures: &'a [FailureEntry],
    filters: &DiffFilters<'_>,
) -> BTreeMap<String, Vec<&'a FailureEntry>> {
    let mut out: BTreeMap<String, Vec<&'a FailureEntry>> = BTreeMap::new();
    for entry in failures {
        if !filters.matches(entry) {
            continue;
        }
        let fp = fingerprint_for(entry);
        out.entry(fp).or_default().push(entry);
    }
    out
}

fn side_json(side: &DiffSide, summary: &SummaryDoc) -> Value {
    json!({
        "run_id": side.run_id,
        "summary_path": side.summary_path.display().to_string(),
        "failures_path": side.failures_path.display().to_string(),
        "exit_code": summary.exit_code,
        "duration_ms": summary.duration_ms,
        "totals": {
            "files": summary.totals.files,
            "tests": summary.totals.tests,
            "steps": summary.totals.steps,
        },
        "failed": {
            "files": summary.failed.files,
            "tests": summary.failed.tests,
            "steps": summary.failed.steps,
        },
    })
}

fn totals_delta_json(from: &SummaryDoc, to: &SummaryDoc) -> Value {
    // Subtracting `usize`s directly would underflow on regression; do
    // the math in `i64` so the delta is signed.
    fn delta(a: usize, b: usize) -> i64 {
        b as i64 - a as i64
    }
    json!({
        "files": delta(from.totals.files, to.totals.files),
        "tests": delta(from.totals.tests, to.totals.tests),
        "steps": delta(from.totals.steps, to.totals.steps),
        "failed_files": delta(from.failed.files, to.failed.files),
        "failed_tests": delta(from.failed.tests, to.failed.tests),
        "failed_steps": delta(from.failed.steps, to.failed.steps),
        "duration_ms": to.duration_ms as i64 - from.duration_ms as i64,
    })
}

/// One entry in the new/fixed/persistent bucket. Keeps the first
/// failure we saw for the fingerprint as the exemplar so output is
/// stable across repeated invocations on identical inputs.
struct FingerprintEntry {
    fingerprint: String,
    count: usize,
    exemplar: FailureEntry,
}

impl FingerprintEntry {
    fn from(fingerprint: String, entries: &[&FailureEntry]) -> Self {
        let count = entries.len();
        let exemplar = entries[0].clone();
        Self {
            fingerprint,
            count,
            exemplar,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "fingerprint": self.fingerprint,
            "count": self.count,
            "exemplar": {
                "file": self.exemplar.file,
                "test": self.exemplar.test,
                "step": self.exemplar.step,
                "category": self
                    .exemplar
                    .failure_category
                    .map(failure_category_as_str),
                "message": self.exemplar.message,
            },
        })
    }
}

fn failure_category_as_str(cat: crate::assert::types::FailureCategory) -> &'static str {
    use crate::assert::types::FailureCategory::*;
    match cat {
        AssertionFailed => "assertion_failed",
        ConnectionError => "connection_error",
        Timeout => "timeout",
        ParseError => "parse_error",
        CaptureError => "capture_error",
        UnresolvedTemplate => "unresolved_template",
        SkippedDueToFailedCapture => "skipped_due_to_failed_capture",
        SkippedDueToFailFast => "skipped_due_to_fail_fast",
        SkippedByCondition => "skipped_by_condition",
    }
}

/// Render the diff envelope as JSON text.
pub fn render_json(view: &Value) -> String {
    serde_json::to_string_pretty(view).expect("diff view is always JSON-serializable")
}

/// Render the diff envelope as human-readable text — counts, then one
/// bullet per fingerprint in each bucket.
pub fn render_human(view: &Value) -> String {
    let mut out = String::new();
    let from_id = view
        .get("from")
        .and_then(|s| s.get("run_id"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let to_id = view
        .get("to")
        .and_then(|s| s.get("run_id"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    out.push_str(&format!("diff: {} -> {}\n", from_id, to_id));

    if let Some(deltas) = view.get("totals_delta") {
        out.push_str(&format!(
            "totals_delta: failed_files={} failed_tests={} failed_steps={} steps={} duration_ms={}\n",
            deltas
                .get("failed_files")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            deltas
                .get("failed_tests")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            deltas
                .get("failed_steps")
                .and_then(Value::as_i64)
                .unwrap_or(0),
            deltas.get("steps").and_then(Value::as_i64).unwrap_or(0),
            deltas
                .get("duration_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        ));
    }

    render_bucket_human("new", view.get("new"), &mut out);
    render_bucket_human("fixed", view.get("fixed"), &mut out);
    render_bucket_human("persistent", view.get("persistent"), &mut out);

    out
}

fn render_bucket_human(label: &str, bucket: Option<&Value>, out: &mut String) {
    let empty = Vec::new();
    let entries = bucket.and_then(Value::as_array).unwrap_or(&empty);
    out.push_str(&format!("{}: {}\n", label, entries.len()));
    for entry in entries {
        let fp = entry
            .get("fingerprint")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let count = entry.get("count").and_then(Value::as_u64).unwrap_or(0);
        out.push_str(&format!("  - [{}×] {}\n", count, fp));
        if let Some(ex) = entry.get("exemplar") {
            out.push_str(&format!(
                "      {} :: {} :: {}\n",
                ex.get("file").and_then(Value::as_str).unwrap_or("?"),
                ex.get("test").and_then(Value::as_str).unwrap_or("?"),
                ex.get("step").and_then(Value::as_str).unwrap_or("?"),
            ));
            if let Some(msg) = ex.get("message").and_then(Value::as_str) {
                let first_line = msg.lines().next().unwrap_or(msg);
                if !first_line.is_empty() {
                    out.push_str(&format!("      {}\n", first_line));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assert::types::FailureCategory;
    use crate::report::summary::{
        Counts, FailureEntry, FailuresDoc, SummaryDoc, SUMMARY_SCHEMA_VERSION,
    };

    fn entry(
        file: &str,
        test: &str,
        step: &str,
        category: FailureCategory,
        message: &str,
    ) -> FailureEntry {
        FailureEntry {
            file: file.into(),
            test: test.into(),
            step: step.into(),
            failure_category: Some(category),
            message: message.into(),
            request: None,
            response: None,
            root_cause: None,
        }
    }

    fn summary_with_counts(failed_tests: usize, failed_steps: usize) -> SummaryDoc {
        SummaryDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            started_at: "2026-01-01T00:00:00Z".into(),
            ended_at: "2026-01-01T00:00:00Z".into(),
            duration_ms: 100,
            exit_code: if failed_steps > 0 { 1 } else { 0 },
            totals: Counts {
                files: 1,
                tests: failed_tests + 1,
                steps: failed_steps + 2,
            },
            failed: Counts {
                files: if failed_steps > 0 { 1 } else { 0 },
                tests: failed_tests,
                steps: failed_steps,
            },
            failed_files: if failed_steps > 0 {
                vec!["a.tarn.yaml".into()]
            } else {
                vec![]
            },
            rerun_source: None,
        }
    }

    fn side(run_id: &str) -> DiffSide {
        DiffSide {
            run_id: run_id.into(),
            summary_path: PathBuf::from(format!("/tmp/{}/summary.json", run_id)),
            failures_path: PathBuf::from(format!("/tmp/{}/failures.json", run_id)),
        }
    }

    fn failures_doc(entries: Vec<FailureEntry>) -> FailuresDoc {
        FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            failures: entries,
        }
    }

    #[test]
    fn fingerprints_only_in_to_are_new() {
        let from = failures_doc(vec![]);
        let to = failures_doc(vec![entry(
            "a.tarn.yaml",
            "t",
            "s",
            FailureCategory::AssertionFailed,
            "JSONPath $.uuid did not match any value",
        )]);
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(0, 0),
            &from,
            &side("b"),
            &summary_with_counts(1, 1),
            &to,
            &DiffFilters::default(),
        );
        assert_eq!(diff["new"].as_array().unwrap().len(), 1);
        assert_eq!(diff["fixed"].as_array().unwrap().len(), 0);
        assert_eq!(diff["persistent"].as_array().unwrap().len(), 0);
        assert_eq!(
            diff["new"][0]["fingerprint"],
            "body_jsonpath:$.uuid:missing"
        );
    }

    #[test]
    fn fingerprints_only_in_from_are_fixed() {
        let from = failures_doc(vec![entry(
            "a.tarn.yaml",
            "t",
            "s",
            FailureCategory::AssertionFailed,
            "JSONPath $.uuid did not match any value",
        )]);
        let to = failures_doc(vec![]);
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(1, 1),
            &from,
            &side("b"),
            &summary_with_counts(0, 0),
            &to,
            &DiffFilters::default(),
        );
        assert_eq!(diff["new"].as_array().unwrap().len(), 0);
        assert_eq!(diff["fixed"].as_array().unwrap().len(), 1);
        assert_eq!(diff["fixed"][0]["count"], 1);
    }

    #[test]
    fn fingerprints_present_on_both_sides_are_persistent() {
        let msg = "JSONPath $.uuid did not match any value";
        let from = failures_doc(vec![entry(
            "a.tarn.yaml",
            "t",
            "s",
            FailureCategory::AssertionFailed,
            msg,
        )]);
        let to = failures_doc(vec![entry(
            "a.tarn.yaml",
            "t",
            "s",
            FailureCategory::AssertionFailed,
            msg,
        )]);
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(1, 1),
            &from,
            &side("b"),
            &summary_with_counts(1, 1),
            &to,
            &DiffFilters::default(),
        );
        assert_eq!(diff["persistent"].as_array().unwrap().len(), 1);
        assert_eq!(diff["new"].as_array().unwrap().len(), 0);
        assert_eq!(diff["fixed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn totals_delta_is_signed() {
        let from = failures_doc(vec![]);
        let to = failures_doc(vec![]);
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(3, 5),
            &from,
            &side("b"),
            &summary_with_counts(1, 2),
            &to,
            &DiffFilters::default(),
        );
        assert_eq!(diff["totals_delta"]["failed_tests"], -2);
        assert_eq!(diff["totals_delta"]["failed_steps"], -3);
    }

    #[test]
    fn file_filter_narrows_each_bucket() {
        let from = failures_doc(vec![
            entry(
                "a.tarn.yaml",
                "t",
                "s",
                FailureCategory::AssertionFailed,
                "JSONPath $.a did not match any value",
            ),
            entry(
                "b.tarn.yaml",
                "t",
                "s",
                FailureCategory::AssertionFailed,
                "JSONPath $.b did not match any value",
            ),
        ]);
        let to = failures_doc(vec![]);
        let filters = DiffFilters {
            file: Some("a.tarn.yaml"),
            ..DiffFilters::default()
        };
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(2, 2),
            &from,
            &side("b"),
            &summary_with_counts(0, 0),
            &to,
            &filters,
        );
        let fixed = diff["fixed"].as_array().unwrap();
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0]["exemplar"]["file"], "a.tarn.yaml");
    }

    #[test]
    fn category_filter_narrows_by_failure_category() {
        let from = failures_doc(vec![
            entry(
                "a.tarn.yaml",
                "t",
                "s",
                FailureCategory::AssertionFailed,
                "Expected HTTP status 200, got 500",
            ),
            entry(
                "a.tarn.yaml",
                "u",
                "net",
                FailureCategory::ConnectionError,
                "connection refused",
            ),
        ]);
        let to = failures_doc(vec![]);
        let filters = DiffFilters {
            category: Some("connection_error"),
            ..DiffFilters::default()
        };
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(2, 2),
            &from,
            &side("b"),
            &summary_with_counts(0, 0),
            &to,
            &filters,
        );
        let fixed = diff["fixed"].as_array().unwrap();
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0]["exemplar"]["category"], "connection_error");
    }

    #[test]
    fn filters_compose_file_and_test() {
        let from = failures_doc(vec![
            entry(
                "a.tarn.yaml",
                "keep",
                "s",
                FailureCategory::AssertionFailed,
                "JSONPath $.x did not match any value",
            ),
            entry(
                "a.tarn.yaml",
                "drop",
                "s",
                FailureCategory::AssertionFailed,
                "JSONPath $.y did not match any value",
            ),
        ]);
        let to = failures_doc(vec![]);
        let filters = DiffFilters {
            file: Some("a.tarn.yaml"),
            test: Some("keep"),
            ..DiffFilters::default()
        };
        let diff = build_diff(
            &side("a"),
            &summary_with_counts(2, 2),
            &from,
            &side("b"),
            &summary_with_counts(0, 0),
            &to,
            &filters,
        );
        let fixed = diff["fixed"].as_array().unwrap();
        assert_eq!(fixed.len(), 1);
        assert_eq!(fixed[0]["exemplar"]["test"], "keep");
    }

    #[test]
    fn render_human_lists_each_bucket() {
        let from = failures_doc(vec![entry(
            "a.tarn.yaml",
            "t",
            "s",
            FailureCategory::AssertionFailed,
            "JSONPath $.kept did not match any value",
        )]);
        let to = failures_doc(vec![
            entry(
                "a.tarn.yaml",
                "t",
                "s",
                FailureCategory::AssertionFailed,
                "JSONPath $.kept did not match any value",
            ),
            entry(
                "a.tarn.yaml",
                "t",
                "s2",
                FailureCategory::AssertionFailed,
                "JSONPath $.new did not match any value",
            ),
        ]);
        let diff = build_diff(
            &side("from-run"),
            &summary_with_counts(1, 1),
            &from,
            &side("to-run"),
            &summary_with_counts(1, 2),
            &to,
            &DiffFilters::default(),
        );
        let human = render_human(&diff);
        assert!(human.contains("diff: from-run -> to-run"));
        assert!(human.contains("persistent: 1"));
        assert!(human.contains("new: 1"));
        assert!(human.contains("fixed: 0"));
    }
}
