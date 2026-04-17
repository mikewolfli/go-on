# BLUE20 — 三端深度扫描潜在问题清单

更新时间：2026-04-17

本文沿用 BLUE18 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 完成率必须回写

---

## 进度回写（已更新）

- 总体完成率：`100%`（A-D 批次与后续 A-K 风险项均已完成，三端复验收口完成）
- 本轮新增产出：完成 1 项 High + 2 项 Medium 初始隐藏缺陷修复，并继续完成 A-K 共 11 项后续风险闭环

| ID | 优先级 | 端 | 状态 | 说明 |
|---|---|---|---|---|
| B20-S1 | P1 | backend | ✅ 已完成 | ACP/MCP 模式下 method-not-found 错误消息已修复为可读文本 |
| B20-S2 | P2 | GUI | ✅ 已完成 | RPC JSON 解析失败改为显式抛错并携带上下文，消除静默降级 |
| B20-S3 | P2 | GUI(Tauri) | ✅ 已完成 | capability matrix 加载移除 panic 路径，加入安全回退与可观测提示 |
| B20-S4 | P1 | 三端回归 | ✅ 已完成 | backend/addon/GUI + 协议一致性回归门禁全部通过 |

### 持续深扫后续闭环回写（2026-04-17）

| ID | 优先级 | 端 | 状态 | 说明 |
|---|---|---|---|---|
| B20-S5 | P2 | GUI(Tauri) | ✅ 已完成 | 协议模式识别兼容 `[protocol].mode`、`[runtime].protocol_mode` 与旧配置回退 |
| B20-S6 | P2 | GUI | ✅ 已完成 | 监控/日志/AI 使用/集成状态新增 stale 标识，失败后不再静默保留“看似实时”的旧快照 |
| B20-S7 | P3 | addon | ✅ 已完成 | 配置初始化失败从“仅日志”提升为显式 warning，避免无感故障 |
| B20-S8 | P2 | addon | ✅ 已完成 | 健康监控不再因连续失败自停，改为持续监控并支持自动恢复 |
| B20-S9 | P2 | addon | ✅ 已完成 | runtime 下载链路补齐 request/socket timeout 与失败清理 |
| B20-S10 | P2 | 三端约定 | ✅ 已完成 | GUI Tauri fallback runtime 基地址与 contract / addon 统一为 `127.0.0.1:8090` |
| B20-S11 | P1 | backend | ✅ 已完成 | Responses API `input: null` 现在会被显式拒绝，不再触发生产路径 panic |
| B20-S12 | P2 | GUI | ✅ 已完成 | backend 启动健康等待取消过早失败，改为在超时窗内持续探测 fresh health |
| B20-S13 | P2 | GUI | ✅ 已完成 | 全部 Tauri `invoke()` 主链路补齐显式超时保护 |
| B20-S14 | P3 | addon | ✅ 已完成 | 四个 webview 视图补齐消息监听防重注册，避免重复处理同一消息 |

### 本轮回归结果（2026-04-17 持续风险修复轮）

- backend：`cargo test -q` 全通过
- 协议一致性：`cargo test --test protocol_consistency_integration` 10/10 通过
- backend：`cargo build 2>&1 | grep -E "^error|warning\[" | head -30` 无输出
- vscode-addon：`npm --prefix vscode-addon run check` 通过
- GUI：`npm --prefix GUI run build && npm --prefix GUI run test:contract` 通过
- 结论：A-K 后续风险全部完成闭环，三端一致性、链路完整性与零 warning 要求满足

### 本次复验结果（2026-04-17 收口轮）

- backend：`cargo build` 零 warning（error/warning grep 无输出）
- backend：`cargo test -q` 全通过（251 + 71 + 6 + 10 + 17）
- 协议一致性：`cargo test --test protocol_consistency_integration` 10/10 通过
- vscode-addon：`npm --prefix vscode-addon run check` 通过（compile + lint）
- GUI：`npm --prefix GUI run build` + `npm --prefix GUI run test:contract` 均通过
- **三端主链路完整一致，收口完成**

