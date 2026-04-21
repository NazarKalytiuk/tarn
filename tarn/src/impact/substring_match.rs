//! Low-confidence name-token substring matcher.
//!
//! Extracts meaningful tokens from a changed file path (basename without
//! extension plus parent directory segments) and flags a test when any
//! token appears as a substring in the test file name, test name, step
//! name, or step request URL. This is intentionally loose — output at this
//! tier is clamped to `low` confidence so it never drowns out a real
//! endpoint match.

/// Extract lowercase tokens useful for substring matching. Drops boring
/// infrastructure segments (`src`, `lib`, …) so they cannot spuriously
/// match every test that mentions `src/` in a comment.
pub fn tokens_for(changed_path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in changed_path.split('/') {
        let stripped = raw.trim_end_matches(".tarn.yaml");
        let without_ext = match stripped.rfind('.') {
            Some(i) => &stripped[..i],
            None => stripped,
        };
        let lower = without_ext.to_ascii_lowercase();
        if lower.is_empty() || lower.len() < 3 {
            continue;
        }
        if matches!(
            lower.as_str(),
            "src"
                | "lib"
                | "app"
                | "pkg"
                | "tests"
                | "test"
                | "spec"
                | "specs"
                | "node_modules"
                | "dist"
                | "build"
                | "target"
                | "tmp"
                | "common"
                | "util"
                | "utils"
                | "index"
                | "main"
        ) {
            continue;
        }
        // Split camelCase / snake_case into sub-tokens too; "NotifyService"
        // should flag `notify` targets.
        for sub in split_identifier(&lower) {
            if sub.len() >= 3 {
                out.push(sub);
            }
        }
        out.push(lower);
    }
    out.sort();
    out.dedup();
    out
}

fn split_identifier(s: &str) -> Vec<String> {
    // We already lowercased so camelCase is erased; split on `_`, `-`, and `.`
    // which survive lowercasing.
    s.split(['_', '-', '.'])
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

/// Return the first token from `tokens` that appears as a substring in
/// `haystack` (case-insensitive). Used to tag the match reason.
pub fn first_hit<'a>(tokens: &'a [String], haystack: &str) -> Option<&'a str> {
    let h = haystack.to_ascii_lowercase();
    tokens
        .iter()
        .find(|t| h.contains(t.as_str()))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_drops_boring_segments() {
        let t = tokens_for("src/services/notify.ts");
        assert!(t.contains(&"services".to_string()));
        assert!(t.contains(&"notify".to_string()));
        assert!(!t.contains(&"src".to_string()));
    }

    #[test]
    fn tokens_drops_too_short_fragments() {
        let t = tokens_for("src/a.ts");
        assert!(t.is_empty(), "expected empty, got {:?}", t);
    }

    #[test]
    fn first_hit_finds_token_substring() {
        let tokens = vec!["notifier".to_string(), "users".to_string()];
        assert_eq!(first_hit(&tokens, "POST /notifier/send"), Some("notifier"));
    }

    #[test]
    fn first_hit_returns_none_when_absent() {
        let tokens = vec!["notify".to_string()];
        assert_eq!(first_hit(&tokens, "GET /health"), None);
    }
}
