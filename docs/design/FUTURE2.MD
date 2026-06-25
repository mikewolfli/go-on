# FUTURE2 — 服务端 MCP 升级路线图（PostgreSQL + pgvector / 单机 ACP 分轨）

> 目标：在不破坏现有单机能力的前提下，将项目升级为可独立部署的服务端 MCP 版本。
>
> 分轨原则：
> - 单机版：`SQLite3 + ACP`（本地快速、零外部依赖）
> - 服务器版：`PostgreSQL + pgvector + MCP`（多租户、可横向扩展）

---

## 背景与结论

现状（v0.4.1）已具备 ACP 运行时、规则注入、回退与审查门控、内存分层与基础观测能力；但在部署形态上仍偏单机运行。

升级到服务端 MCP 版的核心缺口：

| 领域 | 当前状态 | 目标状态 |
|---|---|---|
| 协议入口 | ACP 已完整 | 新增 MCP 服务端主入口并长期维护 |
| 存储后端 | SQLite3 为主 | PostgreSQL + pgvector 成为服务端默认 |
| 构建产物 | 单一二进制为主 | 本地版/服务端版分离构建与发布 |
| 配置模型 | 单套配置偏本地 | 按 profile（local/server）分层配置 |
| 运维能力 | 单机可用 | 服务端可观测、可迁移、可回滚 |

---

## 目标

| ID | 目标 | 目标文件/模块 |
|----|------|---------------|
| M1 | 建立双轨编译特性：`local-acp-sqlite` 与 `server-mcp-postgres` | `Cargo.toml`、构建脚本 |
| M2 | 抽象统一存储接口 `StateStore` / `VectorStore`，解耦 SQLite 与 PostgreSQL | `src/memory/*`、`src/core/*` |
| M3 | 本地轨保持 `SQLite3 + ACP` 默认行为不变 | `src/acp/*` |
| M4 | 服务端轨默认 `PostgreSQL + pgvector + MCP` | `src/mcp/*`、`src/protocol/*` |
| M5 | 建立 DB Migration（schema + index + vector index） | `migrations/` |
| M6 | 支持会话、审计、摘要、向量检索在 PostgreSQL 端落地 | `src/memory/*`、`src/observability/*` |
| M7 | 配置分层：`config.local.toml` 与 `config.server.toml`，统一校验器 | `src/core/config.rs` |
| M8 | 认证与租户隔离（最小可用）：API key + tenant_id 域隔离 | `src/protocol/*` |
| M9 | 观测与运维：健康检查、连接池指标、慢查询告警 | `src/observability/*` |
| M10 | CI/CD 分轨：本地包与服务端包分别构建、测试、发布 | `.github/workflows/*` |
| M11 | 文档与回滚手册：部署、迁移、故障切换、回退步骤 | `README.md`、运维文档 |
| M12 | 双轨验收：本地与服务端全链路测试全部通过 | `tests/*` |

---

## 执行路线（分阶段）

### P0 架构冻结与接口先行
- [x] 冻结双轨边界：协议层、存储层、运行时入口三条边界
- [x] 定义 `StateStore` / `VectorStore` trait（禁止业务代码直接依赖具体 DB）
- [x] 明确 feature flag 与二进制产物命名

### P1 编译与启动分轨
- [x] 在 `Cargo.toml` 增加 features：`local-acp-sqlite`、`server-mcp-postgres`
- [x] 启动参数增加 `--profile local|server`
- [x] 保证 local 为默认 profile（保持向后兼容）

### P2 PostgreSQL + pgvector 落地
- [x] 新增 PostgreSQL 连接池与仓储实现
- [x] 建立 pgvector 列与索引（HNSW 或 IVF，按规模选择）
- [x] 完成 schema migration 与版本同步

### P3 MCP 服务端强化
- [x] 固化 MCP 初始化、工具注册、调用链追踪
- [x] 对工具执行引入租户域上下文与限流
- [x] 建立服务端级并发与超时基线

### P4 数据迁移与灰度
- [x] 提供 SQLite -> PostgreSQL 迁移工具（幂等）
- [x] 支持 dry-run 报告（记录可迁移/冲突/失败条目）
- [x] 分批灰度：只读验证 -> 双写验证 -> 切流

### P5 CI/CD 与发布
- [x] CI 增加矩阵：`local` 与 `server` 两轨检查
- [x] 产出两个发布包：`go-on-local`、`go-on-server`
- [x] 回归门禁：编译、lint、单测、集成、迁移测试

### P6 可运维与验收
- [x] 增加连接池、查询延迟、向量检索命中率指标
- [x] 完成压测基线与容量建议
- [x] 完成 DoD 验收与发布签字

---

## 配置与编译矩阵

| 维度 | 本地版（Local） | 服务端版（Server） |
|---|---|---|
| Profile | `local` | `server` |
| 协议 | ACP | MCP |
| 数据库 | SQLite3 | PostgreSQL |
| 向量检索 | SQLite 路径（保持现状） | pgvector |
| 目标场景 | 单机开发/桌面使用 | 团队服务/多租户 |
| 默认端口 | 本地进程内/本地端口 | 独立服务端口 + 健康检查 |

---

## 风险与应对

| 风险 | 影响 | 应对 |
|---|---|---|
| 双轨代码分叉失控 | 维护成本飙升 | 强制 trait 抽象 + profile 边界测试 |
| 迁移数据不一致 | 服务结果偏差 | dry-run、校验哈希、分批灰度 |
| pgvector 参数不当 | 检索质量下降 | 建立离线评估集与参数回归 |
| 连接池与慢查询 | 吞吐抖动 | 指标告警 + SQL 诊断基线 |

---

## 完成标准（DoD）

1. 所有阶段清单从 `[ ]` 变为 `[x]`。
2. Local 轨：`SQLite3 + ACP` 兼容现有使用方式，无破坏性变更。
3. Server 轨：`PostgreSQL + pgvector + MCP` 可独立部署并通过集成测试。
4. 存储访问全部经统一 trait，业务层无数据库具体实现耦合。
5. 迁移工具支持 dry-run 与正式执行，且具备回滚说明。
6. CI 分轨门禁全部绿色（构建、lint、测试、迁移验证）。
7. 观测面板可看到数据库、向量检索、MCP 调用关键指标。

---

## 里程碑建议（建议节奏）

- R1（1~2 周）：P0 + P1
- R2（2~3 周）：P2 + P3
- R3（1~2 周）：P4 + P5
- R4（1 周）：P6 + 发布验收
