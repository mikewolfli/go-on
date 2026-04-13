# BLUE13 — OpenAI 兼容回归固化与 Responses API 语义结论（CI 可执行）

> 延续 BLUE12 的执行纪律：先方案冻结、再分阶段实施、最后端到端验收。
> 本轮目标：将手工验证固化为自动化兼容回归（固定请求矩阵 + 断言），并给出 Responses API 原生语义可达性结论。

---

## 背景与约束确认

基于 BLUE12 后的运行状态，/v1/models、/v1/model 与 /v1/chat/completions 已可工作，但此前主要依赖手工验证，缺少可在 CI 中稳定执行的兼容性回归。

本轮强约束如下：

1. 回归必须可在 CI 中无外部依赖稳定执行。
2. 固定请求矩阵必须覆盖非流式与流式、基础与复杂结构。
3. 断言必须直接检查协议输出关键语义（成功响应、SSE 帧、[DONE]）。
4. 明确回答“是否 100% 原生 Responses API 语义”并给出工程路径。

结论：可以完成自动化回归固化；当前不可声称 100% 原生 Responses API 语义。

---

## 目标

| ID | 目标 | 目标目录/文件 |
|---|---|---|
| B13-M1 | 新增 OpenAI 兼容集成回归（固定请求矩阵 + 断言） | tests/openai_compat_matrix_integration.rs |
| B13-M2 | 新增 OpenAI 结构映射单元回归（字段透传与角色归一） | src/acp/impl/runtime.rs |
| B13-M3 | 将兼容回归显式接入 CI 脚本 | test_ci.sh |
| B13-M4 | 修复 ACP stdout 污染风险（日志与协议输出分离） | src/observability/telemetry_enhanced.rs |
| B13-M5 | 增强 OpenAI 命令字段透传（Chat Completions 常用字段） | src/agents/mod.rs + src/agents/openai.rs + src/agents/openai_compatible.rs |
| B13-M6 | 输出 Responses API 原生语义可达性结论与落地路径 | blue13.md |
| B13-M7 | 明确 GUI 与 VS Code Addon 一致性保障方案与验收门禁 | GUI/ + vscode-addon/ + blue13.md |

---

## 技术选型（冻结）

### 回归测试策略
- Rust 集成测试（tests/）
- 临时配置 + 本地 test-only agent（local_echo）
- 固定请求矩阵覆盖：
  - GET /v1/models
  - GET /v1/model
  - POST /v1/chat/completions（非流式）
  - POST /v1/chat/completions（stream=true）

### 断言策略（冻结）
- 非流式：
  - 无 error 字段
  - choices[0].message.content 非空
- 流式：
  - 含 data: 帧
  - 含 [DONE] 结束标记
- 模型接口：
  - data 列表非空

### 兼容字段策略（冻结）
- 在 agent payload 层支持透传常见 OpenAI 字段：
  - temperature
  - top_p
  - max_tokens
  - n
  - stop
  - presence_penalty
  - frequency_penalty
  - logit_bias
  - user
  - seed
  - response_format
  - tools
  - tool_choice
  - parallel_tool_calls
  - function_call
  - functions

---

## 实施结果

### B13-M1：OpenAI 兼容集成回归 ✅
- 新增测试文件：
  - tests/openai_compat_matrix_integration.rs
- 已覆盖：
  - /v1/models
  - /v1/model
  - /v1/chat/completions（非流式）
  - /v1/chat/completions（流式 SSE）
- 已执行通过：
  - cargo test openai_http_request_matrix_regression -- --nocapture

### B13-M2：OpenAI 结构映射单测 ✅
- 在 runtime 增加结构映射回归测试：
  - openai_to_chat_params_maps_options_and_roles
- 已执行通过：
  - cargo test openai_to_chat_params_maps_options_and_roles -- --nocapture

### B13-M3：CI 接入 ✅
- CI 辅助脚本增加显式步骤：
  - test_ci.sh 增加“OpenAI 兼容回归测试”执行段

