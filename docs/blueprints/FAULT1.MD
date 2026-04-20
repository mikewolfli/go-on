# FAULT1: 全项目故障与半残能力治理清单（规则同 BLUE5）

> **扫描结论（截至 2026-04-08）**
> - ✅ 主链路可用：主从治理、双评审门禁、会诊/澄清/学习汇总已闭环并通过回归
> - ✅ 性能指标真实性：`memory_usage_bytes` 已去占位化（Windows API + Linux `/proc/self/status`）
> - ✅ 可观测最小闭环：Telemetry/Tracing 已接入 OTLP gRPC 与 stdout fallback
> - ✅ 规则一致性：strict 与 early 冲突入口已移除并统一 strict
> - ✅ 产品面一致性：provider 实现、示例配置、README 已收敛
>
> **完成度标识（本轮）**
> ✅ FAULT1-1.0-M0：已完成必要性评估与保留/删除决策归档
> ✅ FAULT1-1.1-M1：性能内存指标已去占位化（Windows API + Linux `/proc/self/status` 采样路径）
> ✅ FAULT1-1.0-M2：Telemetry/Tracing 最小 OTLP 上报链路已闭环（OTLP gRPC + stdout fallback，`init_otel_provider` 实现已替换占位 stub；unused `tracing-opentelemetry@0.23` dep 已移除）
> ✅ FAULT1-1.1-M3：strict 与 early 规则冲突已消解（agent stage 指令与 setup 默认 review 模板统一 strict）
> ✅ FAULT1-1.0-M4：provider 产品面已收敛（README 新增 qwen/qianfan [experimental] 说明；唯一模板 `config.toml.autopilot-adaptive` 承载配置示例）
> ✅ FAULT1-1.0-M5：`dead_code` 豁免范围首轮收缩（33 → 28 文件；从 error/i18n/i18n_watcher/mode/memory 移除文件级 allow，架构占位内容以 22 条 warning 显式可见；测试全绿 196+23）

## Positioning
- 沿用 BLUE5 原则：所有优化项必须“必要性可证明、收益可量化、回归可验证”。
- 本文新增“删不删”的明确结论，避免只提改进但不做裁剪决策。

## 1. 必要性评估框架

### 1.1 评估维度
- 必要性：是否直接影响生产稳定性/治理正确性。
- 使用度：是否在默认配置、主文档、主流程中被实际使用。
- 维护成本：代码体量、测试成本、认知负担。
- 误导风险：是否会让系统看起来“可用”但实际失效。

### 1.2 决策等级
- `KEEP`: 必须保留并修复。
- `KEEP_MIN`: 保留核心，收缩边界。
- `DEPRECATE`: 先下线默认暴露，再观察窗口移除。
- `REMOVE`: 可直接删除。

## 2. 保留/删除决策矩阵（一次性结论）

1. 性能内存指标链路（`src/performance.rs`）
- 决策：`KEEP`
- 原因：属于治理核心指标，不应删除业务能力，但必须去占位化。
- 动作：实现真实采样；若短期无法实现，临时移除依赖该值的告警分支并显式标注 unavailable。

2. Telemetry/Tracing 链路（`src/telemetry.rs`、`src/telemetry_enhanced.rs`）
- 决策：`KEEP_MIN`
- 原因：可观测是基础设施刚需；但可先保留最小闭环，不做大而全重写。
- 动作：保留统一入口，补 OTLP 最小上报闭环；未实现能力从 README 迁出承诺。

3. early-stage 宽松评审文案（`src/agents/qwen.rs`、`src/agents/wenxin.rs`、`src/agents/qianfan.rs`、`src/setup.rs`）
- 决策：`REMOVE`
- 原因：与 strict 规则冲突，且会诱发“允许占位实现”的门禁漂移。
- 动作：删除/替换 early 宽松语句，统一 strict 口径。

4. provider 能力面（Qwen/Qianfan 等）
- 决策：`DEPRECATE`（非默认）+ `KEEP`（实现）
- 原因：实现可用，但默认配置和主文档未主推，立即删除收益低且可能破坏潜在用户。
- 动作：先标记 experimental，移出默认流程；观察 1-2 个版本后再决定是否删除。

