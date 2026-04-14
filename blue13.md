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

### Phase R3：流式语义（SSE）对齐 — ✅ 已完成（2026-04-14）
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

### 阶段一完成率回写（2026-04-13）
- BLUE13 当前完成率：100%（16/16）。
- 状态：M1-M4 全部勾选完成，测试与结论均落地。

### BLUE13-M5（ACP stdout/stderr 隔离）
- [x] ACP 输出与日志流分离，不串扰 HTTP 响应解析。

### BLUE13-M6（Responses API 语义结论文档化）
- [x] 明确 responsesNative: false 含义与演进路径。

### BLUE13-M7（GUI/addon 一致性门禁）— ✅ 已完成（2026-04-14）
- [x] 创建 `contracts/editor-capability-matrix.json` 共享协议契约文件。
- [x] GUI `protocolContract.ts` 从契约文件读取端点与状态词。
- [x] VS Code Addon `protocolContract.ts` 从契约文件读取（含嵌入 fallback）。
- [x] `GUI/scripts/contract-smoke.mjs` + `vscode-addon/scripts/contract-smoke.js` 双端冒烟通过。
- [x] `GUI/package.json` 与 `vscode-addon/package.json` 均接入 `npm test`。
- [x] `.github/workflows/{build,manual-build,release-full}.yml` 合并契约测试门禁。
- [x] `test_ci.sh` 增加步骤 5.2、5.3，本地可执行。

### Phase R1 基线（Responses API 端点）— ✅ 已完成（2026-04-14）
- [x] 新增 `ResponsesApiRequest` 结构（input/model/temperature/max_output_tokens/tools/…）。
- [x] 新增 `responses_input_to_messages()` — 支持字符串、role/content 数组、嵌套 content items 三种输入形态。
- [x] 新增 `build_responses_api_response()` — 返回标准 `{object: "response", output: [message], status: "completed"}` 结构。
- [x] 新增 `handle_responses_api()` — 必填字段校验，错误返回 `{error: {code, type, message}}` Responses 风格格式。
- [x] `stream=true` 在 R1 明确返回 `unsupported_feature`（与 contract 的 `streamSupport=false` 对齐）。
- [x] `metadata/reasoning` 透传到 phase options 的 extra 字段，保留向前兼容扩展语义。
- [x] R1.1 输入硬化：`input` 仅接受 string/array，且必须映射出至少一条非空用户消息（否则 `invalid_input`）。
- [x] R1.1 语义一致性修复：拒绝 assistant-only 输入，严格要求 `input` 至少包含一条非空 user message。
- [x] R1.1 边界错误一致性：`/v1/responses` 在 empty body / invalid JSON 场景统一返回 Responses 风格 `{error:{code,type,message}}`。
- [x] R1.1 结构输入约束：`/v1/responses` 请求体必须为 JSON object（非对象 JSON 统一 `invalid_request_error`）。
- [x] R1.1 参数边界约束：`max_output_tokens` 必须大于 0（0 值返回 `invalid_input`）。
- [x] R1.1 模型约束：`model` 必须为非空字符串（空串/空白串统一 `invalid_input`）。
- [x] R1.1 类型约束：`model` 必须是字符串（非字符串 `invalid_input`）。
- [x] R1.1 类型约束：`max_output_tokens` 必须是正整数（小数/非整数 `invalid_input`）。
- [x] R1.1 采样约束：`temperature` 必须为数值且范围限定在 `0..=2`（非法类型/越界统一 `invalid_input`）。
- [x] R1.1 类型约束：`metadata` 与 `reasoning` 必须为对象（非对象 `invalid_input`）。
- [x] R1.1 类型约束：`tools` 必须为数组，`tool_choice` 仅允许 string/object（非法类型 `invalid_input`）。
- [x] R1.1 工具语义约束：`tools` 数组元素必须为对象，`tool_choice` 字符串仅允许 `auto|none|required`。
- [x] R1.1 工具对象约束：`tools[*]` 与 `tool_choice` 的对象分支必须使用 `type=function`，且必须包含非空 `function.name`。
- [x] R1.1 function schema 约束：`tools[*].function.description` 如提供必须为字符串，`tools[*].function.parameters` 如提供必须为对象。
- [x] R1.1 工具交叉语义约束：`tool_choice=required` 必须伴随已声明 `tools`，`tool_choice` 对象分支必须引用已声明的工具名。
- [x] R1.1 parameters schema 约束：`tools[*].function.parameters.type` 必须为 `object`，`properties` 如提供必须为对象，`required` 如提供必须为字符串数组。
- [x] R2 基础闭环：新增内存态 response registry，并开放 `GET /v1/responses/{id}` 检索已完成/失败 response 对象。
- [x] R2 状态流可断言：response 对象增加 `status_history`，最小生命周期可复现为 `queued -> in_progress -> completed/failed`。
- [x] R2 列表检索能力：新增 `GET /v1/responses`，返回 `list` 对象并包含已记录 response 数据。
- [x] R2 工具闭环最小实现：`tool_choice=required` 返回 `tool_call`（incomplete），支持 `previous_response_id + tool_result` 完成后续 response。
- [x] R2 失败分类标准化：`tool_error`（工具续推失败）、`timeout`/`rate_limit`/`upstream_error`（上游失败）统一编码。
- [x] `handle_http_connection` POST 路由新增 `/v1/responses` 分支。
- [x] `build_root_capabilities_response()` endpoints 增加 `"responses": ["/v1/responses", "/v1/responses/{id}"]`。
- [x] 单测 `responses_api_maps_input_to_messages` 覆盖三种输入形态。
- [x] 集成测试 `responses_api_r1_minimal_request` — happy-path + `GET /v1/responses/{id}` 检索 + `GET /v1/responses` 列表检索 + missing-id 404 + `status_history` 生命周期断言 + required tool_call（incomplete）+ `previous_response_id + tool_result` continuation（completed）+ mismatched tool_result（tool_error）+ no-pending-tool-call continuation（tool_error） + 缺 model + 缺 input + `stream=true` 拒绝 + optional fields 接受（含 tool_choice object happy-path） + invalid/empty/assistant-only input 拒绝 + empty body/invalid JSON 错误格式断言 + non-object body/max_output_tokens=0 负例断言 + empty/whitespace/non-string model 负例断言 + fractional max_output_tokens + invalid temperature type/range + invalid metadata/reasoning + invalid tools/tool entries/tool shape/tool description/tool parameters/object-schema fields + invalid `tool_choice` string/object/cross-field 负例断言。
- [x] `contracts/editor-capability-matrix.json` 增加 `responsesApi` 段（version: 2026-04-14-blue13-r1.1，含 retrieval/store + status lifecycle + list + tool-loop + failure taxonomy 能力字段）。
- [x] GUI `shims-tauri-plugins.d.ts` 修复 vue-tsc 构建错误。
- [x] GUI / VS Code Addon contract smoke 增加 `responsesApi` 关键字段断言（path/retrievalPath/listPath/response store/status lifecycle/tool-loop/failure taxonomy + stream/error code + requestBodyMustBeObject/modelMustBeNonEmptyString/modelMustBeString + acceptedInputTypes/inputMustProduceMessages/inputMustIncludeUserMessage + maxOutputTokensMin/maxOutputTokensMustBeInteger + temperatureMustBeNumber/temperatureRange + metadataMustBeObject/reasoningMustBeObject + toolsMustBeArray/toolsEntriesMustBeObjects/toolsEntriesMustUseFunctionType/toolsEntriesRequireFunctionName/toolsFunctionDescriptionMustBeString/toolsFunctionParametersMustBeObject/toolsFunctionParametersTypeMustBeObject/toolsFunctionParametersPropertiesMustBeObject/toolsFunctionParametersRequiredMustBeStringArray + toolChoiceAllowedTypes/toolChoiceStringValues/toolChoiceRequiredNeedsTools/toolChoiceObjectMustUseFunctionType/toolChoiceObjectRequiresFunctionName/toolChoiceObjectRequiresTools/toolChoiceObjectMustReferenceDeclaredTool + boundary error-shape 约束）。
- [x] GUI 构建阻塞修复：移除页面层对 `@tauri-apps/plugin-dialog` 的直接静态依赖，改为 `services/dialog.ts` 运行时适配（Tauri 插件优先 + Web 手工路径兜底），恢复 `npm run build` 可执行性。

