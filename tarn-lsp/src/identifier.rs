//! Shared Tarn-identifier grammar helpers.
//!
//! NAZ-299 (rename) and NAZ-303 (code actions) both need the same
//! predicate — "is this string a valid env / capture / YAML identifier
//! that Tarn interpolation can reference by name" — so the helper was
//! promoted out of [`crate::rename`] into its own tiny module. Keeping
//! it here means the rename renderer, the extract-env code action, and
//! any future feature that coins new identifier names all agree on the
//! exact same grammar rule.
//!
//! ## Grammar
//!
//! `^[A-Za-z_][A-Za-z0-9_]*$` — ASCII only. Unicode letters are
//! rejected on purpose so the YAML key, the `{{ env.X }}` interpolation
//! token, and the `${VAR}` shell-expansion placeholder all agree on the
//! same identifier rule. A hand-rolled char walk rather than a regex
//! avoids pulling `regex` into `tarn-lsp` for one predicate.

/// True when `s` matches the Tarn identifier grammar
/// `^[A-Za-z_][A-Za-z0-9_]*$`.
pub fn is_valid_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ascii_letters_and_underscore() {
        assert!(is_valid_identifier("name"));
        assert!(is_valid_identifier("_name"));
        assert!(is_valid_identifier("name_2"));
        assert!(is_valid_identifier("CONST_CASE"));
        assert!(is_valid_identifier("x"));
        assert!(is_valid_identifier("new_env_key"));
        assert!(is_valid_identifier("new_env_key_2"));
    }

    #[test]
    fn rejects_leading_digit_or_hyphen() {
        assert!(!is_valid_identifier("2fast"));
        assert!(!is_valid_identifier("-name"));
        assert!(!is_valid_identifier("my-name"));
    }

    #[test]
    fn rejects_empty_whitespace_and_unicode() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier(" "));
        assert!(!is_valid_identifier("has space"));
        // Unicode letter — intentionally rejected. Tarn identifiers are
        // ASCII to match shell-expansion and YAML key conventions.
        assert!(!is_valid_identifier("café"));
    }

    #[test]
    fn rejects_punctuation_inside_body() {
        assert!(!is_valid_identifier("a.b"));
        assert!(!is_valid_identifier("a/b"));
        assert!(!is_valid_identifier("a$b"));
    }
}
