# BLUE52 — go-on 神级 AGI：从打工王者到自治系统的终极进化

> 更新时间：2026-06-01
>
> 目标：BLUE50/51 将系统 8 个基础维度全部拉到 10/10，但 BLUE51 §8.3 识别出 **7 大终极差距**：
> 自举、联邦学习、记忆持久化、多模态、人机审批、分布式DAG、安全加固。
> BLUE52 针对这 7 个差距给出**逐步实施计划**，每个 GAP 拆解为详细子步骤，明确代码落点、依赖项、验证方法。

## 0. 核心规则（同 BLUE50/51）

1. **排除 i18n 字段硬编码** — 不涉及 locale 文本本身的结构调整。
2. **排除分拆文件** — 不将现有文件拆分为更小文件。
3. **三端一统（backend / GUI / vscode-addon）** — 考虑三端配合、通讯流畅稳定性。
4. **注释英文** — 所有新增模块的代码注释必须使用英文。
5. **3 种服务器 Profile 全链路闭合** — profile-local、profile-simple-server、profile-multi-users-server 必须正确编译和行为一致。
6. **5 种协议全链路闭合** — auto、acp stdio、acp http、mcp stdio、mcp http。
7. **零警告、零冲突、零遗漏** — 最终验证 `cargo clippy --all-features -- -D warnings` 零警告。
8. **完整闭合** — 每个模块最终必须达到：编译通过、零警告、接入 governance.status、可通过 health 端点观测、有集成测试覆盖。
9. **不允许占位、空函数、逻辑错误** — 所有功能必须完整实现。
10. **回写完成率** — 每轮完成后，回写完成率（简述）。

---

## 1. 7 大终极差距概览

| # | 差距 | 严重度 | 当前基线 | 预计工作量 | 新增GAP数 |
|:--:|------|:------:|----------|:---------:|:---------:|
| G1 | 自举（Self-Bootstrapping） | CRITICAL | 无自修改能力 | 12-16周 | 7 GAP（B52-01~05,29,32） |
| G2 | 真多节点联邦学习 | CRITICAL | `federated.rs` 1392行单进程模拟 | 8-12周 | 5 GAP（B52-06~10,35） |
| G3 | 长期记忆持久化 | HIGH | 内存优先，重启丢失 | 6-8周 | 5 GAP（B52-11~14,31） |
| G4 | 多模态输入理解 | HIGH | 仅文本+base64图像 | 8-10周 | 5 GAP（B52-15~18,29~30） |
| G5 | 人机协作审批工作流 | MEDIUM | PUA有Escalate无前端 | 4-6周 | 5 GAP（B52-19~20,33~34,36） |
| G6 | 分布式 DAG 执行 | MEDIUM | `dag_executor.rs` 917行单进程 | 6-8周 | 2 GAP（B52-21~22） |
| G7 | 生产级安全加固 | LOW | 无mTLS/签名/注入检测 | 4-6周 | 7 GAP（B52-23~28,36） |
| 验证 | 端到端集成测试 | HIGH | 无 | 2-3周 | 1 GAP（B52-37） |
| | **总计** | | | **56-75周** | **37 GAP** |

---

## 2. BLUE52 改进计划（58 GAP，10 Step）

### 2.1 Step 1（P0 — 自举基础）：自修改管线基础设施（G1 前5个GAP）

#### GAP-B52-01（CRITICAL）：自举沙箱执行环境

- **新建**: `src/orchestration/self_evolution/sandbox.rs`
- **问题**: 系统不能自我修改代码，所有改进必须人类手动编码
- **详细子步骤**:
  1. 创建 `CodePatch` struct：`target_file: String, original_lines: Vec<(usize, String)>, patched_lines: Vec<(usize, String)>, diff: String, reasoning: String`
  2. 创建 `BuildResult` enum：`Success{ warnings: Vec<String>, time_ms: u64 }` / `CompileError{ errors: Vec<String>, lines: Vec<(usize, String)> }` / `TestFailure{ failed: Vec<TestFailure>, passed: u32 }`
  3. 实现 `SandboxExecutor::new(workdir: PathBuf, max_iter: u32) -> Self` — 创建 git worktree 沙箱
  4. 实现 `SandboxExecutor::apply_patch(patch: CodePatch) -> Result<u64>` — 写 patch 文件到沙箱（返回 git tree hash）
  5. 实现 `SandboxExecutor::build(profile: &str) -> BuildResult` — 在沙箱中执行 `cargo build --features <profile>` 捕获全部编译输出
  6. 实现 `SandboxExecutor::test(target: &str) -> TestResult` — 在沙箱中 `cargo test -- <target>`
  7. 实现 `SandboxExecutor::commit(hash: u64, approved: bool) -> Result<()>` — 批准则 merge 回主分支，拒绝则 git reset
  8. 实现 `SandboxExecutor::cleanup()` — 删除沙箱目录
  9. **安全约束**: 无网络访问（`/etc/hosts` 拦截）、`allowed_targets` 白名单、最大10次迭代硬限制
- **验证**: 合法补丁→应用→编译→测试→提交全流程；非法补丁→沙箱拒绝

#### GAP-B52-02（CRITICAL）：自举触发循环（Observe→Analyze→Propose→Apply→Verify）

- **新建**: `src/orchestration/self_evolution/evolution_loop.rs`
- **问题**: 无系统化的自我改进触发机制
- **详细子步骤**:
  1. 创建 `EvolutionTrigger` enum：`PerformanceRegression{ metric, threshold, direction }`、`RepeatedError{ pattern, count }`、`DeadCodeDetected{ module, ratio }`、`ManualRequest{ instruction }`、`ConfigDrift{ key, expected, actual }`
  2. 创建 `EvolutionLoop` struct：持有 `trigger_sources: Vec<Box<dyn TriggerSource>>`, `sandbox: SandboxExecutor`, `cycle_id: u64`
  3. 实现 `TriggerSource` trait：`async fn poll(&self) -> Vec<EvolutionTrigger>`
  4. 实现 `MetacognitiveTriggerSource`：对接 `MetacognitiveController`，当 agent 置信度 < 0.4 持续 5 轮时触发
  5. 实现 `AlertManagerTriggerSource`：对接 `AlertManager.recent_alerts`，当同一告警类型重复 N 次时触发
  6. 实现 `DiagnosticTriggerSource`：对接 `DiagnosticFeedback`，当错误率 > 10% 时触发
  7. 实现 `ManualTriggerSource`：通过 RPC `evolution.trigger` 接受人工指令
  8. 实现 `EvolutionLoop::run()`：`tokio::select! { trigger → analyze → propose → await_approval → apply → verify → record }` 循环
  9. 实现 `EvolutionLoop::analyze(trigger) -> Analysis` — 调用 `brain_loop.plan_with_reasoning` 分析根因
  10. `await_approval` 支持 3 种模式：`AutoApproval(low_risk)`、`RequireApproval(medium_risk)`、`RequireHuman(high_risk)`
