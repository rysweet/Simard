---
title: Creative-ideas goal routing — fail-closed persistence
description: >
  How a creative idea accepted for implementation is routed to a durable,
  visible GoalRecord tagged source:creative-ideas, with every persistence seam
  on the path made fail-closed. Documents the #2896 fix for silent goal loss:
  the in-process GoalStoreFactory that reuses the daemon's live memory handle
  (InProcessGoalStore + put_via_ops / list_via_ops), the fail-closed
  CognitiveMemoryGoalStore::list read path (no more Ok(empty) on transport
  error), the fail-closed launch_writer_client / open_reader_client tier-1
  contract (a live-daemon connect/transport failure is Err, never a silent
  fall-through to a divergent direct-open handle), the shared write seam,
  socket/state-root permissions, the hermetic regression tests, and the live
  operator validation. Fixes issue #2896.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ./creative-ideas-api.md
  - ./creative-ideas-durable-read-after-write.md
  - ./cognitive-memory-client-helpers.md
  - ./goal-labels.md
  - ./cognitive-memory-goal-store.md
  - ./goal-board-api.md
  - ../concepts/goal-board-persistence.md
  - ../design/creative-ideas-thread.md
  - ../howto/diagnose-lost-creative-ideas-goals.md
  - ../howto/troubleshoot-goal-store.md
---

# Creative-ideas goal routing — fail-closed persistence

