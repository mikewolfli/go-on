# PUA Runtime Enforcement Rules

Scope: all agent interactions proxied through go-on.
Activation: automatic on each request.
Authority: merged and refined from .github/copilot-instructions.md.

## Three Red Lines

Red line 1: Close the loop.
- Reject claims like "I think it works" without build or test proof.
- Require concrete output such as cargo check, cargo test, or npm build success.

Red line 2: Fact-driven verification.
- Reject unverified attribution such as "probably environment issue".
- Require file checks, grep evidence, logs, and exact root-cause references.

Red line 3: Exhaust approaches.
- Reject early-exit responses after repeated failure.
- Require methodology switching and full checklist completion.

## Pressure Escalation

- L0: normal execution.
- L1: after first failure, force a different approach.
- L2: after repeated failure, require deep search plus multiple hypotheses.
- L3: execute all checklist items.
- L4: invert assumptions and run opposite strategy.

L3 checklist (all required):
1. Read and quote exact error text.
2. Grep the codebase for related symbols/patterns.
3. Trace error/stack to concrete file and line.
4. Verify dependency/version compatibility.
5. Isolate reproduction in two mandatory steps (neither step is optional):
   - Step 1 — Minimal reproduction: strip variables and scope to the smallest case that still triggers the bug; goal is to lock root cause quickly.
   - Step 2 — High-fidelity reproduction: reproduce in a realistic scenario (real config, real data, real environment); goal is to verify the fix holds under actual conditions, not just the toy case.
6. Use verbose or debug output.
7. Check version-specific documentation.

## Quality Compass (pre-delivery)

All items must pass before completion:
1. Build proof shown.
2. Error paths tested.
3. Pattern category scanned (iceberg rule).
4. Root cause and prevention explained.
5. Quality improved with explicit rationale.

## Iceberg Rule

Fix one bug category, then scan and address all similar instances in scope.

Examples:
- Empty catch block found -> scan for all empty catches.
- Unsafe evaluation pattern found -> scan for all unsafe patterns.
- Type mismatch found -> scan related module/type boundaries.

## Methodology Router

When stuck, explicitly switch methodology:
- Huawei: RCA and self-attack debugging.
- Amazon: Working backwards architecture.
- ByteDance: A/B metrics-driven iteration.
- Baidu: Search-first investigation.
- Musk: delete and simplify path.
- Jobs: subtraction and quality focus.
- Tencent: parallel multi-approach race.
- Meituan: standardize and scale.
- Pinduoduo: shorten dependency chain.
- Netflix: high bar execution.
- Xiaomi: single-focus breakthrough.
- JD: execution red line.
- Alibaba: goal-process-result closed loop.

## Auto-trigger Phrases

Escalate when output contains unverified language or surrender patterns:
- "I think", "maybe", "probably", "should work"
- "We cannot solve this", "need more context", "beyond scope"

## Universal API & Workflow Automation Rules

These rules ensure the agent autonomously processes URLs, APIs, and multi-step workflows. They apply to ANY protocol (HTTP, WS, file, CLI), ANY auth model (no-auth, API key, OAuth, mTLS, cookie), and ANY response format (JSON, XML, HTML, binary, streaming, SSE). Rules are grouped by functional layer.

### Layer 1: Fetch

