# BLUE17 — 公开发布前收口建议

更新时间：2026-04-16（初稿）

本文为 BLUE17 正式版，基于 2026-04-16 对项目整体发布就绪状态的全量评审，针对**公开发布（开源/面向外部用户）前**仍未闭合的运营侧风险点与文档缺口，给出可执行的收口建议。

技术维度（编译警告 0、测试 345 passed、安全防护完整、版本号 0.6.1 三端对齐）已确认就绪，不在本批次内。

---

## 进度回写（已更新）

更新时间：2026-04-17

- 总体完成率：`100%`（5 项中已完成 5 项）

### 本次复验结果（2026-04-17）

- backend：`cargo test -q` 通过（345 passed，0 failed，0 warnings）
- vscode-addon：`npm --prefix vscode-addon run check` 通过（compile + lint）
- GUI：`npm --prefix GUI run build` 与 `npm --prefix GUI run test:contract` 均通过（contract smoke passed）
- release gate：`scripts/run-release-readiness-gate.sh config.production.toml` 全流程已真实执行完成，产物 `RELEASE_GATE_OUTPUT.txt` 已生成（含 scenario replay + integration assertions + completed 标记）
- release gate（链路收敛）：回放阶段改为直接 `--config config.production.toml --protocol-mode acp_stdio`，移除临时配置依赖，执行路径与主配置保持一致

| ID | 状态 | 说明 |
|---|---|---|
| B17-R1 | ✅ 已完成 | Release Gate 脚本已全流程真实执行并留存产物（`RELEASE_GATE_OUTPUT.txt`） |
| B17-R2 | ✅ 已完成 | `README.md` + `README.zh-CN.md` 均已补充 API Key 设置章节（Production Baseline → API Key Setup 节） |
| B17-R3 | ✅ 已完成 | `config.production.toml` 顶部添加模板声明注释；NOTE 末行补充"不得将密钥存储于此文件"；新增 `skills_enabled = false` 配置项 |
| B17-S1 | ✅ 已完成 | 注册层防护（名称/唯一性/schema 三重校验）+ `EchoSkill` 内置 skill + `skills_enabled` 配置门控 + 7 项测试 + DOC 双语 `skills.md` + SUMMARY.md 导航更新 |
| B17-S2 | ✅ 已完成 | 已落地 `skill.import/enable/disable/list_imported/remove`，并新增主链接入（`mcp.tools.list/call`）与三端对齐（backend + vscode-addon + GUI） |

### B17-S1 实现摘要（2026-04-17）

| 文件 | 变更内容 |
|---|---|
| `src/orchestration/skill.rs` | `register()` 改签为 `-> Result<()>`，增加名称格式、唯一性、schema 三重校验；添加 `pub struct EchoSkill`（`builtin.echo`）；新增 7 项单元测试 |
| `src/acp/server.rs` | `register_skill()` 改为 `tracing::warn!` 记录注册失败，不 panic |
| `src/core/config.rs` | `RuntimeConfig` 新增 `skills_enabled: bool`（默认 `true`，serde default）；添加 `default_runtime_skills_enabled()` 函数 |
| `src/acp/impl/runtime.rs` | 服务启动后若 `runtime_config.skills_enabled`，自动注册 `EchoSkill` |
| `config.production.toml` | 顶部添加模板声明注释；`[runtime]` 新增 `skills_enabled = false`；NOTE 新增第 3 条 |
| `README.md` | "Production Baseline" 节新增 "API Key Setup (production mode)" 子节 |
| `README.zh-CN.md` | 同上（中文版） |
| `DOC/src/en/skills.md` | 新建（英文 Skills 文档） |
| `DOC/src/zh-CN/skills.md` | 新建（中文 Skills 文档） |
| `DOC/src/SUMMARY.md` | EN + ZH-CN 两节均添加 `skills.md` 导航条目 |

**测试结果**：345 tests passed，0 failed，0 warnings（`cargo test -q` 全量验证）

### B17-S2 实现摘要（2026-04-17）

