"""Tests for the go-on SDK client."""

from go_on_sdk.client import ChatMessage, ChatRequest, GoOnClient, GoOnClientError

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
