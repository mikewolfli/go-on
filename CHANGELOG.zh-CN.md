# 更新日志

## [1.5.3] - 2026-08-14

### 版本升级 + Zed agent-server 治理修复（2026-08-14，commit a25cfe0b）

- 全平台版本统一为 **1.5.3**（workspace、GUI、VS Code 插件、rust/python/typescript SDK、crates、cookbook、README 徽章）。
- **敏感内容 / 会话历史升级现在遵循 `governance_policy_mode`**：预检门禁对包含子串关键字（`api_key`、`token=`、`password`、`credentials` 等）的提示词一律以 -32040 硬拒绝，即使策略配置为 `audit`（文档定义为仅记录）。Zed 的 ACP stdio 传输没有审批 UI，导致合法的编码请求（配置/环境变量/密钥任务）硬失败。敏感内容与历史升级现在仅在 `governance_policy_mode` 为 `active`（或默认空值）时强制；`audit`/`advisory`/`disabled` 仅记录并继续。注入检测（High/Critical）与模式级检查（edit `rm -rf`、full_auto）保持不变。
- **ACP options 扁平化修复**：`build_chat_params_from_acp` 产出 `options: {extra: {cwd, model}}`，而 `PhaseOptions.extra` 为 `#[serde(flatten)]`——字面键 `extra` 落入 `extra["extra"]`，cwd/additional_directories/model 被静默丢弃。现在平铺输出；新增回归测试。
- **预检门禁回归覆盖**：良性 safeguard 提示词现在覆盖 `conversation_id`（Zed 始终会发送）、完整治理依赖复刻、以及真实 `session/prompt` 入口；audit 模式与 active 模式的敏感内容行为均由测试锁定。

## [Unreleased] - 2026-08-11

### 第 47 轮 — 超级深度/广度扫描 XV：协议域与并发诚实化（2026-08-11，docs/log/log-20260811-5.md）

#### 协议域

- **HTTP body OOM 窗口修复（P1）**：`route_http_post` 在强制 10MB 上限之前就按攻击者可控的 `Content-Length` 分配读取缓冲区，恶意客户端可强制超大分配。大小检查现在先于分配执行，且 ACP/MCP 两个 HTTP 臂共享单一 `MAX_BODY_SIZE` 常量（不再有会漂移的重复副本）。
- **`notifications/cancelled` 路由补齐**：裸 MCP 通知（`notifications/cancelled` / `notifications_cancelled`）现归一化为 `mcp.notifications_cancelled` 并加入 `ACP_METHODS` 白名单；ACP bridge 臂标记共享 cancelled-request 注册表，使进行中请求的循环提前中止（对齐原生 MCP 臂）。新增回归测试。
- **MCP stdio 停机竞态修复**：一次性 `Notify::notify_one()` 在请求处理期间到达的信号可能丢失（无 waiter 被存储），导致循环永不退出。现改为 notify 之前先置持久 `AtomicBool` 标志，并在每次迭代轮询（与 ACP stdio 臂对齐）。

#### 智能/选择

- **UCB 排序平局规则移除**：`rank_candidates_with_context` 原来按 UCB 分数后按字母序排序，冷启动平局时静默覆盖调用方排序；现仅按 UCB 稳定排序。`latency_tier` 从上下文 key 中移除（执行期才有的结果，执行前读侧无法复现）；`utc_hour()` 提取为与执行路径共享的唯一时间桶来源。
- **任务描述 thread-local 缓存移除**：多线程运行时下请求的 phases 会在 `.await` 点间迁移 worker，缓存值可能被另一个请求读到（跨请求 phase 路由/知识归属污染）。`extract_task_description` 现为纯函数。
- **Hub 共识硬编码回退投票移除**：`init_intelligence_hub` 启动时总是注册 voters，空 voters 分支在生产与测试中均为死代码。

#### 记忆/治理

- **向量锁序修复（P1）**：`ensure_hnsw_index` 先锁 `hnsw` 再锁 `conn`，而 `upsert`/`clear_all` 先锁 `conn`——并发惰性建索引与 upsert 可能死锁。所有路径现统一 `conn` → `hnsw` 顺序，且 `conn` guard 保持到索引发布完成。
- **`known_tool_names` 统一**：`governance/status.rs` 现读取权威 `ToolCapabilityRegistry::known_names` 表（辅以少量旧名补充），不再维护与分类器漂移的平行硬编码清单。
- **`PolicyVerdict::AllowWithConstraints` 删除**：无任何路径构造的变体；harness 记账与降级计数同步更新。
- **内存监控启动解除阻塞**：启动一次性检查改在 blocking 线程运行（`query_system_memory` 在 macOS 会 spawn 子进程、Linux 读 `/proc`）。

#### 配置/三端/文档

- **配置数值统一**：`config/config.toml` 的 cache/vector 键恢复权威值（3600 / 5000 / 192 / 80 / 800 / 10000 / 8 / 1200）并镜像到 vscode `configManager` 默认值；autotune `state_path` 统一为 `sqlite3/` 前缀；`config.reload` 改为 diff 真实顶层 TOML 段，不再硬编码 `["runtime"]`。
- **`[security]`/`[feature]` 节内键回填**：`#[serde(flatten)]` 只吸收顶层键，节内键（如 `[security] entry_auth_enabled = true`）此前被静默丢弃（节可解析但无效）；`sync_legacy_flat_keys` 现在从原始 TOML 把节内键提升到 `[runtime]`；新增回归测试。
- **三端修复**：vscode 把 state-sync heartbeat 转发到状态监视器（状态栏显示最近心跳时间），并停止向 config.toml 写入 camelCase `runtime.protocolMode`（非后端 schema 键）；GUI 强杀进程时按配置的 bind 地址推导端口（不再硬编码 8090）；i18n locale 更新。
- **文档同步**：README/CHANGELOG 数字（lib 1555）、cookbook local/simple-server 事实（协议模式、治理键位置、向量数值、autotune `auto_mode` 语义、`sqlite3/` 路径）、workflow-config 高危关键词表、k8s README 组件清单、声明数口径。

#### 验证

```
cargo test --lib → 1555 passed / 0 failed / 0 ignored
集成套件（152 非 chaos 声明）全通过
```

### 第 46 轮 — 超级深度/广度扫描 XIV：恒值路径清理与统一链路（2026-08-11，docs/log/log-20260811-4.md）

#### P1 正确性

- **warm 层记忆 TTL 修复**：`From<CanonicalEntry>` 写入 `accessed_at: 0`，使所有 warm 条目立即命中 30 天空闲过滤，低 usefulness 记忆约 10 分钟即被永久删除（设计意图 30 天）。现改为 `now_ts()`；新增回归测试 `test_bridge_entry_survives_first_auto_migrate_cycle`。
- **CapabilityBus 任务分类激活**：`apply_capability_bus_selection` 硬编码 `TaskType::Other`，使 task-fit 恒 0.60、recent-outcome target 恒 `"Other"`、UCB task 维度恒 `"Other"`。现经权威 `TaskRouter::analyze_task` → `pua::TaskType` 映射分类（5 个测试）。
- **自适应选择器学习闭环接键**：capability_bus 决策期用 agent 名读 UCB，执行期用 model id 写——两侧值空间永不匹配，排序静默退化为按名排序。执行期现追加 agent 级记录，统一两侧消费者。
- **会话 token 撤销闭环**：`revoke_session` 现黑名单化任意出示的 token（auto-provision 会话从不入 map，原撤销是空操作）；`logout` 对参数认证与 HTTP `Authorization` 头认证均撤销 token；`cleanup_expired` 清理黑名单。`issue_token`/`revoke_token` 保持 test-only（已注明）。

#### 恒值路径清理（本轮重点）

