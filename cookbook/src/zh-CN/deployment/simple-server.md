# 简单服务器模式部署

## 概述

简单服务器模式（`simple-server`）专为单服务器部署设计，提供比本地模式更好的性能和可靠性，同时保持简单性。它使用带有必需向量扩展的 SQLite，适用于小团队或类似生产环境。

## 特性

### 增强能力
- **单服务器部署**：专为专用服务器环境设计
- **必需向量扩展**：使用 `sqlite-vec`（无 JSON 回退）
- **改进的性能**：针对服务器工作负载优化
- **更好的可靠性**：增强的错误处理和恢复
- **生产就绪**：适用于小规模生产使用
- **完整子总线架构**：全部 7 个特性门控子总线（含 distributed-memory），见 `Cargo.toml`
- **条件编译模块**：`DistributedMemoryBus` 使用 `#[cfg(feature = "sub-bus-distributed-memory")]` 门控（由 `simple-server` 配置启用）

### 架构
```
简单服务器架构：
├── 运行时：专用服务器进程
├── 存储：带向量扩展的 SQLite
├── 网络：HTTP/HTTPS 端点
└── 监控：增强的可观测性
```

## 配置

### 服务器配置
创建 `config/config.simple-server.toml`：

```toml
# config/config.simple-server.toml
default_phase = "coding"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"  # 服务器部署的 HTTP 模式

[runtime]
acp_http_bind_addr = "0.0.0.0:8090"  # 绑定到所有接口
production_strict = true
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_SERVER_API_KEY"
entry_rate_limit_rpm = 1000
entry_rate_limit_burst = 200

[cache]
enabled = true
path = "/var/lib/go-on/cache.sqlite3"
default_ttl_seconds = 7200
max_entries = 20000

[vector]
enabled = true
auto_mode = false  # 需要 sqlite-vec
path = "/var/lib/go-on/vector.sqlite3"
dimensions = 384  # 更高维度以获得更好准确性
top_k = 5
min_similarity = 0.75

# OpenTelemetry 设置位于 [runtime] 内
[runtime]
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
```

### 特性标志
简单服务器模式需要：
- `backend-sqlite`：SQLite 数据库支持
- `sqlite-vec`：向量扩展（必需，无回退）

## 安装

### 为服务器部署构建
```bash
# 使用 simple-server 配置文件构建
cargo build --no-default-features -F simple-server

# 或显式启用特性
cargo build --features "backend-sqlite sqlite-vec"
```

### 系统要求
- **CPU**：推荐 2+ 核心
- **内存**：4GB+ RAM
- **存储**：10GB+ 可用空间（推荐 SSD）
- **网络**：稳定的互联网连接用于模型供应商

### Systemd 服务设置
创建 `/etc/systemd/system/go-on.service`：

```ini
[Unit]
Description=go-on Simple Server
After=network.target

[Service]
Type=simple
User=go-on
Group=go-on
WorkingDirectory=/opt/go-on
Environment="GO_ON_SERVER_API_KEY=your-api-key-here"
Environment="RUST_LOG=info"
ExecStart=/opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

## 设置

### 目录结构
```bash
# 创建目录
sudo mkdir -p /opt/go-on /var/lib/go-on /var/log/go-on
sudo chown -R go-on:go-on /opt/go-on /var/lib/go-on /var/log/go-on

# 复制配置
sudo cp config/config.simple-server.toml /opt/go-on/config/
sudo cp scripts/start-go-on.sh /opt/go-on/
sudo chmod +x /opt/go-on/start-go-on.sh
```

### 数据库初始化
```bash
# 以 go-on 用户身份初始化
sudo -u go-on /opt/go-on/go-on --init --config /opt/go-on/config/config.simple-server.toml

# 检查配置
sudo -u go-on /opt/go-on/go-on --check --config /opt/go-on/config/config.simple-server.toml
```

### 用户和权限
```bash
# 创建系统用户
sudo useradd -r -s /bin/false -m -d /opt/go-on go-on

# 设置权限
sudo chown -R go-on:go-on /opt/go-on
sudo chmod 750 /opt/go-on
```

## 运行

### 启动服务器
```bash
# 使用 systemd
sudo systemctl daemon-reload
sudo systemctl enable go-on
sudo systemctl start go-on

# 检查状态
sudo systemctl status go-on

# 查看日志
sudo journalctl -u go-on -f
```

### 手动启动
```bash
# 以 go-on 用户身份
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml

# 带环境变量
GO_ON_SERVER_API_KEY="your-key" sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml
```

### 健康和监控
```bash
# 健康端点
curl http://localhost:8090/health

# Prometheus 指标（文本格式，由 ACP HTTP 端口提供）
curl http://localhost:8090/metrics
```

## 网络配置

### 防火墙规则
```bash
# 允许 HTTP 端口
sudo ufw allow 8090/tcp

# 启用防火墙
sudo ufw enable
```

### 反向代理（Nginx）
创建 `/etc/nginx/sites-available/go-on`：

```nginx
server {
    listen 80;
    server_name go-on.example.com;
    
    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
    
    # 健康检查端点
    location /health {
        proxy_pass http://127.0.0.1:8090/health;
        access_log off;
    }
}
```

### SSL/TLS 配置
```bash
# 使用 Let's Encrypt 和 Certbot
sudo certbot --nginx -d go-on.example.com