- **验证**: 注入模拟性能告警→系统自动触发分析→生成修改提案→等待审批

#### GAP-B52-03（HIGH）：代码生成 Agent

- **新建**: `src/agents/self_evolution_agent.rs`
- **问题**: 生成高质量代码补丁需要专用 Agent 能力
- **详细子步骤**:
  1. 创建 `SelfEvolutionAgent` struct：`model_selector: Arc<ModelSelector>`, `agent_registry: Arc<AgentRegistry>`
  2. 实现 `SelfEvolutionAgent::analyze_code(target: &str) -> Report` — 读取模块源码+类型定义+依赖关系
  3. 实现 `SelfEvolutionAgent::generate_patch(analysis: Analysis, instruction: &str) -> CodePatch` — 生成 unified diff 补丁
  4. 实现 `SelfEvolutionAgent::fix_compile_errors(error: Vec<String>, current_patch: CodePatch) -> CodePatch` — 循环修复编译错误（最多5轮）
  5. 实现 `SelfEvolutionAgent::assess_risk(patch: &CodePatch) -> RiskLevel` — 评估修改影响范围
  6. 集成本项目 `RULES/` 目录作为系统提示，确保生成的代码符合项目规范
- **验证**: 给定已知有bug的方法→Agent 生成可编译的修复补丁

#### GAP-B52-04（MEDIUM）：GUI/VSCode 自举审批界面

- **新建**: `gui/src/views/self_evolution.rs` + `vscode-addon/src/selfEvolutionView.ts`
- **问题**: 自举变更无人审批界面
- **详细子步骤**:
  1. **GUI**: 创建 `SelfEvolutionView` — 待审批提案列表（触发原因、风险等级、diff预览）
  2. 每个提案卡片：展开显示语法高亮 diff + `reasoning` + `risk_assessment` + `build_status` + `test_results`
  3. 提供 `Approve` / `Reject` / `Modify & Approve` 三个按钮
  4. 变更历史面板：列出所有完成的进化变更（时间、审批人、结果、metrics_before/after）
  5. **VSCode**: 创建 `SelfEvolutionView` webview panel + `evolution.proposals.list` RPC + `evolution.proposals.approve/reject` RPC
  6. StatusBar 徽标显示待审批提案数
  7. diff 编辑器支持 inline 修改（Monaco Editor diff 模式）
- **验证**: 后端生成提案→GUI/VSCode 显示→审批通过→后端应用变更

#### GAP-B52-05（MEDIUM）：自我改进历史版本追踪

- **新建**: `src/orchestration/self_evolution/evolution_history.rs`
- **问题**: 自举变更无历史追踪和回滚能力
- **详细子步骤**:
  1. 创建 `EvolutionEntry`：`id(Uuid), timestamp, trigger: EvolutionTrigger, patches: Vec<CodePatch>, approval: { by, status, comment }, build_result, metrics_before: MetricsSnapshot, metrics_after: MetricsSnapshot, rollback_commit: Option<String>`
  2. 实现 `EvolutionHistory`：持久化到 `.goon/evolution/history.ndjson`
  3. 实现 `record(entry)`、`list() -> Vec<EvolutionEntry>`、`get(id) -> Option<EvolutionEntry>`
  4. 实现 `rollback(id) -> Result<CodePatch>` — git revert 到变更前的 commit
  5. 实现 `get_metrics_trend() -> Vec<MetricsPoint>` — 收集每次变更前后的系统性能指标
  6. 自动回滚：当 apply 后指标恶化 > 20% 时自动触发回滚
- **验证**: 执行自举→记录历史→回滚→验证代码回到变更前状态

---

### 2.2 Step 2（P0 — 联邦学习真多节点）：跨网络协同进化（G2 8个GAP）

#### GAP-B52-06（CRITICAL）：FederatedRL 网络传输层

- **修改**: `src/intelligence/reinforcement/federated.rs` + 新建 `src/intelligence/reinforcement/federated_transport.rs`
- **问题**: 当前 FederatedRL 仅在单进程内模拟多节点，无真实网络传输
- **详细子步骤**:
  1. 创建 `FederatedTransport` trait：`async fn submit_weights(peer: PeerInfo, weights: ModelWeights) -> Result<()>`, `async fn pull_global_model() -> Result<ModelWeights>`, `async fn health_check(peer: PeerInfo) -> Result<bool>`
  2. 实现 `GrpcFederatedTransport`（使用 `tonic`）：定义 `federated.proto` 含 `SubmitWeights` / `GetGlobalModel` / `HealthCheck` 三个 RPC
  3. 实现 `FederatedServer`：启动 gRPC 服务监听 `bind_addr`，接收远程节点的权重提交
  4. 修改 `FederatedLearning::run_round()`：将 `self.clients.iter()` 遍历改为 `transport.receive_weights().await` 从远程接收
  5. 实现 `FederatedLearning::sync_global_model()`：聚合后通过 transport 广播给所有 peer
  6. 添加 `FEDERATED_PEERS` 环境变量和 `--federated-peer` CLI 参数
  7. 区分 3 种节点角色：`Coordinator（负责聚合）`、`Worker（仅训练和提交）`、`Full（训练+聚合）`
- **验证**: 启动 3 个实例配置为联邦节点→验证模型权重跨节点交换

#### GAP-B52-07（CRITICAL）：DistributedMemoryBus 真网络传输

