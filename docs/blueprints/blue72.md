# BLUE72 — 新功能候选充分必要性验证报告

> **分析日期**: 2026-08-09
> **分析原则**: `docs/blueprints/principle.md`(§8 禁占位 / §13 禁假修复 / §16 验证证据 / §21 独立验证)
> **分析方法**: 三轮全项目超级深度+广度扫描(技能市场、MCP、多实例、CLI、记忆、图谱、GUI、HTTP 面、进化审批、F-GAP-49 逐一核查代码事实,非文档声明)
> **结论先行**: 上一轮口头建议的 11 项新功能中,**2 项为误判(已实现)、1 项重复既有蓝图、2 项收益低**,实际充分必要的高价值功能仅 **4 项**,其中 **1 项是 principle §8 明令必须补齐的占位违规**。

---

## 1. 候选功能逐一验证结论(基于代码事实)

### ❌ 误判 1:技能市场激活(原建议 #1)——**已实现,不必要**

**证据**(§21 独立验证,非上轮印象):
- `src/orchestration/skill_market.rs`(1648 行)已实现完整链路:
  - `refresh()` 三级拉取:远程 registry API → GitHub `goon-skill-index.json` → 内置样本 fallback
  - `fetch_github_index()` / `fetch_remote_skills()`(HTTP 客户端,15s 超时)
  - `install_skill` / `uninstall_skill` / `set_enabled` / `search_skills` / `list_skills_by_tag`
- CLI 子命令完整:`go-on skill list/search/install/import/enable/disable/remove/info/refresh`
- 协议面 10 个 `skill.*` ACP 方法全部注册(`src/acp/impl/request/protocol.rs`)
- 蓝图 `skill-market.md` 描述的架构**已全部落地**

**判定**: 链路闭合,无缺口。原建议基于上轮快速 grep 未看到 `fetch_github_index` 而误判。

---

### ❌ 误判 2:CLI 子命令体系(原建议 #5)——**已实现,不必要**

**证据**:
- `src/main/cli.rs` 已有 `CliCommand` 子命令树:`Init` / `Status` / `Diagnose` / `Skill{…}`(9 个子命令)/ `Hub{port}`(feature-gated)
- 另有全局 flag 体系(`--config` / `--setup` / `--add-local-model` / `--diagnose` 等)

**判定**: 已完整,无缺口。原建议误判("无子命令树"说法错误)。

---

### ❌ 重复既有蓝图:F-GAP-49 预留项激活(原建议 #2)——**文档与代码已脱节**

**证据**:
- `f-gap-49.md` 声称 300 项预留、42 项已激活、大量 `#[allow(dead_code)]`
- 但**代码实测**:全项目 `#[allow(dead_code)]` 仅剩 4 处且全部合法(skill_market 的 serde 反序列化字段、self_evolution 的字符串模板),**无 F-GAP-49 表所述的预留项**
- 文档计数也自相矛盾:实测 ✅ 仅 3 项、⏳ 302 项,与表头"42 项已激活"不符

**判定**: 该"新功能"方向是**文档幻觉**——预留项早已在历轮清理中删除。真正必要的是**修文档**(同步真实状态),不是"激活预留项"。

---

### ⚠️ 收益低:Web 控制台(原建议 #8)——**已有 GUI+vscode-addon,新增价值有限**

**证据**:
- `gui/`(Tauri 2,egui)已有完整视图:chat / monitor / skills / workflow / settings / security / autotune / providers / risk_decision / setup
- `vscode-addon` 已注册 5 个 WebviewView + 20+ 命令 + status tree
- HTTP 面已有 `/health` `/metrics` `/v1/state/events`(SSE 实时推送)

**判定**: 本地 GUI 与 IDE 插件已覆盖运维场景;新增 HTTP Web 控制台属重复面,收益低。

---

### ⚠️ 收益低:自动 benchmark 回归(原建议 #9)——**有价值但非功能缺口**

