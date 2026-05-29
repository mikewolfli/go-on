from __future__ import annotations

import asyncio
import json
import logging
import uuid
from collections.abc import AsyncGenerator
from dataclasses import dataclass, field
from typing import Any

import httpx

logger = logging.getLogger(__name__)


class GoOnClientError(Exception):
    """Custom exception for go-on SDK client errors."""


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
    status: str
    version: str
    uptime_seconds: int
    modules: dict[str, Any] = field(default_factory=dict)


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


# ── Client ──────────────────────────────────────────────────────────


class GoOnClient:
    """Async client for go-on ACP JSON-RPC endpoints.

    Targets ``POST {base_url}/v1/responses`` for JSON-RPC calls
    and direct HTTP GET for ``/health``.

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
        Delay in seconds between retries (default: 1.0).
    """

    def __init__(
        self,
        base_url: str,
        timeout: float = 30.0,
        max_retries: int = 3,
        retry_delay: float = 1.0,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.max_retries = max_retries
        self.retry_delay = retry_delay
        self._client: httpx.AsyncClient = httpx.AsyncClient(
            timeout=httpx.Timeout(timeout)
        )

    async def aclose(self) -> None:
        await self._client.aclose()

    # ── Internal helpers ──────────────────────────────────────────────

    async def _json_rpc(self, method: str, params: dict[str, Any] | None = None) -> Any:
        payload: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": str(uuid.uuid4()),
            "method": method,
            "params": params or {},
        }

        last_error: Exception | None = None

        for attempt in range(self.max_retries + 1):
            try:
                resp = await self._client.post(
                    f"{self.base_url}/v1/responses", json=payload
                )
                _ = resp.raise_for_status()
                try:
                    data = resp.json()
                except json.JSONDecodeError:
                    raise GoOnClientError(
                        f"Server returned non-JSON response: {resp.text[:500]}"
                    ) from None

                if "error" in data:
                    err = data["error"]
                    raise GoOnJsonRpcError(
                        code=err.get("code", -1),
                        message=err.get("message", "unknown"),
                    )
                return data.get("result", {})
            except (
                httpx.TimeoutException,
                httpx.NetworkError,
                httpx.RemoteProtocolError,
            ) as e:
                last_error = e
                if attempt < self.max_retries:
                    await asyncio.sleep(self.retry_delay)
            except (
                GoOnClientError,
                httpx.HTTPStatusError,
                json.JSONDecodeError,
                KeyboardInterrupt,
                SystemExit,
            ):
                # Non-transient errors — do not retry
                raise
            # No bare except Exception — only known retryable network errors are retried

        raise GoOnClientError(
            f"Request failed after {self.max_retries} retries: {last_error}"
        ) from last_error

    # ── Streaming chat ────────────────────────────────────────────────

    async def chat_stream(
        self, request: ChatRequest
    ) -> AsyncGenerator[dict[str, Any], None]:
        """Send a chat request and yield SSE events as they arrive.

        Each yielded value is a parsed JSON object from a ``data:`` line
        in the SSE stream.

        Yields
        ------
        dict
            A JSON chunk from the stream.
        """
        request_dict: dict[str, Any] = {
            "messages": [
                {"role": m.role, "content": m.content} for m in request.messages
            ],
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
            f"{self.base_url}/acp/chat",
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
        """GET /health — quick health check."""
        resp = await self._client.get(f"{self.base_url}/health")
        _ = resp.raise_for_status()
        data = resp.json()
        return HealthResponse(
            status=data.get("status", "unknown"),
            version=data.get("version", ""),
            uptime_seconds=data.get("uptime_seconds", 0),
            modules=data.get("modules", {}),
        )

    async def runtime_health(self) -> HealthResponse:
        """runtime.health — full runtime health via JSON-RPC."""
        result = await self._json_rpc("runtime.health")
        return HealthResponse(
            status=result.get("status", "unknown"),
            version=result.get("version", ""),
            uptime_seconds=result.get("uptime_seconds", 0),
            modules=result.get("modules", {}),
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
            ok=bool(result.get("ok", False)),
            governance=result.get("governance", {}),
        )

    async def governance_plan_get(self) -> dict[str, Any]:
        """governance.plan.get — get active governance plan."""
        return await self._json_rpc("governance.plan.get")

    async def governance_audit_recent(self, limit: int = 20) -> dict[str, Any]:
        """governance.audit.recent — view recent audit entries."""
        return await self._json_rpc("governance.audit.recent", {"limit": limit})

    # ── Observability ─────────────────────────────────────────────────

    async def health_probes(self) -> HealthProbesResponse:
        """health.probes — module-level health probes (harness_bus + capability_bus)."""
        result = await self._json_rpc("health.probes")
        return HealthProbesResponse(modules=result.get("modules", {}))

    async def metrics_get(self) -> MetricsResponse:
        """metrics.get — get current runtime metrics."""
        result = await self._json_rpc("metrics.get")
        return MetricsResponse(metrics=result.get("metrics", {}))

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
        return BreakerStatusResponse(breakers=result.get("breakers", {}))

    async def breaker_reset(self, name: str) -> dict[str, Any]:
        """breaker.reset — reset a circuit breaker."""
        return await self._json_rpc("breaker.reset", {"name": name})

    async def maintenance_gc(self) -> dict[str, Any]:
        """maintenance.gc — run garbage collection."""
        return await self._json_rpc("maintenance.gc")

    # ── Checkpoint (Phase 4) ──────────────────────────────────────────

    async def checkpoint_create(self, branch: str) -> dict[str, Any]:
        """checkpoint.create — create a runtime checkpoint."""
        return await self._json_rpc("checkpoint.create", {"branch": branch})

    async def checkpoint_list(self) -> CheckpointListResponse:
        """checkpoint.list — list available checkpoints."""
        result = await self._json_rpc("checkpoint.list")
        return CheckpointListResponse(checkpoints=result.get("checkpoints", []))

    async def conversation_rollback(self, checkpoint_id: str) -> dict[str, Any]:
        """conversation.rollback — roll back to a checkpoint."""
        return await self._json_rpc(
            "conversation.rollback", {"checkpoint_id": checkpoint_id}
        )

    # ── Workflow / Task ───────────────────────────────────────────────

    async def workflow_execute(self) -> dict[str, Any]:
        """workflow.execute — execute the current workflow."""
        return await self._json_rpc("workflow.execute")

    async def task_plan(self, description: str) -> TaskPlanResponse:
        """task.plan — plan a task."""
        result = await self._json_rpc("task.plan", {"description": description})
        return TaskPlanResponse(plan=result.get("plan", {}))

    async def task_execute(self, plan_id: str) -> dict[str, Any]:
        """task.execute — execute a planned task."""
        return await self._json_rpc("task.execute", {"plan_id": plan_id})

    # ── Learning / Intelligence ───────────────────────────────────────

    async def learning_summary(self) -> LearningSummaryResponse:
        """learning.summary — get learning loop summary."""
        result = await self._json_rpc("learning.summary")
        return LearningSummaryResponse(summary=result.get("summary", {}))

    async def selector_status(self) -> SelectorStatusResponse:
        """selector.status — get model selector status."""
        result = await self._json_rpc("selector.status")
        return SelectorStatusResponse(selector=result.get("selector", {}))

    async def knowledge_distill(self, source: str) -> dict[str, Any]:
        """knowledge.distill — run knowledge distillation."""
        return await self._json_rpc("knowledge.distill", {"source": source})

    async def rl_alignment_offline_eval(self) -> dict[str, Any]:
        """rl.alignment.offline_eval — run RL alignment offline evaluation."""
        return await self._json_rpc("rl.alignment.offline_eval")

    # ── Optimization / Operations ─────────────────────────────────────

    async def cost_status(self) -> CostStatusResponse:
        """cost.status — get cost optimization status."""
        result = await self._json_rpc("cost.status")
        return CostStatusResponse(cost=result.get("cost", {}))

    async def config_baseline(self) -> ConfigBaselineResponse:
        """config.baseline — get config baseline snapshot."""
        result = await self._json_rpc("config.baseline")
        return ConfigBaselineResponse(baseline=result.get("baseline", {}))

    async def config_reload(self) -> dict[str, Any]:
        """config.reload — reload runtime config."""
        return await self._json_rpc("config.reload")

    async def harness_status(self) -> HarnessStatusResponse:
        """harness.status — get test harness status."""
        result = await self._json_rpc("harness.status")
        return HarnessStatusResponse(harness=result.get("harness", {}))
