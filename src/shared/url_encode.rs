//! Shared URL query-string encoding helper.
//!
//! Single source for percent-encoding a string into a URL query parameter.
//! Modules previously re-implemented this inline via
//! `url::form_urlencoded::byte_serialize` (packages.rs, utils.rs) or with a
//! hand-rolled per-char encoder (game/online.rs) that truncated multi-byte
//! UTF-8 characters (`other as u8` → a single `%XX` byte instead of the
//! correct multi-byte `%XX%XX` sequence).

/// Percent-encode `s` for use in a URL query string.
///
/// Uses the `url` crate's form-urlencoded serialization: unreserved
/// characters pass through, spaces become `+`, and every other byte is
/// `%XX`-encoded. Encoding operates on UTF-8 **bytes**, so non-ASCII input
/// round-trips correctly instead of being truncated to a single byte.
pub fn form_url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::form_url_encode;

    #[test]
    fn replaces_spaces_with_plus() {
        let encoded = form_url_encode("hello world");
        // form-urlencoded uses "+" for spaces (also valid in query strings).
        assert!(encoded == "hello%20world" || encoded == "hello+world");
    }

    #[test]
    fn keeps_unreserved_chars_unchanged() {
        // The url crate's form_urlencoded set passes through alphanumerics
        // and `-._`; `~` is encoded as `%7E` (valid equivalent).
        assert_eq!(form_url_encode("abc-_.XYZ0129"), "abc-_.XYZ0129");
        assert_eq!(form_url_encode("a~b"), "a%7Eb");
    }

    #[test]
    fn percent_encodes_multi_byte_utf8_correctly() {
        // 'é' is U+00E9 → UTF-8 bytes 0xC3 0xA9 → %C3%A9. The removed
        // hand-rolled encoder emitted %E9 by truncating the char to its low
        // byte, producing a malformed URL.
        assert_eq!(form_url_encode("café"), "caf%C3%A9");
    }
}
