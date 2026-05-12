"""Tests for python/bridge_server.py — the JSON-RPC base bridge protocol.

Run with:
    uv run --with pytest python3 -m pytest tests/test_bridge_server.py -v
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

# Add python/ directory to sys.path so the bridge modules are importable
_PYTHON_DIR = Path(__file__).parent.parent / "python"
if str(_PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(_PYTHON_DIR))

import bridge_server as bs


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _make_request(method: str, params: dict = None, req_id: str = "test-1") -> str:
    return json.dumps({"id": req_id, "method": method, "params": params or {}})


def _parse_response(line: str) -> dict:
    return json.loads(line)


# ---------------------------------------------------------------------------
# _error helper
# ---------------------------------------------------------------------------


class TestErrorHelper:
    def test_error_returns_dict_with_code_and_message(self):
        result = bs._error(-32601, "not found")
        assert result == {"code": -32601, "message": "not found"}

    def test_error_preserves_negative_codes(self):
        err = bs._error(-32603, "internal")
        assert err["code"] == -32603

    def test_error_message_is_string(self):
        err = bs._error(bs.ERROR_INTERNAL, "boom")
        assert isinstance(err["message"], str)


# ---------------------------------------------------------------------------
# BridgeServer — registration and dispatch
# ---------------------------------------------------------------------------


class TestBridgeServerRegistration:
    def test_health_handler_registered_at_construction(self):
        server = bs.BridgeServer("test-server")
        assert "bridge.health" in server._handlers

    def test_register_adds_handler(self):
        server = bs.BridgeServer("test-server")
        handler = MagicMock(return_value={"ok": True})
        server.register("my.method", handler)
        assert "my.method" in server._handlers

    def test_register_overwrites_existing_handler(self):
        server = bs.BridgeServer("test-server")
        h1 = MagicMock(return_value=1)
        h2 = MagicMock(return_value=2)
        server.register("my.method", h1)
        server.register("my.method", h2)
        assert server._handlers["my.method"] is h2

    def test_server_name_stored(self):
        server = bs.BridgeServer("my-bridge")
        assert server.server_name == "my-bridge"


class TestBridgeServerDispatch:
    def test_dispatch_calls_registered_handler(self):
        server = bs.BridgeServer("test-server")
        handler = MagicMock(return_value={"result": 42})
        server.register("my.method", handler)
        result = server.dispatch("my.method", {"key": "val"})
        handler.assert_called_once_with({"key": "val"})
        assert result == {"result": 42}

    def test_dispatch_unknown_method_raises_method_not_found_error(self):
        server = bs.BridgeServer("test-server")
        with pytest.raises(bs.MethodNotFoundError) as exc_info:
            server.dispatch("no.such.method", {})
        assert "no.such.method" in str(exc_info.value)

    def test_dispatch_health_check_returns_correct_structure(self):
        server = bs.BridgeServer("healthy-bridge")
        result = server.dispatch("bridge.health", {})
        assert result == {"server_name": "healthy-bridge", "healthy": True}

    def test_dispatch_handler_exception_propagates(self):
        server = bs.BridgeServer("test-server")
        server.register("boom", lambda p: (_ for _ in ()).throw(RuntimeError("kaboom")))
        with pytest.raises(RuntimeError, match="kaboom"):
            server.dispatch("boom", {})


# ---------------------------------------------------------------------------
# MethodNotFoundError
# ---------------------------------------------------------------------------


class TestMethodNotFoundError:
    def test_is_exception_subclass(self):
        err = bs.MethodNotFoundError("foo.bar")
        assert isinstance(err, Exception)

    def test_stores_method_name(self):
        err = bs.MethodNotFoundError("my.missing.method")
        assert err.method == "my.missing.method"

    def test_str_includes_method_name(self):
        err = bs.MethodNotFoundError("foo.bar")
        assert "foo.bar" in str(err)


# ---------------------------------------------------------------------------
# BridgeServer.run() — stdin/stdout processing
# ---------------------------------------------------------------------------


class TestBridgeServerRun:
    """Test the JSON-RPC loop via monkey-patched stdin/stdout."""

    def _run_with_input(self, server: bs.BridgeServer, lines: list[str]) -> list[dict]:
        """Feed lines into server.run() and collect parsed responses."""
        stdin_data = "\n".join(lines) + "\n"
        responses: list[dict] = []

        captured_lines: list[str] = []

        def fake_write_response(response: dict) -> None:
            captured_lines.append(json.dumps(response))

        with (
            patch("sys.stdin", io.StringIO(stdin_data)),
            patch("bridge_server._write_response", side_effect=fake_write_response),
        ):
            server.run()

        return [json.loads(line) for line in captured_lines]

    def test_health_request_returns_success(self):
        server = bs.BridgeServer("test-srv")
        req = _make_request("bridge.health", {}, "req-1")
        responses = self._run_with_input(server, [req])
        assert len(responses) == 1
        assert responses[0]["id"] == "req-1"
        assert responses[0]["result"]["healthy"] is True

    def test_unknown_method_returns_error(self):
        server = bs.BridgeServer("test-srv")
        req = _make_request("no.such.method", {}, "req-2")
        responses = self._run_with_input(server, [req])
        assert len(responses) == 1
        resp = responses[0]
        assert resp["id"] == "req-2"
        assert "error" in resp
        assert resp["error"]["code"] == bs.ERROR_METHOD_NOT_FOUND

    def test_handler_exception_returns_internal_error(self):
        server = bs.BridgeServer("test-srv")
        server.register("bad.method", lambda p: (_ for _ in ()).throw(RuntimeError("oops")))
        req = _make_request("bad.method", {}, "req-3")
        responses = self._run_with_input(server, [req])
        assert len(responses) == 1
        assert responses[0]["error"]["code"] == bs.ERROR_INTERNAL

    def test_malformed_json_returns_internal_error(self):
        server = bs.BridgeServer("test-srv")
        responses = self._run_with_input(server, ["not-json{"])
        assert len(responses) == 1
        assert responses[0]["error"]["code"] == bs.ERROR_INTERNAL

    def test_empty_lines_are_skipped(self):
        server = bs.BridgeServer("test-srv")
        req = _make_request("bridge.health", {}, "req-5")
        responses = self._run_with_input(server, ["", "   ", req])
        assert len(responses) == 1  # only one response for the valid request

    def test_multiple_requests_all_processed(self):
        server = bs.BridgeServer("test-srv")
        server.register("echo", lambda p: p)
        reqs = [
            _make_request("echo", {"n": i}, f"req-{i}")
            for i in range(5)
        ]
        responses = self._run_with_input(server, reqs)
        assert len(responses) == 5
        ids = {r["id"] for r in responses}
        assert ids == {f"req-{i}" for i in range(5)}

    def test_missing_id_defaults_to_unknown(self):
        server = bs.BridgeServer("test-srv")
        req = json.dumps({"method": "bridge.health", "params": {}})
        responses = self._run_with_input(server, [req])
        assert len(responses) == 1
        assert responses[0]["id"] == "unknown"

    def test_missing_params_defaults_to_empty_dict(self):
        server = bs.BridgeServer("test-srv")
        server.register("echo", lambda p: {"got": p})
        req = json.dumps({"id": "x", "method": "echo"})
        responses = self._run_with_input(server, [req])
        assert responses[0]["result"]["got"] == {}


# ---------------------------------------------------------------------------
# EchoBridgeServer
# ---------------------------------------------------------------------------


class TestEchoBridgeServer:
    def test_echo_method_registered(self):
        server = bs.EchoBridgeServer()
        assert "echo" in server._handlers

    def test_echo_returns_params(self):
        server = bs.EchoBridgeServer()
        params = {"foo": "bar", "num": 42}
        result = server.dispatch("echo", params)
        assert result == params

    def test_server_name_is_echo(self):
        server = bs.EchoBridgeServer()
        assert server.server_name == "echo"

    def test_health_check_also_available(self):
        server = bs.EchoBridgeServer()
        result = server.dispatch("bridge.health", {})
        assert result["healthy"] is True
        assert result["server_name"] == "echo"


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------


class TestConstants:
    def test_error_codes_are_negative(self):
        assert bs.ERROR_METHOD_NOT_FOUND < 0
        assert bs.ERROR_INTERNAL < 0
        assert bs.ERROR_TIMEOUT < 0
        assert bs.ERROR_TRANSPORT < 0

    def test_method_not_found_is_jsonrpc_standard(self):
        assert bs.ERROR_METHOD_NOT_FOUND == -32601

    def test_internal_error_is_jsonrpc_standard(self):
        assert bs.ERROR_INTERNAL == -32603