> **Issue [#2896](https://github.com/rysweet/Simard/issues/2896).** When the
> creative-ideas thread accepted an idea for implementation it routed the idea
> to a goal, the daemon telemetry counted the route as a success
> (`N → goal`, `0 review error(s)`), and `put()` returned `Ok` — yet
> `simard goal list --tag source:creative-ideas` returned **zero** goals over
> two days of runs. The goals were **silently lost**. This page documents the
> shipped fix, which makes every persistence seam on the creative-ideas → goal
> write path **fail-closed**: a dropped write now surfaces as a route/review
> error instead of a phantom success, and an in-process write lands in the
> **same** store the live `goal list` reads.

The symptom was not a UI bug, a filter bug, or a "no ideas were accepted" bug.
Routing *was* invoked in non-dry-run mode, `route_idea_to_goal` built a
`GoalRecord` tagged `source:creative-ideas`, and `goals.put(record)?` returned
`Ok`. The record still never became visible. Three independent silent-failure
seams combined to turn a broken write into a counted success:

| # | Seam | Symptom | Fix |
|---|------|---------|-----|
| **1** | `CognitiveMemoryGoalStore::list_via_reader` swallowed both a reader-open error **and** a `search_facts` error, returning `Ok(Vec::new())`. | A transport error on **read** looked identical to a genuinely empty store, so a persisted goal could be invisible. | [Fail-closed read path](#fix-1-fail-closed-read-path). Distinguish *empty store* from *transport error*; propagate the error as `Err`. |
| **2** | `launch_writer_client` / `open_reader_client` tier-1: when the daemon socket was **present but the connection failed** (the "Broken pipe" logs), the launcher `eprintln!`d and **fell through to a tier-2 direct open** — a *different* store view than the daemon's live in-process handle. | An in-process write went to a divergent on-disk handle the daemon never read back; `put()` still returned `Ok`. | [Fail-closed launcher](#fix-2-fail-closed-launcher-tier-1). A live-daemon connect/transport failure is now `Err`; tier-2 is reserved for the genuine no-daemon case. |
| **3** | The creative-ideas goal store opened a **fresh handle keyed on `state_root`** on every call, instead of reusing the daemon's live in-process `Arc`, so a tier-0 miss silently degraded to the socket/tier-2 path from seam #2. | Even without a transport error, the write could land in a store the live `goal list` did not read. | [In-process visibility](#fix-3-in-process-store-visibility). The factory reuses the daemon's `ctx.memory` handle so the write is visible **by construction**. |

The tagging surface itself (`labels`, the `source:creative-ideas` provenance
tag, and `simard goal list --tag`) already shipped in issue
[#2743](https://github.com/rysweet/Simard/issues/2743) — see the
[Goal labels reference](./goal-labels.md). This fix is what makes those tagged
goals actually **persist and become visible**. No `amplihack-memory` engine
change is required; the store format is unchanged.

> **Naming constraint.** No type, struct, enum, or trait introduced by this fix
> contains the word `Bridge` (operator preference). The existing runtime strings
> that read `bridge 'memory-ipc'` are user-facing telemetry and are left
> unchanged — renaming them is out of scope for this PR.

---

## Architecture

### The two paths that must agree

```
WRITE PATH (OODA daemon, creative-ideas thread)          READ PATH (operator / dashboard / TUI)
------------------------------------------------         --------------------------------------
CreativeIdeasThread::run                                 simard goal list --tag source:creative-ideas
  AgenticIdeaPipeline::review_and_route(ctx, idea)         GoalStore::list()  (cold open on state_root)
    IdeaStatus::AcceptedForImplementation                    open_reader_client(state_root)
      let goals = self.goals.open(ctx.memory, …)?  <-- reuse the live handle
      route_idea_to_goal(idea, goals.as_ref(), now)?          tier 0: lookup_in_process_writer (daemon)
        GoalRecord{ labels:[source:creative-ideas] }          tier 1: RemoteCognitiveMemory::connect
        goals.put(record)?  ── put_via_ops(ctx.memory) ──────► SAME store the daemon serves
```

The daemon registers its live writer at bootstrap under its resolved state root
(unchanged from prior work):

```rust
// src/operator_commands_ooda/daemon/mod.rs
let state_root = state_root_override.unwrap_or_else(memory_ipc::default_state_root);
memory_ipc::register_in_process_writer(state_root.clone(), Arc::clone(&shared_mem));
```

`ctx.memory` inside the creative-ideas thread is the `&dyn CognitiveMemoryOps`
borrow of that **same** `shared_mem` — it is exactly the handle the prospective
creative-idea store already uses (and which persists correctly). Before #2896
the goal write path ignored `ctx.memory` and re-derived a handle from
`ctx.state_root`; the fix threads `ctx.memory` all the way to `put()`.

### Root cause

`route_idea_to_goal` mints the record and calls `goals.put(record)?`. Under the
pre-fix implementation `put()` re-resolved a handle from `state_root` through the
memory-IPC launcher rather than reusing `ctx.memory`. Two facts about that
launcher make the loss possible:

1. **The in-process (tier-0) shortcut is keyed on `state_root`.**
   `lookup_in_process_writer` returns the daemon's live `Arc` only when the
   caller's `state_root` *canonicalizes to the same path* as the daemon's
   registered key **and** the registered `Weak` still upgrades. When the
   creative-ideas `ctx.state_root` does not canonicalize to the daemon's
   registered root (a symlinked / overridden / non-canonical state root), tier-0
   **misses** even though the caller is in the daemon process.
2. **On a tier-0 miss the launcher tried tier-1, and `connect()` does a
   handshake write.** `RemoteCognitiveMemory::connect` sends a `Ping` frame; if
   the daemon is alive but the socket is wedged, that handshake `write_frame`
   fails with the `write-len: Broken pipe (os error 32)` transport error seen in
   the live logs. The launcher `eprintln!`d it and **silently fell through to a
   tier-2 direct-open handle addressing a divergent on-disk view**.

The write succeeded against *that* tier-2 handle, `put()` returned `Ok`,
telemetry incremented `routed_goal`, and the daemon's live `goal list` (served
from tier-0) never saw it. (Note: a Broken pipe on the *actual* `store_fact` RPC
already propagates today — `put()` calls `store_fact_with_caller_key(…)?` — so
the residual silent-loss vector is specifically this tier-2 *handle divergence*,
not a swallowed write error.) The read path then compounded the problem: any
transport hiccup on `list()` returned `Ok(Vec::new())`, so the loss was
indistinguishable from "no goals".

---

## Fix 1 — Fail-closed read path

`src/goals/cognitive_memory_store.rs`

`CognitiveMemoryGoalStore::list_via_reader` no longer converts a transport
failure into an empty result. A reader-open error and a `search_facts` error
both **propagate** as `Err`; only a genuinely empty store yields an empty list.

```rust
// AFTER (#2896) — fail-closed
fn list_via_reader(&self) -> SimardResult<Vec<GoalRecord>> {
    let reader = open_reader_client(&self.state_root)?;   // was: Err(_) => Ok(Vec::new())
    list_via_ops(reader.ops())                            // search_facts error now propagates
}
```

```rust
// BEFORE (#2896) — silent swallow (removed)
let reader = match open_reader_client(&self.state_root) {
    Ok(r) => r,
    Err(_) => return Ok(Vec::new()),                     // <-- transport error looked empty
};
let facts = match reader.ops().search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0) {
    Ok(f) => f,
    Err(e) => {
        eprintln!("… search_facts failed ({e}) — returning empty record set");
        return Ok(Vec::new());                           // <-- and so did a search error
    }
};
```

| Condition | Before | After |
|-----------|--------|-------|
| Store opened, no goal facts present | `Ok([])` | `Ok([])` *(unchanged — genuinely empty)* |
| Reader-open transport error | `Ok([])` | **`Err`** |
| `search_facts` transport error | `Ok([])` (+ `eprintln!`) | **`Err`** |
| An individual fact fails to deserialize | skipped (data-quality) | skipped (data-quality) *(unchanged)* |

> **Empty vs. error is the whole point.** A per-record deserialize failure is a
> *data-quality* skip and stays a skip — it does not fail the whole list. A
> *transport* failure is an infrastructure fault and is now surfaced, so a lost
> write can never masquerade as an empty board.

The stray `eprintln!` is removed; the error travels as a typed `SimardError`.

---

## Fix 2 — Fail-closed launcher tier-1

`src/memory_ipc/launcher.rs`

Both `launch_writer_client` and `open_reader_client` share a single tier-1
contract. When the resolved socket for `state_root` **exists** but the
connection cannot be established — for **any** reason: a non-socket file
occupying the path, a refused connection, a broken pipe on the handshake, or a
failed `Ping` — the launcher **returns `Err`**. It must never fall through to a
divergent tier-2 direct-open handle, because a same-process daemon writes and
reads through that socket and a tier-2 handle would address a *different* store —
the silent-loss path #2896 forbids. Only a **genuinely absent** socket (no
daemon) takes the legitimate tier-2 path.

```rust
// src/memory_ipc/launcher.rs — launch_writer_client (post-#2896)
let sock = socket_path_for(state_root);
if sock.exists() {
    // Fail closed: a socket present but unconnectable means a daemon *should*
    // own this store. Surface the error rather than diverging to tier-2.
    let client = RemoteCognitiveMemory::connect(&sock).map_err(|e| {
        SimardError::RpcSpawnFailed {
            bridge: "memory-ipc".into(),
            reason: format!(
                "writer: socket {} present but unconnectable ({e}); refusing to fall \
                 through to a divergent direct open (bug #2896)",
                sock.display()
            ),
        }
    })?;
    return Ok(WriterClient::checked_new(Box::new(client)));
}
// (no socket at all → genuine no-daemon path → tier-2 direct open below)
```

`open_reader_client` applies the identical rule. The
`eprintln!("… falling back to direct open")` lines are removed from both helpers
— the failure is now a typed `Err`, not a log-and-degrade.

> **Why fail closed even for a leftover socket file?** A socket file only
> survives a daemon that did not shut down cleanly (the server unlinks its
> socket on `Drop`, and again before every re-bind). While such a file is
> present, a write that silently diverged to tier-2 would be invisible to the
> daemon that is expected to own the store — precisely the #2896 failure.
> Erroring loudly surfaces the wedged/leftover-socket condition; the daemon's
> next re-bind removes the file and restores the tier-1 path. A genuinely
> daemon-free host has **no** socket file and takes tier-2 unchanged. This is
> the fail-closed mandate of #2896: no silent fallback when the socket says a
> daemon should be there.

### Tier ladder (post-fix)

| Tier | Source | Condition |
|------|--------|-----------|
| 0 | Daemon-registered in-process `Arc<dyn CognitiveMemoryOps>` | Same-process caller; `state_root` canonicalizes to the registered key |
| 1 | `RemoteCognitiveMemory::connect(socket_path_for(state_root))` | Socket exists. **Any** connect / handshake / transport failure here is now **`Err`** (was: silent fall-through to tier-2). |
| 2 | `shared_tier2_store(state_root)` direct open | **No** socket present (genuine no-daemon / hermetic test / standalone CLI) |

Tiers 0 and 2 are unchanged. Tier 1's *failure* semantics are what changed:
`socket present + connect fails` was the silent-loss path and is now closed.
Hermetic `TempDir` tests (which have **no** socket) still take tier-2 unchanged,
so the fix does not perturb the isolated test suites.

> This is a strengthening of the existing "no read-only fallback" contract
> documented in [Cognitive-memory client helpers](./cognitive-memory-client-helpers.md#launch_writer_client).
> That contract already refused to hand back a read-only writer; #2896 extends
> it so a *live-daemon transport failure* also refuses to silently pick a
> divergent store view.

---

## Fix 3 — In-process store visibility

`src/creative_ideas/pipeline.rs`, `src/goals/cognitive_memory_store.rs`

The creative-ideas goal write now reuses the daemon's **live** memory handle
(`ctx.memory`), the same one the daemon serves `goal list` from — so the write
is visible by construction, exactly as the prospective creative-idea store
already is. The path-based `CognitiveMemoryGoalStore` is retained for the
**cold-open CLI** case (`simard goal …` with no daemon in-process).

### `GoalStoreFactory::open` — new signature

```rust
// src/creative_ideas/pipeline.rs
pub trait GoalStoreFactory: Send {
    /// Open a goal store bound to the caller's live memory handle.
    ///
    /// `memory` is the in-process `CognitiveMemoryOps` the daemon serves reads
    /// and writes from (borrowed as `&'a dyn …`). `state_root` is retained for
    /// the cold-open path and diagnostics. The returned store borrows `memory`,
    /// so it may not outlive the borrow.
    fn open<'a>(
        &self,
        memory: &'a dyn CognitiveMemoryOps,
        state_root: &Path,
    ) -> SimardResult<Box<dyn GoalStore + 'a>>;
}
```

The default `CognitiveMemoryGoalStoreFactory` returns an in-process store that
borrows `memory` directly — no `state_root`-keyed re-derivation, no tier-0 miss:

```rust
impl GoalStoreFactory for CognitiveMemoryGoalStoreFactory {
    fn open<'a>(
        &self,
        memory: &'a dyn CognitiveMemoryOps,
        _state_root: &Path,
    ) -> SimardResult<Box<dyn GoalStore + 'a>> {
        Ok(Box::new(InProcessGoalStore::new(memory)?))
    }
}
```

### `InProcessGoalStore<'a>`

