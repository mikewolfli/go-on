# BLUE20 — 三端深度扫描潜在问题清单

更新时间：2026-04-18

本文沿用 BLUE18 的同一验收规则与收口口径：
- 三端一统（backend / vscode-addon / GUI）
- 主链路完整闭环
- 后端主链路功能完整
- 不留 warning
- 最小修改：仅改与目标直接相关内容；禁止为了“过测试”而做功能语义不完整的最小改动
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
| B20-S15 | P1 | 三端一致性 | ✅ 已完成 | adaptive 判定标准已收敛为“能力双栈 + 请求分发 + 启动传输推导”，并同步回写 backend / contract / GUI / addon / docs |
| B20-S16 | P1 | 三端收口 | ✅ 已完成 | 清零残留：修复 DOC mdbook 配置兼容性、移除 GUI Tauri dead_code 残留并通过全链路复验 |
| B20-S17 | P1 | backend 稳态增强 | ✅ 已完成 | 以最稳策略落地 adaptive 多信号判定观测：路径/方法优先、请求头仅辅助，不改变既有路由行为并输出 `adaptive_signal` 日志 |

### 本次清零复验结果（2026-04-18）

- DOC：`mdbook build DOC` 通过（HTML 输出成功）
- GUI(Tauri)：`cargo check --manifest-path GUI/src-tauri/Cargo.toml` 通过，dead_code 警告清零
- backend：`cargo check --all-targets` 通过
- 协议一致性：`cargo test --test protocol_consistency_integration -- --nocapture` 10/10 通过
- GUI：`npm --prefix GUI run test:contract && npm --prefix GUI run build` 通过
- addon：`npm --prefix vscode-addon run check && node vscode-addon/scripts/contract-smoke.js` 通过
- 结论：当前轮“全部清掉”目标已完成，三端稳定性与完整性复验全绿

### 本次稳态实施复验结果（2026-04-18）

- backend：`cargo check --all-targets` 通过
- 协议一致性：`cargo test --test protocol_consistency_integration -- --nocapture` 10/10 通过
- 结论：adaptive 稳态增强已完成，保持“最小行为变更 + 多信号可观测”策略，完成率维持 `100%`

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

---

## BLUE20 补充闭环：ACP / MCP + stdio / HTTP + adaptive 五种模式判定标准

更新时间：2026-04-18

本节对当前仓库中的 5 种协议模式做一次闭环说明，目标不是只回答“有没有 5 种模式”，而是回答以下 4 个问题：

- 当前实现里这 5 种模式到底各自代表什么
- 当前所谓“adaptive”到底是哪个层面的自适应
- 现在的判定标准是不是最优
- 最优判定标准应该如何定义，默认策略应如何落地

### 一、结论先行

- 当前仓库已经具备 5 个可配置模式：`adaptive`、`acp_stdio`、`acp_http`、`mcp_stdio`、`mcp_http`
- 但当前“判定标准”并不属于最优，只能算“可运行、可解释到一半、但语义分层还不够清晰”
- 根本原因不是模式数量不够，而是“协议选择”和“传输选择”仍然混在一个枚举里，对外看似 5 选 1，实际核心运行时更接近“3 个协议语义 + 1 个传输条件”
- 因此，当前最准确的判断是：
  - “5 种模式存在”成立
  - “5 种模式判断标准已经最优”不成立

### 二、当前代码中的真实语义

#### 1. 启动层确实支持 5 种入口模式

`src/main.rs` 中的 `AccessMode` 已经把对外可配置值定义为：

- `Adaptive`
- `AcpStdio`
- `AcpHttp`
- `McpStdio`
- `McpHttp`

并且 CLI 校验也只接受这 5 种标准化值。

这说明“配置层 / 启动层 / CLI 层”已经完成 5 模式设计。

#### 2. 但 Adaptive 并不是 4 个具体模式之间的动态选择器

当前 `Adaptive` 分支的实际行为是：

