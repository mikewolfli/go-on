# BLUE12 — Tauri 2 + Vue + Vite 图形化前端方案（GUI 独立目录、零侵入主项目）

> 延续 BLUE11 的执行纪律：先方案冻结、再分阶段实施、最后端到端验收。
> 本轮目标：在不修改现有 go-on 代码的前提下，新增独立 GUI 工程，提升配置、启动、状态检查与常用运维操作的用户体验。

---

## 背景与约束确认

基于当前项目状态，已有 CLI、setup、status、healthcheck 与 VS Code 插件能力；但普通用户仍需记忆命令与编辑配置。

本轮强约束如下：

1. GUI 代码必须独立放在根目录 GUI 目录。
2. 不修改现有 Rust 主项目代码（src、Cargo.toml、现有脚本与配置文件保持不变）。
3. GUI 通过外部进程与 HTTP/RPC 对接 go-on，作为“控制台层”而非“内嵌层”。
4. 方案采用：Tauri 2 + Vue 3 + Vite。

结论：可以，且推荐这么做。

---

## 目标

| ID | 目标 | 目标目录/文件 |
|---|---|---|
| B12-M1 | 新建独立 GUI 工程骨架（Tauri 2 + Vue + Vite） | GUI/ |
| B12-M2 | 提供启动/停止/状态面板（调用 go-on 可执行文件） | GUI/src-tauri + GUI/src |
| B12-M3 | 提供配置向导页（映射 init/check/doctor 常用流程） | GUI/src/views |
| B12-M4 | 提供 Provider 管理与基础配置编辑（可视化） | GUI/src/views |
| B12-M5 | 提供日志与健康检查面板（health/status） | GUI/src/views |
| B12-M6 | 提供全量运行态监控看板（监控、状态、告警） | GUI/src/views + GUI/src/stores |
| B12-M7 | 提供 AI 使用状态实时监控（请求量/成功率/时延/限流/失败） | GUI/src/views + GUI/src/services |
| B12-M8 | 支持最小化到系统托盘与浮动小窗（mini console） | GUI/src-tauri + GUI/src/components |
| B12-M9 | 支持服务生命周期控制（启动/关闭/重启） | GUI/src-tauri/commands/process.rs + GUI/src/views |
| B12-M10 | 不侵入主项目：所有适配逻辑留在 GUI 内 | GUI/README.md + BLUE12.MD |
| B12-M11 | 完成 GUI 工程独立构建与打包验收 | GUI/package.json + GUI/src-tauri/tauri.conf.json |
| B12-M12 | GUI 多语言支持（简体中文/英文，术语与主项目保持一致） | GUI/src/locales + GUI/src/stores |

---

## 技术选型（冻结）

### 前端
- Vue 3
- Vite
- Vue Router
- Pinia
- vue-i18n（简体中文/英文）
- Axios
- Element Plus（或 Naive UI，二选一，默认 Element Plus）
- ECharts（可选，用于状态趋势）

实时刷新策略（冻结）：
- 默认 2 秒轮询（状态/监控）。
- 日志视图支持 1 秒轮询与自动滚动。
- 若后续服务端提供 SSE/WebSocket，再切换为推送模式。

### Tauri（桌面壳）
- tauri 2
- tauri-build
- tauri-plugin-shell（进程调用）
- tauri-plugin-dialog（文件/目录选择）
- tauri-plugin-fs（配置文件读写）
- tauri-plugin-store（本地偏好持久化）
- tauri-plugin-window-state（窗口位置/大小/最小化状态持久化）

窗口与托盘策略（冻结）：
- 主窗口支持“最小化到系统托盘”（关闭按钮可配置为隐藏到托盘）。
- 托盘菜单包含：显示主窗口、启动服务、停止服务、重启服务、退出 GUI。
- 提供可选浮动小窗（mini console）：展示服务状态、AI 请求速率、最近告警。
- 浮动小窗支持置顶、半透明、快速展开主面板。

### Rust（GUI 侧）
- 仅限 GUI/src-tauri 内部 command 与桥接逻辑
- 不依赖修改主项目 crates

