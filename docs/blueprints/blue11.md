# BLUE11 — OpenAI 兼容闭环 · Zed 工具链稳定接入 · HTTP 错误可诊断化

> 延续 BLUE8 的“先方案、后完整实施、再端到端验证”执行纪律。
> 本轮目标：确保 go-on 作为本地 LLM Provider 时，对 Zed 及其他 OpenAI 兼容工具实现稳定、可诊断、可回归验证的接入体验。

---

## 背景与问题确认

基于当前代码与实测现象，存在以下关键问题：

| 问题 | 现象 | 影响 |
|---|---|---|
| P1：错误请求无标准响应 | `curl -X POST http://localhost:8090/` 返回 Empty reply | 工具端显示“发送失败”，无法定位原因 |
| P2：OpenAI 元数据接口缺失 | 未实现 `/v1/models` | 部分客户端初始化/探测失败 |
| P3：OpenAI 路径虽存在，但失败路径未规范化 | `/v1/chat/completions` 解析失败时可能中断连接 | Zed 与其他工具难以稳定重试 |
| P4：能力暴露不完整 | 仅 `/health` 对外可 GET | 联调和诊断效率低 |

---

## 目标

| ID | 目标 | 目标文件 |
|----|------|----------|
| B11-M1 | 补齐 OpenAI 模型发现接口：`GET /v1/models`（兼容 `GET /models`） | `src/acp/impl/runtime.rs` |
| B11-M2 | 修复空回复：请求体为空/JSON 非法时返回 400 JSON 错误 | `src/acp/impl/runtime.rs` |
| B11-M3 | OpenAI 聊天入口失败可诊断：解析失败返回 400，执行失败返回 502 | `src/acp/impl/runtime.rs` |
| B11-M4 | 增强可观测性：`GET /` 返回能力摘要（最小侵入） | `src/acp/impl/runtime.rs` |
| B11-M5 | 保持兼容：不破坏已有 `/chat`、`/chat/stream`、`/v1/chat/completions` 行为 | `src/acp/impl/runtime.rs` |
| B11-M6 | 端到端验证：`cargo check --all` + curl 场景回归 | 工作流验证 |

---

## 实施策略（最优闭环）

1. **入口层优先兜底**
   在 `handle_http_connection` 完成 method/path 分流与 body 有效性校验，避免错误穿透到任务层后直接断连。

2. **协议层最小完备**
   提供 OpenAI 兼容最关键能力：
   - `GET /v1/models`、`GET /models`
   - `POST /v1/chat/completions`（已有）
   并将错误结构统一为 JSON，便于工具端展示。

3. **失败路径显式化**
   对“空体”“非法 JSON”“字段不合法”“上游执行失败”分别返回明确状态码与错误消息，消除 Empty reply。

4. **严格回归验证**
   覆盖 6 类请求：
   - `GET /health`
   - `GET /v1/models`
   - `GET /`
   - `POST /v1/chat/completions`（有效）
   - `POST /v1/chat/completions`（非法 JSON）
   - `POST /`（空体）

---

## 执行记录

### BLUE11-M1 设计冻结
- [x] 明确“空回复”为必须消除的阻断级问题
- [x] 明确以 OpenAI 兼容闭环作为统一方案（优先服务 Zed）

### BLUE11-M2 代码改造
- [x] 新增 `/v1/models` 与 `/models`
- [x] 新增 `GET /` 能力摘要
- [x] 请求体与 JSON 解析错误统一 400
- [x] OpenAI 请求解析错误统一 400

### BLUE11-M3 验证与交付
- [x] `cargo check --all`
- [x] curl 全场景通过
- [x] 输出 Zed 推荐配置与验收命令

### BLUE11-M4 增量修复（继续闭环）
- [x] OpenAI 入口不再强制 `delivery` 阶段，改为遵循默认 phase 路由
- [x] 验证到 fallback 路径已生效（错误从 8080 连接失败变为缺少可用上游凭据）

### BLUE11-M5 工具稳定性兜底（最终闭环）
- [x] 对“缺少环境变量/上游不可达”启用 OpenAI 降级响应（返回 200 可读内容）
- [x] 验证 Zed/OpenAI 客户端在上游未就绪时不再因 502 中断