- **PUA 方法级红线守卫移除**：`check_red_lines(method)` 用自然语言 plan 红线对比 ACP 方法名恒不匹配——假安全信号。真实红线链为工具参数级 `check_tool_call`（tools_pack.rs）。
- **Rationalization 守卫去恒值**：删除恒 false 的 `is_full_auto` 参数及死分支；`evaluate()` 现返回弱证据状态，harness 调用点传真实置信度（`1.0 - risk`），低置信评审门与 `verify_output` 风险加权生效；计数口径注明（计弱证据触发而非硬阻断）。
- **mode.rs `AutoDegradePolicy` 字段删除**：`auto_degrade` 恒 false（`new_safeguard` deprecated 零调用），ReadOnly 自动降级档恒不可达——随 `evaluate_degradation` 一并删除；`safeguard_policy` 为唯一权威（>0.95 Block / >0.40 ConfirmRequired / else AllowWithAudit）；SafeGuard 只读工具面已文档化。
- **`RuntimeConfig.platform_mode` 删除**：全库零消费（governance_pack 的 `platform_mode` 是逐请求 RPC 参数，无关）；配置模板与测试同步更新。
- **GUI `secret_source` 删除**：keyring/env/file/auto 选择器从不影响行为（密钥恒走 keyring）；UI 现注明密钥仅存系统钥匙串。
- **vscode 死设置删除**：`go-on.chat.maxHistory/maxTokens/chat.streaming` 与 `go-on.ui.fontSize` 声明+展示但从不读取；从 package.json/设置 UI/locales 移除（18 条死字符串）；启动参数 `--verbose` 移除（CLI 本无此参数）。
- **GUI autotune `aggressive` 删除**：每请求发送但后端无消费路径。
- **`AutonomyRound`/`rounds` 删除**：写后即弃的逐轮记录（`retry_count` 恒 0）；报告保留 5 个被消费标量。
- **`AcpServer.verbose` 删除**：恒 false 零读取；`health_endpoint_ready` 恒 true 注明；`McpClientConfig` 超时经 `mcp.client.connect` 参数暴露；会话通知携带真实 `timeout_secs`。
- **死调用清理**：`dispatch.rs` 的 `DispatchOutput::Error` 委托单一错误出口 `io::send_error`（修复该路径缺失 `acp.error` 平台上下文注入）；header 认证解析统一到 `extract_header_values`（ACP/MCP 双臂大小写不敏感）；`vector.clear_all` 重置 HNSW 索引（清空后残留命中）；语义缓存后台清理复用 `purge_expired`（expired_count 原被低估）。

#### 部署/文档/三端

- **GUI 配置生成修复（P1）**：`generate_backend_config` 内嵌 UTF-8 BOM（TOML 解析失败）、`[protocol]` 重复（split 保留模板段）、`default_phase="think"` 不在注入 phases 内（校验拒绝）——三处全修；`--validate-config` 端到端验证通过。
- **k8s kustomization 修复**：`configMapGenerator` 把 ConfigMap YAML 文本当 TOML 内容（不可解析）——删除（configmap.yaml 已是资源）；`.secrets.env` 死键 `server-api-key` 改为 `GO_ON_ENTRY_API_KEY`。
- **`GO_ON_SERVER_API_KEY` 全面改名 `GO_ON_ENTRY_API_KEY`**：config.simple-server.toml、deploy 脚本/compose、cookbook 三语——旧名无任何代码读取。
- **文档同步**：SafeGuard 只读工具面（SAFEGUARD_MODE.md/README）、zed.md 模式回落（chat 路径为 `edit`）、workflow-config skills 默认值、design.md 过时 CLI 参数、simple-server sqlite 路径、CHANGELOG.zh-CN 轮次顺序、README 声明数口径。

#### 验证

```
cargo test --lib → 1549 passed / 0 failed / 0 ignored
cargo clippy --all-targets -D warnings → 零警告
4 profile + GUI + vscode tsc → 零错误；集成套件（151 非 chaos 声明）全通过
```

### 第 45 轮 — 超级深度/广度扫描 XIII：全功能架构统一（2026-08-11，docs/log/log-20260811-3.md）

- **语义缓存多轮误命中修复（P1）**：精确匹配改比较完整请求原文（截断 hash 仅作桶键）；调用方用最后用户消息作 key（原完整对话历史使第 2 轮起恒命中第 1 轮缓存）；新增回归测试。
- **WorldModel 实体更新修复（P1）**：`evolve_world_model` get-or-create（register 返回 id / find_entity_id 回退），实体属性（state/reward）真实写入（原用名称当 id 永不匹配）。
- **MCP 通知哨兵统一**：`notifications/initialized` 的 `id=Some(Null)` 在 HTTP 单请求（202 空体）与 batch（过滤）路径与 stdio 一致，不再泄漏为 200 响应体。
- **mTLS 配置接线（P2）**：`mtls_require_client_cert` 此前零消费点——ACP/MCP 两臂统一读取（CA 路径存在时默认要求，可显式关闭），`with_client_cert` 门控放开。
- **DAG 观测真实化（P2）**：`complete_step`/`fail_step` 从 cfg(test) 放开，`workflow.execute` 执行后按真实 subtask 结果推进 DAG，progress/stalled 不再恒初始值。
- **MCP HTTP drain 接线**：accept 循环停止条件由恒 false 改为 drain_guard（与 ACP 臂一致），排空不再只是固定 sleep。
- **PG 向量驱逐 COUNT 门控 + HNSW 搜索去全量克隆**：postgres upsert 对齐 SQLite 仅在超限时驱逐；hnsw_search 在 guard 内取候选元数据（消除每次搜索 O(n) 深拷贝）。
- **CLI 流式渲染去重**：主响应与 follow-up 两段 token 分类/渲染循环提取共享 `render_streaming_tokens`（行为逐分支一致）。
- **Planner::plan 同步化**：无 await 的 async 传染消除（调用点同步更新）。
- **Council quorum 上界修复**：`<= active_members` 改 `<= total_members`（成员投票后被自动淘汰不再误拒已达标提案）。
- **治理评审门直构 verdict**：消除 i18n 字符串往返（翻译改动不再静默翻转评审门）；删除死包装 `resolve_review_policy`/`review_verdict`。
- **漂移监控误报修复（P2）**：auto-baseline 改滚动均值；harness 延迟改秒单位（0.01 分母成亚 10ms 死区），亚毫秒抖动不再触发 breach；新增回归测试。
- **死代码/冗余清理**：`prompt_assembler` 死字段、`McpHttpServer.tls_acceptor` 恒 None 字段、`request_id_key` 重复实现、pg_migrate v3 死迁移（session_store 无消费者）、`filter_tools_by_exposure` 未用参数、`PromptLayer::all/name` 测试专用、`loop_executor` 改名 `file_walk`、`PerformanceMetrics.p95/p99` 无消费者（消除每次 get_metrics 全量排序）、`events.remove(0)` O(n) 改 VecDeque、`config_gen` 冗余 timeout 计算。
- **工具描述双源统一**：MCP tools/list 优先共享 `tool_descriptors` 表（与 LLM function-calling 一致），per-tool description 仅作回退。
- **三端修复**：GUI `api_key` serde skip 文档如实化、Export Masked/Full 合并（两按钮输出相同）；vscode `go-on.language` 接线生效 + 补声明 `pythonPath`/`execution.*` 配置键、statusMonitor 注释键名修正；i18n 默认值统一 `en-US`。
- **部署链路修复**：multi-users 部署改 `GO_ON_PG_CONNECTION_STRING`（原 DB_* 变量代码不读取）；k8s secret 键名与 envFrom 对齐；CI `cargo deny` 补安装步骤；删除随发布包分发的过时 `.github/workflows` 副本与 `.DS_Store`。
- **文档全面更新**：README/CHANGELOG 数字（lib 1537）、cookbook 三语构建命令（profile 语义）、zed.md 模式推断行为、workflow-config `[skills]` 幽灵段改 `[runtime]` 键、SAFEGUARD_MODE posture 表、storage/路径/日志名等多处对齐代码。
- 验证：lib 1537/0/0；clippy 全目标零警告；4 profile + GUI + vscode tsc + SDK 零错误；16 集成套件全通过。

### 第 44 轮 — 超级深度/广度扫描 XII：统一架构精炼（2026-08-11，docs/log/log-20260811-2.md）

- **PG 向量维度失配修复（P1）**：`VectorStore::new_with_replica` 建表后 `ALTER TABLE ... embedding TYPE vector(N)` 对齐运行时 provider 维度，消除固定 768 与默认环境 upsert 必失败。
- **skill CLI 双套统一**：`go-on skill` 改走与服务器一致的持久化 `SkillImportStore`；`skill import` 修复 `github:/url:/local:` 语法（新增 `parse_cli_import_source`，未知格式回退市场按名安装）；list-imported/info/refresh 读取真实数据。
- **autonomy 主路径缓存回填**：semantic_cache + token_cache 填充移到共享后执行路径，autonomy 成功产出同样填充。
- **CapabilityBus ToolBus 技能注册表统一**：new_acp_server 注入真实 SkillRegistry，导入技能对 agent_tool_match/tool_bus_skills 可见；**MemoryBus 后端注入时序修复**（移到 wire_server 前，注入真实生效）；**fallback 失败路径记账补齐**（Err 分支同样喂 LivePerformanceFeed/hyper_resilience）。
- **CORS preflight 双实现统一**（共享 evaluate_cors_preflight，修复通配双头缺陷 + 4 单测）；**MCP SSE 配置化 CORS**（原硬编码 `*` 绕过白名单）；**MCP 头部读取统一**委托 read_http_header；**config 验证门禁对齐与去重**（精确模板匹配）。
- **死代码/冗余清理**：capability_selector 冗余 reorder、`is_mcp_request` 死分支、`McpServer::new` 收敛为 cfg(test)、CLI `estimate_tokens` 包装与 `len()/4` 统一走共享估算器、`shared/math.rs` allow 收窄、`core/mod.rs` 悬空注释、autonomy 死 ±1 容差、hyper_resilience 默认延迟 100→0 与伪造失败模式移除；**重复实现合并**（copy_dir_recursive、validate_secret_security 收敛到共享实现）；**hub 加固**（头部/正文上限）；**TLS 日志如实化**。
- **文档数字修正**：README lib 1513→1533、SDK LOC ~4K→~6.9K、工具表 find→search_files；CHANGELOG 与 log-20260810-4 §12 集成测试数全部对齐实测（transport 18、e2e 8、pua 3、openai 6、i18n 1）；request.rs 测试 `len() >= 1` → `is_empty()`。
- 验证：4 profile + clippy 全目标零警告；lib 1533/0/0；集成套件全通过；vscode tsc / GUI / SDK 干净。

