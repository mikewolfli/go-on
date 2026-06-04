# Core Runtime API

## Overview

The Core Runtime API provides endpoints for system initialization, shutdown, configuration management, and basic runtime operations. These endpoints are essential for managing the go-on runtime lifecycle.

## Endpoints

### Health Check

#### GET /health
Check the overall health status of the runtime.

**Request:**
```http
GET /health HTTP/1.1
Host: localhost:8090
Accept: application/json
```

**Query Parameters:**
- `verbose` (boolean, optional): Include detailed component status
- `timeout` (integer, optional): Timeout in milliseconds (default: 5000)

**Response:**
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

**Status Codes:**
- `200 OK`: Runtime is healthy
- `503 Service Unavailable`: Runtime is unhealthy

#### GET /health/ready
Check if the runtime is ready to accept requests.

**Response:**
```json
{
  "status": "ready",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

#### GET /health/live
Check if the runtime process is alive (liveness probe).

**Response:**
```json
{
  "status": "alive",
  "timestamp": "2024-01-01T00:00:00Z"
}
```

### Runtime Information

#### GET /runtime/info
Get detailed runtime information.

**Response:**
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
Get runtime statistics.

**Response:**
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

### Configuration Management

#### GET /config
Get current runtime configuration.

**Query Parameters:**
- `include_secrets` (boolean, optional): Include secret values (default: false)
- `format` (string, optional): Output format: `json` or `toml` (default: `json`)

**Response:**
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
Reload configuration from disk.

**Request:**
```http
POST /config/reload HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**Response:**
```json
{
  "success": true,
  "message": "Configuration reloaded successfully",
  "timestamp": "2024-01-01T00:00:00Z",
  "changes": {
    "added": [],
    "modified": ["cache.max_entries"],
    "removed": []
  }
}
```

#### PUT /config
Update runtime configuration.

**Request:**
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

**Response:**
```json
{
  "success": true,
  "message": "Configuration updated successfully",
  "timestamp": "2024-01-01T00:00:00Z",
  "requires_restart": false
}
```

### Initialization

#### POST /initialize
Initialize the runtime with a new configuration.

**Request:**
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

**Query Parameters:**
- `setup_level` (string, optional): `quick`, `standard`, or `custom` (default: `standard`)
- `force` (boolean, optional): Force reinitialization (default: false)

**Response:**
```json
{
  "success": true,
  "message": "Runtime initialized successfully",
  "timestamp": "2024-01-01T00:00:00Z",
  "config_path": "/path/to/config.toml",
  "data_directory": "/path/to/data",
  "components_initialized": ["database", "cache", "vector_store"]
}
```

### Shutdown

#### POST /shutdown
Gracefully shutdown the runtime.

**Request:**
```http
POST /shutdown HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "timeout_seconds": 30,
  "drain_connections": true
}
```

**Query Parameters:**
- `timeout_seconds` (integer, optional): Timeout for graceful shutdown (default: 30)
- `drain_connections` (boolean, optional): Wait for active connections to complete (default: true)

**Response:**
```json
{
  "success": true,
  "message": "Shutdown initiated",
  "timestamp": "2024-01-01T00:00:00Z",
  "shutdown_timeout_seconds": 30
}
```

### Protocol Management

#### GET /protocols
Get available protocol modes and their status.

**Response:**
```json
{
  "current_mode": "adaptive",
  "available_modes": [
    {
      "name": "adaptive",
      "description": "Dual-stack capability with adaptive routing",
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
Activate a specific protocol mode.

**Request:**
```http
POST /protocols/acp_http/activate HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**Response:**
```json
{
  "success": true,
  "message": "Protocol mode activated",
  "timestamp": "2024-01-01T00:00:00Z",
  "previous_mode": "adaptive",
  "new_mode": "acp_http",
  "requires_restart": false
}
```

### Feature Management

#### GET /features
Get enabled features and their status.

**Response:**
```json
{
  "features": [
    {
      "name": "backend-sqlite",
      "enabled": true,
      "description": "SQLite database support",
      "version": "0.39.0"
    },
    {
      "name": "sqlite-vec",
      "enabled": true,
      "description": "Vector extension for SQLite",
      "version": "0.1.9"
    },
    {
      "name": "otel",
      "enabled": false,
      "description": "OpenTelemetry support",
      "version": null
    }
  ]
}
```

