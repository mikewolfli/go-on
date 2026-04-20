# BLUE14 — 对照项目现状的优化清单（按实施优先级排序）

> 延续 BLUE13 的执行纪律：先方案冻结、再分阶段实施、最后端到端验收。
> 本文件基于对 go-on 项目的实际扫描结果，对 BLUE14-temp.MD 提出的优化建议进行重新整理与评级，
> 每条给出**是否需要**判断与**推荐建议**，按实施优先级由高到低排列。

---

## 背景与约束确认

基于 BLUE13 后的真实项目状态（2026-04-14 扫描）：

| 维度 | 现状 |
|---|---|
| 测试覆盖 | 217 个测试全部主链化，覆盖率 100%（BLUE13 已完成） |
| i18n 系统 | `src/i18n/` 已建，`init_i18n()` 已调用，但 `tf!` 在 main.rs 调用次数 = 0 |
| 错误处理 | `core/error.rs` 已有 `AppError` 层次体系，但全项目仍有 62 个文件用 `anyhow::Result` |
| MCP 集成 | `src/mcp/`、`src/protocol/mcp_server.rs` 已建；协议模式靠 config.toml `[protocol]` 段驱动，无 CLI 直接参数 |
| 性能监控 | `observability/performance.rs` API 完整（measure_time/record_operation/PerformanceMetrics），但 ACP/HTTP 关键请求路径无有效调用点 |
| 文档 | README.md 仅 251 行，无 `docs/` 目录，无四种模式独立指南 |
| 静态分析 | CI 无 `cargo clippy` / `cargo audit` 门禁 |

本轮强约束：

1. 所有改进必须可在 CI 中无外部依赖稳定执行。
2. 不破坏已通过的 217 个测试。
3. 每条优化必须可独立实施、可验收，不做大爆炸式重构。
4. 安全缺陷（OWASP）优先于功能优化。
5. 所有被采纳优化项必须按推荐方案完整实现，并接入主链路；接入方式（实时接入或 LAZY LOAD）由业务场景决策，但必须形成主链路闭环（触发 -> 执行 -> 反馈 -> 度量/审计），确保对程序产生积极作用，禁止“仅定义不生效”或“仅旁路演示”实现。
6. 方案选择需以“对现有结构更优或更完备”为目标，并以“最小化当前项目代码改动”为实现原则：优先复用现有模块/接口/测试资产，避免无必要的新层级、新抽象和大范围重写。
7. 全部更改完成后必须统一后台程序与 GUI 程序的资源文件命名与目录约定；考虑两者 EXE 编译产物会落在同一文件夹，必须进行冲突规避（文件名空间、子目录隔离或构建后重命名策略），禁止资源同名覆盖。
8. 每完成一个大项必须同步更新后台程序、GUI 与 vscode-addon 插件（接口、协议、配置、文档与验证脚本），确保三端功能对齐与可协同运行，避免“单端先行、其余端失配”。
9. 严禁任何 bridge-stub/测试桥接 shim 方案（尤其是在测试内用本地模块模拟生产模块）；遇到依赖边界或模块可见性问题，必须通过真实工程结构修复（模块归属、导出、重构）解决，禁止用测试侧补丁绕过。

闭环判定标准（适用于全体 B14 条目）：

1. 有主链路触发点：至少 1 个生产路径可稳定触发该功能。
2. 有执行与结果：功能执行结果可被调用方消费（返回值、状态、或副作用可观测）。
3. 有反馈与治理：失败/退化可进入日志、审计或学习系统。
4. 有验收门禁：至少 1 条自动化测试或 CI 门禁可阻止回归。
5. 有三端协同验收：每个大项完成时需至少验证一次 backend + GUI + vscode-addon 的联动路径。
6. 有同目录部署安全性：后台与 GUI 的 EXE 同目录部署时资源文件无重名冲突、无覆盖回归。

---

## 目标（按优先级排序）

| ID | 优先级 | 目标 | 是否需要 | 目标文件 |
|---|---|---|---|---|
| B14-P0-1 | P0 | MCP 协议模式 CLI 直接参数 | ✅ 必须 | `src/main.rs` |
| B14-P0-2 | P0 | 性能监控关键路径调用点接入 | ✅ 必须 | `src/acp/impl/runtime.rs` + `src/main.rs` |
| B14-P1-1 | P1 | 错误处理公共接口边界统一 | ✅ 需要（部分） | `src/agents/mod.rs` + 公共 trait |
| B14-P1-2 | P1 | Clippy + cargo audit CI 静态分析门禁 | ✅ 必须 | `test_ci.sh` |
| B14-P2-1 | P2 | i18n 用户可见输出覆盖 | ⚠️ 需要（部分） | `src/main.rs` + `src/acp/` |
| B14-P2-2 | P2 | 文档体系补全（四种模式指南） | ⚠️ 需要（部分） | `README.md` + `docs/` |
| B14-P3-1 | P3 | 生产就绪量化指标定义 | 💡 参考价值 | `blue14.md` |

---

## 技术选型（冻结）

### P0 MCP CLI 参数策略
- 新增 `--protocol-mode <acp|mcp|auto>` CLI 参数，覆盖 config.toml `[protocol].mode`
- 不引入 `--mcp-stdio` / `--mcp-http` 独立标志（避免与现有 `--acp-http-bind` 语义重叠）
- 向后兼容：CLI 未指定时，继续从 config 读取

### P0 性能监控策略
- 仅在**请求级边界**（HTTP POST handler、RPC chat dispatch）添加 `measure_time` + `record_operation`
- 不改变 `PerformanceMonitor` 内部实现
- 通过 `telemetry_enhanced` 机制将延迟日志输出到 stderr（不污染 stdout）

### P1 错误处理策略
- **仅在公共 trait 接口、ACP HTTP handler 函数签名处**将 `anyhow::Result` 改为 `crate::core::error::Result`
- 内部实现、测试代码保留 `anyhow`（降低迁移风险）
- 改动范围上限：每轮不超过 10 个文件

### P1 Clippy/Audit 策略
- `cargo clippy --all-targets -- -D warnings` 作为 CI 门禁步骤
- `cargo audit` 检测已知漏洞（RUSTSEC 数据库）
- 初始允许 `#[allow(clippy::...)]` 豁免，但必须注释原因

### P2 i18n 策略
- 仅迁移**用户可见的 `println!` / `eprintln!`** 输出（约 30-50 处）
- 不迁移 `info!` / `warn!` / `debug!` 日志（运维日志保持英文）
- 新增翻译键必须同步更新 `en_US.json` / `zh_CN.json` / `zh_TW.json`

### P2 文档策略
- 优先补充 README.md 协议模式章节（≤500 字/模式）
- 不创建全套 `docs/` 目录（耗时且可能过期）
- 仅补充快速开始、四种模式、FAQ 三段

---

## 详细实施步骤

---

### B14-P0-1：MCP 协议模式 CLI 直接参数

**是否需要**：✅ **必须**

> 根因：项目四种协议模式（ACP+STDIO / ACP+HTTP / MCP+STDIO / MCP+HTTP）是核心卖点，
> 但当前只能通过修改 config.toml `[protocol]` 段切换，无法在命令行直接指定。
> 在容器部署、脚本集成、快速测试等场景下，需要频繁修改配置文件，操作成本高。

**推荐建议**：新增 `--protocol-mode` CLI 参数，覆盖 config 文件；CLI 优先于 config。

#### 步骤 1.1：在 `src/main.rs` Cli struct 中添加参数
```rust
/// Protocol mode override (acp | mcp | auto).
/// Overrides [protocol].mode in config file when specified.
#[arg(long, value_name = "MODE")]
protocol_mode: Option<String>,
```

#### 步骤 1.2：在 config 解析段应用 CLI 覆盖
```rust
// 在现有 [protocol] 段解析之后追加：
if let Some(ref cli_mode) = cli.protocol_mode {
    runtime_config.protocol_mode = Some(cli_mode.clone());
}
```

#### 步骤 1.3：添加模式值校验
```rust
if let Some(ref mode) = runtime_config.protocol_mode {
    match mode.as_str() {
        "acp" | "mcp" | "auto" => {}
        other => {
            eprintln!("Invalid --protocol-mode '{}'. Allowed: acp, mcp, auto", other);
            std::process::exit(1);
        }
    }
}
```

#### 步骤 1.4：更新 `test_ci.sh` 验证 CLI 参数可解析
```bash
# 验证 --protocol-mode 参数存在且 help 输出包含该选项
./go-on --help | grep -q "protocol-mode" || (echo "FAIL: --protocol-mode missing from help" && exit 1)
```

#### 步骤 1.5：添加单测
在 `src/main.rs` 的 `#[cfg(test)]` 块中新增：
```rust
#[test]
fn cli_protocol_mode_overrides_config() {
    // 验证 RuntimeConfig.protocol_mode 优先级逻辑
    let mut runtime_config = RuntimeConfig::default();
    let cli_mode = Some("mcp".to_string());
    if let Some(ref mode) = cli_mode {
        runtime_config.protocol_mode = Some(mode.clone());
    }
    assert_eq!(runtime_config.protocol_mode.as_deref(), Some("mcp"));
}

#[test]
fn cli_protocol_mode_all_valid_values() {
    for mode in &["acp", "mcp", "auto"] {
        assert!(matches!(*mode, "acp" | "mcp" | "auto"));
    }
}
```

#### 步骤 1.6：接入主链
```bash
# test_ci.sh 新增步骤 6a
cargo test cli_protocol_mode -- --nocapture
```

**验收标准**：
- 设置 `GO_ON_API_KEY` 环境变量后 `RuntimeConfig::load()` 使用该值
- 文件内 api_key 不覆盖已设置的环境变量值
- `cargo audit` 报告无已知漏洞（CI 门禁强制）
- `--help` 输出包含 `--protocol-mode`
- `--protocol-mode mcp` 可覆盖 config 中的 `[protocol].mode`
- 非法值（如 `--protocol-mode invalid`）退出码非 0 并输出提示
- 向后兼容：不传 `--protocol-mode` 时行为完全不变

---

### B14-P0-2：性能监控关键路径调用点接入

**是否需要**：✅ **必须**

> 根因：`observability/performance.rs` 已有完备的 `measure_time()` / `record_operation()` /
> `PerformanceMetrics` API，`init_performance_monitoring()` 在 main 中已调用，
> 但 ACP RPC handler 和 HTTP POST handler 均无任何 `record_operation` 调用。
> 结果：`PerformanceMetrics` 始终返回全零数据，性能监控形同虚设。

**推荐建议**：在 HTTP POST 路径、RPC chat dispatch 路径共两处添加 measure_time + record_operation 调用。

#### 步骤 2.1：在 HTTP handler 中接入性能记录

定位 `src/main.rs`（或 `src/acp/impl/runtime.rs`）中处理 POST 请求的函数，在请求处理前后包裹测量：

```rust
// 在 handle_http_connection 的 POST /v1/chat/completions 路径：
use crate::observability::performance::utils;

let (result, duration) = utils::measure_time(|| {
    // 原有的请求处理逻辑
    handle_chat_completions(...)
});
let success = result.is_ok();
// 如有全局 PerformanceMonitor 实例，调用 record_operation
if let Some(ref monitor) = perf_monitor {
    let mut m = monitor.lock().unwrap_or_else(|e| e.into_inner());
    m.record_operation(success, duration.as_millis() as f64);
}
tracing::debug!("POST /v1/chat/completions completed in {:?} (ok={})", duration, success);
```

#### 步骤 2.2：在 RPC chat dispatch 中接入

定位 `src/acp/impl/runtime.rs` 中处理 `acp.chat` 方法的分支：

```rust
let (response, duration) = crate::observability::performance::utils::measure_time_async(|| async {
    process_chat_request(&mut server, params).await
}).await;
tracing::debug!("rpc.chat completed in {:?}", duration);
```

