//! ACP Lock Monitor
//!
//! Lock monitoring infrastructure: tracks acquisitions, poisonings, and wait
//! times for all ACP `std::sync::Mutex` instances.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as TokioMutex;
use tracing::warn;

use crate::acp::prelude::constants::ACP_LOCK_SLOW_WAIT_THRESHOLD;
use crate::acp::prelude::types::AcpLockSnapshot;

// ============================================================================
// Internal lock counters
// ============================================================================

#[derive(Debug, Default)]
struct AcpLockCounters {
    acquisitions: AtomicU64,
    poisoned_total: AtomicU64,
    recovered_total: AtomicU64,
    slow_wait_total: AtomicU64,
    total_wait_nanos: AtomicU64,
    max_wait_nanos: AtomicU64,
}

impl AcpLockCounters {
    fn record_wait(&self, wait: Duration) {
        let wait_nanos = wait.as_nanos().min(u64::MAX as u128) as u64;
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.total_wait_nanos
            .fetch_add(wait_nanos, Ordering::Relaxed);
        if wait >= ACP_LOCK_SLOW_WAIT_THRESHOLD {
            self.slow_wait_total.fetch_add(1, Ordering::Relaxed);
        }

        let mut current = self.max_wait_nanos.load(Ordering::Relaxed);
        while wait_nanos > current {
            match self.max_wait_nanos.compare_exchange(
                current,
                wait_nanos,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    fn record_poison(&self) {
        self.poisoned_total.fetch_add(1, Ordering::Relaxed);
    }

    fn record_recovery(&self) {
        self.recovered_total.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self, name: &'static str) -> AcpLockSnapshot {
        let acquisitions = self.acquisitions.load(Ordering::Relaxed);
        let total_wait_nanos = self.total_wait_nanos.load(Ordering::Relaxed);
        let max_wait_nanos = self.max_wait_nanos.load(Ordering::Relaxed);
        let avg_wait_ms = if acquisitions > 0 {
            total_wait_nanos as f64 / acquisitions as f64 / 1_000_000.0
        } else {
            0.0
        };

        AcpLockSnapshot {
            name: name.to_string(),
            acquisitions,
            poisoned_total: self.poisoned_total.load(Ordering::Relaxed),
            recovered_total: self.recovered_total.load(Ordering::Relaxed),
            slow_wait_total: self.slow_wait_total.load(Ordering::Relaxed),
            avg_wait_ms,
            max_wait_ms: max_wait_nanos as f64 / 1_000_000.0,
        }
    }
}

// ============================================================================
// Lock monitor (public API)
// ============================================================================

/// Monitors lock acquisitions, poisonings, and wait times across all
/// named ACP `std::sync::Mutex` instances.
#[derive(Debug, Default)]
pub struct AcpLockMonitor {
    runtime_config: AcpLockCounters,
    memory_cache: AcpLockCounters,
    memory_store: AcpLockCounters,
    response_cache: AcpLockCounters,
    vector_store: AcpLockCounters,
    maintenance: AcpLockCounters,
    lifecycle: AcpLockCounters,
    circuit_breakers: AcpLockCounters,
    phase_rate_limiter: AcpLockCounters,
    inflight_limiter: AcpLockCounters,
}

impl AcpLockMonitor {
    fn counters(&self, name: &'static str) -> &AcpLockCounters {
        match name {
            crate::acp::prelude::constants::ACP_LOCK_RUNTIME_CONFIG => &self.runtime_config,
            crate::acp::prelude::constants::ACP_LOCK_MEMORY_CACHE => &self.memory_cache,
            crate::acp::prelude::constants::ACP_LOCK_MEMORY_STORE => &self.memory_store,
            crate::acp::prelude::constants::ACP_LOCK_RESPONSE_CACHE => &self.response_cache,
            crate::acp::prelude::constants::ACP_LOCK_VECTOR_STORE => &self.vector_store,
            crate::acp::prelude::constants::ACP_LOCK_MAINTENANCE => &self.maintenance,
            crate::acp::prelude::constants::ACP_LOCK_LIFECYCLE => &self.lifecycle,
            crate::acp::prelude::constants::ACP_LOCK_CIRCUIT_BREAKERS => &self.circuit_breakers,
            crate::acp::prelude::constants::ACP_LOCK_PHASE_RATE_LIMITER => &self.phase_rate_limiter,
            crate::acp::prelude::constants::ACP_LOCK_INFLIGHT_LIMITER => &self.inflight_limiter,
            _ => {
                warn!("Unknown ACP lock monitor component: {name}, using fallback mutex");
                static FALLBACK: AcpLockCounters = AcpLockCounters {
                    acquisitions: AtomicU64::new(0),
                    poisoned_total: AtomicU64::new(0),
                    recovered_total: AtomicU64::new(0),
                    slow_wait_total: AtomicU64::new(0),
                    total_wait_nanos: AtomicU64::new(0),
                    max_wait_nanos: AtomicU64::new(0),
                };
                &FALLBACK
            }
        }
    }

    /// Produce a snapshot for every named lock.
    pub fn snapshot(&self) -> Vec<AcpLockSnapshot> {
        use crate::acp::prelude::constants::{
            ACP_LOCK_CIRCUIT_BREAKERS, ACP_LOCK_INFLIGHT_LIMITER, ACP_LOCK_LIFECYCLE,
            ACP_LOCK_MAINTENANCE, ACP_LOCK_MEMORY_CACHE, ACP_LOCK_MEMORY_STORE,
            ACP_LOCK_PHASE_RATE_LIMITER, ACP_LOCK_RESPONSE_CACHE, ACP_LOCK_RUNTIME_CONFIG,
            ACP_LOCK_VECTOR_STORE,
        };
        [
            ACP_LOCK_RUNTIME_CONFIG,
            ACP_LOCK_MEMORY_CACHE,
            ACP_LOCK_MEMORY_STORE,
            ACP_LOCK_RESPONSE_CACHE,
            ACP_LOCK_VECTOR_STORE,
            ACP_LOCK_MAINTENANCE,
            ACP_LOCK_LIFECYCLE,
            ACP_LOCK_CIRCUIT_BREAKERS,
            ACP_LOCK_PHASE_RATE_LIMITER,
            ACP_LOCK_INFLIGHT_LIMITER,
        ]
        .into_iter()
        .map(|name| self.counters(name).snapshot(name))
        .collect()
    }

    fn record_wait(&self, name: &'static str, wait: Duration) {
        self.counters(name).record_wait(wait);
    }

    fn record_poison(&self, name: &'static str) {
        self.counters(name).record_poison();
    }

    fn record_recovery(&self, name: &'static str) {
        self.counters(name).record_recovery();
    }
}

// ============================================================================
// Lock helper functions
// ============================================================================

/// Acquire a `std::sync::Mutex` with lock monitoring.
///
/// Records wait time and handles poisoned mutexes gracefully,
/// recovering the state and continuing.
pub fn with_acp_lock<T, R, F>(
    monitor: &AcpLockMonitor,
    name: &'static str,
    mutex: &StdMutex<T>,
    operation: F,
) -> R
where
    F: FnOnce(&mut T) -> R,
{
    let wait_started = Instant::now();
    match mutex.lock() {
        Ok(mut guard) => {
            monitor.record_wait(name, wait_started.elapsed());
            operation(&mut guard)
        }
        Err(poisoned) => {
            monitor.record_wait(name, wait_started.elapsed());
            monitor.record_poison(name);
            monitor.record_recovery(name);
            warn!(
                target: "acp::locks",
                "ACP lock '{}' was poisoned; continuing with recovered state",
                name
            );
            let mut guard = poisoned.into_inner();
            operation(&mut guard)
        }
    }
}

/// Acquire a `tokio::sync::Mutex` with lock monitoring (async version).
///
/// Records wait time and handles poisoning (tokio mutexes do not poison, but
/// the interface is kept for consistency).
pub async fn with_acp_lock_async<T, R, F>(
    monitor: &AcpLockMonitor,
    name: &'static str,
    mutex: &TokioMutex<T>,
    operation: F,
) -> R
where
    F: FnOnce(&mut T) -> R,
{
    let wait_started = Instant::now();
    let mut guard = mutex.lock().await;
    monitor.record_wait(name, wait_started.elapsed());
    operation(&mut guard)
}
