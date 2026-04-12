# go-on

简体中文 | [English](README.md)

go-on 是一个基于 Rust 的 ACP 运行时（包含 MCP 适配层能力），重点面向智能体编排、运行时安全与可扩展工作流执行。

- 核心运行时版本：0.5.2
- 默认编译配置：`local-acp-sqlite`
- 可选配置特性：`server-mcp-postgres`（当前为能力预留）


## 当前源码结构

所有主要实现代码均位于 `src/` 目录下：

- `src/acp`：ACP 服务器、请求分发、聊天/审查/运行时/后台维护
- `src/agents`：模型供应商适配器与统一代理接口
- `src/core`：配置、初始化、校验、上下文与错误模型
- `src/governance`：运行治理、审查控制、策略约束
- `src/i18n`：多语言运行时与热更新监听
- `src/intelligence`：选择器、验证、强化学习、评测模块
- `src/mcp`：MCP 适配辅助模块
- `src/memory`：缓存、向量与记忆存储
- `src/observability`：遥测、性能、观测模块
- `src/optimization`：成本/速度/可靠性/故障预防/工作流优化
- `src/orchestration`：流程、模式、任务图、路由、工具编排
- `src/protocol`：协议服务与 JSON-RPC 支持

其他目录：

- `test_i18n/`：i18n 集成测试
- `tests/`：集成与场景测试

项目根目录下无 core、agents 等同名目录，所有实现均在 src 下。

## 已实现 RPC 能力面

ACP 请求分发中已实现的主要方法包括：

- 核心：`initialize`、`chat`、`phase`、`shutdown`、`runtime.health`
- MCP 适配：`mcp.initialize`、`mcp.tools.list`、`mcp.tools.call`
- 指标追踪：`metrics.get`、`metrics.prometheus`、`trace.get`、`trace.metrics`、`debug_panel.get`
- 运行控制：`breaker.status`、`breaker.reset`、`cache.clear`、`vector.clear`、`maintenance.gc`、`config.reload`
- 工作流任务：`workflow.confirm`、`workflow.clarify`、`workflow.research`、`workflow.consult`、`workflow.generate`、`workflow.execute`、`task.plan`、`task.execute`
- 学习与调参：`learning.summary`、`autotune.get`、`autotune.status`、`autotune.reset`、`action.check`
- 会话操作：checkpoint 创建/列表/清理与 rollback

## 构建与校验

```bash
cargo build
cargo check
cargo clippy -- -D warnings
cargo test
```

## Setup 与状态检查

执行交互式 setup（含提供商选择 + API Key 配置）：

```bash
cargo run -- --setup --config config.toml
```

向导级别：

```bash
# Quick 快速推荐配置
cargo run -- --setup --setup-level quick --config config.toml

# Standard 标准推荐配置
cargo run -- --setup --setup-level standard --config config.toml

# Custom 完全自定义配置
cargo run -- --setup --setup-level custom --config config.toml
```

setup 过程中可以：

- 选择一个或多个模型提供商
- 选择密钥模式（`env`、`keyring`、`auto`）
- 可选地仅为已选提供商写入系统 keyring

提供商来源：

- setup 向导现在从 `providers.toml` 读取可选 provider 能力。
- 新增 provider 只需在 `providers.toml` 追加一个 `[[providers]]` 条目，无需改 setup 代码。
- setup 与 status 还会读取 `providers.toml` 中的推荐参数字段：
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

一键将当前配置对齐到能力源推荐值：

```bash
cargo run -- --apply-recommended --config config.toml
```

检查已配置 AI 的运行时就绪状态：

```bash
cargo run -- --status --config config.toml
```

该命令会输出整体就绪状态，以及每个已配置 agent 的 ready 标记、端点状态和缺失环境变量。
同时会输出配置完整度评分（0-100）、未配置项和推荐调整项。

当启动时检测到“没有 runtime-ready 的 AI provider”，程序会自动进入引导选择：

- 快速配置 AI
- 进入完整 Wizard
- 无 provider 继续启动

新增本地模型加入接口：

```bash
cargo run -- --add-local-model \
	--local-model-name local_llm \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-type openai \
	--local-model-model qwen2.5-coder \
	--config config.toml

# 只注册到 [agents]，不自动接入 phases
cargo run -- --add-local-model \
	--local-model-name local_shadow \
	--local-model-url http://127.0.0.1:11434/v1 \
	--local-model-register-only \
	--config config.toml
```

## 配置文件（当前）

- 唯一模板：`config.toml.autopilot-adaptive`
- 运行时生效配置：`config.toml`

提供商凭据可使用环境变量名，或使用 `keyring://go-on/openai_compatible_api_key` 这类 keyring 引用。

如需将本地配置重置为最新模板：

```bash
cp config.toml.autopilot-adaptive config.toml
cargo run -- --config config.toml --validate-config
```

## VS Code 插件

- 插件文档： [vscode-addon/README.md](vscode-addon/README.md)
- 当前同步版本：0.4.7

## 路线图

- [FUTURE2](FUTURE2.MD)
- [FUTURE3](FUTURE3.MD)
- [FUTURE4](FUTURE4.MD)
- [FUTURE5](FUTURE5.MD)
- [FUTURE6](FUTURE6.MD)

## 许可证

遵循仓库许可策略。