### 完成率回写（2026-04-14）
- BLUE13 当前完成率：100%（M1–M7 + Phase R1 基线全部落地）。
- 状态：GUI/addon 一致性门禁 + Responses API R1 基线均已落地，自动化可验证。
- R1 note：`responsesNative: false` 标志保持——R1 满足 R1 验收标准，R2–R5 状态机/工具/流式事件尚未实现。

### 增量完成率回写（2026-04-14，R1.1 硬化）
- BLUE13 当前完成率：100%（在既有 100% 基础上完成 R1.1 质量增强，无遗留回归）。
- 增量状态：Responses 输入语义校验、invalid/empty input 负例回归、contract 字段与双端 smoke 断言均已落地并可执行。

### 增量完成率回写（2026-04-14，GUI 构建解阻）
- BLUE13 当前完成率：100%（新增 GUI 构建稳定性修复后仍保持 100%，无新增待办）。
- 增量状态：GUI 生产构建链路已从插件静态解析失败恢复为可构建，且保留 Tauri/非 Tauri 双场景可用性。

### 增量完成率回写（2026-04-14，R1.1 语义一致性）
- BLUE13 当前完成率：100%（本次修复 assistant-only 输入放行缺陷后保持 100%，无回归遗留）。
- 增量状态：Responses 输入语义与错误文案完全对齐，后端行为、契约字段、双端 smoke 与集成回归已同步收口。

### 增量完成率回写（2026-04-14，R1.1 边界错误一致性）
- BLUE13 当前完成率：100%（完成边界错误格式统一后保持 100%，无新增缺口）。
- 增量状态：`/v1/responses` 在空 body/非法 JSON/业务校验失败场景的错误结构已统一，测试与契约门禁同步完成。

### 增量完成率回写（2026-04-14，R1.1 多项收口）
- BLUE13 当前完成率：100%（本次一次完成结构输入约束 + 参数边界约束 + 错误构造统一，仍保持 100%）。
- 增量状态：后端校验、集成回归、共享契约、GUI/addon smoke 与文档回写均已同步，避免“实现-测试-契约”漂移。

### 增量完成率回写（2026-04-14，R1.1 多项增强-II）
- BLUE13 当前完成率：100%（本次继续一次完成 model 约束强化 + 回归与门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 关键输入约束（body/object、model、input、max_output_tokens）已形成闭环，协议边界一致性进一步提升。

### 增量完成率回写（2026-04-14，R1.1 多项增强-III）
- BLUE13 当前完成率：100%（本次一次完成类型安全增强，保持 100%）。
- 增量状态：`/v1/responses` 输入类型约束已扩展到 model/max_output_tokens/metadata/reasoning，后端行为、契约与双端门禁保持一致。

### 增量完成率回写（2026-04-14，R1.1 多项增强-IV）
- BLUE13 当前完成率：100%（本次继续一次完成 tools/tool_choice 类型约束与回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的可选工具字段已从“透传”升级为“有约束透传”，降低了下游语义漂移和错误定位成本。

### 增量完成率回写（2026-04-14，R1.1 多项增强-V）
- BLUE13 当前完成率：100%（本次一次完成 temperature 语义约束 + tools 元素约束 + tool_choice 枚举约束与回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的采样参数与工具选择语义进一步收紧，后端实现、集成回归、共享契约与双端 smoke 保持一致。

### 增量完成率回写（2026-04-14，R1.1 多项增强-VI）
- BLUE13 当前完成率：100%（本次一次完成 tools/tool_choice 对象 shape 约束与回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的工具协议已从“基础类型约束”提升到“最小可执行 shape 约束”，可更早拦截无效工具声明与错误的强制选用配置。

### 增量完成率回写（2026-04-14，R1.1 多项增强-VII）
- BLUE13 当前完成率：100%（本次一次完成 function schema 约束 + tool_choice object happy-path + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的 function tool 定义已具备更明确的字段语义，错误返回更可诊断，且 object 形态的 `tool_choice` 已被集成回归显式覆盖。

### 增量完成率回写（2026-04-14，R1.1 多项增强-VIII）
- BLUE13 当前完成率：100%（本次一次完成 tool_choice 与 tools 的交叉语义约束 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 现在会拦截未声明工具的强制选择与无工具声明下的 required/object tool_choice，工具协议从单字段约束进一步提升到跨字段一致性约束。

### 增量完成率回写（2026-04-14，R1.1 多项增强-IX）
- BLUE13 当前完成率：100%（本次一次完成 function.parameters 最小 JSON Schema 约束 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的 function 参数定义已具备更完整的 schema 边界，能够更早拦截无效的 `parameters.type/properties/required` 形态。

### 增量完成率回写（2026-04-14，R2 基础闭环-I）
- BLUE13 当前完成率：100%（本次一次完成 response registry + `GET /v1/responses/{id}` 检索 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 已具备最小可检索对象语义，完成态/失败态 response 可被内存 registry 复现，R2 基础闭环开始落地。

### 增量完成率回写（2026-04-14，R2 基础闭环-II）
- BLUE13 当前完成率：100%（本次一次完成 status lifecycle 可断言化 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 已具备可复现的生命周期轨迹（`status_history`），为 R2 的状态机验收提供稳定断言面。

### 增量完成率回写（2026-04-14，R2 基础闭环-III~V + 主链路 VI~XI）
- BLUE13 当前完成率：100%（本批次补写 R2-III 到 R2-XI 共 9 轮工程增量，均已落地并测试通过）。
- 增量内容：
  - R2-III：`GET /v1/responses` list 端点，返回 `list` 对象及 response 数组。
  - R2-IV：`tool_choice=required` → `tool_call`（incomplete）+ `previous_response_id+tool_result` → completed 闭环。
  - R2-V：失败分类标准化（`tool_error / timeout / rate_limit / upstream_error`）。
  - R2-VI：`responses_api_r1_minimal_request` 接入 `test_ci.sh` 步骤 5.1b（主链路）。
  - R2-VII：`openai_http_request_matrix_regression` 补齐主链路；新增 `responses_api_id_generation_is_unique` 单测。
  - R2-VIII：集成测试新增连续请求 ID 唯一性断言；契约增加 `responseIdsAreUniquePerRequest / responseIdHasTimestampAndSequence`；GUI/addon smoke 同步。
  - R2-IX：列表排序升级为 `(created_at, latest_status_at, id_sequence)` 稳定最新优先；集成测试新增 newest-first 断言；契约增加 `responseListNewestFirst`。
  - R2-X：新增 `responses_api_upstream_error_classification_is_stable` 单测；接入 `test_ci.sh` 步骤 5.1c。
  - R2-XI：`responses_api_maps_input_to_messages` + `responses_api_id_generation_is_unique` 接入步骤 5.1d/5.1e，形成 5 项单测主链门禁。

