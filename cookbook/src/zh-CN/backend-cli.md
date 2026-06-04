# 后端 CLI

后端可执行文件是当前系统的权威控制面，负责运行时启动、setup、健康检查、任务规划以及协议模式选择。

## 调用方式

生产或打包后的二进制：

```bash
go-on --config config.toml
```

开发阶段：

```bash
cargo run -- --config config.toml
```

当前帮助入口形式为：

```text
Usage: go-on.exe [OPTIONS]
```

没有子命令，所有操作由标志驱动。

## 核心运行时选项

### `--config <CONFIG>`

指定显式配置文件路径。如果省略，运行时将从可执行文件所在目录解析 `config.toml`。

示例：

```bash
go-on --config D:\go-on\config.toml
```

### `--phase <PHASE>`

选择要运行的特定阶段（phase）配置文件。当你的配置定义了多个阶段行为，并希望使用一个确定的入口点时使用。

### `--verbose`

启用详细日志输出。在诊断启动、配置、传输或 Provider 就绪问题时首先使用此选项。

## Phase 与 Sub-Phase 配置

Phase 定义运行时执行的工作流阶段。每个 phase 可以包含可选的 sub-phase，实现更精细的控制。

### 在 config.toml 中配置 phase

Phase 在 `[phases.<name>]` 节中配置，由 `flow.phases` 列表引用：

```toml
[flow]
# 执行顺序。根据你的工作流增删 phase。
phases = ["think", "act", "check", "done"]

[phases.think]
description = "Think — 分析、规划、收集上下文"
# 分配给此 phase 的 agent（空 = setup wizard 会提示填写）
agents = []
# 为 true 时，即使没有配置 agent 也继续执行
fallback = true

[phases.think.options]
request_timeout_seconds = 120
review_timeout_seconds = 60
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 8      # 此 phase 内最大并发任务数
global_max_inflight = 128    # 所有 phase 全局最大并发任务数
```

### Sub-phases（子阶段）

Sub-phases 提供分层工作流分解。一个 phase 可以定义 `sub_phases` 列表，配合嵌套的 `[phases.<parent>.<child>]` 节：

```toml
[flow]
phases = ["think", "act", "check", "done"]

[phases.act]
description = "主执行阶段"
agents = []
fallback = true
# Sub-phases 在此 phase 内按顺序执行
sub_phases = ["plan", "code", "test"]

[phases.act.options]
request_timeout_seconds = 300
cache_enabled = true
phase_max_inflight = 24

[phases.act.plan]
description = "实现计划"
agents = []
fallback = true

[phases.act.plan.options]
request_timeout_seconds = 120
phase_max_inflight = 4

[phases.act.code]
description = "编写代码"
agents = []
fallback = true

[phases.act.code.options]
request_timeout_seconds = 180
phase_max_inflight = 12

[phases.act.test]
description = "运行测试"
agents = []
fallback = true

[phases.act.test.options]
request_timeout_seconds = 120
phase_max_inflight = 8
```

Sub-phases 会继承父级的 `options` 作为默认值，可在每个 sub-phase 中覆盖。

### Phase-only 与 sub-phase 执行的区别

- **无 sub-phases**：每个 phase 按 `phases` 列表顺序从上到下依次执行。
- **有 sub-phases**：父 phase 先编排其 sub-phases 按顺序执行，完成后才进入下一个父 phase。
- Sub-phases 是可选的——大多数工作流使用扁平 phase 即可。

### 内置 phase 预设文件

项目内置四个预设配置文件，各有不同的 phase 设置：

| 文件 | Phases | 适用场景 |
|------|--------|----------|
| `config.toml` | think, act, check, done | 通用工作流（默认） |
| `config.coding.toml` | coding | IDE 集成（Zed、VS Code） |
| `config.simple-server.toml` | think, act, check, done | 单服务部署 |
| `config.multi-users-server.toml` | think, act, check, done | 多用户企业环境 |

### 使用特定 phase 配置

