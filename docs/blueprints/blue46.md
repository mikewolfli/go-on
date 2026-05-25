# BLUE46 — go-on 全方位深度评估与就绪蓝图

> **评估日期**: 2026-05-25  
> **项目**: go-on v1.0.0 — Rust-based ACP/MCP Agent Runtime  
> **评估轮次**: 第四轮（继承 BLUE45 满分基线，重新深度审计）  
> **核心规则**: 同 BLUE43.md — 5协议全链路闭合、3 profile全链路、英文注释、i18n全覆盖、零警告、三端一致、完整闭环

---

## 0. 评估方法论

BLUE45 完成了34项功能改进，宣称所有25个维度达到100分。然而满分基线并不等于"钢铁侠战衣级"可用性。BLUE46采用三层审计：

| 层级 | 方法 | 输出 |
|:-----|:-----|:-----|
| **广度扫描** | 遍历全部17个src模块、15个测试文件、gui/、vscode-addon/ | 模块清单与质量分 |
| **深度审计** | 逐文件代码走读，检查架构质量、代码气味、死代码、集成状态 | 具体问题与改进项 |
| **语义检验** | 交叉验证功能声明vs实际可执行代码路径 | 虚标功能标记 |

---

## 一、综合评分

### 1.1 四轮总分

```mermaid
graph TD
    A:::accent0["R1 架构设计<br/>78/100"] --> E["BLUE46 综合分<br/>67.1/100<br/>★★★☆☆"]
    B:::accent1["R2 执行运行<br/>65/100"] --> E
    C:::accent2["R3 能力集成<br/>72/100"] --> E
    D:::accent3["R4 压测推演<br/>60/100"] --> E
```

### 1.2 各维度详细得分

