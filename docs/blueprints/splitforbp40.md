# BP40 — 巨型文件拆分计划

## 已完成拆分 ✅

| 原文件 | 行数 | 拆分结果 | 状态 |
|--------|------|----------|------|
| `src/core/config.rs` | 4983 | `config/mod.rs`, `types.rs`, `defaults.rs`, `load.rs`, `autotune.rs` | ✅ |
| `src/acp/impl/chat.rs` | 6097 | `chat/mod.rs`, `params.rs`, `helpers.rs`, `pipeline.rs`, `risk.rs` | ✅ |
| `src/acp/impl/runtime.rs` | 4049 | `runtime/mod.rs`, `server.rs`, `openai.rs`, `responses.rs` | ✅ |

## 待拆分 ⏳

### 1. `src/acp/impl/request/runtime_pack.rs` (6275行) — 最大文件
这个文件包含运行时指标、健康检查、Copilot集成、锁监控、自模型等功能。

提议拆分:
| 新文件 | 内容 |
|--------|------|
| `request/copilot.rs` | GitHub Copilot 集成相关 (build_github_client, copilot_models_cache, resolve_copilot_github_token 等) |
| `request/metrics.rs` | 指标窗口历史、QPS、错误率等 |
| `request/health.rs` | 健康检查、健康探针、自模型构建 |
| `request/locks.rs` | 锁监控、锁健康摘要 |
| `request/governance.rs` | 治理审计事件 |
| 保持 `runtime_pack.rs` | 剩余函数和 re-exports |

### 2. `src/acp/impl/request/ops_pack.rs` (4540行)
操作类请求处理。

| 新文件 | 内容 |
|--------|------|
| `request/ops_security.rs` | 安全基线、安全相关操作 |
| `request/ops_harness.rs` | Harness套件、故障预防相关 |
| 保持 `ops_pack.rs` | 剩余通用操作函数 |

### 3. `src/acp/impl/request/exec_pack.rs` (3198行)
执行相关操作 — 中等大小，可根据函数聚类拆分。

| 新文件 | 内容 |
|--------|------|
| `request/exec_workflow.rs` | 工作流执行相关 |
| 保持 `exec_pack.rs` | 核心执行函数 |

### 4. `src/acp/impl/request/protocol_pack.rs` (2808行)
协议处理包 — 可拆分出能力事件schema、响应构建等。

### 5. `src/acp/impl/request/learning_pack.rs` (1391行)
学习相关 — 可拆分但当前大小尚可接受。

## 拆分原则

1. **glob re-export**: 所有子模块通过 `pub use submodule::*;` 在父级 re-export
2. **use paths**: 现存的 `use crate::acp::impl::request::handle_xxx` 路径保持不变
3. **super::***: 各 pack 文件使用 `use super::*;` 访问 request 模块公共类型
4. **无行为变更**: 拆分后函数签名、逻辑完全相同
5. **编译验证**: 每次拆分后运行 `cargo check` 和 `cargo clippy`
