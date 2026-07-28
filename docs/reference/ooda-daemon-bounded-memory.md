---
title: OODA daemon bounded steady-state memory
description: How the simard-ooda daemon bounds its resident and swap footprint — concurrent bounded-tail capture of engineer child output, single-shot memory-ipc reconnect with structured broken-pipe tracing, bounded corrupt-WAL recovery, heartbeat main_pid for status RSS/CPU, and the N-cycle bounded-envelope regression gate.
last_updated: 2026-07-28
owner: simard
doc_type: reference
status: reference
related:
  - ./concurrent-engineer-dispatch.md
  - ./status-snapshot-api.md
  - ./cognitive-memory-wal-crash-consistency.md
  - ../concepts/unified-telemetry-and-status.md
  - ../howto/diagnose-and-recover-ooda-step-failures.md
---

# OODA daemon bounded steady-state memory

> **Goal:** the long-lived `simard-ooda.service` daemon holds a **bounded**
> resident and swap footprint across an unbounded number of OODA cycles. Under
> a 32 GiB `memory.max` cgroup, steady-state memory must not grow monotonically
> with uptime, must not force chronic swap/reclaim, and `simard status` must be
> able to report the daemon's own RSS/CPU.

Modules touched (all additive / non-breaking):

| Concern | Module |
|---------|--------|
| Engineer child output capture (primary) | `simard::engineer_loop::agent_spawn` |
| memory-ipc broken-pipe logging | `simard::memory_ipc::server` |
| memory-ipc client reconnect | `simard::memory_ipc::client` |
| Heartbeat `main_pid` | `simard::operator_commands_ooda::daemon` |
| Status RSS/CPU from heartbeat | `simard::status::provider` |
| Corrupt-WAL recovery bounding | `simard::operator_commands_ooda::daemon` (checkpoint cadence), `simard::cognitive_memory::open_guard` (fail-closed guard) |
| OODA retained-state pruning | `simard::ooda_loop::types` |

## The defect this bounds

Live evidence (`2026-07-28T02:15Z`): the `simard-ooda.service` cgroup held
**~247.7 GiB in swap** (`memory.swap.current=265946054656`) with only ~17 GiB
resident after ~67 min of uptime. `memory.events` reported **2,641,976
`memory.max` pressure events** with `oom_kill=0` — the cgroup chronically hit
its 32 GiB `memory.max` and forced continuous reclaim/swap even though the
504 GiB host had ~469 GiB free. `simard status` could not read the daemon's own
CPU/RSS ("daemon CPU / RSS absent"), and the journal showed repeated
`memory-ipc: connection error: … write-len: Broken pipe` plus recurring
`cognitive.wal.corrupt` rotations.

**Attributed root cause (diagnose-first):** memory accumulated primarily in the
engineer subprocess capture path. `run_engineer_subprocess` used
`Child::wait_with_output()`, which **fully buffers** the child's entire stdout
and stderr in RAM until the (unbounded-runtime) agent exits. A long-running,
chatty `amplihack copilot` engineer therefore grew the daemon's heap without
limit for the life of the cycle, driving the cgroup into perpetual reclaim.
Secondary contributors were the memory-ipc broken-pipe path (journal-flooding
`eprintln!` with no reconnect) and unbounded WAL retention (the cognitive store
was checkpointed only at shutdown, so the WAL grew for the daemon's whole
uptime).

## F1 — Bounded concurrent capture of engineer child output (primary)

`run_engineer_subprocess` no longer calls `wait_with_output()`. It now:

