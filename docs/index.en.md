# Go-On Documentation Index

## Overview

Go-On is an ACP runtime proxy with integrated multi-agent orchestration, capable of running as a local development tool, a simple server, or a multi-user production server.

## Getting Started

- [README](../../README.md) — Project overview and quick start
- [Development Rules](DEVELOPMENT_RULES.md) — Coding standards and contribution guidelines
- [Protocol Guide](protocol-guide.md) — ACP/MCP protocol integration
- [GUI Guide](gui-guide.md) — Desktop GUI usage

## Architecture

### Core System

| Module | Description |
|--------|-------------|
| `src/core/` | Configuration, error handling, bootstrap, providers |
| `src/acp/` | ACP protocol implementation (chat, runtime, transport) |
| `src/mcp/` | MCP protocol compatibility layer |
| `src/protocol/` | Shared protocol types and server definitions |

### AI Orchestration

| Module | Description |
|--------|-------------|
| `src/agents/` | AI provider adapters (OpenAI, Anthropic, DeepSeek, etc.) |
| `src/orchestration/` | Skill system, tool registry, planner, task routing |
| `src/intelligence/` | Capability bus, model selection, reinforcement learning |
| `src/memory/` | Vector store, cache, embedding providers |

### Governance & Security

| Module | Description |
|--------|-------------|
| `src/governance/` | Sandbox, RBAC, audit, PUA rules, governance policies |
| `src/security/` | Authentication, encryption, prompt injection detection |

### Tools

- [Tool System](guides/tool-system.en.md) — Tool registry, pipeline, and custom tools
- [Code Index](guides/code-index.en.md) — Semantic code search tool
- [Skill System](../cookbook/src/en/skills.md) — SKILL.md discovery, import, execution

### Observability

| Module | Description |
|--------|-------------|
| `src/observability/` | Telemetry, performance metrics |
| `src/intelligence/capability_bus/optimization_bus.rs` | OptimizationBus — cost/speed/reliability recommendations |

## Client Integration

- [VS Code Extension](../vscode-addon/README.md) — IDE integration with ACP/MCP
- [GUI Application](../cookbook/src/en/gui.md) — EGUI/eframe-based desktop application
- [Rust SDK](../sdk/rust/src/lib.rs) — Programmatic access (see `sdk/rust/examples/` for usage)

## Deployment

- [Configuration](workflow-config.md) — Profile and workflow configuration
- [Deployment Guide](../cookbook/src/en/deployment/simple-server.md) — Simple server deployment
- [Multi-Users Deployment](../cookbook/src/en/deployment/multi-users-server.md) — Multi-user production deployment

## Blueprints

- [Principle](blueprints/principle.md) — Core development principles
- [Skill Market](blueprints/skill-market.md) — Community plugin marketplace plan

## Logs

- [Scan Logs](log/) — Multi-round deep scan records
- [Reports](reports/) — Optimization and analysis reports
