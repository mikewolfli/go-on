# go-on Documentation

This book documents the current `1.4.3` architecture and usage model of `go-on`.

The runtime implements a **14-bus capability architecture** with **38 AI provider integrations**,
**2069 unit tests** (zero failures), and **zero clippy warnings across all build profiles**.

All 122+ JSON-RPC handlers return a unified `DispatchOutput` enum, with the dispatch layer
handling serialization for JSON-RPC, SSE streaming, text/plain, and silent responses.

It is organized as a trilingual mdBook:

- English chapters start at [Architecture Overview](en/overview.md).
- 简体中文章节从 [架构总览](zh-CN/overview.md) 开始。
- 繁體中文章節從 [架構總覽](zh-TW/overview.md) 開始。

The content is based on the current workspace structure and runtime surfaces:

- Rust backend runtime and CLI (three build profiles: local / simple-server / multi-users-server)
- 14-bus capability architecture (CapabilityBus + HarnessBus + 12 sub-buses)
- Unified `DispatchOutput` handler dispatch pattern (Json / Error / Stream / Text / Checkpoint / Silent)
- Autonomous agent orchestration, DAG task execution, and cognitive modules
- Setup wizard and secret management (system keyring, Vault)
- Zed integration through ACP stdio and ACP or MCP HTTP
- VS Code addon runtime wiring
- EGUI GUI configuration and operations
- Node.js SDK — TypeScript async client (30+ methods)
- Governance, security (mTLS, request signing, content safety, prompt injection detection)
- Observability (Prometheus `/metrics`, OTel tracing, governance status endpoint)
- Full i18n coverage (~95%) across backend, GUI, and VS Code addon

Use this book as the operational reference. Root `README` files stay concise; the detailed setup and integration procedures live here.