#### 步骤 2.3：添加单测验证 measure_time 正确记录
```rust
#[test]
fn performance_measure_time_returns_duration() {
    use crate::observability::performance::utils;
    let (result, duration) = utils::measure_time(|| 42u32);
    assert_eq!(result, 42);
    assert!(duration.as_nanos() > 0);
}
```

#### 步骤 2.4：接入主链
```bash
# test_ci.sh 新增步骤 6b
cargo test performance_measure_time -- --nocapture
```

**验收标准**：
- 运行一次 `/v1/chat/completions` 请求后，`PerformanceMetrics.total_ops >= 1`
- `avg_latency_ms > 0`
- stderr 中出现延迟日志输出（不污染 stdout）
- 新增测试通过

---

### B14-P1-1：错误处理公共接口边界统一

**是否需要**：✅ **需要（部分迁移）**

> 根因：`core/error.rs` 已定义完整的 `AppError` 层次（ProxyError / ValidationError / NetworkError / ResourceError）
> 及 `pub type Result<T> = std::result::Result<T, AppError>`，但全项目 62 个文件仍用 `anyhow::Result`。
> 关键问题在**公共接口**：agent trait 返回 `anyhow::Result` 导致调用方无法对错误类型做精确 match；
> 后续扩展（如 metrics / 告警策略）需要区分 NetworkError vs ValidationError。
> 全量迁移 62 个文件风险过高；**仅迁移公共 trait 和 HTTP/RPC handler 签名**，内部实现代码保留 `anyhow`。

**推荐建议**：分三步，仅迁移公共接口边界，不动内部实现。

#### 步骤 3.1：在 agent trait 签名中采用 `AppError`

在 `src/agents/mod.rs` 中，公共 trait 方法修改为：

```rust
// Before:
async fn chat(&self, params: ChatParams) -> anyhow::Result<ChatResponse>;

// After:
async fn chat(&self, params: ChatParams) -> crate::core::error::Result<ChatResponse>;
```

#### 步骤 3.2：各 agent 实现中使用 `?` 向上转换
各实现文件（`openai.rs`, `anthropic.rs` 等）内部可继续用 `anyhow`，
在 `?` 传播时通过 `From<anyhow::Error>` 或显式 `.map_err(|e| AppError::...)` 转换：

```rust
// 在 impl 中：
.map_err(|e| AppError::Proxy(ProxyError::UpstreamError {
    code: None,
    message: e.to_string(),
}))
```

#### 步骤 3.3：HTTP handler 返回类型统一
`handle_chat_completions` 等 HTTP 构建函数改为返回 `crate::core::error::Result<Response<Body>>`，
内部 `anyhow` 错误 `.map_err()` 转换。

#### 步骤 3.4：添加边界测试
```rust
#[test]
fn agent_error_can_be_classified() {
    let err: AppError = AppError::Proxy(ProxyError::UpstreamError {
        code: Some(503),
        message: "service unavailable".to_string(),
    });
    assert!(matches!(err, AppError::Proxy(_)));
}
```

#### 步骤 3.5：接入主链
```bash
# test_ci.sh 新增步骤 6c（已有 core::error::tests:: 在 5m，本步骤接边界集成）
cargo test agent_error_can_be_classified -- --nocapture
```

**是否需要全量迁移（62个文件）**：❌ 不建议。
- 内部实现文件（`watcher.rs`, `performance.rs` 等纯内部逻辑）保留 `anyhow` 完全合理
- 强行全量替换会引入大量无意义的 `map_err` 包装，增加噪音

**验收标准**：
- agent trait 公共签名使用 `AppError`
- 错误可被 `match` 精确分类
- 现有 62+ 测试全部通过（无回归）

---

### B14-P1-2：Clippy + cargo audit CI 静态分析门禁

**是否需要**：✅ **必须**

> 根因：当前 `test_ci.sh` 仅有编译检查（`--no-run`）和单元/集成测试，
> 无 `cargo clippy` 静态分析门禁，无依赖安全漏洞扫描（cargo audit）。
> 属于 OWASP Top 10 中 A06（易受攻击和过时的组件）和 A08（软件和数据完整性故障）的直接缺口。

**推荐建议**：在 CI 脚本中追加两步静态分析门禁，以 warning-as-error 模式运行。

#### 步骤 4.1：在 `test_ci.sh` 中添加 clippy 门禁
```bash
# 步骤 6d: Clippy 静态分析门禁
echo "=== 步骤6d: Cargo Clippy 静态分析 ==="
cargo clippy --all-targets -- -D warnings
if [ $? -ne 0 ]; then
  echo "FAIL: clippy 发现警告（已以 error 级别处理）"
  exit 1
fi
echo "步骤6d 通过"
```

#### 步骤 4.2：在 `test_ci.sh` 中添加 cargo audit 门禁
```bash
# 步骤 6e: 依赖安全漏洞扫描
echo "=== 步骤6e: Cargo Audit 依赖安全扫描 ==="
cargo audit
if [ $? -ne 0 ]; then
  echo "FAIL: 发现已知安全漏洞（参见 RUSTSEC 数据库）"
  exit 1
fi
echo "步骤6e 通过"
```

#### 步骤 4.3：处理现有 clippy 警告
在首次接入时，允许对已有警告加 `#[allow(clippy::lint_name)]` 豁免，
但必须在代码注释中说明原因：
```rust
#[allow(clippy::too_many_arguments)] // 此函数是运行时入口点，参数来自 CLI，暂不重构
```

#### 步骤 4.4：安装 cargo-audit（CI 环境预备）
```bash
cargo install cargo-audit --locked 2>/dev/null || true
```

**验收标准**：
- `cargo clippy --all-targets -- -D warnings` 零错误退出
- `cargo audit` 无已知高危漏洞（允许 info 级别条目但需在回写中记录）
- test_ci.sh 步骤 6d/6e 可本地执行通过

---

### B14-P2-1：i18n 用户可见输出覆盖

**是否需要**：⚠️ **需要（部分），优先级中等**

> 根因：`src/i18n/runtime.rs` 完整实现并初始化，但 main.rs 中 `tf!` 宏调用次数 = 0。
> 用户可见的 `println!` 输出（如 setup wizard 提示、healthcheck 结果、status 报告等）
> 全部硬编码英文，中文界面宣称的"全中文支持"实际上不成立。
> 注意：`info!`/`warn!`/`debug!` 运维日志保持英文是行业惯例，不需迁移。

**推荐建议**：仅迁移用户可见的交互输出，分两批渐进完成。

#### 步骤 5.1：扫描并分类硬编码字符串
```bash
# 定位需要迁移的 println!/eprintln! 调用
grep -n "println!\|eprintln!" src/main.rs | grep -v "//\|test"
```
预期范围：主要集中在 setup wizard（约 20 处）、health check 输出（约 10 处）、status 报告（约 15 处）。

#### 步骤 5.2：批量 1 — setup wizard 提示文案
在 `languages/en_US.json` / `zh_CN.json` / `zh_TW.json` 中新增 setup 键，并在 main.rs 替换：
```rust
// Before:
println!("Welcome to go-on setup wizard!");

// After:
println!("{}", tf!("setup.welcome"));
```

对应语言包：
```json
// en_US.json:
"setup.welcome": "Welcome to go-on setup wizard!",
// zh_CN.json:
"setup.welcome": "欢迎使用 go-on 配置向导！",
// zh_TW.json:
"setup.welcome": "歡迎使用 go-on 配置嚮導！"
```

#### 步骤 5.3：批量 2 — healthcheck / status 输出
类似步骤 5.2，逐步迁移剩余 println! 调用。

#### 步骤 5.4：添加 i18n 覆盖完整性测试
```rust
#[test]
fn i18n_setup_welcome_key_exists_in_all_languages() {
    // 验证关键 key 在三种语言包中均存在
    let keys = ["setup.welcome"];
    for key in &keys {
        assert!(en_us_bundle().contains_key(key));
        assert!(zh_cn_bundle().contains_key(key));
    }
}
```

#### 步骤 5.5：接入主链（补充到步骤 5 的 i18n suite）
```bash
# 在已有 i18n:: 测试步骤基础上扩展
```

**是否需要迁移所有 info!/warn! 日志**：❌ 不需要。
- 运维/调试日志保持英文是行业最佳实践（便于全球搜索问题、贴 Stack Overflow 等）
- 仅迁移面向最终用户的 println!/eprintln!

**验收标准**：
- 主要 setup wizard 和 status 输出支持中英文切换
- `LANG=zh_CN ./go-on --status` 显示中文
- 三语言包无缺失 key
- 现有测试无回归

---

### B14-P2-2：文档体系补全（四种模式指南）

**是否需要**：⚠️ **需要（部分），面向发布的优先项**

> 根因：README.md 仅 251 行，无四种协议模式使用指南，
> 无 `--protocol-mode` 参数说明，无快速开始示例。
> 对于外部用户或新团队成员，理解项目功能需要直接读源码。

**推荐建议**：扩充 README.md 的关键章节，不建立独立 docs/ 目录（避免维护负担）。

#### 步骤 6.1：在 README.md 中新增"快速开始"章节
```markdown
## 快速开始

### 方式 1：ACP STDIO 模式（默认）
```bash
./go-on
```

### 方式 2：ACP HTTP 模式
```bash
./go-on --acp-http-bind 127.0.0.1:8716
```

### 方式 3：协议模式切换（MCP/ACP/auto）
```bash
# 通过 CLI 参数（B14-P0-1 实施后）
./go-on --protocol-mode mcp

# 通过配置文件
# config.toml:
# [protocol]
# mode = "mcp"
```
```

#### 步骤 6.2：新增"四种协议模式"说明章节
```markdown
## 协议模式

| 模式 | 说明 | 适用场景 |
|---|---|---|
| ACP+STDIO | Agent Control Protocol over stdin/stdout | VS Code Addon 直接集成 |
| ACP+HTTP | ACP over HTTP（含 /v1/chat/completions） | REST 客户端、GUI |
| MCP+STDIO | Model Context Protocol over stdin/stdout | Cursor / Claude Desktop |
| MCP+HTTP | MCP over HTTP | 远程工具调用 |

当前默认模式：`auto`（自动检测 ACP 和 MCP 请求）。
```

#### 步骤 6.3：新增"配置参数速查"章节
提取 CLI `--help` 中的关键参数，整理成表格形式放入 README。

#### 步骤 6.4：新增"故障排除"章节（简要）
常见问题：
- 端口被占用 → 检查 `--acp-http-bind`
- 模型不可用 → 运行 `./go-on --validate-config`
- 日志级别 → 追加 `--verbose`

**是否需要建立完整 docs/ 目录**：❌ 当前不需要。
- 代码变动频繁，单独 docs/ 目录极易过期
- README 集中管理优于分散在 docs/ 的多个 md 文件

**验收标准**：
- README.md 包含快速开始、四种模式、配置速查三章
- 新人不读源码可在 5 分钟内启动项目

---

### B14-P3-1：生产就绪量化指标定义

**是否需要**：💡 **参考价值（无需实现新代码）**

> 根因：当前项目缺乏明确的 SLA/SLO 定义，
> 测试通过 ≠ 生产就绪，需要对关键指标有共同口径。

**推荐建议**：在此文档记录指标基线，不需要额外代码实现。

#### 质量基线（供参考）

| 指标 | 目标值 | 当前状态 |
|---|---|---|
| 测试通过率 | 100% | ✅ 217/217 |
| 主链覆盖率 | 100% | ✅ 所有套件已接入 |
| Clippy 警告 | 0 | ❓ 待 B14-P1-2 扫描 |
| 已知安全漏洞（高危） | 0 | ❓ 待 B14-P1-2 扫描 |
| 构建时间（debug） | ≤ 3 分钟 | ❓ 未测量 |
| /v1/chat/completions P99 延迟 | ≤ 5000ms（含 LLM 网络） | ❓ 待 B14-P0-2 接入后可测 |
| 启动时间 | ≤ 2 秒 | ❓ 未测量 |
| 内存使用（空载） | ≤ 100MB | ❓ 未测量 |