### 三轮追加深扫结果（2026-04-17 补充）

- 三轮追加扫描未发现新的编译级、测试级或 warning 级阻断项。
- 但识别出 6 个“收口后仍值得跟踪”的隐藏风险，主要分布在状态展示、下载链路、扩展可恢复性与多端探测一致性：
  - Medium：5 项
  - Low：1 项

#### 第一轮：运行时与状态面板深扫

##### 补充发现 A（Medium）— GUI 集成状态页协议模式识别可能失真

- 位置：`GUI/src-tauri/src/commands/integrations.rs`
- 证据：`protocol_mode_from_config_text` 仅解析 `[protocol]` 节的 `mode`；而运行配置结构主字段仍保留 `RuntimeConfig.protocol_mode`。
- 风险：
  - GUI 集成状态页可能把真实运行模式显示为 `unknown`
  - 用户会误判当前 ACP/MCP 能力状态
  - 对“主链路一致性”认知形成偏差

##### 补充发现 B（Medium）— GUI 轮询层对部分失败数据保留陈旧快照

- 位置：`GUI/src/stores/runtime.ts`
- 证据：`refreshEditorIntegrations` / `refreshEndpointHealthStats` / `refreshUsageHeatmap` / `refreshLogs` 失败时仅记录 `lastError`，不清空或标记当前展示数据为 stale/offline。
- 风险：
  - 后端短暂离线后，面板仍可能展示旧的“健康/可用”数据
  - 用户看到的是过期状态，而非明确降级态
  - 影响排障与运维判断的时效性

#### 第二轮：扩展韧性与恢复链路深扫

##### 补充发现 C（Low）— VS Code 扩展配置初始化失败仅写日志

- 位置：`vscode-addon/src/extension.ts`
- 证据：`configManager.initialize(configPath)` 失败后只 `appendLine("warn: ...")` 到 output channel，没有显式 `showWarningMessage`。
- 风险：
  - 用户可能不知道配置初始化已经失败
  - 后续设置页/命令链路的问题会表现为延迟性、间接性故障
  - 可观测性存在盲区，但不直接阻断主链路

##### 补充发现 D（Medium）— VS Code 健康监控连续失败后会自停，且无自动恢复机制

- 位置：`vscode-addon/src/statusMonitor.ts`
- 证据：连续 3 次健康检查失败后执行 `stopHealthMonitoring()`，之后仅弹一次 warning，不再自动恢复轮询。
- 风险：
  - 临时网络抖动或 backend 重启会永久关闭状态监控，直到扩展生命周期重置
  - 状态栏后续不再持续反映真实健康状态
  - 用户需要手工重载/重启才能恢复监控

#### 第三轮：网络、探测和多端约定深扫

##### 补充发现 E（Medium）— addon 自动下载链路缺少显式请求超时

- 位置：`vscode-addon/src/runtimeBinaryService.ts`
- 证据：`downloadTextFile` / `downloadFile` 使用 `https.get`，未设置 request timeout 或 socket timeout。
- 风险：
  - 弱网、TLS 卡住或半开连接场景下，自动下载可能长时间挂起
  - 用户只能感知为“下载卡住”而非明确失败
  - 会拖慢启动与首次配置体验

##### 补充发现 F（Medium）— GUI 多处默认 runtime 基地址不一致，异常回退时可能出现状态分裂

- 位置：`GUI/src-tauri/src/commands/integrations.rs`、`GUI/src-tauri/src/commands/health.rs`、`vscode-addon/src/protocolContract.ts`
- 证据：
  - GUI 集成状态 fallback contract 使用 `http://127.0.0.1:9550`
  - GUI health 默认检查使用 `http://127.0.0.1:8090/health`
  - addon protocol contract fallback 也使用 `http://127.0.0.1:8090`