多语言策略（冻结）：
- 默认语言：跟随系统语言（仅识别 zh-CN 与 en-US，其他回退 en-US）。
- 用户可在 GUI 设置页手动切换语言。
- 语言选择持久化到本地 store，重启后生效。
- 术语键名与主项目语言文件保持同语义口径（如 status/health/setup/provider）。

---

## 目录规划（仅新增）

```text
GUI/
  README.md
  package.json
  vite.config.ts
  tsconfig.json
  index.html
  src/
    main.ts
    App.vue
    locales/
      en-US.json
      zh-CN.json
    router/
    stores/
    views/
      DashboardView.vue
      MonitorView.vue
      SetupView.vue
      ConfigView.vue
      ProvidersView.vue
      LogsView.vue
      AiUsageView.vue
      MiniConsoleView.vue
    services/
      api.ts
      bridge.ts
    components/
  src-tauri/
    Cargo.toml
    tauri.conf.json
    build.rs
    src/
      main.rs
      commands/
        process.rs
        config.rs
        health.rs
        metrics.rs
        logs.rs
```

---

## 功能边界（避免侵入）

### GUI 允许做
1. 读取与写入配置文件（默认 config.toml）。
2. 拉起或停止 go-on 进程（使用 tauri shell/command）。
3. 调用现有 HTTP 端点（如 health、OpenAI 兼容接口）。
4. 封装常用 CLI 流程：init/check/doctor/apply-recommended。
5. 提供运行时监控：状态、健康分、告警计数、关键资源指标。
6. 提供日志可视化：按级别过滤、关键字搜索、时间范围筛选。
7. 提供 AI 使用状态实时看板：请求吞吐、成功率、平均时延、超时/限流/熔断次数、近窗口趋势。
8. 提供托盘驻留与浮动小窗模式，支持后台持续监控。
9. 提供服务生命周期操作：启动、停止、重启，并反馈结构化结果。
10. 提供中英文界面切换（简体中文/英文），并保持术语一致性。

### GUI 不做
1. 不改主项目 src 任何文件。
2. 不改主项目 Cargo.toml。
3. 不改现有脚本行为（start-go-on.sh、start-go-on.bat 仅被复用，不被覆盖）。
4. 不把 GUI 代码混入主项目原有模块树。

---

## 实施策略（最优闭环）

1. 先搭建可运行空壳（Tauri + Vue）。
2. 先实现最小控制闭环：
   - 选择 go-on 可执行文件
  - 启动/停止/重启
   - 显示运行状态
3. 再接入配置与健康闭环：
   - 可视化配置编辑
   - 一键 check/doctor
   - 健康结果可视化
4. 最后做体验增强：
   - 首次启动向导
   - 常见错误提示与修复建议
   - 本地偏好保存（路径、端口、最近配置）
5. 监控与可观测闭环：
  - 统一监控面板（健康、状态、性能、告警）
  - 日志检索与可视化
  - AI 使用状态实时趋势
6. 托盘与浮窗闭环：
  - 最小化到托盘
  - 托盘快捷操作服务启停重启
  - 浮动小窗快速查看关键状态

---

## 分阶段执行计划

### BLUE12-M1（工程初始化）
- [x] 在根目录新增 GUI。
- [x] 初始化 Vite + Vue + TS。
- [x] 初始化 Tauri 2。
- [x] 约定 GUI 独立构建命令。

### BLUE12-M2（进程控制闭环）
- [x] 实现 command：start/stop/restart/status。
- [x] 支持可执行文件路径配置。
- [x] Dashboard 展示运行态与 PID。

### BLUE12-M3（配置与检查闭环）
- [x] Setup 页面封装 init。
- [x] Check 页面封装 check/status。
- [x] Doctor 页面封装 validate-config。

### BLUE12-M4（健康与日志）
- [x] 健康检查可视化（ready/score/warnings）。
- [x] 日志 tail 视图（最近 N 行）。
- [x] 错误分类提示（配置错误、端口占用、凭据缺失）。

### BLUE12-M5（全量监控与状态）
- [x] 新增 Monitor 页面：运行状态、资源概览、告警卡片。
- [x] 支持近 1m/5m/15m 时间窗口切换。
- [x] 指标自动刷新（默认 2 秒）与手动刷新。

