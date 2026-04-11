# go-on 教程（简体中文）

## 1. 编译与运行
1. 安装 Rust 工具链（推荐使用 rustup）。
2. 在项目根目录执行：
   ```bash
   cargo build --release
   ```
3. 运行服务：
   ```bash
   ./target/release/go-on
   ```

## 2. 配置文件
- 默认读取 `config.toml`，可用 `--config` 指定其他路径。
- 支持热加载和多语言。

## 3. 常用命令
- 校验配置：
  ```bash
  ./target/release/go-on --validate-config
  ```
- 指定 phase 运行：
  ```bash
  ./target/release/go-on --phase review
  ```

## 4. HTTP API 调用
- 聊天接口：`/chat`，支持 POST JSON。
- 流式接口：`/chat/stream`。
- 健康检查：`/health`。

## 5. 日志与调试
- `--verbose` 查看详细日志。
- 日志采用 tracing，支持多级别。

## 6. 进阶
- 支持多 AI 代理、任务流自定义、国际化等。
- 详细见源码和配置说明。
