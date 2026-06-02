---
title: Bridge Pattern
description: How Simard communicates with ecosystem services through in-process native Rust transports and the SubprocessBridgeTransport fallback, with JSON-line protocol and circuit breaker fault tolerance.
last_updated: 2026-06-02
owner: simard
doc_type: concept
---

# Bridge Pattern

Simard uses **native Rust transports** (`NativeBridgeTransport`) for in-process bridge communication with the knowledge and gym subsystems. The original Python subprocess bridges were removed in #2181 after the native Rust transport layer was merged in #2172.

The `SubprocessBridgeTransport` type still exists for cases where external JSON-line subprocess communication is needed, but the production knowledge and gym bridges are now fully native Rust.

## Why Bridges?

| Approach | Pros | Cons |
|----------|------|------|
| **Native Rust** | Zero overhead, no process lifecycle | Requires Rust implementation |
| **PyO3 FFI** | Zero-copy, native speed | Tight coupling, GIL contention, complex build |
| **HTTP/gRPC** | Standard, debuggable | Server lifecycle, port management, overhead |
| **Subprocess bridges** | Simple, isolated, no dependencies | Serialization overhead, process lifecycle |

Native Rust transports win because:
- They eliminate Python process startup latency
- No external Python dependencies required
- Direct in-process function dispatch via the same JSON-line protocol

> **Note**: The memory bridge was replaced by a native Rust implementation (`NativeCognitiveMemory`) in an earlier milestone. The knowledge and gym bridges followed suit in #2172/#2181.

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
- Native transports execute within a single process, eliminating cross-process data loss
- On failure, Simard re-issues the last failed write

## Testing

### Unit Tests (no Python needed)

```rust
let transport = InMemoryBridgeTransport::echo("test");
let response = transport.call(health_request()).unwrap();
assert!(response.result.is_some());
```

### Integration Tests (SubprocessBridgeTransport)

```rust
// Uses an inline echo bridge script as a fixture (see tests/bridge.rs)
let transport = SubprocessBridgeTransport::new(
    "echo-test",
    &echo_script_path,
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