**证据**: `benches/` 已有 3 个 criterion benchmark(acp_bench / error_code_bench / pipeline_bench),`run-quality-gate.sh` 已存在。

**判定**: 这是"流程增强"(脚本封装),非功能缺口;可并入 §3 的运维配套,不作为独立新功能。

---

### 🟡 部分已有:模型路由 A/B 实验(原建议 #10)——**框架在,缺实验层**

**证据**:
- `adaptive_selector.rs` 已实现 UCB + 上下文特征(time-of-day bucket / task_type / latency_tier)+ `exploration_bias` 可调
- **缺**: 分流比例配置、对照组隔离、实验报告输出——全项目无 `experiment/variant/treatment` 概念

**判定**: 中等收益,属"增量增强"而非新功能。**不列入 P0**(与自学习主线弱相关,且 UCB 本身已具备探索-利用,显式 A/B 层收益存疑)。

---

### ✅ 真实缺口 1:EvolutionLoop 人工审批(blue71 P0 遗留)

**证据**(principle §8/§13 **占位违规,必须修**):
- `src/orchestration/self_evolution/evolution_loop/mod.rs:562-568`:
  ```rust
  validate::ApprovalMode::RequireHuman => {
      info!("waiting for human approval");
      Err(EvolutionLoopError::Rejected(
          "Human approval not implemented yet — rejecting".to_string(),
      ))
  }
  ```
- 同文件 `RequireApproval` 分支同样拒绝("no approval subsystem wired")
- blue71(§4.3、L210、L1335)已明确标记此为 go-on 唯一自进化缺口,竞品(Codex/Harness)均不具备自进化,此功能是**差异化核心**

**判定**: **P0 必做**。补接线:审批事件 → 通知 GUI/vscode-addon → 用户 approve/deny → watch/oneshot 事件驱动恢复(而非轮询)。

---

### ✅ 真实缺口 2:MCP 客户端(原建议 #3)——**服务端完备,客户端为零**

**证据**:
- 服务端完备:`mcp_stdio` / `mcp_http` + 内部 `mcp.*` handler(10+ 方法)全注册
- **客户端零实现**:全项目 grep `McpClient` / `mcp_client` / `connect.*mcp` **0 匹配**
- `protocol_bus` 只跟踪 5 种传输的延迟/健康,不消费外部 MCP

**判定**: **P1 高收益**。agent 无法调用外部 MCP 工具服务器(如 Playwright、GitHub MCP),工具生态受限。实现 `mcp_client`(连接 → 发现 tools → 映射进 `ToolRegistry` 热路径)。

---

### ✅ 真实缺口 3:记忆导出/导入快照(原建议 #6)——**完全空白**

**证据**:
- 三层持久化(hot/warm/cold)+ 会话恢复已闭环(第 4 轮)
- **无 `export/import/snapshot` 任何实现**;全项目 grep 0 匹配
- 蓝图层 `blue-summary-dp.md` 亦无此方向

**判定**: **P1**。跨实例迁移、备份、冷启动恢复依赖此能力;与 DMB 增量同步互补(批量迁移 vs 实时同步)。

---

### 🟡 中等:知识图谱可视化(原建议 #7)——**有数据,无观测面**

**证据**:
- `evolution_graph.rs`(能力演化图)+ `DiscoveryCenter`(解决方案库)+ `governance.status` 已存在
- **缺**: 图谱查询/导出端点,无可视化消费方

**判定**: **P2**。收益取决于 GUI 消费;可并入"图谱摘要端点 + GUI 视图",非核心链路。

---

### 🟡 中等:多实例节点发现(原建议 #4)——**基础设施在,缺自动注册**

**证据**:
- Hub daemon 完整(handshake/status/store/retrieve/memory.ingest + discovery file + Bearer 鉴权),已接 CLI `go-on hub`
- DMB 有 `register_peer`(手动)+ 增量同步 + 并行推送
- **缺**: 自动节点发现/心跳注册(现需手工 `register_peer`)

