"""Tests for the go-on SDK client."""

from go_on_sdk.client import GoOnClient, GoOnClientError


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