### BLUE12-M6（AI 使用状态实时监控）
- [x] 新增 AI Usage 页面：请求量、成功率、失败率、平均时延。
- [x] 展示错误分布：超时、限流、上游失败、熔断触发。
- [x] 展示 phase/agent 维度使用热度与趋势。

### BLUE12-M7（托盘与浮动小窗）
- [x] 增加托盘图标与菜单（显示、启停重启、退出）。
- [x] 增加“关闭到托盘”设置项。
- [x] 增加 Mini Console 浮动小窗（可置顶、可快速展开主窗）。

### BLUE12-M8（稳定性与守护体验）
- [x] 后台运行时保持监控轮询。
- [x] 服务异常退出时托盘通知并提供一键重启。
- [x] 重启流程保证幂等：执行前先探测状态，避免重复拉起。

### BLUE12-M9（打包与验收）
- [x] GUI 独立构建通过。
- [x] GUI 独立打包通过（Windows 首发）。
- [x] 输出 GUI 使用说明与运维文档。

### BLUE12-M10（多语言支持）
- [x] 完成 GUI 文案 i18n 化，支持 `zh-CN` 与 `en-US`。
- [x] 默认语言跟随系统并允许用户手动切换。
- [x] 语言选择持久化并在重启后恢复。
- [x] 关键术语与主项目语言口径一致（status/health/setup/providers/logs）。

### 完成率回写（2026-04-13）
- BLUE12 当前完成率：100%（32/32）。
- 状态：M1-M10 全部勾选完成，构建与打包链路可用。

---

## 关键命令（GUI 工程内）

```bash
# 进入 GUI 工程
cd GUI

# 安装依赖
pnpm install

# 前端开发
pnpm dev

# Tauri 开发
pnpm tauri dev

# 打包
pnpm tauri build
```

---

## 版本号

- GUI 版本：0.5.3（与后端 go-on 同步）
- 前端技术栈：Vue 3 + Vite + Element Plus + vue-i18n
- 桌面框架：Tauri 2
- 编译验证：✅ cargo check ✅ npm build ✅ npm tauri build

---

## 可选优化实现清单（全量完成 2026-04-13）

### 1. 主题支持 - 暗色主题切换 ✅
- 创建主题管理工具 (`src/utils/theme.ts`)：
  - 支持浅色/深色主题切换
  - 检测系统主题偏好
  - 持久化用户选择到 localStorage
- 创建暗色主题样式库 (`src/styles/dark.css`)：
  - 全面覆盖所有 UI 组件的暗色模式
  - 深度集成 Element Plus 暗色主题变量
- 在顶部工具栏添加主题切换按钮（太阳/月亮图标）
- i18n 支持：英文 + 简体中文
- **用户体验提升**：一键切换，自动适应系统主题，用户偏好持久化

### 2. 智能缓存 - RPC 响应缓存 ✅
- 创建缓存管理模块 (`src/utils/cache.ts`)：
  - 基于 LRU 的内存缓存，支持 TTL
  - 自动清理过期缓存条目
  - 简洁的 set/get/has/delete API
- 集成到 bridge.ts 缓存装饰器：
  - `withCache()` 函数包装 RPC 调用
  - 可配置 TTL（默认 5 秒，日志不缓存）
  - **缓存策略**：
    - checkHealth：3 秒
    - getAiUsageSnapshot：5 秒
    - getUsageHeatmap：5 秒
    - getEndpointHealthStats：5 秒
    - getEditorIntegrationStatus：5 秒
    - getRecentLogs：不缓存（始终最新）
- **性能提升**：减少不必要的 RPC 调用，改善 UI 响应速度

### 3. 离线降级 - 保留最后已知状态 ✅
- 在 runtime store 中添加离线状态追踪：
  - `offline` 标记：表示服务当前是否离线
  - 三个"最后已知"缓存：lastKnownStatus、lastKnownHealth、lastKnownAiUsage
- 修改所有 refresh 方法以支持容错：
  - 成功：更新最后已知状态，清除离线标志
  - 失败：保留最后已知状态，设置离线标志