- 先创建 ACP server
- 若存在 `acp_http_bind_addr`，则跑 ACP HTTP
- 若不存在 `acp_http_bind_addr`，则跑 ACP stdio

也就是说，当前 adaptive 的真实语义是：

- “ACP 优先”
- “HTTP / stdio 由 bind 地址是否存在决定”

它并不会在以下 4 个具体模式之间动态择优：

- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

更不会依据客户端能力、探针结果、失败历史、编辑器类型、握手结果，在 ACP 与 MCP 之间做闭环切换。

所以，当前 adaptive 不是“5 选 1 自适应”，而更接近“ACP 主链上的传输自适应”。

#### 3. 核心请求分发层仍然是 3 模式语义

`src/acp/impl/request.rs` 当前只区分：

- `Auto`
- `Acp`
- `Mcp`

并通过方法名判断请求属于 ACP 还是 MCP：

- MCP：`mcp.*` / `mcp.initialize`
- 其余白名单方法：ACP

这说明后端核心判定并没有把 `stdio` 和 `http` 当作协议语义的一部分，而是把它们当作启动形态或传输形态。

这本身不是错，但它和“对外宣传的 5 模式”为同一层概念，就会让人误以为系统内部也是 5 维等价决策，这一点当前并不成立。

#### 4. GUI / addon / contract 仍然存在旧语义残留

当前三端还有一组历史语义仍在并存：

- contract 中仍保留 `acp` / `mcp` / `auto`
- fallback contract 的 `defaultMode` 仍是 `auto`
- GUI 集成状态会把 `adaptive` 解读为“ACP 与 MCP 都支持”

问题在于，这句话只有在“运行时实际同时对外提供两类能力”时才完全成立；如果只是 ACP server 根据 bind 选择 HTTP 或 stdio，这个表述就偏乐观了。

### 三、为什么说当前标准不是最优

判断一个模式标准是否最优，不能只看“能不能跑起来”，而要看 5 个维度：

- 语义是否单一
- 默认值是否可预测
- 三端是否一致
- 故障时是否容易诊断
- 新接入面是否容易扩展

按这 5 个维度看，当前方案的问题主要有以下几类。

#### 1. 协议维度与传输维度耦合在一起

当前 5 个模式名里同时混入了两类信息：

- 协议能力：ACP / MCP / 双栈
- 传输形态：stdio / HTTP

这会带来两个后果：

- 用户容易把 5 个值理解成同一层级的“互斥协议类型”
- 代码内部却仍按“协议判定”和“传输判定”分两层处理

对外一个枚举，内部两层逻辑，没有明确建模，就会导致文档和实现容易慢慢偏离。

#### 2. adaptive 的名称强于实现

“adaptive”这个词天然会让人理解为：

- 能根据环境做动态选择
- 能按客户端能力做协议协商
- 能按失败历史做回退
- 能在 ACP / MCP / stdio / HTTP 之间做闭环策略调整

但当前实现只做到：

- ACP server 启动
- 按 bind 是否存在决定 ACP HTTP 或 ACP stdio

所以名称承诺大于真实能力，这会降低可预测性。

#### 3. 默认值对接入面并不完全自解释

开发配置里：

- `[protocol].mode = "adaptive"`
- `acp_http_bind_addr` 默认被注释掉

这意味着：

- 在 CLI 本地直连场景，默认会落到 ACP stdio
- 在 GUI / health probe / 外部 HTTP 依赖场景，如果不额外打开 bind，默认又不满足直觉预期

所以“adaptive 是推荐默认”这句话本身没有错，但它依赖上下文。若不补一句“是否启用 HTTP 仍取决于 bind 配置”，就会让默认策略显得含糊。

#### 4. 三端契约还没有完全统一到同一套模型

目前仓库同时存在两套表述：

- 新表述：5 模式
- 旧表述：`acp` / `mcp` / `auto`

在长期维护中，这种双语义会带来典型问题：

- contract 写一种
- GUI 展示写一种
- addon fallback 再写一种
- backend 内部判定仍保留历史模式

