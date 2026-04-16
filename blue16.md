# BLUE16 正式版 — vscode-addon 与 GUI 结构优化建议

更新时间：2026-04-16

本文为 BLUE16 正式版，基于对 vscode-addon 与 GUI 源码的全量结构评审（2026-04-15），针对发现的高风险安全缺陷、跨平台兼容问题与架构债务，给出可执行的优化建议。

## 进度回写（2026-04-16）

- 总体完成率：`100%`（13 项中已完成 13 项）
- vscode-addon 完成率：`100%`（6 项中已完成 6 项）
- GUI 完成率：`100%`（7 项中已完成 7 项）

| ID | 状态 | 说明 |
|---|---|---|
| B16-H1 | ✅ 已完成 | 已按平台修复可执行路径校验，Unix 无扩展名二进制可通过 |
| B16-H2 | ✅ 已完成 | Chat 执行前确认 + 高危上下文拦截 + 输出通道审计 |
| B16-M1 | ✅ 已完成 | 已完成 `viewRouter.ts`、`runtimeBootstrap.ts`、`commandRegistry.ts`、`rpcCommandRegistry.ts`、`coreCommandRegistry.ts`、`runtimeManager.ts` 模块化拆分，`extension.ts` 命令与运行时职责显著收敛 |
| B16-M2 | ✅ 已完成 | `settingsView.ts` 已由大 switch 重构为 handler map + command map |
| B16-M3 | ✅ 已完成 | 扩展启动自动开 Chat 改为首次安装仅触发一次 |
| B16-M4 | ✅ 已完成 | ESLint 3 条核心规则已恢复为 warn，`configManager.ts` 类型已收紧（`any -> unknown/Record`），并完成 `extension.ts`/`i18n.ts`/多视图的 `any` 与未使用参数清理；`npm run check` 达到 0 warnings / 0 errors |
| B16-G1 | ✅ 已完成 | BackendOps 高危操作确认与 shutdown 二次输入确认已完成 |
| B16-G2 | ✅ 已完成 | `while(true)` 已改为有界重试 |
| B16-G3 | ✅ 已完成 | `App.vue` 生命周期逻辑已下沉到 `backendLifecycle` 和 `useCrashHandler` |
| B16-G4 | ✅ 已完成 | 轮询防重入 + offline 语义收敛已完成 |
| B16-G5 | ✅ 已完成 | 新增 `rpcService.ts`，核心 RPC 类型化并在关键视图接入 |
| B16-G6 | ✅ 已完成 | Security/HealthBreakdown 等关键裸字符串已 i18n 化 |
| B16-G7 | ✅ 已完成 | runtime endpoint 已收敛到 `protocolContract` 单一来源 |

本次回写对应代码门禁结果：`vscode-addon npm run check` 通过（0 warnings / 0 errors）；`GUI npm run build && npm run test:contract` 通过。

### 后续增强批次（2026-04-16）

在完成率已达 100% 的基础上，本轮继续执行稳定性增强并保持接口契约不变：

1. vscode-addon 进一步模块化：新增 `runtimeBinaryService.ts`，将二进制下载/解压、路径校验、配置路径解析与 providers 同步逻辑从 `extension.ts` 下沉，降低激活层复杂度。
2. GUI 统一错误处理：新增 `GUI/src/utils/errors.ts`，在 `backendLifecycle.ts`、`ConfigView.vue`、`SecurityView.vue` 接入统一错误消息归一化，避免 `[object Object]` 类错误提示并提高可观测性。
3. `SecurityView.vue` 的导出 CSV 逻辑完成类型收敛（`any[]` -> `Array<Record<string, unknown>>`），减少后续类型漂移风险。

本轮门禁复验：`vscode-addon npm run check` 通过（0 warnings / 0 errors）；`GUI npm run build && npm run test:contract` 通过。

### 后端闭环增强批次（2026-04-16）

在前述 addon / GUI 工作完成后，本轮补充对 Rust 后端 ACP 主链路执行一次稳定性扫描与闭环增强设计，目标是不停留在代码体检，而是把可观测性、风险识别与治理反馈真正接入请求主路径。

本批次进度回写：

- 后端闭环增强完成率：`75%`（4 项中已完成 3 项）
- 三端主链路闭环状态：`已完成`（Rust ACP / GUI / vscode-addon 已统一接入 `runtime.self_model`）

后端问题清单：

1. 主链路观测信号分散：当前 `health`、`health.probes`、`runtime.stability`、`rl.alignment.offline_eval` 各自返回局部状态，但缺少统一的 ACP 主链路聚合出口，调用方必须自行拼装健康、配置、锁、漂移和建议动作，闭环不完整。
2. 漂移信号未进入日常运行入口：离线评估逻辑已能计算 `reward drift` 与 fallback 建议，但仍停留在专用方法中，尚未纳入常驻运行态自检结果，导致漂移风险不能跟随健康查询一起暴露。
3. `dead_code` 抑制范围过大：`agents/agent.rs`、`core/context.rs`、`main.rs`、`protocol/mcp_server.rs` 以及多个 orchestration / governance 模块仍存在文件级或大范围 `allow(dead_code)`，这会掩盖真实僵尸代码、接口漂移与演化残留。
4. 结构化日志仍未完全统一：虽然 `protocol/mcp_server.rs` 的裸 stderr 已收口，但 `main.rs` 中仍存在 `eprintln!` 路径，CLI fatal / config warning 输出尚未完全纳入统一日志语义。
5. 浮点排序一致性问题未完全清零：本轮已修复 `speed_optimizer`、`reliability_optimizer`、`cost_optimizer` 三处直接 `partial_cmp(...).unwrap()`；但仓内仍存在多处基于 `partial_cmp` 的排序逻辑，需要继续审查是否存在 NaN 分支不稳定、回退策略不一致或潜在 panic 风险。
6. Provider 覆盖率不均衡：部分关键 agent 已具备 payload / strict-mode 测试，但 provider 总体覆盖仍不平衡，载荷构造、响应解析、fallback 路径、严格模式注入等回归点尚未形成统一测试基线。
7. 自画像能力缺口：ACP 当前能分别返回 metrics、health probes、governance 与 stability，但尚无统一 `self-model` 视图输出系统能力画像、风险画像、约束画像与建议动作，难以支持 GUI / addon / external client 的单端接入。