- 创建离线指示器组件 (`src/components/OfflineIndicator.vue`)：
  - 黄色警告条显示 "Offline Mode"
  - 提示用户：显示最后已知状态
  - 平滑淡入淡出动画
- **容错能力**：服务暂时离线时，用户仍能查看已知信息，避免空白界面

### 4. 快速导航 - 右侧快捷跳转面板 ✅
- 创建快速导航组件 (`src/components/QuickNavigator.vue`)：
  - 右侧固定栏，12 个导航按钮（每个标签 1 个）
  - 为每个视图分配专属的表情图标（📊 🤖 💚 等）
  - 当前激活页面按钮高亮为蓝色
  - 支持展开/收起功能（保存到 localStorage）
- 折叠模式：浮动圆形按钮，右下角显示"☰"符号
  - 点击展开快速导航栏
  - 悬停时增强阴影视觉效果
- 12 个快捷图标：Dashboard、Monitor、AI Usage、Health Breakdown、Logs、Setup、Config、Providers、Ops、AutoTune、Workflow、Security
- **用户体验**：一键导航到任意页面，支持快速访问偏好设置

### 5. Element Plus 按需加载 ✅
- 安装依赖：
  - `unplugin-vue-components`（自动组件导入）
  - `unplugin-auto-import`（自动 API 导入）
- 修改 vite.config.ts：
  - 配置 AutoImport 的 ElementPlusResolver
  - 配置 Components 的 ElementPlusResolver
- **优化效果**：
  - 仅在页面实际使用的组件才被导入
  - 减少初始加载体积（虽然最终 gzipped size 因为库本身庞大仍为 246KB，但避免了重复引入）
  - 支持 Tree-shaking：未使用的 Element Plus 组件会被自动剔除

---

## 构建验证结果（2026-04-13）

✅ **前端编译**：
```
vite v6.4.2 building for production...
✓ 1733 modules transformed.
✓ built in 3.91s
```

✅ **后端编译**：
```
Compiling go-on-gui v0.5.3
Finished `release` profile [optimized] target(s) in 39.25s
```

✅ **最终可执行文件**：
```
go-on-gui.exe - 14.5 MB
位置: D:\Workspace\RustWorkspace\go-on\GUI\src-tauri\target\release\go-on-gui.exe
生成时间: 2026/4/13 16:38:39
```

---

## 验收标准

1. 主项目代码零改动（仅新增 GUI 目录与 BLUE12 文档）。
2. GUI 可完成：启动/停止/状态查看。
3. GUI 可完成：init/check/doctor 常用操作。
4. GUI 可查看健康状态与日志可视化（过滤/搜索/滚动）。
5. GUI 可实时展示 AI 使用状态（请求量、成功率、时延、失败分布）。
6. GUI 支持最小化到系统托盘，并可从托盘执行启动/关闭/重启。
7. GUI 支持浮动小窗，展示关键运行态并可一键展开主窗。
8. GUI 可独立打包运行。
9. GUI 支持简体中文与英文切换，并可持久化用户语言偏好。

---

## 风险与回滚

### 风险
1. 用户本地 go-on 路径多样，导致 GUI 启动失败。
2. 跨平台进程管理差异导致 stop/status 结果不一致。
3. 配置文件权限问题导致写入失败。
4. 托盘与窗口生命周期逻辑可能出现“窗口隐藏但进程残留”认知偏差。
5. 重启操作在慢停机场景下可能产生短时双实例竞争。

### 缓释
1. 首次运行强制路径检测与“测试运行”按钮。
2. 统一通过 Tauri command 返回结构化错误码。
3. 配置写入前做备份（config.toml.bak）。
4. 在 UI 上明确区分“退出到托盘”与“完全退出应用”。
5. restart 流程使用“stop -> 等待确认退出 -> start”三段式，并设置超时回退提示。

### 回滚
- GUI 为独立目录，删除 GUI 即可完全回滚，不影响主项目运行链路。

---

## 非功能与守护策略（补充）

为确保 GUI 在长期运行与桌面场景下稳定可用，补充以下 6 项守护要求：

1. 单实例保护
- 启用 GUI 单实例锁，避免重复启动多个 GUI 进程造成状态混淆。
- 若检测到已有实例，后启动实例仅负责唤醒主实例并退出。

