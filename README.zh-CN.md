# go-on

简体中文 | [English](README.md)

go-on 是一个基于 Rust 的 ACP/MCP 运行时，聚焦智能体编排、治理能力与生产安全落地。

## 版本

- 后端 Runtime：0.6.1
- GUI 桌面端：0.6.1
- VS Code 插件：0.6.1
- 默认特性：`local-acp-sqlite`
- 可选特性预留：`server-mcp-postgres`

## 当前仓库结构

顶层关键目录：

- `src/`：后端 Runtime 主实现
- `GUI/`：Tauri + Vue 桌面控制台
- `vscode-addon/`：VS Code 扩展
- `requests/`：NDJSON 场景基准与回放输入
- `scripts/`：质量门禁与发布门禁脚本
- `deploy/nginx/`：入口网关与 TLS 反向代理模板
- `tests/`、`test_i18n/`：集成测试与 i18n 测试
- `languages/`：运行时多语言资源
- `RULES/`：治理与编码规则集

后端 `src/` 模块：

- `acp`：ACP 服务、请求路由、workflow/task/chat 主链
- `agents`：模型供应商适配与统一契约
- `core`：配置、初始化、就绪性检查、错误模型
- `governance`：策略/规则治理与审计支持
- `intelligence`：选择器、强化学习、质量模型
- `optimization`：成本/速度/可靠性/故障预防
- `orchestration`：流程、模式、路由、工具编排
- `observability`：指标/追踪/性能观测
- `memory`：缓存与向量存储
- `protocol`：协议服务与 JSON-RPC 支撑
- `mcp`、`i18n`：MCP 适配辅助与语言运行时

## 协议模式

`[protocol].mode` 支持 5 种：

- `adaptive`（推荐默认）
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

示例：

```toml
[protocol]
mode = "adaptive"
```

## 快速开始

### 1）构建与测试

```bash
cargo build
cargo check --all-targets
cargo test --all-targets
```

### 2）首次初始化

```bash
cargo run -- --init --config config.toml
cargo run -- --check --config config.toml
```

可选初始化级别：

```bash
cargo run -- --init --setup-level quick --config config.toml
cargo run -- --init --setup-level standard --config config.toml
cargo run -- --init --setup-level custom --config config.toml
```

### 3）启动 Runtime

- Linux/macOS：`./start-go-on.sh`
- Windows：`start-go-on.bat`

默认健康检查地址：

- `http://127.0.0.1:8090/health`

## 生产基线

生产模板：

- `config.production.toml`

当前默认包含：

- 默认环回地址监听
- 入口鉴权与入口限流配置
- 严格模式 fail-fast（`runtime.production_strict = true`）
- OTEL 相关运行时配置

入口网关与 TLS 模板：

- `deploy/nginx/go-on.conf`
- `deploy/nginx/README.md`

发布就绪清单：

- `RELEASE_READINESS.md`

## 场景与门禁工具

`requests/` 已包含 runtime health、governance、cost、harness、security、release drill 等主链场景。

门禁脚本：

- `scripts/run-quality-gate.sh`
- `scripts/run-quality-gate.ps1`
- `scripts/run-release-readiness-gate.sh`
- `scripts/run-release-readiness-gate.ps1`
- `test_ci.sh`

## 三端协同组件

- GUI 文档：`GUI/README.md`
- VS Code 插件文档：`vscode-addon/README.md`

二者已对齐后端 RPC 能力、治理状态与健康探针语义。

## 常用 RPC 能力分组

当前主链代表方法：

- 核心运行：`initialize`、`shutdown`、`runtime.health`、`runtime.stability`、`config.reload`
- 安全治理：`governance.status`、`governance.plan.get`、`governance.plan.update`、`governance.audit.recent`、`security.baseline`
- 可观测：`metrics.get`、`metrics.prometheus`、`trace.get`、`trace.metrics`、`observability.alerts`、`health.probes`
- 稳定性：`breaker.status`、`breaker.reset`、`breaker.recovery`、`maintenance.gc`
- 工作流任务：`workflow.execute`、`task.plan`、`task.execute`
- 学习与智能：`learning.summary`、`learning.replay`、`learning.guardrail`、`selector.status`、`knowledge.distill`、`rl.alignment.offline_eval`、`hardness.status`
- 优化与治理：`cost.status`、`config.baseline`、`error.contract`、`build.repro`、`data.lifecycle`、`harness.status`、`optimization.peak`、`quality.baseline`

## 相关文档

- `blue15.md`（实施进展总账）
- `README-PUA-UNIVERSAL.md`
- `MCP_LAYER.md`
- `GO-ON_PUA_IMPLEMENTATION.md`

## 许可证

本项目按 MIT 或 BSD 许可（可任选其一）。