`src/goals/cognitive_memory_store.rs`

A `GoalStore` that borrows a live `&'a dyn CognitiveMemoryOps`. It mirrors how
`ProspectiveCreativeIdeaStore::new(ctx.memory)` already takes the handle, so a
routed goal lands in the same store the prospective idea did.

```rust
pub struct InProcessGoalStore<'a> {
    ops: &'a dyn CognitiveMemoryOps,
    descriptor: BackendDescriptor,
}

impl<'a> InProcessGoalStore<'a> {
    pub fn new(ops: &'a dyn CognitiveMemoryOps) -> SimardResult<Self> {
        Ok(Self {
            ops,
            descriptor: BackendDescriptor::for_runtime_type::<Self>(
                "goals::in-process-store",
                "runtime-port:goal-store:in-process",
                Freshness::now()?,
            ),
        })
    }
}

impl GoalStore for InProcessGoalStore<'_> {
    fn put(&self, record: GoalRecord) -> SimardResult<()> {
        put_via_ops(self.ops, record)
    }

    fn list(&self) -> SimardResult<Vec<GoalRecord>> {
        list_via_ops(self.ops)
    }
    // descriptor() + the top_goals_by_status / active_top_goals ranking
    // delegate through the same shared helpers as CognitiveMemoryGoalStore
}
```

