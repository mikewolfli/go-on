# go-on 协议配置与 Zed 集成

go-on 支持多种协议模式，用于与外部编辑器（如 Zed、VS Code）和 AI agent 工具进行集成。

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
| `adaptive`     | 自适应模式，自动检测和切换协议                    | 通用场景，推荐使用             |

---

## 协议自动检测流程

当设置为 `adaptive`（自适应）模式时，系统会按照以下流程自动检测并选择合适的协议：

```
启动 adaptive 模式
        │
        ▼
┌──────────────────────┐
│  检测运行环境          │
│  - 是否在 Zed 中运行   │
│  - 是否在 VS Code 中   │
│  - 是否有 stdin 可用   │
└──────────┬───────────┘
           │
    ┌──────┴──────┐
    ▼             ▼
  Zed/VS Code    其他环境
    │             │
    ▼             ▼
 acp_stdio    acp_http
 或 mcp_stdio  或 mcp_http
```

检测规则：
1. 如果检测到 Zed 编辑器环境，优先使用 `acp_stdio` 或 `mcp_stdio`
2. 如果检测到 VS Code 扩展环境，优先使用 `acp_stdio` 或 `mcp_stdio`
3. 其他环境默认使用 `acp_http` 或 `mcp_http`
4. 可以通过命令行动态覆盖：`--protocol mcp_http`

---

## Zed 编辑器集成配置

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
         "args": ["--protocol", "mcp_stdio"]
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
  "go-on.mcp.command": "go-on",
  "go-on.mcp.args": ["--protocol", "mcp_stdio"],
  "go-on.provider.name": "go-on"
}
```

### MCP 配置方式（跨编辑器通用）

创建或编辑 `~/.config/go-on/mcp-config.json`：

```json
{
  "mcpServers": {
    "go-on": {
      "command": "go-on",
      "args": ["--protocol", "mcp_stdio"],
      "env": {}
    }
  }
}
```

---

## 可用 MCP 工具列表

通过 MCP 协议，go-on 向外部编辑器暴露以下工具：

| MCP 工具         | 功能                                           |
|------------------|------------------------------------------------|
| `prompts_list`   | 列出所有可用的提示词模板                       |
| `prompts_get`    | 获取指定模板的详细内容                         |
| `workflow_list`  | 列出所有可用的 workflow                        |
| `workflow_execute` | 执行指定 workflow                            |
| `skills_list`    | 列出所有可用的技能                             |
| `tool_list`      | 列出所有可用的工具                             |

这些工具可以在支持 MCP 的编辑器（Zed、VS Code、Cursor 等）中自动发现并使用。

---

## 命令行示例

```bash
# 使用 ACP HTTP 协议启动（默认端口）
go-on --protocol acp_http

# 使用 MCP STDIO 协议启动（适用于编辑器集成）
go-on --protocol mcp_stdio

# 使用自适应模式
go-on --protocol adaptive

# 指定配置文件和协议
go-on --config config.toml --protocol mcp_http
```