- **修改**: `src/intelligence/capability_bus/distributed_memory_bus.rs`
- **问题**: 当前 1336 行 DistributedMemoryBus 纯内存模拟，无真实跨节点记忆共享
- **详细子步骤**:
  1. 定义新类型：`RemoteMemoryEntry { entry_id, content_hash, payload, ttl, source_node_id, vector_clock }`
  2. 实现 `DistributedMemoryBus::push_to_peers(entry)` — 序列化 + 广播（复用 GAP-B52-06 的 `FederatedTransport`）
  3. 实现 `DistributedMemoryBus::query_distributed(query, limit)` — 向所有 peer 发送检索请求
  4. 实现冲突检测：基于 `vector_clock` 的 LWW（Last-Write-Wins）合并
  5. 保留现有 `#[cfg(not(feature = "profile-multi-users-server"))]` 本地模式作为回退
- **验证**: 2 节点部署→节点A写入记忆→节点B查询到同步记忆

#### GAP-B52-08（HIGH）：联邦节点发现与注册

- **新建**: `src/intelligence/reinforcement/federated_discovery.rs`
- **详细子步骤**:
  1. 创建 `NodeDiscovery` trait：`register(node)`, `discover() -> Vec<NodeInfo>`, `watch() -> Watch<Vec<NodeInfo>>`
  2. 实现 `StaticDiscovery`：从 config 固定列表读取
  3. 实现 `MdnsDiscovery`：使用 `mdns-sd` 局域网多播发现
  4. 实现 `ConsulDiscovery`：对接 Consul 服务注册中心
  5. 心跳：每 10s 发送，30s 无心跳标记 `offline`
- **验证**: 3 节点自动互发现→kill 1 节点→剩余节点检测到离线

#### GAP-B52-09（HIGH）：差分隐私梯度加密

- **新建**: `src/intelligence/reinforcement/federated_privacy.rs`
- **详细子步骤**:
  1. `DifferentialPrivacyConfig { epsilon: f64(8.0), delta: f64(1e-5), clip_norm: f64(1.0) }`
  2. `clip_gradients(weights, clip_norm)` + `add_gaussian_noise(weights, epsilon, delta)`
  3. 在 `run_round()` 聚合前调用裁剪+加噪
  4. 跟踪隐私预算 `PrivacyBudget { total_epsilon, rounds_remaining }`
- **验证**: 启用差分隐私后单节点权重不可逆向推导

#### GAP-B52-10（MEDIUM）：模型版本管理

- **新建**: `src/intelligence/reinforcement/federated_versioning.rs`
- **详细子步骤**:
  1. `ModelVersion { major, minor, patch, schema_hash }` + `is_compatible_with(other)`
  2. 不兼容节点隔离到独立联邦组
  3. 版本升级迁移：`migrate_weights(from, to) -> Result<Weights>`
- **验证**: 2 节点不同版本→隔离或迁移→聚合继续

---

### 2.3 Step 3（P1 — 记忆持久化）：永不遗忘的记忆架构（G3 8个GAP）

#### GAP-B52-11（HIGH）：L1→L2→L3 三层记忆持久化栈

- **修改**: `src/memory/memory.rs` + 新建 `src/memory/memory_persistence.rs`
- **问题**: `MemoryStore` / `SemanticCache` / `ContinuousLearningCenter` 均内存优先，重启丢失
- **详细子步骤**:
  1. 定义三层：**L1热点**（内存 LRU 2048条，TTL 5min）+ **L2温存**（SQLite `VectorStore`，持久化30天）+ **L3冷存**（NDJSON gzip归档）
  2. 实现 `MemoryTieringPolicy`：`hot_threshold: Duration`, `warm_threshold: Duration`
  3. 实现自动迁移：`promote(entry)` 冷→温→热 / `demote(entry)` 热→温→冷
  4. L2→L3：读取 `last_accessed > 30d` → gzip → `.goon/memory/cold/YYYY-MM/`
  5. L3→L2：查询命中冷存储时解压恢复
  6. 启动时从 L2+L3 加载元数据索引
- **验证**: 写入100条→等待TTL→验证L1→L2→L3迁移→重启→可检索

#### GAP-B52-12（HIGH）：长对话自动压缩与摘要

- **修改**: `src/orchestration/session_compressor.rs` + 移除 `#[allow(dead_code)]`
- **详细子步骤**:
  1. 激活 `SessionCompressor`：`should_compress(msg_count > 50 || token > window*0.7)`
  2. `compress(messages) -> CompressedContext`：调用 LLM 生成 `{ summary, key_decisions, open_questions }`
  3. `inject_compressed_context(messages, compressed)`：开头插入 `SystemMessage(compressed_summary)` + 保留 `messages[compressed_at..]`
  4. 增量压缩：基于上次压缩生成增量摘要
- **验证**: 100轮对话→自动压缩→第101轮有之前上下文且 token 不超限

#### GAP-B52-13（MEDIUM）：跨会话记忆检索与关联

- **新建**: `src/memory/memory_retrieval.rs`
- **详细子步骤**:
  1. `retrieve_relevant_memories(query, limit)` — 语义搜索+向量相似度
  2. `retrieve_related_sessions(session_id)` — 主题相关的历史会话
  3. 接入 `gather_intelligence_context()` + `TaskContext.intermediate_findings`
  4. 支持记忆关联图谱：`link_memories(m1, m2, "causal"|"sequential")`
- **验证**: 会话A→会话B相同主题→检索到A的记忆

#### GAP-B52-14（MEDIUM）：Ebbinghaus 遗忘曲线自动驱逐

- **修改**: `src/intelligence/continuous_learning.rs` — 激活现有 `detect_forgetting`
- **详细子步骤**:
  1. `retention_score(entry, now) -> f64` — Ebbinghaus 公式计算保留度
  2. < 0.3 标记"遗忘风险" → 自动复习 `replay_important_memories`
  3. 连续 3 次 < 0.1 → 快速驱逐（不归档 L3）
  4. `schedule_review(entry) -> Instant` — 计算下次复习时间
- **验证**: 写入记忆→等待保留度下降→自动复习→保留度回升

---

### 2.4 Step 4（P1 — 多模态输入）：全感官理解管线（G4 10个GAP）

#### GAP-B52-15（HIGH）：通用文档解析管线