## [Unreleased] - 2026-08-10

### 第 43 轮 — 超级深度/广度扫描 XI：统一链路与收敛（2026-08-10，docs/log/log-20260810-4.md）

- **MCP 客户端双实现合并**为 `McpClientCore` + 可插拔 `McpTransport`（stdio/http 共享全部方法）。
- **workflow 工具统一执行链**：`workflow_execute/ask/generate` 移入 `execute_tool_call` 统一链（删除 native MCP 特判），两臂行为一致；补人读 `message` 与审计。
- **tool fallback 链合并**、**任务分类器去重**（Planner 委托 TaskRouter）、**SSE 帧复用**、**auto_migrate 三处触发收敛**为单一后台任务。
- **Delphi 收敛恒 false 修复**；**fault_tolerance 心跳语义修复**（`has_reported`，注册未上报不误判 Offline）+ 真实执行信号接入。
- **echo_skill 广告-执行闭环**（注册 `EchoSkillAlias`）；**skill_market CLI 崩溃修复**（blocking_write→await，配回归测试）；**selector context 学习信号接通**；**VoteMode::Legacy 真简单多数**；**GLOBAL_VOTERS 可替换化**（测试确定性）。
- **自进化循环门禁**：`runtime.evolution_enabled`（默认 false），杜绝无门禁 AutoApproval 改写源码。
- **死代码/假保留清理**：`preferred_agent_from_request`/`review_overrides`/`ADAPTIVE_TEMPLATE`/`find_template`/`adaptive` 死臂/unused re-export（multimodal 29 项等）；远程 skill index 移除 `deny_unknown_fields`（宽容 + schema 校验）。
- **GUI 重试状态集对齐**（408/429/全 5xx）；**vscode reconnect 测试重写**（真实 backoffDelayMs）；**skills/ 收敛**（删 11 符号链接，目录=34=builtin 数）；**结构性测试迁入 src**；**部署修复**（Dockerfile 数据目录、compose 卷+PG DSN、k8s `/metrics`、ps1 bug）；config 幽灵键清理。
- 验证：4 profile + clippy 零警告；lib 1527/0；全部集成套件通过；vscode tsc / GUI / SDK 干净。

### 第 41 轮 — 超级深度/广度扫描 + 依赖审计（2026-08-10，docs/log/log-20260810-1.md）

- `$/cancel_request` / `session/cancel` 取消语义真实化；Council 投票真实参与路由决策；SelfModelCore 假持久化清理；metacognitive llm_agent 假注入链整体删除；熔断器状态机三份合并；工具分类三份映射合并；Cold tier 接通读取（warm 不足回退冷层）；fault_tolerance 状态可恢复；配置双源合并；i18n 启动断点修复；`.goon` 路径统一；大量死代码清理。
- 依赖升级（全部 84 个直接依赖审计）：reqwest 0.13、base64 0.23、keyring 4、criterion 0.8、egui/eframe 0.36、comrak 0.54、quick-xml 0.41。
- 验证：4 profile + workspace `cargo check` 零警告；clippy（local/multi-users/GUI）零警告；`cargo test --lib` 1490 通过；Rust SDK 21/21。

### 第 42 轮 — 超级深度/广度扫描 + 统一架构优化（2026-08-10，docs/log/log-20260810-2.md）

- `$/cancel_request` 真实取消机制（ACP 镜像 MCP `cancelled_requests` 注册表，飞行中请求以 -32800 中止）；`session/cancel` 真实语义（取消会话拒绝新提示）。
- RecoveryAction 真实 dispatch（5 个动作可观察执行 + 恢复后一致性检查）；postgres warm tier DSN 修复，不可达时降级 cold；prompt_layers 注入接线；双遗忘循环统一；council tally 参与路由。
- 三套校验引擎收敛为单一 `ConfigValidator` + 硬门禁；profile 推荐名真实化（local/simple-server/multi-users-server/full）；keyring async 封装（阻塞 I/O 移出 tokio worker）；四 profile 编译全绿。

## [1.5.2] - 2026-08-11

### 版本升级 + 第 45 轮（2026-08-11，docs/log/log-20260811-3.md）

- 全平台版本统一为 **1.5.2**（workspace、GUI、VS Code 插件、rust/python/typescript SDK、crates、cookbook、README 徽章）。
- 第 45 轮全功能架构统一：完整变更见上方 `[Unreleased]` 区块（P1 语义缓存/world_model 修复、协议/mTLS/drain、DAG/council/治理/漂移、死代码清理、三端、部署/文档）。

## [1.5.1] - 2026-08-07

### 版本升级 + 全项目 errors/warnings 清理（2026-08-07，docs/log/log-20260807-1.md）

- 全平台版本统一为 **1.5.1**（workspace、GUI、VS Code 插件、rust/python/typescript SDK、crates、cookbook、README 徽章）。
- 全项目 errors/warnings 清理：`cargo check`（4 profile + workspace）与 `cargo clippy --all-targets -D warnings` 零警告零错误；GUI `cargo check` 零警告；`sdk/typescript` 与 `vscode-addon` `tsc --noEmit` 零错误。

## [1.5.0] - 2026-08-07

### 第 40 轮 — 超级深度/广度扫描第 1-6 轮（2026-08-07，docs/log/log-20260807-1.md）

#### SDK / 三端契约
- **删除 `sdk/nodejs`**（用户确认与 `sdk/typescript` 重复）：`vscode-addon` 保持对 `go-on-sdk-typescript` 的硬依赖；TS SDK 补 `configReload`（对齐后端 `config.reload`）、`health()` 返回类型修正为真实 `ServerStatus` 载荷；`.gitignore`/`contracts/cross-client-sync.md`/cookbook 索引同步移除 Node.js 引用。

#### 反假指标清理（原则 §13/§18）
- **删除 `AutonomyLoopReport` 恒零字段**（`planner_guidance_used`/`trace_alignment_coverage`/`corrective_actions_applied_total`/`corrective_action_effectiveness_ratio`）：ACP/CLI 自治循环路径不执行纠错动作，暴露恒 0/1.0 属假指标；`contract_snapshot()` 收敛为统一执行证据契约（轮次/工具数/阶段/耗时/终止原因）；workflow.execute 的 runtime 契约保留真实修复轮次计数。
- **`circuit_breaker_open` 告警接入真实源**：30s 告警循环读 `HyperResilienceEngine.profile().open_circuits`（与 Prometheus 导出器同源）；`RuntimeMetrics` 快照中 4 个非自有信号字段注释标明由外部填充。
- **`serial_work_ms` 接线真实值**（workflow.execute/task.execute 学习事件，原恒 0）；chat 蒸馏路径注释说明 0=无时序数据。
- **`record_autonomy_loop_stop_reason` 值域统一**：chat 路径 `completed`/`tools_executed`/`all_tools_failed` 正确映射完成/失败桶（此前 completion_ratio 恒失真）；删除无生产者的 `escalated` 死桶。
- **删除 `fallback_unhealthy_ratio`**（分子分母衡量无关事件）；保留真实计数 `fallback_unhealthy_agent_total`。
- **删除恒零假字段**：`build_change_bundle.test_coverage`（无测试数据源）、`org_policy.exceptions.active_total`（从不追踪）；`cpu_usage_percent` 接真实 perf 快照；`clarification_quality_score` 假满分 1.0 删除。
- **澄清指标真实推导**：chat 蒸馏学习事件调用 `resolve_learning_clarification_metrics`（与 task.execute 同源），从任务最新需求契约推导 rounds/quality/change-count；无契约时如实回退 0。

#### 加速 / 去重
- **`dedup_skill_calls` 共享 helper**（`orchestration/tool/mod.rs`）统一 CLI chat 与 ACP `run_agent_collecting`；修复 ACP 版 dedup 丢弃非技能工具的行为 bug。
- **`build_cli_principles` 缓存 key 升级**：技能数量 → 内容指纹（name+description 哈希），技能描述变更即失效重建。
- **SafeGuard 取消返回真实 prompt tokens**（原占位 0）；删除未用 `_response` 参数。
- **`recommend_parallelism_from_learning_bus` 按 source 过滤**：蒸馏占位 speedup 不再稀释并行度推荐。
- **i18n `format_template` helper** 去重 `get_formatted`/`tf`。
- **AppConfig 进程级 mtime 缓存**（单请求 6× load → 1×）、healthcheck 报告 2s TTL、`start_drift_monitor` 惰性化、HTTP client 统一、harness/communication/memory-vacuum 死 API 删除（第 1-3 轮）。

