---
title: Cognitive memory bridge helpers
description: Reference for launch_writer_bridge and open_reader_bridge — the canonical entry points for obtaining a CognitiveMemoryOps adapter from non-daemon contexts.
last_updated: 2026-05-08
owner: simard
doc_type: reference
status: design — not yet implemented
related:
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
  - ./bridge-wire-protocol.md
---

# Cognitive memory bridge helpers

> **Status: design specification — not yet implemented.**
>
> Neither `launch_writer_bridge` nor `open_reader_bridge` exists in
> [`src/memory_ipc/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/memory_ipc/mod.rs)
> today. That module currently exports `default_socket_path`,
> `default_state_root`, `reap_stale_open_lock`, `RemoteCognitiveMemory`, and
> `SharedMemory`. The bridge-acquisition pattern lives inline in
> [`launch_real_meeting_bridge`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_meeting/meeting_session.rs)
> at `meeting_session.rs:29`.
>
> This document is the **target API** that issue
> [#1590](https://github.com/rysweet/Simard/issues/1590) will land. It
> exists so that consumer migrations and call-site updates can be reviewed
> against a stable contract. Once the helpers are implemented, this status
> banner will be removed and the doc will return to mkdocs nav.

`src/memory_ipc/mod.rs` will expose two helper functions that every non-daemon
consumer should use to obtain a typed cognitive-memory bridge:

| Helper | Returns | Use case |
|--------|---------|----------|
| `launch_writer_bridge` | `SimardResult<WriterBridge>` | Anything that may write — dashboard mutation handlers, meeting REPL flows, restore CLI |
| `open_reader_bridge` | `SimardResult<ReaderBridge>` | Read-only consumers — dashboard read handlers (`workboard`, `current_work`, `metrics`, `goals` GET), engineer-loop top-5 read, inspection tools |

These helpers will encapsulate the **daemon-or-direct fallback ladder** that
currently lives only inside `launch_real_meeting_bridge`. They replace ad-hoc
instantiation of `NativeCognitiveMemory` and `RemoteCognitiveMemory` across
the codebase.

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

Returns a bridge that supports both reads and writes. Tries two writer
sources in order, stopping at the first success:

| Tier | Source | Condition |
|------|--------|-----------|
| 1 | `RemoteCognitiveMemory::connect(default_socket_path())` | A running OODA daemon's IPC socket exists at `~/.simard/memory.sock` |
| 2 | `NativeCognitiveMemory::open(state_root)` | No daemon socket; this process can take the writer lock directly |

If both tiers fail (the daemon socket is absent **and** another writer holds
the local LadybugDB lock that the stale-lock reaper could not free), the
helper returns `Err(SimardError::BridgeTransportError { … })`. There is **no
silent read-only fallback at the writer-acquisition path** — a caller that
asked for a writer always learns synchronously whether one was obtainable.

The helper additionally:

- Creates `state_root` (and parents) on first call via `fs::create_dir_all`.
- Runs `reap_stale_open_lock` before tier 2 to clear locks left by crashed
  writers.

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

Today, each consumer either:

- Instantiates `NativeCognitiveMemory` or `RemoteCognitiveMemory` inline,
  sometimes with subtle variations in tier order, lock handling, and error
  reporting; or
- Reads `goal_records.json` directly via `std::fs` and `serde_json`,
  bypassing cognitive memory entirely.

The most-mature inline pattern lives in
[`src/operator_commands_meeting/meeting_session.rs::launch_real_meeting_bridge`](https://github.com/rysweet/Simard/blob/main/src/operator_commands_meeting/meeting_session.rs).
That function currently:

1. Tries `RemoteCognitiveMemory::connect(default_socket_path())`.
2. Falls back to `NativeCognitiveMemory::open(state_root)`.
3. Falls back to `NativeCognitiveMemory::open_read_only(state_root)` with a
   warning.

Issue #1590 will:

- Extract the writer-bearing tiers (1 + 2) into `launch_writer_bridge`.
- Extract the read-only tier into `open_reader_bridge` (combined with
  tier 1 of the writer ladder for daemon-aware reads).
- Reduce `launch_real_meeting_bridge` to a thin wrapper:

  ```rust
  fn launch_real_meeting_bridge() -> SimardResult<WriterBridge> {
      launch_writer_bridge(&default_state_root())
  }
  ```

  with the `Box<dyn Error>` shim preserved at its current call site as long
  as the meeting backend's caller signature still uses it.

New consumers should always call the helpers directly rather than copying
the ladder.

---

## Migration call-site map

After issue #1590 lands, the following sites will use one of the two
helpers in place of inline instantiation or `FileBackedGoalStore`:

| File | Current pattern | Target helper |
|------|-----------------|---------------|
| `src/operator_commands_meeting/meeting_session.rs:29` | inline three-tier ladder | `launch_writer_bridge` (wrapped) |
| `src/operator_commands_meeting/goal_curation.rs:58` | `FileBackedGoalStore::try_new(... goal_records.json)` | `open_reader_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/operator_commands_meeting/improvement_curation.rs:123` | `FileBackedGoalStore::try_new(... goal_records.json)` | `launch_writer_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/engineer_loop/mod.rs:276` | `FileBackedGoalStore::try_new(... goal_records.json).active_top_goals(5)` | `open_reader_bridge` + `load_goal_board` + `active_goals_as_records` |
| `src/operator_commands_dashboard/goals.rs:12,48` | `std::fs::read_to_string(... goal_records.json)` + `serde_json::from_str` | `open_reader_bridge` (GET) + `launch_writer_bridge` (mutation handlers) |
| `src/operator_commands_dashboard/workboard.rs:112` | `std::fs::read_to_string(... goal_records.json)` | `open_reader_bridge` |
| `src/operator_commands_dashboard/current_work.rs` | inline file read | `open_reader_bridge` |
| `src/operator_commands_dashboard/metrics.rs` | inline file read | `open_reader_bridge` |

`FileBackedGoalStore` itself remains in `src/goals/store.rs` as a value type
used by `meeting_backend` and tests — issue #1590 only retires its use as a
production goal-board persistence target.

---

## Related reading

- [Goal board API reference](./goal-board-api.md) — the primary consumers of
  these helpers.
- [Cognitive memory bridge wire protocol](./bridge-wire-protocol.md) — what
  the IPC tier negotiates.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md) —
  the lifecycle the helpers participate in.