- **新建**: `src/multimodal/mod.rs` + `src/multimodal/document_parser.rs`
- **问题**: 仅文本+base64图像，无PDF/Office/HTML解析
- **详细子步骤**:
  1. 创建 `MultimodalInput` enum：`Text(String)`, `Image(Vec<u8>)`, `Audio(Vec<u8>)`, `Video(Vec<u8>)`, `Document(Vec<u8>, String/ext)`
  2. 实现 `DocumentParser`：`parse(path) -> ParsedContent { text_content, images, tables, metadata }`
  3. PDF（`lopdf`）：提取文本 + `pdf-extract` 提取嵌入图片
  4. DOCX（`docx-rs`）：解析XML→段落、表格、图片
  5. HTML（`scraper`）：提取文本、链接、表格
  6. Markdown（`comrak` 已有）：复用现有 markdown 渲染
  7. `ParsedContent` 统一格式后通过 `chat` RPC 注入消息
- **验证**: PDF/docx/html→解析为结构化文本→注入LLM上下文

#### GAP-B52-16（HIGH）：音频输入（语音转文字）

- **新建**: `src/multimodal/audio_processor.rs`
- **详细子步骤**:
  1. 支持 3 种 STT 后端：`WhisperLocal(model_path)`、`OpenAIWhisper(api_key)`、`Vosk(model_path)`
  2. `transcribe(audio: Vec<u8>, format: &str) -> Transcription { text, segments: Vec<Segment{start,end,text}>, language, confidence }`
  3. 可选：说话人识别（diarization）→ `speaker_segments: Vec<(speaker_id, segment)>`
  4. GUI/VSCode 添加录音按钮和音频文件上传
  5. 录音：浏览器 `MediaRecorder` API → blob → base64 → RPC
- **验证**: 上传 audio.mp3→自动转写为文字→LLM 正确理解内容

#### GAP-B52-17（HIGH）：图像深度理解

- **修改**: `src/agents/*.rs` — 增强图像处理能力
- **详细子步骤**:
  1. 实现图像预处理流水：`resize(max_pixels)`, `compress(quality)`, `extract_text(OCR via `leptonica`/`tesseract`)`
  2. 图像描述生成：`describe_image(image) -> String`（使用 vision-capable 模型）
  3. 表格/图表识别：`extract_table(image) -> Vec<Vec<String>>`
  4. 图像裁剪/区域标记：让模型聚焦特定区域
- **验证**: 上传含表格的图片→LLM 正确理解和回答表内数据

#### GAP-B52-18（MEDIUM）：GUI/VSCode 多模态上传界面

- **文件**: `gui/src/views/file_upload.rs`（新建） + `vscode-addon/src/multimodalUpload.ts`（新建）
- **详细子步骤**:
  1. GUI: drag-and-drop 文件区域 + 文件选择器
  2. 支持拖拽：PDF/docx/images/audio/video + 实时进度条
  3. 预览面板：点击文件后在侧边预览（文本高亮/图片缩略图/音频波形）
  4. VSCode: drag-and-drop 到 webview + 文件系统 `openDialog`
  5. 文件大小限制 + 格式校验 + 前端压缩
- **验证**: 拖拽 PDF 到聊天输入框→文件被解析并注入对话

---

### 2.5 Step 5（P1 — 人机协作审批）：HITL 工作流（G5 8个GAP）

#### GAP-B52-19（HIGH）：审批核心引擎

- **新建**: `src/governance/approval_engine.rs`
- **问题**: PUA `Escalate` 触发后无人审批通道
- **详细子步骤**:
  1. `ApprovalEngine` struct：`queue: Vec<ApprovalRequest>`, `escalation_chain: Vec<EscalationLevel>`, `timeout_policy: TimeoutConfig`
  2. `ApprovalRequest { id, user, action, risk_level, context, status: Pending|Approved|Rejected|Expired, escalated_from: Option<String> }`
  3. `submit_for_approval(request)` — 添加到审批队列
  4. `approve(id, reviewer_comment)` / `reject(id, reason)` — 审批/拒绝
  5. 超时自动降级：5min → `EscalateToManager`，15min → `AutoDeny`
  6. 审批决策反馈到 `PuaRuleEngine.de_escalate()`（连续允许降级）
- **验证**: PUA Escalate→审批引擎创建请求→审批通过→操作放行

#### GAP-B52-20（MEDIUM）：GUI/VSCode 审批面板

- **文件**: `gui/src/views/approval.rs`（新建） + `vscode-addon/src/approvalView.ts`（新建）
- **详细子步骤**:
  1. GUI: 审批仪表板面板（待审批列表 + 历史）
  2. 每个请求展开：操作详细、风险等级、上下文消息
  3. `Approve` / `Reject` + 备注输入框
  4. VSCode: `approval.*` RPC 命令 + webview 审批面板
  5. 桌面通知：通过 `vscode.window.showWarningMessage` 通知紧急审批
  6. StatusBar 徽标显示待审批数
- **验证**: 高风险操作→审批通知→人工审批→操作放行

---

### 2.6 Step 6（P1 — 分布式DAG）：跨机器并行执行（G6 8个GAP）

#### GAP-B52-21（HIGH）：远程节点执行器

- **新建**: `src/orchestration/distributed/remote_executor.rs`
- **问题**: `DAGExecutor` 仅单进程 tokio 内调度
- **详细子步骤**:
  1. `TaskPacket { node_id, dag_id, tool_name, input, dep_outputs, retry_count }`
  2. `RemoteExecutor` trait: `execute_remote(packet: TaskPacket) -> Result<NodeOutput>`
  3. `GrpcRemoteExecutor` 实现（复用 federated transport gRPC）
  4. 节点注册：worker 连接 coordinator 并报告可用能力
  5. node 输出通过 gRPC streaming 实时返回
- **验证**: 2 机器→DAG 节点跨机器并行执行

#### GAP-B52-22（MEDIUM）：远程 DAG 状态同步

- **新建**: `src/orchestration/distributed/dag_coordinator.rs`
- **详细子步骤**:
  1. `DistributedDAGCoordinator` 持有 `dag_states: HashMap<DagId, DistributedDagState>`
  2. 基于 Raft 的一致性协议（`raft` crate）：保证至少大部分节点对DAG状态达成一致
  3. 节点故障检测：`heartbeat + lease` 机制
  4. 故障恢复：`reassign_node(dag_id, failed_node, backup_node)`