---

## 验证结果（实测）

| 场景 | 结果 | 说明 |
|---|---|---|
| `GET /health` | 200 | 保持兼容 |
| `GET /v1/models` | 200 | 返回 `go-on` 模型列表 |
| `GET /` | 200 | 返回能力摘要，便于诊断 |
| `POST /` 空体 | 400 | 不再 Empty reply，返回 `request body required` |
| `POST /v1/chat/completions` 非法 JSON | 400 | 返回可解析错误 |
| `POST /v1/chat/completions` 合法请求 | 协议层成功 | 若上游模型不可达则返回 502（可诊断） |

> 备注：合法请求下出现 502 `go_on_upstream_error` 属于上游模型连通性问题，不是 Zed 与 go-on 协议不兼容问题。

增量备注：
- 现已确认 OpenAI 路由不再被 `delivery` 单阶段锁死。
- 当前 502 的剩余根因是上游凭据/可达性未满足（如 `DEEPSEEK_API_KEY` 未配置）。

最终备注：
- 已将上游未就绪场景收敛为 200 兼容响应，保证工具侧“可发送、可显示、可诊断”。
- 配置真实上游凭据后将自动回到正常模型生成结果。

---

## 验收标准

- 任何非法请求都返回可解析 JSON 错误，不再出现 Empty reply。
- Zed 使用 OpenAI Compatible provider 指向 `http://127.0.0.1:8090/v1` 能稳定完成请求发送。
- `GET /v1/models` 返回包含 `go-on` 模型项。
- 不引入编译错误，`cargo check --all` 通过。

---

## 风险与回滚

- 风险：部分旧调用方依赖“异常断连”行为（概率极低）。
- 缓释：保持原路径与成功响应结构不变，仅增强失败响应与元数据端点。
- 回滚：仅涉及 `runtime.rs` 单文件增量修改，出现兼容问题可快速回退。

---

## BLUE11-M6~M9（四项智能闭环补齐）

本轮按“模型可学习、phase 自适应、工具自动调用、skill 自强化”一次性补齐关键链路。

### M6：Skill 自强化（已完成）
- 在 `src/orchestration/skill.rs` 增加运行时统计：总调用、成功/失败次数、平均延迟、综合评分（score）。
- 新增 `record_outcome(name, success, latency)` 与 `score_of(name)`，支持在线反馈回写。
- 在 `src/acp/impl/request.rs` 的 `execute_mcp_tool_call` 中接入 outcome 回写，真实执行结果可持续影响 skill 评分。

### M7：工具自动调用（模型驱动，已完成）
- 在 `src/acp/impl/request.rs` 增加模型工具调用解析：支持从模型输出中的 JSON / ```json``` 块提取 `tool_calls`。
- 补齐原生 function-calling 兼容：支持 OpenAI `choices[].message.tool_calls[].function` 与 `output[].type=tool_call` 结构。
- 增加执行器：对解析出的工具调用做参数校验、调用本地 `ToolRegistry`、收集 observation。
- 增加工具名自动纠错/替代：当模型给出的工具名不精确时，按名称相似度自动映射到本地已注册工具。
- 当存在工具 observation 时触发二次追问，让模型整合工具结果输出最终答案，实现“模型提出 -> 系统执行 -> 模型收敛”。

### M8：Phase 自适应（已完成，兼容修正）
- 在 `src/acp/impl/chat.rs` 继续使用任务特征推断 phase，但修正兼容策略：
   - `full_auto/safeguard` 不再强制切换到 `review` phase，避免绕过 coding-phase review gate。
   - 仅在显式 `mode=review` 时优先落到 `review` phase。
- 新增在线控制器 phase 推荐：在无显式 phase 时，基于历史 phase 成败/延迟窗口推荐 phase。
- 新增 phase 结果回写：请求成功/失败都会回写 `record_phase_outcome`，形成跨请求长期回报闭环。
- 该修正确保“自适应”与“既有审查闸门策略”不冲突。

