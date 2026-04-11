# go-on

[简体中文](README.zh-CN.md) | English

go-on is a Rust ACP runtime with MCP adapter capabilities, focused on agent orchestration, runtime safety, and extensible workflow execution.

## Version

- Core runtime version: 0.4.7
- Default compile profile: `local-acp-sqlite`
- Optional profile flag: `server-mcp-postgres` (feature scaffold)


## Current Source Structure

All main implementation code is under the `src/` directory:

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

Other directories:

- `test_i18n/`: i18n integration tests
- `tests/`: integration and scenario tests

There are no top-level `core`, `agents`, etc. directories outside `src/`.

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

## Config Files (Current)

- Single template: `config.toml.autopilot-adaptive`
- Active runtime config: `config.toml`

If you want to reset local config to latest template:

```bash
cp config.toml.autopilot-adaptive config.toml
cargo run -- --config config.toml --validate-config
```

## VS Code Add-on

- Extension docs: [vscode-addon/README.md](vscode-addon/README.md)
- Synced extension version: 0.4.7

## Roadmaps

- [FUTURE2](FUTURE2.MD)
- [FUTURE3](FUTURE3.MD)
- [FUTURE4](FUTURE4.MD)
- [FUTURE5](FUTURE5.MD)
- [FUTURE6](FUTURE6.MD)

## License

Same as repository policy.

## Quick Start: go-on with Zed

### 1. Start go-on backend

For macOS/Linux:
```sh
./start-go-on.sh
```
For Windows:
```bat
start-go-on.bat
```
This will launch go-on on port 8090 and write logs to go-on.log.

### 2. Zed Integration (OpenAI Compatible)

1. Open Zed, go to Agent Panel settings.
2. Add an OpenAI Compatible provider.
3. Set API URL to: `http://127.0.0.1:8090/v1`
4. Example settings.json snippet:
```json
{
	"language_models": {
		"openai_compatible": {
			"go-on": {
				"api_url": "http://127.0.0.1:8090/v1",
				"available_models": [
					{
						"name": "go-on",
						"max_tokens": 200000,
						"max_output_tokens": 32000,
						"max_completion_tokens": 200000,
						"capabilities": {
							"tools": true,
							"images": false,
							"parallel_tool_calls": false,
							"prompt_cache_key": false,
							"chat_completions": true
						}
					}
				]
			}
		}
	}
}
```
5. Set go-on as the default model if needed.

> Note: go-on must be running before Zed can connect. Automatic start/stop is not yet supported by Zed.
