# 设置向导

设置向导由后端实现，是新机器或新工作目录初始化的推荐入口。

## 会写入什么

setup 会以目标配置路径为核心，写出一组默认文件：

- `config.toml`
- 配置目录附近的默认规则文件
- 供环境变量或 keyring 使用的 secret 引用

当前 setup 走的是 adaptive 模板族。

## 入口方式

交互式：

```bash
go-on --setup
```

偏非交互式：

```bash
go-on --setup --setup-profile adaptive --setup-level standard --setup-secrets auto
```

覆盖已有文件：

```bash
go-on --setup --force
```

## setup profile

当前只接受：

- `adaptive`

这和当前架构一致，即一份配置尽量同时服务 ACP 与 MCP 风格前端。

## setup level

### `quick`

适合追求最快成功初始化。

实现特征：

- 为了减少流程长度，会跳过额外 agent 提示

### `standard`

默认推荐。

适合大多数用户，在引导性和可控性之间比较平衡。

### `custom`

适合想手动控制更多 Provider 与 agent 选择的场景。

## secret 模式

### `env`

把 secret 继续作为环境变量驱动。

适合已经有 shell、`.env`、CI 或进程管理器注入方案的场景。

### `keyring`

把 secret 存入操作系统 keyring，让配置引用 keyring-backed 值。

适合桌面本地使用，希望减少明文暴露的场景。

### `auto`

自动选择。

实现行为：

- 如果机器上已经有可用环境变量，setup 会优先走 `env`
- 否则 setup 会继续询问 secret 处理方式

## Provider 检测行为

setup 会根据 secret 模式和当前机器状态检测可用 Provider。

流程大致是：

1. 检测 Provider
2. 让用户选择要启用的 Provider
3. 应用 setup level 对应默认值
4. 生成 adaptive 配置
5. 如有需要，把 secret 写入 keyring

如果最终没有选中任何 Provider，setup 会直接失败，而不是产出一个表面成功但不可运行的配置。

## keyring 行为

选择 keyring 模式后，生成的配置会从环境变量占位符转换成 keyring 引用。

本仓库当前使用的引用形式是：

```text
keyring://go-on/<account>
```

## setup 之外的 secret 管理

setup 不是唯一入口，后续也可以单独用 CLI 管理：

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
```

这也是 setup 正常但凭证后续变更时最干净的修复路径。

## 推荐初始化顺序

对大多数使用者：

1. 运行 `go-on --setup --setup-level standard --setup-secrets auto`。
2. 运行 `go-on --status`。
3. 如果要让 Zed 或 GUI 走 HTTP，使用 `--protocol-mode adaptive --acp-http-bind 127.0.0.1:8090` 启动后端。
4. 如果是编辑器自拉起的 stdio 场景，再切到 `acp_stdio` 或 `mcp_stdio`。

## 什么时候重跑 setup

以下场景建议重跑：

- 更换机器
- 更换 Provider 组合
- 从 env 模式切换到 keyring 模式
- `config.toml` 丢失或损坏

除非明确要替换现有文件集，否则不要轻易带 `--force` 重跑。