#### 验证
```
cargo check 4 profile + --workspace   → 零警告
cargo clippy --all-targets -D warnings → 零警告
cargo test --lib                     → 1590 passed / 0 failed
```

---

### 第 39 轮 — 架构统一与加速（2026-08-06/07，docs/log/log-20260806-7.md）

#### 安全/正确性
- **ACP_METHODS 排序修复**：`tool.approve`/`terminal/create` 逆序导致 ACP 模式两个 handler 不可达；新增排序不变量回归测试。
- **`WriteFileTool::run_async` 沙箱漏洞**：async 路径绕过 50MB 限制与系统敏感路径拦截，现与 sync 路径共用 `enforce_write_sandbox`。
- **`detect_audio_format` 越界 panic 修复**；**双重 base64 修复**（document_parser 已编码，不再二次编码）。
- **`RepoAnalyzer::clone` 临时目录生命周期修复**（`Arc<TempDir>` 保活）、`head_commit` 真实获取、`loc` 真实行数、`extract_types` 复用仓库映射去双扫。
- 修复外部提交损坏的 `AcpErrorCode::from_code`/`is_acp_request`。

#### 三端契约（原则 #2）
- Rust/Python/Node SDK：`task.plan`/`task.execute` 改发 `{task}`（此前 `description`/`plan_id` 与后端不符）。
- 四端 `HealthResponse` 对齐后端真实载荷（Rust SDK `health()` 此前恒解析失败）。
- Node SDK `health()`=GET /health、`runtimeHealth()`=runtime.health（原先颠倒）；`taskPlan`/`taskExecute` 参数名修正。
- VS Code 工作流/流程视图将本地步骤模型扁平化为后端实际读取的 `task` 参数。
- `knowledge.distill` 调用端改传后端 `limit` 窗口（删除幻影 `query`/`source`）。

#### 反假修复/指标（原则 #13/18）
- `SecurityGovernor` 计数集中到 `record_audit`（denials/reviews/escalations 原被双计；`active_escalations` 不再逐条审计恒增）。
- `harness_bus` `current_active_policies` 由硬编码 12 改为真实计数。
- drift 自动基线按 metric 名分组（`validate_action`/`verify_output` 延迟不再互相污染基线）。
- `governance.status` 单次加载配置（`governance_config_summary_with`）。
- `hash_file` 未知算法显式报错（不再静默回退 sha256 并谎报算法名）；`random_token` 改用加密安全 `rand`。
- `cache_strategy` 大小写不敏感扫描不再复制会话历史。
- `default_non_ai_config_toml` 写规范名 `mode = "adaptive"`；OTel 默认端点提示与实际 `localhost:4317` 一致。

#### 性能
- `http_request` 异步/同步双路径共享连接池 client + 请求级 timeout（原每请求新建）。
- `record_outcome` 锁次数 7→4（惰性 entry 初始化，去掉无条件 `register_service`）。
- `synthesize_keyword_heuristic` O(n²)→O(n)；`health`/`release.readiness` 独立 future `tokio::join!` 并行。
- `load_recent_knowledge_context` 按 mtime 缓存（不再每请求读文件）；`web_search`/LSP client 缓存复用。

#### 死代码/生命周期（原则 #11/13）
- 删除：`builtin_tools!` 宏、`ToolProgress`/`run_with_progress`/`run_streaming` 子系统、`ShardedGovernanceCache` 命中/未命中计数器、`with_default_trigger_source`、`Agent::send_message` 空操作、token cache TTL 死机制与 `report`/`stats_snapshot`/`reset`/`put_string*`/`get_string`/`summarize_sync`/`expected_dimension` 死 API。
- **故障恢复计划现在按一致性检查结果 complete/fail**（`recovery_plans_in_progress` 不再无限增长）。

#### 文档
- cookbook API 文档重写为真实接口面：JSON-RPC 走 `POST /rpc`（非 `/v1/responses`）、7 条真实 GET 路由、真实 CLI 标志/子命令；删除 `core-runtime`/`learning-intelligence`/`workflow-task`/`safety-governance`/`optimization-ops`/`observability` 的虚构 REST 端点。
- `scripts/verify-zed-integration.sh` 修复（端口 8090、`agent_servers` schema）→ PASS。
- README 子总线口径统一为 7 个特性门控子总线；SDK 统计补 Node.js；删除无法核实的测试计数。
- `contracts/cross-client-sync.md` 路径引用修正。

#### 验证
```
cargo check 4 profile + --workspace   → 零警告
cargo clippy --all-targets -D warnings（4 profile）→ 零警告
cargo test --lib                     → 1645 passed / 0 failed
TS/Node/vscode tsc --noEmit、Python py_compile、verify-zed-integration.sh → PASS
```

---

### 第 38 轮 — 遗留 7 项收口（2026-08-06）

#### 文档一致性修复（2026-08-07）

- 文档与代码对齐（原则 #18）：技能数（33）、JSON-RPC handler 数（148）、供应商数（37）、提示词模板数（149 个模板 / 16 个类别）、后端 i18n 键数（扁平化 711）、`docs/protocol-guide.md` 的 MCP 工具表（真实 27 个基线工具）、CLI 标志（`--protocol-mode`；不存在 `--export`/`--import`/`--mock`/`--dry-run`）、配置预设（`config/config.simple-server.toml` 等），并移除对不存在的环境变量（`GO_ON_CONFIG`、`GOON_LOG`）与配置段（`[observability]`、`[access]`、`[logging]`、`[metrics]`、`[concurrency]`、`[timeouts]`、`[security]`、`[database]`）的引用。
- `.zed/settings.json` 现在按 `docs/zed-integration.md` 与 `scripts/verify-zed-integration.sh` 的检查项预注册 go-on 为 Zed agent server（`agent_servers.go-on` + 自动批准 + `auto_approve_tools`）；`scripts/verify-zed-integration.ps1` 重写以匹配当前 settings schema 与文档位置。
- 修复 `prompts/zh-CN.json` 与 `prompts/zh-TW.json`（错位的 `goon_agent` 类别移入正规类别；zh-TW 字符串内两处裸换行转义）；`scripts/validate-prompts.sh`（含 `--strict-i18n`）零错误零警告通过。
- README/README.zh-CN 的测试数字改为引用下方最新一次完整 `cargo test --all-targets`（第 38 轮：**3478 passed / 0 failed**），不再保留过期的分配置数字。

第37轮遗留的 7 项（见 `docs/log/log-20260806-6.md`）全部完成并一次验证：重复逻辑唯一收敛、隐藏重复栈统一、自进化子系统接线真实 LLM。

#### 重复功能合并（原则 #8）

- **requirement 自动恢复 3×→1×**：`evaluate_requirement_gate_facade` 改为纯求值；synthesize→inject→re-evaluate 序列唯一收敛到 `try_auto_recover_requirement_gate`（唯一指标记录点）；删除死变体 `RequirementContinuationKind::ClarificationInProgress`；**修复双计数/误计数**：删除 `workflow_pack.rs` 两处手动 `record_requirement_auto_recovery()`（该指标已在恢复成功处记录一次，纯 `Confirmed` 续流不再被误记为自动恢复）。
- **agent token 分类 3×→1×**：新增 `AgentToken` enum + `classify_agent_token()`；三个收集循环（`collect_agent_responses`/`run_agent_collecting`/`run_followup_after_tool_observation`）共用；`autonomy_loop.rs` 的 SSE 转发循环保留内联处理（需区分 finish_reason/usage、回填工具调用文本、SSE 帧语义，非重复）。
- **ACP bridge 与原生 MCP resources/prompts 去重**：五个共享函数成为唯一实现，bridge `mcp.*` 负载与原生 `handle_*` 全部委托；删除原生侧 3 个重复构造方法。
- **文档解析统一**：`read_pdf`/`read_docx` 改委托 `DocumentParser::parse_bytes`（删除内联 lopdf/docx-rs 文本提取副本）；保留 `page_count`/`paragraph_count` 兼容字段并新增 `images`/`tables`/`metadata`（纯增量 key）。对象级 PDF 合并/拆分与 DOCX 生成不属于文本提取，保留。
- **TokenCache 主路径写入 + 消除 3× embedding**：`lookup`/`peek_similar` 返回预计算置信度（L1/L3=1.0，L2=cosine 得分），`decide_from_entry` 不再每次 L2 命中重新 embedding；主路径（非 fallback）新增 `store_async` 填充 token cache；`CachedAgentWrapper::chat` 命中同样套用执行类 bypass 门。
- **三套 embedding/similarity 栈统一（大重构）**：全部收敛到 `local_hash_embed` + `shared::math::cosine_similarity_f32`。token cache `simple_embedding`（256 维）、语义响应缓存（bigram/Jaccard 改为预计算 embedding + cosine，128 维）、skill 语义匹配（f64/DefaultHasher 桶实现，96 维）均委托权威实现；删除已无调用者的 f64 `cosine_similarity` 及 4 个死代码测试。

