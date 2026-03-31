"""Base bridge server protocol for Simard subprocess bridges.

Each bridge server reads newline-delimited JSON requests from stdin and writes
newline-delimited JSON responses to stdout. The protocol mirrors JSON-RPC
conventions without requiring the full spec.

Wire format (request, one per line on stdin):
    {"id": "<uuid>", "method": "<name>", "params": {...}}

Wire format (response, one per line on stdout):
    {"id": "<uuid>", "result": {...}}
    {"id": "<uuid>", "error": {"code": <int>, "message": "..."}}

Subclasses register method handlers and the base loop dispatches automatically.
The built-in `bridge.health` method is always registered.

Usage:
    class MyBridge(BridgeServer):
        def __init__(self):
            super().__init__("my-bridge")
            self.register("my.method", self.handle_my_method)

        def handle_my_method(self, params):
            return {"ok": True}

    if __name__ == "__main__":
        MyBridge().run()
"""

from __future__ import annotations

import json
import sys
import traceback
from typing import Any, Callable

ERROR_METHOD_NOT_FOUND = -32601
ERROR_INTERNAL = -32603
ERROR_TIMEOUT = -32000
ERROR_TRANSPORT = -32001


class BridgeServer:
    """Base class for Simard bridge servers."""

    def __init__(self, server_name: str) -> None:
        self.server_name = server_name
        self._handlers: dict[str, Callable[[dict[str, Any]], Any]] = {}
        self.register("bridge.health", self._handle_health)

    def register(
        self, method: str, handler: Callable[[dict[str, Any]], Any]
    ) -> None:
        """Register a handler for a method name."""
        self._handlers[method] = handler

    def _handle_health(self, _params: dict[str, Any]) -> dict[str, Any]:
        return {"server_name": self.server_name, "healthy": True}

    def dispatch(self, method: str, params: dict[str, Any]) -> Any:
        """Dispatch a method call to the registered handler."""
        handler = self._handlers.get(method)
        if handler is None:
            raise MethodNotFoundError(method)
        return handler(params)

    def run(self) -> None:
        """Read requests from stdin, dispatch, write responses to stdout."""
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                request = json.loads(line)
            except json.JSONDecodeError as exc:
                # Cannot recover request id; write a bare error.
                _write_response(
                    {"id": "unknown", "error": _error(ERROR_INTERNAL, str(exc))}
                )
                continue

            request_id = request.get("id", "unknown")
            method = request.get("method", "")
            params = request.get("params", {})

            try:
                result = self.dispatch(method, params)
                _write_response({"id": request_id, "result": result})
            except MethodNotFoundError:
                _write_response(
                    {
                        "id": request_id,
                        "error": _error(
                            ERROR_METHOD_NOT_FOUND,
                            f"method '{method}' is not registered",
                        ),
                    }
                )
            except Exception:
                _write_response(
                    {
                        "id": request_id,
                        "error": _error(ERROR_INTERNAL, traceback.format_exc()),
                    }
                )


class MethodNotFoundError(Exception):
    """Raised when a requested method is not registered."""

    def __init__(self, method: str) -> None:
        super().__init__(f"method '{method}' is not registered")
        self.method = method


def _error(code: int, message: str) -> dict[str, Any]:
    return {"code": code, "message": message}


def _write_response(response: dict[str, Any]) -> None:
    line = json.dumps(response, separators=(",", ":"))
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


# --- Standalone echo server for integration testing ---

class EchoBridgeServer(BridgeServer):
    """Minimal bridge that echoes params back as results, for testing."""

    def __init__(self) -> None:
        super().__init__("echo")
        self.register("echo", self._handle_echo)

    def _handle_echo(self, params: dict[str, Any]) -> dict[str, Any]:
        return params


if __name__ == "__main__":
    EchoBridgeServer().run()
