---
title: Bridge Pattern
description: How Simard communicates with the Python ecosystem through subprocess bridges with JSON-line protocol and circuit breaker fault tolerance.
last_updated: 2026-03-31
owner: simard
doc_type: concept
---

# Bridge Pattern

Simard is Rust. The amplihack ecosystem (memory-lib, kg-packs, agent-eval) is Python. Rather than rewrite the ecosystem in Rust or use FFI, Simard communicates through **subprocess bridges** — Python processes that speak a simple JSON-line protocol on stdin/stdout.

## Why Bridges?

| Approach | Pros | Cons |
|----------|------|------|
| **PyO3 FFI** | Zero-copy, native speed | Tight coupling, GIL contention, complex build |
| **HTTP/gRPC** | Standard, debuggable | Server lifecycle, port management, overhead |
| **Subprocess bridges** | Simple, isolated, no dependencies | Serialization overhead, process lifecycle |

Bridges win because:
- The Python ecosystem already works — we don't want to port 3,000+ LOC of production Python code
- LadybugDB has Python bindings but no Rust bindings
- Process isolation means a Python crash can't take down Simard
- The circuit breaker pattern handles intermittent failures gracefully

## Wire Protocol

Each bridge speaks newline-delimited JSON. One request per line on stdin, one response per line on stdout.

### Request Format

```json
{"id": "01970b2f-...", "method": "memory.store_fact", "params": {"concept": "cargo test", "content": "runs all workspace tests", "confidence": 0.9}}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | UUIDv7 for request-response matching |
| `method` | string | yes | Dotted method name (e.g., `memory.store_fact`) |
| `params` | object | yes | Method-specific parameters |

### Response Format (success)

```json
{"id": "01970b2f-...", "result": {"fact_id": "sem_01abc..."}}
```

### Response Format (error)

```json
{"id": "01970b2f-...", "error": {"code": -32601, "message": "method 'nonexistent' is not registered"}}
```

| Error Code | Meaning |
|-----------|---------|
| `-32601` | Method not found |
| `-32603` | Internal server error |
| `-32000` | Timeout |
| `-32001` | Transport error |

## Rust-Side Architecture

### BridgeTransport Trait

```rust
pub trait BridgeTransport: Send + Sync {
    fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse>;
    fn descriptor(&self) -> BackendDescriptor;
    fn health(&self) -> SimardResult<BridgeHealth>;  // default implementation
}
```

### Implementations

| Type | Purpose |
|------|---------|
| `SubprocessBridgeTransport` | Spawns Python, manages stdin/stdout, kills on drop |
| `InMemoryBridgeTransport` | Handler function for unit tests, no Python needed |
| `CircuitBreakerTransport<T>` | Wraps any transport with fault tolerance |

### Circuit Breaker

```mermaid
stateDiagram-v2
    [*] --> Closed
    Closed --> Open: 3 consecutive failures
    Open --> HalfOpen: cooldown elapsed (30s)
    HalfOpen --> Closed: probe succeeds
    HalfOpen --> Open: probe fails
```

- **Closed**: Normal operation, calls pass through
- **Open**: Calls rejected immediately with `BridgeCircuitOpen`
- **Half-Open**: One probe call allowed; success closes, failure reopens

Only transport-level errors (code `-32001`) trip the circuit. Application errors (method not found, internal) do not.

## Python-Side Architecture

### BridgeServer Base Class

```python
class BridgeServer:
    def __init__(self, server_name: str) -> None
    def register(self, method: str, handler: Callable) -> None
    def run(self) -> None  # stdin/stdout loop
```

Each bridge server extends `BridgeServer` and registers method handlers:

```python
class SimardMemoryBridge(BridgeServer):
    def __init__(self, agent_name, db_path):
        super().__init__("simard-memory")
        self.adapter = CognitiveAdapter(agent_name, db_path)
        self.register("memory.store_fact", self.handle_store_fact)
        # ... register all memory methods

    def handle_store_fact(self, params):
        fact_id = self.adapter.store_fact(
            context=params["concept"],
            fact=params["content"],
            confidence=params.get("confidence", 0.9),
        )
        return {"fact_id": fact_id}
```

The built-in `bridge.health` method is always registered and returns `{"server_name": "...", "healthy": true}`.

## Error Handling

### Simard-Side Errors

| Error Type | When | Recovery |
|-----------|------|----------|
| `BridgeSpawnFailed` | Python binary not found | Check PATH, install python3 |
| `BridgeTransportError` | Stdin/stdout broken, process exited | Circuit breaker opens, auto-respawn on next call |
| `BridgeProtocolError` | Malformed JSON, type mismatch | Log and surface to operator |
| `BridgeCallFailed` | Method returned error payload | Surface to caller with method context |
| `BridgeCircuitOpen` | Too many recent failures | Wait for cooldown, check bridge health |

### Data Loss Prevention

- Memory writes are idempotent (LadybugDB `node_id` is primary key)
- Python bridge wraps each write in a LadybugDB transaction
- If bridge dies mid-write, the transaction rolls back
- On reconnect, Simard re-issues the last failed write

## Testing

### Unit Tests (no Python needed)

```rust
let transport = InMemoryBridgeTransport::echo("test");
let response = transport.call(health_request()).unwrap();
assert!(response.result.is_some());
```

### Integration Tests (real Python subprocess)

```rust
let transport = SubprocessBridgeTransport::new(
    "echo-test",
    "python/bridge_server.py",
    vec![],
    Duration::from_secs(5),
);
let health = transport.health().expect("bridge should be healthy");
assert_eq!(health.server_name, "echo");
```

### Feral Tests

- Kill bridge mid-request → `BridgeTransportError`
- Send malformed JSON → `BridgeProtocolError`
- Bridge script doesn't exist → `BridgeSpawnFailed` or `BridgeTransportError`
- Bridge exits immediately → EOF detection
- 3 consecutive transport failures → circuit opens
