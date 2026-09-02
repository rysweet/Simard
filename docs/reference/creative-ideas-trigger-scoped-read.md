---
title: Creative Ideas trigger-scoped read
description: >
  How the Creative Ideas dashboard reader retrieves persisted ideas out of a
  live store that holds thousands of unrelated prospective memories, without the
  ideas falling outside a LIMIT window. Documents the root cause (a
  LIMIT-before-filter truncation in the prospective read path), the new
  trigger-scoped, priority-ordered primitive
  `CognitiveMemoryOps::list_prospective_by_trigger` and its four impls, the IPC
  round-trip that carries it to tier-1 readers, the `ProspectiveCreativeIdeaStore`
  read-path change, the amplihack-memory-lib pin bump that adds
  `get_prospective_by_trigger`, the fail-closed contract, and the >512-node
  regression test. Fixes issue #122.
last_updated: 2026-07-07
review_schedule: as-needed
owner: simard
doc_type: reference
status: current — shipped; the upstream `get_prospective_by_trigger` primitive
  landed in amplihack-memory-lib PR #125 and the pin is bumped (see Pin bump)
related:
  - ./creative-ideas-api.md
  - ./creative-ideas-durable-read-after-write.md
  - ./cognitive-memory-fact-recall.md
  - ./rpc-wire-protocol.md
  - ../design/creative-ideas-thread.md
  - ../howto/configure-creative-ideas-thread.md
---

# Creative Ideas trigger-scoped read

The Creative Ideas thread persists a batch of ten candidate ideas per run into
prospective memory. On a **live** brain — one that already holds thousands of
ordinary prospective (trigger → action) memories — the dashboard **Creative
Ideas** tab was empty: `GET /api/creative-ideas` returned `ideas: []` and every
status count `0`, even while journald showed the thread reporting `10 persisted`
on every tick.