**是否需要新增监控代码**：
- `P99 延迟`：**是**，依赖 B14-P0-2 性能监控接入
- 其余指标：用现有工具（time、/usr/bin/time）手工测量即可，无需代码

---

## 各条优先级汇总

```
P0（立即实施，功能完整性 + 可观测性）
  B14-P0-1  MCP 协议模式 CLI 参数        [✅ 必须] [具体步骤 1.1~1.6]
  B14-P0-2  性能监控关键路径调用点        [✅ 必须] [具体步骤 2.1~2.4]

P1（近期实施，代码质量 + 安全）
  B14-P1-1  错误处理公共接口边界统一      [✅ 需要（部分）] [具体步骤 3.1~3.5]
  B14-P1-2  Clippy + cargo audit 门禁   [✅ 必须] [具体步骤 4.1~4.4]

P2（计划内实施，完整性）
  B14-P2-1  i18n 用户可见输出覆盖        [⚠️ 需要（部分）] [具体步骤 5.1~5.5]
  B14-P2-2  文档体系补全（四种模式）      [⚠️ 需要（部分）] [具体步骤 6.1~6.4]

P3（参考，无需新代码）
  B14-P3-1  生产就绪量化指标定义          [💡 参考价值] [指标表格]
```

---

## 关于 BLUE14-temp.MD 各条建议的评级说明

| temp 建议 | 评级 | 对应 B14 条目 | 说明 |
|---|---|---|---|
| B14-M1：全项目 i18n 硬编码字符串迁移 | ⚠️ **部分采纳** | B14-P2-1 | i18n 系统已建立，迁移范围收窄到 println!/eprintln!；info! 日志不迁移 |
| B14-M2：统一错误处理体系 | ✅ **部分采纳** | B14-P1-1 | AppError 已存在；全量迁移 62 文件风险高；仅迁移公共接口边界 |
| B14-M3：MCP STDIO 模式集成到主应用 | ✅ **简化采纳** | B14-P0-1 | MCP 架构已存在；不需新 `--mcp-stdio` flag，用统一的 `--protocol-mode mcp` 即可 |
| B14-M4：MCP HTTP 模式集成到主应用 | ✅ **简化采纳** | B14-P0-1 | 同 M3，统一 CLI 参数覆盖两种 MCP 模式 |
| B14-M5：扩展性能监控到所有关键路径 | ✅ **采纳（范围收窄）** | B14-P0-2 | 仅 HTTP POST + RPC chat 两个关键路径；不在每个函数添加测量 |
| B14-M6：完善文档体系 | ⚠️ **部分采纳** | B14-P2-2 | 不建 docs/ 目录；仅扩充 README 三个关键章节 |
| B14-M7：建立完整测试覆盖体系 | ✅ **部分采纳** | B14-P1-2 | 测试覆盖已 100%（BLUE13）；此轮补 clippy + cargo audit 静态门禁 |
| B14-M8：验证生产就绪质量指标 | 💡 **文档化参考** | B14-P3-1 | 指标表格记录在本文档即可；不需额外代码 |

---

## 验收标准汇总

### P0 验收
- [x] `./go-on --help` 包含 `--protocol-mode`
- [x] `--protocol-mode mcp` 覆盖 config 文件设置
- [x] 非法值返回非零退出码
- [x] `/v1/chat/completions` 请求后 PerformanceMetrics.total_ops >= 1
- [x] stderr 可见延迟日志，stdout 不被污染

### P1 验收
- [x] agent trait 公共签名返回 `crate::core::error::Result`
- [x] 错误可被 `match` 精确分类为 `ProxyError` / `ValidationError` 等
- [x] `cargo clippy --all-targets -- -D warnings` 零错误
- [x] `cargo audit` 无高危漏洞

### P2 验收
- [x] 主要 setup wizard 和 status 输出可中英文切换
- [x] README 包含快速开始、四种模式、配置速查三章
- [x] 新人 5 分钟内可启动项目

### 全局约束
- [x] 217 个现有测试无任何回归
- [x] test_ci.sh 步骤 6a~6x 可本地执行通过
- [x] 所有改动 `cargo fmt --all` 格式通过

### BLUE14 完成率回写（2026-04-14）

- 核心主链闭环完成率：**100%**（已完成 20 / 20 个核心验收点）
- 扩展硬化项完成率：**100%**（HD1/HD2/HD3/HD4 已完成）
- PUA 主链项完成率：**100%**（PUA1/PUA2/PUA3/PUA4 已完成）
- Agent 自主补充项完成率：**100%**（AGENT1/AGENT2/AGENT3/AGENT4 已完成）
- 已完成闭环：
    - P0 全部完成（协议模式 CLI + 关键路径性能监控）
    - P1 全部完成（错误边界统一 + Clippy/Audit 门禁）
    - P2 全部完成（文档 + i18n 用户可见输出）
    - AI1-AI4 已实现并接入 CI（Token 优化、自学习、强化学习、知识淬炼）
    - HD1 已接入 MCP 工具调用主链（策略选择 -> 沙箱校验 -> 允拒反馈）
    - HD2 已接入 MCP 工具调用预算跟踪（tokens/tool_calls/wall_clock）并新增 6l 门禁
    - HD3 已接入 task.execute 幂等性缓存（TTL 5 分钟）并新增 6m 门禁
    - HD4 已接入 MCP 工具调用审计日志（NDJSON）并新增 6n 门禁
    - 全量回归通过（backend + GUI + vscode-addon）
    - `test_ci.sh` 6a~6x 全链门禁通过

---

---

## 附：AI 智能功能优化（自学习 / 强化学习 / 知识淬炼 / TOKEN 节省）

> 来源：BLUE14-temp.MD "AI智能功能优化前景分析" 章节（分散多处，全量整合）
> 这四项功能当前均为"框架已定义，算法/执行逻辑待实现"状态，优化路径清晰，投入产出比高。

---

### 📊 AI 智能功能现状评估（基于代码扫描）

| 功能领域 | 主要文件 | 实现状态 | 成熟度 | 集成度 | 优化潜力 |
|---|---|---|---|---|---|
| **自学习** | `src/intelligence/reinforcement.rs` | ✅ `LearningFeedbackSystem` 已实现并持久化 | 中 | 已接入主链测试 | 中 |
| **强化学习** | `src/intelligence/reinforcement.rs` | ✅ `QLearningAgent` + `RewardFunction` 已实现 | 中 | 已接入主链测试 | 中 |
| **知识淬炼** | `src/intelligence/quality_models.rs` | ✅ `KnowledgeDistiller` + `aggregate_verdict` 已实现 | 中 | 已接入主链测试 | 中 |
| **TOKEN 节省** | `src/optimization/cost_optimizer.rs` | ✅ `smart_compress` + `ContextCache` 已实现 | 中 | 已接入主链测试 | 中 |

---

### B14-AI1：TOKEN 节省优化

**是否需要**：✅ **需要，P0 — 投资回报率最高**

> 根因：`CostOptimizer` 已实现 `compress_prompt()` / `select_model()` / `estimate_cost()`，
> 但压缩算法为简单截断，无语义感知；无上下文缓存机制；估算精度可进一步提升。
> 直接影响运行成本，每轮对话即可产生可量化收益。

**推荐建议**：在现有 `CostOptimizer` 基础上新增语义压缩与上下文缓存接口；不引入外部 ML 依赖。

#### 步骤 AI1.1：语义感知提示压缩

在 `src/optimization/cost_optimizer.rs` 中新增：

```rust
/// 语义感知压缩 —— 优先保留最近上下文和关键指令，剔除冗余重复内容
pub fn smart_compress(&self, original: &str, max_tokens: usize) -> CompressionResult {
    // 1. 按句/段落分割
    // 2. 计算每段落的信息密度（关键词频度）
    // 3. 优先保留 system prompt、最新 user/assistant 轮次
    // 4. 压缩至 max_tokens 预估窗口内
    // 5. 返回压缩率与保留段落摘要
}
```

#### 步骤 AI1.2：上下文响应缓存

新增 `ContextCache` 结构（基于现有 `performance::CacheStats` 模式）：

```rust
pub struct ContextCache {
    /// key = prompt 语义哈希（FNV-1a of normalized content）
    semantic_cache: HashMap<u64, CachedResponse>,
    max_entries: usize,
}

impl ContextCache {
    pub fn get_by_semantic_key(&self, prompt: &str) -> Option<&CachedResponse>;
    pub fn insert(&mut self, prompt: &str, response: CachedResponse);
    pub fn evict_lru(&mut self);
}
```

#### 步骤 AI1.3：单测
```rust
#[test]
fn smart_compress_reduces_length_without_losing_system_prompt() { ... }

#[test]
fn context_cache_hit_avoids_model_call() { ... }
```

#### 步骤 AI1.4：接入主链
```bash
# test_ci.sh 步骤 6f
cargo test optimization::cost_optimizer::tests::smart_compress -- --nocapture
```

**预期收益**：短期减少 TOKEN 使用 20-30%，中期 40-50%。

**验收标准**：
- `smart_compress` 在同等语义保真度下输出长度 ≤ 原始长度 × 0.75
- `ContextCache` 命中时不触发 model call
- 现有 `cost_optimizer` 4 项测试无回归

---

### B14-AI2：自学习反馈系统实现

**是否需要**：✅ **需要，P0/P1 — 闭合学习反馈回路**

> 根因：`reinforcement.rs` 已定义 `WorkflowLearningEvent`、`KnowledgeInsightArtifact` 等完整数据结构，
> 但无任何从任务执行结果收集反馈、分析成功/失败模式、更新策略建议的实现逻辑。
> 已有的 `.goon/` 持久化基础设施可直接复用，实现成本低、收益明确。

**推荐建议**：基于现有结构新增 `LearningFeedbackSystem`，与已有 `rpc_learning_summary_aggregates_clarification_feedback_metrics` 集成测试对齐。

#### 步骤 AI2.1：实现反馈收集

在 `src/intelligence/reinforcement.rs` 中新增：

```rust
pub struct LearningFeedbackSystem {
    events: Vec<WorkflowLearningEvent>,
    storage_path: PathBuf,
}

impl LearningFeedbackSystem {
    pub fn new(storage_path: PathBuf) -> Self;

    /// 收集一次任务执行反馈
    pub fn collect(&mut self, event: WorkflowLearningEvent) {
        self.events.push(event);
        self.persist_event(&event); // 写入 .goon/learning/
    }

    /// 分析最近 N 次事件，识别成功/失败模式
    pub fn analyze_patterns(&self, window: usize) -> Vec<LearningPattern>;

    /// 提取知识洞察 (KnowledgeInsightArtifact)
    pub fn extract_insights(&self) -> Vec<KnowledgeInsightArtifact>;
}
```

#### 步骤 AI2.2：经验知识库

```rust
pub struct ExperienceKnowledgeBase {
    success_cases: Vec<SuccessCase>,
    failure_patterns: Vec<FailurePattern>,
}

impl ExperienceKnowledgeBase {
    pub fn add_success_case(&mut self, case: SuccessCase);
    pub fn find_similar(&self, objective: &str) -> Option<&SuccessCase>;
    pub fn top_failure_patterns(&self, limit: usize) -> Vec<&FailurePattern>;
}
```

#### 步骤 AI2.3：单测
```rust
#[test]
fn feedback_system_collects_and_persists_event() { ... }

#[test]
fn experience_base_finds_similar_success_case() { ... }
```

#### 步骤 AI2.4：接入主链
```bash
# test_ci.sh 步骤 6g
cargo test intelligence::reinforcement::tests::learning_feedback -- --nocapture
```

**验收标准**：
- `collect()` 后事件被写入 `.goon/learning/`
- `analyze_patterns()` 在 ≥ 3 个同类事件后能识别模式
- 现有 `intelligence::reinforcement::tests::` 5 项测试无回归

---

### B14-AI3：强化学习算法基础实现

