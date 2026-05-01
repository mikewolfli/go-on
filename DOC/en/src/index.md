# go-on Documentation

This book documents the current `0.8.4` architecture and usage model of `go-on`.

The runtime has completed **Phase 4** (FutureDesign) — 100% of 21 F-GAP modules implemented,
all 38 capability dimensions at ★★★★★, and 14-bus architecture fully operational.

Start at [Architecture Overview](overview.md).

The content is based on the current workspace structure and runtime surfaces:

- Rust backend runtime and CLI (three build profiles: local / simple-server / multi-users-server)
- 14-bus capability architecture (CapabilityBus + HarnessBus + 12 sub-buses)
- 21 F-GAP modules spanning orchestration, governance, resilience, fault tolerance, protocol
- Setup wizard and secret management
- Zed integration through ACP stdio and ACP or MCP HTTP
- VS Code addon runtime wiring
- Tauri GUI configuration and operations
- Full i18n coverage (~95%) across backend, GUI, and VS Code addon

Use this book as the operational reference. Root `README` files stay concise; the detailed setup and integration procedures live here.
