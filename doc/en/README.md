# go-on English Help

This project is a backend service for multi-agent routing and governance, supporting various AI agents, workflow orchestration, and hot-reloadable configuration.

## Main Features
- Multi-phase task flow auto-routing
- Supports multiple AI agents (OpenAI, Anthropic, Moonshot, etc.)
- Hot-reloadable config and i18n
- Structured logging and performance monitoring
- CLI and HTTP API entrypoints

## Quick Start
1. Install Rust toolchain and build:
   ```bash
   cargo build --release
   ```
2. Run the service:
   ```bash
   ./target/release/go-on
   ```
3. Show CLI options:
   ```bash
   ./target/release/go-on --help
   ```

## Main CLI Options
- `--config` Specify config file
- `--phase` Specify phase
- `--validate-config` Validate config
- `--verbose` Enable verbose logging

## HTTP API
- `/chat` Chat endpoint
- `/chat/stream` Streaming chat endpoint
- `/health` Health check

See the tutorial for advanced usage and configuration.
