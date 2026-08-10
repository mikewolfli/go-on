"""Basic usage example for the go-on Python SDK.

Run with: python examples/basic_usage.py
(requires a running go-on backend at http://127.0.0.1:8090)
"""

import asyncio
from typing import cast

from go_on_sdk import GoOnClient
from go_on_sdk.client import ChatMessage, ChatRequest, ToolCallRequest


async def main() -> None:
    """Run basic go-on SDK operations against a local backend."""
    client = GoOnClient(base_url="http://localhost:8090")

    # Health check — `GET /health` (ServerStatus) carries `lifecycle` and
    # `timestamp`; it does NOT emit `status`/`version`, so print the real fields.
    health = await client.health()
    print(f"Health: lifecycle={health.lifecycle}, timestamp={health.timestamp}")

    # Governance status
    governance = await client.governance_status()
    print(f"Governance OK: {governance.ok}")

    # List available tools
    tools = await client.tools_list()
    print(f"\nAvailable tools ({len(tools)}):")
    for tool in tools:
        schema_str = str(tool.input_schema.get("type", "unknown"))
        print(f"  - {tool.name}: {tool.description} (schema: {schema_str})")

    # Call a tool if any are available
    if tools:
        first = tools[0]
        print(f"\nCalling tool: {first.name}")
        req = ToolCallRequest(tool_name=first.name, arguments={})
        result = await client.tools_call(req)
        if result.success:
            print(f"  Success ({result.duration_ms}ms): {result.output}")
        else:
            print(f"  Failed ({result.duration_ms}ms): {result.error}")

    # Stream a chat message
    print("\nStreaming chat...")
    msg = ChatMessage(role="user", content="Say hello in one word.")
    request = ChatRequest(messages=[msg], model="gpt-4", stream=True)
    async for chunk in client.chat_stream(request):
        content = chunk.get("content") or chunk.get("token")
        if content:
            print(cast(str, content), end="", flush=True)
    print()

    await client.aclose()


asyncio.run(main())
