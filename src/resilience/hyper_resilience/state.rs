//! Pure circuit-breaker state machine: the single transition authority shared
//! by every entry point of the hyper-resilience engine.

use super::types::{BreakerOutcome, CircuitBreaker, CircuitState, DegradationLevel};

/// Reset a breaker to its fresh Closed state (failure count 0, no last-failure
/// timestamp). Single reset implementation — manual reset paths
/// (recover_service, ClearCircuitBreaker, ReinitializeComponent) and the
/// half-open success transition all use this so the reset semantics cannot
/// drift.
pub(crate) fn reset_breaker(cb: &mut CircuitBreaker) {
    cb.state = CircuitState::Closed;
    cb.failure_count = 0;
    cb.last_failure_ms = 0;
}

/// Map open-breaker / failover counts to a system degradation level.
///
/// Single implementation shared by `system_health()` and `profile()` so the
/// thresholds (`> n/2` Emergency, `> n/3` Constrained) cannot drift between
/// the two views.
pub(crate) fn degradation_from_counts(
    open: usize,
    total: usize,
    failovers: usize,
) -> DegradationLevel {
    if open > 0 && open > total / 2 {
        DegradationLevel::Emergency
    } else if open > total / 3 && open > 0 {
        DegradationLevel::Constrained
    } else if open > 0 || failovers > 0 {
        DegradationLevel::Degraded
    } else {
        DegradationLevel::Normal
    }
}

/// Apply a single execution outcome to the circuit breaker state machine.
///
/// This is the **single** state-transition authority: the sync
/// `record_outcome` path, the async `record_failure_with_mode` /
/// `record_success` methods and `record_execution` all delegate here, so the
/// Closed/Open/HalfOpen rules (threshold open, success reset while closed,
/// half-open re-trip) cannot drift between entry points.
pub(crate) fn transition_breaker(cb: &mut CircuitBreaker, outcome: BreakerOutcome, now: u64) {
    match outcome {
        BreakerOutcome::Success => match cb.state {
            CircuitState::HalfOpen => {
                reset_breaker(cb);
                cb.last_state_change_ms = now;
            }
            CircuitState::Closed => {
                cb.failure_count = 0;
            }
            CircuitState::Open => {
                // No-op: an open breaker can't accept successes directly;
                // it must transition through half-open first.
            }
        },
        BreakerOutcome::Failure => match cb.state {
            CircuitState::Closed => {
                cb.failure_count += 1;
                cb.last_failure_ms = now;
                if cb.failure_count >= cb.threshold {
                    cb.state = CircuitState::Open;
                    cb.last_state_change_ms = now;
                }
            }
            CircuitState::Open => {
                // Already open; update last_failure so the timer resets.
                cb.last_failure_ms = now;
            }
            CircuitState::HalfOpen => {
                // Failure in half-open immediately trips back to open.
                cb.state = CircuitState::Open;
                cb.failure_count += 1;
                cb.last_failure_ms = now;
                cb.last_state_change_ms = now;
            }
        },
    }
}

/// Apply a success/failure to a circuit breaker without persistence
/// (sync convenience wrapper over [`transition_breaker`]).
pub(crate) fn apply_breaker_outcome(cb: &mut CircuitBreaker, success: bool) {
    transition_breaker(
        cb,
        if success {
            BreakerOutcome::Success
        } else {
            BreakerOutcome::Failure
        },
        crate::shared::timestamps::now_ts_ms_u64(),
    );
}
