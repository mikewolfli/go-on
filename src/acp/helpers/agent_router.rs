use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, Default)]
struct RouteStat {
    success_count: u64,
    total_count: u64,
}

static TASK_AGENT_SUCCESS: OnceLock<Mutex<HashMap<(String, String), RouteStat>>> = OnceLock::new();

fn route_table() -> &'static Mutex<HashMap<(String, String), RouteStat>> {
    TASK_AGENT_SUCCESS.get_or_init(|| Mutex::new(HashMap::new()))
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
    let entry = guard.entry(key).or_default();
    entry.total_count = entry.total_count.saturating_add(1);
    if success {
        entry.success_count = entry.success_count.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_rate_is_zero_when_missing() {
        assert_eq!(task_agent_success_rate("missing-task", "missing-agent"), 0.0);
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
}
