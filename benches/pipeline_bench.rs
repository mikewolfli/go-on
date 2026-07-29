//! End-to-end pipeline benchmarks for go-on.
//!
//! Measures throughput and latency of the critical paths:
//! - ToolRegistry lookup + governance check (every tool execution)
//! - CacheLayer stats aggregation (governance/metrics endpoint)
//! - Skill registry lookup + discovery (skills/list + skills/find)
//! - FastPathCache hit/miss (full-auto flow cache)
//!
//! Run with: cargo bench --bench pipeline_bench

use std::sync::Arc;
use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use go_on::orchestration::cache_layer::CacheMetricsCollector;
use go_on::orchestration::fast_path_cache::FastPathCache;
use go_on::orchestration::skill::SkillRegistry;

// ═══════════════════════════════════════════════════════════════════════════
// 1. CacheLayer stats aggregation (for governance/metrics endpoint)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_cache_layer_aggregate_stats(c: &mut Criterion) {
    let mut collector = CacheMetricsCollector::with_capacity(8);
    // Simulate registering several caches.
    for _ in 0..4 {
        let cache = FastPathCache::new();
        collector.register(Box::new(cache));
    }

    c.bench_function("cache_layer/aggregate_stats", |b| {
        b.iter(|| {
            let _ = black_box(collector.aggregate_stats());
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. FastPathCache hit & miss (full-auto flow)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_fast_path_cache_get_intent(c: &mut Criterion) {
    let cache = FastPathCache::new();
    // Populate with N entries
    for i in 0..100 {
        cache.set_intent(
            &format!(
                "task number {} with some padding text to make it realistic",
                i
            ),
            go_on::orchestration::fast_path_cache::IntentCacheValue {
                goals: vec![format!("goal {}", i)],
                constraints: vec![],
                prerequisites: vec![],
                deliverables: vec![],
            },
        );
    }

    let hit_key = "task number 42 with some padding text to make it realistic";

    c.bench_function("fast_path_cache/get_intent_hit", |b| {
        b.iter(|| {
            let _ = black_box(cache.get_intent(hit_key));
        });
    });

    c.bench_function("fast_path_cache/get_intent_miss", |b| {
        b.iter(|| {
            let _ = black_box(cache.get_intent("completely unknown task description"));
        });
    });
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Skill registry lookup
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
// 4. CacheMetricsCollector with multiple caches (governance endpoint)
// ═══════════════════════════════════════════════════════════════════════════

fn bench_cache_metrics_serialize(c: &mut Criterion) {
    let mut collector = CacheMetricsCollector::with_capacity(8);
    for _ in 0..6 {
        collector.register(Box::new(FastPathCache::new()));
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
        bench_fast_path_cache_get_intent,
        bench_skill_registry_discover,
        bench_cache_metrics_serialize,
}
criterion_main!(pipeline_benches);
