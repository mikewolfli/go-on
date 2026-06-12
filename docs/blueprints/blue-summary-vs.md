# BLUE-SUMMARY-VS — 多 Agents 编排系统超深度复扫评估（SRC 全域）

> 日期：2026-06-04
> 扫描范围：`src/` 为主，联动 `gui/src/`、`sdk/`、`vscode-addon/src/`、`tests/`、`scripts/`、`config/`
> 扫描方式：4 轮反复扫描（结构检索 -> 热路径精读 -> 跨层交叉验证 -> 收敛复扫）
> 评估目标：处理问题/执行操作的速度与流畅度、智能程度、公正自评、14层缺陷与改进计划

---

## 0. 执行规则（拷贝自 BLUE61）

1. 排除 i18n 字段硬编码 — 不涉及 locale 文本本身的结构调整。
2. 支持按要求按逻辑分步骤分拆文件 — 可按模块目录拆分重组。
3. 三端一统（backend / GUI / vscode-addon）— 考虑三端配合、通讯流畅稳定性。
4. 注释英文 — 所有新增模块的代码注释必须使用英文。
5. 3 种服务器 Profile 全链路闭合 — local、simple-server、multi-users-server 全部正确编译和行为一致（零警告）。
6. 5 种协议全链路闭合 — auto、acp stdio、acp http、mcp stdio、mcp http。
7. 零警告、零冲突、零遗漏 — cargo clippy -- -D warnings 在全部4个profile下零警告通过。
8. 完整闭合 — 每个模块达到：编译通过、零警告、接入 governance.status、可通过 health 端点观测、有集成测试覆盖。
9. 不允许占位、空函数、逻辑错误 — 所有功能必须完整实现。
10. 回写完成率 — 每轮完成后回写完成率至本文。
11. 多轮反复扫描 — 至少3轮并行/增量扫描并收敛。
12. 最后一趟扫描 — 本文记录最终收敛结果和剩余真实风险。

---

## 1. 多轮扫描记录（直到无新发现）

### Round 1：全域结构检索（广度）
- 统计发现：
  - `allow(dead_code)`：594 处（`src/`+周边）
  - `#[ignore]`：10 处
  - `placeholder/simulated/stub/future wiring/planned wiring`：408 处
  - `unimplemented!/todo!(`：6 处
- 初步结论：系统有大量“预留能力”和“占位实现”，完整度存在层级差异。

### Round 2：热路径精读（深度）
- 聚焦：`src/acp/impl/chat.rs`、`src/acp/helpers/autonomy/autonomy_loop.rs`、`src/orchestration/*`、`src/security/*`、`src/protocol/*`
- 关键确认：
  - 主聊天入口 `process_chat_request` 仍是超大函数（起始于 `src/acp/impl/chat.rs:1172`，直到 `resolve_request_phase` 前）。
  - 自治循环已具备“工具结果回注下一轮消息”的闭环，不再是一次性工具调用（`src/acp/helpers/autonomy/autonomy_loop.rs:1000+`）。
  - 但分布式执行与多模态视频等模块存在明显占位。

### Round 3：跨层交叉验证（三端与工程侧）
- GUI：存在“失败回退陈旧缓存”的可用性风险（`gui/src/backend.rs:372+`）。
- VS Code Addon：TOML 编辑仍是正则法，文件内已自述局限（`vscode-addon/src/extension.ts:170+`）。
- SDK：三语言都有重试与 jitter，基础可用；但高级编排能力暴露不足。
- 发布门禁：脚本覆盖三 profile + clippy + tests（`scripts/run-release-readiness-gate.sh`）。

### Round 4：收敛复扫（无新类别缺陷）
- 再次统计：
  - `src` placeholders 165，`gui` 18，`sdk` 111，`addon` 12。
- 结论：新增问题类别已收敛为 0；剩余风险集中在“预留功能未接线”和“测试真实性不足”。

---

## 2. 公正自评（中肯）

### 2.1 速度与流畅度
- 评分：7.8/10
- 优点：
  - 有重试、jitter、超时、并发执行骨架。
  - 自治循环支持多轮与工具执行，并能记录治理/观测指标。
- 短板：
  - 主链路函数过大导致优化粒度粗。
  - 部分高价值模块仍占位，真实执行深度不足（尤其分布式与视频处理）。

### 2.2 智能程度
- 评分：7.4/10
- 优点：
  - 具备能力路由、委员会协商、记忆注入、风险治理等丰富构件。
- 短板：
  - 构件“存在”不等于“充分驱动主链路”；若干能力仍偏框架级、非强执行级。

### 2.3 综合结论
- 当前状态：强架构骨架 + 中等执行实度。
- 与“高自治多 agents 编排系统”的差距：主要在可运行深度、闭环验证、真实负载下的稳定性证据。

---

## 3. 14层不足与缺陷（证据化）

## 3.1 架构层
1. 主流程过于集中：`src/acp/impl/chat.rs:1172` 起的大函数影响可维护性、可测试性和性能定位。
2. 大量 F-GAP 预留接口未完成接线，造成“结构完整、行为未完整”。

## 3.2 运行层
1. 分布式远端执行仍有占位返回：`src/orchestration/distributed/remote_executor.rs:377`、`src/orchestration/distributed/remote_executor.rs:507`。
2. gRPC/远端执行路径更多是框架定义，生产级端到端证据不足。

## 3.3 智能层
1. Full-auto 中部分流程仍“前瞻占位”而非真实重执行闭环：`src/orchestration/full_auto.rs:1110`。
2. 任务复杂度与策略虽有，但“策略 -> 真实动作”的闭环覆盖不均。

