# Go-On 文档索引

## 概述

Go-On 是一个 ACP 运行时代理，集成了多智能体编排能力，可运行本地开发工具、简单服务器或多用户生产服务器。

## 快速开始

- [项目说明](../../README.zh-CN.md) — 项目概览与快速上手
- [开发规则](DEVELOPMENT_RULES.md) — 编码规范与贡献指南
- [协议指南](protocol-guide.md) — ACP/MCP 协议集成
- [GUI 指南](gui-guide.md) — 桌面 GUI 使用

## 架构

### 核心系统

| 模块 | 说明 |
|------|------|
| `src/core/` | 配置、错误处理、启动、供应商 |
| `src/acp/` | ACP 协议实现（聊天、运行时、传输） |
| `src/mcp/` | MCP 协议兼容层 |
| `src/protocol/` | 共享协议类型和服务端定义 |

### AI 编排

| 模块 | 说明 |
|------|------|
| `src/agents/` | AI 供应商适配器（OpenAI、Anthropic、DeepSeek 等） |
| `src/orchestration/` | 技能系统、工具注册表、规划器、任务路由 |
| `src/intelligence/` | 能力总线、模型选择、强化学习 |
| `src/memory/` | 向量存储、缓存、嵌入引擎 |

### 治理与安全

| 模块 | 说明 |
|------|------|
| `src/governance/` | 沙盒、RBAC、审计、PUA 规则、审批引擎 |
| `src/security/` | 认证、加密、提示注入检测 |

### 工具

- [工具系统](guides/tool-system.zh.md) — 工具注册表、管道和自定义工具
- [代码索引](guides/code-index.zh.md) — 语义代码搜索工具
- [技能系统](guides/skill-system.zh.md) — SKILL.md 发现、导入、执行

### 可观测性

| 模块 | 说明 |
|------|------|
| `src/observability/` | 遥测、性能指标 |
| `src/optimization/` | 故障预防、工作流优化 |

## 客户端集成

- [VS Code 扩展](../vscode-addon/README.md) — IDE 集成（ACP/MCP）
- [GUI 应用](../gui/README.md) — 基于 Tauri 的桌面应用
- [Rust SDK](../sdk/rust/README.md) — 程序化访问

## 部署

- [配置](workflow-config.md) — Profile 和工作流配置
- [部署指南](../deploy/README.md) — 服务端部署说明

## 蓝图

- [原则](blueprints/principle.md) — 核心开发原则
- [技能市场](blueprints/skill-market.md) — 社区插件市场计划

## 日志

- [扫描日志](log/) — 多轮深度扫描记录
- [报告](reports/) — 优化与分析报告
