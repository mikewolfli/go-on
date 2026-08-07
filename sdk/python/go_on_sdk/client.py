"""Async JSON-RPC client for go-on."""

from __future__ import annotations

import asyncio
import json
import logging
import random
import time
import uuid
from collections.abc import AsyncGenerator
from dataclasses import dataclass
from typing import Any, cast

import httpx

logger = logging.getLogger(__name__)


class GoOnClientError(Exception):
    """Custom exception for go-on SDK client errors."""


class GoOnRateLimitedError(GoOnClientError):
    """Raised when the server responds with HTTP 429 (Too Many Requests)."""

    retry_after: float | None

    def __init__(self, retry_after: float | None = None) -> None:
        self.retry_after = retry_after
        super().__init__(
            f"Rate limited by server (retry_after={retry_after})"
        )


class GoOnJsonRpcError(GoOnClientError):
    """JSON-RPC protocol-level error."""

    code: int
    message: str

    def __init__(self, code: int, message: str) -> None:
        self.code = code
        self.message = message
        super().__init__(f"JSON-RPC error [{code}]: {message}")


# ── Chat types (streaming support) ──────────────────────────────────


@dataclass
class ChatMessage:
    role: str
    content: str


@dataclass
class ChatRequest:
    messages: list[ChatMessage]
    model: str | None = None
    temperature: float | None = None
    max_tokens: int | None = None
    stream: bool | None = None


# ── Response types ──────────────────────────────────────────────────


@dataclass
class HealthResponse:
    lifecycle: dict[str, Any] | None = None
    version: str | None = None
    stats: dict[str, Any] | None = None
    maintenance: dict[str, Any] | None = None
    timestamp: int | None = None
    metrics: dict[str, Any] | None = None
    status: str | None = None
    uptime_seconds: int | None = None
    modules: dict[str, Any] | None = None


@dataclass
class GovernanceStatusResponse:
    ok: bool
    governance: dict[str, Any]


@dataclass
class HealthProbesResponse:
    modules: dict[str, Any]


@dataclass
class MetricsResponse:
    metrics: dict[str, Any]


@dataclass
class BreakerStatusResponse:
    breakers: dict[str, Any]


@dataclass
class CheckpointListResponse:
    checkpoints: list[dict[str, Any]]


@dataclass
class TaskPlanResponse:
    plan: dict[str, Any]


@dataclass
class LearningSummaryResponse:
    summary: dict[str, Any]


@dataclass
class SelectorStatusResponse:
    selector: dict[str, Any]


@dataclass
class CostStatusResponse:
    cost: dict[str, Any]


@dataclass
class ConfigBaselineResponse:
    baseline: dict[str, Any]


@dataclass
class HarnessStatusResponse:
    harness: dict[str, Any]


# ── BLUE68 P5-10: Missing key types ────────────────────────────────────────


@dataclass
class ToolCall:
    """Record of a tool call made by an agent."""

    tool_name: str
    arguments: dict[str, Any]
    agent_name: str
    result: dict[str, Any] | None = None
    duration_ms: int = 0


@dataclass
class MultimodalInput:
    """Multimodal input for rich chat requests."""

    type: str  # "text" | "image" | "document" | "audio"
    text: str | None = None
    image_url: str | None = None
    detail: str | None = None  # "auto" | "low" | "high"
    data: str | None = None
    mime_type: str | None = None
    filename: str | None = None
    format: str | None = None


@dataclass
class StreamChunk:
    """A single chunk in an SSE streaming response."""

    token: str
    done: bool = False
    reasoning: str | None = None
    tool_calls: list[ToolCall] | None = None
    index: int = 0
    total_chars: int = 0


@dataclass
class AgentInfo:
    """Metadata about an available agent."""

    name: str
    agent_type: str
    description: str
    models: list[str] | None = None
    capabilities: list[str] | None = None
    healthy: bool = True


@dataclass
class ToolInfo:
    """Descriptor for a tool exposed via the tools/list endpoint."""

    name: str
    description: str
    input_schema: dict[str, Any]


@dataclass
class ToolCallRequest:
    """Request payload for executing a tool via tools/call."""

    tool_name: str
    arguments: dict[str, Any]
    session_id: str | None = None


@dataclass
class ToolCallResult:
    """Result of a tool execution via tools/call."""

    success: bool
    output: str | None = None
    error: str | None = None
    duration_ms: int = 0


