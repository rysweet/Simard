---
title: Library-backed Cognitive Memory (the sole backend)
description: How Simard's CognitiveMemoryOps trait is backed by amplihack-memory-lib's persistent CognitiveMemory through the LibraryCognitiveMemory adapter. As of de-fork Phase 2b this is the ONLY on-disk cognitive-memory backend; the native LadybugDB fork has been deleted.
last_updated: 2026-07-03
owner: simard
doc_type: reference
related:
  - ./cognitive-memory.md
  - ../memory.md
  - ../operations/cognitive-memory-durability.md
  - ./episode-distillation.md
  - ../reference/cognitive-memory-provenance.md
---

# Library-backed Cognitive Memory (the sole backend)

Simard's cognitive memory is implemented by the upstream
[`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib)
crate. Its persistent, `lbug`-backed `CognitiveMemory` is reached through a
single thin Simard adapter, **`LibraryCognitiveMemory`**, which implements the
`CognitiveMemoryOps` trait. This adapter is the **only** on-disk
cognitive-memory backend Simard ships.

> **History.** Simard once maintained a *fork* of cognitive memory — a native
> Rust implementation (`NativeCognitiveMemory`, written directly over LadybugDB
> / the `lbug` crate) that duplicated the 6-type model the library now provides.
> The de-fork retired that fork in two steps:
>
> - **Phase 2a** introduced `LibraryCognitiveMemory` as an *opt-in, additive*
>   second backend behind a `library-memory` cargo feature, proving Simard could
>   run on the library without changing the default.
> - **Phase 2b** (this page) makes the library backend the **default and sole**
>   backend, **deletes** `NativeCognitiveMemory` and every module that
>   implemented cognitive memory directly over `lbug`, and **removes the feature
>   and env gates**. There is no longer a "native vs library" choice — the
>   library is the only path.

For the 6-type model itself (sensory / working / episodic / semantic /
procedural / prospective), see
[Cognitive Memory Architecture](cognitive-memory.md).

---

## What Phase 2b changed

| Aspect | Before (Phase 2a) | Now (Phase 2b) |
|---|---|---|
| On-disk backend | `NativeCognitiveMemory` (default) | `LibraryCognitiveMemory` (only) |
| `library_adapter` module | `#[cfg(feature = "library-memory")]` | compiled **unconditionally** |
| `library-memory` cargo feature | present, off by default | **removed** |
| `SIMARD_COGMEM_BACKEND` env switch | selects native vs library | **removed** |
| Native modules (`ops`, `schema`, `fsync`, `backup`, `metrics`) | present | **deleted** (~5,000 LoC) |
| Episode distillation on the library | no-op (loud warning) | **re-enabled** — delegates to the library |
| On-disk store | `~/.simard/cognitive_memory.ladybug` (native) | `state_root/cognitive` (library `GraphStore`) |
| `lbug` direct dependency | used by the native backend | retained for **one** reader only (`src/bin/simard_tui/goals.rs`) |

The native store at `~/.simard/cognitive_memory.ladybug` is **abandoned, not
migrated**. Memory rebuilds from scratch in the new `state_root/cognitive`
store, and goals re-derive from issues. Simard never reads, migrates, or
deletes the old native file.

---

## Architecture

The stable seam is the `CognitiveMemoryOps` trait
(`src/cognitive_memory/mod.rs`). Every memory call site depends only on this
trait object, so swapping the implementation underneath requires **no call-site
changes**.

```text
callers (OODA, goals, consolidation, dashboards, bootstrap)
        │
        ▼
   Box/Arc<dyn CognitiveMemoryOps>
        │
        ├─ LibraryCognitiveMemory   (SOLE on-disk backend, this page)
        │      └─ amplihack_memory::CognitiveMemory  (persistent, lbug-backed)
        │             └─ state_root/cognitive  (LadybugDB GraphStore)
        │
        └─ RemoteCognitiveMemory    (the daemon's IPC client — unchanged)

   selected in ooda_loop::bridge_factory::connect_memory()
```

`RemoteCognitiveMemory` is the daemon's IPC client; `connect_memory()` still
prefers it when a live daemon socket exists (so writers do not contend for the
single-writer store). When no socket is present, the direct backend is always
`LibraryCognitiveMemory`. There is no third (native) implementor any more.

> **Provenance (#2325).** `LibraryCognitiveMemory` additionally overrides the
> defaulted provenance methods on `CognitiveMemoryOps` —
> `store_fact_with_provenance`, `store_procedure_with_provenance`, and
> `episodes_for_fact` — delegating to the library's `DERIVES_FROM` /
> `PROCEDURE_DERIVES_FROM` edge API (rev `758e0a7…`). The other implementors
> (`RemoteCognitiveMemory`, bridge/test stubs) inherit no-provenance defaults
> and keep compiling unchanged. See
> [Cognitive-memory provenance](../reference/cognitive-memory-provenance.md).

`LibraryCognitiveMemory` is a thin adapter:

```rust
// src/cognitive_memory/library_adapter.rs  (always compiled)
pub struct LibraryCognitiveMemory {
    inner: std::sync::Mutex<amplihack_memory::CognitiveMemory>,
}

impl CognitiveMemoryOps for LibraryCognitiveMemory {
    // each method: lock() → call the library's &mut method → convert types → SimardResult
}
```

Two structural differences between the trait and the library drive the design:

| Difference | Trait (Simard) | Library (`amplihack_memory`) | Adapter resolution |
|---|---|---|---|
| **Receiver** | `&self` + `Send + Sync` | `&mut self` writes | Wrap in `std::sync::Mutex`; lock per op. `PoisonError` → `SimardError::StoragePoisoned`. |
| **Return types** | flat `Cognitive*` DTOs (`memory_cognitive.rs`) | `EpisodicMemory`, `SemanticFact`, … (carry `node_id` + `created_at`) | Thin converter functions drop/translate fields. |

A process-local `Mutex` makes the adapter a single-writer, which matches the
single-writer discipline the OODA daemon already enforces by owning the writer
`Arc` and routing every other process through IPC.

---

## Configuration

### Dependency pin

`Cargo.toml` pins `amplihack-memory` at the Phase 2b library commit (the one
that ships the complete persistent `CognitiveMemory`, distillation API, and
conformance tests) and enables the library's `persistent` feature:

```toml
# Cargo.toml
[dependencies]
amplihack-memory = { git = "https://github.com/rysweet/amplihack-memory-lib.git", rev = "26d49bf864ac2c03b80c4ab075c4a907c51f82a8", features = ["persistent"] }

# Retained for ONE remaining direct reader: src/bin/simard_tui/goals.rs.
# Not used by the cognitive-memory backend, which goes through amplihack-memory.
lbug = "=0.17.1"
```

- The library's `persistent` feature compiles **LadybugDB from source**, which
  requires a working **CMake + C++ toolchain** and noticeably longer build
  times. Because the library backend is now the only backend, every build pays
  this cost — budget extra time for `cargo build --release --bin simard`.
- Both Simard and the library pin `lbug = "=0.17.1"`, so they share one
  LadybugDB build.

### No cargo feature, no env switch

Phase 2b **removes** the `library-memory` cargo feature and the
`SIMARD_COGMEM_BACKEND` environment variable. They no longer exist:

```toml
# Cargo.toml — the [features] entry below is GONE in Phase 2b
# library-memory = []
```

```rust
// src/cognitive_memory/mod.rs — unconditional, no cfg gate
mod library_adapter;
pub use library_adapter::LibraryCognitiveMemory;
```

Selecting a backend is no longer a build-time or runtime decision. A standard
`cargo build` compiles the library adapter and LadybugDB; the daemon, tests, and
CLI all run on the library backend.

### Runtime backend selection

`connect_memory(state_root)` (in `ooda_loop::bridge_factory`) collapses to a
**two-tier** precedence:

1. a live daemon socket exists → `RemoteCognitiveMemory` (IPC client; unchanged),
   **else**
2. `LibraryCognitiveMemory::open(state_root)` (the direct backend).

There is no env-gated front tier any more. As with the old backends, the direct
library path is seeded with bootstrap procedures via `seed_bootstrap_or_log` at
construction.

### Store location (and what it must never touch)

`LibraryCognitiveMemory::open(state_root)` constructs the library memory with
`amplihack_memory::CognitiveMemory::open_persistent(state_root.join("cognitive"), "simard")`:

```rust
pub fn open(state_root: &Path) -> SimardResult<Self> {
    let db_path = state_root.join("cognitive");   // the GraphStore directory
    let inner = CognitiveMemory::open_persistent(&db_path, "simard")
        .map_err(/* → SimardError::PersistentStoreIo */)?;
    Ok(Self { inner: Mutex::new(inner) })
}
```

- The agent name is the fixed identity `"simard"` (`LIBRARY_AGENT_NAME`). The
  library rejects an empty name with `MemoryError::InvalidInput`, so the adapter
  always passes a validated, non-empty name.
- In production, `state_root` resolves to `~/.simard` (so the store lives at
  `~/.simard/cognitive`). In tests, `state_root` is always a `TempDir`.
- The adapter **never** opens, reads, writes, or migrates the abandoned native
  store at `~/.simard/cognitive_memory.ladybug`. No data migration runs.

---

## API reference

`LibraryCognitiveMemory` implements the **full** `CognitiveMemoryOps` trait by
locking its inner `Mutex` and delegating to the library's high-level
episodic / semantic / procedural / prospective / working / sensory methods, then
converting result types.

### Construction

```rust
use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

let mem = LibraryCognitiveMemory::open(&state_root)?;   // open_persistent under the hood
let mem: Box<dyn CognitiveMemoryOps> = Box::new(mem);   // the trait object every caller uses
```

`open` maps `amplihack_memory::MemoryError` → `SimardError::PersistentStoreIo`,
preserving the upstream message and recording the store name, action, and path.

For tests there is also an in-memory constructor (drop-in for the deleted
`NativeCognitiveMemory::in_memory()`):

```rust
// Builds CognitiveMemory::new("simard") — no on-disk LadybugDB directory.
let mem = LibraryCognitiveMemory::in_memory()?;
```

### Trait method mapping

| `CognitiveMemoryOps` method | Library delegate | Notes |
|---|---|---|
| `record_sensory(modality, raw, ttl)` | `record_sensory` | TTL mapped onto the library's expiry. |
| `prune_expired_sensory()` | `prune_expired_sensory` | Returns count. |
| `push_working / get_working / clear_working` | `push_working` / `get_working` / `clear_working` | Library enforces a 20-slot cap per task. |
| `store_episode(content, label, metadata)` | `store_episode` | `metadata` folded into the library's episode payload. |
| `consolidate_episodes(batch_size)` | `consolidate_episodes::<F>(summarizer)` | Adapter supplies a deterministic concat/truncate summarizer. See [Documented divergences](#documented-behavioral-divergences). |
| `store_fact / search_facts` | `store_fact` / `search_facts` / `get_all_facts` | `min_confidence`/`limit` cast; wildcard handled (below). |
| `store_procedure / recall_procedure` | `store_procedure` / `recall_procedures` | Recall bumps `usage_count` (library `&mut`). |
| `store_prospective / check_triggers / resolve_prospective` | `store_prospective` / `check_triggers` / `resolve_prospective` | `priority` i64↔i32 cast. Trigger semantics differ (below). |
| `get_statistics()` | `get_statistics() -> HashMap<String, usize>` | Folded into the typed `CognitiveStatistics` DTO. |
| `mark_episode_distilled(node_id)` | `mark_episode_distilled(node_id) -> bool` | **Implemented (Phase 2b).** Delegates; the returned `bool` (false if the id is absent) is ignored to satisfy the trait's `Result<()>` no-payload contract. |
| `list_undistilled_episodes(limit)` | `list_undistilled_episodes(limit) -> Vec<EpisodicMemory>` | **Implemented (Phase 2b).** Delegates and converts to `CognitiveEpisode`. |
| `search_episodes_by_keywords(keywords, limit)` | `get_episodes(usize::MAX, include_compressed = true)` + filter | Recall all episodes (compressed included so consolidation sources stay recallable), filter on case-insensitive `content.contains`, short-circuit at `limit`. |
| `search_episodes_starting_with(prefix, limit)` | `get_episodes(usize::MAX, include_compressed = true)` + filter | Recall all episodes, filter on `content.starts_with`, pair each match with the library record's `created_at` to build the `(content, recorded_at)` return. |
| `is_read_only()` | n/a | Always `false` — the library backend is a writer (no read-only constructor). |
| `checkpoint()` | library `close()` (CHECKPOINT) | Issues a LadybugDB CHECKPOINT, collapsing the WAL into the main file so a subsequent reopen of the same path observes all committed writes. |

### Type conversion

Thin converters translate library records into Simard DTOs
(`src/memory_cognitive.rs`). Each converter maps the fields Simard models
(`content`, `source_label`, `temporal_index`, `node_id`, …), drops fields the
flat DTOs do not carry (e.g. `created_at` — which
`search_episodes_starting_with` instead reads directly to build its
`(content, recorded_at)` return), and casts widths (`priority` i32 → i64):

| Library type | Simard DTO |
|---|---|
| `EpisodicMemory` | `CognitiveEpisode` (`compressed` ← `compressed`) |
| `SemanticFact` | `CognitiveFact` |
| `ProceduralMemory` | `CognitiveProcedure` |
| `ProspectiveMemory` | `CognitiveProspective` (`priority` i32 → i64) |
| `WorkingMemorySlot` | `CognitiveWorkingSlot` |
| `SensoryItem` | `CognitiveSensoryItem` |
| `HashMap<String, usize>` | `CognitiveStatistics` (by well-known key; absent keys → 0) |

### Wildcard / empty query

For `search_facts`, a `"*"` or empty `query` maps to the library's **return-all**
path (`get_all_facts` / empty-token query) rather than tokenizing the literal
`*`. This preserves the historical "list everything" behavior.

---

## Episode distillation (re-enabled in Phase 2b)

The library at the pinned commit `ece725b` exposes both distillation methods on
`impl CognitiveMemory`:

- `mark_episode_distilled(&mut self, node_id: &str) -> bool`
- `list_undistilled_episodes(&self, limit: usize) -> Vec<EpisodicMemory>`

The adapter therefore implements the trait methods by **delegating** to the
library, replacing the Phase 2a no-op:

```rust
fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
    self.lock()?.mark_episode_distilled(node_id); // bool ignored (id-missing latch)
    Ok(())
}

fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
    Ok(self.lock()?
        .list_undistilled_episodes(limit as usize)
        .into_iter()
        .map(to_episode)
        .collect())
}
```

The one-time "distillation DISABLED" warning and the `distillation_warning: Once`
field are removed. The OODA distillation pass
(`distill_recent_episodes` → `list_undistilled_episodes` + `mark_episode_distilled`)
now functions on the library backend exactly as the episode-distillation design
specifies. See [Episode distillation](episode-distillation.md).

---

## Documented behavioral divergences

The library is not a byte-for-byte clone of the deleted fork. Where its behavior
legitimately differs, Simard documents the difference rather than forcing the
old native semantics into the adapter.

### `check_triggers`

| | Old native | Library (current) |
|---|---|---|
| Matching | case-sensitive whole-substring | tokenized, lowercased, keyword-overlap |
| Side effect | read-only | mutates matched status → `"triggered"` |
| Re-firing | re-fires on every call | fires once, then is `"triggered"` |

A matching trigger fires once; the adapter does **not** emulate native re-fire
semantics. Prospective-trigger consumers (the OODA objective probe) tolerate the
first-fire contract. One consequence: code that used `check_triggers` as a
**read-only existence probe** (the goal store's `reconcile_prospectives`) must
not probe-then-reuse a prospective, because probing consumes it (and the
keyword-overlap match fires on the ubiquitous token `goal`). `reconcile_prospectives`
was restructured into two phases — resolve every goal-prospective, then store one
fresh pending entry per active goal — so freshly stored entries are never
re-probed within the same pass.

### `consolidate_episodes`

The library creates a separate `ConsolidatedEpisode` node (a `con`-prefixed id)
rather than marking the source `Episode` in place. A consolidation artifact
exists and is recallable; the id scheme and the exact `episodic_count` differ
from the old fork. The adapter passes a deterministic summarizer closure so the
summary content is reproducible.

### `get_statistics`

The library returns `HashMap<String, usize>` keyed by `MemoryCategory::as_str()`;
the adapter folds it into the typed `CognitiveStatistics`. Keys the library does
not emit default to `0`.

> **Caveat (issue #2561).** At the pinned commit the library counts via reads
> (`query_nodes(...).len()`) that **swallow a query failure as an empty result**
> (`Err(_) => Vec::new()`). A transient read failure at daemon startup therefore
> surfaces to the adapter as a legitimate all-zeros `Ok(..)` rather than an
> `Err` — an "empty" reading that may be a *read failure*, not a *confirmed-empty
> store*. Any consumer that acts on emptiness (below) must treat this as a
> fail-closed decision, not a bare count.

### Fail-closed emptiness probe & auto-restore (`probe_emptiness`, #2561)

`CognitiveMemoryOps::probe_emptiness()` is the single seam that decides whether a
store is *confirmed empty* — the precondition the snapshot auto-restore path
(`memory_snapshot::auto_restore_if_empty`) uses to self-heal a fresh or
corruption-reset (#2550) store from its newest on-disk snapshot. It returns a
three-way answer via `Result<StoreEmptiness>`:

| Outcome | Meaning | Auto-restore action |
|---|---|---|
| `Ok(ConfirmedEmpty)` | read succeeded, zero memories | hydrate from snapshot |
| `Ok(NonEmpty)` | read succeeded, ≥1 memory | skip (re-import would duplicate) |
| `Err(..)` | read **failed** | **propagate; import nothing** |

The `Err` arm is the crux: a read failure is never coerced into "empty", so a
snapshot can never be layered on top of still-present-but-unreadable durable
memory (which would duplicate every memory once reads recover). The default trait
implementation derives the answer from `get_statistics()?`, so every backend that
*surfaces* its read/transport errors (the bridge and IPC clients, test mocks)
fails closed automatically.

**Residual gap.** For the direct library backend the last gap stays open until
the pinned `amplihack-memory-lib` propagates the read error it currently swallows
(see the `get_statistics` caveat above) — the complete fix is upstream, as issue
#2561 records. Until then this seam guarantees the *decision path* is fail-closed
for every backend that can surface the error and centralises the one place the
upstream error-propagating count will plug in. A Simard-side filesystem "is the
store file large?" heuristic is deliberately **not** used: after the #2550
corruption-reset this feature is meant to heal, a large-but-reset file could
wrongly block a legitimate self-heal.

### Fact recency ordering (sequence stamping)

Several Simard call sites select "the most recent fact for concept X" by taking
the lexicographically-largest `node_id` (goal-board snapshots, the goal store,
memory consolidation). That worked when fact ids were UUID-v7 (time-ordered). The
library uses **random UUID-v4** ids and only second-granularity `created_at`, so
neither reliably orders two facts written within the same second. The adapter
therefore stamps a process-wide monotonic sequence into each fact's metadata
(`_simard_seq`) at store time and folds it into the **front** of the `node_id` it
surfaces. This restores the "max `node_id` == newest" invariant for those
consumers **without** changing `search_facts` result ordering (which stays
confidence-ranked for general recall). The sequence is recovered from the maximum
already-persisted value on open so it keeps advancing across reopens.

### Procedure reinforcement on store

Simard's contract treats a duplicate `store_procedure` as an idempotent upsert
that **reinforces** (`usage_count += 1`, node count unchanged). The library only
auto-reinforces on a mutating recall, so the adapter detects the exact-name
duplicate and reinforces it after the idempotent store.

---

## In-adapter keyword / prefix search

Two trait methods have no single library call and are composed in the adapter:

| Method | Why it matters | Adapter implementation |
|---|---|---|
| `search_episodes_by_keywords` | keyword episode recall | recall **all** episodes via `get_episodes(usize::MAX, include_compressed = true)`, filter on case-insensitive `content.contains`, short-circuit at `limit` |
| `search_episodes_starting_with` | progress-evidence `since` timestamp gate | recall all episodes, filter on `content.starts_with`, pair each with the record's `created_at` |

These remain in-adapter because the library does not expose an equivalent
single-shot query at the pinned commit. Promoting them into the library is a
desirable upstream follow-up (tracked at
[amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85)).
No trait method panics or silently degrades.

---

## Conformance test

`src/cognitive_memory/tests_library_parity.rs` runs the cognitive-memory
scenarios against the library backend through `Box<dyn CognitiveMemoryOps>`, so
the test body is backend-agnostic. (Phase 2b drops the old native arm; there is
nothing else to compare against.) All filesystem use goes through `TempDir`; the
test never touches `~/.simard`.

| Scenario | Asserted behavior |
|---|---|
| Store + recall episodes | stored episode is recallable; content matches |
| Store + search facts | fact searchable by concept/keyword; confidence filter honored |
| Store + recall procedures | procedure recallable by query; steps preserved |
| Trigger first-fire | a matching prospective fires once on `check_triggers` |
| Distillation round-trip | `store_episode` → `list_undistilled_episodes` returns it → `mark_episode_distilled` → subsequent `list` excludes it |
| Persistence across reopen | write → `checkpoint()`/drop → reopen → read round-trip returns the data |

Ids may differ from the old fork; the test asserts on **counts, content, and
search hits**, not on id strings. The documented divergences above are asserted
in their tolerant form.

---

## Examples & tutorials

### Build and test

A standard build now compiles the library adapter and LadybugDB-from-source:

```bash
# Requires CMake + a C++ toolchain. Expect a longer first build.
cargo build --release --bin simard
cargo test cognitive_memory
cargo test memory_consolidation
cargo test ooda_loop
```

There are no feature flags to pass — the library backend is the default.

### Walkthrough: store and recall through the trait

Application code depends only on `CognitiveMemoryOps`, so it is identical
regardless of whether `connect_memory()` returns the direct library backend or
the IPC client:

```rust
use simard::cognitive_memory::CognitiveMemoryOps;

fn exercise(mem: &dyn CognitiveMemoryOps) -> simard::error::SimardResult<()> {
    // Episodic: store then recall
    let epi = mem.store_episode("fixed auth.rs null check; tests pass", "session", None)?;

    // Semantic: store a fact, then search (wildcard returns all)
    mem.store_fact("auth.rs", "unwrap() on line 42 can panic", 0.9, &[], &epi)?;
    let hits = mem.search_facts("auth panic", 10, 0.0)?;
    assert!(!hits.is_empty());

    // Procedural: store then recall
    mem.store_procedure(
        "ooda:advance-goal",
        &["plan: spawn engineer".into(), "result: tests pass".into()],
        &[],
    )?;
    let procs = mem.recall_procedure("advance-goal", 5)?;
    assert!(!procs.is_empty());

    // Prospective: store a trigger, then fire it once
    mem.store_prospective("watch for errors", "error", "alert", 1)?;
    let fired = mem.check_triggers("error: build failed")?;
    assert_eq!(fired.len(), 1);

    Ok(())
}
```

### Distillation round-trip

```rust
// state_root is a TempDir in tests — never ~/.simard.
let mem = LibraryCognitiveMemory::open(&state_root)?;

let id = mem.store_episode("CI was red because the lockfile drifted", "session", None)?;
let undistilled = mem.list_undistilled_episodes(50)?;
assert!(undistilled.iter().any(|e| e.node_id == id));

mem.mark_episode_distilled(&id)?;
let after = mem.list_undistilled_episodes(50)?;
assert!(!after.iter().any(|e| e.node_id == id)); // marked rows are excluded
```

### Persistence across reopen

```rust
// state_root is a TempDir in tests — never ~/.simard.
{
    let mem = LibraryCognitiveMemory::open(&state_root)?;
    mem.store_fact("rust", "is a systems language", 0.95, &[], "epi_seed")?;
    mem.checkpoint()?; // CHECKPOINT: collapse the WAL into the main file
} // dropped — store closed

let reopened = LibraryCognitiveMemory::open(&state_root)?;
let hits = reopened.search_facts("systems language", 10, 0.0)?;
assert!(!hits.is_empty());
```

---

## Durability

The library owns its own durability: writes go through LadybugDB's WAL, and
`checkpoint()` (the trait method, delegating to the library's `close()`) issues a
CHECKPOINT that collapses the WAL into the main store. The OODA shutdown sequence
calls `CognitiveMemoryOps::checkpoint` before exit, so a graceful restart
observes every committed write.

The native per-write fsync barrier, the native verified-backup loop, and the
native crash-recovery machinery are **removed** along with the fork. File-level
snapshot backups continue to be provided by the trait-based `memory_backup/`
module (which operates through `CognitiveMemoryOps`, not raw `lbug`). See
[Cognitive Memory Durability](../operations/cognitive-memory-durability.md).

---

## Remaining direct `lbug` use

After the fork is deleted, exactly one Simard file still depends on the `lbug`
crate directly: `src/bin/simard_tui/goals.rs`, which opens a read-only
`lbug::Database` to render the TUI goal-board snapshot. This is the sole reason
`lbug` remains a direct dependency in `Cargo.toml`.

That reader currently points at the **abandoned** native goal store and will
increasingly return `GoalBoard::default()` as the old file goes stale. The
documented follow-up is to migrate it to read the goal-board snapshot through the
library (e.g. `search_facts("goal-board:snapshot", …)` via the goals
reader-bridge) and then drop the direct `lbug` dependency entirely. It is
deferred here to avoid writer contention with the daemon (there is no read-only
library constructor at the pinned commit) and to keep TUI/IPC changes out of the
de-fork scope.

---

## Completed in Phase 2b (formerly out of scope)

The items Phase 2a explicitly deferred are now done:

- **Deleting the native fork** — `NativeCognitiveMemory` and the native-only
  modules (`ops.rs`, `schema.rs`, `fsync.rs`, `backup.rs`, `metrics.rs`) and
  their native-only tests are deleted (~5,000 LoC).
- **Switching the default backend** — the library backend is the default and
  only backend.
- **Closing the distillation gap** — `mark_episode_distilled` /
  `list_undistilled_episodes` are implemented against the library.

Still tracked as follow-ups:

- **Migrating the TUI goal-board reader** off direct `lbug` (see above), after
  which `lbug` can be dropped from `Cargo.toml`.
- **Promoting the in-adapter keyword/prefix episode filters** into the library
  ([amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85)).
- **A read-only library constructor**, which would let TUI/dashboard readers open
  the store without contending with the daemon writer.
