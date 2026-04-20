# 核心运行时 API

## 概述

核心运行时 API 提供系统初始化、关闭、配置管理和基本运行时操作的端点。这些端点对于管理 go-on 运行时生命周期至关重要。

## 端点

### 健康检查

#### GET /health
检查运行时的整体健康状态。

**请求：**
```http
GET /health HTTP/1.1
Host: localhost:8090
Accept: application/json
```

**查询参数：**
- `verbose` (boolean, 可选)：包含详细的组件状态
- `timeout` (integer, 可选)：超时时间（毫秒，默认：5000）

**响应：**
```json
{
  "status": "healthy",
  "timestamp": "2024-01-01T00:00:00Z",
  "version": "0.6.1",
  "uptime_seconds": 3600,
  "components": {
    "database": "healthy",
    "cache": "healthy",
    "vector_store": "healthy",
    "model_providers": {
      "openai": "healthy",
      "anthropic": "healthy"
    }
  }
}
```

**状态码：**
- `200 OK`：运行时健康
- `503 Service Unavailable`：运行时不健康

#### GET /health/ready
检查运行时是否准备好接受请求。

**响应：**
```json
{
  "status": "ready",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

#### GET /health/live
检查运行时进程是否存活（存活探针）。

**响应：**
```json
{
  "status": "alive",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

### 运行时信息

#### GET /runtime/info
获取详细的运行时信息。

**响应：**
```json
{
  "version": "0.6.1",
  "build_date": "2024-01-01T00:00:00Z",
  "git_commit": "a1b2c3d4e5f6",
  "features": ["backend-sqlite", "sqlite-vec"],
  "protocols": ["acp_stdio", "acp_http", "mcp_stdio", "mcp_http"],
  "config_path": "/path/to/config.toml",
  "data_directory": "/path/to/data",
  "start_time": "2024-01-01T00:00:00Z",
  "uptime_seconds": 3600
}
```

#### GET /runtime/stats
获取运行时统计信息。

**响应：**
```json
{
  "requests": {
    "total": 1000,
    "successful": 950,
    "failed": 50,
    "rate_per_second": 10.5
  },
  "memory": {
    "used_mb": 256,
    "total_mb": 1024,
    "peak_mb": 512
  },
  "cache": {
    "hits": 800,
    "misses": 200,
    "hit_rate": 0.8,
    "size": 5000,
    "max_size": 10000
  },
  "vector_store": {
    "entries": 1000,
    "searches": 500,
    "avg_search_time_ms": 150
  }
}
```

### 配置管理

#### GET /config
获取当前运行时配置。

**查询参数：**
- `include_secrets` (boolean, 可选)：包含密钥值（默认：false）
- `format` (string, 可选)：输出格式：`json` 或 `toml`（默认：`json`）

**响应：**
```json
{
  "default_phase": "coding",
  "model_selection_mode": "adaptive",
  "protocol": {
    "mode": "adaptive"
  },
  "cache": {
    "enabled": true,
    "path": "acp_cache.sqlite3",
    "default_ttl_seconds": 3600,
    "max_entries": 5000
  },
  "vector": {
    "enabled": true,
    "auto_mode": true,
    "path": "acp_vector.sqlite3",
    "dimensions": 192
  }
}
```

#### POST /config/reload
从磁盘重新加载配置。

**请求：**
```http
POST /config/reload HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**响应：**
```json
{
  "success": true,
  "message": "配置重新加载成功",
  "timestamp": "2024-01-01T00:00:00Z",
  "changes": {
    "added": [],
    "modified": ["cache.max_entries"],
    "removed": []
  }
}
```

#### PUT /config
更新运行时配置。

**请求：**
```http
PUT /config HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "cache": {
    "max_entries": 10000
  }
}
```

**响应：**
```json
{
  "success": true,
  "message": "配置更新成功",
  "timestamp": "2024-01-01T00:00:00Z",
  "requires_restart": false
}
```

### 初始化

#### POST /initialize
使用新配置初始化运行时。

**请求：**
```http
POST /initialize HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "setup_level": "standard",
  "config_overrides": {
    "default_phase": "coding"
  }
}
```

**查询参数：**
- `setup_level` (string, 可选)：`quick`、`standard` 或 `custom`（默认：`standard`）
- `force` (boolean, 可选)：强制重新初始化（默认：false）

**响应：**
```json
{
  "success": true,
  "message": "运行时初始化成功",
  "timestamp": "2024-01-01T00:00:00Z",
  "config_path": "/path/to/config.toml",
  "data_directory": "/path/to/data",
  "components_initialized": ["database", "cache", "vector_store"]
}
```

### 关闭

#### POST /shutdown
优雅地关闭运行时。

**请求：**
```http
POST /shutdown HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "timeout_seconds": 30,
  "drain_connections": true
}
```

**查询参数：**
- `timeout_seconds` (integer, 可选)：优雅关闭的超时时间（默认：30）
- `drain_connections` (boolean, 可选)：等待活动连接完成（默认：true）

**响应：**
```json
{
  "success": true,
  "message": "关闭已启动",
  "timestamp": "2024-01-01T00:00:00Z",
  "shutdown_timeout_seconds": 30
}
```

### 协议管理

#### GET /protocols
获取可用的协议模式及其状态。

**响应：**
```json
{
  "current_mode": "adaptive",
  "available_modes": [
    {
      "name": "adaptive",
      "description": "具有自适应路由的双栈能力",
      "enabled": true,
      "active": true
    },
    {
      "name": "acp_stdio",
      "description": "ACP over stdio",
      "enabled": true,
      "active": false
    },
    {
      "name": "acp_http",
      "description": "ACP over HTTP",
      "enabled": true,
      "active": false
    },
    {
      "name": "mcp_stdio",
      "description": "MCP over stdio",
      "enabled": true,
      "active": false
    },
    {
      "name": "mcp_http",
      "description": "MCP over HTTP",
      "enabled": true,
      "active": false
    }
  ]
}
```

#### POST /protocols/{mode}/activate
激活特定的协议模式。

**请求：**
```http
POST /protocols/acp_http/activate HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**响应：**
```json
{
  "success": true,
  "message": "协议模式已激活",
  "timestamp": "2024-01-01T00:00:00Z",
  "previous_mode": "adaptive",
  "new_mode": "acp_http",
  "requires_restart": false
}
```