| 维度 | 权重 | 得分 | 加权 | 评级 | 核心问题 |
|:-----|:---:|:----:|:----:|:----:|:-----|
| **总线设计正交性** | 8% | 65 | 5.20 | ★★★☆☆ | main.rs 为2,378行god-file；14-Bus在代码中无结构性体现 |
| **F-GAP 覆盖度** | 8% | 72 | 5.76 | ★★★☆☆ | BrainLoop为孤儿模块，零集成；DAG为假实现；元认知未被消费 |
| **模块化与接口设计** | 6% | 55 | 3.30 | ★★☆☆☆ | mode.rs 5×重复代码；OnceLock全局单例破坏可测试性 |
| **扩展性** | 6% | 60 | 3.60 | ★★★☆☆ | PluginRegistry存在但未经入；hot_reload和schema_version为死代码 |
| **配置管理** | 4% | 65 | 2.60 | ★★★☆☆ | hot_reload/schema_version完全未集成；governance字段在config中存在但schema缺失 |
| **文档化程度** | 4% | 55 | 2.20 | ★★☆☆☆ | DOC声称WebSocket/OAuth/SDK但全部不存在；blueprints与DOC割裂 |
| **路由与调度速度** | 6% | 68 | 4.08 | ★★★☆☆ | Scheduler dequeue为O(n log n)；aging在热路径重建堆 |
| **工具执行速度** | 5% | 60 | 3.00 | ★★★☆☆ | mode.rs每次chat创建新的tokio runtime；DAG执行无真实依赖图 |
| **流式响应速度** | 4% | 78 | 3.12 | ★★★★☆ | SSE优化器完备但未被主路径引用；brotli压缩未启用 |
| **缓存效率** | 4% | 72 | 2.88 | ★★★☆☆ | CacheWarmingEngine存在但未被集成；full_auto.rs缓存耦合侵入 |
| **并行执行** | 4% | 50 | 2.00 | ★★☆☆☆ | "DAG"为伪实现——纯parallel fan-out，无拓扑排序、无依赖边 |
| **模式切换平滑度** | 3% | 70 | 2.10 | ★★★☆☆ | 5模式80%重复代码；每次创建新Tokio runtime |
| **Brain Loop 自适应** | 3% | 45 | 1.35 | ★★☆☆☆ | 被动状态追踪器非执行器；零集成；off-by-one bug |
| **会话管理** | 3% | 75 | 2.25 | ★★★☆☆ | SessionContextManager完备但未被main路径调用 |
| **错误恢复流畅度** | 3% | 65 | 1.95 | ★★★☆☆ | recovery使用Levenshtein字符串相似度匹配策略——脆弱 |
| **幂等性设计** | 2% | 78 | 1.56 | ★★★★☆ | IdempotencyStore + WAL 机制健全 |
| **事务回滚** | 2% | 72 | 1.44 | ★★★☆☆ | DistributedTx(2PC)存在但Coordinator从未被实例化 |
| **原子性/隔离性/持久性** | 2% | 75 | 1.50 | ★★★☆☆ | ToolLockManager具备但未被工具执行路径引用 |
| **多模型供应商覆盖** | 4% | 85 | 3.40 | ★★★★☆ | 覆盖广泛，但Gemini functionCall解析缺失 |
| **动态模型选择** | 3% | 72 | 2.16 | ★★★☆☆ | 模型成本/延迟表硬编码，含已废弃模型(gpt-3.5-turbo) |
| **Skill 抽象与发现** | 3% | 73 | 2.19 | ★★★☆☆ | SkillMarket完备但未集成；skill_discovery.rs仅为entry定义 |
| **Function Call 原生支持** | 3% | 75 | 2.25 | ★★★★☆ | OpenAI/Anthropic优秀；Gemini有严重bug |
| **工具数量与多样性** | 2% | 80 | 1.60 | ★★★★☆ | 16工具+流水线完备，但ToolLockManager/ToolRecommender未集成 |
| **极限场景表现** | 4% | 55 | 2.20 | ★★☆☆☆ | 混沌引擎完备但未在CI运行；无压测脚本 |
| **问题解决能力** | 4% | 58 | 2.32 | ★★★☆☆ | DiagnosticFeedback完备但未被BrainLoop消费 |
| ────────────────── | ─── | ─── | ──── | ──── |
| **BLUE46 加权总计** | **100%** | — | **67.1** | **★★★☆☆** |

---

## 二、评级标准

| 分数区间 | 评级 | 含义 |
|:--------|:----:|:-----|
| 90-100 | ★★★★★ | 卓越，生产级 |
| 80-89 | ★★★★☆ | 优秀，少量改进即可生产 |
| 70-79 | ★★★☆☆ | 良好，存在明显短板需补齐 |
| 60-69 | ★★☆☆☆ | 基础可用，需重大改进 |
| <60 | ★☆☆☆☆ | 不可用于生产 |

**BLUE46 结论: 67.1/100 ★★★☆☆** — 功能骨架丰富（34项新功能），但大量模块处于"孤岛"状态：已实现但未被主执行路径集成。系统在概念上完备，在实际可执行路径上存在显著差距。

---

## 三、核心差距矩阵

### 🔴 RED — 阻塞性缺陷（必须修复才能达到生产级）

#### GAP-46-01（P0）: main.rs God-File 重构

**现状**: `main.rs` 2,378行，混合了CLI解析、遥测初始化、i18n、密钥管理、配置加载/校验、agent onboarding(交互式终端)、缓存/向量/autotune初始化、任务计划持久化、health checks、协议模式分发、5种传输模式服务器启动。一个模块承担8+职责。

**影响**: 变更风险极高；新功能接入main.rs每次都引入回归风险；测试难以隔离。

**实施**:
1. 提取 `src/core/bootstrap.rs` — 启动初始化流程（遥测、i18n、缓存、health）
2. 提取 `src/core/onboarding.rs` — agent readiness onboarding
3. 提取 `src/acp/transport_factory.rs` — 5种协议模式统一构造（消除重复的 `new_acp_server()`）
4. main.rs缩减至<500行，仅保留CLI解析和dispatch