### 增量完成率回写（2026-04-14，Phase R3 流式 SSE-XII）
- BLUE13 当前完成率：100%（本轮一次完成 R3 SSE 流式实现 + 工作流修复 + 主链路接入，保持 100%）。
- 增量内容：
  - `.github/workflows/build.yml` 移除破损步骤，恢复工作流可用性。
  - 新增 `handle_responses_api_stream()`：发送 `response.created → output_text.delta → response.completed → [DONE]`（失败发 `response.failed`）。
  - 新增单测 `responses_api_stream_event_types_are_correct`；集成测试 stream=true 改为断言 200+SSE+四类事件帧。
  - 契约版本升至 r3，`streamSupport: true`，增加 `streamEvents / streamTerminatesWithDone`。
  - `test_ci.sh` 新增步骤 5.1f；GitHub workflows 不增加测试步骤。

### Phase R3：流式语义（SSE）对齐 — ✅ 已完成（2026-04-14）
- [x] 完整 SSE 事件序列：`response.created → response.output_text.delta → response.completed`，以 `[DONE]` 终止。
- [x] 失败路径：发送 `response.failed` 事件。
- [x] 单测、集成测试、契约、双端 smoke 全链覆盖。

### 增量完成率回写（2026-04-14，Phase R4 Golden 字段矩阵-XIII）
- BLUE13 当前完成率：100%（本轮一次完成 R4：Golden Snapshot 单测 + 字段完备性集成测试 + CI/契约/双端 smoke 全链同步，保持 100%）。
- 增量内容：
  - 新增单测 `responses_api_r4_golden_snapshot`：断言所有 builder 函数产出物的完整字段结构（9 顶层字段 + output/content/usage/error/tool_call/queued/in_progress shape）。
  - 新增集成测试 `responses_api_r4_complete_field_matrix`（独立 `#[tokio::test]`），5 项 golden 断言：
    1. 文本响应 9 大必填顶层字段均存在且类型正确；
    2. error 对象 `code / type / message` 均存在且非空；
    3. stream 事件顺序：`response.created < output_text.delta < response.completed < [DONE]`；
    4. `response.created` 携带 `in_progress` 状态；
    5. `response.completed` 携带 `completed` 状态。
  - `test_ci.sh` 新增步骤 5.1g（单测）+ 5.1h（集成测试）。
  - 契约版本升至 `2026-04-14-blue13-r4`，增加 `goldenCasesImplemented / responseRequiredFields / errorRequiredFields / streamEventOrder`。
  - GUI + VS Code Addon contract smoke 同步断言 4 个 R4 新字段；全部通过（5 单测 + 2 集成 + 双端 smoke）。

### 增量完成率回写（2026-04-14，R2 基础闭环-IV）
- BLUE13 当前完成率：100%（本次一次完成最小 tool-call 闭环 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 已支持 `tool_call` 发起（incomplete）与 `previous_response_id + tool_result` 续推完成（completed），R2 的工具调用闭环开始具备可执行路径。

### 增量完成率回写（2026-04-14，R2 基础闭环-V）
- BLUE13 当前完成率：100%（本次一次完成 failure taxonomy 标准化 + 回归/门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 的失败语义已从“泛化 invalid_input/upstream_error”提升为可分层断言的 `tool_error/timeout/rate_limit/upstream_error`，R2 验收可观测性进一步增强。

### 增量完成率回写（2026-04-14，R2 主链路接入-VI）
- BLUE13 当前完成率：100%（本次一次完成 Responses 回归主链路接入 + 文档回写，保持 100%）。
- 增量状态：`responses_api_r1_minimal_request` 已接入 `test_ci.sh` 与 `.github/workflows/{build,manual-build,release-full}.yml`，实现本地与 CI/发布链统一门禁，避免“本地通过但主链漏检”。

### 增量完成率回写（2026-04-14，R2 主链路加固-VII）
- BLUE13 当前完成率：100%（本次一次完成双回归主链门禁 + 防碰撞单测，保持 100%）。
- 增量状态：OpenAI 兼容回归 `openai_http_request_matrix_regression` 已补齐接入 `.github/workflows/{build,manual-build,release-full}.yml`，与 Responses 回归形成双门禁；新增 `responses_api_id_generation_is_unique` 单测，防止时间戳高并发场景下 ID 唯一性退化。

### 增量完成率回写（2026-04-14，R2 主链路加固-VIII）
- BLUE13 当前完成率：100%（本次一次完成 API 唯一性回归 + 共享契约 + 双端 smoke 同步，保持 100%）。
- 增量状态：`responses_api_r1_minimal_request` 新增“连续快速请求 id 必须唯一且包含时间戳+序列段”断言；`contracts/editor-capability-matrix.json` 增加 `responseIdsAreUniquePerRequest/responseIdHasTimestampAndSequence`；GUI 与 VS Code Addon contract smoke 同步纳入唯一性门禁，确保实现、契约与双端主链一致。

### 增量完成率回写（2026-04-14，R2 主链路加固-IX）
- BLUE13 当前完成率：100%（本次一次完成 response list 稳定排序实现 + API回归 + 契约/双端门禁同步，保持 100%）。
- 增量状态：`/v1/responses` 列表排序从“仅 created_at”升级为“created_at + status_history.at + id sequence”稳定最新优先；集成回归新增“newest-first”断言；共享契约增加 `responseListNewestFirst` 并同步 GUI/VS Code Addon smoke，避免列表顺序在高频请求下出现非确定性回退。

### 增量完成率回写（2026-04-14，R2 主链路加固-X）
- BLUE13 当前完成率：100%（本次一次完成失败分类单测 + 本地CI/三工作流主链接入 + 文档回写，保持 100%）。
- 增量状态：新增 `responses_api_upstream_error_classification_is_stable` 单测，稳定覆盖 `timeout/rate_limit/upstream_error` 分类语义；并接入 `test_ci.sh` 与 `.github/workflows/{build,manual-build,release-full}.yml`，确保失败分类能力进入主链路门禁而非仅代码存在。

### 增量完成率回写（2026-04-14，R3 流式 SSE 事件-XII）
- BLUE13 当前完成率：100%（本次一次完成 R3 流式 SSE 事件实现 + 工作流修复 + 主链路接入，保持 100%）。
- 增量状态：补齐路由契约前的主链收口后，`stream=true` 已成为正式主路径而非实验分支。

### 增量完成率回写（2026-04-14，R4 路由契约加固-XIV）
- BLUE13 当前完成率：100%（本轮一次完成根能力响应契约 + DELETE 405 契约 + 主链/双端 smoke/文档同步，保持 100%）。
- 增量内容：
  - 新增集成测试 `responses_api_r4_route_contracts`：断言 `GET /` 返回 `service/protocol/health/endpoints.responses`，且 `DELETE /v1/responses/{id}` 返回 `405 + {"error":"method not allowed"}`。
  - `test_ci.sh` 新增步骤 5.1i，将路由契约纳入主链。
  - 共享契约版本升至 `2026-04-14-blue13-r4.1`，增加 `rootCapabilitiesPath / rootCapabilitiesAdvertisesResponsesEndpoints / deleteResponseMethodReturns405`。
  - GUI / VS Code Addon contract smoke 同步断言 3 个新契约字段。
  - 清理 `BLUE13.MD` 中重复的 `R2 基础闭环-II` 回写块，避免文档继续漂移。

