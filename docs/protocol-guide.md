# go-on 协议配置与 Zed 集成

go-on 支持多种协议模式，用于与外部编辑器（如 Zed、VS Code）和 AI agent 工具进行集成。

> **范围说明**：本文档只介绍协议模式的选择、命令行标志以及 Zed/VS Code 编辑器集成方式，**不包含** JSON-RPC 方法参考。方法的权威清单在代码中：ACP 分发表见 `src/acp/impl/request.rs`（`handle_request` 的 match 分支），ACP 方法白名单见 `src/acp/impl/request/protocol.rs`（`ACP_METHODS`）。

> 相关文档：[工作流配置](workflow-config.md) | [GUI 使用指南](gui-guide.md)

---

## 协议模式

go-on 支持以下几种协议模式：

| 协议模式       | 说明                                             | 适用场景                       |
|----------------|--------------------------------------------------|--------------------------------|
| `acp_http`     | ACP（Agent-to-Computer Protocol）通过 HTTP 传输   | 远程 agent 集成                |
| `acp_stdio`    | ACP 通过标准输入输出传输                          | 本地 agent 集成                |
| `mcp_stdio`    | MCP（Model Context Protocol）通过标准输入输出传输 | 本地 MCP 工具集成              |
| `mcp_http`     | MCP 通过 HTTP 传输                                | 远程 MCP 工具集成              |
| `adaptive`     | 自适应模式：双栈分发，按是否传入 `--acp-http-bind` 选择启动传输 | 通用场景，推荐使用             |

---

## 协议模式解析

当设置为 `adaptive`（自适应）模式时，系统保持 ACP/MCP 双栈请求分发能力，并根据启动参数决定启动传输方式（见 `src/protocol/access_mode.rs` 的 `resolve_access_selection`）：

- 传入了 `--acp-http-bind <ADDR>` → 启动 HTTP 传输（`acp_http` 风格的 HTTP 入口）
- 未传入 `--acp-http-bind` → 启动 stdio 传输

请求分发（ACP 还是 MCP）在两种情况下都由请求形状自动判定，不受客户端环境（Zed / VS Code / 终端）影响；`adaptive` 不会根据编辑器检测来选择协议。

检测规则：
1. 显式模式（`acp_stdio` / `acp_http` / `mcp_stdio` / `mcp_http`）固定使用对应传输，不做自动切换
2. 未指定模式或指定 `adaptive` 时，仅按上述 `--acp-http-bind` 规则选择启动传输
3. 命令行覆盖标志为 `--protocol-mode <MODE>`（别名 `-m` / `--mode`）

---

## Zed 编辑器集成配置

> 注：以下 `assistant.provider` 配置是旧版 Zed 的 MCP 接入方式。当前 Zed 使用 `agent_servers` 注册 go-on（ACP over stdio），见 [Zed 集成指南](zed-integration.md) 与 [多编辑器快速配置](editor-quick-config.md)。

### 前提条件

- 安装 Zed 编辑器（v0.150+）
- 已安装 `go-on` 可执行文件

### 配置步骤

1. 在 Zed 中打开设置：
   ```
   Ctrl + Shift + P → zed: open settings
   ```

2. 添加以下配置到 `settings.json`：

   ```json
   {
     "assistant": {
       "version": "2",
       "provider": {
         "name": "go-on",
         "type": "mcp",
         "command": "go-on",
         "args": ["--protocol-mode", "mcp_stdio"]
       }
     }
   }
   ```

3. 保存设置，Zed 会自动启动 `go-on` 并建立 MCP 连接。

### 验证集成

在 Zed 中打开 Chat 面板（`Ctrl + Shift + ~`），如果能看到 go-on 的 AI agent 响应，则表示集成成功。

---

## VS Code 扩展集成

### 前提条件

- 安装 VS Code（v1.85+）
- 已安装 `go-on` 可执行文件
- 安装 go-on VS Code 扩展（如果有）

### 配置步骤

在 VS Code 的 `settings.json` 中添加：

```json
{
  "go-on.executablePath": "./target/release/go-on",
  "go-on.configPath": "./config.toml",
  "go-on.runtime.protocolMode": "from_config"
}
```

### MCP 配置方式（跨编辑器通用）

创建或编辑 `~/.config/go-on/mcp-config.json`：

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol-mode", "mcp_stdio"],
      "env": {}
    }
  }
}
```

---

## 可用 MCP 工具列表

通过 MCP 协议，go-on 向外部编辑器暴露以下基线工具（`src/acp/impl/request/tools_pack.rs` 的 `build_mcp_tool_descriptors`；实际列表还包含随构建特性注册的内置工具与已导入技能）：

| MCP 工具         | 功能                                           |
|------------------|------------------------------------------------|
| `acp_trace_get`  | 获取 ACP 追踪事件                              |
| `acp_debug_panel_get` | 获取 ACP 调试面板快照                     |
| `goon_workflow_run_list` | 分页列出工作流运行，支持状态过滤         |
| `goon_workflow_run_get` | 按 run_id 获取工作流运行详情             |
| `goon_workflow_run_cancel` | 按 run_id 取消工作流运行               |
| `goon_workflow_run_pause` | 按 run_id 暂停工作流运行                |
| `goon_workflow_run_resume` | 按 run_id 恢复工作流运行               |
| `goon_provider_test_connection` | 校验供应商连通性与密钥就绪状态       |
| `goon_provider_test_completion` | 校验供应商/模型补全路由               |
| `goon_provider_capabilities` | 查询供应商模型能力元数据               |
| `goon_metrics_window_query` | 查询指标时间窗口序列（1m/5m/1h）      |
| `goon_metrics_errors_summary` | 查询分组错误与失败样例                 |
| `goon_skill_update` | 更新导入技能的 manifest 字段                |
| `goon_skill_version_list` | 列出导入技能的版本快照                 |
| `goon_skill_version_rollback` | 将导入技能回滚到指定版本              |
| `prompts_list`    | 列出所有可用的提示词模板                       |
| `prompts_get`     | 获取指定模板的详细内容                         |
| `skill-finder`    | 按自然语言查询搜索已注册技能                   |
| `echo_skill`      | 回显结构化输入，用于技能管道诊断               |
| `skill-creator`   | 根据结构化定义创建或更新提示词技能             |
| `builtin.echo`    | 回显工具负载，用于连通性与契约诊断             |
| `http_request`    | 发起 HTTP 请求（GET/POST/PUT/DELETE 等）       |
| `workflow_execute` | 以工作流方式执行一个任务（多步自主计划）      |
| `workflow_ask`    | 让 AI 使用完整推理与可用技能完成任务           |
| `workflow_generate` | 仅为任务生成结构化工作流计划（不执行）      |
| `import_skill`    | 从远端源（GitHub、URL 等）导入技能             |
| `github_search_skills` | 在 GitHub 上按查询搜索 go-on 兼容技能    |

这些工具可以在支持 MCP 的编辑器（Zed、VS Code、Cursor 等）中自动发现并使用。

---

## 命令行示例

```bash
# 使用 ACP HTTP 协议启动（默认端口）
go-on --protocol-mode acp_http

# 使用 MCP STDIO 协议启动（适用于编辑器集成）
go-on --protocol-mode mcp_stdio

# 使用自适应模式
go-on --protocol-mode adaptive

# 指定配置文件和协议
go-on --config config.toml --protocol-mode mcp_http
```