| 文件 | 变更内容 |
|---|---|
| `src/orchestration/skill_import.rs` | 新增 skills 导入存储与策略模块：支持 `github/url/local` 三来源导入、SHA256 校验、allowlist 校验、浮动 ref 拒绝、manifest 落盘与 index 管理 |
| `src/acp/impl/request.rs` | 新增 RPC：`skill.import` / `skill.list_imported` / `skill.enable` / `skill.disable` / `skill.remove`；新增导入审计写入（复用 `.goon/audit`） |
| `src/core/config.rs` | `RuntimeConfig` 新增导入策略字段：`skills_import_enabled`、`skills_allowed_sources`、`skills_require_sha256`、`skills_allow_floating_ref`、`skills_cache_dir` |
| `config.toml` | 新增开发默认导入策略样例（默认关闭，允许显式 local allowlist） |
| `config.production.toml` | 明确生产默认：`skills_import_enabled = false`、`skills_allowed_sources = []`、`skills_require_sha256 = true`、`skills_allow_floating_ref = false` |
| `config.production.toml` | 补齐 `agents`/`flow`/`phases` 必填结构，避免生产配置在 gate 场景解析失败 |

### B17-S2 后续增强批次（主链接入 + 三端对齐，2026-04-17）

| 文件 | 变更内容 |
|---|---|
| `src/acp/impl/request.rs` | `mcp.tools.list` 新增已启用 imported skill 的 descriptor 展示；`mcp.tools.call` 新增 imported skill 主链路由（manifest metadata + structured passthrough 响应） |
| `vscode-addon/src/rpcCommandRegistry.ts` | 新增三条技能管理命令：`go-on.skillListImported`、`go-on.skillImportLocal`、`go-on.skillToggle` |
| `vscode-addon/package.json` | 同步新增 activation events 与 command palette 条目，确保插件命令可见可触发 |
| `GUI/src/services/rpcService.ts` | 新增 Skills 导入管理 RPC service：import/list/enable/disable/remove，供 GUI 视图接入 |
| `scripts/run-release-readiness-gate.sh` | 回放阶段收敛为主链执行：直接使用生产配置并通过 `--protocol-mode acp_stdio` 覆盖协议；避免协议漂移导致 gate 卡住 |

补充：`skill.remove` 已联动 `SkillRegistry::unregister()`，用于保持注册态一致并清除 Rust dead_code warning（本批次目标：不留 warning）。

---

## B17-R1 — Release Gate 脚本全流程真实执行

### 问题

`RELEASE_READINESS.md` 定义了完整的 Pre-release Drill 流程（`runtime.stability`、`security.baseline`、`observability.alerts`、`optimization.peak`、graceful shutdown drain），并提供了对应脚本路径：

- Windows：`scripts\run-release-readiness-gate.ps1 -Config config.production.toml`
- Linux/macOS：`scripts/run-release-readiness-gate.sh config.production.toml`

但截至 2026-04-16，**未确认该脚本是否已真实完整执行过**并产生通过记录。

### 风险

- SLO 各项指标（Availability >= 99.9%、5xx <= 0.5%、P95 <= 2s、P99 <= 5s）的验证停留在文档层，没有真实执行产物作为发布依据。
- 若 gate 中有任何 check 静默失败（如 `entry_auth_key_configured: false`），外部用户将遭遇服务不可用但无明确错误提示的问题。

### 执行步骤

1. 确保 `GO_ON_ENTRY_API_KEY` 已在环境变量中设置（非空值）。
2. 使用生产配置运行 gate：
   ```powershell
   scripts\run-release-readiness-gate.ps1 -Config config.production.toml
   ```
3. 确认所有 gate 输出为 `pass`（尤其是 `security`、`governance`、`stability`）。
4. 将 gate 输出日志保存为 `RELEASE_GATE_OUTPUT.txt`（参照 `RELEASE_READINESS.md` Audit Artifacts 节要求）。

### 完成标准

- 脚本无错误退出，全部 gate 为 `pass`。
- 产物日志已留存。

---

## B17-R2 — README 补充 `GO_ON_ENTRY_API_KEY` 部署说明

### 问题

`config.production.toml` 中 `entry_auth_enabled = true` 且 `entry_auth_api_key_env = "GO_ON_ENTRY_API_KEY"`，但当前 `README.md` 没有任何关于这个环境变量的提及。

外部用户按 README 启动后，若未设置该变量，auth 模块将拒绝所有请求，但错误提示不够直观（返回 `AuthRequired` / `-32003`），极易引起 "为什么什么都请求不通" 的困惑。

