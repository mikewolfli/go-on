# go-on

简体中文 | [English](README.md)

go-on 是一个基于 Rust 的 ACP/MCP 运行时，聚焦智能体编排、治理能力与生产安全落地。

## 版本

- 后端 Runtime：0.6.1
- GUI 桌面端：0.6.1
- VS Code 插件：0.6.1
- 默认特性：`local-acp-sqlite`
- 可选特性预留：`server-mcp-postgres`

## 仓库结构

### 核心目录
- `src/`：后端 Runtime 主实现（Rust）
- `GUI/`：Tauri + Vue 桌面控制台
- `vscode-addon/`：VS Code 扩展

### 配置与脚本
- `config/`：配置文件（`config.toml`、`config.production.toml`、`providers.toml`）
- `scripts/`：质量门禁、发布门禁脚本与部署工具
  - `scripts/deploy/nginx/`：入口网关与 TLS 反向代理模板

### 文档
- `docs/`：完整项目文档
  - `docs/blueprints/`：蓝图文档（blue1.md 到 blue34.md）
  - `docs/design/`：设计文档与未来规划
  - `docs/guides/`：实施指南与状态文档
  - `docs/reports/`：项目评估与代码审查报告
- `DOC/`：书籍格式的项目文档

### 测试与开发
- `tests/`：集成测试与测试产物
  - `tests/artifacts/`：测试产物与基准测试结果
  - `tests/requests/`：NDJSON 场景基准与回放输入
- `test_i18n/`：国际化测试套件

### 资源与规则
- `languages/`：运行时多语言资源与 PUA 规则
- `RULES/`：治理与编码规则集
- `contracts/`：编辑器能力矩阵与契约

### 归档与临时文件
- `archive/`：归档的临时文件与日志
  - `archive/temp/`：临时编译输出与日志
  - `archive/logs/`：运行时日志文件

### 后端源模块（`src/`）
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

解释说明：

- `adaptive` 表示双栈协议能力加按请求类型路由，不等于写死某个固定接口。
- `acp_stdio`、`acp_http`、`mcp_stdio`、`mcp_http` 这 4 个固定模式仍严格由显式配置决定。
- 当前启动传输会根据运行前提推导：存在 `acp_http_bind_addr` 时优先提供 HTTP，否则提供 stdio。

## 快速开始

### 1）构建与测试

```bash
cargo build
cargo check --all-targets
cargo test --all-targets
```

### 2）首次初始化

```bash
cargo run -- --init --config config/config.toml
cargo run -- --check --config config/config.toml
```

可选初始化级别：

```bash
cargo run -- --init --setup-level quick --config config/config.toml
cargo run -- --init --setup-level standard --config config/config.toml
cargo run -- --init --setup-level custom --config config/config.toml
```

### 3）启动 Runtime

- Linux/macOS：`./scripts/start-go-on.sh`
- Windows：`scripts/start-go-on.bat`

默认健康检查地址：

- `http://127.0.0.1:8090/health`

## 生产基线

生产模板：

- `config/config.production.toml`

当前默认包含：

- 默认环回地址监听
- 入口鉴权与入口限流配置
- 严格模式 fail-fast（`runtime.production_strict = true`）
- OTEL 相关运行时配置

### API 密钥配置（生产模式）

使用 `config/config.production.toml` 启动时，入口鉴权默认已开启。
启动前必须设置以下环境变量：

```bash
# Linux / macOS
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
./scripts/start-go-on.sh
```

```powershell
# Windows PowerShell
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
scripts\start-go-on.bat
```

所有 RPC 请求需在 `Authorization` 头中携带密钥：

```
Authorization: Bearer your-secret-key-here
```

若该变量缺失或为空，服务将以错误码 `-32003`（`AuthRequired`）拒绝所有请求。

> **安全提示**：绝不要将密钥写入任何配置文件或提交到版本控制。请使用环境变量、密钥管理器或 keyring 注入方式。

入口网关与 TLS 模板：

- `scripts/deploy/nginx/go-on.conf`
- `scripts/deploy/nginx/README.md`

发布就绪清单：

- `docs/RELEASE_READINESS.md`

## 场景与门禁工具

`tests/requests/` 已包含 runtime health、governance、cost、harness、security、release drill 等主链场景。

门禁脚本（位于 `scripts/`）：

- `scripts/run-quality-gate.sh`
- `scripts/run-quality-gate.ps1`
- `scripts/run-release-readiness-gate.sh`
- `scripts/run-release-readiness-gate.ps1`
- `scripts/test_ci.sh`

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

完整文档已组织在 `docs/` 目录中：

### 蓝图文档（`docs/blueprints/`）
- `blue1.md` 到 `blue34.md` - 实施蓝图与进展总账

### 设计文档（`docs/design/`）
- `design.md` - 系统设计概述
- `FUTURE*.md` - 未来规划文档
- `future-last.md` - 完整的未来改进计划

### 指南文档（`docs/guides/`）
- `README-PUA-UNIVERSAL.md` - PUA 通用实施指南
- `MCP_LAYER.md` - MCP 层实施详情
- `GO-ON_PUA_IMPLEMENTATION.md` - PUA 实施细节
- `IMPLEMENTATION_STATUS.md` - 当前实施状态
- `MIGRATION_STATUS.md` - 迁移状态与计划
- `PHASE_10_COMPLETE_IMPLEMENTATION.md` - 第 10 阶段实施详情

### 报告文档（`docs/reports/`）
- `PROJECT_EVALUATION_REPORT.md` - 完整的项目评估报告
- `CODE_REVIEW_FINAL_REPORT.md` - 代码审查发现
- `PHASE_10_DELIVERY_REPORT.md` - 第 10 阶段交付报告
- `MIGRATION_FINAL_SUMMARY.md` - 迁移总结

### 其他关键文档
- `docs/RELEASE_READINESS.md` - 发布就绪清单
- `docs/RULES.md` - 项目规则与指南
- `docs/DEVELOPMENT_RULES.md` - 开发规则与标准

## 许可证

本项目按 MIT 或 BSD 许可（可任选其一）。
