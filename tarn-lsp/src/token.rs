//! Shared scanning primitives for interpolation-token detection.
//!
//! L1.3 (hover) and L1.4 (completion) both need to reason about the
//! cursor's relationship to `{{ … }}` interpolation syntax inside a Tarn
//! YAML buffer. Hover asks "am I *inside* a full `{{ … }}` pair?" and
//! needs the whole token span so it can highlight it. Completion asks
//! "what identifier am I in the middle of typing right now?" and needs
//! the line prefix between the most recent unclosed `{{` and the cursor.
//!
//! The two questions have different answers, but they share:
//!
//!   * UTF-8 ↔ LSP `Position` conversion.
//!   * Line-start / line-end byte offset helpers.
//!   * A byte-substring scan for `{{` / `}}`.
//!
//! L1.4 promotes all of the above out of `hover.rs` into this module so
//! both consumers share one well-tested implementation. Keeping the
//! helpers behind a narrow public surface also stops future tickets from
//! reaching into each other's internals.
//!
//! Nothing in here touches the filesystem, the parser, or `tarn::*`. It
//! is LSP-types-only, which is why every helper is trivially
//! unit-testable.

use lsp_types::Position;

// Re-export the token classifier + its token enum under the "interpolation"
// naming so downstream modules (definition, references) can depend on a
// neutral name instead of one that bakes in the original L1.3 feature.
// The implementation still lives in [`crate::hover`] to keep the NAZ-292
// history intact — renaming the module would be strictly cleanup and is
// out of scope for NAZ-297, which only needs a clean public alias.
pub use crate::hover::{
    resolve_hover_token as resolve_interpolation_token, HoverToken as InterpolationToken,
    HoverTokenSpan as InterpolationTokenSpan,
};

/// Convert a 0-based LSP [`Position`] into a byte offset into `source`.
///
/// LSP addresses each `character` as a UTF-16 code unit. Tarn YAML is
/// overwhelmingly ASCII, but the helper walks characters defensively —
/// a cursor past the end of the line folds to the line's end rather
/// than overflowing the slice. Returns `None` only when the line index
/// is impossibly far past the document (beyond every existing
/// newline).
pub fn position_to_byte_offset(source: &str, position: Position) -> Option<usize> {
    let line_start = position_to_line_start(source, position.line as usize)?;
    let line_end = find_line_end(source, line_start);
    let line = &source[line_start..line_end];
    let char_count_limit = position.character as usize;
    let offset_in_line: usize = line
        .chars()
        .take(char_count_limit)
        .map(char::len_utf8)
        .sum();
    Some(line_start + offset_in_line.min(line.len()))
}

/// Convert a byte offset back into a [`Position`]. Used by classifiers
/// that scan raw bytes and need to report the answer in LSP coordinates.
pub fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;
    let clamped = offset.min(source.len());
    for (i, ch) in source.char_indices() {
        if i >= clamped {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

/// Byte offset of the start of a 0-based line in `source`.
///
/// Returns `Some(source.len())` for lines past the document end so
/// callers get a well-defined empty slice instead of a `None` they have
/// to branch on.
pub fn position_to_line_start(source: &str, target_line: usize) -> Option<usize> {
    if target_line == 0 {
        return Some(0);
    }
    let mut newline_count = 0usize;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            newline_count += 1;
            if newline_count == target_line {
                return Some(i + 1);
            }
        }
    }
    Some(source.len())
}

/// Byte offset of the newline terminating the line that starts at
/// `line_start`. Returns `source.len()` when the line is the last one
/// and has no terminating `\n`.
pub fn find_line_end(source: &str, line_start: usize) -> usize {
    source[line_start..]
        .bytes()
        .position(|b| b == b'\n')
        .map(|rel| line_start + rel)
        .unwrap_or(source.len())
}

/// First occurrence of `needle` inside `haystack`, or `None`.
///
/// Used by the hover-token scanner to find `}}` after a `{{` without
/// pulling in `memchr` or paying for `str::find` overhead on every
/// call.
pub fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