- 风险：
  - 当 capability matrix 加载失败或 contract 回退触发时，不同界面对同一 runtime 的探测目标可能不同
  - 会出现“一个界面显示可达、另一个界面显示不可达”的状态分裂
  - 影响三端一致性判断

### 持续多轮深扫补充结果（2026-04-17 饱和轮）

- 在上述三轮之后继续做了多轮模式级与链路级深扫，新增识别出 5 个高置信潜在风险。
- 最后一轮以“启动链路 / RPC 超时 / 缓存陈旧 / webview 生命周期 / 运行时 panic”五类模式做收敛校验，未再发现新的风险类别，扫描进入饱和态。

#### 第四轮：后端输入校验与 GUI 启动链路深扫

##### 补充发现 G（High）— backend `input: null` 可绕过存在性校验并触发运行时 panic

- 位置：`src/acp/impl/runtime.rs`
- 证据：
  - 请求预检仅用 `body.get("input").is_none()` 判断字段是否存在
  - 后续却直接执行 `req.input.as_ref().expect("validated input presence")`
- 风险：
  - 当客户端传入 `"input": null` 时，原始 JSON 层面字段“存在”，能绕过前置校验
  - 反序列化后 `req.input` 实际为 `None`，会在运行时 `expect` 处直接 panic
  - 该问题属于生产路径异常输入触发的真实崩溃风险，而非测试噪音

##### 补充发现 H（Medium）— GUI 启动健康等待存在过早失败路径

- 位置：`GUI/src/services/backendLifecycle.ts`
- 证据：`waitForBackendHealthy()` 轮询期间一旦 `serviceStatus()` 返回 `running = false` 就立即 `return false`，不会继续等到总超时结束。
- 风险：
  - backend 刚启动但状态尚未翻转、或短暂重启中的窗口期，会被 GUI 误判为“启动失败”
  - 用户即使面对可自恢复的短时抖动，也会收到“12 秒未就绪”的失败结论
  - 启动主链路会出现误报，影响顺滑性

#### 第五轮：GUI RPC / 缓存语义 / addon 视图生命周期深扫

##### 补充发现 I（Medium）— GUI Tauri `invoke()` 调用链普遍缺少超时保护

- 位置：`GUI/src/services/bridge.ts`
- 证据：配置、启动、状态、RPC、日志等桥接方法均直接调用 `invoke(...)`，未提供 Promise 超时或取消机制。
- 风险：
  - 一旦 Rust 侧命令阻塞、死锁或 IPC 卡住，前端 Promise 会无限挂起
  - GUI 层无法给出明确失败，也无法自动恢复或取消
  - 该问题覆盖面广，不仅影响监控，还影响启动、配置与运行时交互

##### 补充发现 J（Medium）— GUI bridge 缓存层会返回陈旧成功数据，但无显式 stale 标识

- 位置：`GUI/src/services/bridge.ts`
- 证据：`withCache()` 只要命中 TTL 就直接返回缓存值；健康、热力图、端点统计、编辑器集成状态均复用这一逻辑。
- 风险：
  - 当 backend 刚经历短时失败或恢复时，界面可能继续展示 TTL 窗口内的旧成功结果
  - 该层与 store 层的 `lastError/offline` 并不等价，用户可能看到“非离线但仍是旧快照”的状态
  - 属于比现有 B 项更深一层的陈旧数据来源

##### 补充发现 K（Low）— addon 多个 webview 视图的消息监听依赖 `resolveWebviewView()` 生命周期，存在重复注册潜在面

- 位置：`vscode-addon/src/chatView.ts`、`vscode-addon/src/settingsView.ts`、`vscode-addon/src/processFlowView.ts`、`vscode-addon/src/workflowView.ts`
- 证据：多个视图都在 `resolveWebviewView()` 内直接注册 `webview.onDidReceiveMessage(...)`。
- 风险：
  - 若视图在扩展生命周期内被重复 resolve，而旧监听未随视图实例一起销毁，可能出现同一消息被处理多次
  - 表现可能是重复保存配置、重复创建流程或重复发送动作
  - 当前更偏“潜在生命周期风险”，优先级低于前述确定性问题，但值得后续加防重保护