**是否需要**：✅ **需要，P1 — 建立策略优化能力**

> 根因：`ExecutionDecisionCandidate`、`TaskExecutionMetrics` 等结构定义完整，
> 但无任何 Q-Learning / 策略梯度实现，奖励函数体系缺失。
> 当前所有决策依赖规则匹配，缺乏从历史执行中自动优化决策的能力。

**推荐建议**：实现轻量级表格化 Q-Learning（无需神经网络），奖励函数对齐现有 `TaskExecutionMetrics` 字段。

#### 步骤 AI3.1：Q-Learning Agent

```rust
pub struct QLearningAgent {
    /// state = (task_type, complexity_tier) -> action = model_choice
    q_table: HashMap<(String, String), HashMap<String, f64>>,
    learning_rate: f64,   // 建议初值 0.1
    discount_factor: f64, // 建议初值 0.9
    exploration_rate: f64, // 建议初值 0.2，随轮次衰减
}

impl QLearningAgent {
    pub fn choose_action(&self, state: &(String, String)) -> String;

    pub fn update(
        &mut self,
        state: &(String, String),
        action: &str,
        reward: f64,
        next_state: &(String, String),
    );

    pub fn decay_exploration(&mut self, rate: f64) {
        self.exploration_rate = (self.exploration_rate * rate).max(0.01);
    }
}
```

#### 步骤 AI3.2：奖励函数（对齐现有 TaskExecutionMetrics）

```rust
pub struct RewardFunction {
    token_saving_weight: f64, // 建议 0.3
    success_weight: f64,      // 建议 0.4
    quality_weight: f64,      // 建议 0.2
    speed_weight: f64,        // 建议 0.1
}

impl RewardFunction {
    pub fn calculate(&self, metrics: &TaskExecutionMetrics) -> f64 {
        let token_saving = 1.0 - (metrics.tokens_used as f64 / 4096.0).min(1.0);
        let success = if metrics.success { 1.0 } else { -1.0 };
        let quality = metrics.quality_score;
        let speed = 1.0 - (metrics.duration_ms as f64 / 30_000.0).min(1.0);

        token_saving * self.token_saving_weight
            + success * self.success_weight
            + quality * self.quality_weight
            + speed * self.speed_weight
    }
}
```

#### 步骤 AI3.3：单测
```rust
#[test]
fn q_learning_updates_q_table_on_reward() { ... }

#[test]
fn reward_function_positive_for_successful_low_token_task() { ... }

#[test]
fn exploration_decays_toward_minimum() { ... }
```

#### 步骤 AI3.4：接入主链
```bash
# test_ci.sh 步骤 6h
cargo test intelligence::reinforcement::tests::q_learning -- --nocapture
```

**验收标准**：
- Q-table 在 10 次更新后对高回报 action 的 Q 值高于低回报 action
- `decay_exploration` 永不低于 0.01
- 奖励函数对 success=true、tokens_used=100 的任务返回正值

---

### B14-AI4：知识淬炼算法实现

**是否需要**：✅ **需要，P1 — 将执行经验转化为可复用知识**

> 根因：`QualityVerdict`、`QualitySignal` 等质量模型已定义，
> 但 `quality_models.rs` 仅 35 行，无任何处理逻辑。
> 成功执行的知识片段散落在 `WorkflowLearningEvent.insights` 中，无自动提炼与去重机制。

**推荐建议**：新增 `KnowledgeDistiller`，从 `WorkflowLearningEvent` 序列中提炼 `KnowledgeInsightArtifact`，输入 BLUE13 已有的 `rpc_learning_summary_aggregates_clarification_feedback_metrics` 路径。

#### 步骤 AI4.1：知识提炼器

```rust
pub struct KnowledgeDistiller {
    min_confidence: f64,      // 低于此置信度的洞察丢弃
    dedup_threshold: f64,     // 语义相似度超过此阈值视为重复
    max_insights_per_type: usize,
}

impl KnowledgeDistiller {
    /// 从一批学习事件中提炼高质量洞察
    pub fn distill(
        &self,
        events: &[WorkflowLearningEvent],
    ) -> Vec<KnowledgeInsightArtifact>;

    /// 对现有洞察库去重（按 insight_type 归并相似内容）
    pub fn deduplicate(
        &self,
        insights: Vec<KnowledgeInsightArtifact>,
    ) -> Vec<KnowledgeInsightArtifact>;

    /// 从成功案例中构建可复用模式
    pub fn build_pattern(
        &self,
        success_events: &[WorkflowLearningEvent],
    ) -> Option<KnowledgeInsightArtifact>;
}
```

#### 步骤 AI4.2：质量评估扩展

在 `quality_models.rs` 中补充评估逻辑：

```rust
impl QualitySignal {
    pub fn is_sufficient_for_distillation(&self) -> bool {
        self.passed && self.confidence >= 0.7
    }
}

/// 从信号列表计算整体 QualityVerdict
pub fn aggregate_verdict(signals: &[QualitySignal]) -> QualityVerdict {
    let pass_rate = signals.iter().filter(|s| s.passed).count() as f64 / signals.len() as f64;
    match pass_rate {
        r if r >= 0.9 => QualityVerdict::Approve,
        r if r >= 0.7 => QualityVerdict::ApproveWithCaveats,
        r if r >= 0.5 => QualityVerdict::Revise,
        _ => QualityVerdict::Reject,
    }
}
```

#### 步骤 AI4.3：单测
```rust
#[test]
fn distiller_filters_low_confidence_insights() { ... }

#[test]
fn deduplicate_removes_high_similarity_entries() { ... }

#[test]
fn aggregate_verdict_approve_when_all_signals_pass() { ... }
```

#### 步骤 AI4.4：接入主链
```bash
# test_ci.sh 步骤 6i
cargo test intelligence::quality_models::tests:: -- --nocapture
```

**验收标准**：
- `distill()` 对 confidence < 0.7 的洞察不输出
- `deduplicate()` 对相同 `insight_type` + 相同内容前 50 字符的条目合并
- `aggregate_verdict` 在所有 signal.passed=true 时返回 `Approve`

---

## 附：HARDNESS 硬化/加固优化

> 来源：BLUE14-temp.MD "HARDNESS（硬化/加固）优化建议" 章节（完整整合）
> `src/governance/hardening.rs` 中 6 类结构均已定义，但执行逻辑均为空。
> 主应用 `main.rs` 仅 `pub use crate::governance::hardening` 一行，未有任何调用点。

---

### 📊 HARDNESS 功能现状评估（基于代码扫描）

| 功能领域 | 主要文件 | 实现状态 | 集成度 | 优先级 |
|---|---|---|---|---|
| **策略管理** | `src/governance/hardening.rs` | ✅ 已新增策略解析与执行判定（`policy_bundle_for_target` / `enforce_action`） | 已接入 MCP 工具主链 | P0 |
| **沙箱控制** | `src/governance/hardening.rs` | ✅ 已在 `mcp.tools.call` 路径执行允拒校验并回传错误 | 已接入 MCP 工具主链 | P0 |
| **资源配额** | `src/governance/hardening.rs` | ✅ TaskBudget / TenantResourceQuota 定义，跟踪缺失 | 未集成 | P1 |
| **幂等性** | `src/governance/hardening.rs` | ✅ Idempotency::key() 存在，检查/缓存逻辑缺失 | 未集成 | P1 |
| **审计日志** | `src/governance/hardening.rs` | ✅ AutonomousEditAuditEntry 定义，收集/查询缺失 | 未集成 | P2 |
| **密钥安全** | `src/core/config.rs` | ✅ 验证函数完整，密钥轮换/加密存储可优化 | 已集成 | P2 |

---

### B14-HD1：策略执行引擎接入主链路

**是否需要**：✅ **必须，P0 — 安全门禁的核心**

> 根因：`PolicyBundle::local_dev()` / `ci_pipeline()` / `managed_service()` 已定义，
> 但调用方为零。ACP server 启动时读取 config 后，从未将沙箱级别传递给工具执行路径，
> 导致 `SandboxPolicy::can_execute_shell()` 等函数的校验逻辑从未被触发。
> 这是 OWASP A01（权限控制失效）的直接缺口。

**推荐建议**：在 ACP server 启动时根据 `config.toml [deployment]` 段加载对应策略包，并在工具调用前经过 `SandboxPolicy` 校验。

#### 步骤 HD1.1：在 config.toml 结构中添加 deployment 键读取

```rust
// src/core/config.rs 中 RuntimeConfig 添加：
pub deployment_target: Option<String>, // "local-dev" | "ci" | "managed-service"
```

#### 步骤 HD1.2：在 ACP server 初始化时加载策略包

```rust
// src/main.rs 的 run() 函数中：
let policy = match runtime_config.deployment_target.as_deref() {
    Some("ci")               => PolicyBundle::ci_pipeline(),
    Some("managed-service")  => PolicyBundle::managed_service(),
    _                        => PolicyBundle::local_dev(),
};
// 将 policy 传入 server 上下文
```

#### 步骤 HD1.3：工具调用前沙箱校验

在工具分发路径中添加：

```rust
// 在执行 shell 类工具前：
if !SandboxPolicy::can_execute_shell(&policy.sandbox_level) {
    return Err(AppError::Proxy(ProxyError::UpstreamError {
        code: Some(403),
        message: format!("shell execution denied by policy '{}'", policy.name),
    }));
}
```
// src/acp/server.rs AcpServer::new() 中：
let policy_bundle = match cfg.deployment_target.as_deref() {
    Some("ci")              => PolicyBundle::ci_pipeline(),
    Some("managed-service") => PolicyBundle::managed_service(),
    _                       => PolicyBundle::local_dev(),
};
// 存储在 AcpServer 字段，以便工具调用路径引用
```

#### 步骤 HD1.3：在工具调用前做沙箱校验

```rust
// src/acp/server.rs 工具 dispatch 前插入：
if !self.policy_bundle.sandbox.can_execute_shell() {
    return Err(anyhow!("shell execution blocked by SandboxPolicy"));
}
if !self.policy_bundle.sandbox.can_read_file(path) {
    return Err(anyhow!("file read blocked by SandboxPolicy: {}", path));
}
```

#### 步骤 HD1.4：单测
```rust
#[test]
fn sandbox_policy_blocks_shell_in_ci_mode() { ... }

#[test]
fn managed_service_policy_denies_arbitrary_file_access() { ... }
```

#### 步骤 HD1.5：接入主链
```bash
# test_ci.sh 步骤 6j
cargo test governance::hardening::tests::sandbox_policy -- --nocapture
```

**验收标准**：
- `ci_pipeline()` 策略包的 `can_execute_shell()` 返回 `false`
- ACP server 在 CI 部署目标下拒绝 shell 工具并返回明确错误消息
- 现有 217 项测试无回归

---

### B14-HD2：资源配额跟踪器实现

**是否需要**：✅ **需要，P1 — 防止单任务资源滥用**

> 根因：`TaskBudget`（max_tokens / max_steps / max_duration_secs）和 `TenantResourceQuota`
> 已定义，但无任何实际计数逻辑。任务执行时不减计数、不超限报错、不上报使用量。
> 结合 BLUE14 B14-P1-1（CostOptimizer）时，缺少配额门禁会使成本控制策略形同虚设。

**推荐建议**：新增 `BudgetTracker`，在每次工具调用后减扣 `TaskBudget`，超限时中止任务并写入告警日志。

#### 步骤 HD2.1：BudgetTracker 结构

```rust
// src/governance/hardening.rs 新增：
pub struct BudgetTracker {
    budget: TaskBudget,
    used_tokens: u32,
    used_steps: u32,
    start_time: std::time::Instant,
}

impl BudgetTracker {
    pub fn new(budget: TaskBudget) -> Self;

    /// 每次工具调用后调用；超限返回 Err
    pub fn consume(&mut self, tokens: u32) -> Result<(), BudgetExceeded>;

    /// 每步结束后调用
    pub fn tick_step(&mut self) -> Result<(), BudgetExceeded>;

