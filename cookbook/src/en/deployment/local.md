# Local Mode Deployment

## Overview

Local mode (`local`) is the default build profile for go-on, designed for single-user development environments. It provides a lightweight, self-contained runtime with SQLite-based storage and adaptive vector capabilities.

## Features

### Core Capabilities
- **Single-user operation**: Designed for individual developers
- **SQLite storage**: Local file-based cache and vector storage
- **Adaptive vector store**: Uses `sqlite-vec` when available, falls back to JSON embeddings
- **Zero external dependencies**: No database servers required
- **Quick setup**: Minimal configuration needed
- **All 14 capability buses included**: Full tool, orchestration, observability, optimization, memory, and protocol sub-bus support

### Storage Architecture
```
Local Mode Storage:
├── Cache: SQLite database (acp_cache.sqlite3)
├── Vector Store: SQLite with vector extensions
└── Configuration: Local config.toml files
```

## Configuration

### Default Configuration
The local mode uses `config/config.toml` as the default configuration:

```toml
# config/config.toml (local default)
schema_version = "1.0.0"
default_phase = "think"
model_selection_mode = "adaptive"

[protocol]
mode = "acp_http"

[cache]
enabled = true
path = "acp_cache.sqlite3"
default_ttl_seconds = 1800
max_entries = 2000

[vector]
enabled = true
auto_mode = true
path = "acp_vector.sqlite3"
dimensions = 128
min_query_chars = 120
top_k = 2
min_similarity = 0.82
max_snippet_chars = 600
max_entries = 3000
summary_trigger_messages = 4
summary_max_chars = 800

[runtime]
acp_http_bind_addr = "127.0.0.1:8090"
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 30
entry_auth_enabled = false
entry_rate_limit_rpm = 240
entry_rate_limit_burst = 60
i18n_enabled = true
i18n_default_language = "en-US"
governance_enabled = true
governance_policy_mode = "advisory"
skills_enabled = true
skills_cache_dir = "skills_cache"

# OpenTelemetry
otel_enabled = true
otel_exporter = "otlp"
otel_endpoint = "http://localhost:4317"
```

### Feature Flags
Local mode (`local`) enables the following Cargo features:
- `backend-sqlite`: SQLite database support
- `rusqlite`: SQLite bindings with bundled SQLite
- `sqlite-vec`: Vector extension for SQLite (optional)

## Installation

### Building from Source
```bash
# Default build (local)
cargo build

# Explicit local mode build
cargo build --no-default-features -F local
```

### Binary Distribution
```bash
# Download pre-built binary
curl -L https://github.com/your-org/go-on/releases/latest/download/go-on-x86_64-unknown-linux-gnu.tar.gz | tar xz

# Make executable
chmod +x go-on
```

## Setup

### Initial Configuration
```bash
# Initialize with default config
cargo run -- --init --config config/config.toml

# Check configuration
cargo run -- --check --config config/config.toml
```

### Optional Setup Levels
```bash
# Quick setup (minimal configuration)
cargo run -- --init --setup-level quick --config config/config.toml

# Standard setup (recommended)
cargo run -- --init --setup-level standard --config config/config.toml

# Custom setup (advanced)
cargo run -- --init --setup-level custom --config config/config.toml
```

## Running

### Starting the Runtime
```bash
# Using the start script
./scripts/start-go-on.sh

# Direct execution
cargo run -- --config config/config.toml

# With specific protocol mode
cargo run -- --config config/config.toml --protocol-mode adaptive
```

### Health Check
```bash
# Default health endpoint
curl http://127.0.0.1:8090/health

# With verbose output
curl http://127.0.0.1:8090/health?verbose=true
```

## Development Workflow

### Typical Usage Pattern
1. **Start the runtime**: `./scripts/start-go-on.sh`
2. **Connect IDE**: Configure Zed or VS Code to use local go-on
3. **Develop**: Use AI-assisted coding features
4. **Monitor**: Check health endpoint for status
5. **Stop**: Use `./scripts/stop-go-on.sh` or Ctrl+C

