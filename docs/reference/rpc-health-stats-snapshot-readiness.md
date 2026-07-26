---
title: RPC-Health Readiness & Non-Blocking Statistics Snapshot
description: How the memory-stats/telemetry RPC returns promptly during canary startup via an atomically-maintained CognitiveStatistics snapshot, how the deploy canary rpc-health gate waits for readiness with a bounded retry inside its unchanged 30s window, and how the daemon Resources CPU% stops rendering 'absent' via defensive /proc sampling. Closes the self-deploy failure loop where every canary reddened on 'rpc health timed out after 30s (memory stats did not return)'.
last_updated: 2026-07-26
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ./status-snapshot-api.md
  - ./canary-gate-convergence.md
  - ./overseer-deploy-canary-diagnostics.md
  - ./self-deploy-api.md
  - ./rpc-wire-protocol.md
  - ./telemetry-metrics.md
  - ../architecture/cognitive-memory-library-adapter.md
  - ../safe-self-update.md
---

# RPC-Health Readiness & Non-Blocking Statistics Snapshot

The memory-stats / telemetry RPC (`simard memory stats`, wire op
`MemoryRequest::GetStatistics`) now answers **promptly during daemon
startup**, even while the daemon is doing heavy initialisation work under the
global `Mutex<CognitiveMemory>`. It does this by serving an
atomically-maintained **`CognitiveStatistics` snapshot** on the read path
instead of taking the heavy mutex. The deploy canary's **rpc-health gate**
(`deploy_gate rpc-health`) waits for that populated response with a
**bounded, fail-closed retry inside its existing 30s window**.

Together these close the self-deploy failure loop in which every recent canary
reddened on:

```
rpc health timed out after 30s (memory stats did not return)
```

and `DeployDrift` grew unbounded (2 → 3 commits behind `main` across
consecutive overseer ticks) because no canary could ever land.

> **Modules:**
> `src/cognitive_memory/library_adapter.rs` (snapshot + `get_statistics`),
> `src/cognitive_memory/mod.rs` (`refresh_stats_snapshot` trait method),
> `src/self_relaunch/gates.rs` (`run_rpc_health_gate` readiness retry),
> `src/operator_commands_ooda/daemon/mod.rs` (snapshot priming + refresher),
> `src/status/provider.rs` (`/proc` CPU sampling in `assemble_resources`).
> **Wire type:** `CognitiveStatistics` in `src/memory_cognitive.rs`
> (six `u64` counters — **frozen, byte-for-byte identical**).

## Why

The old read path took the same global `Mutex<CognitiveMemory>` that daemon
startup holds for long stretches (bootstrap, WAL recovery, snapshot restore).
During canary startup the stats RPC would block on that lock and never return
inside the gate's 30s probe budget. The gate — correctly — fails closed on a
timeout, so the canary reddened and self-deploy could never converge.

`simard status` showed a **separate, independent** cosmetic gap on the operator
side: the daemon's `Resources` **CPU (`cpu_pct`) was unconditionally `None`**,
so it always rendered as `absent` regardless of daemon state. (Daemon RSS was
already sampled from `/proc/<daemon_pid>` when the PID was known.) Note that
`simard status` does **not** call the telemetry RPC: its daemon PID/NRestarts
come from `systemctl show`, its per-type memory-record counts come from the
published metrics gauges, and `instance_uptime` is currently unimplemented —
none of those are affected by the RPC lock-starvation, and none are populated
by Bricks A–C. Brick D fixes the one genuine `simard status` gap (CPU%) and is
deliberately decoupled from the RPC fix.

The fix is **additive and non-breaking**. The stats *payload shape does not
change*. Only the *source* of the read (snapshot vs. heavy lock) and the
*gate's readiness behaviour* (bounded retry vs. single shot) change.

## What changed at a glance

| Surface | Before | After |
| --- | --- | --- |
| `get_statistics` read path | Takes global `Mutex<CognitiveMemory>`; blocks during startup | Serves atomic `CognitiveStatistics` snapshot; never takes the heavy mutex |
| Uninitialised stats | Could return empty / `absent` | Retryable **traced error**, never `Ok(default)` |
| `run_rpc_health_gate` | Single probe; a startup-time block → `TimedOut` red | Bounded retry of **only** the `TimedOut` outcome inside the unchanged 30s window |
| Daemon startup | Socket accepts before stats are ready | Snapshot **primed synchronously before `spawn_server`**; try_lock refresher bounds staleness |
| `simard status` resources | `cpu_pct` always `None` (renders `absent`) | `cpu_pct` from non-blocking `/proc` sampling of the daemon PID (Brick D). RSS was already daemon-scoped via `read_process_rss_bytes(daemon_pid)`. PID/NRestarts (systemctl), memory-record counts (metrics gauges) and uptime (unimplemented) are **out of scope** — `simard status` never calls the RPC. |
| Wire type `CognitiveStatistics` | six `u64` | **unchanged** — six `u64` |
| Probe args `RPC_HEALTH_PROBE_ARGS` | `["memory","stats"]` | **unchanged** |
| `health_timeout` | 30s | **unchanged** |

