---
title: Cognitive memory client helpers
description: Reference for launch_writer_client and open_reader_client — the canonical entry points for obtaining a CognitiveMemoryOps adapter, including the planned in-process Arc shortcut and strict no-silent-degradation contract.
last_updated: 2026-07-06
owner: cognitive-memory
doc_type: reference
related:
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
  - ./rpc-wire-protocol.md
---

# Cognitive memory client helpers

> **Status: partially shipped + design.** The two helpers
> (`launch_writer_client`, `open_reader_client`) and their two-tier
> writer/reader ladders are shipped today and used by every consumer listed
> in the [migration call-site map](#migration-call-site-map). The
> **in-process `Arc` shortcut** (tier 0) and the **strict
> no-silent-degradation contract** (removal of the read-only fallback) are
> tracked under issue
> [#1590](https://github.com/rysweet/Simard/issues/1590) and its follow-up
> regression-fix work; sections below marked "Planned" describe behavior that
> is **not yet present** on `main`. Sections without that marker describe
> code that exists today.

`src/memory_ipc/launcher.rs` exposes two helper functions that every
consumer should use to obtain a typed cognitive-memory client:

| Helper | Returns | Use case |
|--------|---------|----------|
| `launch_writer_client` | `SimardResult<WriterClient>` | Anything that may write — dashboard mutation handlers (`promote_goal`, `demote_goal`, `dismiss_goal`, …), meeting REPL flows, restore CLI |
| `open_reader_client` | `SimardResult<ReaderClient>` | Read-only consumers — dashboard read handlers (`workboard`, `current_work`, `metrics`, GET goals), engineer-loop top-5 read, inspection tools |

These helpers encapsulate the **daemon-or-direct fallback ladder** so that
callers never instantiate `LibraryCognitiveMemory` or `RemoteCognitiveMemory`
directly.

---

## Typed client wrappers

The two helpers return distinct types — `WriterClient` and `ReaderClient` —
rather than a common `Box<dyn CognitiveMemoryOps>`. This is a deliberate
design choice: it forces the **failure-to-acquire-a-writer** case to surface
at the helper's `?` site, not later at every `store_fact` call site.

```rust
pub struct WriterClient {
    inner: Box<dyn CognitiveMemoryOps>,
}

pub struct ReaderClient {
    inner: Box<dyn CognitiveMemoryOps>,
}

impl WriterClient {
    /// Borrow as `&dyn CognitiveMemoryOps` for passing to `load_goal_board`,
    /// `save_goal_board`, `persist_board`, etc.
    pub fn ops(&self) -> &dyn CognitiveMemoryOps { &*self.inner }
}

impl ReaderClient {
    pub fn ops(&self) -> &dyn CognitiveMemoryOps { &*self.inner }
}
```

A `ReaderClient` is intentionally **not** convertible into a `WriterClient`.
Callers that hold a `ReaderClient` and discover they need to write must
re-call `launch_writer_client` — at which point any acquisition failure is
reported up-front.

`load_goal_board` and `save_goal_board` continue to accept
`&dyn CognitiveMemoryOps`, so callers pass `client.ops()` rather than
`&*client`.

---

## `launch_writer_client`

```rust
pub fn launch_writer_client(state_root: &Path) -> SimardResult<WriterClient>
```

Returns a client that supports both reads and writes.

### Today's resolution ladder

The shipped implementation tries two writer sources in order, then a
read-only fallback:

| Tier | Source | Condition |
|------|--------|-----------|
| 1 | `RemoteCognitiveMemory::connect(socket_path_for(state_root))` | A running OODA daemon's IPC socket exists at `<state_root>/memory.sock` (or `$SIMARD_MEMORY_SOCKET` if set) |
| 2 | `LibraryCognitiveMemory::open(state_root)` | No daemon socket; this process can take the writer lock directly (after `reap_stale_open_lock`) |
| 3 (fallback) | `LibraryCognitiveMemory::open(state_root)` | Both writer attempts failed; the library has no read-only constructor at the pinned commit, so the fallback re-opens the writer handle |

Tier 3 is the **degradation point** that issue #1590's follow-up
work targets — see "Planned changes" below. (Phase 2b / #2307 history: before
the de-fork, tier 2 used `NativeCognitiveMemory::open` and tier 3 used the
native `open_read_only`; both current paths resolve to
`LibraryCognitiveMemory::open`.)

### Planned: tier 0 in-process `Arc` shortcut

For callers that share a process with the OODA daemon (the dashboard, the
OODA reflection loop), tier 0 is added in front of tiers 1–2:

| Tier | Source | Condition |
|------|--------|-----------|
| 0 (planned) | Daemon-registered in-process `Arc<dyn CognitiveMemoryOps>` | Same-process callers when the daemon has registered its writer via `register_in_process_writer` |

The OODA daemon will register its live `Arc<dyn CognitiveMemoryOps>` (the
same handle backing the IPC server) with the launcher at startup:

```rust
// src/memory_ipc/launcher.rs (planned)
static IN_PROCESS_WRITER: OnceLock<Arc<dyn CognitiveMemoryOps>> = OnceLock::new();

pub fn register_in_process_writer(writer: Arc<dyn CognitiveMemoryOps>) {
    let _ = IN_PROCESS_WRITER.set(writer);
}
```

When the dashboard (which runs inside the daemon process) calls
`launch_writer_client`, the launcher checks the `OnceLock` first. On a hit,
it wraps the `Arc` in a `WriterClient` and returns immediately — no
Unix-socket round-trip, no lock contention, and (importantly) no risk of
falling into the read-only fallback that today's tier-3 ladder still has.

Non-daemon callers (the meeting REPL, the engineer loop, CLI tools) skip
tier 0 because nothing has registered into the `OnceLock` in their process.
They proceed to tier 1 (IPC) and tier 2 (direct open) as before.

### Planned: remove the silent read-only fallback

Tier 3 (read-only fallback wrapped as `WriterClient`) is **removed** in the
follow-up. After the change, if tiers 0–2 all fail to obtain a writer, the
helper returns `Err(SimardError::RuntimeInitFailed { component:
"memory-ipc-launcher", … })`.

This matters because dashboard mutation handlers currently treat
`launch_writer_client` success as "we have a writer". When the helper
silently returns a read-only handle, `save_goal_board(&board, client.ops())`
silently no-ops at the IPC transport layer (or the underlying
`store_fact` call returns `RpcTransportError`), and the handler's HTTP
response body becomes whatever its post-write code path produces (today,
`{"status":"ok"}` for the dashboard mutation handlers). This is the
hollow-success bug class targeted by issue #1590's follow-up.

### Tier 1 → 2 transition: state-root agreement

Tier 1 (IPC) only fires when the requested `state_root` matches the
daemon's owned state root, computed via `state_root_matches_daemon`. Both
sides canonicalize their paths (resolving symlinks and `..` segments)
before comparing. If they disagree, the launcher silently skips IPC and
proceeds to tier 2 — this prevents a daemon owning a different DB from
masking the writes the caller intended for its own DB.

If tier 1 is selected and the IPC connection fails (socket exists but
`RemoteCognitiveMemory::connect` errors), the launcher logs the error to
stderr and falls through to tier 2 rather than returning early. This keeps
short-window daemon restarts (where the socket file lingers a few hundred
milliseconds) from producing spurious failures.

### Planned: defensive `is_read_only()` invariant

`CognitiveMemoryOps` gains a single defaulted method:

```rust
pub trait CognitiveMemoryOps: Send + Sync + 'static {
    // … existing methods …
    fn is_read_only(&self) -> bool { false }
}
```

The library backend (`LibraryCognitiveMemory`) leaves the default `false` — it
has no read-only constructor at the pinned commit, so it is always a writer.
The IPC client (`RemoteCognitiveMemory`) and the daemon's in-process Arc
both leave the default `false` because the daemon is the writer.

`WriterClient`'s constructor calls `assert!(!ops.is_read_only(), …)` — an
always-on assertion (not `debug_assert!`) so the invariant fails loudly
even in release builds. With tier 3 removed, this assertion exists as a
belt-and-braces guard against future regressions; tiers 0–2 all return
writer-capable handles by construction.

We chose `assert!` over `debug_assert!` deliberately: a silent degradation
to read-only is exactly the bug class this work is meant to eliminate, and
catching it in release builds is worth the negligible runtime cost of one
virtual call per `WriterClient` construction.

**Example — dashboard write handler (post-fix)**

```rust
use simard::goal_curation::{load_goal_board, save_goal_board};
use simard::memory_ipc::{launch_writer_client, default_state_root};

let state_root = default_state_root();
let client = launch_writer_client(&state_root)?;     // Err if no writer

let mut board = load_goal_board(client.ops())?;
mutate(&mut board);
save_goal_board(&board, client.ops())?;
```

After the fix, the `?` on `launch_writer_client` is load-bearing: where
today's ladder might silently downgrade and let the handler return
`{"status":"ok"}`, the post-fix ladder returns `Err`, and the HTTP handler
converts that into a 500 with the underlying error message.

---

## `open_reader_client`

```rust
pub fn open_reader_client(state_root: &Path) -> SimardResult<ReaderClient>
```

Returns a client optimised for read-only consumers. Tries two sources in
order:

| Tier | Source | Condition |
|------|--------|-----------|
| 1 | `RemoteCognitiveMemory::connect(socket_path_for(state_root))` | A running daemon's IPC socket exists at `<state_root>/memory.sock` (or `$SIMARD_MEMORY_SOCKET` if set) |
| 2 | `LibraryCognitiveMemory::open(state_root)` | No daemon; the library backend opens the store directly (preferring IPC when the daemon holds the writer to avoid lock contention) |

Read-only callers should always prefer this helper over
`launch_writer_client` because:

- It never attempts to take the writer lock, so it never contends with a
  running daemon when the IPC socket happens to be missing during a daemon
  restart.
- `open_read_only` is cheap — no WAL recovery, no lock acquisition, no
  reaper.

The returned `ReaderClient` does not carry a write capability in its type.
Calling `client.ops().store_fact(…)` will still compile (the underlying
trait object exposes the full `CognitiveMemoryOps` surface) and will fail
at runtime with `RpcTransportError`. Callers should use `WriterClient`
when they intend to write — see "Typed client wrappers" above for the
rationale.

`open_reader_client` is **not** affected by the issue-#1590 follow-up; its
ladder is unchanged.

**Example — dashboard read handler**

```rust
use simard::goal_curation::load_goal_board;
use simard::memory_ipc::{open_reader_client, default_state_root};

let client = open_reader_client(&default_state_root())?;
let board = load_goal_board(client.ops())?;
render_workboard(&board);
```

---

## State root resolution

Both helpers accept a `&Path`. The conventional way to compute that path
is:

```rust
let state_root = simard::memory_ipc::default_state_root();
```

`default_state_root()` resolves to:

1. `$SIMARD_STATE_ROOT` if set, else
2. `$HOME/.simard/state`.

The Unix-domain socket path used by the IPC tier is resolved by
`memory_ipc::socket_path_for(state_root)` and follows the same state
root as the cognitive-memory DB. Resolution order:

1. `$SIMARD_MEMORY_SOCKET` if set (explicit override, used verbatim).
2. `<state_root>/memory.sock`, where `state_root` comes from
   `default_state_root()` above (`$SIMARD_STATE_ROOT` → `$HOME/.simard/state`).

This binding is what makes `SIMARD_STATE_ROOT` actually hermetic: a
test, or an operator running a per-state-root daemon, no longer collides
with the live daemon's socket at `~/.simard/memory.sock`. Both tiers of
[`launch_writer_client`](#launch_writer_client) and the dashboard /
meeting REPL clients call the same helper, so daemon and clients always
agree on the path for a given state root. See the
[Shared socket-path contract](./simard-cli.md#shared-socket-path-contract)
in the CLI reference for the operator-visible surface, and
[How to clean a fixture leak from the live goal board](../howto/clean-fixture-leaks.md)
for the regression that motivated the change
([#1923](https://github.com/rysweet/Simard/issues/1923),
[#1925](https://github.com/rysweet/Simard/issues/1925)).

---

## Migration call-site map

The following sites use one of the two helpers in place of inline
instantiation, `FileBackedGoalStore`, or direct `goal_records.json` reads.
Rows marked **(planned)** are the consumers covered by the issue-#1590
follow-up.

| Site | Helper | Status |
|------|--------|--------|
| `engineer_loop::engineer_loop_run_inner` (top-5 read) | `launch_writer_client` | shipped (uses writer for legacy migration write-back; will move to `open_reader_client` when migration is removed) |
| Meeting REPL goal-curation flows | `open_reader_client` / `launch_writer_client` | shipped |
| Meeting REPL improvement-curation flows | `launch_writer_client` | shipped |
| Operator dashboard goals API (mutations) | `launch_writer_client` | shipped |
| Operator dashboard goals API (GET) | `open_reader_client` | shipped |
| Operator dashboard workboard / current_work / metrics | `open_reader_client` | shipped |
| `bootstrap::assembly` (`RuntimePorts.goal_store`) | `CognitiveMemoryGoalStore` (planned adapter using both helpers) | **planned** — see [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md) |
| Daemon process registers in-process writer | `register_in_process_writer` | **planned** |

`FileBackedGoalStore` itself remains in `src/goals/store.rs` as a value
type. Its only remaining production-shaped consumer after the planned
follow-up is `src/meeting_backend/mod.rs`, which constructs one through a
file path local to that module's setup. The bootstrap adapter migration is
what removes `FileBackedGoalStore` from the production goal-board
persistence path.

---

## Related reading

- [Goal board API reference](./goal-board-api.md) — the primary consumers
  of these helpers.
- [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md)
  — how the planned `RuntimePorts.goal_store` adapter wraps these helpers
  behind the `GoalStore` trait.
- [Cognitive memory RPC wire protocol](./rpc-wire-protocol.md) —
  what the IPC tier negotiates.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md)
  — the lifecycle the helpers participate in.