#### POST /features/{name}/enable
Enable a specific feature.

**Request:**
```http
POST /features/otel/enable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**Response:**
```json
{
  "success": true,
  "message": "Feature enabled",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "otel",
  "requires_restart": true
}
```

#### POST /features/{name}/disable
Disable a specific feature.

**Request:**
```http
POST /features/sqlite-vec/disable HTTP/1.1
Host: localhost:8090
Content-Type: application/json
```

**Response:**
```json
{
  "success": true,
  "message": "Feature disabled",
  "timestamp": "2024-01-01T00:00:00Z",
  "feature": "sqlite-vec",
  "requires_restart": true
}
```

### Maintenance Operations

#### POST /maintenance/gc
Run garbage collection.

**Request:**
```http
POST /maintenance/gc HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "components": ["cache", "vector_store"],
  "aggressive": false
}
```

**Response:**
```json
{
  "success": true,
  "message": "Garbage collection completed",
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
Vacuum databases.

**Request:**
```http
POST /maintenance/vacuum HTTP/1.1
Host: localhost:8090
Content-Type: application/json

{
  "databases": ["cache", "vector_store"],
  "analyze": true
}
```

**Response:**
```json
{
  "success": true,
  "message": "Vacuum completed",
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

### Diagnostics

#### GET /diagnostics
Get runtime diagnostics.

**Query Parameters:**
- `level` (string, optional): `basic`, `detailed`, or `full` (default: `basic`)
- `include_logs` (boolean, optional): Include recent logs (default: false)

**Response:**
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
      "message": "Vector store approaching capacity",
      "suggestion": "Consider increasing max_entries or running maintenance"
    }
  ]
}
```

## WebSocket Endpoints

### WS /ws/runtime
Real-time runtime updates.

**Events:**
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

## Command Line Interface

### Health Check
```bash
go-on --health
go-on --health --verbose
go-on --health --timeout 10000
```

### Configuration
```bash
go-on --config-show
go-on --config-show --format toml
go-on --config-reload
go-on --config-update '{"cache.max_entries": 10000}'
```

### Initialization
```bash
go-on --init
go-on --init --setup-level standard
go-on --init --config config.toml
```

### Shutdown
```bash
go-on --shutdown
go-on --shutdown --timeout 60
```

## Error Codes

### Common Errors
- `RUNTIME_NOT_INITIALIZED`: Runtime has not been initialized
- `CONFIG_INVALID`: Configuration is invalid or malformed
- `FEATURE_NOT_AVAILABLE`: Requested feature is not available
- `PROTOCOL_NOT_SUPPORTED`: Requested protocol is not supported
- `MAINTENANCE_IN_PROGRESS`: Maintenance operation already in progress

### Error Examples
```json
{
  "error": {
    "code": "RUNTIME_NOT_INITIALIZED",
    "message": "Runtime has not been initialized. Run /initialize first.",
    "details": {
      "required_action": "initialize"
    }
  }
}
```

## Rate Limiting

### Default Limits
- Health endpoints: 60 requests per minute
- Configuration endpoints: 30 requests per minute
- Maintenance endpoints: 10 requests per minute

### Headers
```
X-RateLimit-Limit: 60
X-RateLimit-Remaining: 55
X-RateLimit-Reset: 1614556800
```

## Security Considerations

### Authentication
- Local mode: Optional API key
- Server modes: Required API key
- Sensitive operations: Always require authentication

### Authorization
- Health endpoints: Public (read-only)
- Configuration endpoints: Require admin privileges
- Maintenance endpoints: Require admin privileges

### Audit Logging
All configuration changes and maintenance operations are logged to the audit log.

## Best Practices

### Health Monitoring
- Use `/health` for readiness probes
- Use `/health/live` for liveness probes
- Monitor response times and error rates

### Configuration Management
- Use version control for configuration files
- Test configuration changes in staging first
- Use `/config/reload` for dynamic updates

### Maintenance Scheduling
- Schedule maintenance during off-peak hours
- Monitor disk space before running vacuum
- Backup data before major maintenance operations

## Next Steps

- Explore [Safety and Governance API](./safety-governance.md)
- Learn about [Observability API](./observability.md)
- Check [Workflow and Task API](./workflow-task.md)