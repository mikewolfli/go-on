## Zed 配置方法（ACP 与 MCP 模式）

### 1. 作为 External Agent（ACP 协议）

- 在 go-on 的 `config.toml` 中设置：
	```toml
	[protocol]
	mode = "acp"
	```
- 启动 go-on 后，在 Zed 设置面板 → Agents → 添加 External Agent，API URL 填写：
	```
	http://127.0.0.1:8090/v1
	```
- Zed 会自动识别为 ACP/A2A 协议，适合插件/助手/外部智能体集成。

### 2. 作为 LLM Provider（MCP 协议）

- 在 go-on 的 `config.toml` 中设置：
	```toml
	[protocol]
	mode = "mcp"
	```
- 启动 go-on 后，在 Zed 设置面板 → Agents → 添加 OpenAI Compatible provider，API URL 填写：
	```
	http://127.0.0.1:8090/v1
	```
- 可在 Zed 的 `settings.json` 里添加如下片段（示例）：
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
- 适合大模型推理、MCP 工具链等场景。

### 3. 自动自适应（推荐）

- 推荐配置：
	```toml
	[protocol]
	mode = "auto"
	```
- go-on 会自动识别 Zed 客户端协议类型，兼容 ACP/A2A 与 MCP。

---

**操作步骤简述：**
1. 启动 go-on（./start-go-on.sh 或 start-go-on.bat）。
2. 在 Zed 设置中添加 agent，API URL 指向 go-on 服务地址。
3. 根据实际需求选择 protocol mode（acp/mcp/auto）。
4. 无需重启即可切换协议模式。

如需更详细的 Zed 配置说明，可参考本节内容或官方文档。
# go-on

[简体中文](README.zh-CN.md) | English

go-on is a Rust ACP runtime with MCP adapter capabilities, focused on agent orchestration, runtime safety, and extensible workflow execution.

- Core runtime version: 0.5.3
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

## Setup And Status

Run interactive setup (provider selection + API key onboarding):

```bash
cargo run -- --setup --config config.toml
```

Wizard levels:

```bash
# Quick recommended setup
cargo run -- --setup --setup-level quick --config config.toml

# Standard recommended setup
cargo run -- --setup --setup-level standard --config config.toml

# Fully custom setup
cargo run -- --setup --setup-level custom --config config.toml
```

During setup:

- choose one or multiple model providers
- choose secret mode (`env`, `keyring`, or `auto`)
- optionally store only the selected providers' API keys into system keyring

Provider list source:

- Setup now reads provider capabilities from `providers.toml`.
- To add a new provider to wizard selection, append one `[[providers]]` entry into `providers.toml`.
- Setup and status now also read provider recommendation fields from `providers.toml`:
	- `recommended_default_phase`
	- `recommended_request_timeout_seconds`
	- `recommended_review_timeout_seconds`
	- `recommended_planning_request_timeout_seconds`
	- `recommended_coding_request_timeout_seconds`
	- `recommended_review_request_timeout_seconds`
	- `recommended_delivery_request_timeout_seconds`
	- `recommended_cache_enabled`
	- `recommended_vector_enabled`
	- `recommended_phase_max_inflight`
	- `recommended_global_max_inflight`

Apply provider recommendations to existing config in one command:

```bash
cargo run -- --apply-recommended --config config.toml
```

Check configured AI readiness:

```bash
cargo run -- --status --config config.toml
```

This prints overall runtime readiness plus each configured agent's ready flag, endpoint status, and missing environment variables.
It also prints a configuration completeness score (0-100), missing items, and recommended adjustments.

When startup finds no runtime-ready AI provider, it now auto-prompts you to either:

- configure AI quickly
- enter full wizard
- continue startup without provider

Add local model interface:

```bash
cargo run -- --add-local-model \
	--local-model-name local_llm \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-type openai \
	--local-model-model qwen2.5-coder \
	--config config.toml

# Register only under [agents], without auto-adding to phases
cargo run -- --add-local-model \
	--local-model-name local_shadow \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-register-only \
	--config config.toml
```

## Config Files (Current)

- Single template: `config.toml.autopilot-adaptive`
- Active runtime config: `config.toml`

### Protocol Mode (A2A/ACP/MCP auto-adapt)

In `config.toml`, you can set protocol mode for Zed or other clients:

```toml
[protocol]
# auto: auto-detect (recommended), acp: only ACP/A2A, mcp: only MCP
mode = "auto"
```

**auto**: Detects protocol from incoming requests (Zed, Copilot Studio, etc.)
**acp**: Only ACP/A2A methods allowed
**mcp**: Only MCP methods allowed

This ensures maximum compatibility with Zed (A2A/ACP) and new MCP-based clients.

#### Zed Integration

- For Zed external agent, set `mode = "auto"` (recommended) or `mode = "acp"` if only Zed is used.
- For MCP tools/clients, set `mode = "auto"` or `mode = "mcp"`.

No restart is needed after changing this config.

Provider credentials can be referenced by environment variable names or keyring references like `keyring://go-on/openai_compatible_api_key`.

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
