# BLUE25 — Four-Transport Platform Context Parity

## Execution Status (2026-04-19)

- Overall completion: **100%** (4 / 4 weighted items)
- Validation proof:
  - `cargo check --all-targets`: 0 errors, 0 warnings ✅
  - `cargo test --test acp_runtime_rpc_integration`: 75 passed, 0 failed ✅
  - `cargo test --test protocol_consistency_integration`: 17 passed, 0 failed ✅
  - `cargo test --test step2_three_endpoint_contract`: 10 passed, 0 failed ✅
  - `cargo test --test transport_parity_integration`: 10 passed, 0 failed ✅ (10 new blue25 tests)

---

## Objectives

Audit all four transport paths (ACP stdio, ACP HTTP, MCP stdio, MCP HTTP) for `platform_context` injection consistency, close the gap found in ACP HTTP, and lock in the parity guarantee with integration tests.

---

## Gap Discovery

A systematic audit of the four transport paths revealed the pre-fix baseline at BLUE25 start:

| Transport | `inject_platform_profiles_if_absent` called | Before BLUE25 |
|---|---|---|
| ACP stdio | ✅ via `send_result()` in `request.rs` | correct |
| ACP HTTP | ❌ zero inject calls across all HTTP handlers | **broken at audit start; fixed in this delivery** |
| MCP stdio | ✅ `mcp/handlers.rs:65` | correct |
| MCP HTTP | ✅ routes through same `McpServer::handle_request` | correct |

ACP HTTP was the only broken path. Every HTTP response point bypassed `inject_platform_profiles_if_absent`, meaning all HTTP responses silently omitted `platform_context`.

---

## Delivered Changes

### Item 1 — ACP HTTP inject calls (`src/acp/impl/runtime.rs`)

Nine injection points added, covering every response code path:

| Handler | Code path | Injected method tag |
|---|---|---|
| `handle_openai_chat_completions` | degraded fallback (200) | `"openai.chat.completions"` |
| `handle_openai_chat_completions` | success (200) | `"openai.chat.completions"` |
| `handle_responses_api` | degraded fallback (200) | `"responses.api"` |
| `handle_responses_api` | success (200) | `"responses.api"` |
| `handle_responses_api_stream` | success completed event | `"responses.api"` |
| `handle_responses_api_stream` | degraded completed event | `"responses.api"` |
| `/chat` HTTP route | success (200) | `"chat"` |
| `/chat/stream` HTTP route | SSE result event | `"chat"` |
| Import added | `use inject_platform_profiles_if_absent` | — |

### Item 2 — Error-path parity closure (ACP HTTP + MCP stdio + MCP HTTP)

To eliminate residual inconsistency risk, error-chain responses were also unified:

- ACP HTTP OpenAI compat errors now inject `platform_context`:
  - invalid request `400` in `handle_openai_chat_completions`
  - upstream failure `502` in `handle_openai_chat_completions`
- ACP stdio errors now inject `platform_context` in `error.data`:
  - unknown method (`-32601`)
  - invalid params such as `skill.remove` missing `name` (`-32602`)
- ACP HTTP Responses API validation errors now use unified helper:
  - `write_responses_api_error(...)` (internally injects `platform_context` then writes `400`)
  - Covers all validation/tool/not-found branches in `handle_responses_api`
- MCP stdio errors now carry `platform_context` in `error.data`:
  - unknown method (`-32601`)
  - handler failures (`-32602` / `-32603`)
- MCP HTTP parse error now carries `platform_context` in `error.data`.

Second-round closure in the same BLUE25 stream:

- ACP HTTP baseline branches now also inject `platform_context`:
  - `GET /health`
  - `GET /v1/responses` list
  - `GET /v1/models` / `/v1/model` / `/models`
  - `GET /` root capabilities
  - generic `404` / `405` / empty-body `400` / invalid-json `400` branches
- MCP HTTP baseline branches now also inject `platform_context`:
  - `GET /health`
  - non-POST `405` response