**验收**:
- main.rs < 600行
- transport模块消除4次重复 `new_acp_server()` 调用
- 所有现有集成测试通过

---

#### GAP-46-02（P0）: DAG执行器重写

**现状**: `dag_driver.rs` (352行)名为DAG实为并行fan-out。`build_tool_execution_dag()`创建了`branch_id`但丢弃不用。无拓扑排序、无依赖边、无同步点。所有工具被`tokio::spawn`后等`join_all`。`branch_count`和`join_count`为硬编码虚构值。

**影响**: 任何依赖"工具B需要工具A的输出"的场景会得到错误结果。多工具工作流不可靠。

**实施**:
1. 重命名 `dag_driver.rs` → `parallel_tool_executor.rs`（保留，用于简单并行）
2. 新建 `src/orchestration/dag_executor.rs` — 真实DAG执行器：
   - 对接 `planner_execution_graph.rs` 和 `execution_graph.rs`
   - 实现拓扑排序 + 依赖等待 + 并行组识别
   - 保留节点输出注入后续节点输入
   - 支持超时和部分失败的隔离传播

**验收**:
- 多步依赖工作流（tool B depends on tool A output）产生正确结果
- governance payload暴露真实DAG宽度/深度
- 3个复杂度层级的差异化DAG产出

---

#### GAP-46-03（P0）: Gemini Function Call 修复

**现状**: `gemini.rs` 流式处理器仅提取 `candidates[0].content.parts[0].text`。当Gemini返回 `functionCall` 响应（使用 `functionCall.name` + `functionCall.args` 字段），代理静默忽略。所有Gemini模型声明支持 `tools` 能力但功能不可用。

**影响**: 使用Gemini的function calling场景全部失败且无错误提示。

**实施**:
1. 在流式SSE handler中添加 `functionCall` 和 `functionResponse` 路径
2. 映射到 `build_tool_call_token()` 内部格式
3. 当 `finishReason` 为 `SAFETY` 或 `RECITATION` 时记录警告

**验收**:
- Gemini + tools场景下正确提取 `__tool_call__:name:args`
- 新增测试覆盖functionCall流式解析

---

#### GAP-46-04（P0）: 死代码集成 — hot_reload + schema_version

**现状**: `hot_reload.rs` (204行)实现了WatchDog+ReloadObserver+原子配置交换，但从未在 `main.rs` 或 `AppConfig::load()` 中被调用。`schema_version.rs` (236行)实现了SchemaManager+迁移图+版本验证，但从未在生产配置加载中使用。config文件无一包含`schema_version`字段。两个模块合计440行死代码。

**影响**: 配置变更需重启。无配置版本迁移能力。

**实施**:
1. 在 `main.rs` 启动流程中接入 `WatchDog::start()`
2. 在 `AppConfig::load()` 中接入 `SchemaManager::validate_version()`
3. 为4个config文件添加 `schema_version = "1.0.0"` 字段

**验收**:
- `config.toml` 修改后被检测并热重载
- 配置缺少schema_version时产生warning
- 跨版本配置迁移路径可测试

---

### 🟡 YELLOW — 架构债务（应修复以达到优秀级）

#### GAP-46-05（P1）: mode.rs 消除5×重复代码

**现状**: `AskModeRuntime`, `EditModeRuntime`, `AgentModeRuntime`, `FullAutoModeRuntime`, `SafeGuardModeRuntime` 共享80%代码结构。每个创建新Tokio runtime。`execute_agent_chat()` 和 `execute_agent_run_task()` 每次调用 `Runtime::new()`。

**实施**:
1. 创建 `BaseModeRuntime` 模板结构 + 策略回调
2. 5个模式缩减至`allowed_tools()`, `max_tool_calls()`, `user_approval_required()`, risk策略
3. 复用单一Tokio runtime替代每次调用创建