# ── ACP Session Protocol types ────────────────────────────────────────────


@dataclass
class PromptContentBlock:
    """A content block in a session prompt (text, resource, image, audio, etc.)."""

    type: str  # "text" | "resource" | "resource_link" | "image" | "audio"
    text: str | None = None
    uri: str | None = None
    name: str | None = None
    resource: dict[str, Any] | None = None


@dataclass
class SessionInfo:
    """Summary of an active ACP session as returned by session/list.

    The backend emits a minimal shape: ``[{"id": sid}]``.
    """

    id: str


def _prompt_block_to_dict(block: PromptContentBlock | dict[str, Any]) -> dict[str, Any]:
    """Serialize a prompt content block, omitting unset optional fields."""
    if isinstance(block, dict):
        return block
    out: dict[str, Any] = {"type": block.type}
    if block.text is not None:
        out["text"] = block.text
    if block.uri is not None:
        out["uri"] = block.uri
    if block.name is not None:
        out["name"] = block.name
    if block.resource is not None:
        out["resource"] = block.resource
    return out


# ── Client ──────────────────────────────────────────────────────────


class GoOnClient:
    """Async client for go-on ACP JSON-RPC endpoints.

    Targets ``POST {base_url}/rpc`` for JSON-RPC calls
    and ``/chat/stream`` for SSE streaming chat.

    Phase 4 coverage: runtime, governance, observability, reliability,
    checkpoint, workflow, learning, optimization, streaming chat.

    Parameters
    ----------
    base_url:
        The base URL of the go-on server (e.g. ``http://127.0.0.1:8090``).
    timeout:
        Timeout in seconds for HTTP requests (default: 30.0).
    max_retries:
        Number of retries for transient HTTP failures (default: 3).
    retry_delay:
        Base delay in seconds between retries (default: 1.0).
        Uses exponential backoff with jitter for faster recovery.
        Actual delays: retry_delay * 1x, 2x, 4x + random 0-100ms jitter.
    use_exponential_backoff:
        Enable exponential backoff with jitter for retries (default: True).
        When True, retry delays grow exponentially, improving throughput
        during transient failures by avoiding thundering herd.
    """

    def __init__(
        self,
        base_url: str,
        timeout: float = 30.0,
        max_retries: int = 3,
        retry_delay: float = 1.0,
        use_exponential_backoff: bool = True,
    ) -> None:
        self.base_url: str = base_url.rstrip("/")
        self.timeout: float = timeout
        self.max_retries: int = max_retries
        self.retry_delay: float = retry_delay
        self._use_exponential_backoff: bool = use_exponential_backoff
        self._client: httpx.AsyncClient = httpx.AsyncClient(
            timeout=httpx.Timeout(timeout),
            limits=httpx.Limits(
                max_keepalive_connections=20,
                max_connections=100,
                keepalive_expiry=30.0,
            ),
        )

    def _retry_delay_for_attempt(self, attempt: int) -> float:
        """Compute retry delay with the unified backoff contract (seconds).

        contracts/cross-client-sync.md formula:
        delay = min(base * 2^attempt, 30s) * (0.7 + random() * 0.3)
        The ±30% jitter keeps delays above 70% of the base, matching the
        GUI, VS Code, and other SDK implementations exactly.
        - attempt 0: ~0.7-1.0s
        - attempt 1: ~1.4-2.0s
        - attempt 2: ~2.8-4.0s
        - attempt 5+: ~21-30s (capped)
        """
        if not self._use_exponential_backoff:
            return self.retry_delay
        capped = min(30.0, self.retry_delay * (2.0**attempt))
        return capped * (0.7 + random.random() * 0.3)

    async def aclose(self) -> None:
        await self._client.aclose()

    async def __aenter__(self) -> GoOnClient:
        return self

    async def __aexit__(
        self,
        _exc_type: type[BaseException] | None,
        _exc_val: BaseException | None,
        _exc_tb: object | None,
    ) -> None:
        await self.aclose()

    # ── Internal helpers ──────────────────────────────────────────────

    async def _json_rpc(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        """Perform a JSON-RPC call and return the result dict."""
        payload: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": str(uuid.uuid4()),
            "method": method,
            "params": params or {},
        }

        last_error: Exception | None = None

        for attempt in range(self.max_retries + 1):
            try:
                resp = await self._client.post(f"{self.base_url}/rpc", json=payload)
                _ = resp.raise_for_status()
                try:
                    data = cast(dict[str, Any], resp.json())
                except json.JSONDecodeError:
                    raise GoOnClientError(
                        f"Server returned non-JSON response: {resp.text[:500]}"
                    ) from None

                if "error" in data:
                    err = cast(dict[str, Any], data["error"])
                    raise GoOnJsonRpcError(
                        code=cast(int, err.get("code", -1)),
                        message=cast(str, err.get("message", "unknown")),
                    )
                return cast(dict[str, Any], data.get("result", {}))
            except (
                httpx.TimeoutException,
                httpx.PoolTimeout,
                httpx.NetworkError,
                httpx.RemoteProtocolError,
                httpx.ConnectError,
                httpx.ReadError,
            ) as e:
                last_error = e
                if attempt < self.max_retries:
                    await asyncio.sleep(self._retry_delay_for_attempt(attempt))
            except httpx.HTTPStatusError as e:
                status = e.response.status_code
                if status in (429, 502, 503):
                    last_error = e
                    if attempt < self.max_retries:
                        await asyncio.sleep(self._retry_delay_for_attempt(attempt))
                else:
                    raise
            except (
                GoOnClientError,
                json.JSONDecodeError,
                KeyboardInterrupt,
                SystemExit,
            ):
                # Non-transient errors — do not retry
                raise
            # No bare except Exception — only known retryable network errors are retried

        if isinstance(last_error, httpx.HTTPStatusError):
            http_err: httpx.HTTPStatusError = last_error
            if http_err.response.status_code == 429:
                raise GoOnRateLimitedError() from http_err
        raise GoOnClientError(
            f"Request failed after {self.max_retries} retries: {last_error}"
        ) from last_error

    # ── Streaming chat ────────────────────────────────────────────────

    async def chat_stream(self, request: ChatRequest) -> AsyncGenerator[dict[str, Any], None]:
        """Send a chat request and yield SSE events as they arrive.

        Each yielded value is a parsed JSON object from a ``data:`` line
        in the SSE stream.

        Yields
        ------
        dict
            A JSON chunk from the stream.
        """
        request_dict: dict[str, Any] = {
            "messages": [{"role": m.role, "content": m.content} for m in request.messages],
        }
        if request.model is not None:
            request_dict["model"] = request.model
        if request.temperature is not None:
            request_dict["temperature"] = request.temperature
        if request.max_tokens is not None:
            request_dict["max_tokens"] = request.max_tokens
        if request.stream is not None:
            request_dict["stream"] = request.stream

        async with self._client.stream(
            "POST",
            f"{self.base_url}/chat/stream",
            json=request_dict,
        ) as response:
            async for line in response.aiter_lines():
                # Handle both "data: " and "data:" (without space) per SSE spec tolerance
                if line.startswith("data:"):
                    payload = line[5:].lstrip() if len(line) > 5 else ""
                    # Handle the [DONE] SSE terminator — break instead of trying to parse it as JSON
                    if payload.strip() == "[DONE]":
                        break
                    try:
                        yield json.loads(payload)
                    except json.JSONDecodeError:
                        if payload.strip() == "[DONE]":
                            break
                        logger.warning(f"SSE parse error on payload: {payload[:100]}")
                        continue

    # ── Core Runtime ──────────────────────────────────────────────────

    async def health(self) -> HealthResponse:
        """GET /health — quick health check (ServerStatus payload)."""
        resp = await self._client.get(f"{self.base_url}/health")
        _ = resp.raise_for_status()
        data = cast(dict[str, Any], resp.json())
        return HealthResponse(
            lifecycle=cast(Any, data.get("lifecycle")),
            version=cast(Any, data.get("version")),
            stats=cast(Any, data.get("stats")),
            maintenance=cast(Any, data.get("maintenance")),
            timestamp=cast(Any, data.get("timestamp")),
            metrics=cast(Any, data.get("metrics")),
            modules=cast(Any, data.get("modules")),
        )

    async def runtime_health(self) -> HealthResponse:
        """runtime.health — full runtime health via JSON-RPC."""
        result = await self._json_rpc("runtime.health")
        return HealthResponse(
            lifecycle=cast(Any, result.get("lifecycle")),
            version=cast(Any, result.get("version")),
            stats=cast(Any, result.get("stats")),
            maintenance=cast(Any, result.get("maintenance")),
            timestamp=cast(Any, result.get("timestamp")),
            modules=cast(Any, result.get("modules")),
        )

    async def runtime_stability(self) -> dict[str, Any]:
        """runtime.stability — runtime stability snapshot."""
        return await self._json_rpc("runtime.stability")

    async def initialize(self, setup_level: str = "standard") -> dict[str, Any]:
        """initialize — initialize the runtime."""
        return await self._json_rpc("initialize", {"setup_level": setup_level})

    async def shutdown(self) -> dict[str, Any]:
        """shutdown — gracefully shut down the runtime."""
        return await self._json_rpc("shutdown")

    # ── Governance ────────────────────────────────────────────────────

    async def governance_status(self) -> GovernanceStatusResponse:
        """governance.status — full governance status (~120+ profiles)."""
        result = await self._json_rpc("governance.status")
        return GovernanceStatusResponse(
            ok=bool(cast(object, result.get("ok", False))),
            governance=cast(dict[str, Any], result.get("governance", {})),
        )

    async def governance_plan_get(self) -> dict[str, Any]:
        """governance.plan.get — get active governance plan."""
        return await self._json_rpc("governance.plan.get")

    async def governance_audit_recent(self, limit: int = 20) -> dict[str, Any]:
        """governance.audit.recent — view recent audit entries."""
        return await self._json_rpc("governance.audit.recent", {"limit": limit})

    async def governance_audit_verify(
        self,
        from_ms: int | None = None,
        to_ms: int | None = None,
        public_key_hex: str | None = None,
    ) -> dict[str, Any]:
        """governance.audit.verify — verify the tamper-evident audit hash chain.

        Optional: from_ms/to_ms export a time-window audit report;
        public_key_hex (hex-encoded Ed25519 public key) enables signature
        verification of signed chains.
        """
        params: dict[str, Any] = {}
        if from_ms is not None:
            params["from_ms"] = from_ms
        if to_ms is not None:
            params["to_ms"] = to_ms
        if public_key_hex is not None:
            params["public_key_hex"] = public_key_hex
        return await self._json_rpc("governance.audit.verify", params)

    # ── Observability ─────────────────────────────────────────────────

    async def health_probes(self) -> HealthProbesResponse:
        """health.probes — module-level health probes (harness_bus + capability_bus)."""
        result = await self._json_rpc("health.probes")
        return HealthProbesResponse(modules=cast(dict[str, Any], result.get("modules", {})))

    async def metrics_get(self) -> MetricsResponse:
        """metrics.get — get current runtime metrics."""
        result = await self._json_rpc("metrics.get")
        return MetricsResponse(metrics=cast(dict[str, Any], result.get("metrics", {})))

    async def metrics_prometheus(self) -> str:
        """metrics.prometheus — get Prometheus-formatted metrics."""
        result = await self._json_rpc("metrics.prometheus")
        return str(result) if result else ""

    async def trace_get(self, limit: int = 20) -> dict[str, Any]:
        """trace.get — get trace entries."""
        return await self._json_rpc("trace.get", {"limit": limit})

    # ── Reliability ───────────────────────────────────────────────────

    async def breaker_status(self) -> BreakerStatusResponse:
        """breaker.status — get circuit breaker status."""
        result = await self._json_rpc("breaker.status")
        return BreakerStatusResponse(breakers=cast(dict[str, Any], result.get("breakers", {})))

    async def breaker_reset(self, name: str) -> dict[str, Any]:
        """breaker.reset — reset a circuit breaker."""
        return await self._json_rpc("breaker.reset", {"name": name})

    async def maintenance_gc(self) -> dict[str, Any]:
        """maintenance.gc — run garbage collection."""
        return await self._json_rpc("maintenance.gc")

    # ── Checkpoint (Phase 4) ──────────────────────────────────────────

    async def checkpoint_create(
        self,
        conversation_id: str,
        messages: list[dict[str, str]],
        branch: str = "main",
    ) -> dict[str, Any]:
        """conversation.checkpoint.create — create a checkpoint for a conversation.

        The backend requires a non-empty `conversation_id` and a non-empty
        `messages` list; `branch` defaults to "main".
        """
        return await self._json_rpc(
            "conversation.checkpoint.create",
            {
                "conversation_id": conversation_id,
                "branch": branch,
                "messages": messages,
            },
        )

    async def checkpoint_list(self, conversation_id: str) -> CheckpointListResponse:
        """checkpoint.list — list available checkpoints for a conversation.

        The backend requires a non-empty `conversation_id`.
        """
        result = await self._json_rpc(
            "checkpoint.list", {"conversation_id": conversation_id}
        )
        return CheckpointListResponse(
            checkpoints=cast(list[dict[str, Any]], result.get("checkpoints", []))
        )

    async def conversation_rollback(
        self, conversation_id: str, checkpoint_id: str
    ) -> dict[str, Any]:
        """conversation.rollback — roll back to a checkpoint.

        The backend requires both `conversation_id` and `checkpoint_id`.
        """
        return await self._json_rpc(
            "conversation.rollback",
            {"conversation_id": conversation_id, "checkpoint_id": checkpoint_id},
        )

    # ── Workflow / Task ───────────────────────────────────────────────

    async def workflow_execute(self) -> dict[str, Any]:
        """workflow.execute — execute the current workflow."""
        return await self._json_rpc("workflow.execute")

    async def task_plan(self, task: str) -> TaskPlanResponse:
        """task.plan — plan a task."""
        result = await self._json_rpc("task.plan", {"task": task})
        return TaskPlanResponse(plan=cast(dict[str, Any], result.get("plan", {})))

    async def task_execute(self, task: str) -> dict[str, Any]:
        """task.execute — execute a planned task."""
        return await self._json_rpc("task.execute", {"task": task})

    # ── Learning / Intelligence ───────────────────────────────────────

    async def learning_summary(self) -> LearningSummaryResponse:
        """learning.summary — get learning loop summary."""
        result = await self._json_rpc("learning.summary")
        return LearningSummaryResponse(summary=cast(dict[str, Any], result.get("summary", {})))

    async def selector_status(self) -> SelectorStatusResponse:
        """selector.status — get model selector status."""
        result = await self._json_rpc("selector.status")
        return SelectorStatusResponse(selector=cast(dict[str, Any], result.get("selector", {})))

    async def knowledge_distill(self, limit: int | None = None) -> dict[str, Any]:
        """knowledge.distill — run knowledge distillation over the last `limit` events."""
        params: dict[str, Any] = {}
        if limit is not None:
            params["limit"] = limit
        return await self._json_rpc("knowledge.distill", params)

    async def rl_alignment_offline_eval(self) -> dict[str, Any]:
        """rl.alignment.offline_eval — run RL alignment offline evaluation."""
        return await self._json_rpc("rl.alignment.offline_eval")

    # ── Optimization / Operations ─────────────────────────────────────

    async def cost_status(self) -> CostStatusResponse:
        """cost.status — get cost optimization status."""
        result = await self._json_rpc("cost.status")
        return CostStatusResponse(cost=cast(dict[str, Any], result.get("cost", {})))

    async def config_baseline(self) -> ConfigBaselineResponse:
        """config.baseline — get config baseline snapshot."""
        result = await self._json_rpc("config.baseline")
        return ConfigBaselineResponse(baseline=cast(dict[str, Any], result.get("baseline", {})))

    async def config_reload(self) -> dict[str, Any]:
        """config.reload — reload runtime config."""
        return await self._json_rpc("config.reload")

    async def harness_status(self) -> HarnessStatusResponse:
        """harness.status — get test harness status."""
        result = await self._json_rpc("harness.status")
        return HarnessStatusResponse(harness=cast(dict[str, Any], result.get("harness", {})))

    # ── ACP Session Protocol ────────────────────────────────────────────

    async def session_new(
        self,
        mode: str | None = None,
        cwd: str | None = None,
        work_dirs: list[str] | None = None,
        additional_directories: list[str] | None = None,
    ) -> dict[str, Any]:
        """session/new — create a new ACP session.

        All parameters are optional; the backend reads `mode`, `cwd`,
        `work_dirs` (snake_case) and `additionalDirectories` (camelCase).
        """
        params: dict[str, Any] = {}
        if mode is not None:
            params["mode"] = mode
        if cwd is not None:
            params["cwd"] = cwd
        if work_dirs is not None:
            params["work_dirs"] = work_dirs
        if additional_directories is not None:
            params["additionalDirectories"] = additional_directories
        return await self._json_rpc("session/new", params)

    async def session_prompt(
        self,
        session_id: str,
        prompt: list[PromptContentBlock | dict[str, Any]],
        mode: str | None = None,
        cwd: str | None = None,
        additional_directories: list[str] | None = None,
    ) -> dict[str, Any]:
        """session/prompt — send a prompt in an ACP session.

        The backend reads `sessionId`, `prompt` (content blocks), `mode`,
        `cwd` and `additionalDirectories`.
        """
        params: dict[str, Any] = {
            "sessionId": session_id,
            "prompt": [_prompt_block_to_dict(block) for block in prompt],
        }
        if mode is not None:
            params["mode"] = mode
        if cwd is not None:
            params["cwd"] = cwd
        if additional_directories is not None:
            params["additionalDirectories"] = additional_directories
        return await self._json_rpc("session/prompt", params)

    async def session_close(self, session_id: str) -> dict[str, Any]:
        """session/close — close an ACP session."""
        return await self._json_rpc("session/close", {"sessionId": session_id})

    async def session_list(self) -> list[SessionInfo]:
        """session/list — list active ACP sessions.

        The backend returns a minimal summary per session:
        ``[{"id": sid}]``.
        """
        result = await self._json_rpc("session/list")
        raw_sessions = cast(list[dict[str, Any]], result.get("sessions", []))
        return [SessionInfo(id=cast(str, raw.get("id", ""))) for raw in raw_sessions]

    async def session_resume(self, session_id: str, cwd: str | None = None) -> dict[str, Any]:
        """session/resume — resume an existing ACP session."""
        params: dict[str, Any] = {"sessionId": session_id}
        if cwd is not None:
            params["cwd"] = cwd
        return await self._json_rpc("session/resume", params)

    async def session_set_mode(self, session_id: str, mode_id: str) -> dict[str, Any]:
        """session/set_mode — set the mode of an ACP session.

        The backend reads `sessionId` and `modeId`.
        """
        return await self._json_rpc(
            "session/set_mode", {"sessionId": session_id, "modeId": mode_id}
        )

    async def session_set_config_option(
        self, session_id: str, config_id: str, value: Any
    ) -> dict[str, Any]:
        """session/set_config_option — set a configuration option for an ACP session.

        The backend reads `sessionId`, `configId` and `value`.
        """
        return await self._json_rpc(
            "session/set_config_option",
            {"sessionId": session_id, "configId": config_id, "value": value},
        )

    # ── Tools ────────────────────────────────────────────────────────────

    async def tools_list(self) -> list[ToolInfo]:
        """tools/list — list all available tools with their input schemas.

        Returns
        -------
        list[ToolInfo]
            A list of tool descriptors exposed by the server.
        """
        result = await self._json_rpc("tools/list")
        raw_tools = cast(list[dict[str, Any]], result.get("tools", []))
        tools: list[ToolInfo] = []
        for raw in raw_tools:
            tools.append(
                ToolInfo(
                    name=cast(str, raw.get("name", "")),
                    description=cast(str, raw.get("description", "")),
                    input_schema=cast(dict[str, Any], raw.get("input_schema", {})),
                )
            )
        return tools

    async def tools_call(self, request: ToolCallRequest) -> ToolCallResult:
        """tools/call — execute a tool by name with the given arguments.

        Parameters
        ----------
        request:
            The tool call request specifying the tool name, arguments,
            and optionally a session ID for progress streaming.

        Returns
        -------
        ToolCallResult
            The result of the tool execution, including success status,
            output text, error details, and wall-clock duration.
        """
        params: dict[str, Any] = {
            "name": request.tool_name,
            "arguments": request.arguments,
        }
        if request.session_id is not None:
            params["sessionId"] = request.session_id

        start = time.monotonic()
        try:
            result = await self._json_rpc("tools/call", params)
        except GoOnClientError as exc:
            elapsed_ms = int((time.monotonic() - start) * 1000)
            return ToolCallResult(
                success=False,
                output=None,
                error=str(exc),
                duration_ms=elapsed_ms,
            )

        elapsed_ms = int((time.monotonic() - start) * 1000)

        # Extract text output from the MCP-style content array or structured field
        content = cast(list[dict[str, Any]], result.get("content", []))
        output: str | None = cast("str | None", result.get("structured"))
        if output is None and content:
            output = cast(str, content[0].get("text", ""))

        return ToolCallResult(
            success=True,
            output=output,
            error=None,
            duration_ms=elapsed_ms,
        )