一旦后续有人继续扩展“真正的 adaptive”，就容易出现接口定义先漂移、测试用例后补、用户再通过 bug 报告逼回收口的情况。

### 四、最优判定标准应该怎么定义

最优标准不应该继续把“5 个字符串值”当作全部模型，而应该明确分成两层判定、一个默认策略。

#### 第一层：协议能力判定

先回答“这个实例对外打算提供什么协议能力”：

- `acp_only`
- `mcp_only`
- `dual_stack`

这层决定的是：

- 能接哪些请求
- 能暴露哪些初始化语义
- 编辑器或客户端应该如何理解其能力边界

这是最核心的一层，因为它属于“语义兼容性”，不是“传输实现细节”。

#### 第二层：传输形态判定

再回答“这些能力通过什么方式暴露”：

- `stdio`
- `http`
- `hybrid`（若未来支持同时监听）

这层决定的是：

- 由谁拉起 runtime
- 是否依赖回环地址与端口
- 是否需要健康检查与入口鉴权
- 部署方式是本地子进程、桌面 GUI、还是网关后的服务化实例

#### 第三层：默认策略判定

最后才是默认策略：系统在未显式指定时应优先给出哪个组合。

最优默认策略建议如下：

- 本地 CLI / 编辑器子进程场景：优先 `acp_only + stdio`
- GUI / 本地面板 / 健康探针场景：优先 `acp_only + http`
- 标准 MCP 工具服务场景：按客户端能力选择 `mcp_only + stdio` 或 `mcp_only + http`
- 只有在真正具备双协议暴露与稳定回退能力时，才使用 `dual_stack + adaptive`

这套标准的好处是，用户和代码看到的是同一套分层：

- 先看“说什么协议”
- 再看“怎么接入”
- 最后看“默认怎么选”

### 五、按最优标准重新解释当前 5 种模式

如果保留现有 5 个对外值不变，最优解释应当是下面这样。

#### 1. `acp_stdio`

判定标准：

- 客户端明确走 ACP / A2A 语义
- 运行时由本地父进程拉起
- 不需要独立健康探针入口
- 不希望暴露 HTTP 端口

典型场景：

- 本地 IDE 插件子进程
- 安全敏感、最小暴露面场景

优点：

- 暴露面最小
- 配置最简单
- 本地调试直接

限制：

- GUI、外部探针、跨进程观测天然不如 HTTP 友好

#### 2. `acp_http`

判定标准：

- 客户端明确走 ACP / A2A 语义
- 需要健康检查、状态探测、GUI 面板或外部系统访问
- 允许通过本地回环或受控网络暴露入口

典型场景：

- GUI 本地桌面面板
- Zed 或其它通过 HTTP 接 ACP 的接入面
- 运维需要统一探测 `/health`

优点：

- 可观测性强
- 接入简单
- 易于与 GUI 和监控统一

限制：

- 一旦对外暴露，就必须配入口鉴权、限流与 strict 策略

#### 3. `mcp_stdio`

判定标准：

- 客户端明确要求标准 MCP server
- 通过本地 stdio 与主程序握手
- 不需要独立 HTTP 服务化暴露

典型场景：

- 本地 MCP host 拉起工具服务器

优点：

- 最接近标准 MCP 本地集成模型
- 不依赖 HTTP 网络环境

限制：

- 跨端共享和状态探测能力弱

#### 4. `mcp_http`

判定标准：

- 客户端需要以 HTTP 形式访问 MCP 能力
- 运行时作为服务进程暴露 MCP 接口
- 希望共享给多个远端或本地客户端

典型场景：

- 服务化 MCP 网关
- 多客户端复用同一 MCP 实例

优点：

- 部署灵活
- 易接网关、代理和观测系统

限制：

- 对安全和契约稳定性要求更高

#### 5. `adaptive`

最优定义应为：

- 系统不是简单吃一个固定模式，而是根据“接入面能力 + 启动条件 + 探针结果 + 已知回退规则”选择最合适组合