**验收**:
- mode.rs < 400行
- 不再每次chat创建新runtime
- 5个模式行为一致性测试通过

---

#### GAP-46-06（P1）: Orchestrator 全局单例消除

**现状**: `orchestrator.rs` 使用 `OnceLock<LivePerformanceFeed>` 和 `OnceLock<HotFailover>` 全局单例。测试顺序依赖导致不可隔离执行。

**实施**:
1. 将LivePerformanceFeed和HotFailover移至 `AppContext` 注入模式
2. 移除全局OnceLock
3. 测试使用独立实例

**验收**:
- `orchestrator.rs` 无OnceLock
- 测试可并行/独立运行

---

#### GAP-46-07（P1）: BrainLoop 集成与修复

**现状**: `brain_loop.rs` (948行)为零集成孤儿模块。被动状态追踪器非执行器。off-by-one bug (`max_iterations=3` 实际允许4次迭代)。plan存储无持久化。`reflect()` 使用玩具启发式 `1.0 - (issues.len() * 0.2)`。

**实施**:
1. 将BrainLoop接入mode runtimes（至少 FullAutoMode 和 AgentMode）
2. 修复off-by-one
3. 将reflect()置信度替换为基于DiagnosticFeedbackEngine的反馈
4. Plan存储加入持久化（SQLite）

**验收**:
- FullAutoMode run() 内部调用 BrainLoop::execute_step()
- max_iterations=N 精确执行N次迭代
- 重启后plan状态可恢复

---

#### GAP-46-08（P1）: Recovery策略匹配升级

**现状**: `recovery.rs` 使用 `similarity_score()` = `1.0 / (1.0 + levenshtein_distance)` 在failure类型字符串和策略名称间做匹配。此方法脆弱：`"timeout"` 和 `"rate_limit_backoff"` 的编辑距离差异极小。策略中魔法字符串 `"auto"`, `"current"`, `"fallback"` 永不会匹配真实工具名。

**实施**:
1. 将 `select_strategy()` 替换为显式错误分类enum
2. 策略定义改为配置文件驱动
3. 将 `auto_recovery_rate()` 和 `recovery_evidence_chain()` 从 `#[cfg(test)]` 提升到生产代码

---

#### GAP-46-09（P1）: Scheduler dequeue性能修复

**现状**: `dequeue()` 使用O(n log n)扫描——pop所有任务→过滤role→重推非匹配项。BinaryHeap的优势被消除。

**实施**:
1. 为每个role维护独立BinaryHeap
2. aging从热路径移到定时后台任务
3. 统一并发控制（semaphore-only，移除HashSet计数重复）

---

### 🟢 GREEN — 质量完善（达到卓越级）

#### GAP-46-10（P2）: CI增强

**现状**: CI缺少 `rustfmt` 检查、`cargo audit` 依赖审计、CodeQL安全扫描、Docker构建、benchmark回归测试、macOS/Windows PR覆盖。

**实施**:
1. 添加 `cargo fmt --check`
2. 添加 `cargo audit` / `cargo deny`
3. 添加Docker构建workflow
4. 添加所有测试二进制文件到CI（不仅是 `acp_runtime_rpc_integration`）

---

#### GAP-46-11（P2）: 文档修复

**现状**: DOC声明WebSocket/OAuth/OpenAPI/多语言SDK等不存在功能。Blueprints与DOC割裂。

**实施**:
1. 移除DOC中不存在功能的声明
2. 添加CHANGELOG.md
3. 添加TROUBLESHOOTING.md和FAQ.md
4. 将 `docs/blueprints/` 按历史顺序归档

---

#### GAP-46-12（P2）: 新模块集成

**现状**: 10+个BLUE45新增模块处于"孤岛"状态——编译通过但未被主执行路径调用。