## Architecture

Four cooperating "bricks", each self-contained and independently testable.

### Brick A — Statistics snapshot fast-path

`LibraryCognitiveMemory` maintains an in-process
`Mutex<Option<CognitiveStatistics>>` snapshot. `get_statistics` reads that
snapshot and returns immediately — it **never** calls `self.lock()` (the heavy
`Mutex<CognitiveMemory>`) on the read path, so it cannot be starved by startup
work.

Semantics:

- **Populated snapshot** → return it immediately (`Ok(stats)`), schema
  byte-for-byte identical to the historical value.
- **`None` (pre-first-update)** → return a **retryable error**, traced via
  `tracing` + OTel. It is **never** collapsed to `Ok(CognitiveStatistics::default())`,
  because a forged all-zero "healthy" reading would let a genuinely
  uninitialised daemon green the gate.
- The snapshot lock uses **explicit poison recovery** — no `unwrap`/`expect`.
  A poisoned lock degrades to the same retryable error.

A default no-op `refresh_stats_snapshot(&self)` is added to the
`CognitiveMemoryOps` trait so all ~30 implementations and test doubles compile
untouched. `LibraryCognitiveMemory` overrides it to recompute the snapshot
**off the read path** via `try_lock` — it never blocks and never holds the
heavy mutex.

> **Design note (fold moved off the read path).** The historical
> `get_statistics` folded the library's `HashMap<String, usize>` (keyed by
> `MemoryCategory::as_str()`) into the typed `CognitiveStatistics` DTO *inline
> on the read path* while holding the heavy lock. That fold now lives in the
> shared `stats_from_memory` helper called by the primer and by
> `refresh_stats_snapshot` (under `try_lock`), which stores the already-typed
> `CognitiveStatistics` in the `Mutex<Option<..>>`. `get_statistics` triggers a
> best-effort non-blocking refresh and then only clones the stored DTO — the
> read path never touches the heavy `Mutex<CognitiveMemory>`.

### Brick B — RPC-health gate readiness (retry-within-window)

`run_rpc_health_gate` (in `src/self_relaunch/gates.rs`) retries **only** the
`ProbeOutcome::TimedOut` outcome, inside the **unchanged** `health_timeout`
(30s) budget, using short per-attempt probes with backoff. A
connection-refused / not-yet-listening socket during the brief startup window
is treated as retryable; a **successful populated stats round-trip** is
treated as ready (green).

Fail-closed is preserved and **narrowed to the timeout outcome only**:

- **Absent daemon socket** → immediate red (a genuinely dead daemon; the
  candidate would otherwise fall through to a tier-2 on-disk store and green
  without proving reachability — #4639 F2, #2896).
- **Non-zero exit** → immediate red.
- **Spawn failure** → immediate red.
- **Window exhausted without a populated success** → red, with the historical
  detail string `rpc health timed out after 30s (memory stats did not return)`.

This is defence-in-depth: even if Brick A regressed, the gate would still fail
closed. The retry only makes a *healthy-but-still-initialising* daemon
converge instead of flapping red.

### Brick C — Daemon snapshot priming + refresher

The OODA daemon (`src/operator_commands_ooda/daemon/mod.rs`) **primes the
stats snapshot synchronously before `spawn_server`**. The IPC socket therefore
only begins accepting once a real, populated snapshot exists — closing the
race where a client could connect before the first update. A lightweight
`try_lock` refresher thread then recomputes the snapshot on cheap events to
bound staleness. The refresher never blocks and never holds the heavy mutex.

### Brick D — `/proc` CPU sampling

`src/status/provider.rs` replaces the single hardcoded `cpu_pct: None` field in
`assemble_resources` with **non-blocking, defensive `/proc` sampling** of the
daemon's `MainPID`. (RSS is already daemon-scoped via
`read_process_rss_bytes(daemon_pid)`; only `cpu_pct` was unconditionally
`None`.) It validates PID ownership against the authoritative `daemon_pid`
passed into `assemble_resources`, bounds-checks fields, and **degrades to
`None` on any malformed or missing input — never panics**.

