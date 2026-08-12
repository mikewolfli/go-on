//! Shared URL path joining helper.
//!
//! Single source for the "join a path onto a base URL without duplicating or
//! dropping a trailing slash" pattern, which modules previously re-implemented
//! inline (`format!("{}/{}", base.trim_end_matches('/'), path)`). The helper
//! guarantees exactly one `/` separator even when `base` already ends with one
//! and/or `path` begins with one.

/// Join `path` onto `base` with exactly one `/` separator.
///
/// - `join_url("https://api.example.com", "v1/messages")` → `"https://api.example.com/v1/messages"`
/// - `join_url("https://api.example.com/", "/v1/messages")` → `"https://api.example.com/v1/messages"`
///
/// Empty `base` or `path` yields the non-empty side unchanged.
pub fn join_url(base: &str, path: &str) -> String {
    let base_trimmed = base.trim_end_matches('/');
    let path_trimmed = path.trim_start_matches('/');
    if base_trimmed.is_empty() {
        return path_trimmed.to_string();
    }
    if path_trimmed.is_empty() {
        return base_trimmed.to_string();
    }
    format!("{base_trimmed}/{path_trimmed}")
}

#[cfg(test)]
mod tests {
    use super::join_url;

    #[test]
    fn joins_plain_base_and_path() {
        assert_eq!(
            join_url("https://api.example.com", "v1/messages"),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn collapses_redundant_slashes_on_both_sides() {
        assert_eq!(
            join_url("https://api.example.com/", "/v1/messages"),
            "https://api.example.com/v1/messages"
        );
    }

    #[test]
    fn empty_side_returns_the_other_unchanged() {
        assert_eq!(
            join_url("https://api.example.com", ""),
            "https://api.example.com"
        );
        assert_eq!(join_url("", "v1/messages"), "v1/messages");
    }
}
