# Agent World 邀请接入 · 端到端测试报告

**日期:** 2026-07-09  
**测试文件:** `tests/agent_world_invitation_test.rs`  
**测试目标:** 验证 go-on 后端正确检测邀请 URL、解析 fragment、调用 API 获取任务包、并将原始数据呈现给 AI 的完整管道  

## 测试结果概览

```
7 passed; 0 failed; 0 ignored
```

| # | 测试 | 阶段 | 结果 | 说明 |
|---|------|------|------|------|
| 1 | `test_extract_url_from_plain_text` | URL 提取 | ✅ | 自然语言消息中的 URL 被正确提取，包含 fragment |
| 2 | `test_extract_url_from_markdown_link` | URL 提取 | ✅ | Markdown 链接格式 `[text](url)` 正常提取 |
| 3 | `test_extract_url_from_code_block` | URL 提取 | ✅ | 代码块中的 URL 正常提取 |
| 4 | `test_url_fragment_and_path_parsing` | URL 解析 | ✅ | fragment 分割、`task_access_token` 提取、`invite_xxx` 检测全部正确 |
| 5 | `test_api_url_construction` | API 构造 | ✅ | 生成的 API 端点 URL 与 curl 实测一致 |
| 6 | `test_live_task_package_fetch` | **真实 API 调用** | ✅ | 成功调用 Agent World API、返回 11 个必填字段 |
| 7 | `test_context_construction_is_neutral` | 上下文检查 | ✅ | 上下文中无硬编码指令性内容 |

## 详细测试过程

### Phase 1: URL 自动检测

模拟用户发送邀请链接消息，`extract_url()` 函数正确提取：

```
输入: "请完成这个邀请：https://agent-world-test.chuanshuo.com.cn/agent-invite/invite_xxx#task_access_token=task_xxx"
输出: 完整 URL 包含 fragment（提供给 LLM 但 HTTP 请求时不发送）
```

### Phase 2: URL 解析

| 步骤 | 代码逻辑 | 验证结果 |
|------|---------|---------|
| Fragment 分割 | `url.split('#').next()` | ✅ 提取到干净 URL 不含 `#` |
| Fragment 参数 | `url::form_urlencoded::parse` | ✅ 正确解析 `task_access_token` |
| Path segments | `split('/').filter(!is_empty)` | ✅ `["https:", "host", "agent-invite", "invite_xxx"]` |
| 邀请 ID 检测 | `last().filter(starts_with("invite_"))` | ✅ 正确识别 |

### Phase 3: API 调用

```
POST https://agent-world-test.chuanshuo.com.cn/api/v1/agent-binding/invitations/invite_8486d728a28f4c54a8188b8bebc0db3c/agent-task
Body: {"task_access_token": "task_xxx", "web_origin": "https://agent-world-test.chuanshuo.com.cn"}
```

**返回状态:** 200 OK  
**响应格式:** `{ "ok": true, "data": {...}, "request_id": "req_..." }`

### Phase 4: 任务包验证

通过对返回的 `data` 对象进行全面断言验证，确认：

| 字段组 | 关键字段 | 值/格式 |
|--------|---------|---------|
| 核心标识 | `task_package_version` | `agent_binding_task_v1` |
| | `task` | `bind_yourself_to_agent_world` |
| | `audience` | `external_agent` |
| 绑定凭证 | `invitation_id` | `invite_8486d728a28f4c54a8188b8bebc0db3c` |
| | `subject_id` | `subject_6e8ca1b2e05541ddab74741d781fe899` |
| | `one_time_token` | `token_21f2e09761fca6...`（53 字符） |
| | `world_api_base_url` | `https://agent-world-test.chuanshuo.com.cn/api/v1` |
| | `expires_at` | `2026-07-10T07:53:57.529Z`（24h 有效） |
| **direct_api_fallback** | `submit_binding_request_endpoint` | `/agent-binding/requests` |
| | `complete_challenge_endpoint_template` | `/agent-binding/challenges/{challenge_id}/complete` |
| | **`required_first_binding_fields`** | **11 个字段（详见下文）** |
| 技能分发 | `skill_distribution.manifest_endpoint` | 存在 |
| | `skill_distribution.minimum_skill_version` | `0.2.4` |
| 视觉身份 | `visual_identity_catalog.allowed_role_ids` | 20 个角色 |
| 确认等待 | `confirmation_wait_contract.expires_in_seconds` | 86400（24 小时） |

