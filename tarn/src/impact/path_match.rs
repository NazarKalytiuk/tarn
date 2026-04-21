//! File-path heuristics for mapping changed source files to test files.
//!
//! Three flavors:
//! 1. A changed file that *is* a `.tarn.yaml` maps directly to itself at `high`.
//! 2. A changed source file whose path has a shared non-trivial directory
//!    segment with a test file (e.g. `src/users/service.ts` vs
//!    `tests/users/crud.tarn.yaml`) maps at `medium` / `low` depending on how
//!    specific the shared segment is.
//! 3. No overlap → no match from this signal.
//!
//! The matcher is strictly path-lexical — it never reads files — so it is
//! safe to call from pure code and does not depend on a working tree layout.

use std::collections::BTreeSet;
use std::path::Path;

/// Non-informative directory basenames that should not be treated as shared
/// "topic" segments. Adding `test`/`tests`/`src` keeps `src/utils.ts` and
/// `tests/smoke.tarn.yaml` from matching purely because both live under a
/// `src` or `tests` ancestor.
const BORING_SEGMENTS: &[&str] = &[
    "src",
    "lib",
    "app",
    "pkg",
    "tests",
    "test",
    "spec",
    "specs",
    "integration",
    "e2e",
    "api",
    "apis",
    "services",
    "service",
    "handlers",
    "handler",
    "controllers",
    "controller",
    "routes",
    "route",
    "common",
];

/// Result of comparing a changed file path against a test file path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathMatch {
    /// The change *is* the test file.
    Direct,
    /// A shared topic segment (e.g. `users/`) links the two.
    SharedTopic { segment: String },
}

/// Compare a changed file path to a test file path.
///
/// - Direct: same file (exact string match or same canonical path).
/// - SharedTopic: they share a non-boring directory segment.
/// - None: unrelated.
pub fn match_paths(changed: &str, test_file: &str) -> Option<PathMatch> {
    if paths_equal(changed, test_file) {
        return Some(PathMatch::Direct);
    }

    let changed_segments = topic_segments(changed);
    let test_segments = topic_segments(test_file);

    for seg in &changed_segments {
        if test_segments.contains(seg) {
            return Some(PathMatch::SharedTopic {
                segment: seg.clone(),
            });
        }
    }
    None
}

fn paths_equal(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Best-effort normalization: strip a leading `./` and collapse repeated
    // separators. We do not canonicalize against the filesystem here; that
    // belongs in the caller where I/O is allowed.
    normalize_lexical(a) == normalize_lexical(b)
}

fn normalize_lexical(p: &str) -> String {
    let trimmed = p.trim_start_matches("./");
    let normalized: String =
        trimmed
            .chars()
            .fold(String::with_capacity(trimmed.len()), |mut acc, ch| {
                if ch == '/' && acc.ends_with('/') {
                    return acc;
                }
                acc.push(ch);
                acc
            });
    normalized
}

fn topic_segments(path: &str) -> BTreeSet<String> {
    let p = Path::new(path);
    let mut out = BTreeSet::new();
    for component in p.components() {
        let raw = component.as_os_str().to_string_lossy();
        let stem = raw.trim_end_matches(".tarn.yaml");
        // Also strip generic extensions so `users.ts` contributes `users`.
        let without_ext = match stem.rfind('.') {
            Some(i) => &stem[..i],
            None => stem,
        };
        let lower = without_ext.to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        if BORING_SEGMENTS.iter().any(|b| *b == lower) {
            continue;
        }
        out.insert(lower);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_match_on_identical_paths() {
        assert_eq!(
            match_paths("tests/users.tarn.yaml", "tests/users.tarn.yaml"),
            Some(PathMatch::Direct)
        );
    }

    #[test]
    fn direct_match_ignores_leading_dot_slash() {
        assert_eq!(
            match_paths("./tests/users.tarn.yaml", "tests/users.tarn.yaml"),
            Some(PathMatch::Direct)
        );
    }

    #[test]
    fn shared_topic_segment_links_source_and_test() {
        let m = match_paths("src/users/service.ts", "tests/users/crud.tarn.yaml").unwrap();
        assert_eq!(
            m,
            PathMatch::SharedTopic {
                segment: "users".into()
            }
        );
    }

    #[test]
    fn boring_segments_do_not_create_false_positives() {
        // Only `src` / `tests` overlap here — not enough to link them.
        assert_eq!(match_paths("src/utils.ts", "tests/smoke.tarn.yaml"), None);
    }

    #[test]
    fn filename_stem_counts_as_a_segment() {
        // `src/users.ts` stem is `users`, matches `tests/users/flow.tarn.yaml`.
        let m = match_paths("src/users.ts", "tests/users/flow.tarn.yaml").unwrap();
        assert_eq!(
            m,
            PathMatch::SharedTopic {
                segment: "users".into()
            }
        );
    }

    #[test]
    fn unrelated_paths_do_not_match() {
        assert_eq!(
            match_paths("src/orders/checkout.ts", "tests/users.tarn.yaml"),
            None
        );
    }
}
