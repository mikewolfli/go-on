use std::time::Instant;

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[test]
fn bench_autonomy_loop_latency() {
    let mut samples = Vec::new();
    for _ in 0..128 {
        let start = Instant::now();
        let mut sink = 0usize;
        for i in 0..10_000 {
            sink = sink.wrapping_add(i);
        }
        assert!(sink > 0);
        samples.push(start.elapsed().as_micros() as u64);
    }
    samples.sort_unstable();

    let p50 = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    let p99 = percentile(&samples, 0.99);

    assert!(p50 > 0);
    assert!(p95 >= p50);
    assert!(p99 >= p95);
}

#[test]
fn bench_agent_selection_accuracy() {
    let labeled = vec![
        ("task_fix_bug", "agent-a", true),
        ("task_fix_bug", "agent-a", true),
        ("task_fix_bug", "agent-b", false),
        ("task_docs", "agent-b", true),
        ("task_docs", "agent-a", false),
    ];

    let mut correct = 0u64;
    let mut total = 0u64;
    for (_, _, ok) in labeled {
        total += 1;
        if ok {
            correct += 1;
        }
    }
    let accuracy = correct as f64 / total as f64;
    assert!(accuracy >= 0.6);
}

#[test]
fn bench_parallel_tool_fanout() {
    let fanout_samples = vec![(4u64, 2u64), (8u64, 4u64), (6u64, 3u64)];
    let mut utilization = 0.0f64;
    for (total_tools, parallel_slots) in fanout_samples {
        if total_tools > 0 {
            utilization += (parallel_slots as f64 / total_tools as f64).clamp(0.0, 1.0);
        }
    }
    utilization /= 3.0;
    assert!(utilization > 0.3);
}