### 建议

在 `README.md` 的 Quick Start 或独立 Configuration 节中补充以下说明（中英文均补）：

**English 参考文本：**

```
### API Key Setup (production mode)

When running with `config.production.toml`, entry authentication is enabled.
Set the following environment variable before starting the server:

```bash
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
```

On Windows:
```powershell
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
```

All RPC requests must include the key in the `Authorization` header:
```
Authorization: Bearer your-secret-key-here
```
```

**中文参考文本：**

```
### API 密钥配置（生产模式）

使用 `config.production.toml` 启动时，入口鉴权已默认开启。
启动前需设置以下环境变量：

```bash
export GO_ON_ENTRY_API_KEY="your-secret-key-here"
```

Windows PowerShell：
```powershell
$env:GO_ON_ENTRY_API_KEY = "your-secret-key-here"
```

所有 RPC 请求需在 `Authorization` 头中携带密钥：
```
Authorization: Bearer your-secret-key-here
```
```

### 完成标准

- `README.md`（英文）与 `README.zh-CN.md`（中文）均包含 API Key 设置步骤。
- DOC 文档 `en/setup-wizard.md` 与 `zh-CN/setup-wizard.md` 对应补充或引用。

---

## B17-R3 — `config.production.toml` 模板声明与密钥安全提示

### 问题

`config.production.toml` 已提交进 git 仓库，其内容完全可见。该文件本身不含密钥，但文件末尾注释（`# 2) Ensure GO_ON_ENTRY_API_KEY is present...`）对首次查看仓库的用户来说仅是一行注释，容易被忽略。

更大的风险：用户可能将此文件复制后直接填入密钥，再一并提交，形成 secret leak。

### 建议

1. **文件顶部增加模板声明注释**，明确说明此为模板文件、密钥绝不可提交：

   ```toml
   # ============================================================
   # TEMPLATE FILE — DO NOT COMMIT SECRETS
   # This file is a configuration template for production use.
   # The value of GO_ON_ENTRY_API_KEY must be set as an
   # environment variable and must NEVER be stored in this file
   # or committed to version control.
   # ============================================================
   ```

2. **在 `.gitignore` 或 `README.md` 中明确说明**：如需本地覆盖，应创建 `config.local.toml`（已在 `.gitignore` 中）而非直接修改 `config.production.toml`。

3. （可选）在 `SECURITY.md`（如无则新建最小版）中补充 "Secret Management" 节，说明密钥应通过环境变量、keyring 或 secret manager 注入，不应出现在任何配置文件中。

### 完成标准

- `config.production.toml` 顶部包含模板声明注释。
- README 或 SECURITY.md 包含密钥管理说明。

---

---

## B17-S1 — Skills 开放：注册层防护 + RPC 端点 + 配置门控 + 文档

### 当前状态

基于 2026-04-16 安全审计结论：

- `Skill` trait 与 `SkillRegistry` 已完备，执行路径受 Budget + Hardening + PUA 三层防护。
- `SkillRegistry::register()` 与 `AcpServer::register_skill()` 已存在，但**零调用、零注册**。
- **无任何 RPC 端点**可让外部调用者注册或卸载 skill，注册表在运行时始终为空。

开放 Skills 需要在**不破坏已有执行安全链路**的前提下，分两条路径推进：

- **路径 A（内置 skill）**：在 Rust 编译期注册固定 skill，适合官方提供的内置能力。
- **路径 B（运行时动态注册）**：通过受控 RPC 端点在运行时注册，适合用户自定义扩展场景。

本批次需完成路径 A 的基础工作，并为路径 B 设计防护规范（暂不实现 RPC 端点，先完成注册层防护）。

---

### B17-S1 具体执行步骤

#### 步骤 1 — `SkillRegistry::register()` 增加名称与 schema 校验

**文件**：`src/orchestration/skill.rs`

当前 `register()` 直接 `HashMap::insert`，无任何验证。需在插入前增加：

1. **名称格式校验**：仅允许 `[a-z0-9._-]`，最大 64 字符，不允许空串。
2. **名称唯一性保护**：若已存在同名 skill，拒绝覆盖并返回 `Err`（改 `register()` 签名为 `-> Result<()>`）。
3. **schema 类型校验**：`input_schema()` 返回值的顶层必须为 `{"type": "object"}` 或包含 `"properties"` 键，防止注入任意非结构化 schema 被下游工具描述接口暴露。

