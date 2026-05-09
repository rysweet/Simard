---
title: Cognitive memory bridge helpers
description: Reference for launch_writer_bridge and open_reader_bridge — the canonical entry points for obtaining a CognitiveMemoryOps adapter, including the in-process Arc shortcut and the strict no-silent-degradation contract.
last_updated: 2026-05-09
owner: simard
doc_type: reference
related:
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
  - ./bridge-wire-protocol.md
---

# Cognitive memory bridge helpers

`src/memory_ipc/launcher.rs` exposes two helper functions that every
consumer should use to obtain a typed cognitive-memory bridge:

| Helper | Returns | Use case |
|--------|---------|----------|
| `launch_writer_bridge` | `SimardResult<WriterBridge>` | Anything that may write — dashboard mutation handlers (`/api/goals/promote/<id>`, `/api/goals/demote/<id>`, `/api/goals/dismiss/<id>`), meeting REPL flows, restore CLI |
| `open_reader_bridge` | `SimardResult<ReaderBridge>` | Read-only consumers — dashboard read handlers (`workboard`, `current_work`, `metrics`, `goals` GET), engineer-loop top-5 read, inspection tools |

These helpers encapsulate the **daemon-or-direct fallback ladder** so that
callers never instantiate `NativeCognitiveMemory` or `RemoteCognitiveMemory`
directly. Issue [#1590](https://github.com/rysweet/Simard/issues/1590) and
its follow-up regression-fix PR refined the resolution ladder to add an
**in-process Arc shortcut** for callers that share a process with the OODA
daemon, and to **remove the silent read-only fallback** that previously
masked writer-acquisition failures as `{"status":"ok"}` responses.

---

## Typed bridge wrappers

The two helpers return distinct types — `WriterBridge` and `ReaderBridge` —
rather than a common `Box<dyn CognitiveMemoryOps>`. This is a deliberate
design choice: it forces the **failure-to-acquire-a-writer** case to surface
at the helper's `?` site, not later at every `store_fact` call site.

```rust
pub struct WriterBridge {
    inner: Box<dyn CognitiveMemoryOps>,
}

pub struct ReaderBridge {
    inner: Box<dyn CognitiveMemoryOps>,
}

impl WriterBridge {
    /// Borrow as `&dyn CognitiveMemoryOps` for passing to `load_goal_board`,
    /// `save_goal_board`, `persist_board`, etc.
    pub fn ops(&self) -> &dyn CognitiveMemoryOps { &*self.inner }
}

impl ReaderBridge {
    pub fn ops(&self) -> &dyn CognitiveMemoryOps { &*self.inner }
}
```

A `ReaderBridge` is intentionally **not** convertible into a `WriterBridge`.
Callers that hold a `ReaderBridge` and discover they need to write must
re-call `launch_writer_bridge` — at which point any acquisition failure is
reported up-front.

`load_goal_board` and `save_goal_board` continue to accept
`&dyn CognitiveMemoryOps`, so callers pass `bridge.ops()` rather than
`&*bridge`.

---

## `launch_writer_bridge`

```rust
pub fn launch_writer_bridge(state_root: &Path) -> SimardResult<WriterBridge>
```

Returns a bridge that supports both reads and writes. Tries three writer
sources in order, stopping at the first success:

| Tier | Source | Condition |
|------|--------|-----------|
| 0 | Daemon-registered in-process `Arc<dyn CognitiveMemoryOps>` | Same-process callers (dashboard handlers, OODA reflection paths) when the daemon has registered its writer via `register_in_process_writer` |
| 1 | `RemoteCognitiveMemory::connect(default_socket_path())` | A running OODA daemon's IPC socket exists at `~/.simard/memory.sock` and `state_root` matches the daemon's |
| 2 | `NativeCognitiveMemory::open(state_root)` | No daemon socket; this process can take the writer lock directly (after `reap_stale_open_lock`) |

If all three tiers fail, the helper returns
`Err(SimardError::RuntimeInitFailed { component: "memory-ipc-launcher", … })`.

**There is no read-only fallback at the writer-acquisition path.** A caller
that asked for a writer always learns synchronously whether one was
obtainable. Earlier revisions of this helper ended with a tier-3 fallback
to `NativeCognitiveMemory::open_read_only` and returned the read-only handle
wrapped as a `WriterBridge` — that produced silent hollow-success bugs
(dashboard `demote_goal` returning `{"status":"ok"}` while the underlying
`store_fact` was a no-op). Tier 3 has been removed; the launcher now
propagates the read-write open error directly.

The helper additionally:

- Creates `state_root` (and parents) on first call via `fs::create_dir_all`.
- Runs `reap_stale_open_lock` before tier 2 to clear locks left by crashed
  writers.

### Tier 0: in-process Arc shortcut

The OODA daemon registers its live `Arc<dyn CognitiveMemoryOps>` (the same
handle backing the IPC server) with the launcher at startup:

```rust
// src/memory_ipc/launcher.rs
static IN_PROCESS_WRITER: OnceLock<Arc<dyn CognitiveMemoryOps>> = OnceLock::new();

pub fn register_in_process_writer(writer: Arc<dyn CognitiveMemoryOps>) {
    let _ = IN_PROCESS_WRITER.set(writer);
}
```

When the dashboard (which runs inside the daemon process) calls
`launch_writer_bridge`, the launcher checks the `OnceLock` first. On a hit,
it wraps the `Arc` in a `WriterBridge` and returns immediately — no Unix
socket round-trip, no lock contention, no risk of falling into a read-only
fallback. This is the **primary path** for in-process callers.

Non-daemon callers (the meeting REPL, the engineer loop, CLI tools) skip
tier 0 because nothing has registered into the `OnceLock` in their
process. They proceed to tier 1 (IPC) and tier 2 (direct open) as before.

### Tier 1 → 2 transition: state-root agreement

Tier 1 (IPC) only fires when the requested `state_root` matches the
daemon's owned state root, computed via
`state_root_matches_daemon(state_root)`. Both sides canonicalize their
paths (resolving symlinks and `..` segments) before comparing. If they
disagree, the launcher silently skips IPC and proceeds to tier 2 — this
prevents a daemon owning a different DB from masking the writes the caller
intended for its own DB.

If tier 1 is selected and the IPC connection fails (socket exists but
`RemoteCognitiveMemory::connect` errors), the launcher logs the error to
stderr and falls through to tier 2 rather than returning early. This keeps
short-window daemon restarts (where the socket file lingers a few hundred
milliseconds) from producing spurious failures.

### Defensive guard: `is_read_only()`

`CognitiveMemoryOps` exposes:

```rust
pub trait CognitiveMemoryOps: Send + Sync + 'static {
    // … existing methods …
    fn is_read_only(&self) -> bool { false }
}
```

`NativeCognitiveMemory::open_read_only` overrides this to return `true`.
The IPC client (`RemoteCognitiveMemory`) and the daemon's in-process Arc
both leave the default `false` because the daemon is the writer.

`WriterBridge::new`/`wrap` debug-asserts `!ops.is_read_only()` before
returning. In release builds the assertion compiles out, but the launcher
itself enforces the invariant: tier 0 (Arc), tier 1 (IPC), and tier 2
(`open`) all return writer-capable handles by construction; the removed
tier 3 was the only path that could have produced a read-only handle in a
`WriterBridge`.

**Example — dashboard write handler**

```rust
use simard::goal_curation::{load_goal_board, save_goal_board};
use simard::memory_ipc::{launch_writer_bridge, default_state_root};

let state_root = default_state_root();
let bridge = launch_writer_bridge(&state_root)?;     // Err if no writer available

let mut board = load_goal_board(bridge.ops())?;
mutate(&mut board);
save_goal_board(&board, bridge.ops())?;
```

The `?` on `launch_writer_bridge` is now load-bearing: if it would have
returned a read-only handle in the prior implementation, it now returns
`Err`, and the HTTP handler converts that into a 500 with the underlying
error message rather than `{"status":"ok"}`.

---

## `open_reader_bridge`

```rust
pub fn open_reader_bridge(state_root: &Path) -> SimardResult<ReaderBridge>
```

Returns a bridge optimised for read-only consumers. Tries two sources in
order:

| Tier | Source | Condition |
|------|--------|-----------|
| 1 | `RemoteCognitiveMemory::connect(default_socket_path())` | A running daemon's IPC socket exists |
| 2 | `NativeCognitiveMemory::open_read_only(state_root)` | No daemon; the read-only opener never contends with the writer lock |

Read-only callers should always prefer this helper over `launch_writer_bridge`
because:

- It never attempts to take the writer lock, so it never contends with a
  running daemon when the IPC socket happens to be missing during a
  daemon restart.
- `open_read_only` is cheap — no WAL recovery, no lock acquisition, no
  reaper.

The returned `ReaderBridge` does not carry a write capability in its type.
Calling `bridge.ops().store_fact(…)` will still compile (the underlying
trait object exposes the full `CognitiveMemoryOps` surface) and will fail
at runtime with `BridgeTransportError`. Callers should use `WriterBridge`
when they intend to write — see "Typed bridge wrappers" above for the
rationale.

**Example — dashboard read handler**

```rust
use simard::goal_curation::load_goal_board;
use simard::memory_ipc::{open_reader_bridge, default_state_root};

let bridge = open_reader_bridge(&default_state_root())?;
let board = load_goal_board(bridge.ops())?;
render_workboard(&board);
```

---

## State root resolution

Both helpers accept a `&Path`. The conventional way to compute that path is:

```rust
let state_root = simard::memory_ipc::default_state_root();
```

`default_state_root()` already exists today and resolves to:

1. `$SIMARD_STATE_ROOT` if set, else
2. `$HOME/.simard/state`.

The Unix-domain socket path used by the IPC tier is independent of
`SIMARD_STATE_ROOT` — see `default_socket_path()`, which always resolves to
`$HOME/.simard/memory.sock`. This is intentional: the meeting REPL and the
daemon must discover each other even when they disagree about the DB
directory.

---

## Migration from ad-hoc instantiation

Today (post-issue [#1590](https://github.com/rysweet/Simard/issues/1590) and
its regression-fix follow-up), every consumer that touches cognitive
memory uses one of these two helpers. Inline `NativeCognitiveMemory` and
`RemoteCognitiveMemory` instantiation is reserved for the daemon
(`SharedMemory` setup) and tests. Direct `goal_records.json` reads via
`std::fs` and `serde_json` have been removed entirely.

The most-mature inline pattern previously lived in
`src/operator_commands_meeting/meeting_session.rs::launch_real_meeting_bridge`.
That function is now a thin wrapper:

```rust
fn launch_real_meeting_bridge() -> Result<Box<dyn CognitiveMemoryOps>, Box<dyn Error>> {
    let bridge = launch_writer_bridge(&default_state_root())?;
    Ok(bridge.into_box())
}
```

New consumers should always call the helpers directly rather than copying
the ladder.

---

## Migration call-site map

The following sites use one of the two helpers in place of inline
instantiation, `FileBackedGoalStore`, or direct `goal_records.json`
reads:

| File | Helper |
|------|--------|
| `src/operator_commands_meeting/meeting_session.rs` | `launch_writer_bridge` (wrapped as `launch_real_meeting_bridge`) |
| `src/operator_commands_meeting/goal_curation.rs` | `open_reader_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/operator_commands_meeting/improvement_curation.rs` | `launch_writer_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/engineer_loop/mod.rs` | `open_reader_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/operator_commands_dashboard/goals.rs` (mutation handlers) | `launch_writer_bridge` + `save_goal_board` |
| `src/operator_commands_dashboard/goals.rs` (GET handlers) | `open_reader_bridge` + `load_goal_board` |
| `src/operator_commands_dashboard/workboard.rs` | `open_reader_bridge` |
| `src/operator_commands_dashboard/current_work.rs` | `open_reader_bridge` |
| `src/operator_commands_dashboard/metrics.rs` | `open_reader_bridge` |
| `src/bootstrap/assembly.rs` (`goal_store`) | `CognitiveMemoryGoalStore` (which itself uses both helpers) — see [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md) |

`FileBackedGoalStore` itself remains in `src/goals/store.rs` as a value
type used by `meeting_backend` and tests. It is no longer used as a
production goal-board persistence target.

---

## Related reading

- [Goal board API reference](./goal-board-api.md) — the primary consumers of
  these helpers.
- [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md) —
  how `RuntimePorts.goal_store` wraps these helpers behind the `GoalStore`
  trait.
- [Cognitive memory bridge wire protocol](./bridge-wire-protocol.md) — what
  the IPC tier negotiates.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md) —
  the lifecycle the helpers participate in.