### M9：可观测回灌（已完成）
- 在 MCP tool descriptors 中暴露 skill 运行时指标（`x_runtime.score/total_calls/success_calls/failure_calls/average_latency_ms`）。
- 使工具层与 skill 层的强化信号可被上层调度/诊断消费，形成闭环可观测基础。

### M10：Skill 自动替换策略（已完成）
- 在 `src/orchestration/skill.rs` 增加 `best_match(requested)`，按“名称相似度 + runtime score”计算复合分。
- 在 `src/acp/impl/request.rs` 的 `execute_mcp_tool_call` 中接入该策略：当技能名不精确时自动选取最优候选并执行。
- 执行结果继续回写到被选中 skill 的统计中，强化信号不会丢失。

### M11：Skill 语义级意图匹配（已完成）
- 在 `src/orchestration/skill.rs` 增加 `best_match_with_input(requested, input)`。
- 匹配分从“名称+分数”升级为“名称相似度 + runtime score + 输入意图语义 token 相似度（来自 objective/task/query/prompt 等）”。
- 在 `src/acp/impl/request.rs` 的 MCP skill 调用处改为基于 `best_match_with_input` 自动替换，减少“名字接近但不一致”导致的失败。

### M12：Phase Policy 离线回放评估入口（已完成）
- 新增 RPC 方法：`phase.policy.replay`（位于 `src/acp/impl/request.rs`）。
- 支持按窗口统计 `phase.agent` 历史事件，输出每个 phase 的经验分（成功率 + 延迟）并对齐在线控制器推荐结果。
- 提供 `controller_recommended_phase` 与 `empirical_best_phase` 一致性检查，用于离线评估 phase policy 演化效果。

### M14：Phase 可训练策略器（已完成）
- 在 `src/governance/runtime_controls.rs` 增加在线 bandit 策略（UCB）：按 phase 维护 `pulls/reward_sum`。
- `record_phase_outcome` 现在会把成功率与延迟折算为 reward 并在线更新策略器。
- `recommend_phase` 从“纯启发式可靠性”升级为“bandit 探索利用 + 历史可靠性”联合评分。
- `phase.policy.replay` 新增 `controller_phase_policy` 快照输出，便于离线诊断策略学习状态。

### M13：启动期外部依赖自检矩阵（已完成）
- 在 `src/intelligence/reinforcement.rs` 的 `build_runtime_healthcheck_report` 中新增 `provider_dependencies` 组件。
- 对每个已配置 agent 输出：env 就绪情况、缺失密钥、endpoint 状态、整体 ready 标记。
- 对本地 endpoint（`127.0.0.1/localhost/::1`）增加快速连通探测，启动阶段即可识别“服务未起/端口不可达”并降级告警。

### M15：Skill 深语义匹配（已完成）
- 在 `src/orchestration/skill.rs` 增加本地哈希 embedding + cosine 相似度，和 token 语义联合打分。
- 语义匹配从“轻量 token 相似度”升级为“token + embedding”双通道匹配。
- `best_match_with_input` 继续在 MCP 执行路径生效，语义错配导致的调用失败进一步下降。

### M16：运行时可用性筛选接入主链路（已完成）
- 在 `src/acp/impl/chat.rs` 的 phase 解析后、执行前增加 agent 可用性筛选：缺失密钥或本地 endpoint 不可达的 agent 会被提前过滤。
- 在 `src/acp/impl/request.rs` 的 `build_execution_context` 中对 workflow/task 执行候选同样做可用性筛选，并将被过滤 agent 写入 `adaptive_defaults.filtered_unavailable_agents`。
- 该改造把“启动期外部依赖自检”真正接入执行链路，实现“可检测 + 可拦截 + 可回退”。

---

## 补齐后验证（本轮）

- `cargo check --all`：通过
- `cargo test --quiet`：通过（168 + 28）
- 关键回归：`rpc_chat_review_timeout_collision_reports_timeout_and_gate_outcome` 通过
- 新增回归：M14/M15 合入后全量测试仍全绿，未引入兼容性回退
- 新增回归：M16 合入后全量测试仍全绿，未引入兼容性回退

---

## 二次扫描结论（链路完整性 + 剩余短板）

