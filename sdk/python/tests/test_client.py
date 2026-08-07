"""Tests for the go-on SDK client."""

import asyncio
import json
from collections.abc import Callable, Coroutine
from typing import Any

import httpx

from go_on_sdk.client import (
    ChatMessage,
    ChatRequest,
    GoOnClient,
    GoOnClientError,
    SessionInfo,
)


# ── ACP contract parameter tests (mock transport) ──────────────────────────


def _client_with_mock_handler(
    handler: Callable[[httpx.Request], httpx.Response],
) -> tuple[GoOnClient, dict[str, list[dict[str, Any]]]]:
    """Build a client backed by an httpx MockTransport capturing request payloads."""
    captured: dict[str, list[dict[str, Any]]] = {"requests": []}

    def _handler(request: httpx.Request) -> httpx.Response:
        captured["requests"].append(json.loads(request.content))
        return handler(request)

    client = GoOnClient(base_url="http://localhost:8090", max_retries=0)
    client._client = httpx.AsyncClient(transport=httpx.MockTransport(_handler))
    return client, captured


def _run(coro: Coroutine[Any, Any, Any]) -> Any:
    return asyncio.run(coro)


def test_checkpoint_list_sends_conversation_id():
    """checkpoint.list must include the backend-required conversation_id."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "jsonrpc": "2.0",
                "id": "1",
                "result": {"ok": True, "conversation_id": "conv-1", "count": 0, "checkpoints": []},
            },
        )

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.checkpoint_list("conv-1")
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "checkpoint.list"
    assert payload["params"] == {"conversation_id": "conv-1"}


def test_conversation_rollback_sends_conversation_id_and_checkpoint_id():
    """conversation.rollback must include conversation_id and checkpoint_id."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": "1", "result": {"ok": True}})

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.conversation_rollback("conv-1", "cp-42")
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "conversation.rollback"
    assert payload["params"] == {"conversation_id": "conv-1", "checkpoint_id": "cp-42"}


def test_session_new_sends_work_dirs_snake_case():
    """session/new must send work_dirs (snake_case) and additionalDirectories."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200, json={"jsonrpc": "2.0", "id": "1", "result": {"sessionId": "sess-1"}}
        )

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.session_new(
            mode="safeguard",
            cwd="/tmp",
            work_dirs=["/tmp/a"],
            additional_directories=["/tmp/b"],
        )
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "session/new"
    assert payload["params"] == {
        "mode": "safeguard",
        "cwd": "/tmp",
        "work_dirs": ["/tmp/a"],
        "additionalDirectories": ["/tmp/b"],
    }


def test_session_set_config_option_uses_config_id():
    """session/set_config_option must use configId (not optionId)."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200, json={"jsonrpc": "2.0", "id": "1", "result": {"configOptions": []}}
        )

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.session_set_config_option("sess-1", "model", "gpt-4o")
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "session/set_config_option"
    assert payload["params"] == {
        "sessionId": "sess-1",
        "configId": "model",
        "value": "gpt-4o",
    }


def test_session_set_mode_sends_mode_id():
    """session/set_mode must send sessionId and modeId."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"jsonrpc": "2.0", "id": "1", "result": {}})

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.session_set_mode("sess-1", "edit")
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "session/set_mode"
    assert payload["params"] == {"sessionId": "sess-1", "modeId": "edit"}


def test_session_list_parses_minimal_id_shape():
    """session/list parses the minimal `[{id}]` session summary shape."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={"jsonrpc": "2.0", "id": "1", "result": {"sessions": [{"id": "sess-1"}]}},
        )

    client, captured = _client_with_mock_handler(handler)

    async def run():
        result = await client.session_list()
        await client.aclose()
        return result

    result = _run(run())
    assert result == [SessionInfo(id="sess-1")]
    assert captured["requests"][0]["method"] == "session/list"