### 特性管理

#### GET /features
获取启用的特性及其状态。

**响应：**
```json
{
  "features": [
    {
      "name": "backend-sqlite",
      "enabled": true,
      "description": "SQLite 数据库支持",
      "version": "0.39.0"
    },
    {
      "name": "sqlite-vec",
      "enabled": true,
      "description": "SQLite 的向量扩展",
      "version": "0.1.9"
    },
    {
      "name": "otel",
      "enabled": false,
      "description": "OpenTelemetry 支持",
      "version": null
    }
  ]
}
```

#### POST /features/{name}/enable
启用特定特性。

**请求：**
```http
POST /features/otel/enable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**响应：**
```json
{
  "success": true,
  "message": "特性已启用",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "otel",
  "requires_restart": true
}
```

#### POST /features/{name}/disable
禁用特定特性。

**请求：**
```http
POST /features/sqlite-vec/disable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**响应：**
```json
{
  "success": true,
  "message": "特性已禁用",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "sqlite-vec",
  "requires_restart": true
}
```

### 维护操作

#### POST /maintenance/gc
运行垃圾回收。

**请求：**
```http
POST /maintenance/gc HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "components": ["cache", "vector_store"],
  "aggressive": false
}
```

**响应：**
```json
{
  "success": true,
  "message": "垃圾回收完成",
  "timestamp": "2024-01-01T00:00:00Z",
  "components": {
    "cache": {
      "entries_removed": 100,
      "space_freed_mb": 10
    },
    "vector_store": {
      "entries_removed": 50,
      "space_freed_mb": 5
    }
  }
}
```

#### POST /maintenance/vacuum
压缩数据库。

**请求：**
```http
POST /maintenance/vacuum HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "databases": ["cache", "vector_store"],
  "analyze": true
}
```