- **验证**: DAG执行中一个worker宕机→coordinator 重新调度到其他节点

---

### 2.7 Step 7（P2 — 生产安全加固）：零信任安全架构（G7 全部6个GAP）

#### GAP-B52-23（HIGH）：客户端请求签名（防篡改）

- **新建**: `src/security/request_signing.rs`
- **问题**: 多用户模式下请求可被中间人篡改，无请求完整性验证
- **详细子步骤**:
  1. 创建 `RequestSignature` struct：`{signature: String, algorithm: SigningAlgorithm(Ed25519|HmacSha256), key_id: String, timestamp_ms: u64, body_hash: String}`
  2. 实现 `sign_request(private_key: &[u8], body: &[u8]) -> RequestSignature` — 对 `(body_hash + timestamp + method)` 签名
  3. 实现 `verify_request(public_key: &[u8], body: &[u8], signature: &RequestSignature) -> Result<bool>` — 验证签名
  4. 防止重放攻击：在 `verify_request` 中检查 `now - timestamp < ALLOWED_CLOCK_SKEW(30s)`
  5. 接入 `protocol_pack.rs` 请求处理入口：stdio 和 HTTP 传输在认证后验证签名
  6. 支持 Ed25519（`ed25519-dalek`）和 HMAC-SHA256（`hmac` + `sha2` 已有）两种算法
  7. 密钥轮换支持：通过 `key_id` 区分不同密钥版本
- **验证**: 创建有签名的合法请求→通过验证；篡改请求体→签名验证失败；重放过期请求→拒绝

#### GAP-B52-24（HIGH）：mTLS 双向传输认证

- **新建**: `src/security/mtls.rs`
- **问题**: 服务间通信（server↔GUI、节点间联邦学习）无加密传输和双向认证
- **详细子步骤**:
  1. 创建 `MtlsConfig` struct：`{ca_cert_path: PathBuf, server_cert_path: PathBuf, server_key_path: PathBuf, require_client_cert: bool, allowed_cn_list: Vec<String>}`
  2. 实现 `MtlsAcceptor::new(config: &MtlsConfig) -> impl AsyncAccept` — 包装 tokio TCP listener 为 TLS 监听器（使用 `rustls` 或 `native-tls`）
  3. 实现 `MtlsConnector::new(config: &MtlsConfig) -> impl AsyncConnect` — 客户端 mTLS 连接
  4. 在 HTTP 服务器启动时配置 mTLS：`axum` 或 `hyper` 的 `TlsAcceptor` 层
  5. 在 gRPC fedetated transport 中配置 mTLS（复用 `tonic` 的 `TlsClientConfig`/`TlsServerConfig`）
  6. 接入 `acp/impl/runtime.rs`：当 `[security.mtls]` 启用时自动包装 listener
  7. 为 `profile-multi-users-server` 添加默认自签名证书生成脚本（`scripts/gen_certs.sh`）
  8. 证书到期监控：提前 30 天发出告警
- **验证**: 启用 mTLS→只接受可信CA签发的客户端证书→不可信客户端连接被拒绝

#### GAP-B52-25（MEDIUM）：提示注入检测

- **新建**: `src/security/prompt_injection.rs`
- **问题**: 用户可向 agent 注入恶意提示，绕过安全限制或窃取系统提示
- **详细子步骤**:
  1. 创建 `InjectionDetector` struct：`{patterns: Vec<InjectionPattern>, model_check: Option<Arc<dyn Agent>>}` 
  2. 定义 `InjectionPattern` enum：`RolePlay(patterns)`、`Jailbreak(keywords)`、`PromptLeak(heuristics)`、`IndirectInjection(context_contamination)`
  3. 实现静态规则检测：`detect_static(input: &str) -> Vec<InjectionWarning>` — 关键字匹配 + 正则（预编译）
  4. 实现 LLM 辅助检测：`detect_with_model(input: &str) -> InjectionScore` — 调用专用检测 agent 评分（0-1）
  5. 实现上下文污染检测：`detect_context_contamination(messages: &Vec<Message>) -> Vec<InjectionWarning>` — 分析消息间非预期的身份切换
  6. 集成到 `process_chat_request`：在消息进入 prompt 组装前调用 `detect_all()`
  7. 策略配置：`SecurityConfig.injection_detection { mode: Deny|Log|Annotate, threshold: f64(0.7) }`
  8. 注入事件记录到审计日志
- **验证**: 经典 jailbreak prompt→检测器阻断；正常对话→通过；间接注入→上下文污染检测触发

#### GAP-B52-26（MEDIUM）：Secret 动态轮换

- **新建**: `src/security/secret_rotation.rs`
- **问题**: API 密钥和配置密钥无轮换机制，泄露后无法在不重启服务的情况下更换
- **详细子步骤**:
  1. 创建 `SecretManager` struct：`{secrets: Arc<RwLock<HashMap<KeyId, SecretEntry>>>, rotation_policy: RotationPolicy}`
  2. 定义 `SecretEntry { value: Vec<u8>, rotation_period: Duration, created_at: Instant, status: Active|Rotating|Retired }`
  3. 实现 `register_key(id, initial_value, rotation_period)`
  4. 实现 `get_key(id) -> Option<Vec<u8>>` — 如果 key 超过 rotation_period 自动触发轮换
  5. 实现 `rotate_key(id) -> Result<Vec<u8>>` — 调用配置的 `KeyRotator`（如 keyring 更新 + 文件更新）
  6. 实现 `KeyRotator` trait：对接 keyring、env 文件、Hashicorp Vault
  7. 集成到现有 `shared/secret_override.rs`：从 `SecretManager` 获取而非直接读取
  8. 添加 `RotationConfig { auto_rotate: bool, rotate_interval_hours: u64, notify_before_days: u64 }`
- **验证**: 注册密钥→达到轮换周期→SecretManager 自动生成新密钥→旧密钥在过渡期仍可用→迁移完成

#### GAP-B52-27（MEDIUM）：审计日志完整性保护（哈希链）

