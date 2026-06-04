"""Basic usage example for the go-on Python SDK."""

import asyncio

from go_on_sdk import GoOnClient


async def main():
    client = GoOnClient(base_url="http://localhost:8090")

    # Send a chat message
    response = await client.chat(messages=[{"role": "user", "content": "Hello!"}])
    print(f"Response: {response}")

    # Stream a chat
    async for chunk in client.chat_stream(
        messages=[{"role": "user", "content": "Tell me a story"}]
    ):
        print(chunk, end="", flush=True)
    print()

    await client.close()


asyncio.run(main())
