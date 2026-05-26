# Changelog

## [1.1.0] - 2026-05-26

### Changed
- Project version updated to 1.1.0 across all modules
- Zero dead-code suppression — all `#![allow(dead_code)]` removed or replaced with feature-gated cfg_attr
- Zero compiler warnings — cargo check (bin + tests) + clippy -D warnings all clean across 3 profiles
- Documentation reorganized and completed for all 8 advanced orchestration modules
- ACP helpers/ reorganized into 7 domain subdirectories with #[path] backward compatibility

### Fixed
- unreachable!() production panic risk in prelude.rs replaced with graceful warn!() + fallback
- SessionCompressor now wired into SessionContextManager for semantic compression
- SseBufferPool now used for zero-allocation SSE event serialization
- CacheWarmingEngine initialized from main.rs, warmed after server completion
- planner_embedding classifier integrated into Planner::plan() main path
- RBAC tenant isolation tests fixed for i18n key compatibility

### Added
- 43 smoke tests for 6 ACP helper modules + 3 orchestration modules
- Full F-GAP label coverage for all 90+ #[allow(dead_code)] annotations
- profile-local now includes sub-bus-memory and sub-bus-protocol for 14-bus completeness
## [1.0.0] - 2026-05-25

### Added
- ACP/MCP dual-protocol support with 5 transport modes
- 35+ AI provider integrations with native function calling
- 14-Bus architecture with 21 F-GAP cognitive modules
- Full-auto skill discovery and task execution
- Native function calling for OpenAI, Anthropic, DeepSeek
- Multi-model concurrent voting (Majority/Weighted/Unanimous/BestOfN)
- Transaction system with WAL persistence and 2PC
- Session context management with key concept extraction
- Cache warming with adaptive TTL and multi-tier management
- Chaos testing framework with 10 fault types
- Hot-reload configuration system
- Config schema versioning and migration
- Plugin system with Plugin trait and PluginRegistry
- Skill marketplace with install/uninstall/search
- SSE streaming optimizer with adaptive batching

### Changed
- Refactored mode runtimes to eliminate 5x code duplication
- Migrated from global OnceLock singletons to OrchestrationContext
- Upgraded recovery strategy matching from Levenshtein to explicit enums
- Optimized scheduler dequeue from O(n log n) to O(log n)
- Integrated BrainLoop into full-auto execution flow
- Fixed Gemini function call streaming parser
- Cleaned up deprecated model entries across all providers
- Enhanced Groq provider with tool_choice defaults and tests

### Fixed
- BrainLoop off-by-one iteration limit bug
- DAG executor was parallel fan-out (now real topological ordering)
- Dead code modules hot_reload/schema_version (now integrated)