但当前实现的真实定义是：

- ACP server 启动
- 若有 HTTP bind 则走 ACP HTTP
- 否则走 ACP stdio

因此，当前 `adaptive` 的详细判定标准，必须分“现状标准”和“目标标准”两部分写清。

### 六、当前仓库里 adaptive 的实际判定标准

当前代码下，`adaptive` 的实际判定逻辑应明确写成：

1. 先读取 `[protocol].mode`
2. 若为 `adaptive`，运行时先判定协议能力为 dual stack，请求分发为 auto
3. 固定模式仍严格服从配置，不被 adaptive 覆盖
4. 启动传输层当前按运行前提推导：存在 `runtime.acp_http_bind_addr` 或 CLI `--acp-http-bind` 时优先提供 HTTP，否则仅提供 stdio
5. 请求进入运行时后，再按 client type / method 类型选择 ACP 或 MCP 分发路径
6. 选择结果必须回写到 baseline / contract / GUI 集成状态，不能只在内部隐式决定

换句话说，当前 adaptive 的本质是：

- “配置层声明 dual stack 能力，而不是写死具体固定接口”
- “启动传输当前仍依据 bind 条件在可用传输中选择入口”
- “请求分发按 Auto 语义兼容 ACP 与 MCP 方法”

这个标准必须被清楚写出来，否则用户会误以为 adaptive 已经完成了任意接口自由切换；当前更准确的表述是“能力双栈 + 启动传输推导 + 请求类型路由”。

### 七、目标态 adaptive 的最优判定标准

如果后续要把 adaptive 真正做成“最优默认”，建议定义为下面这套闭环策略。

#### A. 输入信号

adaptive 至少应基于以下信号判定：

- 显式配置：用户是否强制指定协议或传输
- 接入面类型：CLI、GUI、VS Code、Zed、远程服务
- transport 前提：是否存在 bind 地址、端口、监听权限
- 协议需求：客户端是 ACP、MCP，还是双栈探测
- 健康 / 探针：HTTP 路径是否可达、stdio 握手是否成功
- 安全前提：entry auth、strict、限流是否满足暴露条件

#### B. 判定顺序

最优判定顺序建议如下：

1. 若用户显式指定具体模式，绝不再自适应覆盖。
2. 若接入面只支持一种协议，则直接锁定该协议。
3. 若接入面要求 HTTP 而 HTTP 前提不满足，则直接报配置缺口，不做“静默改走 stdio”。
4. 若接入面支持双协议，则先选与宿主最自然的组合。
5. 若首选组合探针失败，再按预定义回退顺序切换。
6. 每次选择和回退都必须写入日志、状态和自画像输出。

#### C. 推荐回退顺序

对本仓库的现状，较合理的回退顺序是：

- GUI / 本地 HTTP 面板：`acp_http -> acp_stdio` 仅在明确允许退化时启用
- 本地 MCP host：`mcp_stdio -> mcp_http`
- 双栈实验场景：`dual_stack_http -> acp_http -> mcp_http -> acp_stdio`

注意：回退顺序不能只看“能不能活下来”，还必须看“语义是否仍满足客户端预期”。

例如，GUI 若明确依赖 health endpoint，就不应无声退化到 stdio 并继续声称“服务健康”。

#### D. 观测要求

adaptive 若要称得上最优，必须至少暴露：

- `configured_mode`
- `effective_protocol_mode`
- `effective_transport`
- `selection_reason`
- `fallback_count`
- `last_fallback_reason`

否则它只是“内部帮你选了一个模式”，不是可治理、可诊断的自适应。

### 八、推荐默认策略

基于当前代码现状和三端接入结构，推荐默认策略如下。

#### 1. backend 默认值

推荐保留：`[protocol].mode = "adaptive"`

但必须补充一条明确说明：