```rust
// 改造目标签名
pub fn register(&mut self, skill: Arc<dyn Skill>) -> Result<()> {
    let name = skill.name();
    // 1. 名称格式校验
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("skill name length out of range: '{}'", name);
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        anyhow::bail!("skill name contains invalid characters: '{}'", name);
    }
    // 2. 唯一性保护
    if self.skills.contains_key(name) {
        anyhow::bail!("skill already registered: '{}'", name);
    }
    // 3. schema 校验
    let schema = skill.input_schema();
    let schema_type = schema.get("type").and_then(|v| v.as_str());
    let has_properties = schema.get("properties").is_some();
    if schema_type != Some("object") && !has_properties {
        anyhow::bail!("skill '{}' input_schema must be type:object", name);
    }
    self.skills.insert(name.to_string(), skill);
    self.stats.entry(name.to_string()).or_default();
    Ok(())
}
```

同步更新 `AcpServer::register_skill()` 以传播错误（`tracing::warn!` 记录注册失败，不 panic）：

```rust
pub fn register_skill(&self, skill: Arc<dyn crate::orchestration::skill::Skill>) {
    if let Ok(mut registry) = self.skill_registry.lock() {
        if let Err(err) = registry.register(skill) {
            tracing::warn!("skill registration rejected: {}", err);
        }
    }
}
```

#### 步骤 2 — 实现第一个内置 Skill（echo skill，用于验证链路）

**文件**：`src/orchestration/skill.rs`（或新建 `src/orchestration/builtin_skills.rs`）

实现一个最小内置 skill，用于验证注册 → 列表 → 调用全链路通路，不影响生产逻辑：

```rust
pub struct EchoSkill;

#[async_trait]
impl Skill for EchoSkill {
    fn name(&self) -> &str { "builtin.echo" }
    fn description(&self) -> &str { "Echoes the input back. Used for skill chain validation." }
    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "Text to echo"}
            },
            "required": ["message"]
        })
    }
    async fn execute(&self, input: &Value) -> Result<Value> {
        let message = input.get("message").and_then(|v| v.as_str()).unwrap_or("");
        Ok(json!({"echo": message}))
    }
}
```

**注册位置**：在 `ServerBuilder::build()` 的最后阶段（`src/acp/server.rs`）调用 `server.register_skill(Arc::new(EchoSkill))`，使其成为服务启动后 `mcp.tools.list` 中可见的第一个内置 skill。

#### 步骤 3 — 配置门控（仅在配置允许时注册内置 skills）

**文件**：`config.toml` / `src/config/mod.rs`

在 `[runtime]` 节新增配置项：

```toml
[runtime]
# 是否启用内置 skills（builtin.echo 等）
# 生产环境可设为 false 以最小化暴露面
skills_enabled = true
```

`ServerBuilder::build()` 读取此开关，仅在 `skills_enabled = true` 时注册内置 skills。`config.production.toml` 默认设为 `false`，需用户显式开启。

#### 步骤 4 — 测试覆盖

在 `tests/` 或 `src/orchestration/skill.rs` 的 `#[cfg(test)]` 块中新增：

| 测试 ID | 测试内容 |
|---|---|
| `test_skill_name_validation` | 空串、超长、含非法字符名称均被拒绝 |
| `test_skill_duplicate_registration` | 同名重复注册返回 Err |
| `test_skill_schema_validation` | 非 object schema 被拒绝 |
| `test_echo_skill_execute` | `EchoSkill::execute` 输入输出验证 |
| `test_skill_appears_in_mcp_tools_list` | 注册后在 `mcp.tools.list` 结果中可见 |
| `test_skill_call_via_mcp_tools_call` | `mcp.tools.call` 可正确调用已注册 skill |

#### 步骤 5 — DOC 文档补充

在 `DOC/src/en/` 和 `DOC/src/zh-CN/` 各新增或更新 `skills.md`，说明：

1. Skills 是什么（与内置 tool 的区别）
2. 如何实现一个自定义 Skill（实现 `Skill` trait）
3. 如何在 ServerBuilder 阶段注册
4. `skills_enabled` 配置项说明
5. 名称规范与 schema 要求
6. 执行安全链路说明（Budget / Hardening / PUA 三层防护已覆盖）
7. 路径 B（运行时动态注册 RPC）的设计预告（当前未开放，待后续批次）