### Shared `put_via_ops` / `list_via_ops` helpers

`src/goals/cognitive_memory_store.rs`

The encode / dedup-by-slug / prospective-mirror logic is extracted into two free
functions over `&dyn CognitiveMemoryOps` so that `InProcessGoalStore` and the
legacy path-based `CognitiveMemoryGoalStore` share **one** implementation and
cannot drift (preserving the #2329 caller-key dedup and the #2207 prospective
dual-write):

```rust
/// Persist `record` (dedup by slug, mirror Active goals to prospective memory).
/// Fail-closed: any store_fact / prospective error propagates as Err.
pub(crate) fn put_via_ops(ops: &dyn CognitiveMemoryOps, record: GoalRecord) -> SimardResult<()>;

/// Read + dedup all goal records. Fail-closed: a search_facts transport error
/// is Err; an empty store is Ok([]); a bad individual record is skipped.
pub(crate) fn list_via_ops(ops: &dyn CognitiveMemoryOps) -> SimardResult<Vec<GoalRecord>>;
```

`CognitiveMemoryGoalStore` re-points its `put` and `list_via_reader` at these
helpers after acquiring a client, so the cold-open CLI path and the in-process
daemon path are byte-for-byte identical in how they encode and dedup.

### Call site

`src/creative_ideas/pipeline.rs` — `AgenticIdeaPipeline::review_and_route`:

