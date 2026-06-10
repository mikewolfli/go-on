"""Basic usage example for the go-on Python SDK.

Run with: python examples/basic_usage.py
(requires a running go-on backend at http://127.0.0.1:8090)
"""

import asyncio
from typing import cast

from go_on_sdk import GoOnClient
from go_on_sdk.client import ChatMessage, ChatRequest


async def main() -> None:
    """Run basic go-on SDK operations against a local backend."""
    client = GoOnClient(base_url="http://localhost:8090")

    # Health check
    health = await client.health()
    print(f"Health: status={health.status}, version={health.version}")

    # Governance status
    governance = await client.governance_status()
    print(f"Governance OK: {governance.ok}")

    # Stream a chat message
    print("\nStreaming chat...")
    msg = ChatMessage(role="user", content="Say hello in one word.")
    request = ChatRequest(messages=[msg], model="gpt-4", stream=True)
    async for chunk in client.chat_stream(request):
        if "content" in chunk:
            print(cast(str, chunk["content"]), end="", flush=True)
    print()

    await client.aclose()


asyncio.run(main())