**响应：**
```json
{
  "success": true,
  "message": "压缩完成",
  "timestamp": "2024-01-01T00:00:00Z",
  "databases": {
    "cache": {
      "size_before_mb": 100,
      "size_after_mb": 80,
      "space_freed_mb": 20
    },
    "vector_store": {
      "size_before_mb": 200,
      "size_after_mb": 150,
      "space_freed_mb": 50
    }
  }
}
```

### 诊断

#### GET /diagnostics
获取运行时诊断信息。

**查询参数：**
- `level` (string, 可选)：`basic`、`detailed` 或 `full`（默认：`basic`）
- `include_logs` (boolean, 可选)：包含最近日志（默认：false）

**响应：**
```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "system": {
    "os": "Linux",
    "arch": "x86_64",
    "cpu_cores": 8,
    "total_memory_mb": 16384,
    "available_memory_mb": 8192
  },
  "runtime": {
    "version": "0.6.1",
    "uptime_seconds": 3600,
    "threads": 12,
    "memory_usage_mb": 256
  },
  "components": {
    "database": {
      "status": "healthy",
      "connections": 5,
      "size_mb": 100
    },
    "cache": {
      "status": "healthy",
      "entries": 5000,
      "hit_rate": 0.85
    }
  },
  "issues": [
    {
      "level": "warning",
      "component": "vector_store",
      "message": "向量存储接近容量限制",
      "suggestion": "考虑增加 max_entries 或运行维护"
    }
  ]
}
```

## WebSocket 端点

### WS /ws/runtime
实时运行时更新。

**事件：**
```json
{
  "type": "runtime.status_changed",
  "data": {
    "status": "healthy",
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

```json
{
  "type": "config.updated",
  "data": {
    "path": "cache.max_entries",
    "old_value": 5000,
    "new_value": 10000,
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

## 命令行界面

### 健康检查
```bash
go-on --health
go-on --health --verbose
go-on --health --timeout 10000
```

### 配置
```bash
go-on --config-show
go-on --config-show --format toml
go-on --config-reload
go-on --config-update '{"cache.max_entries": 10000}'
```

### 初始化
```bash
go-on --init
go-on --init --setup-level standard
go-on --init --config config.toml
```

### 关闭
```bash
go-on --shutdown
go-on --shutdown --timeout 60
```

## 错误码

### 常见错误
- `RUNTIME_NOT_INITIALIZED`：运行时未初始化
- `CONFIG_INVALID`：配置无效或格式错误
- `FEATURE_NOT_AVAILABLE`：请求的特性不可用
- `PROTOCOL_NOT_SUPPORTED`：请求的协议不支持
- `MAINTENANCE_IN_PROGRESS`：维护操作已在进行中

### 错误示例
```json
{
  "error": {
    "code": "RUNTIME_NOT_INITIALIZED",
    "message": "运行时未初始化。请先运行 /initialize。",
    "details": {
      "required_action": "initialize"
    }
  }
}
```

## 速率限制

### 默认限制
- 健康端点：每分钟 60 个请求
- 配置端点：每分钟 30 个请求
- 维护端点：每分钟 10 个请求

### 头部
```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 55
X-RateLimit-Reset: 1614556800
```

## 安全考虑

### 认证
- 本地模式：可选的 API 密钥
- 服务器模式：必需的 API 密钥
- 敏感操作：始终需要认证

### 授权
- 健康端点：公开（只读）
- 配置端点：需要管理员权限
- 维护端点：需要管理员权限

### 审计日志
所有配置更改和维护操作都记录到审计日志中。

## 最佳实践

### 健康监控
- 使用 `/health` 进行就绪探针
- 使用 `/health/live` 进行存活探针
- 监控响应时间和错误率

### 配置管理
- 使用版本控制管理配置文件
- 先在暂存环境测试配置更改
- 使用 `/config/reload` 进行动态更新

### 维护调度
- 在非高峰时段安排维护
- 运行压缩前监控磁盘空间
- 重大维护操作前备份数据

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 了解 [可观测性 API](./observability.md)
- 查看 [工作流和任务 API](./workflow-task.md)