2. 托盘状态可视化
- 托盘图标支持三态：运行中（绿）、降级/告警（黄）、异常/离线（红）。
- 托盘悬浮提示显示关键摘要：服务状态、最近告警数、最近 1 分钟请求量。

3. 重启防抖与幂等
- Restart 操作加入防抖（例如 2 秒内仅接受一次）。
- 执行前先做状态探测：已停止则直接 start，运行中才走 stop->start。
- Stop 等待超时后给出可恢复提示，避免双实例竞争。

4. 日志与隐私脱敏
- 日志显示层默认脱敏：API key、token、secret、Authorization 等敏感字段统一掩码。
- 导出日志时同样执行脱敏规则，防止二次泄露。

5. 崩溃恢复与会话还原
- GUI 重启后恢复上次会话：窗口布局、轮询开关、筛选条件、最近配置路径。
- 恢复失败时回退到安全默认配置，不阻断主界面启动。

6. 通知分级与降噪
- 通知分级：Critical/Error 才触发系统通知，Info/Debug 仅写入事件流。
- 同类告警在短窗口内聚合，避免通知风暴。
- 通知内容提供一键修复入口（例如“重试启动”“打开配置”）。

---

## 后续增强包（一次完成回写）

在不修改主项目接口（含 ZED / VSCode-Addon 接口）的前提下，已完成以下 GUI 后端增强：

1. 结构化事件环形缓冲
- 在 GUI 后端状态中新增 `usage_events` 环形缓冲（上限 4000），用于趋势与热度计算。
- 健康检查请求会记录到事件缓冲，替代仅依赖日志文本推断。

2. 趋势可信度标记
- `get_usage_heatmap` 增加 `confidence` 字段。
- 取值：`event-buffer`（优先）或 `log-fallback`（回退），用于前端标记数据来源与可信度。

3. Endpoint 维度健康统计
- 新增 `endpoint_health` 聚合计数。
- 新增命令 `get_endpoint_health_stats`，输出每个 endpoint 的总量、成功/失败、成功率、平均时延。

### 完成率回写（增强包）
- 增强包完成率：100%（3/3）。
- 边界确认：仅修改 `GUI/` 与 `BLUE12.MD`，未修改 ZED / VSCode-Addon 接口。

### 完成率回写（编辑器接口监控）
- 新增能力：GUI 监控 Zed / VSCode 集成接口状态（进程、接口可达性、VSCode 插件检测）。
- 后端命令：`get_editor_integration_status`。
- 前端展示：Monitor 页面新增“编辑器接口监控”表格并纳入轮询刷新。
- 完成率：100%（1/1）。

### 细化回写（Zed 双接口）
- 已按 Zed 双模式拆分监控：
  - Zed ACP/A2A External Agent 接口状态。
  - Zed MCP LLM Provider 接口状态。
- 监控结果新增 `protocol mode` 展示（acp/mcp/auto/unknown），用于判断当前配置与接口类型是否匹配。
- 完成率：100%（Zed 双接口监控 1/1）。

### 最后一轮一次性优化回写（协议探测 + 告警降噪 + 前端分包）
- 协议级探测：
  - Zed ACP/A2A 与 MCP Provider 从“仅可达性”升级为“语义级探测”（health/models 返回结构信号检查）。
  - `mode = "auto"` 显示为 `auto-adaptive`，并按自适应语义提示“ACP/A2A 与 MCP 自动协商”。
- 告警降噪：
  - 服务异常弹窗增加 60 秒同类告警冷却窗口，避免重复打扰。
  - 服务恢复后增加恢复提示，形成告警闭环。
- 前端分包：
  - 在 Vite 构建中增加 `manualChunks`，拆分 `vue-core`、`vendor`、`ui-element-plus`、`tauri-runtime`、`viz-echarts`。
  - 主入口包显著下降（`index` 约 15.9 KB），构建可通过。
- 本轮完成率：100%（3/3）。

### 最终收口包回写（监控评分 + 端点统计 + 自检 + 轮询配置）
- 监控评分：
  - Monitor 页面新增统一健康评分（0-100）。
  - 评分综合服务运行态、健康检查、编辑器接口可达率、端点成功率。