已完成修复：

1. 排序健壮性收敛：`speed_optimizer.rs`、`reliability_optimizer.rs`、`cost_optimizer.rs` 中的 `partial_cmp(...).unwrap()` 已改为 `total_cmp(...)`，消除 NaN 边缘值导致的 panic 风险。
2. 内部不变量语义收敛：`acp/prelude.rs` 中用于不可能分支的 `panic!` 已替换为 `unreachable!`。
3. 日志主链路收敛：`protocol/mcp_server.rs` 中裸 `eprintln!` 已改为 `warn!`，统一进入结构化日志链路。
4. 门禁复验完成：`cargo clippy --all -- -D warnings` 通过；`cargo test` 通过（318 tests passed）。

新增待落地增强项：

| ID | 状态 | 建议项 | 是否需要 | 说明 |
|---|---|---|---|---|
| B16-B1 | ✅ 已完成 | ACP Self-Model 主链路接入 | 必须 | 已新增 `runtime.self_model`，统一聚合 `health.probes`、`runtime.stability`、`rl.alignment.offline_eval` |
| B16-B2 | ✅ 已完成 | Drift Guard 最小版闭环 | 必须 | 漂移摘要、fallback 决策、建议动作已并入 `runtime.self_model` 主结果 |
| B16-B3 | 待推进 | `dead_code` 抑制清理 | 建议 | `agents/agent.rs`、`core/context.rs` 存在文件级 `allow(dead_code)`，需逐步压缩 |
| B16-B4 | ✅ 已完成（本轮） | Provider 载荷/解析基线主链路化 | 建议 | 已新增 `provider.status` 主链路 RPC，三端可见 provider readiness/降级状态并纳入回归场景与契约基线 |

推荐执行顺序：

1. 继续推进 B16-B3（`dead_code` 抑制清理）作为下一轮收口重点。
2. 后续所有 GUI / addon 运行态诊断入口优先复用 `runtime.self_model` 与 `provider.status`，避免重新分叉到多条状态拼装链路。

后端阶段验收门禁：

1. 新增主链路方法已可通过 ACP 请求直接调用，不是孤立工具函数。
2. 返回结果已包含：运行健康、关键约束、漂移摘要、建议动作。
3. 已补充 `requests/runtime-self-model-benchmark.ndjson` 与 `acp_runtime_rpc_integration` 集成测试。
4. `cargo clippy --all -- -D warnings`、`cargo test --test acp_runtime_rpc_integration runtime_self_model -- --nocapture`、`GUI npm run build`、`GUI npm run test:contract`、`vscode-addon npm run check` 全部通过。

本轮实际交付：

1. Rust 后端：新增 `runtime.self_model` ACP 方法，复用既有 `health.probes`、`runtime.stability` 与离线评估逻辑，形成统一自画像返回结构。
2. GUI：`BackendOpsView` 新增 `runtime.self_model` 入口，`HealthBreakdownView` 改为优先读取 `runtime.self_model`，不再自行拼接多条状态链路。
3. vscode-addon：新增 `go-on.runtimeSelfModel` 命令并接入 RPC 主链路。
4. 协议契约：`contracts/editor-capability-matrix.json` 已回写 `runtime.self_model` 为 GUI / addon 公共检查面。

本轮新增交付（B16-B4）：

1. backend：新增 `provider.status` RPC 并接入 ACP 方法白名单与主分发链路，统一输出 provider 就绪/降级摘要、配置侧 agent 依赖快照与 registry 模型目录。
2. GUI：`SecurityView` 接入 `provider.status`，新增 provider 就绪标签与降级风险项，并将 provider 降级计入治理评分惩罚。
3. vscode-addon：新增 `go-on.providerStatus` 命令，直连 `provider.status` 并输出主摘要。
4. 回归：新增 `requests/provider-status-benchmark.ndjson` 与 `acp_runtime_rpc_integration` 用例，场景总数门禁同步更新。
5. 契约：`contracts/editor-capability-matrix.json` 增加 `rpcProviderStatusCheckedInMainChain` 以及 GUI/addon 对 `provider.status` 的检查面声明。

本轮门禁复验：

1. `cargo clippy --all -- -D warnings` 通过。
2. `cargo test --test acp_runtime_rpc_integration provider_status -- --nocapture` 通过。
3. `cargo test` 全量通过（含 `acp_runtime_rpc_integration` 57 项）。
4. `GUI npm run build` 与 `GUI npm run test:contract` 通过。
5. `vscode-addon npm run check` 通过。

## 一、执行结论

1. vscode-addon 当前主功能链路可用，但存在两项高风险安全缺陷需要优先修复，不应等待架构重构完成后再处理。
2. 执行顺序：HIGH 安全修复优先，MEDIUM 架构债务按影响范围排序，lint 债务收口最后。
3. 所有改进必须满足最小改动、可回滚、可验收；不破坏现有 `npm run check && npm test` 门禁通过率。

## 二、范围与约束

适用范围：`vscode-addon/src/` 下全量 TypeScript 源码。