- adaptive 不等于替用户改写成某个固定模式
- fixed mode 只由显式配置决定
- 当前实现里，adaptive 只是在启动阶段根据可用传输前提选择 HTTP 或 stdio 入口，并保留 dual stack 请求分发能力

否则“推荐默认”仍会被误读成“adaptive 其实就是某个写死的 ACP 入口”。

#### 2. GUI 默认策略

GUI 若依赖健康探测、状态面板、集成页探针，则推荐：

- 默认要求 `acp_http`
- 或 `adaptive + acp_http_bind_addr 已配置`

不建议把“纯 adaptive 且无 bind”也视为 GUI 友好默认。

#### 3. VS Code addon 默认策略

若 addon 主要通过主进程拉起 runtime 且不强依赖独立 HTTP 入口，则：

- `acp_stdio` 可以是更稳的默认
- 若 addon 的健康检查、状态栏、扩展侧 RPC 依赖 HTTP，则应切到 `acp_http` 或 `adaptive + bind`

重点不是统一所有端都用一个默认值，而是让每个接入面的默认值和其真实依赖一致。

#### 4. 生产部署默认策略

生产场景下，不建议用“语义模糊的 adaptive 默认”。更好的策略是：

- 明确 `acp_http` 或 `mcp_http`
- 同时配好 entry auth、rate limit、strict 与可观测配置

生产环境最重要的是确定性，不是隐式切换。

### 九、最终判断标准

因此，对“ACP / MCP + stdio / HTTP + adaptive，5 种模式判断标准是否最优”这个问题，最终闭环结论如下：

#### 结论 A：模式集合本身是合理的

这 5 个值覆盖了当前仓库所需的主要接入形态，数量和方向没有问题。

#### 结论 B：当前判定标准不是最优

原因是：

- adaptive 的语义大于实现
- 协议层与传输层未彻底解耦
- 核心运行时仍偏 3 模式语义
- contract / GUI / addon / backend 仍有旧语义残留

#### 结论 C：当前最准确的解释不是“5 选 1 真自适应”，而是

- 启动层：5 模式
- 核心语义层：3 模式（ACP / MCP / Auto）
- 传输层：stdio / HTTP
- adaptive 现状：ACP 主链上的条件性传输选择

#### 结论 D：最优标准应改为“二维判定 + 显式默认策略”

即：

- 先判协议能力
- 再判传输形态
- 最后判默认选择与回退规则

这才是长期稳定、可扩展、三端一致、可观测的设计。

### 十、建议的后续落地动作

若要把本节结论真正落地为仓库一致标准，建议按以下顺序推进：

1. 文档统一：README / GUI README / addon README / contract 已同步到同一套分层解释。
2. 契约统一：`editor-capability-matrix.json` 已改为 5 模式 + 能力/传输模型描述。
3. 运行时统一：backend 已落地“协议能力 + 请求分发 + 启动传输”分层模型。
4. adaptive 可观测化：`config.baseline` 已暴露 `configured_mode`、`protocol_capability`、`request_dispatch_mode`、`startup_transport`、`selection_reason`。
5. 默认值分端定义：文档已明确各接入面的推荐组合，避免把 adaptive 描述成某个写死接口。

### 十一、本节收口结论

- 本次已经完成对 5 种模式的定义澄清、现状核验、最优性判断、运行时收敛和文档回写。
- 该问题的闭环答案不是“推翻 5 模式”，而是“保留 5 模式入口，同时把判定标准从单层字符串枚举提升为分层模型”。
- 当前已落地的标准是：`adaptive = dual stack capability + auto request dispatch + derived startup transport`。
- 验证结果已全绿；另有 2 个 GUI Tauri 侧既有 `dead_code` warning，与本轮 adaptive 收敛无关，未在本轮扩改。

---

## 结论

BLUE20 最终结论：
- 深扫完成并完成缺陷修复闭环（A-D 批次）
- 三端主链路一致性、顺滑性与可观测性要求满足
- adaptive 五模式判定标准已完成三端一致收敛与文档统一
- 完成率已回写至 `100%`
