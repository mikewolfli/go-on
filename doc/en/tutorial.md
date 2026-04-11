# go-on Tutorial (English)

## 1. Build and Run
1. Install Rust toolchain (recommended: rustup).
2. In project root, run:
   ```bash
   cargo build --release
   ```
3. Start the service:
   ```bash
   ./target/release/go-on
   ```

## 2. Configuration
- Reads `config.toml` by default, use `--config` to specify another file.
- Supports hot-reload and i18n.

## 3. Common Commands
- Validate config:
  ```bash
  ./target/release/go-on --validate-config
  ```
- Run with specific phase:
  ```bash
  ./target/release/go-on --phase review
  ```

## 4. HTTP API Usage
- Chat endpoint: `/chat`, accepts POST JSON.
- Streaming endpoint: `/chat/stream`.
- Health check: `/health`.

## 5. Logging & Debugging
- Use `--verbose` for detailed logs.
- Logging uses tracing, supports multiple levels.

## 6. Advanced
- Multi-agent, custom flows, i18n, etc.
- See source code and config docs for details.