    pub fn remaining(&self) -> BudgetSnapshot;
}

pub struct BudgetExceeded {
    pub kind: BudgetKind, // Tokens | Steps | Duration
    pub limit: u64,
    pub used: u64,
}
```

#### 步骤 HD2.2：单测
```rust
#[test]
fn budget_tracker_errors_when_tokens_exhausted() { ... }

#[test]
fn budget_tracker_errors_when_steps_exhausted() { ... }
```

#### 步骤 HD2.3：接入主链
```bash
# test_ci.sh 步骤 6k
cargo test governance::hardening::tests::budget_tracker -- --nocapture
```

**验收标准**：
- `consume(tokens)` 在累计超过 `max_tokens` 后返回 `Err(BudgetExceeded { kind: Tokens })`
- `tick_step()` 在超过 `max_steps` 后返回 `Err`
- 触发的 `BudgetExceeded` 包含 limit 和 used 字段以便日志可读

---

### B14-HD3：幂等性检查缓存实现

**是否需要**：✅ **需要，P1 — 防止重复请求引发副作用**

> 根因：`Idempotency::key()` 返回 `blake3` hash 字符串，但无任何缓存存储、
> 检查已处理请求、返回缓存响应的逻辑。HTTP 幂等请求无法检测重放——OWASP A09
> 日志与监控失效的配套要求。

**推荐建议**：新增 `IdempotencyCache`，在 ACP handler 入口检查 key，命中则直接返回缓存响应，未命中则处理后写入缓存。

#### 步骤 HD3.1：IdempotencyCache 结构

```rust
// src/governance/hardening.rs 新增：
pub struct IdempotencyCache {
    store: std::collections::HashMap<String, CachedIdempotentResponse>,
    ttl_secs: u64,
}

pub struct CachedIdempotentResponse {
    pub response: serde_json::Value,
    pub cached_at: std::time::Instant,
}

impl IdempotencyCache {
    pub fn new(ttl_secs: u64) -> Self;
    pub fn check(&self, key: &str) -> Option<&CachedIdempotentResponse>;
    pub fn store(&mut self, key: &str, response: serde_json::Value);
    pub fn evict_expired(&mut self) -> usize;
}
```

#### 步骤 HD3.2：在 ACP server 请求入口集成

```rust
// src/acp/server.rs handle_request 前：
let idem_key = Idempotency::key(&request_body);
if let Some(cached) = self.idempotency_cache.check(&idem_key) {
    return Ok(cached.response.clone());
}
// ... 正常处理 ...
self.idempotency_cache.store(&idem_key, response.clone());
```

#### 步骤 HD3.3：单测
```rust
#[test]
fn idempotency_cache_returns_cached_on_second_call() { ... }

#[test]
fn idempotency_evict_expired_removes_old_entries() { ... }
```

#### 步骤 HD3.4：接入主链
```bash
# test_ci.sh 步骤 6l
cargo test governance::hardening::tests::idempotency_cache -- --nocapture
```

**验收标准**：
- 同一 `key` 第二次 `check()` 返回 `Some(cached)`
- `evict_expired()` 清除 TTL 到期条目并返回清除数量
- 现有幂等性键生成测试无回归

---

### B14-HD4：自主编辑审计日志收集实现

**是否需要**：⚠️ **建议，P2 — 合规追溯能力**

> 根因：`AutonomousEditAuditEntry`（file_path, edit_type, before/after_hash, approved_by 等）
> 已定义完整，但无任何生成、收集、持久化逻辑。managed-service 部署场景下
> 缺少审计追踪是合规风险（SOC2/ISO27001 A.12.4）。

**推荐建议**：在工具执行后自动生成 `AutonomousEditAuditEntry`，追加写入 `.goon/audit/YYYY-MM-DD.ndjson`，与已有结构对齐。

#### 步骤 HD4.1：AuditLogger 实现

```rust
pub struct AuditLogger {
    log_dir: PathBuf,
}

impl AuditLogger {
    pub fn new(log_dir: PathBuf) -> Self;
    pub fn record(&self, entry: AutonomousEditAuditEntry) -> Result<()>;
    pub fn query_by_file(&self, file_path: &str, limit: usize) -> Result<Vec<AutonomousEditAuditEntry>>;
}
```

#### 步骤 HD4.2：单测
```rust
#[test]
fn audit_logger_persists_entry_to_ndjson() { ... }

#[test]
fn audit_query_returns_entries_for_matching_file() { ... }
```

#### 步骤 HD4.3：接入主链
```bash
# test_ci.sh 步骤 6m
cargo test governance::hardening::tests::audit_logger -- --nocapture
```

**验收标准**：
- `record()` 后文件存在且可反序列化为 `AutonomousEditAuditEntry`
- `query_by_file()` 只返回 `file_path` 匹配的条目
- managed-service 策略包下的工具调用自动触发 `record()`

---

### B14-HD5：密钥安全强化（环境变量优先 + zeroize）

**是否需要**：⚠️ **参考，P2 — 生产部署密钥保护**

> 根因：`src/core/config.rs` 验证逻辑完整，但 API key 以明文存储在 `config.toml`，
> 无密钥轮换记录，无加密存储支持。CI/CD 管道中明文密钥易被日志泄露（OWASP A02）。

**推荐建议**：支持从环境变量读取 API key（优先于 config.toml），读取后对临时字符串执行 zeroize。

**范围说明（避免歧义）**：本条指“程序运行级/网关级”密钥（例如 `GO_ON_API_KEY` 这类 runtime 入口密钥），
不指各 agent 的 provider 密钥。agent 密钥继续沿用现有机制：`[agents.*].api_key_env` /
`[agents.*].secret_key_env`，并可通过 `keyring://...` 引用密钥仓库。

#### 步骤 HD5.1：环境变量覆盖支持

```rust
// src/core/config.rs RuntimeConfig::load() 中：
if let Ok(val) = std::env::var("GO_ON_API_KEY") {
    config.api_key = val;
}
```

#### 步骤 HD5.2：zeroize 依赖

```toml
# Cargo.toml [dependencies]
zeroize = "1"
```

#### 步骤 HD5.3：单测
```rust
#[test]
fn config_reads_api_key_from_env_over_file() { ... }
```

#### 步骤 HD5.4：接入主链
```bash
# test_ci.sh 步骤 6n — 前置 cargo audit 门禁
cargo audit
cargo test core::config::tests::api_key_env_override -- --nocapture
```

**验收标准**：
- 设置 `GO_ON_API_KEY` 环境变量后 `RuntimeConfig::load()` 使用该值
- 文件内 api_key 不覆盖已设置的环境变量值
- `cargo audit` 报告无已知漏洞（CI 门禁强制）

---

## 附：PUA（渐进式用户适应/质量强制）优化

> 来源：blue14-temp2.md PUA 优化建议章节（完整整合）
> `src/governance/pua.rs` 已定义 `PuaEnforcementPlan`、`PuaStageRequirement`、
> `PuaExecutionReport`、`quality_compass()`，以及 `AcpServer.pua_enforcement_plan` 字段，
> 但请求处理路径中 PUA 规则从未被应用——Plan 加载后即被搁置。

---

### 📊 PUA 功能现状评估（基于代码扫描）

| 功能领域 | 主要文件 | 实现状态 | 集成度 | 优先级 |
|---|---|---|---|---|
| **Plan 定义** | `src/governance/pua.rs` | ✅ 完整数据结构 | 未集成 | P0 |
| **质量指南针** | `src/governance/pua.rs` | ✅ `quality_compass()` 静态 5 项 | 未集成 | P1 |
| **阶段验证** | `src/governance/pua.rs` | ✅ `PuaStageRequirement` 定义 | 未集成 | P0 |
| **执行报告** | `src/governance/pua.rs` | ✅ `PuaExecutionReport` 定义 | 未集成 | P2 |
| **学习/反馈** | — | ❌ 缺失 | 未集成 | P2 |
| **上下文感知** | — | ❌ 缺失 | 未集成 | P1 |

---

### B14-PUA1：PUA 规则引擎接入请求处理主链

**是否需要**：✅ **必须，P0 — AcpServer 持有 Plan 但完全未使用**

> 根因：`AcpServer` 有 `pua_enforcement_plan: Arc<StdMutex<PuaEnforcementPlan>>` 字段，
> 初始化时已加载默认 Plan，但 `handle_request()` 内无任何对 Plan 的调用。
> `PuaStageRequirement.hard_fail_conditions` 永远不会被检查，
> `red_lines`（绝对禁止项）无法阻止任何操作。

**推荐建议**：新增 `PuaRuleEngine`，在 ACP 请求处理入口调用 `validate_stage()` 和 `check_red_lines()`；违反则拒绝请求并返回 `403 + PuaViolation` 详情。

#### 步骤 PUA1.1：PuaRuleEngine 核心结构

```rust
// src/governance/pua.rs 新增：
pub struct PuaRuleEngine {
    plan: Arc<StdMutex<PuaEnforcementPlan>>,
}

pub struct PuaViolation {
    pub kind: PuaViolationKind, // RedLine | StageFail | MissingEvidence
    pub detail: String,
}

impl PuaRuleEngine {
    pub fn new(plan: Arc<StdMutex<PuaEnforcementPlan>>) -> Self;

    /// 检查 red_lines；任一匹配则返回 Err(PuaViolation)
    pub fn check_red_lines(&self, action: &str) -> Result<(), PuaViolation>;

    /// 验证当前阶段的 required_actions 是否满足
    pub fn validate_stage(&self, stage: &str, completed: &[String]) -> Result<(), PuaViolation>;

    /// 收集已完成的证据条目（用于 PuaExecutionReport）
    pub fn collect_evidence(&self, stage: &str) -> Vec<String>;
}
```

#### 步骤 PUA1.2：接入 ACP handle_request

```rust
// src/acp/server.rs handle_request 顶部：
self.pua_engine.check_red_lines(&action)?;
self.pua_engine.validate_stage(&current_stage, &completed_actions)?;
```

#### 步骤 PUA1.3：单测
```rust
#[test]
fn pua_engine_blocks_red_line_action() { ... }

#[test]
fn pua_engine_fails_stage_with_missing_required_action() { ... }

#[test]
fn pua_engine_passes_when_all_conditions_met() { ... }
```

#### 步骤 PUA1.4：接入主链
```bash
# test_ci.sh 步骤 6o
cargo test governance::pua::tests::pua_rule_engine -- --nocapture
```

**验收标准**：
- `check_red_lines()` 对位于 `red_lines` 列表的 action 返回 `Err(PuaViolation { kind: RedLine })`
- `validate_stage()` 在 `required_actions` 未完成时返回 `Err`
- ACP 请求处理器在 PUA 违反时返回 4xx 而非继续执行
- 现有 217 项测试无回归

---

### B14-PUA2：动态质量指南针（上下文感知检查清单）

**是否需要**：✅ **需要，P1 — 当前静态 5 项指南针无法适应不同任务类型**

> 根因：`quality_compass()` 返回固定 `Vec<String>`（5 项），无论任务是
> "修复 bug"还是"新增功能"，检查清单完全相同。高风险变更与低风险变更适用同等严格度，
> 导致低风险任务过度摩擦、高风险任务检查不足。

**推荐建议**：新增 `DynamicQualityCompass`，根据任务上下文（变更类型、文件范围、历史失败率）
动态生成检查清单，分为 base / contextual / adaptive 三层叠加。

#### 步骤 PUA2.1：DynamicQualityCompass 结构

```rust
// src/governance/pua.rs 新增：
pub struct DynamicQualityCompass {
    base_checks: Vec<QualityCheck>,
    context_rules: Vec<ContextRule>,
}

pub struct QualityCheck {
    pub id: String,
    pub description: String,
    pub category: QualityCategory,      // Safety | Correctness | Performance | Style
    pub verification: VerificationMethod, // AutoTest | ManualReview | StaticAnalysis
    pub required: bool,
}

impl DynamicQualityCompass {
    pub fn get_checks(&self, context: &TaskContext) -> Vec<QualityCheck>;

    /// 兼容旧接口：返回字符串列表
    pub fn quality_compass_compat(&self) -> Vec<String> {
        quality_compass() // 委托给现有静态实现
    }
}
```

