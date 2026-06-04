# VS Code 插件

VS Code 插件是本仓库里功能最完整的编辑器接入面。它暴露了基于运行时的命令，可以探测运行时健康状态，也允许在设置中覆盖后端协议模式。

## 插件依赖什么

插件需要：

- 可访问的 `go-on` 可执行文件
- 有效的 `config.toml`
- 与当前工作流匹配的协议模式

插件清单当前暴露的协议覆盖值为：

- `from_config`
- `adaptive`
- `acp_stdio`
- `acp_http`
- `mcp_stdio`
- `mcp_http`

其中 `from_config` 表示跟随后端配置，其余值表示显式强制覆盖。

## 首次接入建议

1. 先构建后端或准备好可执行文件。
2. 运行 `go-on --setup --setup-level standard`。
3. 如果自动发现不够稳定，再在 VS Code 设置中显式填写可执行文件路径和配置路径。
4. 除非在排查特定传输问题，否则协议模式保持 `from_config`。

## 各协议模式什么时候用

- `from_config`：日常默认。
- `adaptive`：希望一个运行时同时兼容多类探测时优先使用。
- `acp_stdio`：插件应驱动拉起 stdio 运行时时使用。
- `acp_http`：后端已作为共享本地 HTTP 服务运行时使用。
- `mcp_stdio`：只有明确需要 MCP stdio 才用。
- `mcp_http`：明确需要 `/v1` HTTP 语义时使用。

## 运行时健康面

插件契约中的健康检查路径是：

```text
/health
```

OpenAI 兼容探测路径是：

```text
/v1/models
```

插件同时也知道这些路径：

- `/v1/model`
- `/v1/chat/completions`
- `/v1/responses`

## 实用工作区设置示例

```json
{
  "go-on.runtime.protocolMode": "from_config",
  "go-on.runtime.executablePath": "D:/Workspace/RustWorkspace/go-on/target/debug/go-on.exe",
  "go-on.runtime.configPath": "D:/Workspace/RustWorkspace/go-on/config.toml"
}
```

如果要强制共享 HTTP 运行时：

```json
{
  "go-on.runtime.protocolMode": "acp_http"
}
```

## 实际排查顺序

对插件来说，推荐按下面顺序排查：

1. `go-on --validate-config`
2. `go-on --status`
3. 检查 VS Code 设置里的 executable path
4. 检查 VS Code 设置里的 config path
5. 最后再看是否需要协议模式覆盖

## 什么时候选 HTTP，什么时候选 stdio

优先选 HTTP：

- 希望 GUI 与 VS Code 共享同一个后端
- 希望手工探测 `/health` 与 `/v1/models`
- 希望后端作为长驻本地服务存在

优先选 stdio：

- 希望 VS Code 自己管理进程启停
- 希望不同工作区完全隔离

## 常见失败模式

- 插件能拉起可执行文件，但提示 provider not ready，问题多半在配置或凭证，不在传输层。
- 选了 HTTP 模式但 `/health` 不通，说明后端并未用 `--acp-http-bind` 启动。
- 强制 `mcp_http` 时，要确认当前消费该能力的插件路径确实需要 `/v1` 语义，而不是 ACP 语义。

## 可用命令

插件在 VS Code 命令面板中注册了以下命令：

**进程生命周期**

| 命令 | 说明 |
|---|---|
| `go-on.start` | 启动 Go-On 后端进程 |
| `go-on.stop` | 停止运行中的后端进程 |
| `go-on.shutdown` | 优雅关闭后端 |
| `go-on.healthCheck` | 运行时健康检查 |
| `go-on.healthProbes` | 查看所有健康探针详情 |

**运行时诊断**

| 命令 | 说明 |
|---|---|
| `go-on.runtimeSelfModel` | 获取统一自画像视图：运行健康、漂移摘要、约束画像与建议动作 |
| `go-on.runtimeStability` | 获取运行时稳定性快照 |
| `go-on.providerStatus` | 获取 Provider 就绪状态、降级摘要与 Agent 依赖快照 |
| `go-on.metricsGet` | 获取当前运行时指标 |
| `go-on.metricsReset` | 重置运行时指标 |
| `go-on.traceMetrics` | 获取 Trace 级指标 |
| `go-on.traceGet` | 获取 Trace 条目 |
| `go-on.observabilityAlerts` | 查看可观测性告警 |
| `go-on.releaseReadiness` | 检查发布就绪门禁 |

**治理与质量**

| 命令 | 说明 |
|---|---|
| `go-on.governanceStatus` | 获取治理状态 |
| `go-on.governancePlanGet` | 获取当前治理计划 |
| `go-on.governanceAuditRecent` | 查看最近审计条目 |
| `go-on.qualityBaseline` | 获取质量基线快照 |
| `go-on.securityBaseline` | 获取安全基线 |
| `go-on.rlAlignmentEval` | 运行 RL 对齐离线评估 |
| `go-on.hardnessStatus` | 获取任务难度状态 |
| `go-on.costStatus` | 获取成本优化状态 |
| `go-on.autotuneStatus` | 获取自动调参状态 |
| `go-on.autotuneGet` | 获取自动调参参数 |
| `go-on.autotuneReset` | 重置自动调参参数 |
| `go-on.selectorStatus` | 获取模型选择器状态 |

**工作流与任务**

| 命令 | 说明 |
|---|---|
| `go-on.workflowExecute` | 执行当前工作流 |
| `go-on.taskPlan` | 规划任务 |
| `go-on.taskExecute` | 执行已规划任务 |
| `go-on.harnessStatus` | 获取测试套件状态 |
| `go-on.primarySecondarySummary` | 获取主从 Agent 摘要 |

**学习与优化**

| 命令 | 说明 |
|---|---|
| `go-on.learningSummary` | 获取学习循环摘要 |
| `go-on.learningGuardrail` | 获取学习防护状态 |
| `go-on.learningReplay` | 重放学习数据 |
| `go-on.knowledgeDistill` | 运行知识蒸馏 |
| `go-on.optimizationPeak` | 获取优化峰值状态 |
| `go-on.buildRepro` | 运行构建可复现性检查 |

**配置与运维**

| 命令 | 说明 |
|---|---|
| `go-on.configReload` | 重载运行时配置 |
| `go-on.configBaseline` | 获取配置基线快照 |
| `go-on.lockStatus` | 获取锁状态 |
| `go-on.breakerStatus` | 获取熔断器状态 |
| `go-on.breakerReset` | 重置熔断器 |
| `go-on.breakerRecovery` | 运行熔断器恢复 |
| `go-on.cacheClear` | 清空 ACP 缓存 |
| `go-on.vectorClear` | 清空向量存储 |
| `go-on.dataLifecycle` | 获取数据生命周期状态 |
| `go-on.errorContract` | 获取错误契约摘要 |
| `go-on.checkpointCreate` | 创建运行时检查点 |
| `go-on.checkpointList` | 列出可用检查点 |
| `go-on.conversationRollback` | 回滚到某个检查点 |
| `go-on.maintenanceGc` | 运行垃圾回收 |
| `go-on.actionCheck` | 检查动作安全性 |
| `go-on.debugPanelGet` | 获取调试面板数据 |

## 进程输出通道

所有 Go-On 进程输出（stdout、stderr、退出码、进程错误）均写入 VS Code 的 **"Go-On"** 输出通道。通过 **查看 → 输出** 打开，再从下拉菜单中选择 **Go-On** 即可查看。这是启动失败和运行时错误排查的首选入口。