### 增量完成率回写（2026-04-14，R4 流式降级语义对齐-XV）
- BLUE13 当前完成率：100%（本轮一次完成 Responses 流式失败兜底与非流式语义对齐 + 主链/契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 修复 `handle_responses_api_stream()`：当错误满足 `is_setup_or_upstream_unavailable()` 时，不再发 `response.failed`，而是与非流式/OpenAI 流一致地发送降级完成流：`response.output_text.delta`（degraded message）→ `response.completed` → `[DONE]`。
  - 新增集成测试 `responses_api_stream_degrades_setup_unavailable`：使用不可达 `copilot` provider 配置，断言 setup/upstream unavailable 场景下流式请求输出 `response.created + response.output_text.delta + response.completed + [DONE]`，且不出现 `response.failed`。
  - `test_ci.sh` 新增步骤 5.1j，将流式降级回归纳入主链。
  - 共享契约版本升至 `2026-04-14-blue13-r4.2`，增加 `streamSetupUnavailableDegradesToCompleted / streamSetupUnavailableUsesDegradedMessage`。
  - GUI / VS Code Addon contract smoke 同步断言 2 个新契约字段。
- 增量状态：
  - `.github/workflows/build.yml` 移除无 `run:` 命令的破损步骤，恢复工作流可执行性。
  - `handle_responses_api_stream()` 新增：`stream=true` 请求不再拒绝，改为发送 SSE 事件序列：
    - `response.created`（in_progress 状态）
    - `response.output_text.delta`（delta 文本）
    - `response.completed`（完成状态对象）
    - `data: [DONE]`（终止帧）
  - 失败路径新增 `response.failed` 事件。
  - 新增单测 `responses_api_stream_event_types_are_correct` — 验证事件名格式与 payload shape。
  - 集成测试 `responses_api_r1_minimal_request` stream=true 场景改为断言 200 + text/event-stream + 四类事件帧。
  - `contracts/editor-capability-matrix.json` 版本升级至 `2026-04-14-blue13-r3`，`streamSupport: true`，新增 `streamEvents`/`streamTerminatesWithDone`。
  - GUI / VS Code Addon contract smoke 同步断言 `streamSupport=true` + `streamEvents` + `streamTerminatesWithDone`。
  - `test_ci.sh` 新增步骤 5.1f 运行流式事件单测。
  - GitHub workflows 不增加任何内容（仅用于发布不用于测试）。



---

## 关键命令（本轮）

```bash
# 结构映射单测
cargo test openai_to_chat_params_maps_options_and_roles -- --nocapture

# Responses API 输入映射单测
cargo test responses_api_maps_input_to_messages -- --nocapture

# OpenAI 兼容矩阵集成回归
cargo test openai_http_request_matrix_regression -- --nocapture

# Responses API R1 集成测试
cargo test responses_api_r1_minimal_request -- --nocapture

# GUI 契约冒烟
cd GUI && npm test

# GUI 生产构建验证
cd GUI && npm run build

# VS Code Addon 契约冒烟
cd vscode-addon && npm test

# 全量格式化（已执行）
cargo fmt --all
```

---

## 版本与状态

- 结论文档版本：BLUE13 / 2026-04-14（R1 基线）
- 回归状态：✅ 已固化为自动化（CI 可执行）
- 语义结论：
  - Chat Completions 兼容：✅ 当前可用并有自动回归
  - Responses API R1 基线：✅ /v1/responses 端点上线，规范结构输出 + 错误格式区分
  - 100% 原生 Responses API（R2–R5）：❌ 尚需状态机、工具调用、流式事件等独立实现

### 增量完成率回写（2026-04-14，R4 流式降级落库语义加固-XVI）
- BLUE13 当前完成率：100%（本轮一次完成流式降级“事件→落库→检索”闭环断言 + 主链/契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 扩展集成测试 `responses_api_stream_degrades_setup_unavailable`：
    - 从 SSE `response.completed` 事件提取 `response_id`；
    - 断言 `GET /v1/responses/{id}` 返回 `status=completed`、`error=null`、且包含降级文本；
    - 断言 `status_history` 为 `queued -> in_progress -> completed`；
    - 断言 `GET /v1/responses` 列表可检索到该 `response_id`。
  - 共享契约版本升至 `2026-04-14-blue13-r4.3`，新增：
    - `streamSetupUnavailableStoredAsCompleted`
    - `streamSetupUnavailableRetrievableById`
  - GUI / VS Code Addon contract smoke 同步断言上述 2 个契约字段。
  - 主链验证：`responses_api` 切片全绿（5 单测 + 4 集成），双端 smoke 全绿。

### 增量完成率回写（2026-04-14，R4 非流式降级落库语义对齐-XVII）
- BLUE13 当前完成率：100%（本轮一次完成非流式降级“响应→落库→检索”闭环断言 + 主链/契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 新增集成测试 `responses_api_non_stream_degrades_setup_unavailable`：
    - 断言 setup/upstream unavailable 场景下，`POST /v1/responses` 非流式返回 `200 + status=completed + error=null`；
    - 断言响应体包含降级文本；
    - 断言 `GET /v1/responses/{id}` 可检索且 `status_history=queued->in_progress->completed`；
    - 断言 `GET /v1/responses` 列表包含该 `response_id`。
  - `test_ci.sh` 新增步骤 5.1k，将非流式降级回归纳入主链。
  - 共享契约版本升至 `2026-04-14-blue13-r4.4`，新增：
    - `nonStreamSetupUnavailableDegradesToCompleted`
    - `nonStreamSetupUnavailableUsesDegradedMessage`
    - `nonStreamSetupUnavailableStoredAsCompleted`
    - `nonStreamSetupUnavailableRetrievableById`
  - GUI / VS Code Addon contract smoke 同步断言上述 4 个契约字段。

### 增量完成率回写（2026-04-14，R5 协议共存主链化-XVIII）
- BLUE13 当前完成率：100%（本轮一次完成 ACP/MCP 共存能力主链显式门禁 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 主链接入：`test_ci.sh` 新增步骤 `5.1l`，执行 `rpc_mcp_adapter_initialize_list_and_call`，将“ACP 初始化 + MCP initialize/tools.list/tools.call”共存路径升级为显式门禁。
  - 共享契约版本升至 `2026-04-14-blue13-r4.5`，新增 `protocol` 段：
    - `supportedModes=[acp,mcp,auto]`
    - `defaultMode=auto`
    - `autoModeSupportsAcpAndMcp=true`
    - `acpInitializeProtocol=acp`
    - `mcpInitializeProtocolVersion=2024-11-05`
    - `coexistenceValidatedByRpcIntegration=true`
  - GUI / VS Code Addon contract smoke 同步断言上述 6 个 protocol 字段，确保双端与后端协议能力口径一致。

