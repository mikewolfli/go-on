# BLUE-FUTURE — 能力储备与未来激活登记(设计决策记录)

> **创建**: 2026-08-09
> **原则依据**: `docs/blueprints/principle.md` §10(收敛不扩张)、§11(无意义 dead code)、§14(不制造无消费方的半成品)
> **用途**: 登记"已实现但未激活"的能力储备与"已回滚/已评估"的决策,避免重复开发与误判。代码有测试覆盖即视为储备资产;不因储备而铺协议/CLI/GUI 面。

---

## 1. 记忆快照能力(export_snapshot / import_snapshot)

### 状态:**能力储备,不激活 ACP/CLI 入口**

- **位置**: `src/memory/memory_persistence.rs` — `export_snapshot()` / `import_snapshot()`(gzip NDJSON 全 tier 快照,含 roundtrip 测试 `test_export_import_snapshot_roundtrip`)
- **背景**: 第 7 轮曾暴露为 `memory.export` / `memory.import` ACP 方法,后经收敛审计(BLUE72 A 方案)回滚 ACP 面,保留能力方法。
- **决策理由**(2026-08-09 评估):
  - 重启恢复:hot/warm/cold 三层持久化 + 启动 auto_migrate 已天然覆盖,`session/load` 从 warm 检索,重启不丢记忆
  - 跨实例实时同步:已有 DMB(增量同步 + 并行推送)
  - 快照独特价值仅剩:**显式备份点 / 整库搬迁 / 离线归档**——低频场景
- **激活触发条件**(满足任一再铺入口,预估 80 行 CLI 命令,不加 ACP 面):
  - 用户明确需要定期备份/归档记忆
  - 多实例迁移落地(换机器/迁移环境成为常规操作)
  - 出现"记忆丢失"类事故,验证需要恢复路径

---

## 2. MCP 客户端核心(mcp::client)

### 状态:**已激活最小面,能力储备**

- **位置**: `src/mcp/client.rs`(stdio/http 双传输 + registry,588 行)+ `src/acp/impl/request/mcp_client_pack.rs`(connect/list/call 3 方法)+ `examples/echo_mcp.rs`(真实端到端测试)
- **已接线**: `mcp.client.connect` / `mcp.client.list`(返回工具详情)/ `mcp.client.call`;list 真实消费 `list_tools`,call 失败自动 unregister(无死面)
- **决策理由**: MCP 客户端是行业方向(Agent 调用外部工具服务器),go-on 长期是纯 MCP 服务端;保留为能力储备有真实生态价值
- **未来增强(未做,需真实需求触发)**:
  - 配置驱动自动连接(`[mcp.servers]` + 启动连接)
  - 接入 ToolRegistry 供 agent 直接调用(需 Tool trait 支持动态名,或静态 mcp_call 桥——后者曾实现又回滚,见 §3)

---

## 3. 已回滚项(勿重复开发)

| 项 | 回滚原因 | 证据 |
|----|---------|------|
| **审批子系统**(ApprovalBroker + evolution.approval.* + CLI Evolution + state-sync ApprovalUpdated) | 生产硬编码 AutoApproval,RequireHuman 无 config 切换入口;修复的占位在生产不可达 | log-20260809-2.md §6 |
| **图谱端点 evolution.graph** | 独立方法面不必要;已合并进 governance.status 的 `evolution_graph` profile(真实读数据) | log-20260809-2.md §6 |
| **记忆快照 ACP 面**(memory.export/import) | 无入口死面;能力方法保留(见 §1) | log-20260809-2.md §6 |
| **mcp_call 工具**(ToolRegistry 静态桥) | 依赖手工 connect 才可用,属死面;client 核心保留(见 §2) | log-20260809-2.md §6 |
| **mcp.client.disconnect/tools** | 冗余,保留 connect/list/call 最小面 | log-20260809-2.md §6 |

---

## 4. 审计方法论沉淀(2026-08-09)

blue71 之后多轮新增经收敛审计判定:多数为"有 ACP 接线但无生产消费方"的半成品,违反 §11/§14。
**后续新增功能的必要判定标准**(写入惯例):
1. 该功能是否有**真实生产调用方**或**明确触发条件**?
2. 若需用户手动触发,是否已提供 CLI/GUI 入口(而非仅 ACP 方法)?
3. 与已有机制(持久化/DMB/内置工具)是否**重叠**?
4. 收敛后默认**不扩张**,除非满足以上三条。

---

## 5. 变更记录

| 日期 | 变更 |
|------|------|
| 2026-08-09 | 创建;登记记忆快照储备决策 + MCP 客户端状态 + 回滚清单 + 审计方法论 |