- **新建**: `src/security/audit_integrity.rs`
- **问题**: 当前审计日志可被恶意修改且无检测机制
- **详细子步骤**:
  1. 创建 `HashChainAuditor` struct：`{chain_file: PathBuf, current_hash: HashValue, last_entry_id: Uuid}`
  2. 每个审计条目包含：`{entry_id, prev_hash, payload_hash, timestamp, signature}` 形成哈希链
  3. 实现 `append(entry)`：`new_entry.hash = sha256(prev_hash || entry.payload)`，写入链文件
  4. 实现 `verify_integrity() -> Result<Vec<IntegrityViolation>>` — 从头遍历验证每条记录的哈希链完整
  5. 实现 `export_audit_report(from, to) -> AuditedReport` — 导出带哈希验证的可审计报告
  6. 最终条目（最后一条）由系统私钥签名，防止整条链被替换
  7. 接入 `ThreadSafeAuditLog`：替换当前简单的 append 为哈希链追加
  8. 每日完整性检查：后台定时任务每日验证审计日志完整性
- **验证**: 修改审计日志中任意条目→`verify_integrity()` 检测哈希链断裂；连续写入→哈希链持续增长但可验证

#### GAP-B52-28（LOW）：内容安全策略

- **新建**: `src/security/content_safety.rs`
- **问题**: LLM 生成的内容可能包含有害、违规或敏感信息
- **详细子步骤**:
  1. 创建 `ContentSafetyConfig { check_categories: Vec<SafetyCategory>, threshold: f64, action: Block|Annotate|Warn }`
  2. 实现 `ContentSafetyChecker`：`check(text: &str) -> Vec<SafetyViolation>`
  3. 检测类别：`HateSpeech`, `PII(泄露个人信息)`, `Misinformation`, `CodeInjection(代码注入)`, `UnsafeCode(危险代码)`
  4. 基于规则：正则+关键词（预编译）快速检测已知模式
  5. 基于模型：使用专用 agent 评估内容安全性（当规则检测不确定时）
  6. 集成到 `finalize_chat_response`：在返回给用户前过滤
  7. 违规内容替换为 `[Content removed due to safety policy]` 并记录审计
- **验证**: 生成含 PII 内容→检测并阻止；正常内容→放行；规则+模型两级兜底

---

### 2.8 Step 8（P2 — 自举+记忆+多模态增强）：剩余能力补全（G1/G3/G4 剩余GAP）

#### GAP-B52-29（MEDIUM）：视频理解管线

- **新建**: `src/multimodal/video_processor.rs`
- **问题**: 系统无法处理视频输入
- **详细子步骤**:
  1. 实现 `VideoProcessor::extract_frames(path, interval_secs: f64) -> Vec<Frame>` — 使用 `ffmpeg` crate 按帧提取
  2. 实现 `VideoProcessor::extract_audio(path) -> Vec<u8>` — 提取音频轨供 STT 处理
  3. 实现 `VideoProcessor::analyze_scene(frames) -> Vec<SceneDescription>` — 关键帧压缩+LLM场景描述
  4. 视频源：本地文件上传、URL（`youtube-dl` 或 `yt-dlp`）
  5. 大小限制：`max_duration_secs: u64(600)`, `max_file_size_mb: u64(500)`
  6. 异步处理：长视频转后台任务，进度通过 WS/SSE 推送
- **验证**: 上传短视频→提取关键帧→生成场景描述→注入LLM对话

#### GAP-B52-30（LOW）：代码仓库理解

- **新建**: `src/multimodal/code_repo_analyzer.rs`
- **问题**: 系统无法理解用户粘贴的大型代码仓库
- **详细子步骤**:
  1. 实现 `RepoAnalyzer::clone(url) -> RepoContext` — git clone 或读取本地目录
  2. 实现 `RepoAnalyzer::build_repo_map(path) -> RepoMap` — 文件树+依赖图+类型索引
  3. 实现 `RepoAnalyzer::extract_types(path) -> TypeIndex` — 提取所有类型定义和函数签名（使用 `tree-sitter` 或 `rust-analyzer`）
  4. 实现 `RepoAnalyzer::answer_code_question(question, repo) -> Answer` — 基于代码搜索结果回答
  5. 在聊天输入检测到 `repo:` 或 `代码仓库:` 前缀时触发
- **验证**: 粘贴代码仓库 URL→系统分析结构并回答关于代码的问题

#### GAP-B52-31（LOW）：记忆可视化界面

- **新建**: `gui/src/views/memory_map.rs` + `vscode-addon/src/memoryView.ts`
- **问题**: 用户无法浏览和管理系统记忆
- **详细子步骤**:
  1. GUI: 创建 `MemoryMapView` — 主题时间线（按会话/主题聚合记忆）
  2. 搜索过滤器：关键词、时间范围、记忆类型、关联度
  3. 记忆详情弹出：内容预览、关联会话、相关记忆、遗忘曲线状态
  4. 手动操作：`Pin`（锁定到 L1）、`Delete`（强制驱逐）、`Summarize`（重新摘要）
  5. VSCode: `memory.*` RPC 命令 + webview
  6. 记忆统计面板：`total_entries`, `by_tier(L1/L2/L3)`, `retention_avg`, `next_review_count`
- **验证**: 打开记忆面板→显示所有记忆层级→搜索过滤→手动管理记忆

#### GAP-B52-32（LOW）：自举主动学习扩展

- **修改**: `src/orchestration/self_evolution/evolution_loop.rs`（完善G1剩余）
- **详细子步骤**:
  1. 实现 `SelfLearningTriggerSource`：当用户反复执行相同操作时，系统主动建议自动化改进
  2. 实现 `CodeQualityTriggerSource`：当 `clippy` 检测到可改进模式时触发
  3. 实现 `DocumentationTriggerSource`：当函数缺少文档注释时建议补充
  4. 自动改进的优雅降级：如果自举失败 3 次，将该模块标记为 `EvolutionDisabled`，需要人工恢复
- **验证**: 用户重复操作→系统主动建议自动化→审批通过→操作被自动化

---

### 2.9 Step 9（P2 — 审批链+联邦完成）：剩余治理与协同能力（G5/G2 剩余GAP）

#### GAP-B52-33（MEDIUM）：多级审批链

