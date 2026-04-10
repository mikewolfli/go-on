# go-on

[简体中文](README.zh-CN.md) | English

go-on is a Rust ACP runtime with MCP adapter capabilities, focused on agent orchestration, runtime safety, and extensible workflow execution.

## Version

- Core runtime version: 0.4.1
- Default compile profile: `local-acp-sqlite`
- Optional profile flag: `server-mcp-postgres` (feature scaffold)

## Current Source Structure

- `src/acp`: ACP server, request dispatch, chat/review/runtime/background logic
- `src/agents`: provider adapters and shared agent contract
- `src/core`: configuration, setup, validation, context, error model
- `src/governance`: runtime controls, review controls, policy enforcement
- `src/i18n`: language runtime and watcher
- `src/intelligence`: selectors, verification, reinforcement, evaluation modules
- `src/mcp`: MCP helpers and adapter-layer support
- `src/memory`: cache/vector/memory stores
- `src/observability`: telemetry, performance, observability helpers
- `src/optimization`: cost/speed/reliability/failure-prevention/workflow optimizers
- `src/orchestration`: flow, mode, task graph/router/decomposer/tool orchestration
- `src/protocol`: protocol servers and JSON-RPC support

## Runtime RPC Surface (Implemented)

Main methods currently handled in ACP request routing include:

- Core: `initialize`, `chat`, `phase`, `shutdown`, `runtime.health`
- MCP adapter: `mcp.initialize`, `mcp.tools.list`, `mcp.tools.call`
- Metrics/trace: `metrics.get`, `metrics.prometheus`, `trace.get`, `trace.metrics`, `debug_panel.get`
- Controls: `breaker.status`, `breaker.reset`, `cache.clear`, `vector.clear`, `maintenance.gc`, `config.reload`
- Workflow/task: `workflow.confirm`, `workflow.clarify`, `workflow.research`, `workflow.consult`, `workflow.generate`, `workflow.execute`, `task.plan`, `task.execute`
- Learning/autotune: `learning.summary`, `autotune.get`, `autotune.status`, `autotune.reset`, `action.check`
- Conversation ops: checkpoint create/list/prune and rollback

## Build And Validation

```bash
cargo build
cargo check
cargo clippy -- -D warnings
cargo test
```

## VS Code Add-on

- Extension docs: [vscode-addon/README.md](vscode-addon/README.md)
- Synced extension version: 0.4.1

## Roadmaps

- [FUTURE2](FUTURE2.MD)
- [FUTURE3](FUTURE3.MD)
- [FUTURE4](FUTURE4.MD)
- [FUTURE5](FUTURE5.MD)
- [FUTURE6](FUTURE6.MD)

## License

Same as repository policy.
