# go-on Phase 0/1 Architecture and Implementation Notes

## Runtime Architecture (Phase 0/1)
- All request, phase, and review gate lifecycles must be documented
- cache/vector/summary/breaker invariants must be specified
- All agent/phase/tool entrypoints support envelope/schema/audit log
- Agent task envelope (AgentTaskEnvelope), output (AgentTaskResult), and decision log (AgentAuditLog) structures: see src/agent.rs
- Tool runtime trait and registry: see src/tool.rs
- Mode/phase/provider capability compatibility matrix: see below

## Capability Compatibility Matrix (Example)
| Mode      | Tool Use | Multi-Agent | Review Gate | Memory | Resume | Trace | Evaluation |
|-----------|----------|-------------|-------------|--------|--------|-------|------------|
| ask       | ×        | ×           | ×           | ✓      | ×      | ×     | ×          |
| edit      | △        | ×           | ×           | ✓      | ×      | ×     | ×          |
| agent     | ✓        | ×           | △           | ✓      | △      | △     | ×          |
| full_auto | ✓        | ✓           | ✓           | ✓      | ✓      | ✓     | ✓          |

- ✓: Fully supported, △: Partially supported, ×: Not supported

## Phase 0/1 Key Interfaces/Structures
- AgentTaskEnvelope/AgentTaskResult/AgentAuditLog (src/agent.rs)
- Tool trait/ToolRegistry/ToolInput/ToolOutput (src/tool.rs)
- All agent/tool/phase routing should generate decision audit logs

---

# Task: Generate a Rust ACP Agent Program Supporting Flow Definitions, Multi-Phase, Multi-Agent, and Phase Principles

Please generate a complete Rust project based on the following requirements. The project should implement the ACP (Agent Client Protocol) for the Zed editor, and define the development flow, phases, agents, and phase-specific coding principles via a TOML configuration file. At runtime, the proxy should determine the current phase from the request, locate the corresponding phase config from the flow, route the request to one of the agents bound to that phase, and automatically inject that phase's principles into the system prompt.

## 1. Project Overview

Project name: `go-on`

Goal:
- Communicate with Zed using JSON-RPC 2.0 over stdin/stdout, with full ACP protocol support.
- Use a TOML config file (located in the same directory as the executable) to define the full development flow, phases, available agents for each phase, and phase-specific coding principles.
- Based on the `phase` field in Zed requests (or a default phase), choose the current phase, then try agents bound to that phase in order until one is available and can handle the request. Inject phase principles (if present) into the system prompt when calling the model.
- Allow users to add new phases and new agent types without modifying core code.

## 2. Configuration File Design (`config.toml`)

The config file structure is as follows:

```toml
# Default phase (used when request does not provide phase)
default_phase = "coding"

# Define all agents (models)
[agents.copilot]
type = "copilot"
url = "http://127.0.0.1:8080"   # Local Copilot service URL

[agents.deepseek]
type = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"   # Read from environment variable
model = "deepseek-chat"

[agents.wenxin]
type = "wenxin"
api_key_env = "WENXIN_API_KEY"
secret_key_env = "WENXIN_SECRET_KEY"

# Define flow
[flow]
name = "Software Development Flow"
# Ordered phase list (for display or future auto-advance)
phases = ["coding", "review"]

# Define each phase
[phases.coding]
description = "Coding phase"
agents = ["copilot", "deepseek"]   # Try in order; first available is primary
fallback = true                        # Allow fallback to next agent
# Coding principles for this phase (to be injected into system prompt)
principles = [
    "Use meaningful variable names; avoid abbreviations",
    "Each function should be no more than 50 lines and follow single responsibility",
    "Prefer the standard library; avoid reinventing the wheel",
    "All public functions must include doc comments"
]

[phases.review]
description = "Review phase"
agents = ["wenxin"]
# Extra options passed to this phase's agent (e.g., review strictness)
[phases.review.options]
stage = "strict"   # Passed to Wenxin as stage parameter
# Principles for review phase (stricter)
principles = [
    "Must check null pointers and boundary conditions",
    "All errors must have explicit handling",
    "No unused variables or functions allowed",
    "Code must pass all unit tests"
]

# Optional: users can add more phases, for example:
# [phases.testing]
# description = "Testing phase"
# agents = ["some_test_agent"]
# principles = ["Test coverage must be at least 80%", ...]
```

Notes:
- The `flow` section defines the flow name and phase order (order is mainly informational for now; the proxy primarily locates phase config directly using `phase` from the request).
- Each phase under `phases` must contain `description`, an `agents` list, and optional `principles` (array of strings).
- Principles from `principles` must be merged into the system prompt when calling the model.
- The `fallback` field indicates whether to try the next agent if the first one is unavailable.
- Optional `options` per phase can be passed into agent implementations (for example, Wenxin `stage` parameter).

## 3. ACP Protocol Requirements

Zed chat requests should include a `phase` field (for example, in `params`). If missing, use `default_phase` from config.