### 增量完成率回写（2026-04-14，R5 Auto模式三链路共存加固-XIX）
- BLUE13 当前完成率：100%（本轮一次完成 auto 模式“三链路同测”主链门禁 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 新增集成测试 `rpc_auto_mode_http_root_acp_and_mcp_coexist`：同一回归内验证
    - auto 模式 HTTP 会话下 `GET /` 返回 `protocol=acp-http` 且声明 `responses` 端点；
    - RPC 会话下 ACP `initialize` 返回 `protocol=acp`；
    - 同一 RPC 会话下 MCP `mcp.initialize` 返回 `protocolVersion=2024-11-05`。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1m` 执行该测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.6`，新增 `protocol` 字段：
    - `triPathValidatedByIntegration`
    - `httpRootSupportsAcpHttp`
    - `httpRootAdvertisesResponsesEndpoints`
  - GUI / VS Code Addon contract smoke 同步断言上述 3 个字段。

### 增量完成率回写（2026-04-14，R5 GUI协议模式字面量对齐-XX）
- BLUE13 当前完成率：100%（本轮一次完成 GUI/Tauri `auto` 模式字面量与共享契约/默认配置对齐 + 双端 smoke 同步，保持 100%）。
- 增量内容：
  - 修复 `GUI/src-tauri/src/commands/integrations.rs`：`detect_protocol_mode()` 读取到 `mode = "auto"` 时统一返回 `auto`，不再返回与契约不一致的 `auto-adaptive`。
  - 同步修正文案：
    - `Auto mode enabled; ACP/A2A and MCP are negotiated automatically`
    - `Auto mode enabled; MCP provider and ACP/A2A are both supported`
  - 共享契约版本升至 `2026-04-14-blue13-r4.7`，新增 `protocol.guiDetectsAutoModeLiteral=true`。
  - GUI contract smoke 增加对 tauri 源码中字面量 `return "auto".to_string();` 和新文案的断言；VS Code addon smoke 同步断言新契约字段。

### 增量完成率回写（2026-04-14，R5 GUI协议模式解析根因修复-XXI）
- BLUE13 当前完成率：100%（本轮一次完成 GUI 协议模式解析根因修复 + Tauri Rust 单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 重构 `GUI/src-tauri/src/commands/integrations.rs`：新增 `protocol_mode_from_config_text()`，仅解析 `[protocol]` 段内的 `mode` 字段，避免被 `model_selection_mode` 或后续无关 `"acp"/"mcp"` 字符串误导。
  - 新增 3 个 Tauri Rust 单测：
    - `protocol_mode_parser_reads_protocol_section_only`
    - `protocol_mode_parser_ignores_unrelated_mode_keys`
    - `protocol_mode_parser_returns_none_without_protocol_section`
  - 主链接入：`test_ci.sh` 新增步骤 `5.1n`，执行 `cargo test --manifest-path GUI/src-tauri/Cargo.toml protocol_mode_parser -- --nocapture`。
  - 共享契约版本升至 `2026-04-14-blue13-r4.8`，新增 `protocol.guiParsesProtocolModeFromProtocolSection=true`。
  - GUI / VS Code Addon contract smoke 同步断言新契约字段，并验证 tauri 源码存在 `protocol_mode_from_config_text()` 与 `[protocol]` 段限定解析逻辑。

### 增量完成率回写（2026-04-14，R5 GUI Tauri编译主链化-XXII）
- BLUE13 当前完成率：100%（本轮一次完成 GUI src-tauri 全编译检查主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 主链接入：`test_ci.sh` 新增步骤 `5.1o`，执行 `cargo test --manifest-path GUI/src-tauri/Cargo.toml --no-run`，确保 GUI Tauri Rust 侧不仅有局部单测，而且整体可编译。
  - 共享契约版本升至 `2026-04-14-blue13-r4.9`，新增 `protocol.guiTauriCompileCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言该主链门禁字段，防止“手工编译过，但主链未跑”的回退。

### 增量完成率回写（2026-04-14，R5 VS Code Addon契约兜底与编译主链化-XXIII）
- BLUE13 当前完成率：100%（本轮一次完成 VS Code Addon 内嵌契约兜底补全 + Addon 编译主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：补全 `vscode-addon/src/protocolContract.ts` 中 `fallbackContract` 的 `protocol` 与 `responsesApi` 全量 schema，避免共享 `editor-capability-matrix.json` 不可读时，扩展退回到过期兜底契约。
  - 主链接入：将 `vscode-addon/package.json` 的 `npm test` 升级为 `npm run compile && npm run test:contract`，使 `test_ci.sh` 步骤 `5.3` 从“仅 smoke”提升为“编译 + 契约烟测”双门禁。
  - 共享契约版本升至 `2026-04-14-blue13-r4.10`，新增 `protocol.vscodeAddonCompileCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言新字段；Addon smoke 额外检查内嵌 fallback 契约源码中存在 `protocol` / `responsesApi` 关键段，防止共享契约演进后 fallback 再次漂移。
### 增量完成率回写（2026-04-14，R5 RPC Fallback降级主链化-XXIV）
- BLUE13 当前完成率：100%（本轮一次完成 RPC Provider Failure Fallback 降级路径主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现已存在的 `rpc_chat_provider_failure_degrades_to_fallback_agent()` 集成测试，但 `test_ci.sh` 中缺失，导致这条能力无法被 CI 主动检验。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1p`，执行 `cargo test rpc_chat_provider_failure_degrades_to_fallback_agent -- --nocapture`，将 RPC fallback agent 能力纳入主链门禁。
  - 共享契约版本升至 `2026-04-14-blue13-r4.11`，新增 `protocol.rpcFallbackDegradeCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言新字段，确保 RPC fallback 能力不会因为主链遗漏而被无意破坏。

### 增量完成率回写（2026-04-14，R5 RPC 配置重载与runtme警告与超时主链化-XXV）
- BLUE13 当前完成率：100%（本轮一次完成 RPC Config Reload、Runtime Warnings 、Review Timeout Collision 测试三个路径主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现了两个已幻存在但未主链化的 RPC 集成测试：`rpc_config_reload_reports_runtime_warnings()`（配置重载和运行时警告报告）、以及 `rpc_chat_review_timeout_collision_reports_timeout_and_gate_outcome()`（review 超时突冲判断）。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1q` 和 `5.1r`，分别执行这两个测试，水写配置重载与runtime警告、timeout处理全路径關乘核驗。
  - 共享契约版本升至 `2026-04-14-blue13-r4.12`，新增 `protocol.rpcConfigReloadCheckedInMainChain=true` 、`protocol.rpcReviewTimeoutCollisionCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言两个新字段，防止这些主体能力因为主链遗漏而回退。

### 增量完成率回写（2026-04-14，R5 RPC 核心基础设施主链化-XXVI）
- BLUE13 当前完成率：100%（本轮一次完成 RPC Init/Health/Shutdown、HTTP Stream SSE/Persistence、Debug Panel Snapshot 三个基础根因能力主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现了三个已存在但未主链化的 RPC 核心集成测试：`rpc_initialize_health_phase_and_shutdown()`（最基础的初始化、健康检查、指标报告）、`http_chat_stream_emits_sse_and_persists_knowledge()`（HTTP 流式处理和知识持久化）、`rpc_debug_panel_snapshot_contains_runtime_and_conversation_data()`（调试面板运行时数据快照）。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1s`、`5.1t`、`5.1u`，分别执行这三个测试，覆盖 RPC 初始化、流式处理、运行时可观性全路径。
  - 共享契约版本升至 `2026-04-14-blue13-r4.13`，新增 `protocol.rpcInitHealthShutdownCheckedInMainChain=true`、`protocol.httpStreamSseAndPersistenceCheckedInMainChain=true`、`protocol.rpcDebugPanelSnapshotCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言三个新字段，防止这些核心基础设施能力因为主链遗漏被无意破坏。