This page documents the shipped fix. The symptom was **not** a persistence bug
(the ten ideas were durably written every run), **not** the read-after-write /
state-root defect fixed earlier in
[Creative Ideas durable read-after-write](./creative-ideas-durable-read-after-write.md)
(#2798), and **not** a UI or status-filter bug. It was a distinct, second
read-path defect: the reader asked the store for "up to N prospective nodes" and
**then** filtered them down to creative ideas in Rust — so the `N`-row window was
consumed by unrelated prospective facts and the ten idea nodes fell outside it.

!!! note "Status — shipped; fail-closed (#122)"
    The fix is implemented and green in CI: the trigger-scoped primitive
    `list_prospective_by_trigger`, its four `CognitiveMemoryOps` impls (one trait
    default + three production backends), the IPC request/dispatch/client wiring,
    the `ProspectiveCreativeIdeaStore` read-path change, and the `>512`-node
    regression test all land against the real code seams. The underlying
    trigger-filtered, priority-ordered query is owned upstream in
    `amplihack-memory-lib` (guideline G2) as `get_prospective_by_trigger`, which
    landed in PR #125; Simard's pin is bumped to it (see
    [Pin bump](#pin-bump)). No store-format change (v41 is unchanged). No type or
    method contains the word `Bridge` (operator preference). Live verification
    still requires a deploy plus a thread run (or the dashboard **Run now**
    button) — the daemon persists ten ideas per run.

---

## Root cause — LIMIT applied before the trigger filter

The creative-idea read path deserializes only the prospective nodes whose
`trigger_condition` is the sentinel `CREATIVE_IDEA_TRIGGER` (`"creative-idea"`).
Before this fix, `ProspectiveCreativeIdeaStore` obtained candidates with the
**unfiltered** enumerator `list_all_prospective(limit)` and applied the sentinel
filter afterward, in Rust:

```
READ PATH (before #122) — filter is too late
--------------------------------------------
GET /api/creative-ideas
  load_ideas(state_root)
    ProspectiveCreativeIdeaStore::list(IDEA_LIST_LIMIT = 512)
      all_revisions(512)
        mem.list_all_prospective(512)            <-- LIMIT 512 in the DB query
          library get_all_prospective(512)        <-- returns 512 ARBITRARY nodes
        .filter(trigger == "creative-idea")        <-- 0 survive on a live store
```

`list_all_prospective` routes to the library's `get_all_prospective(limit)`,
which bounded the **whole** prospective-node set to `limit` rows *inside the DB
query*, before any trigger predicate. On the dashboard's read window of
`IDEA_LIST_LIMIT = 512`
([`src/operator_commands_dashboard/creative_ideas.rs`](../reference/creative-ideas-api.md)),
a live store with thousands of prospective facts fills all 512 slots with
non-creative nodes; the ten creative-idea nodes sit outside the window and the
post-filter yields **zero**. On a fresh/test store with only a handful of
prospectives the bug is invisible — which is why it survived the earlier
read-after-write fix and only reproduced in production.

Raising `IDEA_LIST_LIMIT` is **not** the fix: it only moves the cliff and makes
every read enumerate and deserialize the entire prospective store. The correct
fix pushes the trigger predicate **into** the query so the `LIMIT` bounds only
matching nodes.

## The fix — a trigger-scoped, priority-ordered query

The read path now calls a new primitive that filters by trigger **in the query**
and orders by priority, so the `LIMIT` applies to *creative-idea nodes only*:

```
READ PATH (after #122) — filter is in the query
-----------------------------------------------
GET /api/creative-ideas
  load_ideas(state_root)
    ProspectiveCreativeIdeaStore::list(512)
      all_revisions(512)
        mem.list_prospective_by_trigger("creative-idea", 512)
          library get_prospective_by_trigger("creative-idea", 512)  <-- filter, THEN limit
        .filter(trigger == "creative-idea")   <-- cheap defensive guard (all pass)
```

Because the query returns at most 512 nodes that are *already* creative ideas,
the ten persisted ideas are always in the window regardless of how many
unrelated prospectives the brain holds.

### New trait method — `CognitiveMemoryOps::list_prospective_by_trigger`

Module `simard::cognitive_memory` (`src/cognitive_memory/mod.rs`). Additive; sits
next to `list_all_prospective`.

```rust
/// Return up to `limit` prospective memories whose `trigger_condition` equals
/// `trigger`, priority-ordered (highest first) — the trigger predicate is
/// applied **in the query**, so `limit` bounds only matching nodes.
///
/// Unlike `list_all_prospective` (enumerate-all, then caller-side filter), the
/// `limit` here is a bound on the *matching* set, so a caller reading a modest
/// window (e.g. the creative-idea pool) never loses its rows to unrelated
/// prospective memories that happen to sort ahead of them. A pure `&self` read:
/// it neither mutates status (unlike `check_triggers`) nor filters by content.
///
/// The default returns empty so non-library backends degrade gracefully; the
/// production impls (`LibraryCognitiveMemory`, `SharedMemory`,
/// `RemoteCognitiveMemory`) override it.
fn list_prospective_by_trigger(
    &self,
    trigger: &str,
    limit: u32,
) -> SimardResult<Vec<CognitiveProspective>> {
    let _ = (trigger, limit);
    Ok(vec![])
}
```

The default-empty body keeps every existing `impl CognitiveMemoryOps` (including
the many test mocks) compiling unchanged; only the three production backends
override it. `trigger` is borrowed (`&str`); `limit` is `u32` to match the trait's
other list methods and is widened to the library's `usize` only at the
`LibraryCognitiveMemory` boundary (`limit as usize`; lossless on 64-bit).

### Four impls (three production, one default)

| Impl | Location | Behaviour |
|---|---|---|
| Trait default | `cognitive_memory/mod.rs` | `Ok(vec![])` — covers legacy stubs and the ~10 test mocks with no code change. |
| `LibraryCognitiveMemory` | `cognitive_memory/library_adapter.rs` | Delegates to the library's `get_prospective_by_trigger(trigger, limit as usize)` under `lock()`, propagating its `Result` (mapped onto `RpcCallFailed` via `map_op_err` — **fail-closed**, never a masked empty) and mapping each `ProspectiveMemory` via `to_prospective`. Mirrors how `list_all_prospective` delegates to `get_all_prospective`. |
| `SharedMemory` | `memory_ipc/mod.rs` | Forwards to the wrapped ops: `self.0.list_prospective_by_trigger(trigger, limit)`. `open_reader_client` hands the dashboard a `SharedMemory` for **both** the tier-0 in-process daemon-writer shortcut and the tier-2 direct-open reader, and the daemon also dispatches the tier-1 socket request into this backend — so this one delegation is what actually reaches `LibraryCognitiveMemory` on every non-socket read. |
| `RemoteCognitiveMemory` | `memory_ipc/client.rs` | Issues the IPC request (below) and maps the `Prospectives` reply — so **tier-1** (socket) readers get the same trigger-scoped result as tier-0 (in-process). |

`LibraryCognitiveMemory` override (shape):

```rust
fn list_prospective_by_trigger(
    &self,
    trigger: &str,
    limit: u32,
) -> SimardResult<Vec<CognitiveProspective>> {
    Ok(self
        .lock()?
        .get_prospective_by_trigger(trigger, limit as usize)
        .map_err(|e| map_op_err("list_prospective_by_trigger", e))?
        .into_iter()
        .map(to_prospective)
        .collect())
}
```

### IPC round-trip (tier-1)

The dashboard reader may resolve to an in-process writer (tier-0) **or** a
socket client (tier-1). To make both paths identical, the new method is carried
over the memory IPC protocol (`src/memory_ipc`). It reuses the existing
`MemoryResponse::Prospectives(Vec<CognitiveProspective>)` reply variant; only a
new request variant is added.

**Request** — `src/memory_ipc/mod.rs`:

```rust
pub enum MemoryRequest {
    // ...
    /// Issue #122: trigger-scoped, priority-ordered prospective read so a
    /// bounded reader (creative-idea pool) is not truncated by unrelated
    /// prospective memories. Reply: `MemoryResponse::Prospectives`.
    ListProspectiveByTrigger { trigger: String, limit: u32 },
}
```

**Server dispatch** — `src/memory_ipc/server.rs`:

```rust
MemoryRequest::ListProspectiveByTrigger { trigger, limit } => {
    match memory.list_prospective_by_trigger(&trigger, limit) {
        Ok(v) => MemoryResponse::Prospectives(v),
        Err(e) => MemoryResponse::Error(e.to_string()),
    }
}
```

**Client** — `src/memory_ipc/client.rs` (`impl CognitiveMemoryOps for RemoteCognitiveMemory`):

```rust
fn list_prospective_by_trigger(
    &self,
    trigger: &str,
    limit: u32,
) -> SimardResult<Vec<CognitiveProspective>> {
    match self.call(MemoryRequest::ListProspectiveByTrigger {
        trigger: trigger.to_string(),
        limit,
    })? {
        MemoryResponse::Prospectives(v) => Ok(v),
        // Use the client's existing unexpected-reply convention. `ipc_err` takes
        // a `(ctx: &str, err: impl Display)` pair, not a single formatted string;
        // the established helper for a wrong reply variant is `Self::unexpected`.
        other => Err(Self::unexpected("list_prospective_by_trigger", other)),
    }
}
```

End-to-end, a dashboard read reaching the daemon over the socket now returns the
same trigger-scoped, priority-ordered nodes as an in-process read.

### `ProspectiveCreativeIdeaStore` read-path change

Module `simard::cognitive_memory::creative_idea`
(`src/cognitive_memory/creative_idea.rs`). The internal enumerator swaps the
unfiltered call for the trigger-scoped one; the public `list` / `get` and the
`latest_revision_per_idea` dedupe are unchanged:

```rust
fn all_revisions(&self, limit: u32) -> SimardResult<Vec<CreativeIdea>> {
    let nodes = self
        .mem
        .list_prospective_by_trigger(CREATIVE_IDEA_TRIGGER, limit)?;
    nodes
        .iter()
        // Redundant now that the query filters, but retained as a cheap,
        // fail-closed guard against a library regression or a mis-routed node.
        .filter(|n| n.trigger_condition == CREATIVE_IDEA_TRIGGER)
        .map(CreativeIdea::from_prospective)
        .collect()
}
```

The trailing `.filter(...)` is kept deliberately: it costs one string compare per
node and preserves the fail-closed contract if the library ever returns a
mis-tagged node. `CreativeIdea::from_prospective` independently rejects a node
whose sentinel is wrong (`InvalidCreativeIdeaRecord`), so a stray non-creative
node can never be silently deserialized as an idea.

## Library primitive (amplihack-memory-lib)

The trigger-filtered query is owned upstream (guideline G2 — the memory type and
its queries live in `amplihack-memory-lib`, not Simard). The consumed API:

```rust
// amplihack-memory-lib
impl CognitiveMemory {
    /// Prospective nodes whose trigger equals `trigger`, priority-ordered
    /// (highest first), bounded to `limit` MATCHING nodes. Fail-closed: a
    /// backend read error is propagated as `Err`, never masked as empty.
    pub fn get_prospective_by_trigger(
        &self,
        trigger: &str,
        limit: usize,
    ) -> Result<Vec<ProspectiveMemory>>;
}
```

The same release also makes `get_all_prospective` **sort-then-truncate**
(priority-order the full set before applying `limit`), so the two enumerators
agree on ordering.

### Pin bump

`Cargo.toml` bumps the `amplihack-memory` git dependency from the prior pin
(`e005a5963b38bc02610fa5b0bef7e52625dcd092`, issue #120) to the
`amplihack-memory-lib` `main` commit (PR #125's squash-merge) that adds
`get_prospective_by_trigger` — two commits ahead, no regression:

```toml
# Bump (issue #122) from e005a5963b38bc02610fa5b0bef7e52625dcd092 to the
# amplihack-memory-lib main commit that adds get_prospective_by_trigger (a
# trigger-filtered, priority-ordered prospective query) and makes
# get_all_prospective sort-then-truncate. Store format stays v41.
amplihack-memory = { git = "https://github.com/rysweet/amplihack-memory-lib.git", rev = "901f63ad79eb0c2d87cd8263d26025877af43cc5", features = ["persistent"] }
```

Run `cargo build` after the bump to refresh `Cargo.lock`. No store-format
migration is required (v41 unchanged); the change is purely a new read query plus
an ordering guarantee on an existing one.

## Fail-closed contract

The read path is **additive and fail-closed** — there is no fallback to the old
truncating behaviour and no path that silently yields an empty pool:

- The creative-idea reader calls **only** `list_prospective_by_trigger`. It never
  falls back to `list_all_prospective` + post-filter.
- A backend read error is propagated as `Err` (surfaced by the dashboard as
  `{"error": "...", "ideas": [], "counts": {}}`), never coerced into an
  "empty pool" success.
- The retained `trigger_condition == CREATIVE_IDEA_TRIGGER` guard plus
  `from_prospective`'s sentinel check keep a mis-tagged node from ever being read
  as an idea.
- The trait default is `Ok(vec![])` **only** for backends that structurally hold
  no prospective memory (test mocks, IPC stubs). The dashboard reader that
  `open_reader_client` returns is always a `SharedMemory` (tier-0 in-process
  daemon-writer shortcut, or tier-2 direct-open — each delegating to
  `LibraryCognitiveMemory`) or a `RemoteCognitiveMemory` (tier-1 socket, whose
  daemon dispatches into `LibraryCognitiveMemory`). All three production impls
  override the method with a real query; the reader never resolves to a bare
  trait-default backend.

## Regression test

`src/creative_ideas/tests.rs` (reusing the existing creative-idea harness — a
real `LibraryCognitiveMemory::in_memory()` store wrapped by
`ProspectiveCreativeIdeaStore`) adds a test that reproduces the live conditions:

1. Store **more than 512** ordinary (non-creative) prospective memories directly
   via `mem.store_prospective(desc, "some-other-trigger", action, priority)`.
2. Store a handful of creative ideas through
   `ProspectiveCreativeIdeaStore::store`.
3. Assert `store.list(512)` returns exactly the creative ideas (by `idea_id`),
   with none lost to the non-creative bulk.

This test **fails before the fix** (the 512-row window is consumed by the
non-creative nodes, so `list` returns zero ideas) and **passes after** (the
trigger-scoped query bounds only creative-idea nodes). The existing
creative-idea store/list/get/update/promote/prune tests remain green — the public
`CreativeIdeaStore` surface is unchanged.

## Dashboard HTTP contract (unchanged shape)

`GET /api/creative-ideas` is unchanged in shape; it simply returns a non-empty
pool once ideas exist. It reads the live pool (latest revision per idea, newest
first) with a per-status count summary:

```json
{
  "counts": { "New": 7, "UnderReview": 2, "AcceptedForImplementation": 1, "...": 0 },
  "ideas": [
    { "idea_id": "…", "node_id": "…", "status": "New", "idea": "…", "rationale": "…" }
  ]
}
```

`POST /api/creative-ideas/search` (status/free-text filter) reads the same pool
and likewise benefits from the trigger-scoped read. See
[Creative Ideas subsystem API reference](./creative-ideas-api.md#dashboard-http-api-operator-controls)
for the full operator surface (Run now / Promote / Prune).

## Operator verification (live)

Verify the fix in the running system after a redeploy:

1. Deploy the built binary (the reader change ships in the daemon **and** the
   dashboard process).
2. Trigger a generation run — either wait for the daily tick or press
   **Run now** on the Creative Ideas tab (`POST /api/creative-ideas/run`).
3. Reload the tab (or `curl` the endpoint):

   ```console
   $ curl -s http://127.0.0.1:PORT/api/creative-ideas | jq '.counts, (.ideas | length)'
   ```

   The pool is now non-empty — `counts` sums to ≥ 10 after one run and
   `.ideas | length` is `> 0`, on a live store regardless of how many unrelated
   prospective memories it holds.

Live verification requires a deploy plus a thread run; the daemon persists ten
ideas per run, so a single **Run now** is sufficient to observe `count > 0`.

## Constraints honoured

Additive (new method + new request variant; no signature churn on existing
callers) · fail-closed (no `list_all_prospective` fallback on the creative-idea
read; no silent empty) · no `*Bridge` names · no `println!`/`eprintln!` (tracing
only) · store format v41 unchanged · CI green · never `--admin` / `--no-verify`.

## See also

- [Creative Ideas subsystem API reference](./creative-ideas-api.md) — the full
  `CreativeIdeaStore` seam and dashboard surface.
- [Creative Ideas durable read-after-write](./creative-ideas-durable-read-after-write.md)
  — the earlier, distinct read-path fix (state-root resolver, #2798).
- [Creative Ideas background thread (design)](../design/creative-ideas-thread.md)
  — motivation, data model, and roadmap.
- [Configure and operate the Creative Ideas thread](../howto/configure-creative-ideas-thread.md).