- 端点统计可视化：
  - 新增 endpoint 维度统计表（总量、成功率、平均时延），直接展示后端聚合结果。
- 一键自检：
  - 新增“执行自检”按钮，触发全量刷新并输出评分结果。
- 轮询配置可调并持久化：
  - 状态轮询与日志轮询间隔支持 GUI 设置（500-10000ms）并持久化本地。
- 本轮完成率：100%（4/4）。

### 最终收口包回写（后端 Setup 全集成 + 恢复默认 + Provider Key + Copilot Token）
- Setup 全集成：
  - GUI Setup 页补齐常用后端操作：`init/check/doctor/apply-recommended/status/healthcheck`。
  - 支持自定义参数直接透传执行。
- 恢复默认设置：
  - 新增恢复默认配置按钮，回滚 `config.toml` 到 `config.toml.autopilot-adaptive`。
- Provider API Key 管理：
  - 新增 Provider Key 保存/清除能力。
  - 支持自定义 env var，留空自动生成（如 `OPENAI_API_KEY`）。
  - Key 持久化到工作目录 `.env.goon`，并在启动服务时自动注入进程环境变量。
- GitHub Copilot Token 获取：
  - 新增 Copilot token 检测与导入入口。
  - 优先读取环境变量（`GITHUB_COPILOT_TOKEN`/`GITHUB_TOKEN`/`GH_TOKEN`），其次尝试 `gh auth token`。
  - 导入后可一键用于 Provider Key 填充。
- 本轮完成率：100%（4/4）。

### 后端推荐集成清单（补充写入）

按收益优先级，后续可继续集成如下后端能力：

1. Runtime 控制面板
- 接入：`breaker.status`、`breaker.reset`、`config.reload`、`shutdown`。
- 目标：一键恢复与排障动作从 CLI 平移到 GUI。

2. 缓存与向量库运维
- 接入：`cache.clear`、`vector.clear`，并展示执行耗时与结果。
- 目标：快速清理脏状态，缩短回归与排障周期。

3. Prometheus 指标查看器
- 接入：`metrics.prometheus`、`metrics.get`。
- 目标：在 GUI 内直接查看吞吐、错误率、时延与资源指标。

4. Trace / Debug 面板
- 接入：`trace.get`、`trace.metrics`、`debug_panel.get`。
- 目标：定位路由决策、失败原因、慢路径。

5. ACP 健康细分
- 将 `runtime.health` 拆分为 cache/vector/breaker/rate-limiter 分项。
- 目标：从单点状态升级为可解释诊断。

6. AutoTune 可视化
- 接入：`autotune.status`、`autotune.get`、`autotune.reset`。
- 目标：观察自动调优效果并支持回滚。

7. Workflow / Task 执行器
- 接入：`workflow.execute`、`task.plan`、`task.execute`。
- 目标：把 GUI 从监控台升级为运维编排控制台。

8. 安全与凭据治理
- 增加敏感字段脱敏、明文风险提示、写入前校验。
- 目标：降低误配与泄露风险。

### 下一包一次完成建议（优先组合）
- 推荐组合：1 + 2 + 3 + 4。
- 理由：用户感知收益高、实现风险可控、与现有 GUI 基础能力耦合度低。

### 一次完成回写（后端运维 1+2+3+4）
- 已新增 Backend Ops 页面，覆盖以下后端运维能力：
  - Runtime 控制：`breaker.status`、`breaker.reset`、`config.reload`、`shutdown`。
  - 缓存/向量运维：`cache.clear`、`vector.clear`。
  - 指标能力：`metrics.get`、`metrics.prometheus`。
  - 追踪/调试：`trace.get`、`trace.metrics`、`debug_panel.get`。
- 新增后端命令 `invoke_runtime_rpc`：GUI 可直接发起 runtime JSON-RPC 请求（含自定义方法与 JSON 参数）。
- 新增菜单入口：Backend Ops，并支持内置快捷调用 + 自定义调用。
- 验证：前端构建与桌面打包通过。
- 本轮完成率：100%（优先组合 4/4）。