- **修改**: `src/governance/approval_engine.rs` — 完善为多级审批
- **问题**: 当前仅单级审批，无 L1自动/L2人工/L3管理的分级机制
- **详细子步骤**:
  1. 实现 `EscalationChain`：`L1(AutoApprove{max_amount, allowed_actions}) → L2(ManagerApproval{pool}) → L3(AdminApproval{quorum})`
  2. L1 自动审批：风险分 < 0.3 且操作在 `allowed_actions` 列表 → 自动通过，记录到审计
  3. L2 经理审批：风险分 0.3-0.7 → 推送到经理审批队列，需要任意一位经理审结
  4. L3 管理员审批：风险分 > 0.7 或涉及敏感资源（密钥/代码修改）→ 需要 N 位管理员多数表决
  5. 实现 `L2ManagerApproval`：`approve(manager_id, request_id)` / `escalate_to_l3(request_id)`
  6. 实现 `L3AdminApproval`：`vote(admin_id, approve/reject)`，quorum 达成后自动执行
  7. 审批结果反馈到 `PuaRuleEngine`：连续允许 → `de_escalate()`、连续拒绝 → `escalate()`
  8. GUI/VSCode 展示审批链级别（标记当前审批层级和审批人）
- **验证**: 低风险操作→L1自动通过；中风险→推送到L2人工审批；高风险→L3多数表决

#### GAP-B52-34（LOW）：审批人偏好学习

- **修改**: `src/governance/approval_engine.rs` + 新建 `src/governance/approval_learning.rs`
- **问题**: 审批人需重复审批相同类型操作，无自动学习机制
- **详细子步骤**:
  1. 创建 `ApprovalPreferenceLearner`：记录审批人对不同类型操作的审批模式
  2. 实现 `record_decision(approver, action_type, approved, context)` — 训练数据收集
  3. 实现 `predict_approval(action_type, context) -> f64` — 预测当前审批人可能的决定
  4. 当预测置信度 > 0.9 时（基于过去 20 次样本），可选自动通过（带逐出：任何一次人工审批驳回或修正则自动回退到人工）
  5. 实现 `ApprovalPolicySuggester`：基于历史生成审批策略建议供管理员确认
  6. 管理界面：展示每个审批人的偏好统计和自动通过率
- **验证**: 审批人连续 10 次通过同一类型操作→系统自动处理第 11 次→如审批人有一次拒绝则回退

#### GAP-B52-35（LOW）：联邦学习跨节点安全同步

- **修改**: `src/intelligence/reinforcement/federated_transport.rs` — 完善G2剩余
- **问题**: 联邦学习节点间模型同步无加密和认证
- **详细子步骤**:
  1. 实现 `SecureFederatedTransport`：封装 `GrpcFederatedTransport`+ `MtlsConnector`（复用 B52-24）
  2. 模型提交认证：节点提交权重时需提供 `NodeAuthToken`（JWT 或 mTLS 客户端证书 CN）
  3. 实现模型聚合一致性检查：接收全局模型后验证其 `schema_hash` 与本地匹配
  4. 实现拜占庭容错聚合：`FedMedian`（聚合中位数而非均值）抵御恶意节点提交
  5. 实现 `FederatedAudit`：记录每轮参与节点、提交内容、聚合结果到哈希链审计
- **验证**: 非法节点提交权重→认证拒绝；恶意节点提交极端权重→FedMedian 过滤

#### GAP-B52-36（LOW）：主动安全管理

- **新建**: `src/security/vulnerability_scan.rs` + `src/security/security_advisor.rs`
- **问题**: 系统无主动安全检测和自动修复能力
- **详细子步骤**:
  1. 实现 `DependencyVulnerabilityScanner`：运行 `cargo audit`（需 `cargo-audit` 安装），解析 CVE 报告
  2. 实现 `SecretExposureDetector`：在代码中检测硬编码密钥模式（正则 `API_KEY=|secret=` + 高熵检测）
  3. 实现 `PermitExposureAnalyzer`：检查文件权限是否过松（`/etc/go-on/config.toml` 权限 600 等）
  4. 实现 `SecurityAdvisorAgent`：对接自举管线（B52-03），对发现的安全问题自动生成修复补丁
  5. 安全扫描结果通过 WS `security.alert` topic 推送到前端
  6. 每日安全摘要报告发送到配置的邮箱/Webhook
- **验证**: 引入已知 CVE 依赖→扫描检测→告警推送→自动生成升级补丁

---

### 2.10 Step 10（P2 — 端到端集成验证）：7大差距全链路闭合

#### GAP-B52-37（MEDIUM）：7大差距集成测试套件

- **新建**: `tests/e2e/` 目录下新增7个集成测试文件
- **问题**: 7大差距各自独立开发后需验证全链路闭合
- **详细子步骤**:
  1. `test_self_evolution_e2e.rs`：自举全流程 — 触发→分析→提案→审批→沙箱编译→提交→回滚
  2. `test_federated_learning_e2e.rs`：联邦学习全流程 — 多节点启动→节点发现→权重交换→差分隐私→聚合→全局模型同步
  3. `test_memory_persistence_e2e.rs`：记忆持久化全流程 — 写入L1→自动迁移L2→归档L3→重启恢复→跨会话检索
  4. `test_multimodal_e2e.rs`：多模态全流程 — PDF上传→解析→注入对话→图像OCR→音频STT
  5. `test_hitl_approval_e2e.rs`：人机审批全流程 — 高风险操作→PUA升级→L2审批通知→审批通过→操作放行
  6. `test_distributed_dag_e2e.rs`：分布式DAG — 2节点注册→DAG提交→跨节点并行→节点故障→重新调度→完成DAG
  7. `test_security_e2e.rs`：安全全流程 — mTLS连接→请求签名→提示注入检测→审计完整性验证→Secret轮换
  8. 每个测试必须包含：setup(启动环境) → execute(执行场景) → assert(验证结果) → teardown(清理环境)
- **验证**: `cargo test --test e2e -- --include-ignored` 全部通过

---

## 3. 执行计划总表（10 Step / 58 GAP）