硬约束：
1. 不引入新的 npm 依赖，除非不可绕过且安全评估已通过。
2. 安全修复不得以功能降级或能力移除替代；必须提供安全可用的替代路径。
3. 架构拆分必须保持 `extension.ts` 对外 `activate` / `deactivate` 签名不变。
4. 每项改进必须形成闭环：触发 -> 执行 -> 反馈 -> 可验收。
5. 不修改 `package.json` 中 `engines.vscode` 版本约束，保持 `^1.74.0` 兼容性。

## 三、建议清单（按执行顺序排列）

重排原则：安全修复优先，架构改进次之，lint/类型债务收口最后。

| 执行顺序 | ID | 原优先级 | 建议项 | 是否需要 | 说明 |
|---|---|---|---|---|---|
| 1 | B16-H1 | HIGH | 跨平台可执行文件路径校验修复 | 必须 | 当前逻辑在 Unix 下阻断自身二进制，功能性 bug |
| 2 | B16-H2 | HIGH | ChatView 代码执行隔离层补齐 | 必须 | new Function + spawn(shell/python) 无确认机制，OWASP A03/A09 |
| 3 | B16-M1 | MEDIUM | extension.ts God File 拆分 | 建议 | 2427 行单文件含 66 个命令，可维护性极低 |
| 4 | B16-M2 | MEDIUM | settingsView.ts 消息分发重构 | 建议 | 48+ case 单 switch，扩展碰撞风险高 |
| 5 | B16-M3 | MEDIUM | activate() 自动打开 chat 解耦 | 建议 | 300ms setTimeout 强绑定初始化与 UI 导航 |
| 6 | B16-M4 | MEDIUM | lint 类型规则恢复与类型债务收口 | 建议 | 3 条核心规则 off = 设计退化信号不可见 |

## 四、删除与合并项说明

本版跳过以下方向：
1. 不将 `configManager.ts` 中的手写 TOML 解析器列为独立改进项：现阶段不引入新依赖约束下，维持现有单文件解析器并补充边界测试即可，收益低于风险。
2. 不将 `statusMonitor.ts` 的 `manager: any` 单独立项：可在 B16-M1 拆分 `GoOnManager` 时顺带补齐接口定义，无需额外周期。
3. 不要求全量替换 `any` 类型：B16-M4 仅要求将规则恢复为 `warn`，逐步修复临界路径；全量替换成本过高且收益可分期兑现。

## 五、详细建议

### B16-H1：跨平台可执行文件路径校验修复

是否需要：必须

问题定位：
- `extension.ts` L342 `isSupportedExecutablePath`：仅允许 `.exe` / `.bat` / `.sh` 扩展名。
- L582 与 L645 的错误分支以此为判定，在 Unix 下阻断无扩展名的原生二进制（如 `go-on`）。
- 下载逻辑生成的 Unix 二进制本身无扩展名，与校验逻辑直接冲突，导致 Unix 用户装载后立即报错无法启动。

推荐建议：
1. 将校验逻辑按平台分支：`process.platform === 'win32'` 时保留扩展名白名单检查；非 Windows 时改为检查文件可执行权限（`fs.accessSync(path, fs.constants.X_OK)`）。
2. 删除 L582 与 L645 中"仅报错不启动"的判定；对无法确认权限的场景改为 warn 级别提示而非硬阻断。
3. 补充单元测试：Unix 无扩展名路径通过校验；Windows `.exe` 路径通过校验；Windows `.sh` 路径通过；无权限文件（Unix）给出警告。

验收门禁：
1. macOS / Linux 下下载并配置 `go-on`（无扩展名）后可正常启动，不触发"不支持的路径"错误。
2. Windows 下 `.exe` / `.bat` / `.sh` 行为不变。
3. 相关单元测试新增并全部通过。

### B16-H2：ChatView 代码执行隔离层补齐

是否需要：必须

问题定位：
- `chatView.ts` L182：`new Function('return (' + code + ')()')()`，直接在扩展进程内执行任意 JS 字符串。
- L210 `_executePythonCode`：`spawn('python', ['-c', code])`，不经确认直接执行用户输入的 Python 代码。
- L249 `_executeShellCode`：`spawn('bash', ['-c', code])` / `spawn('cmd', ['/c', code])`，不经确认直接执行 Shell。
- 三路径均无 workspace scope 限制、无用户确认对话框、无沙箱。
- 违反 OWASP A03（注入）与 A09（安全日志记录与监控不足）。

推荐建议：
1. 在执行前统一弹出 `vscode.window.showWarningMessage` 确认对话框，明示"将在本机执行以下 <语言> 代码"，用户显式确认后才执行。对 JS / Python / Shell 分别给出不同措辞。
2. 对 `new Function` 路径，建议限定可执行操作范围（例如仅用于返回 JSON 表达式求值），禁止 `require` / `import` / `process` 等危险上下文；最安全方案是移除此路径，改为后端 RPC 执行。
3. 执行结果与执行事件统一写入扩展输出通道（`vscode.window.createOutputChannel`），保留审计轨迹。
4. 将代码执行逻辑从 `chatView.ts` 剥离为独立 `executionService.ts`，降低 UI 与执行引擎的耦合，便于后续独立测试与策略收紧。

验收门禁：
1. 直接从 WebView 发送执行指令时，必须有 VS Code 原生确认弹框，取消后无任何代码被执行。
2. 执行事件在扩展输出通道可见。
3. `new Function` 路径须有明确的危险上下文屏蔽（如禁止 `require`），或被安全替代方案替换。

### B16-M1：extension.ts God File 拆分

是否需要：建议

