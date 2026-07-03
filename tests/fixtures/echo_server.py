"""Minimal echo adapter server for SubprocessServerTransport integration tests.

Reads newline-delimited JSON requests from stdin, dispatches to registered
handlers, and writes JSON responses to stdout. Used by tests/adapter.rs.
"""

from __future__ import annotations

import json
import sys
import traceback
from typing import Any

_REAL_STDOUT = sys.stdout

ERROR_METHOD_NOT_FOUND = -32601
ERROR_INTERNAL = -32603


def _error(code: int, message: str) -> dict[str, Any]:
    return {"code": code, "message": message}


def _write_response(response: dict[str, Any]) -> None:
    line = json.dumps(response, separators=(",", ":"))
    _REAL_STDOUT.write(line + "\n")
    _REAL_STDOUT.flush()


def run() -> None:
    sys.stdout = sys.stderr
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as exc:
            _write_response({"id": "unknown", "error": _error(ERROR_INTERNAL, str(exc))})
            continue

        request_id = request.get("id", "unknown")
        method = request.get("method", "")
        params = request.get("params", {})

        try:
            if method == "bridge.health":
                result = {"server_name": "echo", "healthy": True}
            elif method == "echo":
                result = params
            else:
                _write_response({
                    "id": request_id,
                    "error": _error(ERROR_METHOD_NOT_FOUND, f"method '{method}' is not registered"),
                })
                continue
            _write_response({"id": request_id, "result": result})
        except Exception:
            _write_response({
                "id": request_id,
                "error": _error(ERROR_INTERNAL, traceback.format_exc()),
            })


if __name__ == "__main__":
    run()
