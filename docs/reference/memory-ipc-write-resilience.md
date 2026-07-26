---
title: memory-ipc Write-Path Resilience (EPIPE reconnect + retry)
description: How the RemoteCognitiveMemory client survives a broken-pipe / EPIPE mid-write against the daemon's memory socket by reconnecting and idempotently re-sending the framed request, with a strict fail-closed guarantee that no memory write is ever silently dropped.
last_updated: 2026-07-26
owner: cognitive-memory
doc_type: reference
related:
  - ./rpc-wire-protocol.md
  - ../memory.md
  - ../architecture/cognitive-memory.md
  - ../architecture/distillation-semantic-handoff.md
---

# memory-ipc Write-Path Resilience (EPIPE reconnect + retry)

> Shipped in issue [#4731](https://github.com/rysweet/Simard/issues/4731).

The [`RemoteCognitiveMemory`](./rpc-wire-protocol.md) client — the wire that
`meeting`, `engineer`, and the distiller subprocess use to reach the OODA
daemon's cognitive memory over `{socket_dir}/memory.sock` — is **resilient to a
peer that closes the socket mid-write**. When the daemon drops the connection
while the client is sending a request frame (surfacing as
`write-len: Broken pipe (os error 32)`), the client transparently reconnects
and re-sends the request instead of failing the memory write.

This closes a recurring, systemic process-health defect in which large
distillation payloads (e.g. a 32-episode consolidation) and OODA cycle
transitions produced clusters of identical
`memory-ipc: connection error: rpc endpoint memory-ipc transport error:
write-len: Broken pipe (os error 32)` failures under load.

## Guarantee: fail-closed, never silent

The overriding contract is **no memory write may be silently dropped on EPIPE**:

- Either the framed request **durably succeeds** after an automatic reconnect
  and retry, **or**
- the error is **surfaced** to the caller as
  `SimardError::RpcTransportError { endpoint: "memory-ipc", .. }`.

There is **no silent fallback**, no best-effort drop, and no alternate
transport. A caller that receives `Ok(..)` can trust the write reached the
daemon's authoritative write boundary; a caller that receives `Err(..)` knows
the write did **not** commit and can decide how to react.

## Scope of the retry

Retries apply to the **write half only** — the phase where the client is
sending the request and the daemon has not yet committed anything:

| Frame phase | Retried on EPIPE? | Why |
|-------------|-------------------|-----|
| `write-len` (length prefix) | ✅ yes | Server has not received or committed the request. |
| `write-body` (JSON payload) | ✅ yes | Server has not fully received the request; no commit. |
| `flush` | ✅ yes | Request not yet delivered. |
| `read-len` / `read-body` (response) | ❌ **never** | The server may have **already persisted** the mutation; a blind resend could duplicate it. |

Read-half failures are propagated immediately as `RpcTransportError` and are
**not** retried. This deliberately sidesteps the double-apply hazard for a
mutation the daemon may already have committed.

## EPIPE detection

A failure is treated as a broken pipe when the underlying `std::io::Error`
satisfies **either** condition, evaluated on the raw error **before** it is
stringified into a `SimardError`:

- `error.kind() == std::io::ErrorKind::BrokenPipe`, **or**
- `error.raw_os_error() == Some(32)` (Linux `EPIPE`).

This reuses the `raw_os_error()` classification technique already established in
`operator_commands_ooda/daemon/helpers.rs` (which applies the same pattern to
`EMFILE`/`ENFILE`), extended here to the broken-pipe errno.

## Retry bounds

The retry loop is bounded by compile-time constants — there are **no
environment variables or config knobs** to amplify it (a DoS-hardening
requirement):

| Constant | Value | Meaning |
|----------|-------|---------|
| `MAX_ATTEMPTS` | `3` | Total send attempts, including the first. |
| `BACKOFF` | `50 ms` | Fixed sleep between attempts. |

On exhaustion (all 3 attempts hit a write-half EPIPE), the client returns
`SimardError::RpcTransportError` — it never returns `Ok`.

Each attempt is still governed by the existing **30-second read/write socket
timeouts**, which are re-applied to every reconnected stream.

## Reconnect behavior

On a retryable write-half EPIPE the client:

1. Emits a `warn` tracing event (reconnecting; carries `attempt`,
   `endpoint = "memory-ipc"`, `socket_path`, and the `ErrorKind`/errno — **never
   payload bytes**).
2. Opens a **fresh** connection to the **stored, immutable `socket_path`** — it
   never re-derives, creates, `chmod`s, or falls back to a more permissive
   socket (TOCTOU / socket-redirection defense).
3. Re-applies the 30-second read/write timeouts.
4. Performs an **inline `Ping`/`Pong` handshake** over the new stream and
   verifies the response is **exactly `Pong`**. If the handshake returns
   anything else (or fails), the reconnect is abandoned and the call returns
   `RpcTransportError` **without resending the real payload**.
5. Swaps the new stream into the held `Mutex` guard (`*guard = new_stream`), so
   the stale stream is dropped deterministically and concurrent callers observe
   the healed connection.
6. Sleeps `BACKOFF`, then re-sends the original serialized request.

The request is serialized **once** before the loop and the `Mutex` guard is
held **across** the reconnect + backoff, so concurrent memory writes on the
same client are serialized and never reordered.

### Non-recursive reconnect

`connect()` originally performed its `Ping` handshake through `call()`, which is
the very method that now reconnects. To avoid a `connect → call → reconnect →
call` recursion, reconnect uses a lower-level `connect_stream(path)` primitive
(connect + apply timeouts, **no** handshake) plus an **inline** `Ping`/`Pong`
exchange on the new stream. Both `connect()` and `reconnect()` share this
primitive.

## Observability

All new code paths emit **structured `tracing`** events (plus the existing
OTel wiring). There are **no** `print!` / `println!` / `eprintln!` calls in the
resilience path.

| Event | Level | Fields |
|-------|-------|--------|
| Reconnecting after write-half EPIPE | `warn` | `attempt`, `endpoint`, `socket_path`, `error_kind`/`errno` |
| Retry attempt about to re-send | `debug` | `attempt`, `endpoint` |
| Terminal failure (attempts exhausted / bad handshake) | `error` | `attempt`, `endpoint`, `socket_path`, `error_kind`/`errno` |

**Log confidentiality:** tracing events and the `RpcTransportError` message
carry only the attempt number, `endpoint = "memory-ipc"`, `errno`/`ErrorKind`,
and `socket_path`. They **never** include request payload, episode, or
distillation bytes.

An optional reconnect/retry counter is incremented via
`cognitive_memory::metrics::increment(kind, site)` for dashboards. `increment`
and `cognitive_memory_silent_drop_count` read and write the **same**
`(kind, site)` bucket map, so the resilience path must choose `kind` labels that
keep the accounting semantically clean:

- A successful reconnect + retry is a *recovery*, not a drop — it uses a
  distinct recovery `kind` (e.g. `"epipe_reconnect"`) so a healed write is
  **never** miscounted as lost data.
- Exhausting all attempts fails **closed** as a *surfaced* `RpcTransportError`.
  That is a loud, observable failure — **not** a silent drop — so it must **not**
  reuse the silent-drop `kind` either; if it is counted at all it uses its own
  `kind` (e.g. `"epipe_exhausted"`).

The `cognitive_memory_silent_drop_count` `kind` stays reserved for genuine
silent-drop accounting and is inflated by neither EPIPE recovery nor a loudly
surfaced exhaustion.

## What did *not* change

This fix is **additive and non-breaking**:

- **No wire-format change.** The framing is still a 4-byte big-endian length
  prefix followed by JSON. A single logical request is still exactly one frame;
  the reconnected request is byte-identical to the original. Mixed old/new
  fleets interoperate.
- **No chunking.** Splitting one request across multiple frames was rejected
  because it would break the single-frame `read_frame` / `serve_connection`
  reader. The existing **8 MiB `MAX_FRAME`** cap already accommodates a
  32-episode distillation payload, so no oversized-frame handling is needed.
  `MAX_FRAME` is re-enforced on **every** read, including the post-reconnect
  response read.
- **No public-API change.** `RemoteCognitiveMemory::connect`, `call`, and all
  `CognitiveMemoryOps` methods keep their signatures. Resilience is internal to
  `call()`.
- **`runtime_ipc` is untouched.** `src/runtime_ipc/mod.rs` is a separate
  subprocess transport and is out of scope.

## Affected code

| File | Change |
|------|--------|
| `src/memory_ipc/mod.rs` | Add `write_frame_raw()` (returns `io::Result<()>` without `ipc_err()` stringification) and `is_broken_pipe(&io::Error) -> bool`; `write_frame` becomes a thin wrapper. |
| `src/memory_ipc/client.rs` | Primary fix: `connect_stream()` reconnect primitive, `reconnect()` with inline `Ping`/`Pong`, and the bounded write-half retry loop inside `call()`. |
| `src/memory_ipc/server.rs` | Verify-only: the accept/write loop already writes whole frames and is not the drop source; no functional change. |

## Example: transparent recovery

From a caller's perspective, nothing changes — the write just succeeds even if
the daemon briefly resets the connection mid-frame:

```rust
let mem = RemoteCognitiveMemory::connect(&socket_path)?;

// A large distillation write. If the daemon closes the socket mid-write
// (EPIPE), the client reconnects, re-handshakes, and re-sends automatically.
// This returns Ok only if the write durably reached the daemon.
let outcome = mem.remember_fact_gated(
    concept, content, confidence, &tags, source_id, &source_episode_ids, pass_id,
)?;
```

If every one of the 3 attempts hits an EPIPE, the call fails loudly instead of
dropping the write:

```text
rpc endpoint 'memory-ipc' transport error: write-len: Broken pipe (os error 32)
```

The caller gets an `Err(SimardError::RpcTransportError { .. })` and can retry at
a higher level, back off, or surface the failure — but the write is **never**
silently lost.

## Regression coverage

`src/memory_ipc/tests_epipe_resilience_4731.rs` uses a raw `UnixListener`
"malicious server" harness (based on the `tests_transport_roundtrip.rs` pattern)
to force mid-write resets:

1. **Mid-write reset → durable delivery.** A server that reads a partial frame
   then closes the socket forces one EPIPE; the client reconnects and re-sends,
   and the payload is delivered intact on the second, healthy connection — no
   data loss.
2. **Always-reset → surfaced error.** A server that always resets exhausts the
   3-attempt bound and the client returns `RpcTransportError` within the
   attempt/timeout budget — never a silent `Ok`.
3. **Non-`Pong` reconnect → surfaced error, no resend.** If the post-reconnect
   handshake returns anything other than `Pong`, the client aborts with
   `RpcTransportError` and does **not** resend the real payload.
4. **Log confidentiality.** Emitted logs and the error message contain no
   request/payload bytes.