## 3.4 治理层
1. 治理构件丰富，但与所有执行分支的强绑定仍不均（主要体现在 reserved 标记较多）。
2. 工具级细粒度治理在所有路径的覆盖度需要统一审计。

## 3.5 协议层
1. `src/protocol/grpc.rs` 顶部仍是 `#![allow(dead_code)]`，显示多用户分布式协议路径活跃度不足。
2. 多协议闭环“可编译”与“高负载真实可用”之间仍有证据缺口。

## 3.6 韧性层
1. Chaos 相关模块预留较多（如 `src/resilience/chaos.rs` 多处 dead_code），实战演练覆盖不足。
2. 故障注入回归测试缺少系统级常态化运行证据。

## 3.7 可观测层
1. 观测能力存在，但部分模块仍标记 future wiring（`src/observability/mod.rs:23`）。
2. 对“占位路径命中率、真实执行率”的指标拆分还不够细。

## 3.8 内存层
1. 多层 memory 结构齐全，但仍有大量 reserved/planned wiring（如 `src/memory/memory_retrieval.rs`、`src/memory/semantic_cache.rs`）。
2. 记忆命中质量与错误注入场景下的一致性指标不足。

## 3.9 GUI层
1. 模型拉取失败时会回退陈旧缓存，用户侧缺少足够显著的“数据可能过期”提示（`gui/src/backend.rs:372+`）。
2. 监控页已有退避机制，但更细粒度告警解释仍可加强（`gui/src/views/monitor.rs`）。

## 3.10 SDK层
1. SDK 具备稳定通信能力，但“高级编排能力映射”仍偏基础 RPC 封装。
2. 跨语言 SDK 与自治策略的契约一致性测试可继续增强。

## 3.11 VS Code Addon层
1. TOML 修改仍是正则方案，文件中已注明局限（`vscode-addon/src/extension.ts:170+`）。
2. 运行时二进制校验默认依赖同源 checksums，硬编码信任哈希未默认启用（`vscode-addon/src/runtimeBinaryService.ts`）。

## 3.12 测试层
1. `tests/e2e_tests.rs` 明确说明整组忽略（`tests/e2e_tests.rs:7`）。
2. 多个 e2e 用 simulated/stub 描述，说明真实外部依赖场景覆盖仍不足（例如 `tests/e2e/test_distributed_dag_e2e.rs:253`）。

## 3.13 部署层
1. 有发布门禁脚本，但“线上型长稳压测 + 故障演练”未见等价硬门禁。
2. 多 profile 虽可检查编译/测试，通过不代表等价运行行为已充分验证。

## 3.14 安全层
1. mTLS 与 secret rotation 等模块存在 reserved 标记，生产默认安全强度依赖部署方额外启用（`src/security/mtls.rs`、`src/security/secret_rotation.rs`）。
2. 漏洞扫描器包含 placeholder 路径（`src/security/vulnerability_scan.rs:306`、`src/security/vulnerability_scan.rs:352`）。

---

## 4. 改进计划（具体步骤）

## P0（1-2 周，先解“真实性”）
1. 拆分主聊天函数：将 `process_chat_request` 分为 phase resolve、agent route、execution、response assembly 四段模块，目标单函数 < 300 行。
2. 清理高风险占位实现：
   - `remote_executor` 的 placeholder 输出改为真实执行或显式 fail-fast。
   - `vulnerability_scan` 的 placeholder 分支改为“不可用即硬失败 + 指标上报”。
3. 测试真实性提升：
   - 将 `tests/e2e_tests.rs` 从整组 ignore 改为分层可运行（mockless smoke + external gated suites）。

## P1（2-4 周，提升“速度与流畅度”）
1. 执行并行化实装：在可并发的 agent 预选、健康探测、工具 DAG 执行中增加并行度与早停策略。
2. 引入“占位路径命中率”指标：新增 metrics 标签区分 real vs placeholder/stub execution。
3. GUI 体验改进：缓存回退时展示显著 stale 标记与刷新建议。

## P2（4-8 周，提升“智能度”）
1. 策略闭环化：把复杂度估计/委员会决策结果写入下一轮执行约束，而非仅记录元数据。
2. 记忆质量闭环：建立 memory 命中准确率、误召回率、冲突率指标并进入 release gate。
3. SDK 高阶能力：统一暴露自治循环报告、治理报告、故障恢复轨迹查询接口。

## P3（持续，生产级治理与安全）
1. 默认启用更强供应链校验（Addon runtime 固定可信哈希 + 签名校验）。
2. 将 mTLS/secret rotation 从 reserved feature 升级到可配置默认路径。
3. 建立三 profile 的长稳压测与混沌演练门禁，并纳入发布阻断条件。

---

## 5. 量化目标（验收标准）

1. 占位实现压降：`placeholder/simulated/stub` 命中数 8 周内下降 60%。
2. 主链路复杂度：`process_chat_request` 拆分后，关键流程函数圈复杂度下降 40%。
3. 测试真实性：
   - e2e ignore 数从 10 降到 <= 2（仅外部受控场景）。
   - 分布式与安全关键路径新增可复现集成测试 >= 15 个。
4. 体验指标：
   - 复杂任务 P95 降低 25%。
   - GUI stale 数据误判投诉显著下降（以 issue 数据追踪）。

---

## 6. 本轮完成率回写

1. 多轮反复扫描：完成（4轮，最终收敛无新类别）。
2. 公正中肯自评：完成（优势与不足均基于代码证据）。
3. 14层缺陷盘点：完成。
4. 具体改进步骤：完成（P0-P3 + 量化验收）。
5. 执行规则拷贝 blue61：完成（见第0节）。

完成率：100%
