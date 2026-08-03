//! Unified token estimation.
//!
//! Single canonical estimator used across CLI, ACP request hardening,
//! token cache sizing, and session compression. The CJK/ASCII-weighted
//! heuristic is significantly more accurate than the naive `chars/4`
//! approach for mixed-language prompts:
//!
//! - CJK characters (East Asian) count as ~1.5 tokens each
//! - All other characters count as ~0.25 tokens each (4 chars/token)
//!
//! All other estimators delegate here so the whole binary agrees on a
//! single token estimate for the same text.

/// Estimate the number of tokens in `text`.
///
/// Returns `0` for empty/whitespace-only input. CJK characters are weighted
/// ~1.5 tokens each; remaining characters ~0.25 tokens each.
pub fn estimate_tokens(text: &str) -> usize {
    if text.trim().is_empty() {
        return 0;
    }
    let mut cjk_chars = 0usize;
    let mut ascii_chars = 0usize;
    for ch in text.chars() {
        match ch {
            '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{2F800}'..='\u{2FA1F}' => cjk_chars += 1,
            _ => ascii_chars += 1,
        }
    }
    // CJK ~1.5 tokens/char, ASCII ~0.25 tokens/char (4 chars/token)
    (cjk_chars.saturating_mul(15) / 10) + (ascii_chars / 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("   "), 0);
    }

    #[test]
    fn ascii_is_four_chars_per_token() {
        // 8 ascii chars -> 2 tokens
        assert_eq!(estimate_tokens("hello wo"), 2);
        // 11 ascii chars -> 2 tokens
        assert_eq!(estimate_tokens("hello world"), 2);
    }

    #[test]
    fn cjk_is_weighted_higher() {
        // 10 CJK chars -> 15 tokens
        assert_eq!(estimate_tokens("一二三四五六七八九十"), 15);
    }

    #[test]
    fn mixed_language() {
        // 4 CJK (6 tokens) + 8 ascii (2 tokens) = 8
        assert_eq!(estimate_tokens("你好世界 hello wo"), 8);
    }
}