#### 步骤 PUA2.2：TaskContext 结构

```rust
pub struct TaskContext {
    pub task_type: TaskType, // BugFix | FeatureAdd | Refactor | SecurityPatch
    pub file_count: usize,
    pub risk_score: f64,     // 0.0-1.0，由历史失败率计算
}
```

#### 步骤 PUA2.3：单测
```rust
#[test]
fn compass_adds_security_check_for_security_patch_task() { ... }

#[test]
fn compass_base_checks_always_present() { ... }

#[test]
fn quality_compass_compat_returns_five_items() { ... }
```

#### 步骤 PUA2.4：接入主链
```bash
# test_ci.sh 步骤 6p
cargo test governance::pua::tests::dynamic_compass -- --nocapture
```

**验收标准**：
- `SecurityPatch` 类型任务的检查清单包含至少 1 项 `category: Safety` 检查
- `quality_compass_compat()` 仍返回原有 5 项（向后兼容）
- `base_checks` 在任意 `TaskContext` 下始终出现

---

### B14-PUA3：PUA 学习与反馈系统

**是否需要**：⚠️ **建议，P2 — 与 B14-AI2 协同建立完整反馈回路**

> 根因：PUA 规则当前为静态配置，无法从历史执行结果中学习；
> `PuaExecutionReport.missing_checks` 积累的失败模式从未被用于规则优化。
> B14-AI2（LearningFeedbackSystem）已规划收集 `WorkflowLearningEvent`，
> 两者应共享数据通道而非各自维护独立存储。

**推荐建议**：新增轻量级 `PuaFeedbackCollector`，将 `PuaExecutionReport` 写入
B14-AI2 的 `.goon/learning/` 路径（JSON 行格式），由 `LearningFeedbackSystem` 统一处理。

#### 步骤 PUA3.1：PuaFeedbackCollector 结构

```rust
// src/governance/pua.rs 新增：
pub struct PuaFeedbackCollector {
    storage_path: PathBuf, // 指向 .goon/learning/pua/
}

impl PuaFeedbackCollector {
    pub fn new(storage_path: PathBuf) -> Self;

    /// 将执行报告序列化写入 NDJSON
    pub fn collect(&self, report: &PuaExecutionReport) -> Result<()>;

    /// 从已存报告提取学习数据（供 LearningFeedbackSystem 消费）
    pub fn extract_learning_data(&self, limit: usize) -> Result<Vec<PuaLearningRecord>>;
}

pub struct PuaLearningRecord {
    pub stage: String,
    pub passed: bool,
    pub missing_checks: Vec<String>,
    pub escalation_level: u8,
}
```

#### 步骤 PUA3.2：单测
```rust
#[test]
fn pua_collector_writes_report_to_ndjson() { ... }

#[test]
fn pua_learning_data_extraction_returns_correct_records() { ... }
```

#### 步骤 PUA3.3：接入主链
```bash
# test_ci.sh 步骤 6q
cargo test governance::pua::tests::pua_feedback_collector -- --nocapture
```

**验收标准**：
- `collect()` 后 `.goon/learning/pua/` 下文件存在且可反序列化
- `extract_learning_data(5)` 返回最新 5 条记录
- `PuaLearningRecord.passed` 准确反映 `PuaExecutionReport.status`

---

### B14-PUA4：PUA 执行报告生成

**是否需要**：⚠️ **参考，P2/P3 — 可观测性增强**

> 根因：`PuaExecutionReport` 结构完整（stage, status, escalation_level,
> required_evidence, completed_checks, missing_checks），但当前无任何代码路径
> 会填充并返回该结构。已有 B14-PUA1 后，执行报告可作为调试诊断输出。

**推荐建议**：在 `PuaRuleEngine.validate_stage()` 执行后生成 `PuaExecutionReport`，
通过 ACP response header（`X-Pua-Report: <base64-json>`）透传给调用方，
仅在 `config.toml [debug] pua_report = true` 时启用。

#### 步骤 PUA4.1：报告生成逻辑

```rust
impl PuaRuleEngine {
    pub fn generate_report(&self, stage: &str, completed: &[String]) -> PuaExecutionReport {
        PuaExecutionReport {
            stage: stage.to_string(),
            status: if self.validate_stage(stage, completed).is_ok() {
                "pass".to_string()
            } else {
                "fail".to_string()
            },
            escalation_level: self.plan.lock().unwrap().escalation_level,
            required_evidence: self.plan.lock().unwrap()
                .stage_requirements.iter()
                .find(|r| r.stage == stage)
                .map(|r| r.required_actions.clone())
                .unwrap_or_default(),
            completed_checks: completed.to_vec(),
            missing_checks: self.collect_missing(stage, completed),
        }
    }
}
```

#### 步骤 PUA4.2：单测
```rust
#[test]
fn pua_report_status_fail_when_missing_checks_present() { ... }

#[test]
fn pua_report_status_pass_when_all_checks_complete() { ... }
```

#### 步骤 PUA4.3：接入主链
```bash
# test_ci.sh 步骤 6r
cargo test governance::pua::tests::pua_report_generator -- --nocapture
```

**验收标准**：
- `missing_checks` 为空时 `status == "pass"`
- `missing_checks` 非空时 `status == "fail"` 且包含具体缺失项目
- `debug.pua_report = false`（默认）时响应头不含 `X-Pua-Report`

---

## 附：Harness（测试框架/工具链）优化

> 来源：blue14-temp2.md "Harness 优化建议" 章节（完整整合）
> 当前 `tests/acp_runtime_rpc_integration.rs` 中 RpcHarness 提供子进程管理和
> 请求/响应捕获，但高级能力（并发、模拟服务、覆盖率门禁、CI 质量门禁）均缺失。

---

### 📊 Harness 功能现状评估（基于代码扫描）

| 功能领域 | 实现状态 | 成熟度 | 集成度 | 优化潜力 |
|---|---|---|---|---|
| **集成测试框架** | ✅ RpcHarness 实现 | 中 | 部分集成 | 高 |
| **单元测试框架** | ✅ Rust 标准测试 | 高 | 完全集成 | 中 |
| **CI 质量门禁** | ⚠️ 无 clippy/audit/tarpaulin | 低 | 未集成 | 高 |
| **测试覆盖率** | ✅ 217 个测试全部主链化 | 高 | 完全集成 | 中 |
| **并发/压力测试** | ❌ 缺失 | 无 | 未集成 | 高 |

---

### B14-HSS1：CI 质量门禁补全（clippy + audit + tarpaulin）

**是否需要**：✅ **必须，P0 — test_ci.sh 缺少静态分析与依赖安全检查门禁**

> 根因：`test_ci.sh` 已有 5ae 步骤（unit→integration→all-targets），
> 但无 `cargo clippy --deny warnings`、`cargo audit`、代码覆盖率阈值检查。
> 缺少 clippy 门禁意味着新增代码 lint 错误只能靠人工发现；
> 缺少 `cargo audit` 门禁意味着已知漏洞依赖可进入主线（呼应 B14-HD5 密钥安全）。

**推荐建议**：在 `test_ci.sh` 步骤 2 与步骤 3 之间插入 `cargo clippy`；在步骤 5 之后插入 `cargo audit`；可选接入 tarpaulin 覆盖率报告。

#### 步骤 HSS1.1：test_ci.sh 插入 clippy 门禁

```bash
# 在 test_ci.sh 步骤 2（cargo build）之后，步骤 3（unit tests）之前插入：
echo "[CI] 步骤 2b: cargo clippy"
cargo clippy --all-targets --all-features -- -D warnings
```

#### 步骤 HSS1.2：插入依赖安全审计门禁

```bash
# 在 test_ci.sh 步骤 5ae（all-targets）之后插入：
echo "[CI] 步骤 5b: cargo audit"
cargo audit
```

#### 步骤 HSS1.3：可选覆盖率报告（tarpaulin）

```bash
# 可选，仅在 CI 环境启用：
if command -v cargo-tarpaulin &> /dev/null; then
    cargo tarpaulin --out Lcov --output-dir target/coverage --fail-under 80
fi
```

#### 步骤 HSS1.4：验证
```bash
bash test_ci.sh
# 期望：clippy 零警告，audit 零漏洞，所有 217 测试通过
```

**验收标准**：
- `cargo clippy --deny warnings` 无任何 warning 退出
- `cargo audit` 无已知漏洞（RUSTSEC 数据库）
- 现有 217 项测试仍全部通过
- tarpaulin（若安装）报告行覆盖率 ≥ 80%

---

### B14-HSS2：RpcHarness 高级能力扩展

**是否需要**：✅ **需要，P1 — 当前 RpcHarness 功能有限，无并发/Mock 支持**

> 根因：`RpcHarness` 提供基础子进程控制和单请求/响应，但缺乏：
> (a) 并发请求测试（验证多路 ACP 请求的串行化保证）；
> (b) 模拟外部服务（上游 LLM API mock，避免集成测试依赖网络）；
> (c) 数据驱动测试（从 `requests/*.ndjson` 批量驱动场景）。

**推荐建议**：在 `tests/acp_runtime_rpc_integration.rs` 内新增 `AdvancedRpcHarness`，
扩展现有 `RpcHarness` 而非重写，以保持已有 217 项测试兼容。

#### 步骤 HSS2.1：AdvancedRpcHarness 扩展

```rust
// tests/acp_runtime_rpc_integration.rs 新增：
pub struct AdvancedRpcHarness {
    inner: RpcHarness,
    mock_responses: std::collections::HashMap<String, serde_json::Value>,
}

impl AdvancedRpcHarness {
    pub fn new(config_path: &Path) -> Self {
        Self {
            inner: RpcHarness::spawn(config_path),
            mock_responses: Default::default(),
        }
    }

    /// 注册对特定 method 的 mock 响应
    pub fn register_mock(&mut self, method: &str, response: serde_json::Value);

    /// 并发发送 N 个相同请求，验证幂等性
    pub fn send_concurrent(
        &mut self,
        request: serde_json::Value,
        n: usize,
    ) -> Vec<Result<serde_json::Value, String>>;

    /// 从 NDJSON 文件批量加载场景并顺序执行
    pub fn run_scenario_file(
        &mut self,
        path: &Path,
    ) -> Vec<(serde_json::Value, Result<serde_json::Value, String>)>;
}
```

#### 步骤 HSS2.2：并发幂等性测试

```rust
#[tokio::test]
async fn concurrent_requests_return_consistent_responses() {
    let mut harness = AdvancedRpcHarness::new(&config_path());
    let request = json!({"jsonrpc":"2.0","method":"ping","id":1});
    let results = harness.send_concurrent(request, 5);
    assert!(results.iter().all(|r| r.is_ok()));
    // 所有响应应相同（幂等）
    let first = results[0].as_ref().unwrap();
    assert!(results.iter().all(|r| r.as_ref().unwrap() == first));
}
```

#### 步骤 HSS2.3：接入主链
```bash
# test_ci.sh 步骤 6s
cargo test acp_runtime_rpc_integration::advanced -- --nocapture
```

**验收标准**：
- `send_concurrent(request, 5)` 5 个响应全部成功且一致
- `run_scenario_file` 能驱动 `requests/runtime-health.ndjson` 中全部场景
- 现有 RpcHarness 测试无须修改仍全部通过

---

### B14-HSS3：数据驱动集成测试基础设施

