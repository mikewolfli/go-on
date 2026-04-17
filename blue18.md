# BLUE18 — 三端深度扫描潜在问题清单

更新时间：2026-04-17

本文基于 backend + vscode-addon + GUI 的深度静态扫描与链路复核，聚焦“潜在行为风险”和“发布后可观测性风险”。

已确认基线：
- backend: 当前扫描未发现新增编译错误或 warning。
- vscode-addon: `npm --prefix vscode-addon run check` 口径下无新增错误信号。
- GUI: `npm --prefix GUI run build` 口径下无新增错误信号。

---

## 进度回写（已更新）

- 总体完成率：`75%`（4 项中已完成 3 项，剩余 1 项）
- 本轮新增产出：后端 5 个入口主链路一致性收口 + GUI RPC error 显式抛错 + 全链路复验通过

| ID | 优先级 | 端 | 状态 | 说明 |
|---|---|---|---|---|
| B18-S1 | P1 | backend | ✅ 已完成 | imported skill 返回新增 `executed: false` 与 `code: NOT_IMPLEMENTED_EXECUTOR`，避免误判为真实执行 |
| B18-S2 | P1 | backend | ✅ 已完成 | skills 远程导入下载新增 connect/request timeout 与 2MiB 响应体上限 |
| B18-S3 | P1 | vscode-addon | 待处理 | runtime 自动下载未做 checksum/signature 校验，存在供应链完整性风险 |
| B18-S4 | P2 | GUI | ✅ 已完成 | RPC 解包新增 `error` 检测并抛错，消除静默失败 |

### 本次复验结果（2026-04-17）

- backend：`cargo test -q` 通过（251 + 71 + 6 + 17 全部通过）
- vscode-addon：`npm --prefix vscode-addon run check` 通过（compile + lint）
- GUI：`npm --prefix GUI run build` 与 `npm --prefix GUI run test:contract` 均通过

### 后端 5 入口主链路一致性（已完成）

- 覆盖入口：`skill.import` / `skill.enable` / `skill.disable` / `skill.list_imported` / `skill.remove`
- 一致性收口：
  - 成功返回统一携带 `ok` + `action`（并带 `name/skill/skills` 等对应字段）
  - 参数错误统一走 `-32602`（包括 `name` 缺失与 remove 未命中）
  - `skill.list_imported` 新增 `total/enabled/disabled` 统计字段

---

## B18-S1 — imported skill 执行语义与返回语义不一致（backend）

### 证据

- 在 `mcp.tools.call` 路径中，当命中 imported skill 时，当前实现返回：`ok: true` + `mode: imported_manifest` + 原始入参回传，且 `note` 明确说明“尚未启用真正执行器”。
- 位置：`src/acp/impl/request.rs`（`execute_mcp_tool_call` 分支，约 `mode = imported_manifest` 相关代码段）。

### 风险

- 客户端可能将 `ok: true` 解释为“skill 已成功执行”，但实际仅是元数据回传。
- 该语义偏差会导致三端行为不一致：backend 返回“看似成功”，addon/GUI 侧难以区分“已执行”与“仅回显”。

### 建议修复

1. 将该分支返回调整为明确可判别语义：
   - 方案 A：`ok: false` + `code: NOT_IMPLEMENTED_EXECUTOR`。
   - 方案 B：保留 `ok: true`，但新增强约束字段 `executed: false`，并在协议中声明客户端必须判定该字段。
2. 同步更新 addon 与 GUI 的响应解析逻辑，避免把 passthrough 当成真实执行成功。

### 完成标准

- `mcp.tools.call` 命中 imported skill 时，返回结构可被三端稳定判别为“未执行”。
- 三端 UI/命令行为一致（不会显示“执行成功”误导信息）。

---

## B18-S2 — skills 导入下载缺少 timeout/size guard（backend）

### 证据

- `download_bytes` 使用 `reqwest::get(url)` 直接拉取并 `.bytes()` 全量读入，未设置明确请求超时、读取超时、最大内容长度上限。
- 位置：`src/orchestration/skill_import.rs`（`download_bytes`）。

### 风险

- 在上游响应缓慢或异常大文件场景，可能造成：
  - 导入流程长时间阻塞；
  - 内存占用异常增长（全量读入）。
- 若未来开启远程 source，风险会放大到运行稳定性层面。

### 建议修复

1. 使用 `reqwest::Client` 并配置连接/请求超时（如 10s/30s，按策略可配）。
2. 对响应体增加上限（例如 manifest 最大 1-2 MiB）。
3. 超限与超时写入审计字段，便于发布后追踪。

### 完成标准

- 超时可控、体积可控、错误可观测（日志和审计均可定位）。
- 回归测试覆盖：慢响应、超大响应、正常响应三类场景。

---

## B18-S3 — runtime 自动下载缺少完整性校验（vscode-addon）

### 证据

- addon 在 `autoDownloadBinary` 打开时会从 GitHub release 下载归档并解压使用。
- 当前流程仅校验 HTTP 状态、解压结果和可执行权限，未校验 checksum/signature。
- 位置：`vscode-addon/src/runtimeBinaryService.ts`（`downloadFile` / `ensureGoOnBinary`）。