```bash
# 使用编码阶段配置与 IDE 配合
go-on --config config.coding.toml --phase coding

# 使用通用配置配合 HTTP 端点
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

### 创建自定义 phase

你可以定义任意 phase 名称——没有内置限制：

```toml
[flow]
phases = ["research", "draft", "review", "approve", "publish"]

[phases.research]
description = "收集信息和资料"
agents = []
fallback = true

[phases.research.options]
request_timeout_seconds = 180
cache_enabled = true
vector_enabled = true
summary_enabled = true
phase_max_inflight = 4
```

### 每个 phase 的关键选项

| 选项 | 默认值 | 说明 |
|--------|---------|------|
| `request_timeout_seconds` | 150 | 此 phase 中单个任务请求的最大时间 |
| `review_timeout_seconds` | 60 | 此 phase 中审查的最大时间 |
| `review_timeout_policy` | `"reject"` | 审查超时时的处理方式（`"reject"`、`"degrade_single"` 或 `"warn"`） |
| `review_min_response_chars` | 12 | 审查回复的最小字符数 |
| `cache_enabled` | true | 在此 phase 中启用缓存查找 |
| `vector_enabled` | true | 在此 phase 中启用向量存储查找 |
| `summary_enabled` | true | 启用对话摘要 |
| `phase_max_inflight` | 24 | 此 phase 内最大并发任务数 |
| `global_max_inflight` | 128 | 所有 phase 全局最大并发任务数 |
| `autopilot_complexity` | `"auto"` | 复杂度模式：`"auto"`、`"simple"`、`"complex"` |

## 验证与就绪检查

### `--validate-config` 或 `--doctor`

验证配置并退出。在排查更大的运行时问题之前，这是最快的快速检查。

```bash
go-on --config config.toml --validate-config
```

### `--status` 或 `--check`

打印已配置的 AI Provider 和运行时就绪状态。

在 setup 之后、编辑 `config.toml` 之后或附加编辑器客户端之前使用。

```bash
go-on --status
```

### `--healthcheck`

生成运行时健康报告并持久化到 `.goon/` 下。当需要持久化的工件用于后续审查或分类时使用。

```bash
go-on --healthcheck
```

## Setup 与推荐工作流

### `--setup` 或 `--init`

运行交互式设置向导。

```bash
go-on --setup
```

### `--setup-profile <SETUP_PROFILE>`

当前接受的值：`adaptive`。

示例：

```bash
go-on --setup --setup-profile adaptive
```

### `--setup-level <SETUP_LEVEL>`

接受的值：

- `quick`
- `standard`
- `custom`

实用指导：

- `quick`：最小路径，跳过额外的 Agent 提示。
- `standard`：大多数用户的最佳默认值。
- `custom`：暴露更多手动决策。

### `--setup-secrets <SETUP_SECRETS>`

接受的值：

- `env`
- `keyring`
- `auto`

`auto` 也接受 `autodetect`。

### `--apply-recommended`

将 Provider 能力推荐应用到当前 `config.toml` 并退出。

在接入新 Provider 或更改模型组合后使用。

### `--force`

即使目标文件已存在也强制运行 setup。

谨慎使用，尤其是当你精心维护了一个手写的 `config.toml` 时。

## 本地模型注册

### `--add-local-model` 或 `--add-model`

在配置中添加或更新本地模型 Agent 条目。

此标志通常与下面的 `--local-model-*` 选项组合使用。

### `--local-model-name <NAME>`

逻辑 Agent 名称。

### `--local-model-url <URL>`

本地 Provider 的端点 URL。

### `--local-model-type <TYPE>`

Provider 类型。默认意图为 `openai`。

### `--local-model-model <MODEL_ID>`

要存储在配置中的模型标识符。

### `--local-model-api-key-env <ENV_NAME>`

可选的 API 密钥环境变量字段。

### `--local-model-secret-key-env <ENV_NAME>`

可选的密钥环境变量字段。

### `--local-model-register-only`

仅在 `[agents]` 下注册本地模型，而不自动附加到 phase agent 列表。

示例：

```bash
go-on --add-local-model \
  --local-model-name ollama-local \
  --local-model-url http://127.0.0.1:11434/v1 \
  --local-model-type openai \
  --local-model-model qwen2.5-coder \
  --local-model-register-only