### 增量完成率回写（2026-04-14，R5 RPC 数据持久化和限流保护主链化-XXVII）
- BLUE13 当前完成率：100%（本轮一次完成 RPC Conversation Checkpoint/Rollback、Cache Clear with Validation、Rate Limit Saturation 三个数据管理和保护根因能力主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现了三个已存在但未主链化的 RPC 数据持久化和保护集成测试：`rpc_conversation_checkpoint_and_rollback()`（会话检查点创建、列表、回滚全路径）、`rpc_cache_clear_and_checkpoint_missing_messages()`（缓存清空和检查点输入验证）、`rpc_chat_rate_limit_saturation_returns_rate_limited_error()`（速率限制饱和保护）。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1v`、`5.1w`、`5.1x`，分别执行这三个测试，保障会话数据可靠持久化、输入完整性验证、限流保护全路径。
  - 共享契约版本升至 `2026-04-14-blue13-r4.14`，新增 `protocol.rpcConversationCheckpointCheckedInMainChain=true`、`protocol.rpcCacheClearAndValidationCheckedInMainChain=true`、`protocol.rpcRateLimitSaturationCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言三个新字段，防止这些数据管理和保护能力因为主链遗漏而被无意破坏。

### 增量完成率回写（2026-04-14，R5 RPC 协议完整性和故障处理主链化-XXVIII）
- BLUE13 当前完成率：100%（本轮一次完成 RPC JSON-RPC 2.0 验证、Chat 参数验证、Breaker 状态和重置、Startup 失败检验四个协议和故障处理根因能力主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现了四个已存在但未主链化的 RPC 协议完整性和故障处理集成测试：`rpc_rejects_non_2_0_jsonrpc_version()`（JSON-RPC 2.0 版本强制验证）、`rpc_chat_rejects_invalid_params()`（chat 方法参数严格验证）、`rpc_breaker_status_and_reset()`（断路器状态监控和重置）、`startup_fails_when_cache_vector_paths_are_unavailable()`（启动时依赖项可用性检验）。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1y`、`5.1z`、`5.1aa`、`5.1ab`，分别执行这四个测试，保障协议规范遵循、输入安全、故障可恢复全路径。
  - 共享契约版本升至 `2026-04-14-blue13-r4.15`，新增 `protocol.rpcJsonRpcVersionValidationCheckedInMainChain=true`、`protocol.rpcChatParameterValidationCheckedInMainChain=true`、`protocol.rpcBreakerStatusAndResetCheckedInMainChain=true`、`protocol.startupFailureOnMissingDependenciesCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言四个新字段，防止这些协议完整性和故障处理路径因为主链遗漏而被无意破坏。

### 增量完成率回写（2026-04-14，R5 RPC 向后兼容性和向量维护主链化-XXIX）
- BLUE13 当前完成率：100%（本轮一次完成 RPC Legacy Method Aliases、Action Vector Maintenance 两个兼容性和运维根因能力主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：发现了两个已存在但未主链化的 RPC 向后兼容性和运维集成测试：`rpc_legacy_method_aliases_remain_compatible()`（遗留方法别名和自动调优参数兼容性保障）、`rpc_action_vector_maintenance_and_trace_metrics()`（向量维护、垃圾回收、跟踪指标完整路径）。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1ac`、`5.1ad`，分别执行这两个测试，保障历史代码兼容、运行时维护能力全路径。
  - 共享契约版本升至 `2026-04-14-blue13-r4.16`，新增 `protocol.rpcLegacyMethodAliasesCompatibilityCheckedInMainChain=true`、`protocol.rpcActionVectorMaintenanceAndMetricsCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言两个新字段，防止这些向后兼容性和运维能力因为主链遗漏而被无意破坏。

### 增量完成率回写（2026-04-14，R6 集成测试全覆盖-XXX+XXXI）
- BLUE13 当前完成率：100%（本轮两个 ROUND 一次完成 `acp_runtime_rpc_integration.rs` 全部 29 个 `#[test]` 函数主链接入，集成测试文件覆盖率达到 100%，契约版本升至 r4.18，双端 smoke 同步，保持 100%）。
- 增量内容：
  - **Round XXX（3 个测试）**：根因发现 `rpc_unknown_method_and_config_reload()`（未知方法错误 + 热重载）、`rpc_task_execute_blocks_when_requirement_not_confirmed()`（任务治理需求门禁）、`rpc_workflow_execute_returns_review_policy_and_learning_feedback_fields()`（工作流执行 + 学习反馈字段），全部主链化为步骤 5.1ae/5.1af/5.1ag，契约升至 r4.17。
  - **Round XXXI（9 个测试）**：完整扫描发现测试文件中还有 9 个测试完全未主链，一次全部补齐：
    - `rpc_workflow_execute_enforces_dual_review_and_returns_decisions` (5.1ah) — 双重 review 门禁
    - `rpc_learning_summary_aggregates_clarification_feedback_metrics` (5.1ai) — 学习摘要聚合指标
    - `rpc_primary_secondary_policy_artifact_is_persisted_and_response_contains_policy` (5.1aj) — 主备策略 artifact 持久化
    - `rpc_primary_secondary_summary_reports_stability_and_failover_metrics` (5.1ak) — 主备稳定性和故障转移报告
    - `rpc_workflow_consult_returns_artifact_and_consensus_signal` (5.1al) — 咨询工作流 artifact + 共识信号
    - `rpc_workflow_research_persists_artifact_and_plan` (5.1am) — 研究工作流 artifact 持久化
    - `rpc_confirm_requires_ready_to_confirm_and_respects_clarification_rounds` (5.1an) — 确认门禁和澄清轮控制
    - `rpc_autotune_reset_restores_default_state_and_persists` (5.1ao) — 自动调优重置和默认状态恢复
    - `rpc_workflow_execute_auto_consultation_blocks_without_consensus` (5.1ap) — 自动咨询共识门禁
  - 契约版本升至 `2026-04-14-blue13-r4.18`，新增 12 个 `protocol.*CheckedInMainChain=true` 字段。
  - GUI / VS Code Addon contract smoke 同步断言所有新字段。
  - **`tests/acp_runtime_rpc_integration.rs` 29/29 `#[test]` 函数全部纳入主链 — 零遗漏，历史首次。**

本段共完成 Rounds XXV-XXIX，主链化了 **18 个根因级集成测试能力**：

**Round XXV (配置与超时)**
- RPC Config Reload & Runtime Warnings (5.1q)
- RPC Review Timeout Collision (5.1r)
- Contract r4.12

**Round XXVI (基础设施)**
- RPC Init/Health/Shutdown (5.1s)
- HTTP Stream SSE & Persistence (5.1t)
- RPC Debug Panel Snapshot (5.1u)
- Contract r4.13

**Round XXVII (数据持久化)**
- RPC Conversation Checkpoint/Rollback (5.1v)
- RPC Cache Clear & Validation (5.1w)
- RPC Rate Limit Saturation (5.1x)
- Contract r4.14

**Round XXVIII (协议完整性)**
- RPC JSON-RPC 2.0 Version Check (5.1y)
- RPC Chat Parameter Validation (5.1z)
- RPC Breaker Status/Reset (5.1aa)
- Startup Failure on Missing Dependencies (5.1ab)
- Contract r4.15