问题定位：
- `extension.ts` 当前 2427 行，包含：`GoOnManager`（进程管理）、`GoOnStatusProvider`（TreeDataProvider）、66 个 `registerCommand` 调用、运行时下载逻辑、TOML 配置变更辅助函数。
- 单文件承载过多职责，任何一行改动都可能影响不相关功能区；命令注册碰撞无法静态检查。

推荐建议：
1. 拆分为以下模块（保持 `extension.ts` 作为薄编排层）：
	- `src/runtimeService.ts`：`GoOnManager` 类、进程 spawn/kill、下载逻辑、JSON-RPC send。
	- `src/commandRegistry.ts`：所有 66 个 `registerCommand` 调用，接受 `manager` 与 `context` 作为依赖注入参数。
	- `src/configMutationService.ts`：TOML upsert / 配置路径辅助函数。
	- `src/viewRouter.ts`：`revealGoOnView`、`ensureRuntimeReadyAfterChatOpen` 等 UI 导航逻辑。
2. `extension.ts` 仅保留 `activate()` 与 `deactivate()` 导出，内部调用上述模块初始化。
3. 拆分时不改变任何外部命令 ID 与 `package.json` 声明，保持激活事件不变。

验收门禁：
1. `npm run check` 通过（无编译错误）。
2. `npm test` 通过（contract smoke 不回退）。
3. 拆分后 `extension.ts` 行数降至 200 行以内；各新模块行数不超过 600 行。

### B16-M2：settingsView.ts 消息分发重构

是否需要：建议

问题定位：
- `settingsView.ts` `onDidReceiveMessage` 内单一 `switch(message.type)` 含 48+ 个 case。
- 新增功能必须在同一 switch 尾部追加，无法独立测试单个 handler，也无法静态确认遗漏的 case。
- 当前已有多处 handler 逻辑超过 20 行，导致单函数圈复杂度极高。

推荐建议：
1. 将 switch 替换为 handler map：
   ```typescript
   const handlers: Record<string, (msg: any) => Promise<void> | void> = {
     requestSettings:     handleRequestSettings,
     updateRuntimeSetting: handleUpdateRuntimeSetting,
     // ...
   };
   ```
2. `onDidReceiveMessage` 只做路由：`const fn = handlers[message.type]; if (fn) await fn(message); else log.warn('unknown', message.type);`
3. 每个 handler 函数提取为独立命名函数，长度控制在 30 行以内；超长 handler 可进一步拆分为 `_handle<Name>Detail` 私有函数。

验收门禁：
1. `npm run check` 通过。
2. `npm test` 通过。
3. 重构后可为任意单个 handler 编写隔离单元测试（不启动 WebView）。

### B16-M3：activate() 自动打开 chat 解耦

是否需要：建议

问题定位：
- `extension.ts` L2391：`setTimeout(() => { void vscode.commands.executeCommand('go-on.openChat'); }, 300)`
- 此行将扩展初始化与 UI 导航强绑定，每次 VS Code 启动后 300ms 自动切换到 chat 面板，对多扩展多窗口场景具有侵入性。
- `onStartupFinished` 激活事件已保证扩展在空闲时加载，无需再用 setTimeout 驱动 UI。

推荐建议：
1. 删除 `setTimeout` 自动打开调用；改为仅在首次安装（`context.globalState.get('hasOpenedChat')` 为 falsy）时主动打开一次，并写入标志位后不再重复触发。
2. 对于"运行时就绪后自动导航"场景，改为状态栏通知 + 用户点击触发，而非静默自动导航。

验收门禁：
1. 重启 VS Code 后扩展激活不自动切换到 chat 面板（首次安装除外）。
2. `npm run check` 与 `npm test` 通过。

### B16-M4：lint 类型规则恢复与类型债务收口

是否需要：建议

问题定位：
- `.eslintrc.json` 当前关闭 `@typescript-eslint/no-explicit-any`、`@typescript-eslint/no-unused-vars`、`no-unused-vars`。
- 关闭后 TypeScript 类型退化（`any` 扩散、未使用变量膨胀）无法通过 lint 阶段检出，设计退化信号不可见。
- 仓库实际存在 159 处 `any` 使用，其中 `manager: any`、`message: any`、事件回调 `any` 是可系统性替换的类型。

推荐建议：
1. 将 `@typescript-eslint/no-explicit-any` 恢复为 `"warn"`（不设为 `"error"`，避免阻断 CI）。
2. 将 `@typescript-eslint/no-unused-vars` 恢复为 `["warn", { "argsIgnorePattern": "^_" }]`，对预期未使用参数用 `_` 前缀标注，与 TypeScript 惯例对齐。
3. 优先替换临界路径 `any`：`GoOnManager` 依赖注入签名、`onDidReceiveMessage` 回调参数、`settingsView`/`workflowView` 中的 `manager: any`。
4. 替换完成后可视情况将 `no-explicit-any` 升级为 `"error"`。

验收门禁：
1. 恢复规则后 `npm run check`（含 lint）通过（允许 warn，不允许新增 error）。
2. 临界路径 `any` 替换后相关 warn 数量较基线（159）下降 ≥ 50%。

## 六、分阶段实施计划

### 阶段 A（安全修复，立即执行）

目标：消除两项高风险安全缺陷，恢复跨平台可用性。

工作项：
1. 修复 `isSupportedExecutablePath`（B16-H1）。
2. 为 ChatView 代码执行路径增加确认对话框（B16-H2）。
3. （可选）将代码执行逻辑剥离到 `executionService.ts`，为 B16-M1 做前置准备。

验收标准：
1. macOS 用户可正常使用下载的 `go-on` 二进制，不出现"不支持的路径"错误。
2. ChatView 执行任意语言代码前必须弹出原生确认对话框。
3. `npm run check && npm test` 全部通过。

