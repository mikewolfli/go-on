# 核心運行時 API

## 概述

核心運行時 API 提供系統初始化、關閉、配置管理和基本運行時操作的端點。這些端點對於管理 go-on 運行時生命週期至關重要。

## 端點

### 健康檢查

#### GET /health
檢查運行時的整體健康狀態。

**請求：**
```http
GET /health HTTP/1.1
Host: localhost:8090
Accept: application/json
```

**查詢參數：**
- `verbose` (boolean, 可選)：包含詳細的組件狀態
- `timeout` (integer, 可選)：超時時間（毫秒，默認：5000）

**響應：**
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

**狀態碼：**
- `200 OK`：運行時健康
- `503 Service Unavailable`：運行時不健康

#### GET /health/ready
檢查運行時是否準備好接受請求。

**響應：**
```json
{
  "status": "ready",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

#### GET /health/live
檢查運行時進程是否存活（存活探針）。

**響應：**
```json
{
  "status": "alive",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

### 運行時信息

#### GET /runtime/info
獲取詳細的運行時信息。

**響應：**
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
獲取運行時統計信息。

**響應：**
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
獲取當前運行時配置。

**查詢參數：**
- `include_secrets` (boolean, 可選)：包含密鑰值（默認：false）
- `format` (string, 可選)：輸出格式：`json` 或 `toml`（默認：`json`）

**響應：**
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
從磁盤重新加載配置。

**請求：**
```http
POST /config/reload HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**響應：**
```json
{
  "success": true,
  "message": "配置重新加載成功",
  "timestamp": "2024-01-01T00:00:00Z",
  "changes": {
    "added": [],
    "modified": ["cache.max_entries"],
    "removed": []
  }
}
```

#### PUT /config
更新運行時配置。

**請求：**
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

**響應：**
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
使用新配置初始化運行時。

**請求：**
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

**查詢參數：**
- `setup_level` (string, 可選)：`quick`、`standard` 或 `custom`（默認：`standard`）
- `force` (boolean, 可選)：強制重新初始化（默認：false）

**響應：**
```json
{
  "success": true,
  "message": "運行時初始化成功",
  "timestamp": "2024-01-01T00:00:00Z",
  "config_path": "/path/to/config.toml",
  "data_directory": "/path/to/data",
  "components_initialized": ["database", "cache", "vector_store"]
}
```

### 關閉

#### POST /shutdown
優雅地關閉運行時。

**請求：**
```http
POST /shutdown HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "timeout_seconds": 30,
  "drain_connections": true
}
```

**查詢參數：**
- `timeout_seconds` (integer, 可選)：優雅關閉的超時時間（默認：30）
- `drain_connections` (boolean, 可選)：等待活動連接完成（默認：true）

**響應：**
```json
{
  "success": true,
  "message": "關閉已啟動",
  "timestamp": "2024-01-01T00:00:00Z",
  "shutdown_timeout_seconds": 30
}
```

### 協議管理

#### GET /protocols
獲取可用的協議模式及其狀態。

**響應：**
```json
{
  "current_mode": "adaptive",
  "available_modes": [
    {
      "name": "adaptive",
      "description": "具有自適應路由的雙棧能力",
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
激活特定的協議模式。

**請求：**
```http
POST /protocols/acp_http/activate HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**響應：**
```json
{
  "success": true,
  "message": "協議模式已激活",
  "timestamp": "2024-01-01T00:00:00Z",
  "previous_mode": "adaptive",
  "new_mode": "acp_http",
  "requires_restart": false
}
```

### 特性管理

#### GET /features
獲取啟用的特性及其狀態。

**響應：**
```json
{
  "features": [
    {
      "name": "backend-sqlite",
      "enabled": true,
      "description": "SQLite 數據庫支持",
      "version": "0.39.0"
    },
    {
      "name": "sqlite-vec",
      "enabled": true,
      "description": "SQLite 的向量擴展",
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
啟用特定特性。

**請求：**
```http
POST /features/otel/enable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**響應：**
```json
{
  "success": true,
  "message": "特性已啟用",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "otel",
  "requires_restart": true
}
```

#### POST /features/{name}/disable
禁用特定特性。

**請求：**
```http
POST /features/sqlite-vec/disable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**響應：**
```json
{
  "success": true,
  "message": "特性已禁用",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "sqlite-vec",
  "requires_restart": true
}
```

### 維護操作

#### POST /maintenance/gc
運行垃圾回收。

**請求：**
```http
POST /maintenance/gc HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "components": ["cache", "vector_store"],
  "aggressive": false
}
```

**響應：**
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
壓縮數據庫。

**請求：**
```http
POST /maintenance/vacuum HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "databases": ["cache", "vector_store"],
  "analyze": true
}
```

**響應：**
```json
{
  "success": true,
  "message": "壓縮完成",
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

### 診斷

#### GET /diagnostics
獲取運行時診斷信息。

**查詢參數：**
- `level` (string, 可選)：`basic`、`detailed` 或 `full`（默認：`basic`）
- `include_logs` (boolean, 可選)：包含最近日誌（默認：false）

**響應：**
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
      "message": "向量存儲接近容量限制",
      "suggestion": "考慮增加 max_entries 或運行維護"
    }
  ]
}
```

## WebSocket 端點

### WS /ws/runtime
實時運行時更新。

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

### 健康檢查
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

### 關閉
```bash
go-on --shutdown
go-on --shutdown --timeout 60
```

## 錯誤碼

### 常見錯誤
- `RUNTIME_NOT_INITIALIZED`：運行時未初始化
- `CONFIG_INVALID`：配置無效或格式錯誤
- `FEATURE_NOT_AVAILABLE`：請求的特性不可用
- `PROTOCOL_NOT_SUPPORTED`：請求的協議不支持
- `MAINTENANCE_IN_PROGRESS`：維護操作已在進行中

### 錯誤示例
```json
{
  "error": {
    "code": "RUNTIME_NOT_INITIALIZED",
    "message": "運行時未初始化。請先運行 /initialize。",
    "details": {
      "required_action": "initialize"
    }
  }
}
```

## 速率限制

### 默認限制
- 健康端點：每分鐘 60 個請求
- 配置端點：每分鐘 30 個請求
- 維護端點：每分鐘 10 個請求

### 頭部
```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 55
X-RateLimit-Reset: 1614556800
```

## 安全考慮

### 認證
- 本地模式：可選的 API 密鑰
- 服務器模式：必需的 API 密鑰
- 敏感操作：始終需要認證

### 授權
- 健康端點：公開（只讀）
- 配置端點：需要管理員權限
- 維護端點：需要管理員權限

### 審計日誌
所有配置更改和維護操作都記錄到審計日誌中。

## 最佳實踐

### 健康監控
- 使用 `/health` 進行就緒探針
- 使用 `/health/live` 進行存活探針
- 監控響應時間和錯誤率

### 配置管理
- 使用版本控制管理配置文件
- 先在暫存環境測試配置更改
- 使用 `/config/reload` 進行動態更新

### 維護調度
- 在非高峰時段安排維護
- 運行壓縮前監控磁盤空間
- 重大維護操作前備份數據

## 下一步

- 探索 [安全和治理 API](./safety-governance.md)
- 瞭解 [可觀測性 API](./observability.md)
- 查看 [工作流和任務 API](./workflow-task.md)