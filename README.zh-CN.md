# go-on

简体中文 | [English](README.md)

go-on 是一个基于 Rust 的 ACP 运行时（包含 MCP 适配层能力），重点面向智能体编排、运行时安全与可扩展工作流执行。

## 版本信息

- 核心运行时版本：0.4.7
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

## 配置文件（当前）

- 唯一模板：`config.toml.autopilot-adaptive`
- 运行时生效配置：`config.toml`

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