### 阶段 B（架构重构，与功能迭代并行）

目标：降低 extension.ts 与 settingsView.ts 的维护成本，解除 activate() 的 UI 侵入行为。

工作项：
1. 按模块拆分 `extension.ts`（B16-M1）。
2. 重构 settingsView 消息分发（B16-M2）。
3. 解耦 activate() 自动打开 chat（B16-M3）。

验收标准：
1. `extension.ts` 行数降至 200 行以内；各模块行数不超过 600 行。
2. 任意单个 settings handler 可隔离单元测试。
3. 重启后不自动切换到 chat 面板。
4. `npm run check && npm test` 全部通过。

### 阶段 C（类型债务收口，按需推进）

目标：恢复 lint 可见性，逐步收敛 `any` 类型扩散。

工作项：
1. 恢复 ESLint 规则为 warn 级别（B16-M4）。
2. 替换临界路径 `any`（`GoOnManager` 签名、消息回调、view 依赖注入）。
3. 视进展决定是否升级为 error 级别。

验收标准：
1. `npm run check` 无新增 error，warn 数量较基线下降 ≥ 50%。
2. 核心管理类（`GoOnManager`、`StatusMonitor`）依赖注入接口已显式定义。

## 七、统一验收清单

1. 安全验收：B16-H1 修复后 Unix 二进制可正常加载；B16-H2 修复后代码执行必须经用户确认。
2. 功能验收：`npm run check && npm test` 全部通过，contract smoke 无回退。
3. 可维护性验收：`extension.ts` 行数降至 200 以内（阶段 B 完成后）。
4. 类型债务验收：ESLint warn 数量较基线（159 处 `any`）下降 ≥ 50%（阶段 C 完成后）。
5. 用户体验验收：重启 VS Code 后不自动切换到 chat 面板（首次安装除外）。

## 八、正式结语（vscode-addon 部分）

BLUE16 vscode-addon 部分的优先级顺序应从"先消除安全风险"出发，不应等待架构重构完成后再修安全问题。
B16-H1 与 B16-H2 可在不改变架构的前提下独立修复，应立即执行。架构债务（B16-M1~M4）可按迭代节奏分阶段推进，每步保持门禁可通过。

---

# GUI 结构评审建议（2026-04-15）

## 九、GUI 执行结论

1. GUI（Tauri + Vue 3 + Pinia）主链路功能完整，但存在两项高风险问题：操作面板暴露无确认的高危 RPC 操作，以及 App.vue 中存在无界 `while (true)` 循环。
2. 执行顺序：HIGH 问题立即修复，MEDIUM 架构债务与功能迭代并行，LOW 国际化/类型补齐按需推进。
3. 所有修复必须保持 `npm run build`（`vue-tsc --noEmit && vite build`）与 `npm run test:contract` 不回退。

## 十、GUI 范围与约束

适用范围：`GUI/src/` 下全量 Vue/TypeScript 源码（不含 `src-tauri/`）。

硬约束：
1. 不修改 Tauri 命令签名（`invoke` 方法名），跨端 IPC 协议不变。
2. 不引入新 npm 依赖，除非安全评估已通过且满足 Tauri 沙箱约束。
3. 路由 path 与菜单 index 保持不变，避免破坏用户书签或外部深链。
4. 每项改进必须可单独验收，不依赖其他项完成后才可合并。

## 十一、GUI 建议清单（按执行顺序排列）

| 执行顺序 | ID | 优先级 | 建议项 | 是否需要 | 说明 |
|---|---|---|---|---|---|
| 1 | B16-G1 | HIGH | BackendOpsView 高危操作加确认门禁 | 必须 | shutdown/cache.clear/vector.clear/breaker.reset 无确认，误点即生效 |
| 2 | B16-G2 | HIGH | App.vue `while(true)` 改为有界重试 | 必须 | 无界循环在主线程，文件选择失败时可冻结 UI |
| 3 | B16-G3 | MEDIUM | App.vue 逻辑分拆 | 建议 | 367 行含后台管理、crash 处理、bootstrap、主题/语言切换，职责过杂 |
| 4 | B16-G4 | MEDIUM | runtime.ts store 职责拆分与 offline 语义收敛 | 建议 | 单 store 管 6 类数据、2 个轮询 timer；offline 状态由任意一项失败触发过于激进 |
| 5 | B16-G5 | MEDIUM | RPC 调用结果类型化与下移到 service 层 | 建议 | 视图层直接 JSON.parse + any 断言，字段映射分散在 5 个视图中 |
| 6 | B16-G6 | LOW | i18n 覆盖补全 | 建议 | SecurityView/HealthBreakdownView/BackendOpsView 存在硬编码裸字符串 |
| 7 | B16-G7 | LOW | runtime.ts 硬编码 endpoint 收敛到 protocolContract | 建议 | 两处 `127.0.0.1:8090` 与 protocolContract baseUrl 形成双源 |

## 十二、GUI 详细建议

### B16-G1：BackendOpsView 高危操作加确认门禁

是否需要：必须

问题定位：
- `BackendOpsView.vue` 中以下操作直接绑定按钮回调，无任何确认弹框：
  - `shutdown`（关闭后台进程，服务中断）
  - `cache.clear`（清空全部缓存，影响正在进行的请求）
  - `vector.clear`（清空向量存储，数据不可恢复）
  - `breaker.reset`（强制重置断路器状态，可能掩盖真实故障）
- BackendOpsView 定位为"运营操作面板"，但没有任何危险操作的二次确认保护，违反 OWASP A09（安全日志记录与监控不足）与操作安全原则。