/// Is `s` a bare identifier — `[A-Za-z_][A-Za-z0-9_-]*`?
///
/// Used by both the hover schema-key classifier and the completion
/// schema-key classifier to reject lines whose "key" is clearly not a
/// Tarn field (numbers, sub-scripts, quoted strings, etc).
pub fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Slice of the line that contains `position`, along with the byte
/// offset of that line's start. Returned as a tuple so callers that
/// want both the prefix slice (e.g. completion context detection) and
/// the absolute offset (e.g. diagnostics) share one pass.
///
/// The returned slice covers the full line — callers restrict it to
/// `line[..cursor_col_in_bytes]` when they only care about the prefix
/// up to the cursor.
pub fn line_at_position(source: &str, position: Position) -> Option<(usize, &str)> {
    let line_start = position_to_line_start(source, position.line as usize)?;
    let line_end = find_line_end(source, line_start);
    Some((line_start, &source[line_start..line_end]))
}

/// Byte offset within `line` of `position.character`, clamped to the
/// line length. Mirrors what `position_to_byte_offset` does but works
/// on a pre-computed line slice so completion can ask "how much of
/// this line is the prefix?" without walking the document twice.
pub fn column_to_line_byte_offset(line: &str, character: u32) -> usize {
    let take = character as usize;
    let bytes: usize = line.chars().take(take).map(char::len_utf8).sum();
    bytes.min(line.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- position <-> byte offset ----

    #[test]
    fn position_to_byte_offset_at_document_start_is_zero() {
        let src = "abc\n";
        assert_eq!(position_to_byte_offset(src, Position::new(0, 0)), Some(0));
    }

    #[test]
    fn position_to_byte_offset_mid_line() {
        let src = "abc\ndef\n";
        assert_eq!(position_to_byte_offset(src, Position::new(1, 2)), Some(6));
    }

    #[test]
    fn position_to_byte_offset_past_end_of_line_clamps_to_line_end() {
        let src = "abc\ndef\n";
        // line 0 is `abc`, length 3. Asking for column 10 should clamp.
        assert_eq!(position_to_byte_offset(src, Position::new(0, 10)), Some(3));
    }

    #[test]
    fn byte_offset_to_position_round_trips_ascii() {
        let src = "abc\ndef\n";
        let pos = byte_offset_to_position(src, 5);
        assert_eq!(pos, Position::new(1, 1));
    }

    // ---- line start/end ----

    #[test]
    fn position_to_line_start_first_line_is_zero() {
        assert_eq!(position_to_line_start("abc\ndef", 0), Some(0));
    }

    #[test]
    fn position_to_line_start_second_line() {
        assert_eq!(position_to_line_start("abc\ndef", 1), Some(4));
    }

    #[test]
    fn position_to_line_start_past_end_returns_source_len() {
        let src = "abc\n";
        assert_eq!(position_to_line_start(src, 50), Some(src.len()));
    }

    #[test]
    fn find_line_end_stops_at_newline() {
        let src = "abc\ndef\n";
        assert_eq!(find_line_end(src, 0), 3);
    }

    #[test]
    fn find_line_end_handles_missing_trailing_newline() {
        let src = "abc";
        assert_eq!(find_line_end(src, 0), 3);
    }

    // ---- scanning ----

    #[test]
    fn find_subslice_finds_match() {
        assert_eq!(find_subslice(b"hello world", b"world"), Some(6));
    }

    #[test]
    fn find_subslice_returns_none_when_missing() {
        assert_eq!(find_subslice(b"hello", b"z"), None);
    }

    #[test]
    fn find_subslice_empty_needle_returns_none() {
        assert_eq!(find_subslice(b"hello", b""), None);
    }

    // ---- identifier check ----

    #[test]
    fn is_identifier_accepts_alpha_underscore() {
        assert!(is_identifier("name"));
        assert!(is_identifier("_name"));
        assert!(is_identifier("name_2"));
        assert!(is_identifier("env-file"));
    }

    #[test]
    fn is_identifier_rejects_leading_digit() {
        assert!(!is_identifier("2fast"));
    }

    #[test]
    fn is_identifier_rejects_empty() {
        assert!(!is_identifier(""));
    }

    // ---- line_at_position + column_to_line_byte_offset ----

    #[test]
    fn line_at_position_returns_line_slice_and_start_offset() {
        let src = "abc\ndef\nghi";
        let (start, line) = line_at_position(src, Position::new(1, 0)).unwrap();
        assert_eq!(start, 4);
        assert_eq!(line, "def");
    }

    #[test]
    fn column_to_line_byte_offset_clamps_past_end() {
        assert_eq!(column_to_line_byte_offset("abc", 10), 3);
    }

    #[test]
    fn column_to_line_byte_offset_mid_line_is_char_byte_sum() {
        assert_eq!(column_to_line_byte_offset("abcdef", 3), 3);
    }
}