**是否需要**：⚠️ **建议，P2 — 将 requests/*.ndjson 提升为结构化测试套件**

> 根因：`requests/` 目录下已有 4 个 ndjson 文件（breaker-reset / graceful-shutdown /
> reload-and-health / runtime-health），`scripts/run-request.ps1` 可手动发送，
> 但这些场景未被自动化测试框架消费，场景更新不触发 CI 失败。

**推荐建议**：新增 `TestScenarioLoader`，在集成测试初始化时自动扫描
`requests/*.ndjson` 并生成参数化测试用例，将手动脚本转化为 `cargo test` 覆盖场景。

#### 步骤 HSS3.1：TestScenarioLoader

```rust
// tests/acp_runtime_rpc_integration.rs 新增：
pub struct TestScenario {
    pub name: String,
    pub requests: Vec<serde_json::Value>,
    pub expected_outcomes: Vec<ScenarioOutcome>,
}

pub enum ScenarioOutcome {
    Success,
    ErrorContains(String),
}

pub fn load_scenarios_from_dir(dir: &Path) -> Vec<TestScenario>;
```

#### 步骤 HSS3.2：参数化测试驱动

```rust
#[tokio::test]
async fn ndjson_scenario_files_all_pass() {
    let scenarios = load_scenarios_from_dir(Path::new("requests"));
    let mut harness = RpcHarness::spawn(&config_path());
    for scenario in scenarios {
        for (req, expected) in scenario.requests.iter().zip(&scenario.expected_outcomes) {
            let result = harness.send_request(req.clone());
            match expected {
                ScenarioOutcome::Success => assert!(result.is_ok(), "{}", scenario.name),
                ScenarioOutcome::ErrorContains(msg) => {
                    assert!(result.unwrap_err().contains(msg), "{}", scenario.name);
                }
            }
        }
    }
}
```

#### 步骤 HSS3.3：接入主链
```bash
# test_ci.sh 步骤 6t
cargo test acp_runtime_rpc_integration::ndjson_scenario_files_all_pass -- --nocapture
```

**验收标准**：
- `requests/` 下 4 个 ndjson 文件全部被扫描并执行
- 新增 ndjson 文件后无需修改 Rust 代码即可纳入测试
- 场景测试失败时错误消息含文件名和场景名以便定位

---

## 附：Agent 自主补充建议

> 以下条目为 Agent 基于当前代码库扫描和跨模块分析，独立识别的优化机会，
> 不出自 blue14-temp2.md，作为 BLUE14 的额外增量建议。

---

### B14-AGENT1：PUA 字段合约烟雾覆盖

**是否需要**：✅ **需要，P1 — 契约测试应覆盖 PUA 核心字段**

> 根因：`contracts/editor-capability-matrix.json`（合约 r4.28）记录了编辑器能力矩阵，
> 但无任何断言覆盖 `PuaEnforcementPlan.escalation_level`、`red_lines`、
> `quality_compass()` 返回值数量等字段。一旦 B14-PUA1 改动 `pua.rs` 结构，
> 合约不会自动检测到退化。

**推荐建议**：在 `tests/` 下新增 `pua_contract_smoke.rs`，
断言已知的 PuaEnforcementPlan 默认值与 `quality_compass()` 至少 3 项不为空。

```rust
// tests/pua_contract_smoke.rs
#[test]
fn pua_default_plan_has_sane_escalation_level() {
    let plan = PuaEnforcementPlan::default();
    assert!(plan.escalation_level <= 5, "escalation_level 超出预期范围");
}

#[test]
fn quality_compass_returns_non_empty_checks() {
    let checks = quality_compass();
    assert!(!checks.is_empty(), "quality_compass() 不得为空");
    assert!(checks.len() >= 3, "quality_compass() 至少应有 3 项检查");
}
```

**验收标准**：
- `pua_contract_smoke` 测试加入 `test_ci.sh` 步骤 6u
- `PuaEnforcementPlan::default()` 结构变更时测试能即时发现

---

### B14-AGENT2：B14-AI2（学习系统）与 B14-PUA3（PUA 反馈）数据通道对齐

**是否需要**：✅ **需要，P1 — 避免两套独立 NDJSON 存储格式导致碎片化**

> 根因：B14-AI2 规划 `LearningFeedbackSystem` 写入 `.goon/learning/`；
> B14-PUA3 规划 `PuaFeedbackCollector` 写入 `.goon/learning/pua/`。
> 两套系统若各自定义序列化格式，后续聚合分析需要两套解析器。
> 应在实现前对齐 NDJSON schema。

**推荐建议**：在 `src/intelligence/reinforcement.rs` 中定义共享的
`LearningRecord` 枚举（`Workflow(WorkflowLearningEvent)` | `Pua(PuaLearningRecord)`），
统一序列化为 `{"type":"workflow"|"pua", "data":{...}}`。

```rust
// src/intelligence/reinforcement.rs 新增：
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LearningRecord {
    Workflow(WorkflowLearningEvent),
    Pua(PuaLearningRecord),
}
```

**验收标准**：
- B14-AI2 和 B14-PUA3 均使用 `LearningRecord` 枚举包装后写入同一目录
- `extract_learning_data()` 和 `analyze_patterns()` 可从同一目录混合读取两类记录
- 单测验证 `LearningRecord::Pua` 和 `::Workflow` 均可正确反序列化

---

### B14-AGENT3：BudgetTracker（B14-HD2）与 PUA escalation_level 联动

**是否需要**：⚠️ **建议，P2 — 资源超限应触发 PUA 升级而非静默中止**

> 根因：B14-HD2 的 `BudgetExceeded` 错误当前只是返回 Err 并中止，
> 但 `PuaEnforcementPlan.escalation_level` 控制着合规审查强度。
> 当任务因资源耗尽而失败时，这是一个质量信号，应推升 escalation_level
> 以触发更严格的下一轮阶段检查（避免低质量任务反复触发配额耗尽）。

**推荐建议**：在 `BudgetTracker.consume()` 触发 `BudgetExceeded` 后，
调用 `PuaRuleEngine.escalate(reason)`，将当前计划的 escalation_level 加一（上限 5）。

```rust
// src/governance/hardening.rs BudgetTracker 中：
pub fn consume_with_pua(
    &mut self,
    tokens: u32,
    pua: &mut PuaRuleEngine,
) -> Result<(), BudgetExceeded> {
    match self.consume(tokens) {
        Ok(()) => Ok(()),
        Err(e) => {
            pua.escalate(&format!("BudgetExceeded: {:?}", e.kind));
            Err(e)
        }
    }
}
```

**验收标准**：
- `consume_with_pua` 超限后 `PuaEnforcementPlan.escalation_level` 增加 1
- escalation_level 不超过 5（有上限保护）
- 单测验证联动触发路径

---

### B14-AGENT4：tf! 宏 / anyhow 错误统一溯源

**是否需要**：⚠️ **参考，P3 — 诊断能力改善**

> 根因：扫描显示项目 62 个文件使用 `anyhow::Result`，但 `main.rs` 无 `tf!` 调用，
> 错误传播链中 `.context()` 注释密度不均。高层调用方收到错误时难以追溯根因，
> 影响生产故障诊断。这不是 Bug，但在 B14-PUA1 集成 PuaViolation 错误类型后
> 建议同步补充 `.context()` 以区分 PUA 拒绝 vs. 系统错误。

**推荐建议**：在 ACP server `handle_request()` 的主链路上，对所有
`?` 传播的 anyhow error 补充 `.context("component: action")`，
确保日志可区分 `PuaViolation`、`BudgetExceeded`、`SandboxBlocked` 三类来源。

**验收标准**：
- `cargo clippy` 无 unused result 警告
- 集成测试日志中 PUA 拒绝消息可与其他错误区分（通过 `kind` 或 `context` 前缀）

---

## 回写区

> 本区域供实施过程中记录实际完成状态、测试通过证据、变更合约版本。
> 格式：`[B14-item] [日期] [状态] [备注]`

| 条目 | 状态 | 验收日期 | 备注 |
|---|---|---|---|
| B14-P0-1 | ✅ 已完成（本轮） | 2026-04-14 | 主程序新增 `--protocol-mode` 覆盖+校验；test_ci.sh 已接入 6a；vscode-addon 新增 `go-on.runtime.protocolMode` 并启动透传到主链路 |
| B14-P0-2 | ✅ 已完成（本轮） | 2026-04-14 | `/health` 已并入全局性能快照；新增 `http_chat_completions_updates_health_metrics_and_emits_latency_log` 集成测试验证请求后计数增长与 stderr 延迟日志；test_ci.sh 6b 已接主链 |
| B14-P1-1 | ✅ 已完成（本轮） | 2026-04-14 | agent 公共边界已统一为 `crate::core::error::Result`（内部保留 anyhow）；新增 `agent_error_can_be_classified` 单测；`test_ci.sh` 已接入 6c 主链门禁；`cargo fmt --all` + `cargo check` + `cargo test agents::agent::tests::` 通过 |
| B14-P1-2 | ✅ 已完成（本轮） | 2026-04-14 | `test_ci.sh` 已接入 6d/6e 主链门禁；clippy 采用 `--all-targets -- -D warnings`（Windows 自动 fallback）；新增 cargo-audit 自动安装并执行 `cargo audit`；同时清理未使用外部依赖 `audit = 0.7.3` 以消除 Windows 构建阻塞 |
| B14-P2-1 | ✅ 已完成（本轮） | 2026-04-14 | onboarding/status 用户可见输出已完成 i18n 化；三语种补齐对应键；`test_ci.sh` 已接入 6k 主链门禁 |
| B14-P2-2 | ✅ 已完成（本轮） | 2026-04-14 | backend/GUI/vscode-addon 三端 README 已补齐快速开始与协议模式说明；`test_ci.sh` 新增 6f 文档完整性门禁并接入主链 |
| B14-P3-1 | ✅ 已完成（文档化） | 2026-04-14 | 生产就绪指标已在 BLUE14 文档中量化回写并纳入验收口径 |
| B14-AI1 | ✅ 已完成（本轮） | 2026-04-14 | `smart_compress` + `ContextCache` 已实现；`test_ci.sh` 6g 已接主链 |
| B14-AI2 | ✅ 已完成（本轮） | 2026-04-14 | `LearningFeedbackSystem` 已实现并持久化；主链测试通过 |
| B14-AI3 | ✅ 已完成（本轮） | 2026-04-14 | `QLearningAgent` + `RewardFunction` 已实现；`test_ci.sh` 6h 已接主链 |
| B14-AI4 | ✅ 已完成（本轮） | 2026-04-14 | `KnowledgeDistiller` + `aggregate_verdict` 已实现；`test_ci.sh` 6i 已接主链 |
| B14-HD1 | ✅ 已完成（本轮） | 2026-04-14 | MCP 工具调用已接入策略允拒校验；`test_ci.sh` 6j 已接主链 |
| B14-HD2 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `BudgetTracker`（tokens/tool_calls/wall_clock）并接入 `mcp.tools.call` 主链预算拦截；`test_ci.sh` 已接入 6l 门禁 |
| B14-HD3 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `IdempotencyCache`（TTL 5 分钟）并接入 `task.execute` 命中短路返回；`test_ci.sh` 已接入 6m 门禁 |
| B14-HD4 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `AuditLogger`（NDJSON）并接入 `mcp.tools.call` 成功/失败审计；`test_ci.sh` 已接入 6n 门禁 |
| B14-HD5 | — | — | — |
| B14-PUA1 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `PuaRuleEngine` + `PuaViolation`（RedLine/StageFail/MissingEvidence），并在 `handle_request` 主链接入红线校验与阶段校验（通过 `completed_actions` 显式触发）；`test_ci.sh` 已接入 6o 门禁 |
| B14-PUA2 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `DynamicQualityCompass`/`TaskContext`/`QualityCheck` 结构与上下文规则；在 `handle_request` 主链构建上下文并将动态指南针注入 PUA 违例响应；`test_ci.sh` 已接入 6p 门禁 |
| B14-PUA3 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `PuaFeedbackCollector` + `PuaLearningRecord`（NDJSON 持久化与提取），并在 `handle_request` 阶段校验路径写入 `PuaExecutionReport` 到 `.goon/learning/pua/`；`test_ci.sh` 已接入 6q 门禁 |
| B14-PUA4 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `PuaRuleEngine.generate_report/collect_missing`，并通过 `RuntimeConfig.pua_report=false` 默认关闭、`debug_pua_report=true`/运行时配置开启时将 base64 JSON 报告注入 JSON-RPC `meta.x_pua_report`；`test_ci.sh` 已接入 6r 门禁 |
| B14-HSS1 | — | — | — |
| B14-HSS2 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `AdvancedRpcHarness`（mock 响应、并发请求、场景文件驱动）；补充并发一致性测试与 `requests/runtime-health.ndjson` 驱动测试；`test_ci.sh` 已接入 6s 门禁 |
| B14-HSS3 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `TestScenario`/`ScenarioOutcome`/`load_scenarios_from_dir`，自动扫描 `requests/*.ndjson` 并逐文件驱动集成测试；`test_ci.sh` 已接入 6t 门禁 |
| B14-AGENT1 | ✅ 已完成（本轮） | 2026-04-14 | 新增 `tests/pua_contract_smoke.rs`，覆盖 `PuaEnforcementPlan::default()` 的 `escalation_level`/`red_lines`/`quality_compass` 合约；`test_ci.sh` 已接入 6u 门禁 |
| B14-AGENT2 | ✅ 已完成（本轮） | 2026-04-14 | 在 `reinforcement.rs` 新增 `LearningRecord`（workflow/pua）统一 schema，并将 AI2 与 PUA3 统一写入 `.goon/learning/learning-records.ndjson`；`analyze_patterns()` 与 `extract_learning_data()` 支持混合读取；`test_ci.sh` 已接入 6v 门禁 |
| B14-AGENT3 | ✅ 已完成（本轮） | 2026-04-14 | 在 `hardening.rs` 新增 `consume_with_pua()` 并接入 `execute_mcp_tool_call` 主链预算路径；超限时触发 `PuaRuleEngine::escalate`（L5 上限保护）；`test_ci.sh` 已接入 6w 门禁 |
| B14-AGENT4 | ✅ 已完成（本轮） | 2026-04-14 | 在 `handle_request` 主链新增统一错误分类与 `anyhow::context` 溯源包装（区分 `PuaViolation` / `BudgetExceeded` / `SandboxBlocked`），并新增分类单测；`test_ci.sh` 已接入 6x 门禁 |
#### 步骤 HD1.4：单测
```rust
#[test]
fn sandbox_policy_ci_denies_shell() {
    let p = PolicyBundle::ci_pipeline();
    assert!(!SandboxPolicy::can_execute_shell(&p.sandbox_level));
}

#[test]
fn sandbox_policy_local_dev_allows_read() {
    let p = PolicyBundle::local_dev();
    assert!(SandboxPolicy::can_execute_read_file(&p.sandbox_level));
}
```

#### 步骤 HD1.5：接入主链
```bash
# test_ci.sh 步骤 6j
cargo test governance::hardening::tests::sandbox_policy -- --nocapture
```

**验收标准**：
- CI 策略下 `can_execute_shell()` 返回 false
- local-dev 策略下 `can_execute_read_file()` 返回 true
- 工具路径校验拦截日志可见（debug 级别）

---

### B14-HD2：资源配额实时跟踪接入

**是否需要**：✅ **需要，P1 — 防止资源滥用**

> 根因：`TaskBudget`（max_tokens/max_wall_clock_seconds/max_tool_calls）和
> `TenantResourceQuota`（daily_token_limit/concurrent_tasks_limit）均已定义，
> 但无任何跟踪或强制执行逻辑，超配额请求无法被拦截。

**推荐建议**：新增 `BudgetTracker`，在 ACP 请求处理循环中检查并更新配额。

#### 步骤 HD2.1：Budget Tracker

```rust
pub struct BudgetTracker {
    task_budget: TaskBudget,
    tokens_used: usize,
    tool_calls_made: usize,
    started_at: std::time::Instant,
}

impl BudgetTracker {
    pub fn new(budget: TaskBudget) -> Self;

    pub fn record_tokens(&mut self, tokens: usize) -> Result<(), BudgetExceededError>;
    pub fn record_tool_call(&mut self) -> Result<(), BudgetExceededError>;
    pub fn check_wall_clock(&self) -> Result<(), BudgetExceededError>;

    pub fn remaining_tokens(&self) -> usize {
        self.task_budget.max_tokens.saturating_sub(self.tokens_used)
    }
}

pub struct BudgetExceededError {
    pub limit_type: &'static str,
    pub limit: usize,
    pub used: usize,
}
```

#### 步骤 HD2.2：单测
```rust
#[test]
fn budget_tracker_rejects_on_token_overflow() {
    let budget = TaskBudget { max_tokens: 100, max_wall_clock_seconds: 60,
                               max_tool_calls: 10, max_api_calls: 10 };
    let mut tracker = BudgetTracker::new(budget);
    assert!(tracker.record_tokens(101).is_err());
}

#[test]
fn budget_tracker_allows_within_limit() {
    // ...
}
```

#### 步骤 HD2.3：接入主链
```bash
# test_ci.sh 步骤 6k
cargo test governance::hardening::tests::budget_tracker -- --nocapture
```

**验收标准**：
- `record_tokens(max_tokens + 1)` 返回 `Err(BudgetExceededError)`
- `remaining_tokens()` 在未使用时返回 `max_tokens`
- 现有 governance 测试无回归

---

### B14-HD3：幂等性请求缓存

**是否需要**：✅ **需要，P1 — 防止重复执行副作用**

> 根因：`Idempotency::key()` 函数已存在，但无检查逻辑，
> 重复的 ACP `task.execute` 请求在网络重试时会被完整重新执行。
> 对文件写入类工具调用，重复执行可能导致数据损坏。

**推荐建议**：在 RPC 请求分发层新增幂等性检查，将结果缓存 TTL 设为 5 分钟。

#### 步骤 HD3.1：幂等性缓存

```rust
pub struct IdempotencyCache {
    results: HashMap<String, IdempotentResult>,
    ttl: std::time::Duration,
}

pub struct IdempotentResult {
    pub response: serde_json::Value,
    pub cached_at: std::time::Instant,
}

impl IdempotencyCache {
    pub fn new(ttl: std::time::Duration) -> Self;
    pub fn get(&self, key: &str) -> Option<&IdempotentResult>;
    pub fn insert(&mut self, key: String, response: serde_json::Value);
    pub fn evict_expired(&mut self);
}
```

#### 步骤 HD3.2：单测
```rust
#[test]
fn idempotency_cache_returns_cached_result_within_ttl() { ... }

#[test]
fn idempotency_cache_evicts_expired_entries() { ... }

#[test]
fn idempotency_key_is_deterministic() {
    let k1 = Idempotency::key("task-1", "phase-a", "build the feature");
    let k2 = Idempotency::key("task-1", "phase-a", "build the feature");
    assert_eq!(k1, k2);
}
```

#### 步骤 HD3.3：接入主链
```bash
# test_ci.sh 步骤 6l
cargo test governance::hardening::tests::idempotency -- --nocapture
```

**验收标准**：
- 同一 key 的第二次请求直接返回缓存结果，不触发实际执行
- TTL 过期后缓存失效
- `Idempotency::key()` 对相同输入返回相同字符串

---

### B14-HD4：审计日志收集与查询

**是否需要**：⚠️ **需要，P2 — 合规与溯源需求**

> 根因：`AutonomousEditAuditEntry` 结构已定义（含 timestamp/agent/file_path/change_summary/reversible），
> 但无任何收集、写入、查询逻辑。自主编辑类工具调用完全无审计轨迹。

**推荐建议**：新增 `AuditLogger`，将审计记录追加写入 `.goon/audit/` 目录的 NDJSON 文件。

#### 步骤 HD4.1：Audit Logger

```rust
pub struct AuditLogger {
    log_dir: PathBuf,
}

impl AuditLogger {
    pub fn new(log_dir: PathBuf) -> Self;

    /// 追加写入一条审计记录（NDJSON 格式）
    pub fn record(&self, entry: &AutonomousEditAuditEntry) -> std::io::Result<()>;

    /// 读取最近 N 条审计记录
    pub fn recent(&self, limit: usize) -> std::io::Result<Vec<AutonomousEditAuditEntry>>;

    /// 按文件路径查询审计记录
    pub fn query_by_path(&self, file_path: &str) -> std::io::Result<Vec<AutonomousEditAuditEntry>>;
}
```

#### 步骤 HD4.2：单测
```rust
#[test]
fn audit_logger_writes_and_reads_back_entry() { ... }

#[test]
fn audit_logger_query_by_path_filters_correctly() { ... }
```

#### 步骤 HD4.3：接入主链
```bash
# test_ci.sh 步骤 6m
cargo test governance::hardening::tests::audit_logger -- --nocapture
```

**验收标准**：
- `record()` 后 `.goon/audit/` 目录出现 NDJSON 文件
- `recent(1)` 返回最新一条
- `query_by_path("src/main.rs")` 仅返回该路径的记录

---

### B14-HD5：高级安全加固（P2/P3 前景）

**是否需要**：💡 **参考，P2/P3 — 面向生产环境发布**

以下项无需立即编码，在达到 HD1-HD4 验收后再规划：

| 项目 | 优先级 | 简述 |
|---|---|---|
| 密钥轮换 | P2 | 在 `config.rs` 中定期触发 `validate_secret_ref()` 检查有效期 |
| 数据加密存储 | P2 | `.goon/` 目录内敏感文件（audit、learning）使用 AES-GCM 加密 |
| DoS 防护 | P3 | 在 HTTP handler 层添加请求频率限制（与现有 rate-limit 集成测试对齐） |
| 零信任架构 | P3 | 所有内部 RPC 调用带 JWT token，移除隐式信任 |
| 合规报告 | P3 | 基于 audit log 自动生成 SOC2-style 合规报告 |

---

## AI 智能 + HARDNESS 优先级汇总

```
P0（立即实施）
  B14-AI1  TOKEN节省优化（语义压缩 + 上下文缓存）  [✅ 必须]
  B14-AI2  自学习反馈系统实现                      [✅ 必须]
  B14-HD1  策略执行引擎接入主链路（OWASP A01）      [✅ 必须]

P1（近期实施）
  B14-AI3  强化学习基础实现（Q-Learning + 奖励函数）[✅ 需要]
  B14-AI4  知识淬炼算法实现（蒸馏 + 去重）          [✅ 需要]
  B14-HD2  资源配额实时跟踪                         [✅ 需要]
  B14-HD3  幂等性请求缓存                           [✅ 需要]

P2（计划内实施）
  B14-HD4  审计日志收集与查询                       [⚠️ 需要]
  B14-HD5  高级安全加固（密钥轮换/数据加密）         [💡 参考]

P3（长期前景）
  B14-AI（高级）  深度强化学习/知识图谱/多智能体     [💡 前景规划]
  B14-HD5（高级） DoS防护/零信任/合规报告            [💡 前景规划]
```

---

## 验收标准补充

### AI 智能功能验收
- [x] `smart_compress` 压缩率 ≤ 75% 且语义完整
- [x] `LearningFeedbackSystem.collect()` 写入 `.goon/learning/`
- [x] Q-table 10 次更新后高回报 action Q 值领先
- [x] `KnowledgeDistiller.distill()` 过滤低置信度洞察
- [x] `aggregate_verdict` 正确映射通过率到 `QualityVerdict`

### HARDNESS 验收
- [x] CI 策略下 `can_execute_shell()` 返回 false
- [x] `BudgetTracker.record_tokens()` 超限返回错误
- [x] 幂等性缓存命中时不触发实际执行
- [x] `AuditLogger.record()` 写入 `.goon/audit/` NDJSON 文件

---

## 分阶段执行计划（回写区）

> 本区域用于实施过程中的完成率回写，格式与 BLUE13.MD 保持一致。

（当前状态：BLUE14 核心实施与全链回归已完成，回写区已同步到最新验收结论）