> **Design note (CPU% requires a delta).** Unlike RSS, an instantaneous CPU
> percentage cannot come from a single `/proc/<pid>/stat` read — that yields
> cumulative `utime+stime` jiffies since process start. Two options exist:
> 1. **Cumulative-since-start** — `(utime+stime) / elapsed_ticks` from a single
>    read. Truly non-blocking, reports a lifetime average (matching `ps aux`
>    `%CPU`), and always populated once the process is warm.
> 2. **Inter-call delta** — cache the previous `(jiffies, wall_clock)` sample in
>    the status provider and divide the delta. Reflects recent load, but the
>    first `simard status` after start returns `None` (no prior sample), and it
>    adds cross-call mutable state to the provider.
>
> **Implemented: Option 1.** The hard requirement is a **non-blocking** sample
> with **no sampling sleep** and no new cross-call state, so `read_process_cpu_pct`
> computes the lifetime-average `%CPU` from a single `/proc/<pid>/stat` +
> `/proc/uptime` read: `(utime+stime)/CLK_TCK` over `uptime − starttime/CLK_TCK`,
> `* 100`. Every step degrades to `None` on any malformed / missing input and
> never panics. Brick D is fully independent of the RPC fix: it is the host-side
> `/proc` read of `daemon_pid` and does not depend on `get_statistics` or any RPC
> payload. (The RPC snapshot fixed by Bricks A–C backs `simard memory stats`, not
> the `simard status` Resources section.)

## API

### `CognitiveMemoryOps::get_statistics`

```rust
fn get_statistics(&self) -> SimardResult<CognitiveStatistics>;
```

Returns the current six-counter snapshot. For `LibraryCognitiveMemory` this
reads the maintained snapshot and **does not take** the global
`Mutex<CognitiveMemory>`.

- `Ok(stats)` — populated snapshot (schema unchanged).
- `Err(_)` — snapshot not yet initialised or lock poisoned; **retryable**.
  Callers (the gate, the status provider) treat this as "not ready yet", not
  as "zero".

`CognitiveStatistics` is unchanged:

```rust
pub struct CognitiveStatistics {
    pub sensory_count: u64,
    pub working_count: u64,
    pub episodic_count: u64,
    pub semantic_count: u64,
    pub procedural_count: u64,
    pub prospective_count: u64,
}
```

### `CognitiveMemoryOps::refresh_stats_snapshot`

```rust
/// Recompute the statistics snapshot off the read path.
/// Default: no-op (keeps all implementations compiling untouched).
fn refresh_stats_snapshot(&self) {}
```

`LibraryCognitiveMemory` overrides this to recompute the snapshot via
`try_lock`. It is **internal** — it is never exposed as an IPC operation and
never appears on the wire. If `try_lock` cannot acquire the heavy mutex, the
refresh is skipped (the previous snapshot remains) rather than blocking.

### Wire operation (unchanged)

`MemoryRequest::GetStatistics` → `CognitiveStatistics`. Request and response
bytes are **held identical** across daemon/canary revisions; only the internal
*semantics* of how `get_statistics` sources its answer changed. Mixed
new↔old daemon/canary revisions remain wire-compatible. See
[RPC Wire Protocol](./rpc-wire-protocol.md).

## Configuration

There are **no new configuration knobs** and **no changed values**.

| Setting | Location | Value | Notes |
| --- | --- | --- | --- |
| `health_timeout` | `RelaunchConfig` | 30s (unchanged) | Hard cap for the whole rpc-health gate, including all retries. |
| `RPC_HEALTH_PROBE_ARGS` | `src/self_relaunch/gates.rs` | `["memory", "stats"]` (unchanged, hardcoded) | Never interpolated from env / untrusted input — prevents command injection. |
| `SIMARD_STATE_ROOT` | canary env allow-list | unchanged | Re-injected so the candidate resolves the same daemon socket as the live daemon. |

The gate's per-attempt budget and backoff are internal implementation details
chosen so multiple retries fit inside the single 30s hard cap; they are not
operator-tunable and do not change the total budget.

## Usage

Nothing changes for operators or scripts. The same commands work; they now
just return promptly during startup.

```console
$ simard memory stats
sensory:     0
working:     12
episodic:    418
semantic:    1203
procedural:  57
prospective: 9
```

```console
$ simard memory stats --json
{"sensory_count":0,"working_count":12,"episodic_count":418,
 "semantic_count":1203,"procedural_count":57,"prospective_count":9}
```

`simard status` now shows a populated **CPU** figure in `Resources` (Brick D)
instead of `absent`; the remaining fields are unchanged and come from their
existing sources (PID/NRestarts from `systemctl`, memory-record counts from
metrics gauges):

```console
$ simard status
Daemon:      running (MainPID 48213, NRestarts 0)
Resources:   CPU 3.1%   RSS 214.7 MiB
Memory:      1699 records across 6 types
```

## Deploy canary flow

During self-deploy the overseer builds a candidate, starts it as a canary, and
runs the relaunch gates. The rpc-health gate now converges:

1. Canary starts; daemon **primes the stats snapshot before opening the
   socket** (Brick C).
