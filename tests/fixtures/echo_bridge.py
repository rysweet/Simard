"""Minimal echo bridge server for SubprocessBridgeTransport integration tests.

Reads JSON-line requests from stdin, dispatches to handlers, writes JSON-line
responses to stdout. Only used as a test fixture — not production code.
"""
import json
import sys

_REAL_STDOUT = sys.stdout

def run():
    sys.stdout = sys.stderr
    handlers = {
        "bridge.health": lambda _: {"server_name": "echo", "healthy": True},
        "echo": lambda params: params,
    }
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as exc:
            resp = {"id": "unknown", "error": {"code": -32603, "message": str(exc)}}
            _REAL_STDOUT.write(json.dumps(resp, separators=(",", ":")) + "\n")
            _REAL_STDOUT.flush()
            continue

        rid = request.get("id", "unknown")
        method = request.get("method", "")
        params = request.get("params", {})

        handler = handlers.get(method)
        if handler is None:
            resp = {"id": rid, "error": {"code": -32601, "message": f"method '{method}' is not registered"}}
        else:
            try:
                resp = {"id": rid, "result": handler(params)}
            except Exception as e:
                resp = {"id": rid, "error": {"code": -32603, "message": str(e)}}

        _REAL_STDOUT.write(json.dumps(resp, separators=(",", ":")) + "\n")
        _REAL_STDOUT.flush()

if __name__ == "__main__":
    run()
