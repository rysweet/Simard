---
title: Bounded RPC-Health Liveness via `simard memory ping`
description: The deploy-canary rpc-health gate probes the live memory daemon with a new O(1), lock-free `simard memory ping` liveness command instead of the full `simard memory stats` round-trip. The ping does a bare Ping/Pong socket handshake that never scans the graph and never contends with consolidation/write locks, so the canary greens promptly against a healthy daemon while preserving every fail-closed guarantee (absent socket, unconnectable socket, and hung daemon all still redden). Closes the self-deploy crash-loop where every canary reddened on 'rpc health timed out after 30s (memory stats did not return)'.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./simard-memory-cli.md
  - ./rpc-health-stats-snapshot-readiness.md
  - ./canary-gate-convergence.md
  - ./self-deploy-api.md
  - ./rpc-wire-protocol.md
  - ./state-root-resolution.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../safe-self-update.md
---

# Bounded RPC-Health Liveness via `simard memory ping`

The deploy-canary **rpc-health gate** (`deploy_gate rpc-health`) now proves the
live memory daemon is reachable with a new **liveness** command,
`simard memory ping`, instead of the heavyweight `simard memory stats`
round-trip. `memory ping` performs a bare, **lock-free `Ping → Pong` socket
handshake** that returns in O(1): it never scans the cognitive graph, never
takes the store lock, and never contends with consolidation or write locks.

This closes the systemic self-deploy crash-loop in which every canary reddened
on:

```
gate rpc-health: rpc health timed out after 30s (memory stats did not return)
```

On a large cognitive store (~30.8k nodes, ~109 MiB, RSS spiking to ~1.2 GiB,
disk at 93%), the old `memory stats` probe dialed the live `memory.sock`, ran a
full stats RPC through `dispatch_memory_command → run_stats →
open_reader_client`, and could not return inside the gate's 30s
`health_timeout`. `ProbeOutcome::TimedOut` reddened the canary on every Overseer
tick (observed on commit `953d5a9d407a` at 11:01, 11:39, 13:21, 15:00 on
2026-07-26), the running daemon stayed pinned at `0.38.0`, and `DeployDrift`
grew from 2 to 3 commits behind merged `main` because no canary could ever land.

`memory ping` removes the graph scan and lock contention from the liveness path
while keeping the gate **fail-closed**: an absent socket, an unconnectable
socket, and a hung daemon each still redden the canary exactly as before.

> **Modules:**
> `src/operator_cli/memory.rs` (`run_ping` handler + `ping` dispatch arm + help),
> `src/self_relaunch/gates.rs` (`RPC_HEALTH_PROBE_ARGS = ["memory", "ping"]`,
> `run_rpc_health_gate`).
> **Reused verbatim (no wire change):**
> `MemoryRequest::Ping` / `MemoryResponse::Pong`
> (`src/memory_ipc/mod.rs`, served lock-free at `src/memory_ipc/server.rs`),
> `RemoteCognitiveMemory::connect` (`src/memory_ipc/client.rs`, whose connect
> already performs the `Ping/Pong` handshake).

## Why

`simard memory stats` is a genuine, useful introspection command — it prints
per-type counts, a graph-edge / dedup section, and sample rows. Producing that
output requires folding the whole store into a `CognitiveStatistics` report and
touching the graph, which is exactly what stalls under lock/scan pressure on a
large store. Using it as a *liveness* probe conflated two jobs:

1. **Liveness** — "is the daemon's dispatch thread alive and is the socket
   round-tripping?" This must be **cheap and bounded**.
2. **Introspection** — "what is in the store right now?" This is inherently
   proportional to store size.

`memory ping` does job #1 only. The already-existing `Ping/Pong` branch
(`server.rs`: `MemoryRequest::Ping => MemoryResponse::Pong`) answers on the
daemon's dispatch thread **without touching the store**, so it proves the same
reachability signal the gate needs — the socket exists, the daemon is accepting,
and a request completes a full round-trip — with none of the stats cost.

The fix is **additive and non-breaking**:

- `simard memory stats` is **unchanged, byte-for-byte** — same output, same
  routing, same behaviour for operators and scripts.
- No IPC wire type is added or changed. `memory ping` reuses the existing
  `Ping/Pong` frames.
- `health_timeout` stays at its **30s default** — the round-trip is now bounded
  by construction, so the timeout is a backstop, not the everyday path.

## What changed at a glance