#### 链路接线激活（原则 #14）

- **自进化子系统 LLM 接线**：后台任务从 `agent_registry` 解析（assistant→summarizer→首个）注入 `SelfEvolutionAgent::with_llm`，`generate_patch` 走真实 LLM 路径；`MemorySummarizer` 加 `Clone` 与 `Option<Arc<dyn Agent>>`（`server_builder` 构造并复用，`get_or_init_memory_persistence` 不再新建无 LLM 默认实例）；`summarize_hot_entries` 改 async（快照→await 真实 `summarize`→重锁替换，跨 await 不持 Mutex），`auto_migrate` 唯一调用点 `.await`；`analyze_code` 文档修正（确定性静态分析，不调用 LLM，原则 #18）。

#### 验证

- `cargo check` 4 profile：零警告。
- `cargo clippy --all-targets -- -D warnings`（local/simple-server/multi-users-server/full）：零警告。
- `cargo test --all-targets`：**3478 passed / 0 failed**。

#### 三处"保留"决定补充复核（用户追问）

- **`run_autonomy_loop` 改用共用 `classify_agent_token`**：reasoning 起始/结束标记全仓无任何 agent 发射（仅 CLI/classifier 消费），转换行为中性，且顺带消除"未来若发射则控制符泄漏进 response/SSE"的隐患；同时删除只写不读的死累加器 `round_response`（5 处 push、0 处读取）。
- **`pdf_split` 不再解析同一文件两次**（页数统计与页面删除共用单次加载）；merge/split 共享新增 `load_pdf_document`/`save_pdf_document` 助手。对象级页树操作仍正确地留在 `DocumentParser`（纯文本提取）之外。
- **三处词 tokenizer 仍各自保留**（`signature_similarity`/`semantic_matcher::tokenize`/`execution::tokenize_text`）：min_len/大小写/集合类型/评分公式（Jaccard vs Dice vs TF+tag）各不相同——属领域调优，非重复。

### 第 32–37 轮深度+广度扫描与清理（2026-08-06）

七轮超级深度+超级广度扫描收口（见 `docs/log/log-20260806-{1..6}.md`），继续遵循同一原则：零死代码、零占位、零假修复、三端统一架构。

#### 反假修复与诚实性（原则 #13/#15）

- **导入技能真实执行**：`mcp.tools.call` 调用导入技能此前返回假的 `NOT_IMPLEMENTED_EXECUTOR` 成功；现改走真实 `PromptBasedSkill` LLM 执行器（未接 LLM agent 或 manifest 无执行内容时明确报错）。
- **`health.check` 传播真实失败**：健康探针失败时不再恒返回 `{"ok": true}`。
- **`workflow.execute` 真实评审**：伪造的 `APPROVE` 评审项改为对执行摘要的真实确定性校验；`review_status` 反映真实结果；自治契约上报真实修复轮次与有效性。
- **游戏工具诚实化**：`game_monitor` 的 `window_active` 改由真实进程状态推导；无截屏工具时如实失败；`game_replay_recorder` 真正调用 ffmpeg（x11grab）录制而非返回"ready"提示。
- **审计链轮换保留签名**：`GOON_AUDIT_SIGNING_KEY` 签名的链在 100 MB 轮换后不再变为未签名。
- **分布式记忆传输改为真实 HTTP**：multi-users-server 的 `do_sync` 不再模拟向 peer 发送（此前本地自吞并报 Completed）；现向各 peer 的 `/rpc` 端点 POST JSON-RPC `memory.ingest`，hub server 新增对应 `memory.ingest` 处理器；失败如实上报。
- **PostgreSQL 初始化重试落地**：`initialize_postgres_backend` 文档声称 3 次指数退避重试但从未重试；现按文档在阻塞池上实现（1s/2s/4s）。

#### 死代码清理（原则 #11）

- 删除无生产者的 `IDEMPOTENCY_HIT_TOTAL` 计数器、无调用者的 `GovernanceStatus::to_json`、`record_audit_threadsafe`、`McpServer.logging_level` 字段、`SESSION_UPDATE` 快速路径表项、`mcp/tools.rs` 转发壳（error_codes 移入 `mcp/mod.rs`）、`dispatch_server` 死参数 `_client`。
- 删除 harness_bus `AuditEntry` 中间类型（两处调用点直接构造 canonical `AuditLogEntry`）；intelligence hub 的 `AUDIT_ENTRY_COUNT` 静态改读 canonical sink 长度。
- 三处私有 SHA-256 包装统一为 `shared::sha256_bytes`/`sha256_hex`；`time.rs` 改用 `shared::timestamps`。

#### 链路激活（原则 #14）

- **工具回退链接入执行器**：自治循环与 ACP agent runtime 现在按各工具的 fallback_chain（`read_file→search_files`、`grep→search_files` 等）执行回退，与 CLI 路径一致。
- **故障容忍恢复周期定时调度**：`FaultToleranceEngine::run_recovery_cycle` 以 30s 间隔在 `start_background_tasks` 中运行（此前仅在测试中被调用）。
- **`state_sync` 模型/agent 事件发布**：`config.reload` 对比 agent 集合与配置模型集合，发布 `AgentsChanged`/`ModelsChanged`，激活 GUI/VS Code 的 `onModelsChanged`/`onAgentsChanged`。
- **治理审计并入 canonical sink**：`governance.plan.update` 事件改经 `global_audit_log()`（哈希链+轮转）落盘，删除第二条非链式 `.goon/governance/audit.ndjson`；`governance.audit.recent` 读内存 sink（无逐请求文件 I/O）。
- **漂移监控接入真实指标**：`validate_action`/`verify_output` 向 `DriftProtectionEngine` 上报延迟指标并注册性能漂移策略，60s 监控开始评估真实数据。

#### 重复统一与正确性

- `web_scrape`/`rss_read` 强制走 `http_request` 同一 URL 沙箱（`validate_url`），闭合 SSRF/内网 IP 绕过。
- `is_low_risk_tool` 过期工具名修正（`time_util`/`diff`/`rss_feed` → `date_time`/`file_diff`/`rss_read`）；CLI `/grep` 改命中注册的内容 `grep` 工具（不再被 `search_files` 别名遮蔽）。
- MCP `filter_tools_by_exposure` 过期名修正（`container_*`→`docker_*`，删除 `compile_and_run`/`qrcode_`）；gzip 解压逻辑在 `decompress` 与归档解压间共享。

#### 性能优化

- 启动：配置校验不再二次读取 TOML；Copilot 代理探测+客户端构建按 env 快照缓存（`provider.list_models`/设备码路径每次最多省 ~700ms）；`/proc` 内存/CPU 读取 5s TTL 缓存（status/health/metrics 端点共享）。
- 请求路径：data-URI 附件 `join_all` 并行；`observe_phase` 复用进程级 HTTP 客户端；文档解析改 `spawn_blocking`；MCP HTTP JSON-RPC batch 改 `join_all` 并行分发；能力总线选择与向量上下文加载并行；多代理安全网不再把全新执行标记为缓存命中。

#### 三端对齐

- **VS Code 插件幻影 RPC 映射到真实方法**：`approval.approve/reject` → `session/request_permission`、`checkpoint.create` → `conversation.checkpoint.create`、`skill.import_local` → `skill.import`、`runtime.reload_config` → `config.reload`、`checkpoint.load` → `checkpoint.list`（带 warn）；破坏性命令（`chat.delete`/`session.clear`/`memory.clear`）→ `session/delete`/`vector.clear`；`config.reset`/`agent.remove` 改为明确失败并 warn，不再发送注定失败的空请求。
- **TypeScript SDK 幻影方法改名**：与其余三端 SDK 对齐后端方法名：`workflow.plan`→`task.plan`、`summary.get`→`learning.summary`、`knowledge.search`→`knowledge.distill`、`rl.optimize`→`rl.alignment.offline_eval`。

#### 验证

- `cargo check` 4 profile + `--workspace`：零警告。
- `cargo clippy --all-targets -- -D warnings`（local/simple-server/multi-users-server/full）：零警告。
- `cargo test --all-targets`：**3486 passed / 0 failed**。
- `scripts/gen-provider-catalog.py --check`：双输出 OK（37 providers）。

---

### 24 轮深度+广度扫描与统一优化（2026-07-24 → 2026-08-05）

