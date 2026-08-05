//! Shared exponential-backoff helper.
//!
//! All GUI backoff policies (backend health polling, crash-restart rate
//! limiting, channel-full retry, RPC retry base) use the same `base * 2^n`
//! growth formula with an optional cap. Keeping the formula in one pure
//! function removes duplicated literals and gives a single test point;
//! each caller still picks its own base/cap because the policies are
//! semantically independent (see `contracts/cross-client-sync.md` for the
//! RPC retry contract, which additionally applies jitter on top of the base).

/// Compute `base_ms * 2^exponent`, capped at `cap_ms`.
///
/// - `exponent = 0` yields `base_ms` (the first retry).
/// - An `u64::MAX` cap disables the cap (callers that bound the exponent
///   themselves, e.g. `n.min(5)`).
pub(crate) fn exp_backoff_ms(base_ms: u64, exponent: u32, cap_ms: u64) -> u64 {
    let grown = base_ms.saturating_mul(1u64 << exponent.min(63));
    grown.min(cap_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_is_exponential_from_base() {
        // 5ms, 10ms, 20ms, 40ms…
        assert_eq!(exp_backoff_ms(5, 0, 200), 5);
        assert_eq!(exp_backoff_ms(5, 1, 200), 10);
        assert_eq!(exp_backoff_ms(5, 2, 200), 20);
        assert_eq!(exp_backoff_ms(5, 3, 200), 40);
    }

    #[test]
    fn cap_is_enforced() {
        // 5 * 2^6 = 320 → capped at 200 (channel-full retry policy).
        assert_eq!(exp_backoff_ms(5, 6, 200), 200);
        assert_eq!(exp_backoff_ms(5, 40, 200), 200);
        // 1000 * 2^5 = 32000 → capped at 30000 (RPC retry policy).
        assert_eq!(exp_backoff_ms(1000, 5, 30_000), 30_000);
    }

    #[test]
    fn matches_health_poll_sequence() {
        // 1s, 2s, 4s, 8s, 16s (cap 60s) for consecutive poll failures 1..=5.
        let seq: Vec<u64> = (0..5).map(|e| exp_backoff_ms(1000, e, 60_000)).collect();
        assert_eq!(seq, vec![1000, 2000, 4000, 8000, 16_000]);
    }

    #[test]
    fn matches_crash_restart_sequence() {
        // 3s * 2^n with the exponent bounded by the caller at 5.
        assert_eq!(exp_backoff_ms(3000, 0, u64::MAX), 3000);
        assert_eq!(exp_backoff_ms(3000, 5, u64::MAX), 96_000);
    }

    #[test]
    fn overflow_is_saturated() {
        assert_eq!(exp_backoff_ms(u64::MAX, 1, u64::MAX), u64::MAX);
    }
}