### Item 3 — `is_infrastructure` extension (`src/acp/impl/request.rs`)

The `is_infrastructure` match arm now includes ACP/MCP compat and error method tags:

```rust
| "openai.chat.completions" | "responses.api"
| "mcp.parse_error" | "mcp.unknown_method"
```

This ensures compatibility and error responses stay on the lightweight `profile_class = "infrastructure"` path instead of semantic-class expansion.

### Item 4 — Transport parity integration tests (`tests/transport_parity_integration.rs`)

New test suite with 10 tests (all passing):

| Test | What it verifies |
|---|---|
| `acp_http_chat_response_has_platform_context` | `/chat` returns `platform_context.profile_class = "infrastructure"` + correct schema_version |
| `acp_http_openai_completions_response_has_platform_context` | `/v1/chat/completions` returns `platform_context.profile_class = "infrastructure"` + correct schema_version |
| `acp_http_responses_api_response_has_platform_context` | `/v1/responses` returns `platform_context.profile_class = "infrastructure"` + correct schema_version |
| `acp_stdio_and_acp_http_share_same_schema_version` | ACP stdio and ACP HTTP report identical `schema_version`, proving single shared injection source |
| `acp_http_error_payloads_keep_platform_context` | ACP HTTP OpenAI/Responses 4xx payloads still include `platform_context` |
| `mcp_http_error_data_keeps_platform_context` | MCP HTTP unknown-method + parse-error JSON-RPC responses include `platform_context` in `error.data` |
| `acp_http_health_response_has_platform_context` | ACP HTTP `/health` baseline response includes `platform_context` |
| `mcp_http_health_response_has_platform_context` | MCP HTTP `/health` baseline response includes `platform_context` |
| `acp_http_method_not_allowed_has_platform_context` | ACP HTTP `405 method not allowed` payload includes `platform_context` |
| `mcp_http_method_not_allowed_has_platform_context` | MCP HTTP `405 method not allowed` payload includes `platform_context` |

Tests use the `local_echo` agent type for reliable execution without real AI backend, following the pattern established in `openai_compat_matrix_integration.rs`.

---

## Capability Uplift Summary

| Transport | Before BLUE25 | After BLUE25 |
|---|---|---|
| ACP stdio | platform_context present ✅ | unchanged ✅ |
| ACP HTTP `/chat` | **missing** ❌ | platform_context present ✅ |
| ACP HTTP `/v1/chat/completions` | **missing** ❌ | platform_context present ✅ |
| ACP HTTP `/v1/responses` | **missing** ❌ | platform_context present ✅ |
| MCP stdio | platform_context present ✅ | unchanged ✅ |
| MCP HTTP | platform_context present ✅ | unchanged ✅ |

`platform_context.schema_version` is now `"blue24-platform-universal-v1"` on success and error surfaces of ACP stdio / ACP HTTP / MCP stdio / MCP HTTP, drawn from a single shared injection source.

---

## Completion by Item

- Item 1 (9 inject calls in runtime.rs): **Completed (100%)**
- Item 2 (error-path parity closure): **Completed (100%)**
- Item 3 (is_infrastructure extension): **Completed (100%)**
- Item 4 (transport parity test suite, 10 new passing tests): **Completed (100%)**

---

## Next Focus

Carried from BLUE24 next focus + BLUE25 forward work:

- **Streaming token economy feedback**: real-time compression ratio reporting in SSE stream events — AI can observe its own token efficiency per round
- **Meta-cognition loop persistence across checkpoints**: `metacognitive_loop.cycle_count` and `last_reflection` survive conversation save/restore cycles
- **Cross-agent knowledge distillation**: multi-agent sessions merge their `learning_profile` and `knowledge_refinement` blocks at session end, building a shared epistemic base
- **Transport parity automation**: gate that ensures any new HTTP endpoint added in future is automatically verified for `platform_context` presence (static analysis hook or CI assertion)
