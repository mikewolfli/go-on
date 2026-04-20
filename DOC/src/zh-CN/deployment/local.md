# 本地模式部署

## 概述

本地模式（`profile-local`）是 go-on 的默认部署配置，专为单用户开发环境设计。它提供了一个轻量级、自包含的运行时，具有基于 SQLite 的存储和自适应向量能力。

## 特性

### 核心能力
- **单用户操作**：专为个人开发者设计
- **SQLite 存储**：基于本地文件的缓存和向量存储
- **自适应向量存储**：当 `sqlite-vec` 可用时使用，否则回退到 JSON 嵌入
- **零外部依赖**：无需数据库服务器
- **快速设置**：最小化配置需求

### 存储架构
```
本地模式存储：
├── 缓存：SQLite 数据库 (acp_cache.sqlite3)
├── 向量存储：带向量扩展的 SQLite
└── 配置：本地 config.toml 文件
```

## 配置

### 默认配置
本地模式使用 `config/config.toml` 作为默认配置：

```toml
# config/config.toml（本地模式默认配置）
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "adaptive"

[cache]
enabled = true
path = "acp_cache.sqlite3"
default_ttl_seconds = 3600
max_entries = 5000

[vector]
enabled = true
auto_mode = true
path = "acp_vector.sqlite3"
dimensions = 192
min_query_chars = 80
top_k = 2
min_similarity = 0.82
```

### 特性标志
本地模式启用以下 Cargo 特性：
- `backend-sqlite`：SQLite 数据库支持
- `rusqlite`：带捆绑 SQLite 的 SQLite 绑定
- `sqlite-vec`：SQLite 向量扩展（可选）

## 安装

### 从源码构建
```bash
# 默认构建（profile-local）
cargo build

# 显式本地模式构建
cargo build --no-default-features -F profile-local

# 包含所有特性
cargo build --features "backend-sqlite"
```

### 二进制分发
```bash
# 下载预构建二进制文件
curl -L https://github.com/your-org/go-on/releases/latest/download/go-on-x86_64-unknown-linux-gnu.tar.gz | tar xz

# 设为可执行
chmod +x go-on
```

## 设置

### 初始配置
```bash
# 使用默认配置初始化
cargo run -- --init --config config/config.toml

# 检查配置
cargo run -- --check --config config/config.toml
```

### 可选设置级别
```bash
# 快速设置（最小化配置）
cargo run -- --init --setup-level quick --config config/config.toml

# 标准设置（推荐）
cargo run -- --init --setup-level standard --config config/config.toml

# 自定义设置（高级）
cargo run -- --init --setup-level custom --config config/config.toml
```

## 运行

### 启动运行时
```bash
# 使用启动脚本
./scripts/start-go-on.sh

# 直接执行
cargo run -- --config config/config.toml

# 使用特定协议模式
cargo run -- --config config/config.toml --protocol-mode adaptive
```

### 健康检查
```bash
# 默认健康端点
curl http://127.0.0.1:8090/health

# 详细输出
curl http://127.0.0.1:8090/health?verbose=true
```

## 开发工作流

### 典型使用模式
1. **启动运行时**：`./scripts/start-go-on.sh`
2. **连接 IDE**：配置 Zed 或 VS Code 使用本地 go-on
3. **开发**：使用 AI 辅助编码功能
4. **监控**：检查健康端点状态
5. **停止**：使用 `./scripts/stop-go-on.sh` 或 Ctrl+C

### IDE 集成
- **Zed**：使用 ACP over stdio 或 HTTP
- **VS Code**：使用 go-on 扩展与本地运行时
- **GUI 控制台**：基于 Tauri 的桌面界面

## 存储管理

### 缓存位置
- **默认**：当前目录下的 `acp_cache.sqlite3`
- **自定义**：在配置中设置 `cache.path`
- **大小限制**：默认 5000 条记录

### 向量存储
- **位置**：当前目录下的 `acp_vector.sqlite3`
- **维度**：192 维嵌入
- **自动模式**：自动使用可用的向量扩展

### 维护
```bash
# 清理缓存（手动）
rm -f acp_cache.sqlite3 acp_cache.sqlite3-*

# 重置向量存储
rm -f acp_vector.sqlite3

# 压缩 SQLite 数据库
sqlite3 acp_cache.sqlite3 "VACUUM;"
sqlite3 acp_vector.sqlite3 "VACUUM;"
```

## 性能调优

### 内存设置
```toml
[runtime]
# 根据可用内存调整
cache_max_memory_mb = 256
vector_max_memory_mb = 512
```

### 并发
```toml
[concurrency]
# 最大并发请求数
max_inflight_requests = 32
max_parallel_tasks = 8
```

### 超时
```toml
[timeouts]
# 请求超时
request_timeout_seconds = 120
health_check_timeout_seconds = 30
shutdown_timeout_seconds = 60
```

## 故障排除

### 常见问题

#### SQLite 错误
```bash
# 检查 SQLite 版本
sqlite3 --version

# 修复损坏的数据库
sqlite3 acp_cache.sqlite3 ".recover" | sqlite3 acp_cache_fixed.sqlite3
```

#### 向量存储问题
```bash
# 检查 sqlite-vec 可用性
cargo build --features "sqlite-vec"

# 回退到 JSON 模式
[vector]
auto_mode = false
use_json_fallback = true
```

#### 端口冲突
```bash
# 检查端口使用情况
lsof -i :8090

# 在配置中更改端口
[runtime]
acp_http_bind_addr = "127.0.0.1:8091"
```

### 日志
```bash
# 启用调试日志
RUST_LOG=debug ./scripts/start-go-on.sh

# 查看日志
tail -f go-on.log
```

## 迁移

### 从旧版本迁移
```bash
# 备份现有数据
cp acp_cache.sqlite3 acp_cache.sqlite3.backup
cp acp_vector.sqlite3 acp_vector.sqlite3.backup

# 运行迁移
cargo run -- --migrate --config config/config.toml
```

### 迁移到其他部署模式
本地模式可以迁移到：
- **简单服务器模式**：用于单服务器部署
- **多用户服务器模式**：用于生产多用户环境

## 最佳实践

### 安全
- 将配置文件保存在版本控制中（排除密钥）
- 使用环境变量存储 API 密钥
- 定期更新到最新版本

### 性能
- 将 SQLite 文件放在快速存储上（SSD）
- 监控磁盘空间使用情况
- 定期维护（压缩、分析）

### 开发
- 为不同项目使用单独的配置
- 备份重要的向量存储
- 使用不同的模型供应商进行测试

## 限制

### 已知约束
- **仅限单用户**：不支持并发多用户访问
- **本地存储**：性能取决于本地磁盘速度
- **内存限制**：受可用系统内存限制
- **无高可用性**：单点故障

### 何时考虑其他模式
考虑升级到：
- **简单服务器模式**：需要更好性能时
- **多用户服务器模式**：需要多用户支持时

## 下一步

设置本地模式后，您可以：
1. 探索 [API 文档](../api/overview.md)
2. 了解 [简单服务器模式](./simple-server.md)
3. 查看 [故障排除指南](../troubleshooting.md)
4. 加入 [社区讨论](https://github.com/your-org/go-on/discussions)