```rust
IdeaStatus::AcceptedForImplementation => {
    let goals = self.goals.open(ctx.memory, ctx.state_root)?;   // was: open(ctx.state_root)
    route_idea_to_goal(idea, goals.as_ref(), ctx.now_epoch)?;   // put() now hits the live store
    RouteOutcome::Goal
}
```

`route_idea_to_goal` is unchanged — it still builds a `Proposed` `GoalRecord`
stamped `labels: vec![SOURCE_CREATIVE_IDEAS]` and calls `goals.put(record)?`.
The difference is that `put` now targets the daemon's live store and cannot
silently drop.

---

## Configuration

No new configuration is introduced. The relevant knobs are unchanged:

| Variable | Meaning | Default |
|----------|---------|---------|
| `SIMARD_STATE_ROOT` | State root resolved by both the daemon and the CLI | `$HOME/.simard/state` |
| `SIMARD_MEMORY_SOCKET` | Explicit socket override (rare cross-mount probe) | `<state_root>/memory.sock` |
| `SIMARD_CREATIVE_IDEAS_ENABLED` | Creative-ideas thread on/off | on (opt-out) — see [creative-ideas API](./creative-ideas-api.md) |

### Socket & state-root permissions (SR-1)

As part of hardening the write seam, the launcher tightens local filesystem
permissions opportunistically:

- The state-root directory is created `0700` (owner-only).
- The Unix-domain socket is `chmod 0600` immediately after `bind` in
  `src/memory_ipc/server.rs`.

The memory IPC boundary is a **same-user, local Unix socket**; the OS
filesystem is the trust boundary. These permissions ensure the socket and state
root are not group- or world-accessible. No new authentication, authorization,
or crypto is added.

---

## Fail-closed contract (summary)

The single rule across all three fixes: **a transport/connect failure on a
persist or a read is an error, never a silently-dropped write or a
falsely-empty read.**

