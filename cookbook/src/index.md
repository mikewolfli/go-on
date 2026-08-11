# go-on Documentation

This book documents the current `1.5.2` architecture and usage model of `go-on`.

The runtime implements a **sub-bus capability architecture** (7 feature-gated sub-buses per `Cargo.toml`) with **37 AI provider integrations**,
a passing test suite (zero failures; see `CHANGELOG.md` for the latest counts), and **zero clippy warnings across all build profiles**.

All 154 JSON-RPC methods (the `ACP_METHODS` whitelist) return a unified `DispatchOutput` enum, with the dispatch layer
handling serialization for JSON-RPC, SSE streaming, text/plain, and silent responses.

It is organized as a trilingual mdBook:

- English chapters start at [Architecture Overview](en/overview.md).
- 简体中文章节从 [架构总览](zh-CN/overview.md) 开始。
- 繁體中文章節從 [架構總覽](zh-TW/overview.md) 開始。

The content is based on the current workspace structure and runtime surfaces:

- Rust backend runtime and CLI (four build profiles: local / simple-server / multi-users-server / full)
- Sub-bus capability architecture (CapabilityBus scheduling coordinator + HarnessBus + 7 feature-gated sub-buses)
- Unified `DispatchOutput` handler dispatch pattern (Json / Error / Stream / Text / Checkpoint / Silent)
- Autonomous agent orchestration, DAG task execution, and cognitive modules
- Setup wizard and secret management (system keyring, Vault)
- Zed integration through ACP stdio and ACP or MCP HTTP
- VS Code addon runtime wiring
- EGUI GUI configuration and operations
- TypeScript SDK (`sdk/typescript/`) — async client consumed by the VS Code addon (replaces the removed duplicate Node.js SDK)
- Governance, security (mTLS, request signing, content safety, prompt injection detection)
- Observability (Prometheus `/metrics`, OTel tracing, governance status endpoint)
- Full i18n coverage (~95%) across backend, GUI, and VS Code addon

Use this book as the operational reference. Root `README` files stay concise; the detailed setup and integration procedures live here.
