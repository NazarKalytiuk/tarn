//! Root-cause grouping for a run's `failures.json` (NAZ-402).
//!
//! The plain list of failures produced by NAZ-401 is flat: one entry
//! per failing step, including cascade fallout. A capture that misses
//! in step A typically manifests as N "skipped_due_to_failed_capture"
//! entries in the same test, plus any other test that shared the
//! broken setup. Showing them all as peers trains the eye to ignore
//! repetition instead of focusing on the single broken thing.
//!
//! This module loads a `FailuresDoc`, assigns each entry a stable
//! fingerprint, and folds same-fingerprint entries into a single group
//! with a canonical `root_cause` exemplar and a `blocked_steps` list
//! for the downstream skips. Rendering happens in two shapes: a human
//! one-screen view and a stable JSON envelope for automation.
//!
//! # JSON output schema
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "run_id": "…",
//!   "source": ".tarn/failures.json",
//!   "total_failures": 5,
//!   "total_cascades": 2,
//!   "groups": [
//!     {
//!       "fingerprint": "body_jsonpath:$.uuid:missing",
//!       "occurrences": 2,
//!       "root_cause": { "file": "…", "test": "…", "step": "…",
//!                        "category": "assertion_failed", "message": "…",
//!                        "request": {…}, "response": {…} },
//!       "affected": [ {"file": "…", "test": "…"} ],
//!       "blocked_steps": [ {"file": "…", "test": "…", "step": "…"} ]
//!     }
//!   ]
//! }
//! ```

use crate::assert::types::FailureCategory;
use crate::report::summary::{FailureEntry, FailureRequest, FailureResponse, FailuresDoc};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// Bumped on incompatible changes to the grouped-failures envelope.
pub const FAILURES_REPORT_SCHEMA_VERSION: u32 = 1;

/// Top-level envelope for the grouped failures report. Mirrors the
/// JSON emitted by `tarn failures --format json`.
#[derive(Debug, Clone, Serialize)]
pub struct FailuresReport {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub source: String,
    pub total_failures: usize,
    pub total_cascades: usize,
    pub groups: Vec<FailureGroup>,
}

/// A set of failures that share a fingerprint — one root cause
/// plus any sibling occurrences and any downstream skips it blocked.
#[derive(Debug, Clone, Serialize)]
pub struct FailureGroup {
    pub fingerprint: String,
    pub occurrences: usize,
    pub root_cause: RootCauseExemplar,
    pub affected: Vec<AffectedLocation>,
    pub blocked_steps: Vec<BlockedStep>,
}