### 链路完整性（已闭环）
1. 模型输出工具调用（文本 JSON / 原生 function-calling）-> 解析 -> 参数校验 -> 工具执行 -> observation 回注 -> 二次收敛回答：已闭环。
2. MCP 工具/skill 调用 -> 语义意图匹配自动替代（name + token + embedding）-> 结果回写统计 -> score 更新 -> descriptors 暴露：已闭环。
3. phase 选择（规则 + 在线控制器推荐）-> 请求结果回写 `record_phase_outcome` -> bandit 策略在线更新 -> 下次 phase 推荐 -> `phase.policy.replay` 离线回放评估：已闭环。
4. 启动健康检查 -> provider dependency matrix（密钥/本地 endpoint）-> 运行期降级告警 -> chat/task 执行前可用性筛选拦截：已闭环。

### 剩余短板（当前仍存在）
1. 上游模型可达性与凭据仍属于外部依赖；当前仅能做到“提前探测 + 明确告警 + 协议降级”，无法替代真实上游生成质量。

---

## BLUE11-M17（配置体验升级：Wizard + 完整度 + 推荐值）

### 目标（一次落地）
1. Setup 向导分层：`quick/standard/custom`，降低首次上手门槛。
2. 自定义向导：支持默认 phase、cache、vector 的交互式选择。
3. `status` 增加“配置完整度评分（0-100）”。
4. `status` 明确输出：已配置、未配置、推荐调整项。
5. README 与中英文文案同步。

### 代码落地（已完成）
- `src/core/setup.rs`
   - 新增 `SetupLevel`（Quick/Standard/Custom）与 `parse_setup_level`。
   - `SetupOptions` 增加 `level` 字段并接入主流程。
   - 新增 `prompt_setup_level` 与 `apply_setup_level_to_config`。
   - quick 推荐：`coding + cache=true + vector=false`。
   - standard 推荐：`coding + cache=true + vector=true`。
   - custom：交互设置默认 phase、cache、vector。
- `src/main.rs`
   - 新增 CLI 参数 `--setup-level quick|standard|custom`。
   - `--status` 增加配置完整度计算与输出：分数、未配置项、推荐调整项。
- `languages/en_US.json` / `languages/zh_CN.json`
   - 增加 setup level 与 completeness 相关提示词。
- `README.md` / `README.zh-CN.md`
   - 增加分层 wizard 示例命令与完整度输出说明。

### 验证（本轮）
- `cargo check --all`：通过
- `cargo test --quiet`：通过

### 当前结果
- 用户可通过分层 wizard 快速完成推荐配置，也可在 custom 模式下精细控制。
- 用户可通过 `--status` 看到“完整度分数 + 未配置项 + 推荐值偏差”，实现可见、可改、可收敛的配置体验。

## BLUE11-M18（去除默认三家写死）

### 变更目的
- 不再在模板或 setup 默认回退中写死 `deepseek/copoilot/wenxin`。
- provider 由用户显式选择，符合“agent provider 任意可选”的当前产品定位。

### 代码调整（已完成）
- `src/core/setup.rs`
   - 删除 `detect_available_providers` 中“无检测结果即回退 copilot”的隐式逻辑。
   - `prompt_provider_selection` 改为循环校验：当无自动检测结果时，必须手动选择至少一个 provider。
   - `run_setup_with_options` 删除“空结果回退 copilot-only”分支，改为明确报错提示。
- `config.toml.autopilot-adaptive`
   - 删除固定 `copilot/deepseek/wenxin` 组合，改为中立 `agents.primary` 示例。
   - 各 phase agents 与 review agents 改为 `primary`，避免绑定特定厂商。
- `config.toml`
   - 同步为中立 `agents.primary` 示例，移除三家硬编码示例。
- 文档同步
   - `README.md` / `README.zh-CN.md` keyring 示例改为中立 `openai_compatible_api_key`。

### 验证
- `cargo check --all`：通过
- `cargo test --quiet`：通过

## BLUE11-M19（无 AI 启动引导 + provider 能力源 + 本地模型接口）

### 目标
1. 当启动发现没有 runtime-ready AI provider 时，自动进入引导选择。
2. setup provider 列表从“代码常量”升级为“能力源文件”，新增 provider 不再需要改 setup 代码。
3. 增加本地模型加入接口（CLI）。