版本 1.5.0 汇总 24 轮超级深度+超级广度多智能体扫描成果（见 `docs/log/`），以 `docs/blueprints/principle.md` 为原则收敛：零死代码、零占位、零假修复、三端（backend / GUI / VS Code）统一架构。

#### 第 25–29 轮精炼（均在 1.5.0 下）

- **断路器统一**：按代理的 failure_prevention 并行状态机（~600 行）退役；健康监控、降级策略、恢复与快照全部迁入 `HyperResilienceEngine`，成为唯一韧性权威（`breaker.*` RPC / `governance.status` / 健康探针读同一真源）。
- **模型建议统一**：GUI 手维护的 ~180 行模型表迁入后端权威源（`ProviderSpec::model_suggestions`），由 `gen-provider-catalog.py --check` 生成并校验一致性。
- **审计管道统一**：规范审计 sink 现在把**每一条**落盘记录哈希链入 `~/.goon/audit_chain.ndjson`（单写线程、顺序精确、链文件按尺寸轮转）。独立 per-server `HashChainAuditor` 布线与每请求 `spawn_blocking` 追加被删除；请求台账改经 sink 记录（脱敏、非阻塞）；可选 Ed25519 签名（`GOON_AUDIT_SIGNING_KEY`）与新增 `governance.audit.verify` RPC（链摘要、完整性违规、时间窗报表）闭环验证。
- **SSE 与 SDK 对齐**：VS Code/Node.js SSE 分块解析对齐契约；Node.js 聊天流改为真增量 AsyncGenerator；`tests/e2e/` 更名 `tests/structural/`。
- **Bench 回归修复**：`benches/acp_bench.rs` 原始字符串定界符断损导致运行时无效 JSON（criterion `--test` 模式 panic）。
- **第30轮 — SSE 字段提取统一**：`extract_chunk_text` / `extract_agent_model` / `extract_result_meta` 落在 `gui/src/backend/state.rs`（单一真源）；富流式路径与非流式回退共用，消除 `token`/`text` 回退漂移。
- **第30轮 — 退避骨架统一**：新增 `gui/src/backoff.rs::exp_backoff_ms`，健康轮询、崩溃重启限流、channel 满重试、RPC 重试 base 四处复用。
- **第30轮 — 跨 SDK 退避契约漂移修复**：Rust/Node/Python/TypeScript SDK 原为 AWS full-jitter，与契约 ±30% jitter（`min(base×2^n, 30s) × (0.7+random×0.3)`）不符；4 端全部对齐 GUI/VSCode 实现；VSCode 二进制下载重试补上缺失的 30s cap。
- **第31轮 — `governance.audit.verify` 全链路接线**：4 个 SDK 补齐类型化包装、VSCode 新增 `go-on.governanceAuditVerify` 命令、e2e 测试用真实二进制端到端验证 RPC 路由。
- **第31轮 — TS SDK 测试套件首次运行**：补齐 `node_modules` 后全量执行；修复第30轮退避改动引发的超时回归（HTTP 错误测试改 `maxRetries: 0`）与一直存在的 abort 流挂起缺陷（mock 流在 abort 时 close）。

#### 冗余消除与统一

- **Provider 目录三分拷贝 → 1 权威源 + 2 生成产物**：`src/core/providers.rs` 为唯一权威；GUI `generated_catalog.rs` 与 vscode `providerCatalog.generated.ts` 均由 `scripts/gen-provider-catalog.py` 生成（带 `--check` 双输出校验）。VS Code 目录补齐 kimi/siliconflow，env var 与分组全部派生自后端。
- **MCP 桥接 ↔ 原生处理器漂移闭合**：`mcp.resources.list` 不再返回空列表；`mcp.resources.subscribe` / `mcp.logging.setLevel` / `mcp.completion.complete` 假成功空响应改为与原生 `src/mcp/handlers.rs` 一致的真实实现/诚实错误。
- **PostgreSQL TLS 连接栈合并**（消除 ~200 行重复）：`parse_sslmode` / `PermissiveVerifier` / `connect_postgres` 统一收敛至 `src/memory/pg_pool.rs`。
- **重复时钟助手合并**：`agents::unix_now_secs` 改为委托 `shared::timestamps::now_ts`。
- **`keyring://` 常量统一**：`agents`、`acp::helpers::planning::context`、`config_validation`、`env_override` 共用一份。
- **废弃产物删除**：8.5 MB `scripts/go-on` 二进制、空文件 `debug_binding.py`、孤立 shell 脚本、TypeScript 死导出、Rust 死 API（`Agent::on_message`/`send_message`、`AgentMessenger::with_capacity`/`peek`、`new_safeguard`）。

#### PostgreSQL 生产加固

- 连接池（deadpool）+ 读写副本分离 + 版本化迁移 + `sslmode` TLS（require/verify-ca/verify-full）。

#### 功能补齐（闭合缺口）

- **F-GAP-66 附件多模态**：GUI 附件（文件选择器 + 粘贴/拖拽）真实进入后端多模态管线（图像提取、文档解析、音频转写、`repo:` 分析），不再只是文本摘要。
- **MCP `initialize` 能力声明统一**：仅声明双入口（原生+桥接）均有真实 handler 的能力；`sampling` 从共享声明中移除。
- **Copilot URL 权威值收敛** 至 `https://api.githubcopilot.com`（原为漂移的 localhost 拷贝）。
- **`build_role_routing` 读取已填充的全局角色注册表**（原构造恒空注册表，`available_custom_roles` 恒为 0）。

#### SDK 协议漂移修复

- `checkpoint.create` → `conversation.checkpoint.create`（需 `conversation_id`），覆盖 rust / nodejs / python SDK。
- nodejs `runtime.initialize`/`runtime.shutdown` → 规范名 `initialize`/`shutdown`。
- `breaker.reset` 参数契约与后端（`agent`/`name`）对齐。

#### 文档与版本

- 全平台版本统一为 **1.5.0**（workspace、GUI、VS Code 插件、rust/nodejs/python/typescript SDK、crates）。
- 恢复缺失的 `[1.2.0]` 英文条目（原滞留为陈旧的 `[Unreleased]`）。
- README 统计按实测修正（2018 测试、37 供应商、37 技能、~238K LOC、13 子总线架构）；CI 徽章 URL 修正。

### 验证

- 后端：`cargo check --all-targets` 通过；`cargo test` 全绿；`cargo clippy --all-targets -- -D warnings` 零警告。
- GUI：`cargo check` 通过。
- VS Code 插件：`tsc --noEmit` + mocha 全绿。
- Provider 生成器：`scripts/gen-provider-catalog.py --check` 双输出 OK（37 providers）。

## [1.4.3] - 2026-07-24

### BLUE71 — 三系统深度对比分析与高收益改进

本版本实现了 BLUE71 全部 9 个改进方案，补齐了与 Codex 和 Harness Gitness 深度对比发现的架构差距。总完成率：100%。

#### SessionActor — 树状会话架构（§2.1.1）

- **SessionLifecycle**：有限状态机 — Created → Ready → Active → Draining → Archived，通过 watch channel 传播。
- **SessionInput**：Actor 模型消息队列 (mpsc)，支持 UserMessage、Cancel、Steer 三种变体。
- **SessionHandle**：外部交互句柄，提供 send_message()、cancel()、steer()、生命周期订阅。
- **SessionState**：持有 CommunicationBus、ConversationHistory、CompactionManager、FragmentRegistry、AgentGraphStore。
- **session_main_loop**：持久 tokio 任务，处理 SessionInput、管理生命周期转换、触发自动压缩。
- **AgentThread 集成**：会话启动时创建 1 个 AgentThread，所有 UserMessage 通过 ChatRequest 复用。
- **优雅排空**：Cancel → 发送 Cancel 给 AgentThread → 生命周期：Draining → Archived。

#### AgentThread — 非阻塞 Agent 生成 + 持久循环（§4）

- **AgentThread**：非阻塞 Agent 执行句柄，含输入队列、状态 watch channel、JoinHandle。
- **spawn_agent_non_blocking()**：立即返回 AgentThread 句柄，Agent 作为独立 tokio 任务运行。
- **agent_main_loop**：真正的持久 Actor 循环 — 单条消息后不 break。连续处理 UserMessage、ChatRequest、Cancel。
- **ChatRequest 变体**：接受完整消息列表（含 system prompt）+ 选项 + oneshot 回复通道，为 SpawnAgentTool 集成铺路。
- **SpawnConfig**：可配置 max_depth、max_concurrency、token_ceiling、timeout_secs。

#### SpawnGuard — RAII 并发槽位保护（§5）

- **SpawnGuard**：原子计数器，支持 try_reserve/commit/release_slot/Drop。panic 时自动释放（无泄漏）。
- **提交模式**：所有权从调用者转移给生成的线程，线程完成时释放。
- **集成**：SpawnGuard 替换了 SpawnAgentTool 中的静态 Semaphore，也被 SessionActor 用于 AgentThread 预算。
- **当前使用量追踪**：`SpawnGuard::current_usage()` 用于可观测性。