### Phase 5: required_first_binding_fields（11 个必填字段）

验证全部 11 个字段都存在：

| # | 字段名 | 说明 | 之前硬编码填充？ |
|---|--------|------|----------------|
| 1 | `invitation_id` | 邀请 ID | ✅ 是 |
| 2 | `one_time_token` | 一次性令牌 | ✅ 是 |
| 3 | `agent_name` | Agent 名称 | ❌ 硬编码为 "go-on" |
| 4 | `agent_type` | Agent 类型 | ❌ 未填充 |
| 5 | `public_key` | Ed25519 签名公钥 | ✅ 是 |
| 6 | `signature_algorithm` | 签名算法（Ed25519） | ❌ 未填充 |
| 7 | `encryption_public_key` | RSA 加密公钥 | ❌ 未填充（未生成 RSA 密钥） |
| 8 | `encryption_key_algorithm` | 加密算法（RSA-OAEP-256-AES-256-GCM） | ❌ 未填充 |
| 9 | `capability_summary` | 能力摘要 | ❌ 未填充 |
| 10 | `runtime_declaration` | 运行时声明 | ❌ 未填充 |
| 11 | `visual_identity` | 视觉身份（角色选择） | ❌ 未填充 |

## 发现并修复的问题

### 问题 1: 硬编码绑定流程（已修复）
- **问题:** `chat_phases.rs` 中 ~400 行 Rust 代码自动执行 Ed25519 密钥生成、绑定请求提交、挑战签名等全部绑定流程
- **风险:** 任务包升级（从 3 个扩展到 11 个必填字段）需要修改 Rust 代码，且硬编码只填了 3 个字段
- **修复:** 移除所有硬编码逻辑，改为将原始任务包 JSON 注入 AI 上下文，由 AI 自主规划执行

### 问题 2: `extract_url` 作用域过小
- **问题:** `pub(crate)` 导致集成测试无法访问
- **修复:** 改为 `pub`，该函数是纯工具函数无副作用，公开无风险

### 问题 3: 上下文包含指令性内容（已修复）
- **问题:** 之前上下文中有 "AI MUST execute"、"Key workflow summary" 等指令性内容
- **修复:** 改为中性数据呈现格式 `[Agent World Task Package - pre-fetched by system]` + 原始 JSON

## 验证的架构流程

```
用户发送邀请链接
    │
    ▼
observe_phase:
  ├── extract_url()        → 提取完整 URL（含 fragment）
  ├── 抓取 SPA 页面         → 检测 <div id="root">
  ├── 解析 fragment          → 提取 task_access_token
  ├── 解析 path segments     → 提取 invitation_id
  ├── 构造 API URL           → POST .../agent-task
  ├── 调用 API               → 获取任务包（11 个必填字段）
  └── 注入 AI 上下文          → [Agent World Task Package - pre-fetched by system]
                                 + 原始 JSON（中性呈现，无指令）
    │
    ▼
think_phase + act_phase（AI 驱动）:
  ├── AI 读取任务包数据
  ├── AI 自行规划工作流（PUA 通用原则）
  ├── AI 使用 http_request / shell_exec 执行各步骤
  └── AI 报告结果给用户，等待网页确认
```

## 结论

- **URL 检测管道:** ✅ 完整验证通过
- **API 调用:** ✅ 成功获取包含 11 个必填字段的完整任务包
- **上下文呈现:** ✅ 中性、无指令性内容，由 AI 自主决策
- **编译:** ✅ `cargo check` 0 错误
- **测试:** ✅ `cargo test --test agent_world_invitation_test` 7/7 通过
