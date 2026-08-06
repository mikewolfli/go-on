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
- **All core sub-buses included**: Full tool, orchestration, observability, optimization, memory, and protocol sub-bus support (distributed-memory is gated to server profiles)

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

### Concurrency and timeouts
Concurrency limits are configured per phase via `[phases.<name>.options]`
(`phase_max_inflight` / `global_max_inflight`) and entry rate limits via
`[runtime]` (`entry_rate_limit_rpm` / `entry_rate_limit_burst`). There are no
`[concurrency]` or `[timeouts]` top-level sections.

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
```

The vector store resolves its mode automatically: in the `local` profile it falls
back to a JSON embedding table when sqlite-vec is unavailable; `simple-server` /
`multi-users-server` require sqlite-vec (or pgvector). There is no
`use_json_fallback` config option.

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
```

Config schema is versioned (`schema_version`); go-on validates and migrates
supported schemas on startup. There is no `--migrate` CLI flag.

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