/// The canonical exemplar for a group — the first-encountered failure
/// that produced this fingerprint.
#[derive(Debug, Clone, Serialize)]
pub struct RootCauseExemplar {
    pub file: String,
    pub test: String,
    pub step: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<FailureCategory>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<FailureRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<FailureResponse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AffectedLocation {
    pub file: String,
    pub test: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockedStep {
    pub file: String,
    pub test: String,
    pub step: String,
}

/// Build the grouped report from a deserialized `failures.json`.
///
/// `source` is a display-only path shown in the JSON and human views
/// so the user can tell which archive was inspected.
pub fn build_report(doc: &FailuresDoc, source: impl Into<String>) -> FailuresReport {
    let source = source.into();
    let total_failures = doc.failures.len();

    // First pass: split cascade fallout out of the regular failures so
    // it never counts as a root cause. The cascade's `root_cause` (set
    // by NAZ-401) points at the step we want to blame; fall back to
    // the cascade's own coordinates only if nothing upstream was
    // identified.
    let mut primaries: Vec<&FailureEntry> = Vec::new();
    let mut cascades: Vec<&FailureEntry> = Vec::new();
    for entry in &doc.failures {
        if is_cascade_category(entry.failure_category) {
            cascades.push(entry);
        } else {
            primaries.push(entry);
        }
    }

    // Build groups from primaries, keyed by fingerprint. Preserve
    // first-seen ordering so output is stable and the exemplar is the
    // first chronological failure in the group.
    let mut order: Vec<String> = Vec::new();
    let mut by_fp: BTreeMap<String, GroupBuilder> = BTreeMap::new();
    for entry in &primaries {
        let fp = fingerprint_for(entry);
        let builder = by_fp.entry(fp.clone()).or_insert_with(|| {
            order.push(fp.clone());
            GroupBuilder::new(fp.clone(), entry)
        });
        builder.record_occurrence(entry);
    }

    // Second pass: attach cascade skips to the group that caused them.
    // Match by the `root_cause` pointer first (precise, set by the
    // runner/summary builder). If that fails, fall back to a coordinate
    // match against any existing group exemplar in the same file+test.
    // Anything still unmatched spawns an "unattributed_cascade" group
    // so the user still sees the skip rather than silently dropping it.
    let mut unattributed: Vec<&FailureEntry> = Vec::new();
    for cascade in &cascades {
        let mut matched_fp: Option<String> = None;
        if let Some(rc) = cascade.root_cause.as_ref() {
            matched_fp = find_fp_by_coords(&by_fp, &rc.file, &rc.test, &rc.step);
        }
        if matched_fp.is_none() {
            matched_fp = find_fp_in_same_test(&by_fp, &cascade.file, &cascade.test);
        }
        match matched_fp.and_then(|fp| by_fp.get_mut(&fp)) {
            Some(builder) => builder.record_blocked(cascade),
            None => unattributed.push(cascade),
        }
    }
    if !unattributed.is_empty() {
        let fp = "unattributed_cascade".to_string();
        let exemplar = unattributed[0];
        let builder = by_fp.entry(fp.clone()).or_insert_with(|| {
            order.push(fp.clone());
            GroupBuilder::new(fp.clone(), exemplar)
        });
        for entry in &unattributed {
            builder.record_blocked(entry);
        }
    }

    let groups: Vec<FailureGroup> = order
        .into_iter()
        .filter_map(|fp| by_fp.remove(&fp).map(GroupBuilder::finish))
        .collect();

    FailuresReport {
        schema_version: FAILURES_REPORT_SCHEMA_VERSION,
        run_id: doc.run_id.clone(),
        source,
        total_failures,
        total_cascades: cascades.len(),
        groups,
    }
}

/// Render the report as human-readable text. Set `include_cascades` to
/// expand the list of blocked steps; by default we summarize them with
/// `└─ cascades: N skipped`.
pub fn render_human(report: &FailuresReport, include_cascades: bool, no_color: bool) -> String {
    let mut out = String::new();
    if report.total_failures == 0 {
        out.push_str("tarn: no failures\n");
        out.push_str(&format!("source: {}\n", report.source));
        return out;
    }
    let bullet = if no_color {
        "●"
    } else {
        "\x1b[31m●\x1b[0m"
    };
    let dim_start = if no_color { "" } else { "\x1b[2m" };
    let dim_end = if no_color { "" } else { "\x1b[0m" };

    out.push_str(&format!(
        "tarn: {} distinct problem{} across {} failure{} ({} cascaded)\n",
        report.groups.len(),
        if report.groups.len() == 1 { "" } else { "s" },
        report.total_failures,
        if report.total_failures == 1 { "" } else { "s" },
        report.total_cascades,
    ));
    out.push_str(&format!(
        "{}source: {}{}\n",
        dim_start, report.source, dim_end
    ));
    out.push('\n');

    for group in &report.groups {
        out.push_str(&format!(
            "{} {}  (×{})\n",
            bullet, group.fingerprint, group.occurrences
        ));
        let exemplar = &group.root_cause;
        out.push_str(&format!(
            "  {} :: {} :: {}\n",
            exemplar.file, exemplar.test, exemplar.step
        ));
        let first_line = exemplar
            .message
            .lines()
            .next()
            .unwrap_or(exemplar.message.as_str());
        out.push_str(&format!("  {}{}{}\n", dim_start, first_line, dim_end));
        if let (Some(req), Some(resp)) = (&exemplar.request, &exemplar.response) {
            if let Some(status) = resp.status {
                out.push_str(&format!(
                    "  {}{} {} → {}{}\n",
                    dim_start, req.method, req.url, status, dim_end
                ));
            }
        }
        if group.affected.len() > 1 {
            out.push_str("  affected:\n");
            for loc in &group.affected {
                out.push_str(&format!("    - {} :: {}\n", loc.file, loc.test));
            }
        }
        if !group.blocked_steps.is_empty() {
            if include_cascades {
                out.push_str(&format!("  └─ cascades ({}):\n", group.blocked_steps.len()));
                for blocked in &group.blocked_steps {
                    out.push_str(&format!(
                        "     - {} :: {} :: {}\n",
                        blocked.file, blocked.test, blocked.step
                    ));
                }
            } else {
                out.push_str(&format!(
                    "  └─ cascades: {} skipped\n",
                    group.blocked_steps.len()
                ));
            }
        }
        out.push('\n');
    }
    out
}

/// Render the report as pretty-printed JSON.
pub fn render_json(report: &FailuresReport) -> String {
    // Stable output: sort_keys-style ordering is already given by the
    // structs. `to_string_pretty` keeps field order by design.
    serde_json::to_string_pretty(report).expect("FailuresReport is always serializable")
}

// --- Internal helpers ----------------------------------------------------

fn is_cascade_category(category: Option<FailureCategory>) -> bool {
    matches!(
        category,
        Some(FailureCategory::SkippedDueToFailedCapture)
            | Some(FailureCategory::SkippedDueToFailFast)
    )
}

struct GroupBuilder {
    fingerprint: String,
    root_cause: RootCauseExemplar,
    affected: Vec<AffectedLocation>,
    seen_affected: HashSet<(String, String)>,
    occurrences: usize,
    blocked_steps: Vec<BlockedStep>,
    seen_blocked: HashSet<(String, String, String)>,
    // Coordinates of the exemplar (used for cascade fallback matching).
    root_coords: (String, String, String),
}

impl GroupBuilder {
    fn new(fingerprint: String, exemplar: &FailureEntry) -> Self {
        let root_coords = (
            exemplar.file.clone(),
            exemplar.test.clone(),
            exemplar.step.clone(),
        );
        Self {
            fingerprint,
            root_cause: RootCauseExemplar {
                file: exemplar.file.clone(),
                test: exemplar.test.clone(),
                step: exemplar.step.clone(),
                category: exemplar.failure_category,
                message: exemplar.message.clone(),
                request: exemplar.request.clone(),
                response: exemplar.response.clone(),
            },
            affected: Vec::new(),
            seen_affected: HashSet::new(),
            occurrences: 0,
            blocked_steps: Vec::new(),
            seen_blocked: HashSet::new(),
            root_coords,
        }
    }

    fn record_occurrence(&mut self, entry: &FailureEntry) {
        self.occurrences += 1;
        let key = (entry.file.clone(), entry.test.clone());
        if self.seen_affected.insert(key.clone()) {
            self.affected.push(AffectedLocation {
                file: key.0,
                test: key.1,
            });
        }
    }

    fn record_blocked(&mut self, entry: &FailureEntry) {
        let key = (entry.file.clone(), entry.test.clone(), entry.step.clone());
        if self.seen_blocked.insert(key.clone()) {
            self.blocked_steps.push(BlockedStep {
                file: key.0,
                test: key.1,
                step: key.2,
            });
        }
    }

    fn finish(self) -> FailureGroup {
        FailureGroup {
            fingerprint: self.fingerprint,
            occurrences: self.occurrences,
            root_cause: self.root_cause,
            affected: self.affected,
            blocked_steps: self.blocked_steps,
        }
    }
}

fn find_fp_by_coords(
    by_fp: &BTreeMap<String, GroupBuilder>,
    file: &str,
    test: &str,
    step: &str,
) -> Option<String> {
    by_fp
        .iter()
        .find(|(_, b)| {
            b.root_coords.0 == file && b.root_coords.1 == test && b.root_coords.2 == step
        })
        .map(|(fp, _)| fp.clone())
}

fn find_fp_in_same_test(
    by_fp: &BTreeMap<String, GroupBuilder>,
    file: &str,
    test: &str,
) -> Option<String> {
    by_fp
        .iter()
        .find(|(_, b)| b.root_coords.0 == file && b.root_coords.1 == test)
        .map(|(fp, _)| fp.clone())
}

/// Derive a stable fingerprint for a primary (non-cascade) failure.
/// Cascade categories (`SkippedDueToFailed*`) are filtered out by the
/// caller — they are consequences, not root causes, so they never get
/// fingerprinted.
pub fn fingerprint_for(entry: &FailureEntry) -> String {
    let category = match entry.failure_category {
        Some(c) => c,
        None => return unclassified_fingerprint(entry),
    };

    match category {
        FailureCategory::AssertionFailed => fingerprint_assertion_failed(entry),
        FailureCategory::ResponseShapeMismatch => fingerprint_shape_drift(entry),
        FailureCategory::ConnectionError => fingerprint_connection_error(entry),
        FailureCategory::Timeout => {
            let host = request_host(entry).unwrap_or_else(|| "unknown".into());
            format!("network:{}:timeout", host)
        }
        FailureCategory::UnresolvedTemplate => {
            let var =
                extract_unresolved_variable(&entry.message).unwrap_or_else(|| "?".to_string());
            format!("unresolved_template:{}", var)
        }
        FailureCategory::CaptureError => {
            let target = extract_capture_name(&entry.message).unwrap_or_else(|| "?".to_string());
            format!("capture_error:{}", target)
        }
        FailureCategory::ParseError => "parse_error".to_string(),
        FailureCategory::SkippedDueToFailedCapture
        | FailureCategory::SkippedDueToFailFast
        | FailureCategory::SkippedByCondition
        | FailureCategory::SkippedByPolicy => unclassified_fingerprint(entry),
        FailureCategory::CommandFailed => "command_failed".to_string(),
    }
}

/// Drift fingerprint: `shape_drift:<expected_path>:<observed_top_keys_hash>`.
/// Keying on the structured hint (not the message) keeps two different
/// files that hit the same drift in the same group even if the runner's
/// message text evolves. When the hint is missing (older runs), fall back
/// to the generic assertion-failed fingerprint so the entry still lands
/// in a sensible bucket instead of `unclassified`.
fn fingerprint_shape_drift(entry: &FailureEntry) -> String {
    let Some(hint) = entry.response_shape_mismatch.as_ref() else {
        return fingerprint_assertion_failed(entry);
    };
    let mut keys = hint.observed_keys.clone();
    keys.sort();
    let joined = keys.join(",");
    format!(
        "shape_drift:{}:{:x}",
        hint.expected_path,
        truncated_hash(&joined)
    )
}

fn fingerprint_assertion_failed(entry: &FailureEntry) -> String {
    let msg = entry.message.as_str();

    // Status assertion: the runner's message starts with "Expected HTTP
    // status <expected>". The actual status comes from response.status
    // so cross-run consistency survives message tweaks.
    if let Some(expected) = extract_expected_status(msg) {
        let actual = entry
            .response
            .as_ref()
            .and_then(|r| r.status)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "?".to_string());
        let (method, path) = request_method_and_path(entry);
        return format!("status:{}:{}:{}:{}", expected, actual, method, path);
    }

    // Body JSONPath: the assertion label surfaces via the message in
    // the form `JSONPath <path> did not match any value` (missing) or
    // the generic "body <path>" equality failure.
    if let Some(path) = extract_jsonpath_missing(msg) {
        return format!("body_jsonpath:{}:missing", path);
    }
    if let Some(path) = extract_jsonpath_equality(msg) {
        return format!("body_jsonpath:{}:value_mismatch", path);
    }

    // Header / duration / schema assertion miss — fall through to a
    // coarse bucket keyed by a category label so at least the category
    // groups together. Future iterations can refine this using the
    // embedded assertion results once the artifact carries them.
    if msg.to_ascii_lowercase().contains("header") {
        let key = extract_header_key(msg).unwrap_or_else(|| "?".into());
        return format!("header:{}:mismatch", key);
    }
    if msg.to_ascii_lowercase().contains("duration") {
        return "duration:mismatch".to_string();
    }
    if msg.to_ascii_lowercase().contains("schema") {
        return "schema:mismatch".to_string();
    }

    unclassified_fingerprint(entry)
}

fn fingerprint_connection_error(entry: &FailureEntry) -> String {
    let host = request_host(entry).unwrap_or_else(|| "unknown".into());
    let lower = entry.message.to_ascii_lowercase();
    let kind = if lower.contains("connection refused") {
        "refused"
    } else if lower.contains("dns")
        || lower.contains("failed to lookup")
        || lower.contains("no such host")
        || lower.contains("name or service not known")
    {
        "dns"
    } else if lower.contains("tls") {
        "tls"
    } else if lower.contains("redirect") {
        "redirect"
    } else {
        "network"
    };
    format!("network:{}:{}", host, kind)
}

fn unclassified_fingerprint(entry: &FailureEntry) -> String {
    let cat = entry
        .failure_category
        .map(category_label)
        .unwrap_or("unknown");
    format!("unclassified:{}:{:x}", cat, truncated_hash(&entry.message))
}

fn category_label(cat: FailureCategory) -> &'static str {
    match cat {
        FailureCategory::AssertionFailed => "assertion_failed",
        FailureCategory::ResponseShapeMismatch => "response_shape_mismatch",
        FailureCategory::ConnectionError => "connection_error",
        FailureCategory::Timeout => "timeout",
        FailureCategory::ParseError => "parse_error",
        FailureCategory::CaptureError => "capture_error",
        FailureCategory::UnresolvedTemplate => "unresolved_template",
        FailureCategory::SkippedDueToFailedCapture => "skipped_due_to_failed_capture",
        FailureCategory::SkippedDueToFailFast => "skipped_due_to_fail_fast",
        FailureCategory::SkippedByCondition => "skipped_by_condition",
        FailureCategory::SkippedByPolicy => "skipped_by_policy",
        FailureCategory::CommandFailed => "command_failed",
    }
}

