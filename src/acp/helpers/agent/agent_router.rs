use indexmap::IndexMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, Default)]
struct RouteStat {
    success_count: u64,
    total_count: u64,
}

/// Maximum number of (task_type, agent_name) entries to keep in the route cache.
/// When full, the oldest entry is evicted on each insert.
const MAX_ROUTE_ENTRIES: usize = 10_000;

// ── Observability metrics ────────────────────────────────────────────────

/// Current number of entries in the agent router table.
pub(crate) static AGENT_ROUTER_ENTRY_COUNT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Total number of evictions performed since startup.
pub(crate) static AGENT_ROUTER_EVICTION_TOTAL: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

static TASK_AGENT_SUCCESS: OnceLock<Mutex<IndexMap<(String, String), RouteStat>>> = OnceLock::new();

fn route_table() -> &'static Mutex<IndexMap<(String, String), RouteStat>> {
    TASK_AGENT_SUCCESS.get_or_init(|| Mutex::new(IndexMap::new()))
}

pub(crate) fn task_agent_success_rate(task_type: &str, agent_name: &str) -> f64 {
    let guard = match route_table().lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    let key = (task_type.to_string(), agent_name.to_string());
    guard
        .get(&key)
        .map(|stat| {
            if stat.total_count == 0 {
                0.0
            } else {
                stat.success_count as f64 / stat.total_count as f64
            }
        })
        .unwrap_or(0.0)
}

pub(crate) fn record_task_agent_outcome(task_type: &str, agent_name: &str, success: bool) {
    let mut guard = match route_table().lock() {
        Ok(g) => g,
        Err(poison) => poison.into_inner(),
    };
    let key = (task_type.to_string(), agent_name.to_string());

    // LRU eviction: if we are about to insert a new key and the map is full,
    // remove the oldest entry (front of insertion order).
    if !guard.contains_key(&key) && guard.len() >= MAX_ROUTE_ENTRIES {
        let oldest = guard.keys().next().cloned();
        if let Some(k) = oldest {
            guard.shift_remove(&k);
            AGENT_ROUTER_EVICTION_TOTAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    let entry = guard.entry(key).or_default();
    entry.total_count = entry.total_count.saturating_add(1);
    if success {
        entry.success_count = entry.success_count.saturating_add(1);
    }

    AGENT_ROUTER_ENTRY_COUNT.store(guard.len() as u64, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_rate_is_zero_when_missing() {
        assert_eq!(
            task_agent_success_rate("missing-task", "missing-agent"),
            0.0
        );
    }

    #[test]
    fn success_rate_updates_with_outcomes() {
        let task = "router-test-task";
        let agent = "router-test-agent";
        record_task_agent_outcome(task, agent, true);
        record_task_agent_outcome(task, agent, false);
        let rate = task_agent_success_rate(task, agent);
        assert!(rate > 0.45 && rate < 0.55);
    }

    #[test]
    fn lru_eviction_removes_oldest_entry() {
        // Insert MAX_ROUTE_ENTRIES + 1 entries so one must be evicted.
        let n = MAX_ROUTE_ENTRIES + 1;
        for i in 0..n {
            let task = format!("lru-task-{i}");
            let agent = format!("lru-agent-{i}");
            record_task_agent_outcome(&task, &agent, true);
        }

        // The oldest entry (0) should have been evicted.
        assert_eq!(
            task_agent_success_rate("lru-task-0", "lru-agent-0"),
            0.0,
            "oldest entry should have been evicted"
        );

        // The newest entry should still be present.
        let newest_rate = task_agent_success_rate(
            &format!("lru-task-{}", n - 1),
            &format!("lru-agent-{}", n - 1),
        );
        assert_eq!(newest_rate, 1.0, "newest entry should remain");

        // Entry count must not exceed the cap.
        let guard = route_table().lock().unwrap();
        assert!(guard.len() <= MAX_ROUTE_ENTRIES);
    }

    #[test]
    fn eviction_counter_increments() {
        // Use unique keys so the insertion count is predictable regardless of
        // what other tests left in the shared global table.
        let suffix = std::sync::atomic::AtomicU64::new(0);
        let next_key = || {
            let n = suffix.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (format!("evict-count-{n}"), format!("evict-agent-{n}"))
        };

        let evictions_before =
            AGENT_ROUTER_EVICTION_TOTAL.load(std::sync::atomic::Ordering::Relaxed);

        // Insert MAX_ROUTE_ENTRIES + 1 entries to guarantee at least one eviction.
        for _ in 0..=MAX_ROUTE_ENTRIES {
            let (task, agent) = next_key();
            record_task_agent_outcome(&task, &agent, true);
        }

        let evictions_after =
            AGENT_ROUTER_EVICTION_TOTAL.load(std::sync::atomic::Ordering::Relaxed);

        assert!(
            evictions_after > evictions_before,
            "eviction total should have increased (was {evictions_before}, now {evictions_after})"
        );
    }

    #[test]
    fn entry_count_metric_reflects_table_size() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let before = AGENT_ROUTER_ENTRY_COUNT.load(Ordering::Relaxed);

        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        record_task_agent_outcome(
            &format!("entry-count-task-{n}"),
            &format!("entry-count-agent-{n}"),
            false,
        );
        record_task_agent_outcome(
            &format!("entry-count-task2-{n}"),
            &format!("entry-count-agent2-{n}"),
            true,
        );

        let after = AGENT_ROUTER_ENTRY_COUNT.load(Ordering::Relaxed);

        let delta = after - before;
        assert!(
            delta >= 2,
            "expected at least 2 new entries in the count, got {delta}"
        );
    }
}
