//! Basic usage example for the go-on Node.js SDK.
//!
//! Run with: `npx ts-node examples/basic-usage.ts`
//! (requires a running go-on backend at http://127.0.0.1:8090)

import { GoOnClient } from "../src";

async function main() {
  const client = new GoOnClient({ baseUrl: "http://127.0.0.1:8090" });

  try {
    // ── Health check ────────────────────────────────────────────────
    console.log("Checking health...");
    const health = await client.health();
    console.log(`  Status: ${health.status}`);
    console.log(`  Version: ${health.version}`);
    console.log(`  Uptime: ${health.uptime_seconds}s`);

    // ── Governance status ──────────────────────────────────────────
    console.log("\nGovernance status...");
    const governance = await client.governanceStatus();
    console.log(`  OK: ${governance.ok}`);

    // ── Metrics ────────────────────────────────────────────────────
    console.log("\nGetting metrics...");
    const metrics = await client.metricsGet();
    console.log(`  Metrics keys: ${Object.keys(metrics.metrics).length}`);

    // ── Circuit breaker status ─────────────────────────────────────
    console.log("\nCircuit breaker status...");
    const breaker = await client.breakerStatus();
    console.log(`  Breakers: ${JSON.stringify(breaker.breakers)}`);

    // ── Stream a chat message ─────────────────────────────────────
    console.log("\nStreaming chat...");
    const chunks: string[] = [];
    for await (const chunk of client.chatStream([
      { role: "user", content: "Say hello in one word." },
    ])) {
      chunks.push(chunk);
    }
    console.log(`  Response: ${chunks.join("")}`);
  } catch (err) {
    console.error("Error:", err);
  } finally {
    client.close();
  }
}

main();