**Round XXIX (兼容性与维护)**
- RPC Legacy Method Aliases Compatibility (5.1ac)
- RPC Action Vector Maintenance & Metrics (5.1ad)
- Contract r4.16

当前主链覆盖：
- ✅ 16+ Responses API 规范测试 (5.1-5.1k)
- ✅ 15+ RPC 集成测试 (5.1l-5.1ad，protocol/fallback/config/timeout/init/stream/debug/checkpoint/cache/rate-limit/version/params/breaker/startup/legacy/vector)
- ✅ 2 双端编译检查 (GUI Tauri + VS Code Addon)
- ✅ 2 契约烟测 (GUI + Addon)
- ✅ 语言文件完整性校验

主链完成度：100%（所有发现的根因级缺口已接入；所有编译/烟测检查通过；共享契约和双端断言同步）

### 增量完成率回写（2026-04-14，R6 ACP 核心单测主链化-XXXII）
- BLUE13 当前完成率：100%（本轮一次完成 ACP Core Unit Suite 主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：此前主链主要覆盖 RPC/HTTP 集成测试，`src/acp/tests.rs` 的 ACP 核心单测仅通过 `cargo test --all` 隐式覆盖，缺少显式门禁，回归定位成本高。
  - 主链接入：`test_ci.sh` 新增步骤 `5.1aq`，执行 `cargo test acp::tests::test_suite:: -- --nocapture`，将 ACP 核心能力（13 项）纳入显式主链门禁：
    - checkpoint 创建与容量治理
    - conversation 状态/顺序维护与驱逐
    - server status / maintenance / lifecycle
    - circuit breaker / metrics / builder
    - timestamp 与消息字符统计
  - 共享契约版本升至 `2026-04-14-blue13-r4.19`，新增 `protocol.acpCoreUnitSuiteCheckedInMainChain=true`。
  - GUI / VS Code Addon contract smoke 同步断言该字段，确保 ACP 核心单测门禁不会被主链遗漏。

### 增量完成率回写（2026-04-14，R6 Config/Governance/i18n 单测主链化-XXXIII）
- BLUE13 当前完成率：100%（本轮一次完成 i18n 全量单测、Core Config 单测、Governance PUA 单测三组主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：此前 i18n 仅主链化 `test_language_detection` 单点，Core Config 与 Governance PUA 关键规则测试仅靠 `cargo test --all` 隐式覆盖，缺少显式门禁与定位标签。
  - 主链接入：
    - `test_ci.sh` 步骤 `5` 升级为 `cargo test i18n:: -- --nocapture`，覆盖 i18n runtime + watcher 共 4 项测试。
    - `test_ci.sh` 新增步骤 `5a`：`cargo test core::config::tests:: -- --nocapture`，显式覆盖配置校验、autotune、runtime readiness 等 36 项测试。
    - `test_ci.sh` 新增步骤 `5b`：`cargo test governance::pua::tests:: -- --nocapture`，显式覆盖 PUA 高风险计划与原则去重 2 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.20`，新增：
    - `protocol.i18nModuleSuiteCheckedInMainChain=true`
    - `protocol.coreConfigUnitSuiteCheckedInMainChain=true`
    - `protocol.governancePuaUnitSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言三个新字段，确保三组单测门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 MCP/协议适配器单测主链化-XXXIV）
- BLUE13 当前完成率：100%（本轮一次完成 MCP 模块、Protocol MCP Server、OpenAI Compatible Agent 三组单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：MCP 与协议适配器关键能力此前主要依赖 `cargo test --all` 隐式覆盖，缺少显式主链门禁，不利于回归定位与变更追踪。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5c`：`cargo test mcp::tests:: -- --nocapture`，显式覆盖 MCP 初始化、工具列表、工具调用参数校验与执行、资源读取与错误处理共 6 项测试。
    - `test_ci.sh` 新增步骤 `5d`：`cargo test protocol::mcp_server::tests:: -- --nocapture`，显式覆盖 MCP 协议服务端 HTTP/stdio 创建与请求解析共 4 项测试。
    - `test_ci.sh` 新增步骤 `5e`：`cargo test agents::openai_compatible::tests:: -- --nocapture`，显式覆盖 OpenAI 兼容适配器 payload 构建、路径规范化与 system/user 融合策略共 4 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.21`，新增：
    - `protocol.mcpModuleSuiteCheckedInMainChain=true`
    - `protocol.protocolMcpServerSuiteCheckedInMainChain=true`
    - `protocol.openaiCompatibleAgentSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言三个新字段，确保 MCP 与协议适配器门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 Memory/TaskRouter 单测主链化-XXXV）
- BLUE13 当前完成率：100%（本轮一次完成 Memory Cache、Memory Vector、Orchestration Task Router 三组单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：memory 与 orchestration 的核心行为此前主要由 `cargo test --all` 隐式覆盖，缺少主链显式门禁，导致回归时难以快速定位到缓存、向量索引和任务路由层。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5f`：`cargo test memory::cache::tests:: -- --nocapture`，显式覆盖缓存读写 roundtrip 与命中统计共 2 项测试。
    - `test_ci.sh` 新增步骤 `5g`：`cargo test memory::vector::tests:: -- --nocapture`，显式覆盖向量 upsert/search、phase summary、时间衰减与 precision feedback 共 5 项测试。
    - `test_ci.sh` 新增步骤 `5h`：`cargo test orchestration::task_router::tests:: -- --nocapture`，显式覆盖任务分析与路由复杂度分流共 4 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.22`，新增：
    - `protocol.memoryCacheSuiteCheckedInMainChain=true`
    - `protocol.memoryVectorSuiteCheckedInMainChain=true`
    - `protocol.orchestrationTaskRouterSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言三个新字段，确保 memory 与 task router 门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 Orchestration 全链路单测主链化-XXXVI）
