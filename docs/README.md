# go-on 文档目录

欢迎阅读 go-on 文档。

---

## 📚 用户帮助文档（mdBook 格式）

最完整的使用指南在 `cookbook/` 目录，以 mdBook 格式提供：

```bash
# 构建并阅读
cd cookbook && mdbook serve --open
```

### 在线内容索引

| 章节 | 说明 |
|:-----|:-----|
| [架构总览](../cookbook/src/zh-CN/overview.md) | 系统架构、7 子总线、认知模块 |
| [快速设置](../cookbook/src/zh-CN/setup-wizard.md) | 交互式安装向导 |
| [后端 CLI](../cookbook/src/zh-CN/backend-cli.md) | 命令行使用 |
| [高级编排](../cookbook/src/zh-CN/advanced-orchestration.md) | DAG 执行引擎、FullAuto 等 |
| [GUI 控制台](../cookbook/src/zh-CN/gui.md) | 桌面图形界面完整指南 |
| [提示词模板](../cookbook/src/zh-CN/prompts.md) | 149+ 模板，16 行业类别 |
| [工作流配置](../cookbook/src/zh-CN/workflow-config.md) | Phase 编排与路由 |
| [Zed 接入](../cookbook/src/zh-CN/zed.md) | 编辑器集成 |
| [VS Code 插件](../cookbook/src/zh-CN/vscode-addon.md) | 扩展安装与使用 |

> 完整 API 文档见 `cookbook/src/en/api/` 与 `cookbook/src/zh-CN/api/` 下的文件。

## 📋 项目文档（docs/ 目录）

| 文档 | 说明 |
|:-----|:-----|
| [DEVELOPMENT_RULES.md](DEVELOPMENT_RULES.md) | 工程开发规范 |
| [RULES.md](RULES.md) | 项目规则覆盖层 |
| [SAFEGUARD_MODE.md](SAFEGUARD_MODE.md) | 安全模式说明 |
| [RELEASE_READINESS.md](RELEASE_READINESS.md) | 发布就绪检查清单 |
| [CLAUDE.md](CLAUDE.md) | Agent 规则索引 |
| [protocol-guide.md](protocol-guide.md) | ACP/MCP 协议配置 |
| [gui-guide.md](gui-guide.md) | GUI 使用快速参考 |
| [workflow-config.md](workflow-config.md) | 工作流配置快速参考 |
| [prompts-guide.md](prompts-guide.md) | 提示词模板快速参考 |

> 用户帮助文档统一由 `cookbook/` mdBook 管理；历史一轮式报告归档于 `docs/log/archive/`。