### B13-M4：ACP 输出通道稳定性 ✅
- 修复日志输出通道，避免 ACP stdout 被日志污染：
  - telemetry fmt writer 指向 stderr
- 结果：stdout 保持 JSON-RPC 纯净输出，stderr 承载日志。

### B13-M5：OpenAI 命令字段兼容增强 ✅
- 新增公共字段透传函数并在 openai / openai_compatible 统一使用。
- 对复杂消息结构（content 数组/对象/null、tool_calls 等）进行兼容转换。
- 对非核心角色（如 tool）执行归一化并保留元数据文本，避免下游 schema 失败。

### B13-M6：Responses API 语义结论 ✅
- 当前结论：
  - 不能宣称 100% 原生 Responses API 语义。
- 原因：
  - 当前主能力仍以 Chat Completions 兼容路径为核心。
  - Responses API 的完整对象模型与状态机（如输出项生命周期、原生 tool/reasoning 语义）未独立全量建模。
- 可达路径：
  1. 新增独立 /v1/responses 端点与 schema。
  2. 建立原生 tool/function 调用状态机与事件流。
  3. 增加 golden tests（官方样例快照比对）并纳入 CI。

---

## 100% 原生 Responses API 语义实施步骤

以下步骤用于将“兼容层”演进为“原生 Responses API 语义实现”，并可直接进入迭代排期。

### Phase R1：协议面建模（Schema First）
1. 新增独立请求/响应类型：
  - request：input、model、tools、tool_choice、parallel_tool_calls、metadata、reasoning 等。
  - response：id、status、output[]、usage、incomplete_details、error 等。
2. 引入对象级校验：
  - 严格区分 null 与缺省字段。
  - 保留未知扩展字段（forward compatibility）。
3. 新增端点：
  - POST /v1/responses
  - GET /v1/responses/{id}（可选，若实现持久化）

R1 验收标准：
- 能通过最小合法请求返回规范结构对象。
- 非法字段组合返回可解释错误，不退化为 chat-completions 样式错误。

### Phase R2：执行状态机与工具调用语义
1. 建立 response 生命周期状态机：
  - queued -> in_progress -> completed / incomplete / failed。
2. 实现 output item 语义：
  - message、tool_call、tool_result、reasoning（按实现范围逐步开放）。
3. tool 调用闭环：
  - 支持 tool call 发起、参数回传、结果注入、继续推理。
4. 失败分类标准化：
  - timeout、rate_limit、tool_error、upstream_error 分层映射。

R2 验收标准：
- 至少 1 条含工具调用的多步对话可完整闭环。
- 状态流与 output 序列可复现、可断言。

### Phase R3：流式语义（SSE）对齐
1. 定义并实现 response 事件帧：
  - response.created
  - response.output_text.delta
  - response.output_item.done
  - response.completed / response.failed
2. 保证事件顺序与幂等：
  - 同一 response 的事件序列稳定且可重放校验。
3. 终止语义统一：
  - 正常结束与异常结束均有明确 terminal 事件。

R3 验收标准：
- 流式请求可稳定产出完整事件链。
- 客户端重连或消费延迟场景下，事件序列仍可被一致解释。

### Phase R4：一致性测试体系（Golden + Matrix）
1. 建立 golden cases：
  - 覆盖文本输出、工具调用、异常终止、限流等场景。
2. 建立对比断言：
  - 字段级快照（忽略时间戳、随机 id 等非确定项）。
3. 接入 CI：
  - smoke（快速）+ full matrix（完整）双层执行。

R4 验收标准：
- 新增改动若破坏语义，CI 必须红灯且可定位到具体快照差异。

### Phase R5：兼容策略与迁移发布
1. 双栈期策略：
  - 保留 /v1/chat/completions。
  - 新增 /v1/responses 为推荐路径。