5. 顶层 `#![allow(dead_code)]` 大范围豁免
- 决策：`KEEP_MIN`
- 原因：一次性删除风险高；但必须分批收敛，不再扩大。
- 动作：建立模块级豁免白名单和下降目标，逐模块移除。

## 3. 可立即删除/收缩清单（低风险先行）

1. 立即删除
- 删除 agent 与 setup 中“early 阶段允许占位实现”文案入口。

2. 立即收缩
- README 中未兑现的 Telemetry 能力描述改为“已实现/未实现”双列表。
- 非默认 provider 标注 experimental，避免误导为“默认建议”。

3. 暂不删除（保留并修复）
- 性能内存指标模块本体。
- Telemetry 模块入口。

## 4. 优化路线（按必要性优先）

1. P0：修复或下线失真指标（必须）
- 目标：消除 `memory_usage_bytes` 伪数据。
- 验收：压力下指标变化可观测；告警可触发或被显式禁用并标注原因。

2. P0：补齐最小可观测闭环（必须）
- 目标：请求级 trace/metric 至少可外部查询一条样例链路。
- 验收：具备可复现实验步骤与查询证据。

3. P1：删除规则冲突入口（必须）
- 目标：彻底移除 early 宽松评审文案。
- 验收：静态扫描不再出现“允许占位实现”语义。

4. P1：收敛 provider 产品面（建议）
- 目标：实现、配置、README 一致。
- 验收：主文档、默认配置、运行校验无冲突。

5. P2：收敛 dead_code 豁免（建议）
- 目标：分批降低全局豁免比例。
- 验收：每轮豁免模块数下降，且测试保持全绿。

## 5. 里程碑（FAULT1-M1 ~ M6）

- ✅ M1：性能指标真实性闭环（Windows K32GetProcessMemoryInfo + Linux /proc/self/status）。
- ✅ M2：Telemetry/Tracing 最小 OTLP 闭环（真实 SdkTracerProvider，OTLP gRPC 或 stdout fallback）。
- ✅ M3：移除 early 宽松评审文案（qwen/wenxin/qianfan/setup 统一 strict）。
- ✅ M4：provider 产品面一致化（README + `config.toml.autopilot-adaptive` 统一承载模板示例）。
- ✅ M5：dead_code 豁免首轮收缩（33 → 28，22 条 warning 可见，测试全绿）。
- ✅ M6：发布前全链路复核与文档回写（196 单元 + 23 集成测试全绿，FAULT1 回写完毕）。

## 6. 验收标准（Definition of Done）

- 治理正确性：关键指标不再依赖占位值。
- 可观测性：至少 1 条完整 trace/metric 外部可核验。
- 规则一致性：strict 规则单一真源，无宽松冲突入口。
- 能力一致性：实现、默认配置、README 一致。
- 可维护性：dead_code 豁免范围有可证明下降。

## 7. 测试与发布门禁

- 编译门禁：`cargo check --all`
- 回归门禁：`cargo test --all -- --nocapture`
- 静态门禁：
  - 扫描并阻断 early 宽松文案关键字
  - 扫描并统计顶层 `allow(dead_code)` 变化趋势
- 证据门禁：
  - Telemetry 外部查询截图/日志证据
  - 性能内存指标变化证据或禁用说明证据

## 8. 暂缓项与防过度设计

- 暂缓“全量重写可观测体系”，先做最小闭环。
- 暂缓“立刻删除全部非默认 provider”，先走 deprecate 观察窗口。
- 暂缓“统一去掉所有 dead_code 豁免”，按风险分批收敛。

## 9. 完成定义（FAULT1 Done）

- 不是“做了很多重构”，而是以下五项可证明：
  - 关键指标真实可信
  - 可观测链路可外部验证
  - 规则不冲突
  - 能力面一致
  - 不必要能力已下线或进入明确 deprecate 轨道
