//! End-to-end pipeline benchmarks for go-on.
//!
//! Measures throughput and latency of the critical paths:
//! - ToolRegistry lookup + governance check (every tool execution)
//! - CacheLayer stats aggregation (governance/metrics endpoint)
//! - Skill registry lookup + discovery (skills/list + skills/find)
//!
//! Run with: cargo bench --bench pipeline_bench

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use go_on::orchestration::cache_layer::{CacheLayer, CacheMetricsCollector, CacheStats};
use go_on::orchestration::skill::SkillRegistry;

/// Minimal `CacheLayer` stand-in used to measure the collector's aggregation
/// and serialization cost independently of any concrete cache backend.
struct StatsOnlyCache {
    hits: u64,
    misses: u64,
    entries: usize,
}

impl CacheLayer for StatsOnlyCache {
    fn name(&self) -> &str {
        "stats_only"
    }

    fn stats(&self) -> CacheStats {
        CacheStats {
            hits: self.hits,
            misses: self.misses,
            entries: self.entries,
            max_entries: 128,
            estimated_size_bytes: self.entries.saturating_mul(48),
        }
    }

    fn clear(&mut self) {
        self.hits = 0;
        self.misses = 0;
        self.entries = 0;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. CacheLayer stats aggregation (for governance/metrics endpoint)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_cache_layer_aggregate_stats(c: &mut Criterion) {
    let mut collector = CacheMetricsCollector::with_capacity(8);
    // Simulate registering several caches.
    for _ in 0..4 {
        collector.register(Box::new(StatsOnlyCache {
            hits: 10_000,
            misses: 500,
            entries: 1024,
        }));
    }

    c.bench_function("cache_layer/aggregate_stats", |b| {
        b.iter(|| {
            let _ = black_box(collector.aggregate_stats());
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Skill registry lookup
// ═══════════════════════════════════════════════════════════════════════════

fn bench_skill_registry_discover(c: &mut Criterion) {
    let mut registry = SkillRegistry::default();

    // Register N skills
    for i in 0..50 {
        use async_trait::async_trait;
        use go_on::orchestration::skill::Skill;
        use serde_json::Value;

        struct DummySkill(&'static str, &'static str);
        #[async_trait]
        impl Skill for DummySkill {
            fn name(&self) -> &str {
                self.0
            }
            fn description(&self) -> &str {
                self.1
            }
            async fn execute(&self, _input: &Value) -> anyhow::Result<Value> {
                Ok(serde_json::json!({}))
            }
        }

        registry
            .register(Arc::new(DummySkill(
                Box::leak(format!("skill_{}", i).into_boxed_str()),
                Box::leak(format!("description for skill number {}", i).into_boxed_str()),
            )))
            .ok();
    }

    c.bench_function("skill_registry/discover_50", |b| {
        b.iter(|| {
            let _ = black_box(registry.discover_skills("find me a skill for code review", 5));
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. CacheMetricsCollector with multiple caches (governance endpoint)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_cache_metrics_serialize(c: &mut Criterion) {
    let mut collector = CacheMetricsCollector::with_capacity(8);
    for _ in 0..6 {
        collector.register(Box::new(StatsOnlyCache {
            hits: 5_000,
            misses: 250,
            entries: 512,
        }));
    }

    c.bench_function("cache_metrics/serialize_json", |b| {
        let all = collector.all_stats();
        b.iter(|| {
            let _ = black_box(serde_json::to_value(&all));
        });
    });
}

criterion_group! {
    name = pipeline_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .sample_size(50);
    targets =
        bench_cache_layer_aggregate_stats,
        bench_skill_registry_discover,
        bench_cache_metrics_serialize,
}
criterion_main!(pipeline_benches);