| 模块 | 行数 | 集成状态 |
|:-----|:---:|:--------|
| `tool_lock.rs` | 11 warnings | 工具执行路径未引用LockManager |
| `tool_recommender.rs` | 12 warnings | FullAutoFlow未调用推荐引擎 |
| `tool_pipeline.rs` | 16 warnings | Orchestrator未使用Pipeline |
| `session_context.rs` | 21 warnings | 主会话管理未使用ContextManager |
| `cache_warming.rs` | 35 warnings | FastPathCache未使用预热引擎 |
| `complexity_estimator.rs` | 11 warnings | BrainLoop未使用复杂度估计 |
| `diagnostic_feedback.rs` | 17 warnings | BrainLoop/Recovery未消费诊断 |
| `distributed_tx.rs` | 24 warnings | Coordinator从未被实例化 |
| `skill_market.rs` | 19 warnings | FullAutoFlow未使用Skill市场 |
| `plugin_system.rs` | 16 warnings | 主流程无插件加载点 |
| `chaos.rs` | 21 warnings | 仅在测试中使用 |

**实施**: 将每个模块接入对应的主执行路径。

---

#### GAP-46-13（P2）: 废弃模型清理

**现状**: `orchestrator.rs` 硬编码成本表包含 `gpt-3.5-turbo` 等已废弃模型。`openai.rs` `available_models()` 仍列出 `gpt-3.5-turbo-0125`。

**实施**:
1. 从成本表移除已废弃模型
2. 将成本/延迟表从硬编码改为配置文件驱动
3. 统一使用LivePerformanceFeed动态数据

---

#### GAP-46-14（P2）: Groq Provider完善

**现状**: `groq.rs` 无单元测试。无 `tool_choice` 自动默认。模型列表仅4个模型，功能声明缺失function_calling标记。

**实施**:
1. 添加 `tool_choice: "auto"` 默认逻辑（与OpenAI对齐）
2. 补齐测试覆盖
3. 更新模型列表和功能声明

---

## 四、改进执行计划

| 优先级 | 编号 | 改进项 | 预估周期 | 预期提分 | 新评分 |
|:------:|:-----|:-----|:--------:|:--------:|:------:|
| **P0** | GAP-01 | main.rs重构 | 2周 | +8 | 67.1→75.1 |
| **P0** | GAP-02 | DAG执行器重写 | 2周 | +7 | 75.1→82.1 |
| **P0** | GAP-03 | Gemini FC修复 | 3天 | +5 | 82.1→87.1 |
| **P0** | GAP-04 | hot_reload/schema集成 | 3天 | +5 | 87.1→92.1 |
| **P1** | GAP-05 | mode.rs去重 | 1周 | +3 | 92.1→95.1 |
| **P1** | GAP-06 | Orchestrator单例消除 | 3天 | +2 | 95.1→97.1 |
| **P1** | GAP-07 | BrainLoop集成 | 1周 | +3 | 97.1→100 |
| **P1** | GAP-08 | Recovery升级 | 3天 | — | 质量提升 |
| **P1** | GAP-09 | Scheduler优化 | 3天 | — | 性能提升 |
| **P2** | GAP-10 | CI增强 | 1周 | — | 质量提升 |
| **P2** | GAP-11 | 文档修复 | 3天 | — | 质量提升 |
| **P2** | GAP-12 | 新模块集成 | 2周 | — | 质量提升 |
| **P2** | GAP-13 | 废弃模型清理 | 1天 | — | 维护性 |
| **P2** | GAP-14 | Groq完善 | 2天 | — | 供应商质量 |

---

## 五、BLUE46 执行完成率追踪

### 5.1 完成率计算

| 优先级 | 总数 | 完成 | 完成率 |
|:------|:----:|:----:|:------:|
| P0 | 4 | 2 | **50%** |
| P1 | 5 | 4 | **80%** |
| P2 | 5 | 4 | **80%** |
| ───── | ─── | ─── | ───── |
| **总计** | **14** | **10** | **71%** |

### 5.2 本轮完成项 (10/14)