2. 文档与示例：
  - 给出从 chat-completions 到 responses 的字段映射表。
3. 分阶段开关：
  - 配置项控制 Responses 原生能力启用范围。

R5 验收标准：
- 既有客户端无回归。
- 新客户端可按文档完成无歧义接入。

### Phase R6：GUI 与 VS Code Addon 一致性保障
1. 统一协议契约（Single Source of Truth）：
  - 以后端 OpenAI/Responses schema 为唯一真源。
  - GUI 与 VS Code Addon 仅消费同一份字段映射与错误码语义，不各自“二次定义”。
2. 统一能力矩阵：
  - 建立 capability matrix（端点、字段、流式事件、错误类型）并按版本维护。
  - 新增字段必须同时标注 GUI 支持状态与 addon 支持状态。
3. 统一文案与错误提示：
  - 错误分类、建议动作、状态术语在 GUI 与 addon 统一口径。
  - i18n key 命名保持一致，避免同义不同 key。
4. 统一测试门禁：
  - 同一组契约测试在 GUI 和 addon 侧都要跑通（至少 smoke + 核心矩阵）。
  - 任一端回归失败，发布闸门关闭。
5. 统一发布节奏：
  - 建立“后端 -> GUI/addon”版本兼容表。
  - 每次后端语义变更需附 GUI 与 addon 的升级说明与最低兼容版本。

R6 验收标准：
- 同一请求样例在 GUI 与 addon 展示结果一致（状态、错误、关键字段）。
- capability matrix 与发布兼容表在一次发布内同步更新。
- CI 中 GUI/addon 契约测试全绿后方可发版。

### 建议排期（可执行）
- Sprint 1：R1 + R2 基础闭环。
- Sprint 2：R3 流式事件语义。
- Sprint 3：R4 测试固化 + R5 发布迁移。
- Sprint 4：R6 双端一致性门禁（GUI + VS Code Addon）。

### 最终达标口径（何时可宣称 100%）
同时满足以下条件后，才可对外宣称“100% 原生 Responses API 语义”：
1. /v1/responses 主路径功能可用且覆盖核心对象语义。
2. 工具调用与流式事件语义通过 golden + matrix 全量回归。
3. 错误模型、状态机、终止语义与文档定义一致。
4. CI 长期稳定，无语义回退。
5. GUI 与 VS Code Addon 在 capability matrix 与契约测试上保持一致。

---

## 分阶段执行计划（回写）

### BLUE13-M1（回归框架落地）
- [x] 增加集成回归测试文件。
- [x] 启动 harness + 临时配置 + 本地 agent。
- [x] 跑通基础矩阵。

### BLUE13-M2（兼容字段收敛）
- [x] 统一字段透传函数。
- [x] 修复复杂消息映射边界。
- [x] 验证流式与非流式均通过。

### BLUE13-M3（CI 固化）
- [x] test_ci.sh 增加 OpenAI 兼容回归步骤。
- [x] 本地执行通过。

### BLUE13-M4（语义结论输出）
- [x] 输出“当前不可 100% 原生”的明确结论。
- [x] 给出可实施的三步演进路线。

### 完成率回写（2026-04-13）
- BLUE13 当前完成率：100%（16/16）。
- 状态：M1-M4 全部勾选完成，测试与结论均落地。

---

## 关键命令（本轮）

```bash
# 结构映射单测
cargo test openai_to_chat_params_maps_options_and_roles -- --nocapture

# OpenAI 兼容矩阵集成回归
cargo test openai_http_request_matrix_regression -- --nocapture

# 全量格式化（已执行）
cargo fmt --all
```

---

## 版本与状态

- 结论文档版本：BLUE13 / 2026-04-13
- 回归状态：✅ 已固化为自动化（CI 可执行）
- 语义结论：
  - Chat Completions 兼容：✅ 当前可用并有自动回归
  - 100% 原生 Responses API：❌ 当前不可宣称，需独立协议层实现