| Surface | Before | After |
| --- | --- | --- |
| rpc-health probe args | `["memory", "stats"]` | `["memory", "ping"]` |
| Probe cost | Full stats report: graph scan + store lock + fold to `CognitiveStatistics` | O(1) `Ping/Pong` handshake; **no scan, no store lock** |
| Probe against healthy large store | `TimedOut` at 30s → **red** | Returns in milliseconds → **green** |
| `simard memory stats` | (as-is) | **unchanged, byte-for-byte** |
| `memory ping` command | did not exist | new liveness subcommand |
| IPC wire (`MemoryRequest::Ping` / `MemoryResponse::Pong`) | exists, lock-free | **reused verbatim** — no wire/version bump |
| `health_timeout` | 30s | **unchanged** (backstop) |
| `default_gates()` | `{smoke, unit-test, gym-baseline, rpc-health}` | **unchanged** |
| Fail-closed on absent / unconnectable / hung | red | **red (preserved)** |

## The `simard memory ping` command

`simard memory ping` is a **liveness check**, not an introspection command. It
answers a single yes/no question — *is the live memory daemon reachable and
round-tripping right now?* — via exit status, with no store output.

### Synopsis

```
simard memory ping [state-root]
```

- `state-root` (optional) — the Simard state root whose `memory.sock` to dial.
  Resolved with the same precedence as the rest of the `memory` surface: the
  explicit argument, else `$SIMARD_STATE_ROOT`, else `$HOME/.simard` (see
  [State-root resolution](./state-root-resolution.md)). This keeps the canary
  and the live daemon pointed at the **same** socket (hermeticity, #1967).

### Behaviour

1. Resolve the state root and the socket path (`socket_path_for(state_root)`).
2. Require the socket to be **present** (pre-flight `exists()` check). An absent
   socket is a fail-closed liveness failure, not a "try something else" signal.
3. Connect **directly** with `RemoteCognitiveMemory::connect(&sock)`, which
   performs the existing `Ping → Pong` handshake. On `Pong`, the daemon is live.
4. Exit `0` on `Pong`; exit **non-zero** on any resolution, connection, or
   round-trip failure.

`memory ping` **never** calls `build_report` / `run_stats`, **never** opens the
store, and **never** falls back to a direct on-disk store open. It is a pure
socket liveness probe.

> **No tier-2 fallback (this is the security-critical property).** Unlike the
> read-only introspection commands, `memory ping` does **not** go through
> `open_reader_client`, whose "daemon down → open the on-disk store directly"
> tier-2 path would let a *dead daemon* appear healthy. `memory ping` calls
> `RemoteCognitiveMemory::connect` directly; if the daemon is not answering on
> the socket, the command fails. Greening a dead daemon would re-open the exact
> crash-loop this change fixes and would violate the gate's fail-closed contract
> (#2896 / #4639).

### Exit codes

| Exit | Meaning | Gate outcome |
| --- | --- | --- |
| `0` | `Pong` received — daemon live and round-tripping | **green** |
| non-zero | socket absent, unconnectable, handshake failed, or bad args | **red** |
| non-zero (bounded ≤30s) | daemon connected but hung mid-handshake — the client's own 30s stream read timeout fires, `connect` returns `RpcSpawnFailed` | **red** |
| (never returns) | client read timeout somehow does not fire | **red** via gate `TimedOut` at `health_timeout` (backstop) |

`memory ping` produces **no stdout report** (no `print!`/`println!`); success is
conveyed by exit `0`. Diagnostics are emitted as structured `tracing` + OTel
only (see [Observability](#observability)).

### Examples

Probe the default daemon:

```console
$ simard memory ping
$ echo $?
0
```

Probe a specific state root (as the canary does, with `$SIMARD_STATE_ROOT`
re-injected into its scrubbed environment):

```console
$ SIMARD_STATE_ROOT=/var/lib/simard simard memory ping
$ echo $?
0
```

Daemon down / socket absent — fails closed:

```console
$ simard memory ping /tmp/no-such-root
$ echo $?
1
```

Contrast with introspection (unchanged): use `simard memory stats` when you
want the actual per-type counts, not a liveness verdict:

```console
$ simard memory stats
sensory:     0
working:     12
episodic:    418
semantic:    1203
procedural:  57
prospective: 9
```

## The rpc-health gate

`RPC_HEALTH_PROBE_ARGS` in `src/self_relaunch/gates.rs` is repointed from
`["memory", "stats"]` to `["memory", "ping"]`. `run_rpc_health_gate` is
otherwise structurally unchanged: it pre-flights the socket, runs the candidate
binary's probe argv under `run_probe_with_timeout(cmd, health_timeout)`, and
maps the `ProbeOutcome` to a `GateResult`.

Because the probe is now O(1), a healthy daemon returns well inside the 30s
window and the gate greens on the first attempt.

### Fail-closed guarantees (all preserved)

The gate still reddens in every unhealthy case. The mechanism is unchanged; only
the *cost of the healthy path* dropped.

| Condition | Mechanism | Outcome |
| --- | --- | --- |
| **Absent socket** | `probe_socket_path().exists()` pre-flight fails (also enforced inside `memory ping`) | immediate **red** |
| **Unconnectable socket** (present but not accepting) | `RemoteCognitiveMemory::connect` → `SimardError::RpcSpawnFailed` → non-zero exit (`ProbeOutcome::Exited` non-success) | **red** |
| **Hung daemon** (connected, `Ping` never answered) | `RemoteCognitiveMemory::connect` sets a **30s stream read timeout** (`client.rs`), so the handshake read errors out → `RpcSpawnFailed` → non-zero exit. If that ever fails to fire, `run_probe_with_timeout` at `health_timeout` is the outer backstop → `ProbeOutcome::TimedOut` | **red** (two independent 30s timers) |
| **Healthy live daemon** | `Pong` → exit `0` → `ProbeOutcome::Exited { success }` | **green** |

This is the reachability signal the gate must assert (#2896, #4639 F2): the
candidate must prove it can round-trip against the **running** daemon, never
green by silently falling through to a tier-2 on-disk store.

## API surface

### Operator CLI

```
simard memory ping [state-root]
```

Handled by `run_ping` in `src/operator_cli/memory.rs`, dispatched from the
`memory` subcommand match alongside `stats` / `dump` / `import`:

```rust
match subcommand.as_str() {
    // ...
    "stats" => run_stats(args),   // unchanged
    "ping"  => run_ping(args),    // new: O(1) liveness
    // ...
}
```

`run_ping` resolves the state root (`resolve_state_root`), derives the socket
(`socket_path_for`), requires the socket to exist, calls
`RemoteCognitiveMemory::connect` directly (which performs the `Ping/Pong`
handshake), returns `Ok(())` on `Pong`, and returns an `Err` (non-zero exit) on
any failure. It rejects unknown flags / surplus arguments with a non-zero exit.

### IPC wire (reused, unchanged)

`memory ping` rides the **existing** liveness frames — no new operation, no
version bump, full mixed-revision compatibility:

```rust
// request
MemoryRequest::Ping
// response, served lock-free on the daemon dispatch thread:
//   MemoryRequest::Ping => MemoryResponse::Pong
MemoryResponse::Pong
```

The response is bounded by the existing `MAX_FRAME` (8 MiB) frame cap, and only
`Pong` is accepted — any other variant is a handshake failure. See
[RPC Wire Protocol](./rpc-wire-protocol.md).

## Configuration

There are **no new configuration knobs** and **no changed default values**.

| Setting | Location | Value | Notes |
| --- | --- | --- | --- |
| `RPC_HEALTH_PROBE_ARGS` | `src/self_relaunch/gates.rs` | `["memory", "ping"]` (hardcoded) | Repointed from `["memory","stats"]`. Never interpolated from env / untrusted input — prevents command injection. |
| `health_timeout` | `RelaunchConfig` | 30s (**unchanged**) | Backstop for a hung daemon. The healthy path now returns far inside it. |
| `SIMARD_STATE_ROOT` | canary env allow-list | unchanged | Re-injected so the candidate resolves the **same** `memory.sock` as the live daemon. |
| `default_gates()` | `src/self_relaunch/gates.rs` | `{smoke, unit-test, gym-baseline, rpc-health}` (**unchanged**) | The default gate set is untouched. |

## Observability

All new code uses **structured `tracing` + OpenTelemetry spans only** — no
`print!` / `println!` / `eprintln!` in the liveness path, and **no silent
fallbacks**. Every failure surfaces as a non-zero exit with a traced error.

Representative signals:

- **Ping attempt** — a span recording the resolved socket path and the outcome
  (`pong` / `connect-failed` / `absent` / `bad-args`). Never logs frame bytes or
  environment contents.
- **Fail-closed** — an absent/unconnectable socket is traced as an error event
  before the non-zero exit, so a reddened canary is never silent.
- **Gate mapping** — `run_rpc_health_gate` continues to trace the `ProbeOutcome`
  it maps to the `GateResult` (green / non-success exit / `TimedOut`).

Because `Ping/Pong` carries no graph data, the liveness path exposes **no PII,
no secrets, and no memory contents** — a net reduction in the data surface
versus the previous stats probe.

See [Telemetry & Metrics](./telemetry-metrics.md) and
[Overseer Deploy Red-Canary Diagnostics](./overseer-deploy-canary-diagnostics.md).

## Security considerations

- **Fail-open is impossible (the primary control).** `memory ping` bypasses the
  tier-2 on-disk store open entirely by calling `RemoteCognitiveMemory::connect`
  directly. An absent socket, an unconnectable socket, or a hung daemon each
  reddens (`exists()` pre-flight / `RpcSpawnFailed` non-zero exit / client 30s
  read timeout, with the gate `TimedOut` as an outer backstop). Regression tests
  on all three modes are the enforcement.
- **Trust boundary unchanged.** `AF_UNIX` local socket only, owner-only
  permissions (parent dir `0o700`, socket `0o600`); identity is the UID. `ping`
  only *connects* — it never binds, `chmod`s, or changes privilege. The change
  **reduces** attack surface versus `memory stats`.
- **Untrusted response.** The `Pong` reply is treated as untrusted input: only
  `MemoryResponse::Pong` is accepted, and `read_frame`'s `MAX_FRAME` (8 MiB) cap
  bounds allocation.
- **No command injection.** `RPC_HEALTH_PROBE_ARGS` stays a hardcoded constant;
  env / untrusted args are never interpolated into the probe argv.
- **Hermeticity preserved.** The state root is resolved only via the existing
  `resolve_state_root` / `socket_path_for` helpers (#1967). The request carries
  no user payload.
- **No PII / secrets in telemetry.** `Ping/Pong` carries no graph data; only the
  socket path and an enum outcome appear in traces.

## Testing

Regression tests prove both the green path and every fail-closed path, using the
hermetic live-socket harness (`state_root_with_live_socket` / `unique_tmp`, one
unique tmp state root per test so no stale/hung daemon leaks between tests):

- **Green (bounded):** the probe returns success **within `health_timeout`**
  against a hermetic healthy daemon (`spawn_server`) — the round-trip completes
  far inside the 30s budget.
- **Red — hung daemon:** a connected-but-unresponsive daemon reddens via the
  client's 30s stream read timeout (`RpcSpawnFailed`), with the gate's
  `ProbeOutcome::TimedOut` as the outer backstop (fail-closed). Note: because
  both timers are 30s, an honest hung-daemon test is slow; prefer a fast fake
  (e.g. a socket that accepts then never writes, with a shortened per-test
  timeout) over a real 30s wait.
- **Red — absent socket:** a missing `memory.sock` fails the pre-flight and
  `memory ping` exits non-zero (fail-closed); it never greens via a tier-2
  on-disk open.
- **Red — unconnectable socket:** a present-but-not-accepting socket yields
  `RpcSpawnFailed` → non-zero exit (fail-closed).
- **Dispatch resolution (R5):** `RPC_HEALTH_PROBE_ARGS = ["memory", "ping"]`
  resolves to a **dispatched** subcommand (not the `unsupported command`
  regression), and greens against a live daemon.
- **`memory stats` untouched:** existing `stats` / `dump` / `import` tests pass
  unchanged, confirming byte-for-byte output preservation.

The frozen fail-closed regression tests in
`src/memory_ipc/tests_launcher_fail_closed_2896.rs` keep their original fast
timing and continue to pass.

## Related

- [Memory introspection CLI](./simard-memory-cli.md) — `stats` / `dump` /
  `import` (unchanged).
- [RPC-Health Readiness & Non-Blocking Statistics Snapshot](./rpc-health-stats-snapshot-readiness.md)
- [Canary Gate Isolation & Self-Deploy Convergence](./canary-gate-convergence.md)
- [Self-Deploy API](./self-deploy-api.md)
- [RPC Wire Protocol](./rpc-wire-protocol.md)
- [State-root resolution](./state-root-resolution.md)
- [Safe Self-Update](../safe-self-update.md)