### IDE Integration
- **Zed**: Uses ACP over stdio or HTTP
- **VS Code**: Uses the go-on extension with local runtime
- **GUI Console**: EGUI (Rust native) desktop GUI

## Storage Management

### Cache Location
- **Default**: `acp_cache.sqlite3` in current directory
- **Custom**: Set `cache.path` in configuration
- **Size limit**: 5000 entries by default

### Vector Store
- **Location**: `acp_vector.sqlite3` in current directory
- **Dimensions**: 192-dimensional embeddings
- **Auto-mode**: Automatically uses available vector extensions

### Maintenance
```bash
# Clean cache (manual)
rm -f acp_cache.sqlite3 acp_cache.sqlite3-*

# Reset vector store
rm -f acp_vector.sqlite3

# Vacuum SQLite databases
sqlite3 acp_cache.sqlite3 "VACUUM;"
sqlite3 acp_vector.sqlite3 "VACUUM;"
```

## Performance Tuning

### Memory Settings
```toml
[runtime]
# Adjust based on available memory
cache_max_memory_mb = 256
vector_max_memory_mb = 512
```

### Concurrency
```toml
[concurrency]
# Maximum concurrent requests
max_inflight_requests = 32
max_parallel_tasks = 8
```

### Timeouts
```toml
[timeouts]
# Request timeouts
request_timeout_seconds = 120
health_check_timeout_seconds = 30
shutdown_timeout_seconds = 60
```

## Troubleshooting

### Common Issues

#### SQLite Errors
```bash
# Check SQLite version
sqlite3 --version

# Repair corrupted database
sqlite3 acp_cache.sqlite3 ".recover" | sqlite3 acp_cache_fixed.sqlite3
```

#### Vector Store Issues
```bash
# Check sqlite-vec availability
cargo build --features "sqlite-vec"

# Fallback to JSON mode
[vector]
auto_mode = false
use_json_fallback = true
```

#### Port Conflicts
```bash
# Check port usage
lsof -i :8090

# Change port in config
[runtime]
acp_http_bind_addr = "127.0.0.1:8091"
```

### Logging
```bash
# Enable debug logging
RUST_LOG=debug ./scripts/start-go-on.sh

# View logs
tail -f go-on.log
```

## Migration

### From Previous Versions
```bash
# Backup existing data
cp acp_cache.sqlite3 acp_cache.sqlite3.backup
cp acp_vector.sqlite3 acp_vector.sqlite3.backup

# Run migration
cargo run -- --migrate --config config/config.toml
```

### To Other Deployment Modes
Local mode can be migrated to:
- **Simple Server Mode**: For single-server deployments
- **Multi-Users Server Mode**: For production multi-user environments

## Best Practices

### Security
- Keep configuration files in version control (excluding secrets)
- Use environment variables for API keys
- Regularly update to latest versions

### Performance
- Place SQLite files on fast storage (SSD)
- Monitor disk space usage
- Regular maintenance (vacuum, analyze)

### Development
- Use separate configurations for different projects
- Backup important vector stores
- Test with different model providers

## Limitations

### Known Constraints
- **Single-user only**: Not designed for concurrent multi-user access
- **Local storage**: Performance depends on local disk speed
- **Memory limits**: Limited by available system memory
- **No high availability**: Single point of failure

### When to Consider Other Modes
Consider upgrading to:
- **Simple Server Mode**: When needing better performance
- **Multi-Users Server Mode**: When requiring multi-user support

## Next Steps

After setting up local mode, you can:
1. Explore [API Documentation](../api/overview.md)
2. Learn about [Simple Server Mode](./simple-server.md)
3. Check [Troubleshooting Guide](../troubleshooting.md)
4. Join the [Community Discussions](https://github.com/your-org/go-on/discussions)