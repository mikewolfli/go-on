# SERVER-BLUE1

## 2026-04-11 Scope Clarification: Single-Node vs MCP Server

This note records which capabilities are required for MCP server deployments versus optional for single-node usage.

| Capability | Single-Node Version | MCP Server Version | Decision |
|---|---|---|---|
| Unified pluggable strategy engine (dynamic loading / multi-strategy orchestration) | Not required by default | Required for platform operation | Prioritize on MCP server roadmap |
| Online A/B optimizer switching | Optional (manual config switch is usually enough) | Required for gray rollout / rollback / experimentation | Prioritize on MCP server roadmap |
| Offline evaluation feedback into real-time routing decisions | Optional (can be manual or periodic) | Strongly recommended for continuous optimization loop | Prioritize on MCP server roadmap |
| Tenant/project-specific promotion/workflow policy injection | Usually not needed | Required for multi-tenant isolation and customization | Prioritize on MCP server roadmap |

## Current Baseline

- Main-chain closure is already complete for current single-node runtime paths.
- Lightweight extension interfaces are retained for future expansion:
  - `src/intelligence/promotion.rs`
  - `src/optimization/workflow_optimizer.rs`
- Do not force full platform features into single-node main chain unless deployment mode explicitly requires it.