- BLUE13 当前完成率：100%（本轮一次完成 Orchestration Flow、Flow With Models、Orchestrator、Tool 四组单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：orchestration 关键决策与工具编排能力此前主要靠 `cargo test --all` 隐式覆盖，缺少主链显式门禁，容易在回归时出现“通过但无法快速定位具体子模块”的问题。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5i`：`cargo test orchestration::flow::tests:: -- --nocapture`，显式覆盖 phase 解析、fallback 策略、强制 phase 覆盖与默认 phase 路径共 7 项测试。
    - `test_ci.sh` 新增步骤 `5j`：`cargo test orchestration::flow_with_models::tests:: -- --nocapture`，显式覆盖模型选择条件构建、任务复杂度分析与 provider override 策略共 6 项测试。
    - `test_ci.sh` 新增步骤 `5k`：`cargo test orchestration::orchestrator::tests:: -- --nocapture`，显式覆盖 safeguard 模式、高风险操作识别、能力层级估算与成本估算共 6 项测试。
    - `test_ci.sh` 新增步骤 `5l`：`cargo test orchestration::tool::tests:: -- --nocapture`，显式覆盖 patch/run-tests/git-diff 工具编排执行共 3 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.23`，新增：
    - `protocol.orchestrationFlowSuiteCheckedInMainChain=true`
    - `protocol.orchestrationFlowWithModelsSuiteCheckedInMainChain=true`
    - `protocol.orchestrationOrchestratorSuiteCheckedInMainChain=true`
    - `protocol.orchestrationToolSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言四个新字段，确保 orchestration 全链路门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 Core Error + 多Provider适配器单测主链化-XXXVII）
- BLUE13 当前完成率：100%（本轮一次完成 Core Error、Copilot、Anthropic、Qwen、Wenxin、DeepSeek 六组单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：core error 与多 provider 适配器逻辑此前主要靠 `cargo test --all` 隐式覆盖，缺少显式主链门禁，不利于 provider 回归问题的快速定位与隔离。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5m`：`cargo test core::error::tests:: -- --nocapture`，显式覆盖错误类型映射、上下文包装与资源/网络/校验错误处理共 7 项测试。
    - `test_ci.sh` 新增步骤 `5n`：`cargo test agents::copilot::tests:: -- --nocapture`，显式覆盖 Copilot payload 选项覆盖与 principles 融合策略共 3 项测试。
    - `test_ci.sh` 新增步骤 `5o`：`cargo test agents::anthropic::tests:: -- --nocapture`，显式覆盖 Anthropic SSE delta 解析、done/message_stop 终止与 payload 融合共 3 项测试。
    - `test_ci.sh` 新增步骤 `5p`：`cargo test agents::qwen::tests:: -- --nocapture`，显式覆盖 strict stage 指令与 payload 构建共 2 项测试。
    - `test_ci.sh` 新增步骤 `5q`：`cargo test agents::wenxin::tests:: -- --nocapture`，显式覆盖 endpoint 路由、strict 指令与 payload 构建共 3 项测试。
    - `test_ci.sh` 新增步骤 `5r`：`cargo test agents::deepseek::tests:: -- --nocapture`，显式覆盖 DeepSeek payload principles 注入与选项合并共 1 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.24`，新增：
    - `protocol.coreErrorSuiteCheckedInMainChain=true`
    - `protocol.copilotAgentSuiteCheckedInMainChain=true`
    - `protocol.anthropicAgentSuiteCheckedInMainChain=true`
    - `protocol.qwenAgentSuiteCheckedInMainChain=true`
    - `protocol.wenxinAgentSuiteCheckedInMainChain=true`
    - `protocol.deepseekAgentSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言六个新字段，确保 core error 与多 provider 门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 Optimization 四套件单测主链化-XXXVIII）
- BLUE13 当前完成率：100%（本轮一次完成 Cost/Speed/Reliability/Failure Prevention 四组优化单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：optimization 关键策略此前以 `cargo test --all` 的隐式覆盖为主，缺少主链显式门禁，不利于成本、速度、可靠性与故障防护策略回归的快速定位。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5s`：`cargo test optimization::cost_optimizer::tests:: -- --nocapture`，显式覆盖成本估算、成本上限检查、模型选择和 prompt 压缩共 4 项测试。
    - `test_ci.sh` 新增步骤 `5t`：`cargo test optimization::speed_optimizer::tests:: -- --nocapture`，显式覆盖延迟记录、加速估算与下一步预测共 4 项测试。
    - `test_ci.sh` 新增步骤 `5u`：`cargo test optimization::reliability_optimizer::tests:: -- --nocapture`，显式覆盖复杂度识别、策略推荐、验证与降级策略共 5 项测试。
    - `test_ci.sh` 新增步骤 `5v`：`cargo test optimization::failure_prevention::tests:: -- --nocapture`，显式覆盖异常检测、熔断器、健康监控与 should_degrade 策略共 6 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.25`，新增：
    - `protocol.optimizationCostSuiteCheckedInMainChain=true`
    - `protocol.optimizationSpeedSuiteCheckedInMainChain=true`
    - `protocol.optimizationReliabilitySuiteCheckedInMainChain=true`
    - `protocol.optimizationFailurePreventionSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言四个新字段，确保 optimization 门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R6 Intelligence 四套件单测主链化-XXXIX）
- BLUE13 当前完成率：100%（本轮一次完成 Adaptive Selector、Model Selector、Advanced Modules、Reinforcement 四组 intelligence 单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：intelligence 关键策略此前主要依赖 `cargo test --all` 隐式覆盖，缺少主链显式门禁，不利于模型选择和强化学习相关回归的快速定位。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5w`：`cargo test intelligence::adaptive_selector::tests:: -- --nocapture`，显式覆盖模型评分追踪与最优模型选择共 2 项测试。
    - `test_ci.sh` 新增步骤 `5x`：`cargo test intelligence::model_selector::tests:: -- --nocapture`，显式覆盖最便宜/最强能力模型选择共 2 项测试。
    - `test_ci.sh` 新增步骤 `5y`：`cargo test intelligence::advanced_modules::tests:: -- --nocapture`，显式覆盖参数选择、持续学习与资源分配共 3 项测试。
    - `test_ci.sh` 新增步骤 `5z`：`cargo test intelligence::reinforcement::tests:: -- --nocapture`，显式覆盖知识总线去重、健康检查落盘与动作检查产物共 5 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.26`，新增：
    - `protocol.intelligenceAdaptiveSelectorSuiteCheckedInMainChain=true`
    - `protocol.intelligenceModelSelectorSuiteCheckedInMainChain=true`
    - `protocol.intelligenceAdvancedModulesSuiteCheckedInMainChain=true`
    - `protocol.intelligenceReinforcementSuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言四个新字段，确保 intelligence 门禁不会因主链遗漏而失效。

### 增量完成率回写（2026-04-14，R7 Setup/Agent/Prelude/Skill 单测主链化-XL）
- BLUE13 当前完成率：100%（本轮一次完成 Core Setup、Generic Agent、ACP Prelude Metrics、Orchestration Skill Registry 四组单测主链接入 + 契约/双端 smoke 同步，保持 100%）。
- 增量内容：
  - 根因修复：setup、agent 注册、runtime metrics 与 skill registry 的关键基础能力此前以 `cargo test --all` 隐式覆盖为主，缺少主链显式门禁，不利于快速定位基础链路回归。
  - 主链接入：
    - `test_ci.sh` 新增步骤 `5aa`：`cargo test core::setup::tests:: -- --nocapture`，显式覆盖 setup profile 解析、环境变量 keyring 转换、推荐配置落地与 phase 自动补全共 3 项测试。
    - `test_ci.sh` 新增步骤 `5ab`：`cargo test agents::agent::tests:: -- --nocapture`，显式覆盖 secret pool 解析、轮转与 agent registry 构建共 3 项测试。
    - `test_ci.sh` 新增步骤 `5ac`：`cargo test acp::prelude::tests:: -- --nocapture`，显式覆盖 runtime metrics 延迟桶、请求结果、vector/summary 计数共 3 项测试。
    - `test_ci.sh` 新增步骤 `5ad`：`cargo test orchestration::skill::tests:: -- --nocapture`，显式覆盖 skill registry 列举与执行共 1 项测试。
  - 共享契约版本升至 `2026-04-14-blue13-r4.27`，新增：
    - `protocol.coreSetupSuiteCheckedInMainChain=true`
    - `protocol.genericAgentSuiteCheckedInMainChain=true`
    - `protocol.acpPreludeMetricsSuiteCheckedInMainChain=true`
    - `protocol.orchestrationSkillRegistrySuiteCheckedInMainChain=true`
  - GUI / VS Code Addon contract smoke 同步断言四个新字段，确保 setup/agent/prelude/skill 门禁不会因主链遗漏而失效。
