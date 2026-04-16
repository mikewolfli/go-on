# 后端 CLI

后端可执行文件是当前系统的权威控制面，负责运行时启动、setup、健康检查、任务规划以及协议模式选择。

## 调用方式

生产或打包后的二进制：

```bash
go-on --config config.toml
```

开发阶段：

```bash
cargo run -- --config config.toml
```

当前帮助入口形式为：

```text
Usage: go-on.exe [OPTIONS]
```

这里没有子命令，全部通过参数控制。

## 基础运行参数

### `--config <CONFIG>`

指定配置文件路径。如果不传，后端会从可执行文件所在目录解析 `config.toml`。

### `--phase <PHASE>`

指定运行阶段。适合你的配置里定义了多个 phase，并且希望固定选中某一个入口时使用。

### `--verbose`

开启详细日志。排查启动、配置、传输层、Provider 就绪问题时应优先开启。

## 校验与就绪检查

### `--validate-config` 或 `--doctor`

校验配置后退出。这是排查问题前最便宜、最直接的一步。

```bash
go-on --config config.toml --validate-config
```

### `--status` 或 `--check`

打印已配置 Provider 和运行时就绪状态。

适合在 setup 之后、手改 `config.toml` 之后、接编辑器之前执行。

```bash
go-on --status
```

### `--healthcheck`

生成运行时健康报告并持久化到 `.goon/`。适合需要留档或后续排查时使用。

## setup 与推荐配置

### `--setup` 或 `--init`

运行交互式设置向导。

### `--setup-profile <SETUP_PROFILE>`

当前接受值只有：

- `adaptive`

### `--setup-level <SETUP_LEVEL>`

接受值：

- `quick`
- `standard`
- `custom`

实际建议：

- `quick`：最短路径，跳过额外 agent 提示。
- `standard`：默认推荐。
- `custom`：适合想手动控制更多细节的场景。

### `--setup-secrets <SETUP_SECRETS>`

接受值：

- `env`
- `keyring`
- `auto`

实现上 `autodetect` 也会被接受。

### `--apply-recommended`

把 Provider 能力推荐应用到当前 `config.toml` 后退出。适合补齐 Provider 或调整模型组合之后使用。

### `--force`

即使目标文件已存在也强制 setup。只有在你明确要覆盖当前配置时才应使用。

## 本地模型注册

### `--add-local-model` 或 `--add-model`

向配置里新增或更新本地模型 agent 条目。

通常要和下面的 `--local-model-*` 参数一起使用。

### `--local-model-name <NAME>`

本地 agent 的逻辑名称。

### `--local-model-url <URL>`

本地模型服务地址。

### `--local-model-type <TYPE>`

Provider 类型，默认意图是 `openai`。

### `--local-model-model <MODEL_ID>`

配置中写入的模型 ID。

### `--local-model-api-key-env <ENV_NAME>`

可选的 API Key 环境变量名。

### `--local-model-secret-key-env <ENV_NAME>`

可选的 secret key 环境变量名。

### `--local-model-register-only`

只把模型注册到 `[agents]`，不自动挂接到 phase agent 列表。

示例：

```bash
go-on --add-local-model \
  --local-model-name ollama-local \
  --local-model-url http://127.0.0.1:11434/v1 \
  --local-model-type openai \
  --local-model-model qwen2.5-coder \
  --local-model-register-only
```

## Secret 管理

### `--secret <ACTION>`

接受动作：

- `set`
- `get`
- `delete`
- `list`

### `--secret-name <SECRET_NAME>`

逻辑 secret 名称。

### `--secret-value <SECRET_VALUE>`

与 `set` 配合使用的值。

示例：

```bash
go-on --secret list
go-on --secret set --secret-name openai --secret-value YOUR_KEY
go-on --secret get --secret-name openai
go-on --secret delete --secret-name openai
```

## 计划与制品检查

### `--action-check <ACTION_CHECK>`

对 `.goon/` 下的制品执行检查。帮助文本给出的取值为：

- `all`
- `spec`
- `qa`
- `retest`
- `final`

### `--plan-task <PLAN_TASK>`

为复杂任务构建并持久化受控计划制品。

## 传输模式选择

### `--protocol-mode <MODE>`

接受值：

- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

使用建议：

- `adaptive`：默认最稳，适合多前端共享。
- `acp_stdio`：适合编辑器直接拉起子进程。
- `acp_http`：适合 ACP 客户端连接共享 HTTP 后端。
- `mcp_stdio`：只在客户端明确要求 MCP stdio 时使用。
- `mcp_http`：适合需要 OpenAI 兼容 `/v1` HTTP 面的客户端。

### `--acp-http-bind <ADDR>`

绑定 HTTP 监听，并暴露：

- `/health`
- `/chat`
- `/chat/stream`

当前实现里，同一个运行时也会同时暴露 Zed 和探测逻辑常用的 `/v1` 兼容端点。

示例：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

## 常用命令组合

最常见 setup：

```bash
go-on --setup --setup-level standard --setup-secrets auto
```

先校验再检查就绪：

```bash
go-on --config config.toml --validate-config
go-on --config config.toml --status
```

启动共享本地 HTTP 运行时：

```bash
go-on --config config.toml --protocol-mode adaptive --acp-http-bind 127.0.0.1:8090
```

为编辑器拉起式场景运行 ACP stdio：

```bash
go-on --config config.toml --protocol-mode acp_stdio --verbose
```

## 运维建议

- 遇到问题先跑 `--validate-config`，不要先怀疑传输层。
- 打开 GUI 或编辑器前先跑一次 `--status`。
- 除非客户端契约明确要求 ACP-only 或 MCP-only，否则优先用 `adaptive`。
- 接本地 OpenAI 兼容模型时，优先用 `--add-local-model`，不要直接手改配置。