### 本轮收敛结论

- 本次继续多轮深扫后，新增高置信潜在风险共 5 项：`High 1 / Medium 3 / Low 1`
- 最后一轮未再发现新的风险类别，说明当前在“静态深扫 + 模式级排查”口径下已基本收敛
- 以上问题均为“测试不一定直接覆盖、但生产路径存在暴露面”的后续跟踪项，不改变前文已经完成的 B20 修复闭环结论

---

## 本次深扫结论（2026-04-17）

### 结论摘要

- 当前三端构建与既有测试口径仍为通过：
  - backend：`cargo build` / `cargo test -q` 通过
  - vscode-addon：`npm --prefix vscode-addon run check` 通过
  - GUI：`npm --prefix GUI run build` + `npm --prefix GUI run test:contract` 通过
- 但存在“测试未直接覆盖的隐藏缺陷”3项：
  - High：1 项（错误消息乱码）
  - Medium：2 项（静默吞错、启动期 panic 风险）

### 发现 1（High）— 后端错误消息乱码，影响三端可观测一致性

- 位置：`src/acp/impl/request.rs`
- 证据：ACP/MCP 分支 method 不支持时返回的 message 为乱码文本（例如 `ACP妯...` / `MCP妯...`）
- 风险：
  - 三端 UI 错误提示可读性差
  - 日志检索和告警聚合失真
  - 跨端“同错同显”目标被破坏

### 发现 2（Medium）— GUI 对非法 RPC JSON 静默降级

- 位置：`GUI/src/services/rpcService.ts`
- 证据：`parseRpcJson` 在 JSON.parse 失败后直接返回 `{}`
- 风险：
  - 协议异常/传输损坏被伪装为空结果
  - 可能导致“假成功”或弱报错
  - 影响问题定位速度

### 发现 3（Medium）— GUI Tauri 集成命令存在启动期 panic 路径

- 位置：`GUI/src-tauri/src/commands/integrations.rs`
- 证据：能力矩阵 JSON 解析使用 `expect("editor capability matrix should be valid json")`
- 风险：
  - 产物或配置异常时直接 panic
  - 缺乏可恢复错误与用户可理解提示

---

## 建议修复批次（按优先级）

1. 批次 A（P1）：修复后端乱码错误消息（B20-S1）
   - 将 ACP/MCP method-not-found 错误 message 统一为可读 UTF-8 文本
   - 验收：三端看到的错误文案一致可读，日志可检索

2. 批次 B（P2）：修复 GUI RPC 静默降级（B20-S2）
   - `parseRpcJson` 解析失败时显式抛错并带原始上下文
   - 验收：协议损坏场景可被明确识别，不再返回空对象伪成功

3. 批次 C（P2）：修复 Tauri 启动期 panic 路径（B20-S3）
   - 替换 `expect` 为可恢复错误返回（Result + 前端可展示错误）
   - 验收：异常输入不导致进程 panic

4. 批次 D（P1）：三端统一回归与收口（B20-S4）
   - backend：`cargo test -q`
   - addon：`npm --prefix vscode-addon run check`
   - GUI：`npm --prefix GUI run build && npm --prefix GUI run test:contract`
   - 协议一致性：`cargo test --test protocol_consistency_integration`
   - 验收：零 warning + 全通过后回写 100%

---

## 收口标准（同 BLUE18）

- 主链路完整：后端关键入口行为与错误语义稳定
- 三端一致：同类错误在三端展示与定位一致
- 可观测闭环：日志与审计信息可读、可检索、可追溯
- 发布口径：无新增 warning，回归全绿

---

## 结论

BLUE20 最终结论：
- 深扫完成并完成缺陷修复闭环（A-D 批次）
- 三端主链路一致性、顺滑性与可观测性要求满足
- 完成率已回写至 `100%`