/// Stable, low-collision hash of a message for the unclassified
/// bucket. Uses FNV-1a so it doesn't pull in a crypto dependency; we
/// only need "same message → same bucket".
fn truncated_hash(input: &str) -> u32 {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;
    let mut hash = FNV_OFFSET;
    for byte in input.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn request_method_and_path(entry: &FailureEntry) -> (String, String) {
    let (method, url) = match entry.request.as_ref() {
        Some(r) => (r.method.as_str(), r.url.as_str()),
        None => ("?", "?"),
    };
    let path = normalize_url_path(url);
    (method.to_string(), path)
}

fn request_host(entry: &FailureEntry) -> Option<String> {
    let url = entry.request.as_ref()?.url.as_str();
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let host = without_scheme.split('/').next().unwrap_or("");
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Strip scheme/host/query from a URL and replace UUID-shaped segments
/// with `:id` so two requests to different resources in the same
/// collection collapse into the same fingerprint. Falls back to the
/// raw string when the URL shape is unexpected.
fn normalize_url_path(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let after_host = match without_scheme.find('/') {
        Some(idx) => &without_scheme[idx..],
        None => "/",
    };
    let path_only = after_host.split('?').next().unwrap_or(after_host);
    let trimmed = path_only.trim_end_matches('/');
    let mut pieces: Vec<String> = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            continue;
        }
        if looks_like_uuid_segment(segment) {
            pieces.push(":id".to_string());
        } else {
            pieces.push(segment.to_string());
        }
    }
    if pieces.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", pieces.join("/"))
    }
}