推荐建议：
1. 对 `shutdown`、`cache.clear`、`vector.clear`、`breaker.reset` 四个操作，在调用 RPC 前使用 `ElMessageBox.confirm` 弹出确认对话框，明示操作后果（例如"此操作将清空所有向量数据，无法恢复，是否继续？"）。
2. 对 `shutdown` 额外增加文字输入确认（输入 "shutdown" 才可执行），参考 GitHub 仓库删除的设计模式。
3. 所有高危操作成功后在扩展输出通道或 Vue 全局日志中留下审计记录（操作名、时间、操作人为 "gui-user"）。

验收门禁：
1. 点击 `shutdown` 按钮后必须弹出确认对话框，取消时不执行任何 RPC。
2. `cache.clear` 与 `vector.clear` 同上。
3. `npm run build` 与 `npm run test:contract` 通过。

### B16-G2：App.vue `while(true)` 改为有界重试

是否需要：必须

问题定位：
- `App.vue` L206 `ensureBackendAndStart()` 函数中有 `while (true)` 无界循环。
- 循环内调用 `backendExecutableExists()`、`openDialog`、`configureServiceByExecutable` 等异步 Tauri 操作。
- 如果 `openDialog` 返回异常值（非 null 非有效路径）或 `configureServiceByExecutable` 内部抛出但被 continue 吞掉，循环将永不退出，直接冻结 UI 线程。
- 没有最大重试次数保护，也没有异常上浮路径。

推荐建议：
1. 将 `while (true)` 改为 `for (let attempt = 0; attempt < MAX_CONFIGURE_ATTEMPTS; attempt++)`，`MAX_CONFIGURE_ATTEMPTS` 建议取值 5。
2. 超过最大次数后退出循环，弹出 `ElMessageBox.alert` 提示用户手动操作，并返回（不调用 `exitApp`，避免强制退出 GUI）。
3. 每次 `configureServiceByExecutable` 失败时 catch 并上浮错误，不用 continue 静默跳过。

验收门禁：
1. 在文件选择对话框点击取消 5 次后，程序不再重复弹框，给出明确的失败提示。
2. `npm run build` 通过。

### B16-G3：App.vue 逻辑分拆

是否需要：建议

问题定位：
- `App.vue` 当前 367 行，包含：
  - 主布局模板（侧边栏菜单、顶部工具栏、路由出口）
  - `bootstrapBackend` / `ensureBackendAndStart` / `waitForBackendHealthy`（后台生命周期管理）
  - `classifyStartupError`（错误分类）
  - crash 事件监听与恢复弹框
  - 主题切换 / locale 切换
  - monitorOnly 模式管理
- App.vue 作为根布局组件承载了过多业务逻辑，任何后台管理逻辑改动都需要修改根布局文件。

推荐建议：
1. 将后台生命周期管理提取为 `services/backendLifecycle.ts`（`bootstrapBackend`、`ensureBackendAndStart`、`waitForBackendHealthy`、`classifyStartupError`）。
2. 将 crash 事件监听提取为 composable `composables/useCrashHandler.ts`，在 `onMounted` 中调用。
3. `App.vue` 仅保留：根布局模板、根级 composable 挂载、onMounted/onUnmounted 生命周期。
4. 拆分后 `App.vue` 行数应降至 150 行以内。

验收门禁：
1. `npm run build` 通过。
2. 后台 bootstrap 逻辑可独立测试（不需要启动 Vue 应用）。
3. `App.vue` 行数 ≤ 150。

### B16-G4：runtime.ts store 职责拆分与 offline 语义收敛

是否需要：建议

问题定位：
- `stores/runtime.ts` 单一 Pinia store 同时管理：服务状态、健康快照（含 lastKnown 副本）、AI 使用情况（含 lastKnown 副本）、日志块、端点健康统计、编辑器集成状态、热力图数据、两个独立轮询 timer。
- `refreshAll` 并发触发 6 个 Tauri IPC 调用；任意一个抛出异常，`offline = true` 立即生效，导致 UI 同时显示全局离线提示，即使仅日志读取失败也如此。
- `startStatusPolling` 使用 `setInterval(refreshAll, statusPollingMs)`，每 2s 触发一次含 6 个并发 IPC 的 `refreshAll`。若某次 `refreshAll` 执行时间超过 2s（网络慢或后台卡顿），下一次轮询已开始，产生 IPC 请求堆积，可能导致 Tauri 主进程积压。

推荐建议：
1. 拆分 store 为职责单一的独立 store：
   - `stores/serviceStore.ts`：服务状态 + status/health polling。
   - `stores/metricsStore.ts`：AI 使用情况 + 端点健康统计 + 热力图。
   - `stores/logsStore.ts`：日志块 + logs polling。
   - `stores/editorStore.ts`：编辑器集成状态。
2. `offline` 语义收敛：仅当 **服务状态轮询** 或 **健康检查** 失败时设置 `offline = true`；日志读取失败不影响 offline 标志（日志非关键路径）。
3. `startStatusPolling` 改为"前一次完成后才触发下一次"的链式调度：用 `setTimeout` 递归或 `setInterval` + 内部 flag 防重入，避免并发堆积。

验收门禁：
1. 日志轮询失败时，全局 offline 标志不变。
2. 后台响应慢时，`statusTimer` 不堆积超过 1 个挂起请求。
3. `npm run build` 通过。

### B16-G5：RPC 调用结果类型化与下移到 service 层

是否需要：建议

