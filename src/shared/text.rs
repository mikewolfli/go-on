//! Shared ASCII case-insensitive text primitives.
//!
//! All matching is done byte-wise directly on the original string — no
//! lowercased copy is ever produced. This matters for two reasons:
//!
//! 1. **Efficiency**: no O(n) allocation for potentially large haystacks
//!    (conversation history, feed bodies, GPX track files).
//! 2. **Correctness**: `to_lowercase()` can change byte lengths (e.g. `K`
//!    U+212A 3→1, `İ` U+0130 2→3, `ẞ` U+1E9E 3→2), so offsets derived from a
//!    lowercased copy cannot be used to slice the original without risking a
//!    mid-code-point panic. Byte-wise comparison is exact for ASCII needles.

/// Find the first ASCII case-insensitive occurrence of `needle` in `text`,
/// returning its byte offset.
///
/// Byte slicing never requires char boundaries, so this is safe for any
/// needle; when `needle` starts with an ASCII byte (e.g. `<`), every match
/// position is additionally guaranteed to be a char boundary in valid UTF-8
/// (continuation bytes are 0x80..=0xBF and can never equal an ASCII byte).
pub fn find_ascii_case_insensitive(text: &str, needle: &str) -> Option<usize> {
    let hay = text.as_bytes();
    let needle_b = needle.as_bytes();
    if needle_b.is_empty() || needle_b.len() > hay.len() {
        return None;
    }
    // Inclusive end: an exclusive `0..len.saturating_sub(n)` range misses the
    // last valid start position, where the needle ends exactly at the end of
    // the text.
    (0..=hay.len() - needle_b.len())
        .find(|&i| hay[i..i + needle_b.len()].eq_ignore_ascii_case(needle_b))
}

/// Whether `needle` occurs in `text` (ASCII case-insensitive).
pub fn contains_ascii_case_insensitive(text: &str, needle: &str) -> bool {
    find_ascii_case_insensitive(text, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_matches_first_occurrence() {
        assert_eq!(find_ascii_case_insensitive("AbCd", "bc"), Some(1));
        assert_eq!(find_ascii_case_insensitive("abcdef", "x"), None);
        assert_eq!(find_ascii_case_insensitive("abc", "abcd"), None);
        assert_eq!(find_ascii_case_insensitive("abc", ""), None);
        // Needle ending exactly at the end of the text (inclusive range).
        assert_eq!(
            find_ascii_case_insensitive("<ele>1</ELE>", "</ele>"),
            Some(6)
        );
    }

    #[test]
    fn find_never_panics_on_multibyte_haystack() {
        // `K` U+212A is 3 bytes; a lowercased copy would shift offsets.
        let text = "<title>你K你好</title>";
        assert_eq!(find_ascii_case_insensitive(text, "<title>"), Some(0));
        let start = find_ascii_case_insensitive(text, "</title>").unwrap();
        assert_eq!(&text[start..], "</title>");
    }

    #[test]
    fn contains_works() {
        assert!(contains_ascii_case_insensitive("HELLO World", "hello"));
        assert!(!contains_ascii_case_insensitive("hello", "xyz"));
        assert!(contains_ascii_case_insensitive("fix the bug", "FIX"));
    }
}
