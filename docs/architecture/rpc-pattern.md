---
title: RPC Transport Pattern
description: How Simard's RPC transport infrastructure provides typed clients for knowledge and gym services, using native Rust transports with circuit breaker fault tolerance.
last_updated: 2026-07-06
owner: simard
doc_type: concept
---

# RPC Transport Pattern

Simard uses an **RPC transport abstraction** — typed `RpcTransport` implementations that speak a JSON-line protocol — to isolate client code from transport details.

## Transport Types

| Transport | Use Case | Notes |
|-----------|----------|-------|
| **NativeRpcTransport** | Production (knowledge, gym) | In-process Rust handlers, zero overhead |
| **SubprocessRpcTransport** | Testing infrastructure | Spawns a Python subprocess; used only in integration tests |
| **InMemoryRpcTransport** | Unit testing | In-memory mock; no I/O |

> **History**: Prior to #2181, the knowledge and gym clients used Python subprocess transports with a native Rust fallback. The native Rust transports are now the only production path. Cognitive memory is provided by the library-backed `LibraryCognitiveMemory` (over `amplihack-memory-lib`) as the sole on-disk backend after the de-fork (Phase 2b) — see [Cognitive Memory Architecture](cognitive-memory.md) and [Library-backed Cognitive Memory](cognitive-memory-library-adapter.md).

## Wire Protocol

Each transport speaks newline-delimited JSON. One request per line on stdin, one response per line on stdout.

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

> **Wire contract is frozen.** Only the Rust identifiers changed in the
> RPC-vs-Bridge rename; every JSON method name (`memory.store_fact`,
> `knowledge.query`, `gym.run_scenario`, the built-in `bridge.health` probe)
> and on-disk format is byte-for-byte unchanged. Renaming a struct or module
> never alters what goes on the wire.

## Rust-Side Architecture

### RpcTransport Trait

```rust
pub trait RpcTransport: Send + Sync {
    fn call(&self, request: RpcRequest) -> SimardResult<RpcResponse>;
    fn descriptor(&self) -> BackendDescriptor;
    fn health(&self) -> SimardResult<RpcHealth>;  // default implementation
}
```

### Implementations

| Type | Purpose |
|------|---------|
| `SubprocessRpcTransport` | Spawns Python, manages stdin/stdout, kills on drop |
| `InMemoryRpcTransport` | Handler function for unit tests, no Python needed |
| `CircuitBreakerTransport<T>` | Wraps any transport with fault tolerance |

These live in `src/rpc_transport/` (`in_memory`, `native`, `subprocess`
submodules); the shared request/response types (`RpcRequest`, `RpcResponse`,
`RpcHealth`, the `RpcTransport` trait) live in `src/rpc.rs`.

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
- **Open**: Calls rejected immediately with `RpcCircuitOpen`
- **Half-Open**: One probe call allowed; success closes, failure reopens

The circuit breaker lives in `src/rpc_circuit_breaker.rs`. Only
transport-level errors (code `-32001`) trip the circuit. Application errors
(method not found, internal) do not.

## Python-Side Architecture

The subprocess transport is only used by integration tests. The Python test
fixtures extend a small `RpcServer` base class that runs the stdin/stdout
loop and dispatches registered method handlers:

```python
class RpcServer:
    def __init__(self, server_name: str) -> None
    def register(self, method: str, handler: Callable) -> None
    def run(self) -> None  # stdin/stdout loop
```

A fixture server extends `RpcServer` and registers method handlers:

```python
class EchoRpcServer(RpcServer):
    def __init__(self):
        super().__init__("echo")
        self.register("echo", self.handle_echo)

    def handle_echo(self, params):
        return {"echoed": params}
```

The built-in `bridge.health` method is always registered and returns
`{"server_name": "...", "healthy": true}`. The method string stays
`bridge.health` for wire compatibility — it is part of the frozen protocol,
not a Rust identifier.

## Error Handling

### Simard-Side Errors

| Error Type | When | Recovery |
|-----------|------|----------|
| `RpcSpawnFailed` | Python binary not found | Check PATH, install python3 |
| `RpcTransportError` | Stdin/stdout broken, process exited | Circuit breaker opens, auto-respawn on next call |
| `RpcProtocolError` | Malformed JSON, type mismatch | Log and surface to operator |
| `RpcCallFailed` | Method returned error payload | Surface to caller with method context |
| `RpcCircuitOpen` | Too many recent failures | Wait for cooldown, check transport health |

### Data Loss Prevention

Cognitive-memory writes no longer flow through an `RpcTransport`. Since the
de-fork (Phase 2b, issue #2307) they go directly through the in-process
[`LibraryCognitiveMemory`](cognitive-memory-library-adapter.md) adapter over
`amplihack-memory-lib`:

- Writes are idempotent — each fact, episode, or procedure is keyed by its
  LadybugDB `node_id`, so a replayed write reinforces the existing node rather
  than duplicating it (the *upsert-that-reinforces* contract; see
  [Procedural Idempotency](../reference/cognitive-memory-procedural-idempotency.md)).
- Concurrent writers are serialized through a single-writer IPC guard
  (`memory_ipc::launcher::launch_writer_client`), so parallel Simard processes
  cannot interleave writes to the same store.
- Durability and recovery are provided by verified backups in the
  `memory_backup` module (`backup_memory_verified`, `verify_backup`,
  `restore_from_backup`) rather than by transport-level transaction replay —
  see [Verified Backups](../operations/verified-backups.md) and
  [Cognitive-Memory Durability](../operations/cognitive-memory-durability.md).

## Testing

### Unit Tests (no Python needed)

```rust
let transport = InMemoryRpcTransport::echo("test");
let response = transport.call(health_request()).unwrap();
assert!(response.result.is_some());
```

### Integration Tests (subprocess transport)

```rust
let transport = SubprocessRpcTransport::new(
    "echo-test",
    "tests/fixtures/echo_rpc.py",
    vec![],
    Duration::from_secs(5),
);
let health = transport.health().expect("transport should be healthy");
assert_eq!(health.server_name, "echo");
```

### Feral Tests

- Kill the subprocess mid-request → `RpcTransportError`
- Send malformed JSON → `RpcProtocolError`
- Fixture script doesn't exist → `RpcSpawnFailed` or `RpcTransportError`
- Subprocess exits immediately → EOF detection
- 3 consecutive transport failures → circuit opens