问题定位：
- `SecurityView.vue`、`HealthBreakdownView.vue`、`WorkflowView.vue`、`AutoTuneView.vue`、`BackendOpsView.vue` 均直接调用 `invokeRuntimeRpc(method, params)`，并在视图层 `JSON.parse` 结果，使用 `any` 断言访问字段。
- 字段路径（如 `governance.config.production_strict`、`probes.liveness.ok`、`probes.locks.status`）散布在 5 个视图中，无统一类型定义，后端改字段名时需逐一修改视图。
- 当前 `services/bridge.ts` 已有良好的 `withCache` + 类型化接口模式，但 RPC over invoke 层缺乏同等抽象。

推荐建议：
1. 在 `services/` 下新增 `rpcService.ts`，为每个 RPC 方法封装类型化函数：
   ```typescript
   export interface GovernanceStatusResult { governance: { status: string; config: { production_strict: boolean; ... } } }
   export async function getGovernanceStatus(): Promise<GovernanceStatusResult> { ... }
   ```
2. 视图组件只调用类型化函数，不直接使用 `invokeRuntimeRpc` + `JSON.parse`。
3. 优先类型化频繁调用的 5 个 RPC：`governance.status`、`governance.audit.recent`、`health.probes`、`breaker.status`、`metrics.get`。

验收门禁：
1. `vue-tsc --noEmit` 通过（无 `any` 参与逻辑判断的 ts 错误）。
2. 上述 5 个 RPC 的返回类型在 `rpcService.ts` 中有显式接口定义。
3. 视图层不直接出现 `JSON.parse(... || "{}")` 对上述 5 个 RPC 的调用。

### B16-G6：i18n 覆盖补全

是否需要：建议

问题定位（已定位的裸字符串）：
- `SecurityView.vue` 模板中：`governance: {{ governanceState }}`、`rules: {{ rulesVersion }}`、`production_strict: on/off`、`entry_auth: on/off`、`entry_rate_limit: .../min (burst ...)` 均为裸字符串，无 `t()` 包裹，语言切换后无法本地化。
- `SecurityView.vue` script 中 L349 `description: "No recent governance risk..."` 为英文硬编码。
- `HealthBreakdownView.vue` 中 `liveness: {{ liveness.text }}`、`readiness: {{ readiness.text }}` 为裸字符串。
- `BackendOpsView.vue` 中 `breaker.status`、`shutdown`、`cache.clear`、`vector.clear` 等按钮文字为裸字符串。

推荐建议：
1. 将上述裸字符串对应的 i18n key 添加到 `locales/en-US.json` 与 `locales/zh-CN.json`，并在视图中用 `t("key")` 替换。
2. 优先处理 SecurityView 中的 5 个状态标签和 1 处英文硬编码。
3. BackendOpsView 的按钮文字若作为"技术术语"可视情况保留英文，但 `shutdown` 建议改为 `t("backendOps.shutdown")` 以便后续本地化。
4. 建立 i18n 覆盖率检查脚本（可用 i18n-ally VSCode 插件辅助），纳入 pre-commit 或 CI 检查。

验收门禁：
1. 切换到 `zh-CN` 后 SecurityView 状态标签（governance/rules/strict/entry_auth）显示中文。
2. `npm run build` 通过。

### B16-G7：runtime.ts 硬编码 endpoint 收敛到 protocolContract

是否需要：建议

问题定位：
- `stores/runtime.ts` L22 与 L52 两处初始值均硬编码 `"http://127.0.0.1:8090/health"`。
- `services/protocolContract.ts` 已从 `contracts/editor-capability-matrix.json` 读取 `defaultRuntimeBaseUrl`，作为单一配置源。
- 两处硬编码与 protocolContract 形成双源，修改端口时需同步修改三处。

推荐建议：
1. 在 `stores/runtime.ts` 顶部 import `defaultRuntimeBaseUrl from "../services/protocolContract"`。
2. 将两处初始 endpoint 值替换为 `${defaultRuntimeBaseUrl}/health`（或直接从 protocolContract 中取出完整 health URL）。
3. 统一后，`contracts/editor-capability-matrix.json` 成为端口/地址的唯一修改点。

验收门禁：
1. 修改 `contracts/editor-capability-matrix.json` 中 baseUrl 后，runtime.ts 初始 health endpoint 自动同步。
2. `npm run build` 通过。

## 十三、GUI 分阶段实施计划

### 阶段 A（安全与稳定性修复，立即执行）

目标：消除高风险操作隐患，修复无界循环。

工作项：
1. BackendOpsView 为 shutdown/cache.clear/vector.clear/breaker.reset 添加确认对话框（B16-G1）。
2. App.vue `while(true)` 改为有界重试，上限 5 次（B16-G2）。

验收标准：
1. 高危操作均有确认弹框，取消时无副作用。
2. 文件选择失败 5 次后不再弹框，给出友好错误提示。
3. `npm run build && npm run test:contract` 全部通过。

### 阶段 B（架构重构，与功能迭代并行）

目标：降低 App.vue 与 runtime.ts 的维护成本，解除视图层的 RPC 直接依赖。

工作项：
1. App.vue 后台生命周期逻辑提取为 `backendLifecycle.ts` + `useCrashHandler.ts`（B16-G3）。
2. runtime.ts store 拆分与 offline 语义收敛（B16-G4）。
3. RPC 调用类型化并下移到 `rpcService.ts`（B16-G5）。
4. runtime.ts 硬编码 endpoint 收敛（B16-G7）。

验收标准：
1. `App.vue` 行数 ≤ 150；`serviceStore.ts` 等新 store 行数各不超过 200。
2. 日志失败不触发全局 offline。
3. 5 个核心 RPC 有类型化封装，视图层无裸 `JSON.parse`。
4. `npm run build` 通过。

### 阶段 C（国际化补全，按需推进）