| Situation | Result |
|-----------|--------|
| Idea accepted, routed, and persisted to the live store | `put() → Ok`, goal visible in `list()` and `simard goal list --tag source:creative-ideas` |
| Broken pipe / write-len error on the in-flight write | `put() → Err` → surfaces as a creative-ideas **route/review error** (telemetry `review error(s)` increments), not a phantom `routed_goal` |
| Live daemon socket present but connect fails | launcher → `Err` (no divergent tier-2 handle) |
| Socket file present but unconnectable (any cause: dead/wedged daemon, non-socket file) | launcher → `Err` (fail closed; the daemon's next re-bind clears a stale file) |
| No socket present (genuine no-daemon CLI / hermetic test) | tier-2 direct open (`Ok`) |
| Reader-open or `search_facts` transport error | `list() → Err` (not `Ok([])`) |
| Store opened, no goals present | `list() → Ok([])` |
| One malformed goal fact among many | skipped; the rest are returned |

---

## Tests

All tests are hermetic (isolated `TempDir` state roots) and real.

### Visibility regression (guards #2896)

`src/creative_ideas/tests_visibility_2896.rs`

Route an `AcceptedForImplementation` `CreativeIdea` through `route_idea_to_goal`
against the real `CognitiveMemoryGoalStore`, and (in a companion test) through
the production `CognitiveMemoryGoalStoreFactory` given a live in-process
`CognitiveMemoryOps` handle; then call `GoalStore::list()` on the **same** store
/ handle and assert the routed `GoalRecord` is returned, tagged
`source:creative-ideas`. A third test injects a write-ok/read-fault backend and
asserts that a "successful" route followed by a faulted read surfaces `Err`, not
a phantom empty list.

> These tests **fail before the fix**: with the read path swallowing an injected
> transport error into `Ok([])`, the subsequent `list()` returns empty even
> though `put()` returned `Ok` — the silent-loss signature of #2896.

### Fail-closed put / list

`src/goals/tests_fail_closed_2896.rs`

A fault-injecting fake `CognitiveMemoryOps` (registered as the in-process tier-0
writer) whose `store_fact*` / `search_facts` return a simulated `Broken pipe`
transport error. Asserted through the real `CognitiveMemoryGoalStore` (which
delegates to the shared `put_via_ops` / `list_via_ops`):

- `store.put(record)` returns **`Err`** (never a silent `Ok`).
- `store.list()` and `store.active_top_goals(..)` return **`Err`** (never
  `Ok(Vec::new())`).

### Launcher tier-1 fail-closed

`src/memory_ipc/tests_launcher_fail_closed_2896.rs`

- **Socket present but unconnectable** (a non-socket file at the socket path) →
  `launch_writer_client` and `open_reader_client` return `Err` (no tier-2
  divergence).
- **No socket present** (hermetic `TempDir`) → tier-2 direct open returns `Ok`
  (the standalone / test path is preserved).
- **Live socket whose backend op errors** → the socket client op surfaces `Err`,
  never a swallowed `Ok`.

### Regression suites kept green

The existing `creative_ideas`, `goals`, and `memory_ipc` suites (and the
`tests/goal_stewardship.rs` / `tests/goal_store_flock.rs` / `tests/meeting_goals.rs`
integration suites) remain green — the change is additive and the encode/dedup
format is unchanged.

---

## Live validation (post-deploy)

The fix's headline acceptance criterion is verifiable on a running daemon:

```bash
# 1. Trigger a creative-ideas run — dashboard "Run now" (the `ci-run-btn`
#    control) or its HTTP API (there is no `simard creative-ideas` CLI
#    subcommand). The scheduled run also produces goals; its interval is
#    SIMARD_CREATIVE_IDEAS_INTERVAL_SECS (default 86400).
DASHKEY="$(cat ~/.simard/.dashkey)"
curl -s -u "operator:$DASHKEY" -X POST \
  http://localhost:8080/api/creative-ideas/run -d '{}' | jq   # {"ok":true,"report":{…}}

# 2. Any accepted idea now yields a durable, visible goal:
simard goal list --tag source:creative-ideas
#    → returns > 0 goals (was: always 0 before #2896)
```

If a memory-IPC transport error occurs during a run, it now shows up as a
creative-ideas **review/route error** in the daemon telemetry
(`… M review error(s)` with `M > 0`) rather than a phantom `N → goal` success —
the loss is loud, not silent. The `bridge 'memory-ipc': … Broken pipe` log
lines may still appear (they reflect the underlying transport condition), but
they no longer correlate with lost writes: either the write reconnects and
persists, or `put()` returns `Err` and the route is recorded as failed.

See [Diagnose and recover lost creative-ideas goals](../howto/diagnose-lost-creative-ideas-goals.md)
for the operator walkthrough.

---

## Scope and non-goals

**In scope**

- Fail-closed read path (`list_via_reader` / `list_via_ops`).
- Fail-closed launcher tier-1 for both writer and reader helpers (one shared
  seam, fixed once — so prospective-update, journal, and other in-process
  writers inherit it).
- In-process visibility for creative-ideas goal writes via `ctx.memory`.
- Socket / state-root permission hardening (SR-1).
- Hermetic regression tests.

**Explicitly out of scope**

- Eliminating the underlying `Broken pipe (os error 32)` transport condition in
  the memory-IPC layer (this fix makes it *safe*, not *absent*).
- Redesigning the tiered IPC architecture.
- Renaming the user-facing `bridge 'memory-ipc'` runtime strings, or introducing
  any new `*Bridge`-named type (operator preference).
- The `labels` / `--tag` / `source:creative-ideas` surface itself, which already
  shipped in [#2743](https://github.com/rysweet/Simard/issues/2743).

---

## Related reading

- [Creative Ideas subsystem API](./creative-ideas-api.md) — `route_idea_to_goal`,
  the `IdeaStatus` state machine, and the pipeline this write path lives in.
- [Creative Ideas durable read-after-write](./creative-ideas-durable-read-after-write.md)
  — the sibling read-after-write fix for the *prospective* creative-idea store
  (#2798); this page is the analogous fix for the *goal* store.
- [Cognitive-memory client helpers](./cognitive-memory-client-helpers.md) — the
  `launch_writer_client` / `open_reader_client` ladder and the no-silent-
  degradation contract this fix strengthens.
- [Goal labels / tags API reference](./goal-labels.md) — the `labels` field and
  the `source:creative-ideas` provenance tag (#2743).
- [Cognitive-memory goal store adapter](./cognitive-memory-goal-store.md) —
  the `CognitiveMemoryGoalStore` history and the cold-open CLI path.
- [Goal board persistence — concept](../concepts/goal-board-persistence.md).
- [Diagnose and recover lost creative-ideas goals](../howto/diagnose-lost-creative-ideas-goals.md)
  — operator troubleshooting.