2. `run_rpc_health_gate` requires the daemon socket to exist (fail-closed on
   absent — Brick B), then probes `simard memory stats`.
3. If the daemon is still finishing startup, an early attempt may time out or
   be refused. The gate **retries the timeout only**, inside the 30s window.
4. `get_statistics` serves the primed snapshot without the heavy lock
   (Brick A) → a populated round-trip → **gate green**.
5. Remaining gates run; the canary lands; `DeployDrift` shrinks toward 0.

If the daemon is genuinely dead or wedged, the socket is absent or every
attempt fails, the window exhausts, and the gate stays **red** — self-deploy
correctly refuses to promote a broken binary.

## Observability

All new code uses **structured `tracing` + OpenTelemetry spans only** — no
`print!`/`println!`, no silent fallbacks. Aggregate counters go through
`cognitive_memory::metrics::increment(kind, site)` with numeric/enum fields
only (no PII, no secrets, no memory contents).

Representative signals:

- **Snapshot not ready** — traced retryable error from `get_statistics` when
  the snapshot is `None`, with the call site; a metrics counter for
  snapshot-miss so a persistent miss (not just the brief init gap) is visible.
- **Gate retry** — each retried `TimedOut` attempt is traced within the
  rpc-health span, so a canary that needed N attempts to converge is
  observable; a distinct terminal event for green vs. exhausted-window red.
- **`/proc` sampling degrade** — a malformed/missing `/proc` read that
  degrades `cpu_pct` to `None` is traced, so "absent" CPU telemetry is never
  silent.

See [Telemetry & Metrics](./telemetry-metrics.md) and
[Overseer Deploy Red-Canary Diagnostics](./overseer-deploy-canary-diagnostics.md).

## Fail-closed guarantees

The change **narrows** fail-closed behaviour to the timeout outcome; it never
trades the original DoS-style lock-starvation red for a fail-open green.

- Absent socket, non-zero exit, spawn failure → **immediate red** (unchanged).
- Snapshot `None` → **retryable error**, never `Ok(default)`; a forged
  all-zero "healthy" reading is impossible.
- Retry lives entirely inside the existing 30s cap; exhausting it → **red**.
- The frozen fail-closed regression tests keep their original fast timing.

## Security considerations

- **No new IPC operation and no wire-shape change.** The snapshot is served
  through the existing `GetStatistics` arm only; `refresh_stats_snapshot` is
  internal and never exposed.
- **Trust boundary preserved.** The Unix domain socket stays local-only with
  owner-only filesystem permissions — not widened to TCP or an abstract
  socket.
- **No command injection.** `RPC_HEALTH_PROBE_ARGS` stays hardcoded
  (`["memory","stats"]`); env/untrusted args are never interpolated.
- **No forged-healthy readings.** `None` snapshot propagates as a retryable
  error; the snapshot is primed before the socket accepts.
- **Defensive `/proc` parsing.** PID ownership validated, fields bounds-checked,
  degrade to `None` on any error, never panic.
- **No PII / secrets in telemetry.** Only the six aggregate `u64` counters and
  enum outcomes appear in the snapshot, logs, and metrics.
- **Least privilege.** No new capabilities; the `try_lock` refresher never
  blocks or holds the heavy mutex; nothing is persisted (all state is
  in-process `Mutex<Option<..>>` plus `/proc` reads — no schema, no
  migrations).

## Testing

- **Brick A** — snapshot `None` ⇒ `Err`; primed-empty ⇒ `Ok` all-zero;
  `get_statistics` never takes the heavy mutex; `refresh_stats_snapshot` via
  `try_lock` never blocks.
- **Brick B** — `TimedOut` retried within the window; non-zero exit /
  spawn-failure / absent-socket immediate red; exhausted window ⇒ `TimedOut`
  red.
- **Brick D** — `/proc` happy path parses; malformed/missing ⇒ `None`;
  PID-reuse/missing ⇒ `None`; no panic.
- **Regression (frozen — preserved, not modified):** the fail-closed tests in
  `src/self_relaunch/gates.rs` and
  `src/memory_ipc/tests_launcher_fail_closed_2896.rs` still pass with their
  original fast timing.

## Related

- [StatusSnapshot API reference](./status-snapshot-api.md)
- [Canary Gate Isolation & Self-Deploy Convergence](./canary-gate-convergence.md)
- [Overseer Deploy Red-Canary Diagnostics](./overseer-deploy-canary-diagnostics.md)
- [Self-Deploy API](./self-deploy-api.md)
- [RPC Wire Protocol](./rpc-wire-protocol.md)
- [Cognitive Memory Library Adapter](../architecture/cognitive-memory-library-adapter.md)
- [Safe Self-Update](../safe-self-update.md)