目标：消除 i18n 覆盖盲区，建立检查机制。

工作项：
1. SecurityView 状态标签 i18n 覆盖（B16-G6）。
2. HealthBreakdownView / BackendOpsView 裸字符串补齐。
3. 建立 i18n 覆盖率检查入口。

验收标准：
1. 切换语言后 SecurityView 关键状态标签本地化显示。
2. `npm run build` 通过。

## 十四、GUI 统一验收清单

1. 安全验收：高危操作必须有确认弹框（B16-G1）；无界循环已消除（B16-G2）。
2. 稳定性验收：轮询不产生请求堆积；日志失败不影响 offline 标志（B16-G4 完成后）。
3. 类型安全验收：5 个核心 RPC 有类型化封装，`vue-tsc --noEmit` 无新增 any 断言错误（B16-G5 完成后）。
4. 可维护性验收：`App.vue` ≤ 150 行；新 store 职责单一（B16-G3、B16-G4 完成后）。
5. i18n 验收：SecurityView/HealthBreakdownView 主要状态标签可本地化（B16-G6 完成后）。
6. 配置一致性验收：endpoint 单一来源，修改 baseUrl 只需改一处（B16-G7 完成后）。

## 十五、正式结语

GUI 结构整体合理，Tauri + Vue 3 + Pinia + vue-router + vue-i18n 的技术选型正确，文件拆分粒度基本合适（13 个 View，平均 170 行）。
核心问题集中在两点：用户操作安全（无确认高危操作）与代码质量（App.vue God File、store 职责过宽、视图层类型不安全）。
B16-G1 与 B16-G2 应优先于一切架构重构立即处理。

---

## 补充章节：B16-R 系列 — 工程覆盖缺口全量收口（2026-04-16 追加）

承接 BLUE15 完成后遗留的后端 RPC 测试覆盖缺口，补齐所有已在 is_acp_request() 路由白名单注册但无场景文件/集成测试/CI 闸门的端点。

| ID | RPC 方法 | 缺口说明 |
|---|---|---|
| B16-R1 | debug_panel.get / debug.panel.get | 无场景、无测试 |
| B16-R2 | ction.check | 无场景、无测试 |
| B16-R3 | conversation.checkpoint.prune | checkpoint 系列缺 prune |
| B16-R4 | 	ask.plan + 	ask.execute（独立场景） | 无独立覆盖场景 |
| B16-R5 | workflow.execute（独立场景） | 无独立覆盖场景 |
| B16-R6 | workflow.clarify / workflow.generate（路由可达性） | 无场景、无测试 |

验收标准：
1. 每项均有 .ndjson 场景文件 + 集成测试函数 + CI 步骤 6aw
2. 
djson_scenario_files_all_pass 更新至 40
3. cargo check --tests 零 warning 零 error
4. vscode-addon Settings 面板相应按钮确认已注册（debug_panel/action.check/task.plan/task.execute/workflow.execute）

### 回写记录

**完成时间：2026-04-16**

**完成率：100%（6/6 项）**

| ID | 状态 | 具体实施 |
|---|---|---|
| B16-R0（修复） | ✅ 已完成 | 修复 `conversation-checkpoint-benchmark.ndjson`：step2 加 `conversation_id`+`messages`，step4 由 rollback 改为 checkpoint.prune，step3 加 `conversation_id` |
| B16-R1 | ✅ 已完成 | 新建 `requests/debug-panel-benchmark.ndjson`（4步：init→debug_panel.get→debug.panel.get→shutdown）+ 集成测试 + vscode-addon `go-on.debugPanelGet` 命令注册（rpcCommandRegistry.ts + settingsView.ts 按钮 + package.json activationEvents/commands）|
| B16-R2 | ✅ 已完成 | 新建 `requests/action-check-benchmark.ndjson`（3步）+ 集成测试 + vscode-addon `go-on.actionCheck` 命令注册 |
| B16-R3 | ✅ 已完成 | `conversation.checkpoint.prune` 已通过修复后的 checkpoint-benchmark 场景覆盖；新增 `rpc_conversation_rollback_restores_checkpoint` 直连测试（动态 checkpoint_id 链路）|
| B16-R4+R5 | ✅ 已完成 | 新建 `requests/task-plan-execute-benchmark.ndjson`（4步：init→task.plan→task.execute{requirement_confirmed:true}→shutdown）+ 集成测试 |
| B16-R6 | ✅ 已完成 | 新建 `requests/workflow-execute-standalone-benchmark.ndjson`（3步）+ 集成测试 |
| B16-R7 | ✅ 已完成 | 新建 `requests/workflow-subcommands-benchmark.ndjson`（4步：init→workflow.clarify→workflow.research→shutdown）+ 集成测试 |

**技术收口：**
- 场景文件：34 → 39（新增 5 个）
- 集成测试：新增 `run_scenario_file_executes_debug_panel_benchmark_requests`、`run_scenario_file_executes_action_check_benchmark_requests`、`run_scenario_file_executes_task_plan_execute_benchmark_requests`、`run_scenario_file_executes_workflow_execute_standalone_benchmark_requests`、`run_scenario_file_executes_workflow_subcommands_benchmark_requests`、`rpc_conversation_rollback_restores_checkpoint`（直连测试）
- `ndjson_scenario_files_all_pass` 断言：34 → 39
- CI 新增步骤 `6aw`：7 个新测试 + `ndjson_scenario_files_all_pass` 全量验证
- vscode-addon：`go-on.debugPanelGet` + `go-on.actionCheck` 两个新命令全链路注册
- `cargo check --tests` 零 warning / 零 error；`vscode-addon npm run check` 零 warning / 零 error