fn looks_like_uuid_segment(segment: &str) -> bool {
    // Either a UUID (8-4-4-4-12 hex, 36 chars) or an "all digits"
    // segment. Guard the digit case with a length floor so short ids
    // like `/v1` don't collapse to `:id`.
    if segment.len() == 36 {
        let hex_digits = segment
            .chars()
            .filter(|c| c.is_ascii_hexdigit() || *c == '-')
            .count();
        if hex_digits == 36 {
            let dashes: Vec<usize> = segment.match_indices('-').map(|(idx, _)| idx).collect();
            if dashes == vec![8, 13, 18, 23] {
                return true;
            }
        }
    }
    if segment.len() >= 4 && segment.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

fn extract_expected_status(msg: &str) -> Option<String> {
    // Matches "Expected HTTP status 200, got 500" and the range/in-set
    // variants that start with "Expected HTTP status" too.
    let rest = msg.strip_prefix("Expected HTTP status ")?;
    let end = rest.find(", got").unwrap_or(rest.len());
    let trimmed = rest[..end].trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_jsonpath_missing(msg: &str) -> Option<String> {
    let rest = msg.strip_prefix("JSONPath ")?;
    let (path, _) = rest.split_once(" did not match any value")?;
    Some(path.trim().to_string())
}

fn extract_jsonpath_equality(msg: &str) -> Option<String> {
    // The body assertion's equality failure message is produced by
    // `equality_failure` in `assert::body`; it starts with
    // `JSONPath <path>` but does *not* contain "did not match any
    // value". We want the path.
    if !msg.starts_with("JSONPath ") {
        return None;
    }
    if msg.contains("did not match any value") {
        return None;
    }
    let rest = &msg["JSONPath ".len()..];
    let end = rest
        .find(|c: char| c == ':' || c.is_whitespace())
        .unwrap_or(rest.len());
    let path = rest[..end].trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

fn extract_header_key(msg: &str) -> Option<String> {
    // Typical header messages embed the header name in single quotes.
    let start = msg.find('\'')?;
    let rest = &msg[start + 1..];
    let end = rest.find('\'')?;
    let key = &rest[..end];
    if key.is_empty() {
        None
    } else {
        Some(key.to_ascii_lowercase())
    }
}

fn extract_unresolved_variable(msg: &str) -> Option<String> {
    let prefix = "Unresolved template variables: ";
    let rest = msg.strip_prefix(prefix)?;
    let first = rest.split(',').next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

fn extract_capture_name(msg: &str) -> Option<String> {
    // Capture extraction errors look like
    //   "Capture '<name>' failed: ..."
    let rest = msg.strip_prefix("Capture '")?;
    let end = rest.find('\'')?;
    let name = &rest[..end];
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::summary::{
        FailureEntry, FailureRequest, FailureResponse, FailuresDoc, RootCauseRef,
        SUMMARY_SCHEMA_VERSION,
    };

    fn entry(
        file: &str,
        test: &str,
        step: &str,
        cat: Option<FailureCategory>,
        msg: &str,
    ) -> FailureEntry {
        FailureEntry {
            file: file.into(),
            test: test.into(),
            step: step.into(),
            failure_category: cat,
            message: msg.into(),
            request: None,
            response: None,
            root_cause: None,
            response_shape_mismatch: None,
        }
    }

    fn with_request(mut e: FailureEntry, method: &str, url: &str) -> FailureEntry {
        e.request = Some(FailureRequest {
            method: method.into(),
            url: url.into(),
        });
        e
    }

    fn with_status(mut e: FailureEntry, status: u16) -> FailureEntry {
        e.response = Some(FailureResponse {
            status: Some(status),
            body_excerpt: None,
        });
        e
    }

    #[test]
    fn status_mismatch_fingerprint_includes_expected_actual_method_and_path() {
        let e = with_status(
            with_request(
                entry(
                    "a.tarn.yaml",
                    "t",
                    "s",
                    Some(FailureCategory::AssertionFailed),
                    "Expected HTTP status 200, got 500",
                ),
                "GET",
                "https://api.test/users?limit=10",
            ),
            500,
        );
        assert_eq!(fingerprint_for(&e), "status:200:500:GET:/users");
    }

    #[test]
    fn uuid_segments_normalize_to_id_so_sibling_resources_collapse() {
        let url = "https://api.test/users/11111111-2222-3333-4444-555555555555/posts";
        let e = with_status(
            with_request(
                entry(
                    "a.tarn.yaml",
                    "t",
                    "s",
                    Some(FailureCategory::AssertionFailed),
                    "Expected HTTP status 200, got 404",
                ),
                "GET",
                url,
            ),
            404,
        );
        assert_eq!(fingerprint_for(&e), "status:200:404:GET:/users/:id/posts");
    }

    #[test]
    fn numeric_id_segments_also_normalize() {
        let e = with_status(
            with_request(
                entry(
                    "a.tarn.yaml",
                    "t",
                    "s",
                    Some(FailureCategory::AssertionFailed),
                    "Expected HTTP status 200, got 404",
                ),
                "GET",
                "https://api.test/users/12345",
            ),
            404,
        );
        assert_eq!(fingerprint_for(&e), "status:200:404:GET:/users/:id");
    }

    #[test]
    fn body_jsonpath_missing_fingerprint() {
        let e = entry(
            "a.tarn.yaml",
            "t",
            "s",
            Some(FailureCategory::AssertionFailed),
            "JSONPath $.uuid did not match any value",
        );
        assert_eq!(fingerprint_for(&e), "body_jsonpath:$.uuid:missing");
    }

    #[test]
    fn body_jsonpath_value_mismatch_fingerprint() {
        let e = entry(
            "a.tarn.yaml",
            "t",
            "s",
            Some(FailureCategory::AssertionFailed),
            "JSONPath $.name: expected \"Alice\", got \"Bob\"",
        );
        assert_eq!(fingerprint_for(&e), "body_jsonpath:$.name:value_mismatch");
    }

    #[test]
    fn connection_refused_fingerprint_uses_host_and_refused_kind() {
        let e = with_request(
            entry(
                "a.tarn.yaml",
                "t",
                "s",
                Some(FailureCategory::ConnectionError),
                "Connection refused to http://127.0.0.1:9",
            ),
            "GET",
            "http://127.0.0.1:9/health",
        );
        assert_eq!(fingerprint_for(&e), "network:127.0.0.1:9:refused");
    }

    #[test]
    fn timeout_fingerprint_uses_host() {
        let e = with_request(
            entry(
                "a.tarn.yaml",
                "t",
                "s",
                Some(FailureCategory::Timeout),
                "Request timed out",
            ),
            "GET",
            "https://api.test/slow",
        );
        assert_eq!(fingerprint_for(&e), "network:api.test:timeout");
    }

    #[test]
    fn shape_drift_fingerprint_uses_structured_hint() {
        use crate::report::shape_diagnosis::{
            CandidateFix, ShapeConfidence, ShapeMismatchDiagnosis,
        };
        // NAZ-415: drift-classified entries fingerprint on the
        // structured hint so two files that hit the same drift land in
        // the same bucket even if the message text drifts across runs.
        let diag = ShapeMismatchDiagnosis {
            expected_path: "$.uuid".into(),
            observed_keys: vec!["request".into(), "stageStatus".into()],
            observed_type: "object".into(),
            candidate_fixes: vec![CandidateFix {
                path: "$.request.uuid".into(),
                confidence: ShapeConfidence::High,
                reason: "wrap".into(),
            }],
            high_confidence: true,
        };
        let mut e = entry(
            "a.tarn.yaml",
            "t",
            "s",
            Some(FailureCategory::ResponseShapeMismatch),
            "JSONPath $.uuid did not match any value",
        );
        e.response_shape_mismatch = Some(diag.clone());
        let fp = fingerprint_for(&e);
        assert!(
            fp.starts_with("shape_drift:$.uuid:"),
            "unexpected fingerprint: {}",
            fp
        );

        // Same drift in a different file yields the same fingerprint.
        let mut e2 = entry(
            "b.tarn.yaml",
            "other_test",
            "other_step",
            Some(FailureCategory::ResponseShapeMismatch),
            "JSONPath $.uuid matched no values",
        );
        e2.response_shape_mismatch = Some(diag);
        assert_eq!(fingerprint_for(&e), fingerprint_for(&e2));
    }

    #[test]
    fn shape_drift_without_hint_falls_back_to_assertion_fingerprint() {
        // Defensive: if a legacy artifact is read back with the new
        // category but no structured hint (not possible for fresh
        // runs, but robust against historical schemas), we still put
        // the entry in a sensible bucket instead of `unclassified`.
        let e = entry(
            "a.tarn.yaml",
            "t",
            "s",
            Some(FailureCategory::ResponseShapeMismatch),
            "JSONPath $.uuid did not match any value",
        );
        assert_eq!(fingerprint_for(&e), "body_jsonpath:$.uuid:missing");
    }

    #[test]
    fn unknown_message_falls_back_to_unclassified_with_stable_hash() {
        let e = entry(
            "a.tarn.yaml",
            "t",
            "s",
            Some(FailureCategory::AssertionFailed),
            "something unparseable happened",
        );
        let first = fingerprint_for(&e);
        let second = fingerprint_for(&e);
        assert_eq!(first, second);
        assert!(first.starts_with("unclassified:assertion_failed:"));
    }

    fn doc(failures: Vec<FailureEntry>) -> FailuresDoc {
        FailuresDoc {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: Some("rid".into()),
            failures,
        }
    }

    #[test]
    fn same_fingerprint_across_files_collapses_into_single_group() {
        let a = entry(
            "a.tarn.yaml",
            "t",
            "check",
            Some(FailureCategory::AssertionFailed),
            "JSONPath $.uuid did not match any value",
        );
        let b = entry(
            "b.tarn.yaml",
            "t",
            "check",
            Some(FailureCategory::AssertionFailed),
            "JSONPath $.uuid did not match any value",
        );
        let report = build_report(&doc(vec![a, b]), "test");
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.fingerprint, "body_jsonpath:$.uuid:missing");
        assert_eq!(group.occurrences, 2);
        let files: Vec<&str> = group.affected.iter().map(|a| a.file.as_str()).collect();
        assert_eq!(files, vec!["a.tarn.yaml", "b.tarn.yaml"]);
    }

    #[test]
    fn cascade_skips_are_not_occurrences_and_are_listed_as_blocked_steps() {
        let root = entry(
            "a.tarn.yaml",
            "t",
            "create_user",
            Some(FailureCategory::AssertionFailed),
            "Expected HTTP status 201, got 500",
        );
        let root = with_status(with_request(root, "POST", "https://api.test/users"), 500);
        let cascade = FailureEntry {
            file: "a.tarn.yaml".into(),
            test: "t".into(),
            step: "delete_user".into(),
            failure_category: Some(FailureCategory::SkippedDueToFailedCapture),
            message: "Skipped: capture user_id missing".into(),
            request: None,
            response: None,
            root_cause: Some(RootCauseRef {
                file: "a.tarn.yaml".into(),
                test: "t".into(),
                step: "create_user".into(),
            }),
            response_shape_mismatch: None,
        };
        let report = build_report(&doc(vec![root, cascade]), "test");
        assert_eq!(report.total_cascades, 1);
        assert_eq!(report.groups.len(), 1);
        let group = &report.groups[0];
        assert_eq!(group.occurrences, 1);
        assert_eq!(group.blocked_steps.len(), 1);
        assert_eq!(group.blocked_steps[0].step, "delete_user");
    }

    #[test]
    fn cascade_without_root_pointer_matches_by_same_test_coordinates() {
        let root = entry(
            "a.tarn.yaml",
            "t",
            "create_user",
            Some(FailureCategory::AssertionFailed),
            "Expected HTTP status 201, got 500",
        );
        let root = with_status(with_request(root, "POST", "https://api.test/users"), 500);
        let cascade = entry(
            "a.tarn.yaml",
            "t",
            "followup",
            Some(FailureCategory::SkippedDueToFailFast),
            "Skipped by fail_fast",
        );
        let report = build_report(&doc(vec![root, cascade]), "test");
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].blocked_steps.len(), 1);
    }

    #[test]
    fn unclassified_cascade_without_any_primary_still_surfaces() {
        let cascade = entry(
            "a.tarn.yaml",
            "t",
            "followup",
            Some(FailureCategory::SkippedDueToFailFast),
            "Skipped by fail_fast",
        );
        let report = build_report(&doc(vec![cascade]), "test");
        assert_eq!(report.total_cascades, 1);
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].fingerprint, "unattributed_cascade");
        assert_eq!(report.groups[0].blocked_steps.len(), 1);
    }

    #[test]
    fn empty_failures_yields_zero_groups_and_zero_counts() {
        let report = build_report(&doc(vec![]), "test");
        assert_eq!(report.total_failures, 0);
        assert_eq!(report.total_cascades, 0);
        assert!(report.groups.is_empty());
    }

    #[test]
    fn render_human_prints_zero_failures_message_when_empty() {
        let report = build_report(&doc(vec![]), ".tarn/failures.json");
        let text = render_human(&report, false, true);
        assert!(text.contains("no failures"));
        assert!(text.contains(".tarn/failures.json"));
    }

    #[test]
    fn render_human_summarizes_cascades_as_suffix_by_default() {
        let root = with_status(
            with_request(
                entry(
                    "a.tarn.yaml",
                    "t",
                    "create_user",
                    Some(FailureCategory::AssertionFailed),
                    "Expected HTTP status 201, got 500",
                ),
                "POST",
                "https://api.test/users",
            ),
            500,
        );
        let cascade = FailureEntry {
            file: "a.tarn.yaml".into(),
            test: "t".into(),
            step: "delete_user".into(),
            failure_category: Some(FailureCategory::SkippedDueToFailedCapture),
            message: "Skipped".into(),
            request: None,
            response: None,
            root_cause: Some(RootCauseRef {
                file: "a.tarn.yaml".into(),
                test: "t".into(),
                step: "create_user".into(),
            }),
            response_shape_mismatch: None,
        };
        let report = build_report(&doc(vec![root, cascade]), "test");
        let summary = render_human(&report, false, true);
        assert!(summary.contains("cascades: 1 skipped"));
        assert!(!summary.contains("delete_user"));
        let expanded = render_human(&report, true, true);
        assert!(expanded.contains("delete_user"));
    }

    #[test]
    fn render_json_envelope_is_stable() {
        let e = with_status(
            with_request(
                entry(
                    "a.tarn.yaml",
                    "t",
                    "s",
                    Some(FailureCategory::AssertionFailed),
                    "Expected HTTP status 200, got 500",
                ),
                "GET",
                "https://api.test/users",
            ),
            500,
        );
        let report = build_report(&doc(vec![e]), "test.json");
        let v: serde_json::Value = serde_json::from_str(&render_json(&report)).unwrap();
        assert_eq!(v["schema_version"], FAILURES_REPORT_SCHEMA_VERSION);
        assert_eq!(v["run_id"], "rid");
        assert_eq!(v["source"], "test.json");
        assert_eq!(v["total_failures"], 1);
        assert_eq!(v["total_cascades"], 0);
        let group = &v["groups"][0];
        assert_eq!(group["fingerprint"], "status:200:500:GET:/users");
        assert_eq!(group["occurrences"], 1);
        assert_eq!(group["root_cause"]["step"], "s");
        assert_eq!(group["root_cause"]["response"]["status"], 500);
    }
}
