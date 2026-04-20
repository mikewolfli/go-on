from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict

import httpx


@dataclass
class GovernanceStatusResponse:
    ok: bool
    governance: Dict[str, Any]


class GoOnClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient()

    async def governance_status(self) -> GovernanceStatusResponse:
        """S15 SDK stub: fetch governance.status via JSON-RPC."""
        payload = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "governance.status",
            "params": {},
        }
        resp = await self._client.post(f"{self.base_url}/v1/responses", json=payload)
        resp.raise_for_status()
        data = resp.json()
        result = data.get("result", {})
        return GovernanceStatusResponse(
            ok=bool(result.get("ok", False)),
            governance=result.get("governance", {}) or {},
        )

    async def aclose(self) -> None:
        await self._client.aclose()
