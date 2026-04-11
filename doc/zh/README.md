# go-on 简体中文帮助文档

本项目是一个多智能体路由与治理的后端服务，支持多种 AI 代理、流程编排、配置热加载等。

## 主要功能
- 多阶段（phase）任务流自动路由
- 支持多家 AI 代理（OpenAI、Anthropic、Moonshot 等）
- 配置热加载与国际化
- 结构化日志与性能监控
- 命令行与 HTTP API 入口

## 快速开始
1. 安装 Rust 环境，编译：
   ```bash
   cargo build --release
   ```
2. 运行服务：
   ```bash
   ./target/release/go-on
   ```
3. 查看命令行参数：
   ```bash
   ./target/release/go-on --help
   ```

## 主要命令行参数
- `--config` 指定配置文件
- `--phase` 指定运行阶段
- `--validate-config` 校验配置
- `--verbose` 启用详细日志

## HTTP API
- `/chat` 聊天接口
- `/chat/stream` 流式聊天接口
- `/health` 健康检查

详细配置与进阶用法请参考教程文档。
