//! End-to-end pipeline benchmarks for go-on.
//!
//! Measures throughput and latency of the critical paths:
//! - ToolRegistry lookup + governance check (every tool execution)
//! - Skill registry lookup + discovery (skills/list + skills/find)
//!
//! Run with: cargo bench --bench pipeline_bench
//!
//! Note: the former CacheLayer / CacheMetricsCollector benchmarks were removed
//! together with the `orchestration::cache_layer` module (zero production
//! callers — see docs/log/log-20260804-2.md).

use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use go_on::orchestration::skill::SkillRegistry;

// ═══════════════════════════════════════════════════════════════════════════
// 1. Skill registry lookup
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

criterion_group! {
    name = pipeline_benches;
    config = Criterion::default()
        .measurement_time(Duration::from_secs(5))
        .sample_size(50);
    targets =
        bench_skill_registry_discover,
}
criterion_main!(pipeline_benches);