### 风险

- 供应链完整性风险：即使来源仓库可信，也建议对下载产物做 hash 或签名校验，避免中间环节污染导致运行未知二进制。

### 建议修复

1. 在 release 产物中发布校验文件（如 `.sha256`）。
2. addon 下载后先验 hash，再解压启用。
3. 校验失败时强制转入手动选择路径，并提示风险。

### 完成标准

- 自动下载链路默认执行完整性校验。
- 校验失败不会启动下载得到的二进制。

---

## B18-S4 — GUI RPC 解包未显式处理 error（GUI）

### 证据

- `unwrapResult` 仅优先读取 `result` 字段；若无 `result`，直接返回原 payload（或 `{}`），未对 JSON-RPC `error` 字段进行抛错。
- 位置：`GUI/src/services/rpcService.ts`（`unwrapResult` / `callRpcJson`）。

### 风险

- 后端返回错误时，GUI 调用方可能拿到“结构上可访问但语义失败”的对象，形成“静默失败”或错误提示不一致。

### 建议修复

1. 在 `callRpcJson` 增加标准 JSON-RPC error 分支：检测到 `error` 即抛异常。
2. UI 层统一错误提示策略，避免把错误态当成空数据态。

### 完成标准

- GUI 对 RPC 错误具备一致抛错与提示行为。
- contract smoke 增加至少 1 个错误路径断言。

---

## 三端对齐执行建议（按批次）

1. 第 1 批（P1）：B18-S1 + B18-S2 + B18-S3 一次打包收口。
2. 第 2 批（P2）：B18-S4 与 GUI 错误态交互一并收口。
3. 每批完成后统一执行：backend / addon / GUI 全链路复验，要求无新增 warning。

建议复验命令：
- backend: `cargo test -q`
- addon: `npm --prefix vscode-addon run check`
- GUI: `npm --prefix GUI run build && npm --prefix GUI run test:contract`

---

## 协议一致性推荐落地步骤（新增）

目标：采用“语义一致、接口可异形”策略，不强制 ACP/MCP 外形完全一致，但保证能力、结果、错误和可观测数据一致。

### 目标层级

1. L1 传输可用：四模式都可稳定收发。
2. L2 语义一致：同能力在四模式下返回同等语义与错误分类（推荐目标）。
3. L3 形态一致：路径和字段完全同形（不作为本项目当前目标）。

### 实施批次（按优先级）

1. 批次 A：能力矩阵冻结（P1）
  - 建立 ACP stdio / ACP http / MCP stdio / MCP http 的能力矩阵（chat、tools、skill 管理、health、metrics、governance）。
  - 明确每项能力的 owner 与协议映射关系，禁止“隐式支持”。
  - 验收：矩阵入库并在 PR 中强制更新。

2. 批次 B：统一结果语义（P1）
  - 统一成功字段：`ok`、`action`、`name`、`executed` 的语义约束。
  - 统一失败语义：参数错误、权限错误、未实现、内部错误四级分类与错误码映射。
  - 验收：四模式同能力调用时，结果分类一致、客户端可稳定判别。

3. 批次 C：统一错误契约与文档（P1）
  - 发布 ACP↔MCP 错误码映射表（例如参数错误、unknown method、not implemented executor）。
  - 在 addon/GUI 侧统一展示逻辑，避免不同协议下“同错异显”。
  - 验收：同类错误在三端展示一致，且可追溯到同一 code。

4. 批次 D：统一可观测性（P2）
  - 审计日志统一字段：`protocol`、`action`、`resource`、`status`、`reason`。
  - 指标统一标签：请求总数、错误分类、耗时分位、协议维度。
  - 验收：同一场景可跨协议对齐查询和对比。

5. 批次 E：跨协议一致性测试门禁（P1）
  - 增加协议参数化测试：同一测试向四模式回放，断言能力可用性与语义一致性。
  - 对未覆盖能力标记为 TODO，不允许静默缺失。
  - 验收：CI 中新增 consistency suite，作为 release gate 必选项。

### 建议的代码落点

1. 协议模式分发：`src/main.rs`
2. ACP 请求总线：`src/acp/impl/request.rs`
3. ACP HTTP 路由层：`src/acp/impl/runtime.rs`
4. MCP 语义处理层：`src/mcp/handlers.rs`
5. MCP 传输层：`src/protocol/mcp_server.rs`

### 最小交付顺序（建议）

1. 先做批次 A + B：先冻结能力，再冻结返回语义。
2. 再做批次 E：用参数化测试锁定回归。
3. 最后做批次 C + D：完善错误体验与可观测收口。

---

## 结论

本轮扫描未发现立即阻断发布的“编译级”问题，但存在 3 项 P1 潜在风险（执行语义、下载稳态、供应链完整性）与 1 项 P2 交互一致性问题。建议按上述批次尽快收口，并保持“三端对齐 + 无 warning”验收口径。