```

## Secret 管理

### `--secret <ACTION>`

接受的动作：

- `set`
- `get`
- `delete`
- `list`

### `--secret-name <SECRET_NAME>`

逻辑 Secret 目标的名称。

### `--secret-value <SECRET_VALUE>`

与 `set` 一起使用的 Secret 值。

示例：

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
go-on --secret get --secret-name openai
go-on --secret delete --secret-name openai
```

## 规划与制品检查

### `--action-check <ACTION_CHECK>`

针对 `.goon/` 制品运行操作检查。

帮助中描述的预期值：

- `all`
- `spec`
- `qa`
- `retest`
- `final`

### `--plan-task <PLAN_TASK>`

为复杂任务构建并持久化一个受控的任务规划制品。

当你希望运行时在执行前物化一个持久的规划对象时使用。

## 传输模式选择

### `--protocol-mode <MODE>`

接受的值：

- `adaptive`（推荐默认）
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

推荐用法：

- `adaptive`：当多个客户端可能连接时的最安全默认值；它保留双栈请求路由并从运行时前提条件推导启动传输。
- `acp_stdio`：当编辑器将 `go-on` 作为子进程启动时的最佳选择。
- `acp_http`：当 ACP 兼容客户端需要一个共享的长时间运行后端时的最佳选择。
- `mcp_stdio`：仅当你的客户端明确期望 MCP over stdio 时使用。
- `mcp_http`：当你的客户端需要 OpenAI 兼容的 `/v1` HTTP 端点时的最佳选择。

### `--acp-http-bind <ADDR>`

绑定 HTTP 监听器并暴露：

- `/health`
- `/chat`
- `/chat/stream`

实践中，同一运行时也会暴露 OpenAI 兼容的 `/v1` 端点，用于 Zed 模型提供方风格的集成和运行时探测。

示例：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

## 常用命令配方

最小化 setup：

```bash
go-on --setup --setup-level standard --setup-secrets auto
```

验证然后检查就绪状态：

```bash
go-on --config config.toml --validate-config
go-on --config config.toml --status
```

为 GUI、Zed 和探测启动共享的本地 HTTP 运行时：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

为编辑器启动的集成运行 ACP over stdio：

```bash
go-on --config config.toml --protocol-mode acp_stdio --verbose
```

终端聊天（交互式，类似 Claude Code / Codex）：

```bash
go-on -a
```

## 终端聊天模式（`--chat` / `-a`）

启动交互式终端聊天会话（类似 Claude Code 或 Codex）。

```bash
go-on -a
# 或
go-on --chat
```

如果配置文件在其他路径：

```bash
go-on -c /path/to/config.toml -a
```

### 前置条件

至少需要在 `config.toml` 中配置一个 AI 供应商并拥有有效的 API 密钥。API 密钥自动从系统 keyring 读取（keyring → 环境变量回退）。

### 行为说明

1. 根据已配置的供应商构建智能体注册表。
2. 打开 readline 风格的对话循环。
3. 每条消息发送到第一个可用智能体，支持流式输出。
4. 维护对话历史（上限 1000 条消息）。
5. Ctrl+C 或 `/quit` 优雅退出。

### 内置命令

| 命令 | 说明 |
|---------|------|
| `/quit` 或 `/exit` | 退出聊天模式 |
| `/help` | 显示可用命令 |
| `/clear` | 清除对话历史 |
| `/agents` | 列出已配置的智能体 |

### 自动跳转到设置

如果传入 `--chat` 时未配置任何供应商，将跳过交互式引导直接提示运行 `--setup`：

```bash
go-on -c config.toml -a
# → "未配置 AI 智能体。请先运行 go-on --init 来设置供应商。"
```

## 操作指导

- 在假设传输层故障之前，先使用 `--validate-config`。
- 在打开 GUI 或编辑器插件之前，先使用 `--status`。
- 除非你有具体的客户端契约要求仅 ACP 或仅 MCP 行为，否则使用 `adaptive`。
- 在接入本地 OpenAI 兼容端点时，优先使用 `--add-local-model` 而不是手动编辑配置。