//! Multi-Agent Task Orchestration Example
//!
//! Demonstrates how to use the go-on Rust SDK to:
//! 1. Set up an ACP client connected to a go-on runtime
//! 2. Plan a task that involves multiple agents
//! 3. Execute the task and collect results
//! 4. Check governance status and other runtime information
//!
//! Run with:
//! ```bash
//! cargo run --example multi_agent_orchestration -- http://127.0.0.1:8090
//! ```
//!
//! If no URL is provided, defaults to http://127.0.0.1:8090.

use std::env;
use std::time::Duration;

use go_on_sdk::GoOnClientBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Parse the server URL ─────────────────────────────────────────
    let base_url = env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8090".to_string());

    println!("🔌 Connecting to go-on runtime at {base_url}");

    // ── 2. Build a client with sensible defaults ────────────────────────
    let client = GoOnClientBuilder::new(&base_url)
        .with_timeout(Duration::from_secs(30))
        .with_max_retries(2)
        .build()?;

    // ── 3. Initialize the runtime ───────────────────────────────────────
    println!("🚀 Initializing runtime...");
    let init_result = client.initialize(Some("full")).await?;
    println!(
        "   ✓ Runtime initialized: {}",
        init_result
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("ok")
    );

    // ── 4. Check health ─────────────────────────────────────────────────
    // `runtime.health` (JSON-RPC) emits `version`; `GET /health`
    // (ServerStatus) does not, so use the RPC variant here.
    let health = client.runtime_health().await?;
    println!(
        "   ✓ Health: version={} lifecycle={}",
        health.version.as_deref().unwrap_or("unknown"),
        health
            .lifecycle
            .as_ref()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    );

    // ── 5. Check governance status (agents, capabilities, phases) ───────
    println!("📋 Checking governance status...");
    let governance = client.governance_status().await?;
    println!("   ✓ Governance: ok={}", governance.ok);

    // ── 6. Plan a multi-agent task ──────────────────────────────────────
    // The go-on runtime will decompose the description into a plan that
    // may involve multiple agents (planner, coder, reviewer, etc.)
    println!("📝 Planning multi-agent task...");
    let task_description = "\
        Create a REST API endpoint that:\n\
        1. Accepts POST /api/users with JSON body { name, email }\n\
        2. Validates the input (name non-empty, email format)\n\
        3. Stores the user in a database\n\
        4. Returns 201 Created with the user object\n\
        5. Has proper error handling for duplicate email\n\
        Use Rust with Actix-Web framework.\
    ";
    let plan = client.task_plan(task_description).await?;
    println!("   ✓ Plan received:");
    println!("     {}", serde_json::to_string_pretty(&plan.plan)?);

    // ── 7. Execute the planned task ─────────────────────────────────────
    // task.execute takes the task text (the backend re-plans/executes it);
    // plan_id is not consumed by the current backend contract.
    println!("🚀 Executing task...");
    let execution = client.task_execute(task_description).await?;
    println!(
        "   ✓ Execution started: {}",
        execution
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
    );

    // ── 8. Check circuit breaker status (reliability monitoring) ────────
    println!("🔧 Checking reliability status...");
    let breakers = client.breaker_status().await?;
    println!(
        "   ✓ Circuit breakers: {}",
        serde_json::to_string(&breakers.breakers)?
    );

    // ── 9. Fetch runtime metrics ────────────────────────────────────────
    println!("📊 Fetching runtime metrics...");
    let metrics = client.metrics_get().await?;
    println!("   ✓ Metrics: {}", serde_json::to_string(&metrics.metrics)?);

    // ── 10. View learning summary (agent performance data) ──────────────
    println!("🧠 Fetching learning summary...");
    let learning = client.learning_summary().await?;
    println!(
        "   ✓ Learning summary: {}",
        serde_json::to_string(&learning.summary)?
    );

    // ── 11. Check model selector status ─────────────────────────────────
    println!("🎯 Checking model selector status...");
    let selector = client.selector_status().await?;
    println!(
        "   ✓ Selector: {}",
        serde_json::to_string(&selector.selector)?
    );

    // ── 12. Retrieve health probes (module-level health) ────────────────
    println!("💓 Retrieving health probes...");
    let probes = client.health_probes().await?;
    println!("   ✓ Probes: {}", serde_json::to_string(&probes.modules)?);

    // ── Summary ─────────────────────────────────────────────────────────
    println!();
    println!("═══════════════════════════════════════════════");
    println!("  ✅ Multi-agent orchestration cycle complete");
    println!("═══════════════════════════════════════════════");
    println!();
    println!("Agents involved in this flow:");
    println!("  • planner     — decomposed the task into a plan");
    println!("  • coder       — generates code for each step");
    println!("  • reviewer    — reviews generated code");
    println!("  • orchestrator— coordinates agent handoffs");
    println!();
    println!("The go-on runtime handles agent selection, circuit breaking,");
    println!("rate limiting, and failure recovery automatically.");

    Ok(())
}
