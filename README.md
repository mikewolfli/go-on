## Zed 配置方法（ACP 与 MCP 模式）

### 1. 作为 External Agent（ACP 协议）

- 在 go-on 的 `config.toml` 中设置：
	```toml
	[protocol]
	mode = "acp_http"
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
	mode = "mcp_http"
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
	mode = "adaptive"
	```
- go-on 会自动识别客户端协议与传输场景，兼容 ACP/A2A 与 MCP。

### 4. 协议接入模式（5 选项）

`[protocol].mode` 支持以下 5 个值：

- `adaptive`（默认）：自适应接入，按业务/上下文选择 ACP/MCP 与 transport。
- `acp_stdio`：ACP + STDIO。
- `acp_http`：ACP + HTTP。
- `mcp_stdio`：MCP + STDIO。
- `mcp_http`：MCP + HTTP。

---

**操作步骤简述：**
1. 启动 go-on（./start-go-on.sh 或 start-go-on.bat）。
2. 在 Zed 设置中添加 agent，API URL 指向 go-on 服务地址。
3. 根据实际需求选择 protocol mode（adaptive/acp_stdio/acp_http/mcp_stdio/mcp_http）。
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

## Setup And Readiness

First run is now installation-style:

- If `config.toml` is missing AI setup, blank, or has no runtime-ready agent, go-on enters onboarding instead of throwing a config error.
- A blank `config.toml` is auto-filled with runnable non-AI defaults (`runtime`, `cache`, `vector`, `autotune`, `flow`, `phases`).
- AI credentials stay empty until you choose a provider.

Recommended commands:

```bash
# Recommended first-run entry
cargo run -- --init --config config.toml

# Check readiness and configuration completeness
cargo run -- --check --config config.toml

# Validate config without starting services
cargo run -- --doctor --config config.toml
```

Onboarding modes:

```bash
# Quick mode: recommended values, minimal input
cargo run -- --init --setup-level quick --config config.toml

# Advanced mode: expand all setup details
cargo run -- --init --setup-level custom --config config.toml

# Optional middle preset
cargo run -- --init --setup-level standard --config config.toml
```

During onboarding you can:

- choose one or more model providers
- choose secret mode (`env`, `keyring`, or `auto`)
- skip AI for now and keep a usable base config
- see recommended values with reasons in the flow itself

Provider list source:

- Onboarding reads provider capabilities from `providers.toml`.
- To add a new provider to selection, append one `[[providers]]` entry into `providers.toml`.
- Onboarding and readiness checks both read recommendation fields from `providers.toml`:
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

Readiness output now includes:

- overall runtime readiness
- per-agent ready flag, endpoint status, and missing secrets
- configuration completeness score (0-100)
- missing items and recommended adjustments
- a clear pending item when AI provider setup was skipped

Add local model interface:

```bash
cargo run -- --add-model \
	--local-model-name local_llm \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-type openai \
	--local-model-model qwen2.5-coder \
	--config config.toml

# Register only under [agents], without auto-adding to phases
cargo run -- --add-model \
	--local-model-name local_shadow \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-register-only \
	--config config.toml
```

## Config Files (Current)

- Single template: `config.toml.autopilot-adaptive`
- Active runtime config: `config.toml`

### Protocol Mode (5-option adaptive access)

In `config.toml`, you can set protocol mode for Zed or other clients:

```toml
[protocol]
# adaptive (default), acp_stdio, acp_http, mcp_stdio, mcp_http
mode = "adaptive"
```

**adaptive**: Adaptive access (recommended)
**acp_stdio**: ACP over stdio
**acp_http**: ACP over HTTP
**mcp_stdio**: MCP over stdio
**mcp_http**: MCP over HTTP

This ensures maximum compatibility with Zed (A2A/ACP) and new MCP-based clients.

#### Zed Integration

- For Zed external agent over HTTP, use `mode = "acp_http"` (or `adaptive`).
- For MCP tools/clients, use `mode = "mcp_http"`/`"mcp_stdio"` (or `adaptive`).

No restart is needed after changing this config.

Provider credentials can be referenced by environment variable names or keyring references like `keyring://go-on/openai_compatible_api_key`.

If you want to reset local config to latest template:

```bash
cp config.toml.autopilot-adaptive config.toml
cargo run -- --doctor --config config.toml
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

## 快速开始（5分钟）
<!-- BLUE14-P2-2-README-QUICKSTART -->

1. 启动后端：

```bash
# Linux/macOS
./start-go-on.sh

# Windows
start-go-on.bat
```

2. 使用默认自适应协议（推荐）：

```toml
[protocol]
mode = "adaptive"
```

3. 连通性验证：

```bash
curl http://127.0.0.1:8090/health
```

4. 使用 VS Code 插件或 GUI 连接运行中的 go-on。

## 四种模式（主用）
<!-- BLUE14-P2-2-README-MODES -->

日常使用建议优先选择下列四种主用模式；此外仍支持 `adaptive` 作为默认自动协商模式。

| 模式 | 说明 | 典型场景 |
|---|---|---|
| `acp_stdio` | ACP over stdio | 本地 IDE 内嵌链路 |
| `acp_http` | ACP over HTTP | GUI/外部 Agent 走 HTTP |
| `mcp_stdio` | MCP over stdio | MCP 客户端本地接入 |
| `mcp_http` | MCP over HTTP | 远程 MCP 服务接入 |

附加模式：`adaptive`（默认）根据请求路径与上下文自动协商 ACP/MCP。

## 配置速查
<!-- BLUE14-P2-2-README-CHEATSHEET -->

最常用运行参数：

| 参数 | 作用 | 示例 |
|---|---|---|
| `--config` | 指定配置文件 | `--config config.toml` |
| `--protocol-mode` | 覆盖配置中的协议模式 | `--protocol-mode mcp_http` |
| `--acp-http-bind` | 指定 HTTP 监听地址 | `--acp-http-bind 127.0.0.1:8090` |
| `--verbose` | 输出详细日志 | `--verbose` |

最小可运行配置：

```toml
[runtime]
acp_http_bind_addr = "127.0.0.1:8090"

[protocol]
mode = "adaptive"
```

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