Example request (JSON-RPC):

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "chat",
  "params": {
    "messages": [...],
    "phase": "coding",
    "context": {...}
  }
}
```

The proxy must extract `phase` from `params`, then:

1. Validate whether the phase exists according to `flow.phases`.
2. Get the bound agent list and phase principles (if present).
3. Try each agent in order until one successfully returns a streaming response.
4. Inject phase principles as part of the system prompt when calling the selected agent.

## 4. Agent Implementation Requirements

### System Prompt Construction Rules

All agents must include current phase principles (`principles`) as part of the system prompt when invoking the model. Rules:

- If the model supports system prompts (e.g., DeepSeek, Wenxin), prepend principles into a system message, then append model default system prompt (if any).
- For Copilot (local service), convert principles to a prefixed user instruction because Copilot API does not support system messages.

Example (DeepSeek):

```json
{
  "model": "deepseek-chat",
  "messages": [
    {"role": "system", "content": "Please follow these programming principles:\n- Use meaningful variable names...\n- Keep each function under 50 lines..."},
    {"role": "user", "content": "User message..."}
  ],
  "stream": true
}
```

### Supported Agent Types

1. `copilot`
   - Type identifier: `copilot`
   - Config fields: `url` (local Copilot service URL)
   - Implementation: transform user messages into OpenAI-style request, send to `{url}/v1/chat/completions`, support streaming response. Return error on connection failure.
   - Principle handling: since Copilot does not support `system` role, transform principles into a prefixed user instruction (e.g., comment block or standalone instruction message).

2. `deepseek`
   - Type identifier: `deepseek`
   - Config fields: `api_key_env` (env var name), `model` (default `deepseek-chat`)
   - Implementation: call DeepSeek API `https://api.deepseek.com/v1/chat/completions` with streaming support. API key loaded from environment variable.
   - Principle handling: append principles into system message.

3. `wenxin`
   - Type identifier: `wenxin`
   - Config fields: `api_key_env`, `secret_key_env`
   - Implementation: call Baidu Wenxin API. Must first get `access_token`, then call chat endpoint. Support streaming response.
   - Principle handling: append principles into system prompt. If phase `options` contains `stage` (`early` or `strict`), dynamically adjust review strictness:
     - `early`: append a relaxed review instruction after principles:
       "The project is still in an early stage and architecture is not finalized. Only check core logic validity; empty functions and implementation TODOs are acceptable."
     - `strict`: append a strict review instruction:
       "The project is in a mature stage. Enforce strict completeness checks: no empty functions, no unhandled errors, no missing boundary checks, etc."
     - Default: `strict`.

### Unified Agent Interface

All agents should implement a unified `Agent` trait:

```rust
#[async_trait]
pub trait Agent: Send + Sync {
    async fn chat(
        &self,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,   // Principle list for current phase
        options: Option<HashMap<String, Value>>,   // Extra phase options
        sender: mpsc::UnboundedSender<String>,     // Streaming token output channel
    ) -> Result<()>;
}
```

`Message` should include at least `role` and `content` fields.

## 5. Project Structure and Code Organization

```text
.
├── Cargo.toml
├── config.toml.autopilot-adaptive
├── README.md
└── src
    ├── main.rs               # Entry: load config and run ACP main loop
    ├── acp.rs                # ACP protocol parsing and response generation
    ├── config.rs             # Config loading and data structures
    ├── error.rs              # Custom error types
    ├── agent.rs              # Agent trait and registry
    ├── agents
    �?  ├── mod.rs
    �?  ├── copilot.rs
    �?  ├── deepseek.rs
    �?  └── wenxin.rs
    └── flow.rs               # Flow management: phase lookup and agent routing
```

- `config.rs`: define config structs (including Agents, Phases, Flow, and phase Principles), and implement TOML loading.
- `agent.rs`: define `Agent` trait and provide a global `AgentRegistry` to instantiate agents by type.
- `agents/*.rs`: implement `Agent` trait; each implementation must accept `principles` and inject them correctly into outgoing model requests.
- `flow.rs`: based on request `phase` and config, return an available agent (tried in order), plus phase principles and options.
- `acp.rs`: handle JSON-RPC requests; for `chat` method, use flow to pick agent, then pass principles and options to `agent.chat`.

## 6. CLI Arguments

Use `clap` to support:

- `--config <PATH>`: config file path (default: `config.toml` next to executable)
- `--phase <PHASE>`: force phase override (for testing)
- `--verbose`: enable verbose logging

## 7. Performance and Robustness Requirements

- Use `tokio` async runtime; all I/O must be non-blocking.
- Streaming response: send each token to stdout immediately and flush.
- Retry API calls (max 2 retries) with exponential backoff.
- Convert all internal errors into JSON-RPC error responses for Zed; do not crash.
- Graceful shutdown: upon `shutdown` request, wait for in-flight request completion before exit.
- Catch and log all panics.

## 8. Dependency Versions (`Cargo.toml`)

```toml
[package]
name = "go-on"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.11", features = ["json", "stream"] }
anyhow = "1"
thiserror = "1"
clap = { version = "4", features = ["derive", "env"] }
futures-util = "0.3"
async-trait = "0.1"
toml = "0.8"
env = "0.10"
log = "0.4"
env_logger = "0.11"
```

## 9. Output Requirements

Generate all file contents and list them by path (for example, `Cargo.toml`, `src/main.rs`, `src/config.rs`, etc.).

Finally, provide a simple `README.md` explaining:
- how to build,
- how to configure environment variables,
- and how to configure this proxy in Zed `settings.json` as an `agent_servers` entry.

The configuration file should include example principles, and users should be able to customize them per project.

Code should include detailed comments for key logic, especially:
- how principles are injected,
- and how fallback among multiple agents is handled.