**判定**: **P2**。多实例部署是明确方向(蓝图/代码均预留),但当前单机+Hub 模式可用;自动发现属"部署体验增强"。

---

### 🟡 中等:技能自动生成闭环(原建议 #11)——**已部分实现**

**证据**:
- `create_skill_from_prompt`(`registry.rs:527`)已存在 + 自动持久化(`set_persistence_path`)+ 磁盘重建(`load_prompt_skills_from_disk`)
- `SelfEvolutionAgent` 已能 analyze_code / generate_patch / fix_compile_errors

**判定**: "发现重复工具→自动生成 skill"是增量方向,核心原语已在;**不列入新功能清单**,归入未来增强。

---

## 2. 验证矩阵(§16/§21 证据)

| 扫描轮次 | 范围 | 方法 | 结论 |
|---------|------|------|------|
| 第 1 轮 | 技能市场 / MCP / DMB | grep + 关键函数通读(300-380 行) | 技能市场已实现;MCP 客户端为 0;DMB 手动注册 |
| 第 2 轮 | CLI / 记忆 / 图谱 / 自进化 / F-GAP | 子命令枚举通读 + grep 导出/实验/A-B | CLI 完整;导出/图谱/A-B 无实现;F-GAP 文档脱节 |
| 第 3 轮 | HTTP 面 / GUI / vscode / blue71 交叉 | 路由枚举 + 视图枚举 + blue71 P0 对照 | Web 控制台重复;RequireHuman 占位实锤 |

**误判修正记录**(§21 独立验证的价值):原 11 项建议中,技能市场与 CLI 两项经代码验证为**已实现**——上轮快速印象不可靠,本轮全部以代码事实为准。

---

## 3. 最终建议:真正必要的新功能清单(修正后)

| 优先级 | 功能 | 依据 | 工作量估计 |
|:------:|------|------|-----------|
| **P0** | EvolutionLoop 人工审批补接线 | principle §8/§13 占位违规;blue71 差异化核心;审批事件 → GUI/IDE → 事件驱动恢复 | 中(跨 orchestration + GUI + 协议) |
| **P1** | MCP 客户端 | 服务端完备/客户端为零;工具生态扩展 | 中(新模块 + ToolRegistry 映射) |
| **P1** | 记忆导出/导入快照 | 完全空白;跨实例迁移/备份/冷启动 | 小-中(持久层扩展 + 2 个 ACP 方法) |
| **P2** | 知识图谱观测端点 | 有数据无消费面 | 小 |
| **P2** | 多实例自动节点发现 | 基础设施在,缺自动注册 | 中(心跳 + discovery 扩展) |
| **文档** | F-GAP-49 状态同步(302 ⏳ → 实际已删) | 文档与代码脱节(§18 文档欺骗风险) | 小 |

**明确排除**:技能市场激活(已实现)、CLI 子命令(已实现)、Web 控制台(重复面)、自动 benchmark(流程非功能)、A/B 实验(UCB 已含探索)、技能自动生成(已部分实现)。

---

## 4. 执行建议

1. **P0 人工审批**最先做:它违反 principle §8/§13 的"占位"禁令,是唯一**必须**补齐的项,且是 go-on 相对竞品的差异化能力(blue71 结论)。
2. **P1 两项**(MCP 客户端、记忆快照)收益明确、边界清晰,可并行实施。
3. 实施时遵循 principle:每条修复附带端到端行为验证(§16)、不引入新占位(§14)、审批链路需真实用户可操作(§13)。
4. F-GAP-49 文档修正在实施周期内顺带完成(§18)。

> **验证声明**:本报告所有结论基于 2026-08-09 对 `src/` 的实际代码扫描(grep + 函数通读),非文档声明;涉及"已实现"的判断均有文件:行号佐证。