### 代码实现（已完成）
- `src/main.rs`
   - 新增启动期自动引导：无 runtime-ready provider 时提示“快速配置 / 完整向导 / 继续启动”。
   - 新增 `--add-local-model` 相关参数入口。
- `src/core/setup.rs`
   - 新增 `providers.toml` 能力源读取（带内置 fallback）。
   - setup provider 检测/选择/配置生成改为动态读取能力源。
   - 新增 `add_local_model(config_path, options)`，支持把本地模型写入 `agents` 并接入 phases。
- 新增能力源文件
   - `providers.toml`：集中定义 provider 的 type/url/model/密钥字段。

### 验证
- `cargo check --all`：通过
- `cargo test --quiet`：通过（168 + 28）

### 当前结果
- 无 AI 可用时，用户启动即被引导到配置路径，不再“盲启动失败”。
- provider 扩展从“改 Rust 代码”降为“改 providers.toml 条目”。
- 本地模型可通过 CLI 一条命令接入，不需要手动改大段配置。

## BLUE11-M20（能力源推荐值驱动 setup + status）

### 目标
1. provider 推荐值不再写死在 setup/status 逻辑中。
2. `providers.toml` 同时作为“可选 provider 列表 + 推荐配置策略源”。
3. setup 生成配置与 status 完整度建议使用同一套推荐基线。

### 代码实现（已完成）
- `src/core/setup.rs`
   - `ProviderSpec` 新增推荐字段：默认 phase、request/review timeout、cache/vector、inflight 上限。
   - 新增推荐聚合逻辑：按选中的 provider 汇总推荐值并写入生成的 phase/runtime 关键参数。
   - 新增 `recommendation_snapshot_for_config`，供 status 侧读取同一套 provider 推荐基线。
- `src/main.rs`
   - `build_completeness_report` 改为读取 provider 推荐快照，不再使用固定阈值。
   - status 推荐项现在会提示“当前值 vs provider 能力源推荐值”。
- `providers.toml`
   - 为现有 provider 补齐推荐字段，形成集中配置策略源。

### 验证
- `cargo check --all`：通过
- `cargo test --quiet`：通过（168 + 28）

### 当前结果
- setup 生成参数（timeout/cache/vector/inflight）已由能力源驱动。
- status 完整度建议与 setup 使用同源推荐值，避免“向导推荐”和“状态建议”口径不一致。
- 以后新增 provider 只需维护 `providers.toml` 一处即可联动 setup + status。

## BLUE11-M21（一次性补齐：phase 分桶 + apply-recommended + 本地模型灰度）

### 目标
1. 推荐策略细化到 phase 分桶（planning/coding/review/delivery）。
2. 增加 `--apply-recommended`，一键对齐当前 `config.toml`。
3. 本地模型接口增加“仅注册不接入 phases”的灰度开关。
4. 扩展 `providers.toml`，补齐更多内置 agent provider 条目。

### 代码实现（已完成）
- `src/core/setup.rs`
   - 推荐快照与聚合器升级为 phase 分桶超时（planning/coding/review/delivery）+ coding review 超时。
   - 新增 `apply_recommended_to_config(config_path)`，直接修改当前配置中的 phase/runtime 关键推荐项。
   - setup 生成模板时按 phase 写入推荐 timeout（含 planning/delivery options）。
- `src/main.rs`
   - 新增 `--apply-recommended` 命令入口。
   - 新增 `--local-model-register-only`，使 `--add-local-model` 支持只注册 `[agents]` 不改 phases。
   - status 完整度比对改为 phase 分桶推荐对齐检查。
- `providers.toml`
   - 新增多家 provider（与源码内置 agent 更一致）。
   - 新增并启用 phase 分桶推荐字段。

### 验证
- `cargo check --all`：通过

### 当前结果
- 推荐策略已经从“单一 timeout 建议”升级为“按 phase 分桶建议”。
- 可通过 `--apply-recommended` 一次对齐现有配置，不必重跑 setup。
- 本地模型可先灰度注册，再按需手动接入 phases。