1. Takes ownership of the child's `stdout` and `stderr` pipes.
2. Spawns **one reader thread per pipe**, each draining its pipe into a
   fixed-capacity **ring buffer** (`VecDeque<u8>` capped at
   [`SUMMARY_TAIL_BYTES`](#summary_tail_bytes) = 8 KiB) plus a `dropped_bytes`
   counter. Bytes beyond the cap are discarded from the front — only the
   **trailing window** is retained.
3. Keeps the existing Copilot **stdin feeder thread** verbatim (the prompt is
   still streamed on STDIN from a separate thread so a large prompt can never
   deadlock against the child filling its stdout pipe).
4. Calls `child.wait()` with **no wall-clock timeout** — agentic work still runs
   to natural completion; there is still no SIGKILL deadline.
5. Joins both reader threads and the feeder thread, **surfacing any thread
   panic or read error as `Err`** (no silent swallow).
6. Reconstructs the same 8 KiB tail summary string the previous implementation
   returned, preserving the `[truncated N earlier bytes; tail follows]` prefix
   contract via the shared `keep_summary_tail` format.

### Invariant

At all times each capture buffer satisfies:

```
buffer.len() <= SUMMARY_TAIL_BYTES + chunk_size
```

where `chunk_size` is the reader's read-granularity. Total capture RAM is
therefore **O(1)** in the child's output volume — a child that emits gigabytes
of logs contributes at most ~16 KiB (two rings) to the daemon's heap, versus
the previous unbounded growth.

### Preserved behaviour (contract unchanged)

- Unbounded child runtime (no wall-clock SIGKILL).
- The Copilot prompt is still delivered on STDIN via the feeder thread.
- The returned summary is still the trailing ≤ 8 KiB of combined output, with
  the truncation banner when earlier bytes were dropped.
- On non-zero exit, the error still carries `stderr_tail`.

> The stale doc comment on `SUMMARY_TAIL_BYTES` claiming the full output is
> "streamed to Simard's own stdout/stderr (via inherit)" was corrected — the
> pipes are captured and bounded, not inherited.

## F2 — memory-ipc: structured broken-pipe logging + single-shot reconnect

### F2a — Server logging (`memory_ipc::server`)

The per-connection `eprintln!("[simard] memory-ipc: connection error: {e}")`
(and the accept/spawn siblings) are replaced with **structured `tracing`**:

```text
tracing::warn!(endpoint = "memory-ipc", error = %e, "connection error");
```

Emission is **single-shot per failed connection** (not per read attempt), so a
peer that hangs up mid-frame can no longer flood the journal — the previous
behaviour could emit millions of identical lines and fill the disk. No
`eprintln!`/`println!` remain on this path.

### F2b — Client reconnect (`memory_ipc::client`)

`MemoryIpcClient::call` now performs an **at-most-once reconnect** on a broken
pipe:

1. On a write/read error that indicates a severed connection, the client resets
   the poisoned `UnixStream` and reconnects **once** to the same stored
   `socket_path` (never a path derived from wire data).
2. It retries the single in-flight request on the fresh stream.
3. If the retry also fails, it returns a structured `Err`
   (`SimardError::RpcCallFailed { endpoint: "memory-ipc", method, reason }` —
   all three fields populated) — **no retry loop, no silent fallback**.

The public trait surface is unchanged; callers see the same
`SimardResult<MemoryResponse>` and the same error variants. All existing frame
length/size caps are preserved — reconnect allocates only bounded buffers and
never echoes a server-supplied length as an allocation size.

## F3 — Bounded WAL retention (adapter-scoped)

The recurring `cognitive.wal.corrupt` rotations are treated as a **symptom**,
not redesigned. Retention is bounded **only** through the adapter Simard owns;
the upstream `amplihack-memory-lib` internals are **not** modified.

- `cognitive_memory::library_adapter::checkpoint()` is invoked on a **bounded
  cadence** from the OODA daemon loop
  (`operator_commands_ooda::daemon`) — every `WAL_CHECKPOINT_EVERY_CYCLES`
  cycles — so the LadybugDB WAL is compacted into the main file regularly
  instead of growing for the daemon's whole uptime. That keeps replay work and
  on-disk WAL size bounded, shrinking the surface a corrupt rotation can span.
  Previously `checkpoint()` ran only at shutdown.
- The cadence checkpoint is **fail-visible**: a failure logs
  `tracing::warn!(cycle, error, …)` and the loop continues — it is never a
  silent swallow, and it never aborts the cycle.
- `cognitive_memory::open_guard` keeps recovery bounded and **fail-closed**: it
  refuses (returns `Err`) to open a second concurrent handle rather than
  tripping the library's lock-conflict-as-corruption rebuild that would wipe
  memory. This is the existing recovery posture the cadence complements.

See
[Cognitive-memory WAL crash consistency](./cognitive-memory-wal-crash-consistency.md).

## F5 — Heartbeat `main_pid` unblocks `simard status` RSS/CPU

Both `daemon_health.json` heartbeat writers now stamp the daemon's own PID:

```jsonc
{
  "timestamp": "2026-07-28T02:15:00+00:00",
  "cycle_number": 42,
  "status": "running",
  "cycle_phase": "orient",
  "main_pid": 81234,          // additive, optional
  "cycle_start_epoch": 1769565300,
  "interval_secs": 300,
  "actions_taken": "Starting cycle #42"
}
```

`status::provider::daemon_from_heartbeat` reads `main_pid` **defensively**
(`.get("main_pid").and_then(|v| v.as_u64()).map(|p| p as u32)`) and threads it
into `assemble_resources`, which already reads `VmRSS` from
`/proc/<pid>/status` and lifetime-average `%CPU` from `/proc/<pid>/stat`. A
fresh heartbeat therefore renders live daemon **RSS and CPU** instead of the
former `absent`.

### Safety

`main_pid` is **observability-only**. Status never signals, kills, or
authorizes anything by the heartbeat PID; a stale/missing PID degrades to empty
metrics (`None`), never an action. `daemon_health.json` keeps its existing
`0600` permissions and atomic-write; adding the field does not change the file
mode.

## F4 — Bounded OODA retained state (already satisfied)

`OodaState` retains two per-goal maps, `goal_failure_counts` and the
`no_progress_tracker` counters. Investigation confirmed these are **already
bounded**: `OodaState::prune_stale_failure_counts()` (issue #2167) is invoked
once per cycle (`ooda_loop::cycle`, ~L1032) and derives the live goal-ID set
from `active_goals.active` internally, then:

- `goal_failure_counts.retain(|id, _| active_ids.contains(id))`, and
- `no_progress_tracker.retain_goals(&active_ids)`.

Entries for removed/completed goals therefore cannot accumulate across an
unbounded run; live goal state is never dropped. **F4 required no new pruning
API** — the earlier design placeholder `prune_stale_goal_ids(active_goal_ids)`
does not exist and must not be added, as it would duplicate the existing
`prune_stale_failure_counts()`. F4 is reduced to a regression guard (see F6)
that asserts these maps stay bounded across N cycles.

## F6 — Regression gate: bounded envelope across N cycles

Two deterministic, in-process tests assert the bounded envelope without a live
daemon or an external memory-lib server:

- **Engineer capture** (`engineer_loop::tests_bounded_memory` /
  `tests_agent_spawn`): drives an adversarial child that emits far more than
  `SUMMARY_TAIL_BYTES` of output and asserts the returned summary is
  `<= SUMMARY_TAIL_BYTES + margin` and the ring invariant
  `buffer.len() <= cap + chunk` holds.
- **OODA retained state** (`ooda_loop::tests_types`): drives N cycles
  in-process — exercising the existing `prune_stale_failure_counts()` each
  cycle — and asserts the retained collections (`goal_failure_counts`,
  `no_progress_tracker`, and/or sampled RSS) stay within a **non-monotonic
  bounded envelope** (`size <= k × baseline`), not an absolute byte threshold —
  so the test is CI-stable and not RSS-flaky.

## Configuration

This feature is **automatic** and requires no operator configuration. The
relevant tunables are compile-time constants and existing environment
variables:

| Name | Kind | Default | Meaning |
|------|------|---------|---------|
| <a id="summary_tail_bytes"></a>`SUMMARY_TAIL_BYTES` | `const` (`agent_spawn`) | `8 * 1024` | Per-pipe capture ring capacity and returned summary tail size. |
| `SIMARD_ENGINEER_AGENT` | env | `copilot` | Which amplihack agent the engineer subprocess runs (`copilot` \| `rustyclawd`). Unchanged. |
| `SIMARD_AMPLIHACK_BIN` | env | `amplihack` | Override the amplihack binary path. Unchanged. |

> The cgroup `memory.max` (32 GiB) is **not** raised by this change — the fix
> bounds the daemon's demand so it fits, rather than enlarging the limit.

## Verifying the fix in production

```bash
# 1. Daemon RSS/CPU is now visible via status (F5).
simard status            # daemon section shows RSS + %CPU, not "absent"

# 2. Swap growth is bounded — resident + swap track cycle count flatly.
cat /sys/fs/cgroup/system.slice/simard-ooda.service/memory.swap.current
cat /sys/fs/cgroup/system.slice/simard-ooda.service/memory.current

# 3. memory.max pressure events stop climbing once steady state is reached.
grep -E 'max ' /sys/fs/cgroup/system.slice/simard-ooda.service/memory.events

# 4. Broken-pipe events are now structured, single-shot tracing (not a flood).
journalctl -u simard-ooda.service | grep 'memory-ipc'
```

A healthy daemon shows `memory.current` and `memory.swap.current` that plateau
as uptime grows, `max` pressure events that stop incrementing at steady state,
and at most one structured `memory-ipc … connection error` line per severed
connection.

## Guarantees and non-goals

**Guarantees**

- Engineer child output capture is **O(1)** RAM regardless of child output
  volume.
- memory-ipc broken pipes are logged **once** per connection and recovered with
  **one** bounded reconnect, else surfaced as `Err`.
- WAL retention is bounded: the cognitive store is checkpointed on a per-cycle
  cadence (not only at shutdown), and checkpoint failures are fail-visible.
- `simard status` reports daemon RSS/CPU whenever the heartbeat is fresh.
- All failures are surfaced (`tracing` + `Err`) — **no silent fallbacks**.

**Non-goals (explicitly out of scope)**

- Redesigning the OODA loop or the cognitive-memory subsystem.
- Raising the 32 GiB `memory.max`.
- Modifying `amplihack-memory-lib` internals.
- Any change to external CLI/API/behaviour — this work is purely additive.