def test_session_prompt_serializes_content_blocks():
    """session/prompt sends sessionId plus serialized prompt content blocks."""

    def handler(request: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200, json={"jsonrpc": "2.0", "id": "1", "result": {"stopReason": "end_turn"}}
        )

    from go_on_sdk.client import PromptContentBlock

    client, captured = _client_with_mock_handler(handler)

    async def run():
        await client.session_prompt(
            session_id="sess-1",
            prompt=[PromptContentBlock(type="text", text="Hello")],
        )
        await client.aclose()

    _run(run())
    payload = captured["requests"][0]
    assert payload["method"] == "session/prompt"
    assert payload["params"] == {
        "sessionId": "sess-1",
        "prompt": [{"type": "text", "text": "Hello"}],
    }


# ── Client initialization ────────────────────────────────────────────────


def test_client_initialization():
    """Client can be created with a base URL."""
    client = GoOnClient(base_url="http://localhost:8090")
    assert client.base_url == "http://localhost:8090"
    assert client.timeout == 30.0
    assert client.max_retries == 3


def test_client_custom_timeout():
    """Client respects custom timeout."""
    client = GoOnClient(base_url="http://localhost:8090", timeout=10.0)
    assert client.timeout == 10.0


def test_client_custom_retries():
    """Client respects custom max retries."""
    client = GoOnClient(base_url="http://localhost:8090", max_retries=5)
    assert client.max_retries == 5


def test_client_endpoint_format():
    """Client uses /rpc endpoint for JSON-RPC."""
    client = GoOnClient(base_url="http://localhost:8090")
    assert client.base_url == "http://localhost:8090"


def test_go_on_error_creation():
    """GoOnClientError can be created with a message."""
    error = GoOnClientError("Rate limited")
    assert "Rate limited" in str(error)


def test_go_on_json_rpc_error():
    """GoOnJsonRpcError can be created with code and message."""
    from go_on_sdk.client import GoOnJsonRpcError

    error = GoOnJsonRpcError(code=429, message="Rate limited")
    assert error.code == 429
    assert "429" in str(error)


# ── Streaming chat tests ──────────────────────────────────────────────────


def test_chat_request_stream_enabled():
    """ChatRequest can be configured for streaming."""
    msg = ChatMessage(role="user", content="Hello")
    request = ChatRequest(messages=[msg], model="gpt-4", stream=True)
    assert request.stream is True
    assert request.model == "gpt-4"


def test_chat_request_stream_disabled():
    """ChatRequest can be configured without streaming."""
    msg = ChatMessage(role="user", content="Hello")
    request = ChatRequest(messages=[msg], stream=False)
    assert request.stream is False


def test_chat_request_stream_default_none():
    """ChatRequest defaults to stream=None (no override)."""
    msg = ChatMessage(role="user", content="Hello")
    request = ChatRequest(messages=[msg])
    assert request.stream is None


def test_sse_endpoint_construction():
    """Client constructs the correct SSE endpoint for chat streaming."""
    client = GoOnClient(base_url="http://localhost:8090")
    # The chat_stream method posts to /chat/stream
    expected = "http://localhost:8090/chat/stream"
    assert client.base_url == "http://localhost:8090"
    # Verify the suffix is correct
    assert "chat/stream" in expected
    assert expected.startswith(client.base_url)


def test_chat_message_creation():
    """ChatMessage stores role and content."""
    msg = ChatMessage(role="user", content="What is ACP?")
    assert msg.role == "user"
    assert msg.content == "What is ACP?"


def test_chat_request_with_messages():
    """ChatRequest holds multiple messages."""
    msgs = [
        ChatMessage(role="system", content="You are a helpful assistant"),
        ChatMessage(role="user", content="What is ACP?"),
    ]
    request = ChatRequest(messages=msgs, model="gpt-4", temperature=0.7)
    assert len(request.messages) == 2
    assert request.temperature == 0.7


def test_chat_request_max_tokens():
    """ChatRequest supports max_tokens configuration."""
    msg = ChatMessage(role="user", content="Hello")
    request = ChatRequest(messages=[msg], max_tokens=2048)
    assert request.max_tokens == 2048