#### 事件驱动状态传播 — 零轮询（§6）

- **AgentMessenger.notify**：每条消息投递时递增的 watch channel。
- **wait_for()**：使用 `notify_rx.changed().await` 替代 `tokio::time::sleep` 轮询。
- **AgentNode.lifecycle_tx**：生命周期状态的 watch channel 发送端 — 订阅者在每次状态转换时收到通知。

#### AgentLifecycle — 有限状态机（§7）

- **AgentLifecycle**：6 种状态 — Registered、Idle、Active（含 Planning/Executing/Reflecting/Waiting 阶段）、Completed、Errored、Cancelled。
- **AgentLifecycleBuilder**：便捷构造器，自动计时。
- **集成**：AgentTree 中的每个 AgentNode 都携带 `lifecycle_tx: watch::Sender<AgentLifecycle>`。
- **摘要方法**：人类可读的状态描述，用于日志和调试。

#### AgentGraphStore — 持久化抽象（§8）

- **AgentGraphStore trait**：upsert_edge / set_edge_status / list_descendants / remove_subtree。
- **InMemoryAgentGraphStore**：基于 HashMap 的默认实现 — 通过 Arc<RwLock> 线程安全。
- **SqliteAgentGraphStore**：SQLite 实现（feature: backend-sqlite）— rusqlite + spawn_blocking 模式。
- **Checkpoint 序列化**：ConversationHistory.to_checkpoint_json() / from_checkpoint_json() — 完整 JSON 往返。
- **集成**：SessionState 持有 `graph_store: Arc<dyn AgentGraphStore>`。Checkpoint 将序列化历史作为边存储。

#### ContextFragment — 结构化上下文注入（§9）

- **ContextFragment trait**：role() / priority() / body() / weight() 用于可注入的上下文片段。
- **FragmentRole**：System, Developer, User — 控制片段在提示中的位置。
- **FragmentPriority**：Low, Normal, High, Critical — Critical 总是包含，不受 token 预算限制。
- **FragmentRegistry**：register() + build_context(budget) + build_context_pairs(budget)，支持优先级排序和预算感知截断。
- **SimpleFragment**：基于静态字符串的片段内置实现。
- **集成**：SessionState.fragments 在 UserMessage handler 中填充 system prompt。

#### AdaptiveCompactor — 自适应对话压缩（§10）

- **ConversationTurn / ConversationHistory**：Token 感知的对话追踪，支持 drain、prepend、to_text 操作。
- **CompactionStrategy**：SlidingWindow（保留 N 轮）、Summarize（LLM 摘要）、Hybrid（摘要 + 保留最近）。
- **CompactionManager**：同步压缩引擎 — 无需 tokio runtime，可在任何上下文中使用。
- **AdaptiveCompactor**：自适应学习 — 基于对话长度和历史质量评分自动选择策略。
- **AdaptiveThreshold**：动态阈值 — 高质量时提高（少压缩），低质量时降低（更积极压缩）。
- **用户反馈融合**：quality * 0.6 + feedback * 0.4 混合评分。
- **30 个测试**：ConversationTurn、ConversationHistory、CompactionManager、AdaptiveThreshold、AdaptiveCompactor。

#### GuardianReviewer — 独立模型审查（§11）

- **GuardianReviewer**：使用独立 Agent 实例在执行前审查工具操作。
- **GuardianDecision**：Allow / Deny / EscalateToUser — 故障关闭（错误/超时/解析失败 → Deny）。
- **GuardianCircuitBreaker**：双阈值 — 最大连续拒绝（3）+ 最大近期拒绝（10/50）。
- **from_registry()**：从 AgentRegistry 查找审查 agent — 返回 None 用于优雅降级。
- **16 个测试**：熔断器、决策解析、允许/拒绝/无效/触发。

#### 跨模块重构与清理

- **agent_main_loop break 移除**：UserMessage 和 ChatRequest handler 现继续循环 — 持久 agent。
- **InterAgentComms 占位移除**：该变体只有空 handler（仅日志）— 按原则 §9 移除。
- **SessionActor 异步化**：spawn_session 从 sync（含 block_on）改为 async fn。
- **panic! 消除**：spawn_session 返回 Result<SessionHandle, String> 而非在路径解析时 panic。
- **根路径缓存**：AgentPath::parse("root") 只解析一次，缓存在 SessionState 中。
- **代码清理**：生产代码中零 #[allow(dead_code)] 或 #[expect(dead_code)]。零未使用导入。

#### 新增文件

| 文件 | 行数 | 描述 |
|------|------|------|
| `src/agents/session.rs` | ~700 | SessionActor 树状架构 |
| `src/agents/graph_store.rs` | ~280 | AgentGraphStore trait + 内存 + SQLite |
| `src/agents/fragment.rs` | ~300 | ContextFragment trait + FragmentRegistry |
| `src/governance/guardian.rs` | ~600 | GuardianReviewer + 熔断器 |
| `src/optimization/compaction.rs` | ~1000 | AdaptiveCompactor + 对话类型 |

### 验证

- **新测试**：所有新增模块约 70 个新测试。
- **所有 Profile**：local、simple-server、multi-users-server、full 全部编译通过。
- **蓝本符合性**：BLUE71 的 P0/P1/P2 改进 100% 实现。

## [1.4.1] - 2026-07-21

### 架构 — Transport Trait 第四阶段完成 + i18n 统一

本版本完成了 Transport Trait 迁移（第四阶段）并统一了三端（CLI、GUI、ACP）的错误消息。

#### Transport Trait 第四阶段（RPC_BUFFER 移除）

- **RPC_BUFFER task-local 已移除**：所有 JSON-RPC 输出现在通过 `CURRENT_TRANSPORT`（基于 RwLock 的全局传输层）路由，消除了双路径遗留机制（io.rs）。
- **RpcBufferTransport 已接线**：HTTP RPC handler（`/rpc`）和 TLS handler 现在使用 `set_current_transport(RpcBufferTransport)` 而非 `RPC_BUFFER.scope()`，响应捕获保持不变。
- **SseTransport 已接线**：`/chat/stream`、`/v1/chat/completions`、`/v1/responses` handler 在连接建立时设置 `SseTransport`。
- **CURRENT_TRANSPORT 升级**：OnceLock → RwLock，支持运行时在 stdio/HTTP-SSE/HTTP-RPC 模式间切换。
- **测试串行化**：Dispatch 测试使用 `DISPATCH_TEST_LOCK` 防止全局传输的并行竞争。
- **dead_code 标注清理**：移除 transport.rs 中所有过期的 Phase-1/2 `#[allow(dead_code)]`。

#### i18n 错误消息统一（跨模块）

- **12 个新 CLI i18n 密钥**：`cli.chat.git_diff_failed`、`cli.chat.summarization_failed`、`cli.chat.ai_review_failed`、`cli.chat.find_path_usage`、`cli.chat.tool_call_limit_mode`、`cli.chat.conversation_long_warning`、`cli.chat.tool_blocked_by_mode`、`cli.chat.tool_call_blocked_by_mode`、`cli.chat.tip_compact` — 已添加到 en-US.json、zh-CN.json、zh-TW.json。
- **6 个新 GUI i18n 密钥**：与后端错误模板匹配的 `chat.error.*` 提示密钥。
- **10 个硬编码 CLI eprintln! 消息** 已迁移至 i18n `t()`/`tf()` 调用。
- **GUI 后端空响应**：用透明的空内容传播替换了误导性的预设消息（GUI 的 `finalize_stream_result` 已提供有用的诊断）。

#### 对话循环可靠性

- **GUI generation 超时保护**：添加 `generation_deadline` — 330s 后强制重置，防止永久 UI 锁定。
- **GUI 事件溢出修复**：`process_pending` 从固定上限事件处理改为无限制 `while let Ok(...)` 排空，消除高吞吐 token 下的静默事件丢失。
- **GUI 空 generation_id 清理**：当 `generation_id=None` 时添加了孤立空 assistant 消息的回退清理。
- **GUI 阶段同步**：流请求体现在包含 `phase` 字段；`ChatCompleted` 响应携带 `actual_mode` 用于后端模式同步。
- **GUI SSE flush 优化**：`/v1/chat/completions` 流路径从每次事件 flush 改为每 4 事件批量 flush（与 `/chat/stream` 行为一致）。
- **GUI StreamProcessor 字段移除**：消除设置了但从未读取的死字段。
- **GUI split_thinking 死代码修复**：`extra_thinking` 现在在显示前正确与权威 thinking 合并。
- **CLI stdin 异步化**：用 `tokio::io::stdin().lines()` 替换 `spawn_blocking` 以获得响应式的 Ctrl+C 处理。
- **CLI Ctrl+C 可重复**：`signal::ctrl_c()` 现在每轮迭代重新 arm，支持多次中断。
- **CLI 模式持久化**：`/mode` 切换时模式保存到 `goon-cli-mode.json`，启动时恢复。支持 `GOON_DEFAULT_MODE` 环境变量。
- **CLI 失败消息清理**：失败时自动从历史记录中移除 assistant 消息。
- **CLI 输入回压**：`unbounded_channel` → 有界 channel(32) 防止粘贴风暴。
- **CLI 多行输入**：支持反斜线续行、空格续行、括号不平衡检测。

