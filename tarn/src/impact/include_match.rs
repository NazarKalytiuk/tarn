//! Include / fixture reference matcher.
//!
//! Finds changed files that are referenced by a test file via
//! `include: <path>` step entries or `multipart.files[].path` uploads. The
//! matcher works on a raw snapshot of the test file's source YAML so it
//! does not depend on include resolution or interpolation semantics — a
//! simple grep for the changed file's path (or its basename) is enough to
//! catch the "this shared snippet / upload fixture changed" case.

/// Return true when `source` references `changed_path` in a way that
/// would plausibly make the test run differently.
///
/// We do a literal substring search for the whole path first, then for
/// the basename as a fallback. False positives are expected for very
/// short basenames; callers cap the resulting match at `medium` so the
/// signal cannot drown out real endpoint matches.
pub fn references_path(source: &str, changed_path: &str) -> Option<String> {
    let trimmed = changed_path.trim_start_matches("./");
    if trimmed.is_empty() {
        return None;
    }
    if source.contains(trimmed) {
        return Some(trimmed.to_string());
    }
    if let Some(base) = basename(trimmed) {
        // Only fall back to the bare basename when it's long enough to be
        // meaningful; otherwise "a.ts" would match every `a` in a YAML doc.
        if base.len() >= 5 && source.contains(base) {
            return Some(base.to_string());
        }
    }
    None
}

fn basename(p: &str) -> Option<&str> {
    p.rsplit('/').next().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_include_directive_reference() {
        let source = r#"
name: x
steps:
  - include: ./shared/auth.tarn.yaml
"#;
        assert_eq!(
            references_path(source, "shared/auth.tarn.yaml"),
            Some("shared/auth.tarn.yaml".into())
        );
    }

    #[test]
    fn detects_upload_path_reference() {
        let source = r#"
name: upload
steps:
  - name: put
    request:
      method: POST
      url: http://localhost/upload
      multipart:
        files:
          - name: photo
            path: ./fixtures/avatar.jpeg
"#;
        assert_eq!(
            references_path(source, "fixtures/avatar.jpeg"),
            Some("fixtures/avatar.jpeg".into())
        );
    }

    #[test]
    fn no_reference_returns_none() {
        let source = "name: none\nsteps: []\n";
        assert_eq!(references_path(source, "shared/auth.tarn.yaml"), None);
    }

    #[test]
    fn short_basename_alone_does_not_match() {
        // `a.ts` would falsely match many snippets; guard against it.
        let source = "name: a\nsteps: []\n";
        assert_eq!(references_path(source, "src/a.ts"), None);
    }

    #[test]
    fn long_basename_alone_can_match() {
        let source = "# references avatar.jpeg inline\nname: x\nsteps: []\n";
        assert_eq!(
            references_path(source, "fixtures/avatar.jpeg"),
            Some("avatar.jpeg".into())
        );
    }
}