# 或手动 SSL 配置
ssl_certificate /etc/ssl/certs/go-on.crt;
ssl_certificate_key /etc/ssl/private/go-on.key;
```

## 存储管理

### 数据库维护
```bash
# 定期压缩（每周）
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "VACUUM;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "VACUUM;"

# 分析查询优化
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "ANALYZE;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "ANALYZE;"
```

### 备份策略
```bash
# 每日备份脚本
#!/bin/bash
BACKUP_DIR="/backup/go-on"
DATE=$(date +%Y%m%d)

# 备份数据库
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 ".backup $BACKUP_DIR/cache-$DATE.sqlite3"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 ".backup $BACKUP_DIR/vector-$DATE.sqlite3"

# 备份配置
cp /opt/go-on/config/config.simple-server.toml $BACKUP_DIR/config-$DATE.toml

# 轮转旧备份（保留 30 天）
find $BACKUP_DIR -name "*.sqlite3" -mtime +30 -delete
find $BACKUP_DIR -name "*.toml" -mtime +30 -delete
```

### 磁盘空间监控
```bash
# 检查数据库大小
du -h /var/lib/go-on/*.sqlite3

# 监控增长
df -h /var/lib/go-on
```

## 性能调优

### 内存与并发
并发限制通过 `[phases.<name>.options]` 按阶段配置（`phase_max_inflight` / `global_max_inflight`），
入口限流通过 `[runtime]` 配置（`entry_rate_limit_rpm` / `entry_rate_limit_burst`）。
不存在 `[concurrency]` 或 `[timeouts]` 顶层段。

## 安全

### API 密钥管理
```bash
# 在环境中设置 API 密钥
export GO_ON_SERVER_API_KEY="secure-random-key-here"

# 或使用密钥环
keyring set go-on server-api-key
```

### 速率限制
入口限流在 `[runtime]` 中配置：

```toml
[runtime]
entry_rate_limit_rpm = 1000
entry_rate_limit_burst = 200
```

### 访问控制
CORS 与入口认证在 `[runtime]` 中配置：

```toml
[runtime]
entry_auth_enabled = true
entry_auth_api_key_env = "GO_ON_SERVER_API_KEY"
cors_allowed_origins = ["https://your-domain.com"]
```

> 不存在 `[security]` 或 `[access]` 段。IP 白/黑名单与 HTTPS 强制开关不受支持；
> 请使用防火墙/反向代理实现这些控制。

## 监控和日志

### 日志
通过 `RUST_LOG` 环境变量设置日志级别；不存在 `[logging]` 段，也没有 `--verbose` 标志。
日志输出到 stderr，可由 systemd 或日志管理器重定向。

### 指标收集
Prometheus 格式指标由 ACP HTTP 端口（8090）的 `GET /metrics` 提供——无需独立指标端口。

### 告警
```bash
# Prometheus 告警规则示例
groups:
- name: go-on-alerts
  rules:
  - alert: GoOnHighErrorRate
    expr: rate(go_on_errors_total[5m]) > 0.1
    for: 2m
    labels:
      severity: warning
    annotations:
      summary: "检测到高错误率"
      description: "错误率为每秒 {{ $value }}"
```

## 扩展考虑

### 何时扩展
- CPU 使用率持续高于 70%
- 内存使用率持续高于 80%
- 响应时间显著增加
- 并发用户超过 50

### 扩展选项
1. **垂直扩展**：升级服务器资源
2. **水平扩展**：迁移到多用户服务器模式
3. **负载均衡**：添加多个简单服务器实例

## 迁移

### 从本地模式迁移
go-on 没有 `--export`/`--import` CLI；直接复制 SQLite 数据文件即可：

```bash
# 先停止两个实例，然后复制数据文件
scp ./sqlite3/acp_cache.sqlite3 go-on@server:/var/lib/go-on/cache.sqlite3
scp ./sqlite3/acp_vector.sqlite3 go-on@server:/var/lib/go-on/vector.sqlite3
```

### 备份和恢复
```bash
# 完整备份
tar czf go-on-backup-$(date +%Y%m%d).tar.gz /opt/go-on /var/lib/go-on

# 恢复
tar xzf go-on-backup-20240101.tar.gz -C /
```

## 故障排除

### 常见问题

#### 服务无法启动
```bash
# 检查日志
sudo journalctl -u go-on --no-pager -n 50

# 检查权限
sudo ls -la /opt/go-on/
sudo ls -la /var/lib/go-on/

# 手动测试
sudo -u go-on /opt/go-on/go-on --config /opt/go-on/config/config.simple-server.toml --validate-config
```

#### 数据库问题
```bash
# 检查 SQLite 完整性
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "PRAGMA integrity_check;"
sudo -u go-on sqlite3 /var/lib/go-on/vector.sqlite3 "PRAGMA integrity_check;"

# 需要时修复
sudo -u go-on cp /var/lib/go-on/cache.sqlite3 /var/lib/go-on/cache.sqlite3.backup
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3.backup ".recover" | sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3
```

#### 性能问题
```bash
# 监控资源使用情况
top -u go-on
iotop -u go-on

# 检查数据库性能
sudo -u go-on sqlite3 /var/lib/go-on/cache.sqlite3 "EXPLAIN QUERY PLAN SELECT * FROM cache WHERE key = 'test';"
```

## 下一步

设置简单服务器模式后，您可以：
1. 配置 [监控和告警](./monitoring.md)
2. 设置 [备份策略](./backup.md)
3. 探索 [API 文档](../api/overview.md)
4. 考虑 [多用户服务器模式](./multi-users-server.md) 用于更大规模部署