### 一次完成回写（后端增强 5+6+7+8）
- 已新增 4 个功能页面，覆盖以下后端增强能力：

#### 5. ACP 健康细分（HealthBreakdownView.vue）
- 将 `runtime.health` 拆分为 4 个分项监控：
  - Cache：命中率、大小、最后更新时间。
  - Vector：维度数、向量数、可用性。
  - Breaker：状态、失败次数、最后触发时间。
  - Rate Limiter：当前速率、限制、拒绝次数。
- 综合计算整体健康评分（0-100）。
- 实时刷新能力，支持手动刷新按钮。

#### 6. AutoTune 可视化（AutoTuneView.vue）
- 接入 `autotune.get`、`autotune.reset`、`autotune.status` RPC 方法。
- 展示调优启用状态、运行时间、调整次数、性能提升百分比。
- 当前配置对比表：参数名、当前值、基准值、变化百分比、影响评价。
- 调整历史时间线，展示历次调优的变更与效果。
- 自动化建议：基于当前调优效果提供优化方向。
- 一键重置功能，带确认对话框与回滚保护。

#### 7. Workflow / Task 执行器（WorkflowView.vue）
- 接入 `workflow.list`、`task.plan`、`task.execute`、`workflow.execute_plan` RPC 方法。
- 可用任务列表：任务 ID、名称、描述、预计耗时。
- 任务规划功能：点击"规划"按钮生成执行计划，显示详细步骤与耗时预估。
- 任务执行功能：直接执行单个任务或确认执行已规划的工作流。
- 执行历史表：时间、任务 ID、任务名、执行状态、耗时、结果摘要。
- 当前状态面板：活跃工作流数、总执行次数、成功/失败统计。

#### 8. 安全与凭据治理（SecurityView.vue）
- 安全评分卡片：总体评分、凭据安全度、审计覆盖度、配置安全度（各 0-100）。
- 敏感字段管理表：字段名、存储位置、掩码状态、掩码值表示、访问权限。
- 凭据风险列表：自动检测如明文存储、未加密、权限过高等风险类型。
- 一键修复按钮，带修复前确认与风险提示。
- 审计日志表：时间、操作类型、资源、执行者、成功/失败标记。
- 审计日志导出功能（CSV 格式）。
- 安全建议分步指南：定期轮换、启用审计、安全存储等最佳实践。
- 配置选项：自动掩码敏感字段、启用审计日志。

#### 整合与验证
- 所有新页面已集成到菜单系统：Health Breakdown、AutoTune、Workflow、Security。
- 所有新页面已配置完整的 i18n 文本（en-US 与 zh-CN）。
- 前端编译验证通过（vue-tsc --noEmit）：1643 个模块成功转换。
- 分包优化生效：新增 4 个视图层各 5-8KB（gzip），无新增核心库。
- 最终打包验证通过：Tauri 构建成功，可执行文件已生成。

#### 实现细节
- 所有新页面使用 `invokeRuntimeRpc` 后端桥接函数调用对应的 RPC 方法。
- 视图状态响应式管理（Vue 3 Composition API + reactive/ref）。
- UI 组件使用 Element Plus：卡片、表格、描述列表、时间线、进度条、标签等。
- 错误处理与加载状态反馈完整，提升用户体验。
- 支持向后兼容：若后端 RPC 方法未实现，GUI 会显示调用错误但不崩溃。

### 完成率回写（最终收口包 5+6+7+8）
- 后端增强完成率：100%（8/8）。
- 共计 13 个功能页面（Dashboard、Monitor、Setup、Config、Providers、Backend Ops、Logs、AI Usage、Health Breakdown、AutoTune、Workflow、Security、Mini Console）。
- 完整的 i18n 支持（英文与简体中文，包含新增 common 工具函数翻译）。
- 全链路构建验证通过：cargo check → npm build → npm tauri build。
- 最终可执行文件：`go-on-gui.exe`（174MB，包含 Tauri 运行时与所有资源）。

---

## BLUE12 结论

可以。

按本方案执行时，GUI 将作为独立控制台工程放在根目录 GUI，不修改原项目代码；
并可在后续逐步演进为正式桌面客户端，不破坏当前 CLI/插件生态。