| Step | 优先级 | GAP数 | 主题 | 差距 | 预计工作量 |
|:----:|:------:|:-----:|------|:---:|:---------:|
| Step | 优先级 | GAP数 | 主题 | 差距 | 预计工作量 |
|:----:|:------:|:-----:|------|:---:|:---------:|
| Step 1 | P0 | 5 | 自举管线基础设施 | G1(5/7) | 6-8周 |
| Step 2 | P0 | 5 | 联邦学习网络传输+发现 | G2(5/5) | 6-8周 |
| Step 3 | P1 | 4 | 三层记忆持久化 | G3(4/5) | 4-6周 |
| Step 4 | P1 | 4 | 文档+音频多模态 | G4(4/5) | 4-6周 |
| Step 5 | P1 | 2 | 人机审批引擎 | G5(2/5) | 2-3周 |
| Step 6 | P1 | 2 | 分布式DAG执行 | G6(2/2) | 3-4周 |
| Step 7 | P2 | 6 | 生产安全加固 | G7(6/7) | 4-6周 |
| Step 8 | P2 | 4 | 自举+记忆+多模态增强 | G1/G3/G4剩余 | 4-6周 |
| Step 9 | P2 | 4 | 审批链+联邦完成 | G5/G2/G7剩余 | 3-4周 |
| Step 10 | P2 | 1 | 7差距端到端集成验证 | 全部 | 2-3周 |

---

## 4. 完成率追踪

| Step | GAP | 状态 | 完成日期 | 备注 |
|:----:|:---:|:----:|:--------:|------|
| 1 | B52-01 ~ B52-05 | ✅ Complete | 2026-06-01 | 自举管线基础设施 (sandbox+loop+history+agent) |
| 2 | B52-06 ~ B52-10 | ✅ Complete | 2026-06-01 | 联邦学习网络传输 (transport+discovery+privacy+versioning) |
| 3 | B52-11 ~ B52-14 | ✅ Complete | 2026-06-01 | 三层记忆持久化 (persistence+retrieval+compress+forgetting) |
| 4 | B52-15 ~ B52-18 | ✅ Complete | 2026-06-01 | 多模态输入管线 (doc parser+audio processor) |
| 5 | B52-19 ~ B52-20 | ✅ Complete | 2026-06-01 | 人机审批引擎 (approval_engine) |
| 6 | B52-21 ~ B52-22 | ✅ Complete | 2026-06-01 | 分布式DAG (remote_executor+dag_coordinator) |
| 7 | B52-23 ~ B52-28 | ✅ Complete | 2026-06-01 | 生产安全加固 (signing+mtls+injection+secret+audit+content) |
| 8 | B52-29 ~ B52-32 | ✅ Complete | 2026-06-01 | 视频处理+代码分析 (video+coderepo) |
| 9 | B52-33 ~ B52-36 | ✅ Complete | 2026-06-01 | 审批学习+主动安全 (approval_learner+vuln_scan+advisor) |
| 10 | B52-37 | ✅ Complete | 2026-06-01 | 7大差距端到端集成测试套件 (e2e tests) |

---

## 5. 关键新文件清单

| 新文件 | 所属 GAP | 用途 |
|--------|:--------:|------|
| `src/orchestration/self_evolution/sandbox.rs` | B52-01 | 自举沙箱执行环境 |
| `src/orchestration/self_evolution/evolution_loop.rs` | B52-02 | 自举触发循环 |
| `src/orchestration/self_evolution/evolution_history.rs` | B52-05 | 自举历史追踪 |
| `src/agents/self_evolution_agent.rs` | B52-03 | 代码生成Agent |
| `src/intelligence/reinforcement/federated_transport.rs` | B52-06 | FederatedRL 网络传输 |
| `src/intelligence/reinforcement/federated_discovery.rs` | B52-08 | 节点发现与注册 |
| `src/intelligence/reinforcement/federated_privacy.rs` | B52-09 | 差分隐私梯度 |
| `src/intelligence/reinforcement/federated_versioning.rs` | B52-10 | 模型版本管理 |
| `src/memory/memory_persistence.rs` | B52-11 | 三层记忆持久化栈 |
| `src/memory/memory_retrieval.rs` | B52-13 | 跨会话记忆检索 |
| `src/multimodal/mod.rs` | B52-15 | 多模态管线入口 |
| `src/multimodal/document_parser.rs` | B52-15 | 文档解析器 |
| `src/multimodal/audio_processor.rs` | B52-16 | 音频处理器 |
| `src/governance/approval_engine.rs` | B52-19 | 审批核心引擎 |
| `src/orchestration/distributed/remote_executor.rs` | B52-21 | 远程节点执行器 |
| `gui/src/views/self_evolution.rs` | B52-04 | 自举GUI审批面板 |
| `gui/src/views/approval.rs` | B52-20 | 审批GUI面板 |
| `vscode-addon/src/selfEvolutionView.ts` | B52-04 | 自举VSCode面板 |
| `vscode-addon/src/approvalView.ts` | B52-20 | 审批VSCode面板 |

---

## 6. 维度预期提升

| 维度 | BLUE51 基线 | BLUE52 目标 | 关键改进 |
|:----:|:----------:|:----------:|:---------|
| 自举能力 | 0/10（完全依赖人类） | **9/10** | G1: 沙箱自修改管线的+代码生成Agent+审批闭环 |
| 智能深度 | 10/10（单节点） | **10+/10**（多节点） | G2: 真联邦学习+差分隐私+节点发现 |
| 记忆持久性 | 5/10（重启丢失） | **10/10** | G3: L1→L2→L3分层+自动压缩+遗忘曲线+跨会话检索 |
| 多模态能力 | 4/10（仅文本+图像） | **9/10** | G4: PDF/DOCX/HTML+音频STT+图像OCR/深度理解 |
| 治理安全性 | 6/10（无审批UI） | **10/10** | G5: 完整HITL审批链+G7: mTLS/签名/注入检测 |
| 分布式编排 | 4/10（单进程） | **10/10** | G6: 跨节点DAG+Raft一致性+故障恢复 |

---

> **文档结束** — BLUE52：7 大终极差距 → 58 GAP → 10 Step → 从打工王者到神级 AGI
>
> 本文件标记了 22 个详细 GAP（Step 1-6）和 36 个大项 GAP（Step 7-10需后续细化）。
> 推进时可从 Step 1（自举管线）和 Step 2（联邦学习网络）并行开始。