#### ACP/ZED Agent Server 集成

- **平台 profile 注入增强**：`initialize`、`session/new`、`tools/list` 响应现在包含 `platform_metadata`，内含可用模式、能力列表和默认模式。
- **session/prompt 思考正则增强**：现在同时支持 `<thinking>...</thinking>` 和 `__thinking__` 前缀格式。
- **session/close 清理**：Session 关闭和删除现在清理权限状态以阻止过期授权。
- **session/config per-session**：验证并记录 — `session_set_config_option` 已通过 `acp_session_state().entry()` 实现 per-session 存储。
- **MCP notifications/initialized**：现在返回 `id: Some(Value::Null)` 哨兵值（由 dispatch 层跳过发送），防止 Zed 客户端记录无关错误。

#### 并发与配置

- **AgentFactory 锁统一**：将 `instances` 和 `expirations` 合并为单个 Mutex 保护的 `AgentFactoryInner`，消除了容量检查与插入之间的 TOCTOU 竞争，移除了 `destroy_agent` 的双锁崩溃安全缺口。
- **Config 热重载**：完整 config 克隆从 2 次减为 1 次；通过释放写锁前捕获快照消除了过时读取竞争。
- **Config 解析器修复**：自动规则现在在 schema 迁移后应用，避免引用过期的阶段名称；写入磁盘前验证解析结果。
- **Config serde 安全**：`AppConfig` 中的 `flow` 字段现在有 `#[serde(default)]` — 缺失 `[flow]` 部分时使用默认值而非失败。

#### 代码质量

- **`is_clean()` cfg(test) 修复**：从 `#[cfg(test)] pub fn` 改为 `#[cfg(test)] pub(crate) fn` — 先前形式在非测试构建中被其他模块调用时会编译失败。
- **18 行注释的 criterion 基准代码** 从 `adaptive_selector.rs` 中删除。
- **`connect_direct_for_test` 重命名** 为 `connect_direct` — 该方法在生产环境和测试中均有使用。
- **所有 dead_code allow 已清理**：生产代码中零 `#[allow(dead_code)]` 或 `#[expect(dead_code)]`。
- **所有 profile 零警告**：`local`、`simple-server` 0 警告；`multi-users-server` 仅有 2 个预先存在的 `config_path` 警告。

### 验证

- **测试**：2069 通过，0 失败，0 忽略（完整套件）。
- **GUI 测试**：25 通过，0 失败。
- **MCP 测试**：20 通过，0 失败。
- **Agent Factory 测试**：12 通过，0 失败。
- **Config 核心测试**：49 通过，0 失败。
- **ACP 测试**：385 通过，0 失败。
- **Clippy**：`-D warnings` 零违规（后端 + GUI）。
- **Profiles**：`local`、`simple-server` 零警告。

## [1.3.0] - 2026-06-23

### 架构 — 锁竞争消除（第四阶段）

本版本完成整个运行时系统级的锁架构升级，通过精准的锁类型选择和基于 channel 的写操作卸载，消除了 12 个热路径互斥锁争用点。

#### Mutex → RwLock（读重型路径）

- **agent_router**（1 文件）：全局路由统计表从 `Mutex` 升级为 `RwLock`。并发的 agent 路由查询不再相互串行化。
- **agent_preference**（1 文件）：Agent 到阶段绑定状态从 `StdMutex` 升级为 `RwLock`。每个请求的阶段解析读取可并行执行。
- **semantic_cache**（4 文件）：语义响应缓存从 `StdMutex` 升级为 `RwLock`。近重复请求检测读取现可并发。
- **skill_registry**（17 文件）：全局技能注册表从 `Arc<StdMutex>` 升级为 `Arc<RwLock>`，覆盖编排层、MCP 处理器、能力总线、自治适配器等整个调用链。每次技能评分和检索读取现无锁争用。
- **maintenance_tracker**（3 文件）：100% 只读诊断快照 — RwLock 消除不必要的串行化。
- **inflight_limiter**（2 文件）：100% 只读诊断快照 — RwLock 消除不必要的串行化。
- **lifecycle_state**（3 文件）：80/20 读写比。服务器健康检查（读）不再互斥；唯一的 shutdown 写入不受影响。
- **review_timeout_policy**（1 文件）：死字段随架构一致性转换。

#### Mutex → mpsc 通道（写重型热路径）

- **online_controller**（6 文件，13 个调用点）：最重要的架构变更。请求热路径上的 9 个只写 outcome 记录调用（record_agent_outcome、record_phase_outcome）现通过 `mpsc::UnboundedSender` 分发 — 零锁争用。4 个需要返回值的读取调用（rank_agent_names_for_phase、recommend_phase、phase_policy_snapshot）保留同步锁访问。后台事件处理器异步排空通道并应用变更。

#### Clone 死代码移除

- **HyperResilienceEngine**（1 文件）：移除了顺次获取 5 个内部锁的 `Clone` 实现。生产代码从未调用此实现（所有实例通过 `Arc` 共享），使其既是死代码又是潜在死锁风险。

#### 语义精确性 — 有意保留的 StdMutex 字段

经过分析，`ResilienceContext` 中的三个字段保留为 `StdMutex`，因为 RwLock 不会带来有意义收益：
- **circuit_breakers**（62% 读，38% 写）：内部双重锁使外层 RwLock 无意义。
- **failure_prevention**（50/50 平衡）：RwLock 写路径与 Mutex 相同，无收益。
- **phase_rate_limiter**（60% 读，40% 写 per-request）：每个请求的令牌桶变更是写操作；RwLock 同样串行化。

### 死代码消除

- **run_health_check**：将空桩函数替换为真实的子系统验证（治理、运行时配置、agent 注册表）。
- **BrainLoopReport** 和 `with_diagnostic_feedback`：移除了 reflection 模块中废弃的结构和方法。
- **Pipeline 变体**：移除了 5 个死的 `PipelineStep` 和 `PipelineErrorStrategy` 变体（Parallel、Sequence、Conditional、Stop、Rollback）及所有相关分支函数和测试。
- **execute_with_two_phase_coordination**：移除了整个 2PC 协调器函数（预留 F-GAP-49，未使用）。
- **PluginRegistry::unregister**、**SkillDiscovery::invalidate_cache**、**session_context** 死方法：移除了单独标记的死代码。
- **DiagnosticFeedbackEngine** 死方法链：移除了 `has_errors`、`recommend_repair`、`latest_batch` 及 3 个相关测试。
- **sign_request**、**make_signature_for_test**、**subscriber_count**：移除了零调用者的纯测试辅助函数。
- **ApprovalPolicySuggester::new()**：移除了冗余构造函数（Default trait 提供相同功能）。
- **HyperResilienceEngine::clone()**：移除了死 Clone 实现（5 锁顺次获取）。
- **e2e 测试死导入**：移除了 `ImageAttachment`、`MtlsConfig`、`sign_request` 导入及相关测试代码。

### 构建与 Lint 清理

- **temp_env 依赖**：从可选的 feature 门控依赖移至 `[dev-dependencies]` — 解决了 `federated_transport.rs` 中的 3 个测试编译失败。
- **BrainLoopReport 可见性**：添加了 `pub use reflection::BrainLoopReport` — 解决了测试编译错误。
- **空 coordinator 模块**：移除了 `pub mod coordinator` 并删除了空文件 — 消除了 6 个死代码警告。
- **Clippy lint 修复**：解决了 7 个 lint（manual_pattern_char_comparison、len_zero、manual_is_multiple_of ×4、needless_borrow、for_kv_map、manual_range_contains、未使用导入）。

### 测试可靠性

- **video_processor 测试**：修复了不一致的 ffmpeg 检测 — 测试现在通过 match 统一处理 ffmpeg 可用和不可用情况，消除了假性 panic。
- **shell_exec 测试**：使其对环境更鲁棒 — 在没有 `sh` 访问权限的系统（macOS CI）上接受超时为有效结果，不再 panic。

### 性能

- **I18nManager::clone()**：从深度拷贝所有翻译（每次 clone O(n)）重新设计为 `Arc<I18nInner>` 共享（O(1)）。之前的实现在每次 clone 时拷贝整个 `HashMap<Language, HashMap<String, String>>`。

---

## [1.2.0] - 2026-06-10

[For previous versions, see English CHANGELOG.md]