| # | 改进项 | 优先级 | 状态 | 变更 |
|:--|:-------|:------:|:----:|:-----|
| GAP-03 | Gemini Function Call 修复 | P0 | ✅ | `gemini.rs` 流式处理器增加functionCall解析 + finishReason检测 |
| GAP-04 | 死代码集成 — hot_reload/schema | P0 | ✅ | `load.rs`加入schema验证; config文件添加version字段; `main.rs`启动WatchDog |
| GAP-05 | mode.rs 5×重复代码消除 | P1 | ✅ | 提取`BaseModeRuntime` + `ModeStrategy` trait; 改用共享Tokio runtime |
| GAP-06 | Orchestrator全局单例消除 | P1 | ✅ | 移除OnceLock; 创建`OrchestrationContext`; 注入到`exec_pack.rs` |
| GAP-07 | BrainLoop集成与修复 | P1 | ✅ | 修复off-by-one; RAII plan模式; 添加持久化; 接入FullAutoFlow |
| GAP-08 | Recovery策略匹配升级 | P1 | ✅ | `FailureKind`枚举替代Levenshtein; `ToolReference` enum替代magic strings |
| GAP-09 | Scheduler dequeue性能修复 | P1 | ✅ | per-role BinaryHeap; Semaphore-only并发; aging移到后台timer |
| GAP-12 | 新模块集成 | P2 | ✅ | 创建`SystemIntegration` hub, 集成6个孤立模块 |
| GAP-13 | 废弃模型清理 | P2 | ✅ | 移除gpt-3.5-turbo/deepseek-v3; 合并claude-sonnet-4重复条目 |
| GAP-14 | Groq Provider完善 | P2 | ✅ | 添加tool_choice自动默认; 8个单元测试; 更新模型列表 |

### 5.3 剩余待补齐项

| # | 改进项 | 优先级 | 说明 |
|:--|:-------|:------:|:-----|
| GAP-01 | main.rs God-File重构 | P0 | 2,378→<600行：提取bootstrap/onboarding/transport_factory |
| GAP-02 | DAG执行器重写 | P0 | 真实拓扑排序+依赖边+节点输出注入 |
| GAP-10 | CI增强 | P2 | rustfmt check, cargo audit, Docker构建, macOS/Windows PR覆盖 |
| GAP-11 | 文档修复 | P2 | 移除DOC中不存在功能的声明; CHANGELOG/FAQ/TROUBLESHOOTING |

---

## 六、核心规则（继承BLUE43）

1. 5种协议全链路闭合
2. 3种服务器Profile全链路闭合
3. 注释英文 — 所有新增模块代码注释使用英文
4. i18n全覆盖
5. 完整闭合 — 编译通过、零警告、接入governance.status、health端点可观测、集成测试覆盖
6. 三端一致性 — Backend/GUI/vscode-addon无字段漂移
7. `cargo clippy --all-features -- -D warnings` 为硬门
8. 回写完成率

## 七、核心优势总结

1. **治理体系** — 10文件governance模块：PUA规则引擎+RBAC+自适应控制+审计+漂移检测
2. **韧性工程** — 全状态机CircuitBreaker+FailoverGroup+ChaosEngine(10故障类型)
3. **供应商覆盖** — 35+ AI供应商
4. **测试基础设施** — 1,569个测试，99.94%通过率
5. **功能骨架** — BLUE45 34模块覆盖工具/多模型/事务/性能/流畅度/配置/生态

## 八、核心短板总结（剩余部分）

1. **架构兑现率** — 部分路径仍存在实现与命名偏差（DAG/main.rs）
2. **CI工具链** — 缺少rustfmt audit Docker构建 macOS/Windows PR覆盖
3. **文档准确度** — DOC部分页面描述尚未实现的功能

---

*评估报告: go-on 多Agents编排系统 | BLUE46 全方位评估 | 2026-05-25 | 10/14项完成 (71%)*