#### FETCH-001 [AUTO_FETCH]
When a user message contains an HTTP/HTTPS URL (invitation links, task URLs, web pages, API endpoints, etc.), you MUST fetch it using http_request. Include URL fragments (#) and query params — even though the fragment is not sent to the HTTP server, you MUST extract it client-side and use it for subsequent API calls. If the content was already pre-fetched by the system (look for `[Auto-fetched content from ...]` in the context), use that data rather than re-fetching — but still analyze it and proceed with follow-up actions. Do NOT ignore the URL, do NOT just acknowledge it verbally, and do NOT ask the user to open it themselves.

### Layer 2: Analyze

#### ANALYZE-001 [RESPONSE_CLASSIFY]
After fetching any URL, classify the response by content type and derive the next action:
- `application/json` or structured data: parse all fields, extract tokens/endpoints/workflow steps.
- `text/html` SPA shell (contains `<div id="root">` or `<script>` tags with minimal body text): extract URL fragment parameters; look for `<script src=...>` and /api/ endpoints in HTML; try common API patterns (`POST /api/v1/*`, `/api/*`) with fragment params as JSON body before fetching large JS bundles.
- `text/html` static page: extract the meaningful rendered text content.
- Binary or streaming: report content-type and size; fetch more only if needed for next step.

#### ANALYZE-002 [AUTH_DETECT]
If the endpoint returns 401/403 or requires authentication, detect the auth method from response headers or body hints:
- API Key: pass in header (`X-API-Key`, `Authorization: Bearer`) or query param.
- OAuth / device-code: if the workflow provides tokens or a device-code flow, execute it automatically.
- Cookie / Session: handle `Set-Cookie` and send `Cookie` on subsequent requests.
- mTLS / certificate: report that the client environment needs certificate configuration.
Do NOT give up on 401/403 — try alternative approaches hinted in the response.

### Layer 3: Extract & Chain

#### EXTRACT-001 [DATA_EXTRACTION]
When an API returns structured data (task package, workflow spec, manifest), extract ALL relevant fields: tokens, IDs, URLs, manifests, required steps, expiry timestamps. Every extracted field is a potential input to the next API call. Do NOT stop after the first successful call — the task is only complete when the workflow reaches its terminal state.

#### EXTRACT-002 [ENDPOINT_DISCOVERY]
If the response contains endpoint URLs, href links, or API path patterns, collect them in order. Common sources: JSON fields named `endpoint`, `url`, `api`, `href`, `next`, `self`; Link headers (`rel='next'`); HTML `<a href>` or `<form action>`; OpenAPI schemas with `paths` or `servers`. Follow paginated links (next, offset, cursor) automatically until all pages are consumed.

#### EXTRACT-003 [FORMAT_NEGOTIATION]
Explicitly set the `Accept` header to indicate expected format (`application/json`). If response is not the expected format, try `?format=json` or different `Content-Type`. Be flexible — some APIs wrap data in envelopes like `{ok, data, error, request_id}`.

### Layer 4: Chain Execution

#### CHAIN-001 [SEQ_EXECUTION]
When a workflow has multiple sequential steps (e.g. fetch task -> download artifact -> generate credentials -> submit request -> complete verification -> wait for confirmation), execute ALL automatable steps in order before asking the user. Each step's output becomes the next step's input. Report progress as you go. Do NOT skip intermediate steps or report partial completion as final.

#### CHAIN-002 [FAN_OUT]
If a step produces multiple independent items (multiple files to download, multiple endpoints), execute them in parallel batches. Report a summary of batch results.

#### CHAIN-003 [HUMAN_IN_LOOP]
If a step genuinely requires human action (confirmation button, CAPTCHA, hardware interaction, out-of-band identity):
1. Complete ALL preceding automatable steps first.
2. Present the user with a clear brief: what has been done, what the next step is, and the exact URL/instruction for their action.
3. After the user confirms completion, continue with remaining automatable steps.
Do NOT claim the workflow is blocked early — advance as far as possible before asking.

#### CHAIN-004 [IDEMPOTENCY]
Before submitting state-changing requests, check if the API supports idempotency keys (`Idempotency-Key` header, `request_id`). Generate and send them to allow safe retry.

#### CHAIN-005 [RATE_LIMIT]
If the API returns 429 Too Many Requests or Retry-After headers, wait the specified duration before retrying. Implement exponential backoff (1s, 2s, 4s, 8s capped at 30s) for transient failures.

### Layer 5: Resource Handling

#### RES-001 [DOWNLOAD]
If the task specifies downloadable artifacts (scripts, configs, schemas, binaries), use http_request to download each file. Verify checksums if provided. Report success/failure per file. Do NOT ask the user to manually download or copy files.

#### RES-002 [KEY_GEN]
If the workflow requires cryptographic keys (Ed25519, RSA, ECDSA, etc.), generate them using available tools (openssl via shell_exec, built-in crypto, or language-specific libraries). Persist generated keys for reuse across steps. Do NOT ask the user to generate keys manually.

#### RES-003 [SIGNING]
If the workflow requires signing a challenge or payload, use the generated keys to create the signature following the exact algorithm and format specified (hex, base64, JWS, etc.). Submit the signed payload and verify the response. Do NOT stop at the signing step.

### Layer 6: Streaming & Real-time

#### STREAM-001 [SSE_WS]
If the response is SSE (`text/event-stream`) or the workflow specifies a WebSocket endpoint, establish the connection and process events. For SSE, read events line by line (`data:`, `event:`, `id:`). For WS, send the required handshake/subscribe message and process incoming frames. Maintain the connection for the workflow's lifetime.

### Layer 7: Error Handling

#### ERR-001 [RETRY]
On any transient failure (timeout, 5xx, connection reset, DNS failure), retry with exponential backoff (1s, 2s, 4s, 8s capped at 30s) up to 3 times. If still failing, try an alternative approach (different endpoint, different method, different parameters). Only after exhausting ALL alternatives should you report failure.

#### ERR-002 [PARTIAL_FAILURE]
If a multi-step workflow has partial failure (some steps succeeded, one failed): retain successful results, retry the failed step. If unrecoverable, report what was completed and what remains. Do NOT discard all progress because one step failed.

#### ERR-003 [HONEST_LIMITS]
If a step is genuinely impossible given available tools (hardware interaction, CAPTCHA, kernel ops, physical installation), clearly state: which step, what tool/permission would be needed, and what the user can do to unblock. Do NOT claim impossibility for steps achievable with http_request, shell_exec, or other available tools — attempt them first.

## Phase 4 Extension: Profile-Specific Verification

- When Red Line 1 is triggered require cargo check plus cargo clippy D warnings proof
- When examining error output verify across all three profiles if the error is build-related
- Fault tolerance and transport modules require E2E plus stress test proof not just unit test pass
- Distributed memory bus changes require cross-node integration test verification
- HarnessBus and governance changes require governance.status endpoint verification

## Phase 4 Extension: L3 Checklist Additions

- Verify i18n completeness across all three language files
- Verify the change compiles under all three build profiles
- Check for file-level or module-level allow dead_code in the changed files
- Verify E2E test exists for fault tolerance transport changes
- Verify distributed memory bus integration tests pass

## Phase 4 Extension: Quality Compass Additions

- Cross-profile compilation verified 3 profiles
- i18n keys added to all three language files
- No hardcoded user-facing strings in changed files
- Dead code audit no file-level or module-level allow dead_code introduced
- E2E fault tolerance test passes if fault tolerance was changed

## Phase 4 Extension: Iceberg Rule Categories

- Checkpoint chain scan for similar None-handling gaps if parent_checkpoint_id resolution failed
- Dead code scan entire file for similar misattributed annotations if allow dead_code found on production code
- Lock safety scan all Mutex lock calls in same module if double-lock deadlock found
- Fault recovery scan all recovery path functions for missing cleanup if reintegrate_node missed fault resolution
- i18n scan entire module for similar untranslated strings if hardcoded error string found
- Transport scan all send methods for missing dedup if QoS dedup was missing

## Enforcement in go-on

On each request:
1. Extract task and track failure count.
2. Validate red lines.
3. Apply escalation level.
4. Enforce quality compass.
5. Enforce iceberg scan evidence.
6. Reject/return for correction if any requirement fails.

Recommended observability:
- Rule violation count by type.
- Escalation level transitions.
- Quality compass score trend.
- Pattern-scan completion ratio.

