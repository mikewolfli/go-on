# 工具系统

## 概述

Go-On 提供统一的工具系统，围绕 `ToolRegistry`、`Tool` trait 和工具执行器（`execute_tool_call` / `run_fallback_chain_async`）构建。工具是 AI 智能体与文件系统交互、执行命令、搜索代码和执行其他操作的主要机制。

## 架构

```text
ToolRegistry（全局单例）
  ├── Tool trait（read_file、write_file、grep 等）
  ├── ToolCapabilityProfile（风险、超时、备用）
  └── 别名（semantic_search → code_index_search）
         │
         ▼
  ToolExecutor（execute_tool_call + 备用链）
         │
         ▼
  SandboxPolicy（治理关卡）
```

## 内置工具

### 核心工具（始终可用）

| 工具 | 类型 | 说明 |
|------|------|------|
| `read_file` | 读取 | 读取文件内容 |
| `write_file` | 写入 | 创建或覆盖文件 |
| `search_files` | 搜索 | 按模式查找文件 |
| `apply_patch` | 写入 | 应用差异到文件 |
| `run_tests` | Shell | 执行测试命令 |
| `inspect_git_diff` | 读取 | 显示 git diff |

### 扩展工具

#### 搜索与发现

| 工具 | 说明 |
|------|------|
| `grep` | 使用正则搜索文件内容 |
| `find_files` | 按名称模式查找文件 |
| `code_index_search` | 语义代码符号搜索 |
| `diagnostics` | 获取项目诊断信息 |

#### 文件操作

| 工具 | 说明 |
|------|------|
| `list_directory` | 列出目录内容 |
| `file_move` | 移动/重命名文件 |
| `file_delete` | 删除文件（需确认） |
| `archive_inspect` | 检查归档内容 |
| `archive_extract` | 解压归档 |
| `compress`/`decompress` | 文件压缩 |

#### Shell 与执行

| 工具 | 说明 |
|------|------|
| `shell_exec` | 执行 Shell 命令 |
| `cargo_check` | 运行 cargo check |
| `cargo_test` | 运行 cargo test |
| `git` | 执行 git 命令 |

#### 网络

| 工具 | 说明 |
|------|------|
| `http_request` | 发起 HTTP 请求 |
| `dns_lookup` | DNS 解析 |
| `ping` | 网络 Ping |
| `port_scan` | 端口扫描 |

#### 技能管理

| 工具 | 说明 |
|------|------|
| `skill_list` | 列出已注册技能 |
| `skill_execute` | 按名称执行技能 |
| `skill_create` | 创建基于提示词的技能 |
| `skill_reload` | 从磁盘重新加载技能 |

## 创建自定义工具

1. 实现 `Tool` trait：
```rust
pub struct MyTool;

impl Tool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // 实现
    }
}
```

2. 在 `ToolRegistry::new()` 中注册：
```rust
registry.register_with_profile(
    MyTool,
    ToolCapabilityProfile { ... },
);
```

3. 注册表将工具接入 `execute_tool_call`（备用链由 `run_fallback_chain_async` 处理）。

## 沙盒集成

每个工具按操作类型（读取、搜索、写入、Shell、网络）分类，并根据活动的 `SandboxLevel` 进行检查。
