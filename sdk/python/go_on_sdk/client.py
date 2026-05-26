from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass, field
from typing import Any, AsyncGenerator, Dict, List, Optional

import httpx


class GoOnClientError(Exception):
    """Custom exception for go-on SDK client errors."""


class GoOnJsonRpcError(GoOnClientError):
    """JSON-RPC protocol-level error."""

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
    messages: List[ChatMessage]
    model: Optional[str] = None
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None
    stream: Optional[bool] = None


# ── Response types ──────────────────────────────────────────────────


@dataclass
class HealthResponse:
    status: str
    version: str
    uptime_seconds: int
    modules: Dict[str, Any] = field(default_factory=dict)


@dataclass
class GovernanceStatusResponse:
    ok: bool
    governance: Dict[str, Any]


@dataclass
class HealthProbesResponse:
    modules: Dict[str, Any]


@dataclass
class MetricsResponse:
    metrics: Dict[str, Any]


@dataclass
class BreakerStatusResponse:
    breakers: Dict[str, Any]


@dataclass
class CheckpointListResponse:
    checkpoints: List[Dict[str, Any]]


@dataclass
class TaskPlanResponse:
    plan: Dict[str, Any]


@dataclass
class LearningSummaryResponse:
    summary: Dict[str, Any]


@dataclass
class SelectorStatusResponse:
    selector: Dict[str, Any]


@dataclass
class CostStatusResponse:
    cost: Dict[str, Any]


@dataclass
class ConfigBaselineResponse:
    baseline: Dict[str, Any]


@dataclass
class HarnessStatusResponse:
    harness: Dict[str, Any]


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
        self._client = httpx.AsyncClient(timeout=httpx.Timeout(timeout))

    async def aclose(self) -> None:
        await self._client.aclose()

    # ── Internal helpers ──────────────────────────────────────────────

    async def _json_rpc(
        self, method: str, params: Optional[Dict[str, Any]] = None
    ) -> Any:
        payload: Dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params or {},
        }

        last_error: Optional[Exception] = None

        for attempt in range(self.max_retries + 1):
            try:
                resp = await self._client.post(
                    f"{self.base_url}/v1/responses", json=payload
                )
                resp.raise_for_status()
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
            except (GoOnClientError, httpx.HTTPStatusError, json.JSONDecodeError):
                # Non-transient errors — do not retry
                raise
            except Exception as e:
                last_error = e
                if attempt < self.max_retries:
                    await asyncio.sleep(self.retry_delay)

        raise GoOnClientError(
            f"Request failed after {self.max_retries} retries: {last_error}"
        ) from last_error

    # ── Streaming chat ────────────────────────────────────────────────

    async def chat_stream(self, request: ChatRequest) -> AsyncGenerator[dict, None]:
        """Send a chat request and yield SSE events as they arrive.

        Each yielded value is a parsed JSON object from a ``data:`` line
        in the SSE stream.

        Yields
        ------
        dict
            A JSON chunk from the stream.
        """
        request_dict: Dict[str, Any] = {
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
                if line.startswith("data: "):
                    yield json.loads(line[6:])

    # ── Core Runtime ──────────────────────────────────────────────────

    async def health(self) -> HealthResponse:
        """GET /health — quick health check."""
        resp = await self._client.get(f"{self.base_url}/health")
        resp.raise_for_status()
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

    async def runtime_stability(self) -> Dict[str, Any]:
        """runtime.stability — runtime stability snapshot."""
        return await self._json_rpc("runtime.stability")

    async def initialize(self, setup_level: str = "standard") -> Dict[str, Any]:
        """initialize — initialize the runtime."""
        return await self._json_rpc("initialize", {"setup_level": setup_level})

    async def shutdown(self) -> Dict[str, Any]:
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

    async def governance_plan_get(self) -> Dict[str, Any]:
        """governance.plan.get — get active governance plan."""
        return await self._json_rpc("governance.plan.get")

    async def governance_audit_recent(self, limit: int = 20) -> Dict[str, Any]:
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

    async def trace_get(self, limit: int = 20) -> Dict[str, Any]:
        """trace.get — get trace entries."""
        return await self._json_rpc("trace.get", {"limit": limit})

    # ── Reliability ───────────────────────────────────────────────────

    async def breaker_status(self) -> BreakerStatusResponse:
        """breaker.status — get circuit breaker status."""
        result = await self._json_rpc("breaker.status")
        return BreakerStatusResponse(breakers=result.get("breakers", {}))

    async def breaker_reset(self, name: str) -> Dict[str, Any]:
        """breaker.reset — reset a circuit breaker."""
        return await self._json_rpc("breaker.reset", {"name": name})

    async def maintenance_gc(self) -> Dict[str, Any]:
        """maintenance.gc — run garbage collection."""
        return await self._json_rpc("maintenance.gc")

    # ── Checkpoint (Phase 4) ──────────────────────────────────────────

    async def checkpoint_create(self, branch: str) -> Dict[str, Any]:
        """checkpoint.create — create a runtime checkpoint."""
        return await self._json_rpc("checkpoint.create", {"branch": branch})

    async def checkpoint_list(self) -> CheckpointListResponse:
        """checkpoint.list — list available checkpoints."""
        result = await self._json_rpc("checkpoint.list")
        return CheckpointListResponse(checkpoints=result.get("checkpoints", []))

    async def conversation_rollback(self, checkpoint_id: str) -> Dict[str, Any]:
        """conversation.rollback — roll back to a checkpoint."""
        return await self._json_rpc(
            "conversation.rollback", {"checkpoint_id": checkpoint_id}
        )

    # ── Workflow / Task ───────────────────────────────────────────────

    async def workflow_execute(self) -> Dict[str, Any]:
        """workflow.execute — execute the current workflow."""
        return await self._json_rpc("workflow.execute")

    async def task_plan(self, description: str) -> TaskPlanResponse:
        """task.plan — plan a task."""
        result = await self._json_rpc("task.plan", {"description": description})
        return TaskPlanResponse(plan=result.get("plan", {}))

    async def task_execute(self, plan_id: str) -> Dict[str, Any]:
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

    async def knowledge_distill(self, source: str) -> Dict[str, Any]:
        """knowledge.distill — run knowledge distillation."""
        return await self._json_rpc("knowledge.distill", {"source": source})

    async def rl_alignment_offline_eval(self) -> Dict[str, Any]:
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

    async def config_reload(self) -> Dict[str, Any]:
        """config.reload — reload runtime config."""
        return await self._json_rpc("config.reload")

    async def harness_status(self) -> HarnessStatusResponse:
        """harness.status — get test harness status."""
        result = await self._json_rpc("harness.status")
        return HarnessStatusResponse(harness=result.get("harness", {}))