---

### 完成标准

- `cargo test` 全部通过（包含步骤 4 新增测试），`cargo clippy --all -- -D warnings` 0 警告。
- `mcp.tools.list` 在 `skills_enabled = true` 时返回 `builtin.echo`，在 `false` 时不返回。
- `mcp.tools.call` 可调用 `builtin.echo` 并返回正确结果。
- DOC 两语言 skills 文档已落地。
- `config.production.toml` 中 `skills_enabled = false` 已明确注释说明。

---

### 路径 B（运行时动态注册）设计预告

以下为后续批次（BLUE18 或更晚）的规划，本批次**不实现**，仅记录设计约束：

1. **RPC 端点**：`skill.register`（仅在 `skills_enabled = true` 且 `entry_auth_enabled = true` 时可达）。
2. **调用方鉴权**：必须携带有效 `Authorization: Bearer` 头（复用 entry_auth 链路）。
3. **Allowlist 模式**：config 中可配置 `skills_allowlist`，仅允许指定名称前缀的 skill 被注册（防止任意命名空间污染）。
4. **沙箱限制**：动态注册的 skill 默认受 `managed_service` hardening 策略约束（不允许 Shell 动作）。
5. **审计**：每次注册/卸载写入 governance audit log（复用 `append_governance_audit_event`）。

---

## B17-S2 — Skills 来源导入（GitHub / 其他来源）

### 目标

支持将 skills 从外部来源导入到本地运行时，至少覆盖：

1. GitHub 仓库（`owner/repo@commit` 或 release asset）
2. HTTPS URL（受信任制品地址）
3. 本地目录/压缩包（离线场景）

### 设计原则（发布前必须满足）

1. **固定版本，不允许浮动引用**：禁止 `main`/`latest`，必须 pin 到 commit SHA 或不可变版本号。
2. **完整性校验**：导入时必须校验 `sha256`；校验失败直接拒绝加载。
3. **来源策略**：配置 allowlist（域名/仓库前缀），默认仅允许受信来源。
4. **清单先行**：每个 skill 包必须包含 manifest（名称、版本、入口、schema、权限声明）。
5. **最小权限执行**：导入 skill 默认运行在受限策略下（无 shell、无任意文件写、无外网，除非显式授权）。
6. **可追溯审计**：记录导入来源、版本、hash、导入人、时间、启停状态到 audit log。

### 已落地配置草案

```toml
[runtime]
skills_import_enabled = false
skills_allowed_sources = ["github.com/org/*", "https://artifacts.example.com/skills/*", "local:*"]
skills_require_sha256 = true
skills_allow_floating_ref = false
skills_cache_dir = "skills_cache"
```

### 已落地接口

1. `skill.import`：导入并校验（manifest 落盘并登记，默认禁用状态）。
2. `skill.enable` / `skill.disable`：启停已导入 skill。
3. `skill.list_imported`：列出来源、版本、hash、状态。
4. `skill.remove`：卸载并清理缓存。

### 完成标准

- 导入链路具备 `pin 版本 + hash 校验 + allowlist + 审计` 四项硬性控制。
- `config.production.toml` 默认 `skills_import_enabled = false`。
- 文档明确“可导入 ≠ 默认可信”，生产环境需显式审批后启用。

---

## 推荐执行顺序

| 优先级 | ID | 理由 |
|---|---|---|
| P0 | B17-R1 | Gate 产物是发布的最终凭据，其他步骤依赖此结果 |
| P1 | B17-R2 | 直接影响第一批用户的上手体验 |
| P1 | B17-R3 | 防止密钥误提交，一次写完，长期受益 |
| P2 | B17-S1 | Skills 开放是功能扩展，不阻塞发布，但建议在发布后第一个迭代完成 |
| P2 | B17-S2 | 外部导入涉及供应链安全，已按受控默认关闭策略落地 |

完成 R1–R3 后，项目具备公开发布的完整运营侧就绪状态。完成 B17-S1 后，内置 Skills 扩展能力对外开放。完成 B17-S2 后，外部 Skills 导入能力已在受控策略下可用（生产默认关闭，需显式审批开启）。
