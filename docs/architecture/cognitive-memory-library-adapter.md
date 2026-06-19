---
title: Library-backed Cognitive Memory (de-fork Phase 2a)
description: How Simard's CognitiveMemoryOps trait can be backed by amplihack-memory-lib's persistent CognitiveMemory through the LibraryCognitiveMemory adapter, selected behind the opt-in `library-memory` cargo feature. Native LadybugDB backend remains the unchanged default.
last_updated: 2026-06-19
owner: simard
doc_type: reference
---

# Library-backed Cognitive Memory (de-fork Phase 2a)

Simard historically maintained a **fork** of cognitive memory: a native Rust
implementation (`NativeCognitiveMemory`, over LadybugDB / the `lbug` crate) that
duplicates the 6-type model now shipped by the upstream
[`amplihack-memory-lib`](https://github.com/rysweet/amplihack-memory-lib). The
upstream library has since grown a persistent, `lbug`-backed `GraphStore` and a
pluggable `CognitiveMemory`, making it mature enough to back Simard directly.

This page documents the **Phase 2a safe-integration** result: a second
`CognitiveMemoryOps` implementor — `LibraryCognitiveMemory` — that delegates to
`amplihack_memory::CognitiveMemory`. It is **additive and opt-in**, compiled
only behind the `library-memory` cargo feature, and the native backend remains
the default. Nothing about the default build, the live daemon, or on-disk data
changes.

> **Scope** — Phase 2a proves that Simard *can* run on the library behind its
> existing memory seam, without committing to it. The full fork deletion and
> the migration of live daemon data are **deliberately out of scope** and are
> tracked for a follow-up Phase 2b. See [Out of scope](#out-of-scope-phase-2b).

For the native backend and the 6-type model itself, see
[Cognitive Memory Architecture](cognitive-memory.md).

---

## Why this exists

`Simard` and `amplihack-memory-lib` independently implement the same cognitive
memory model. Two parallel implementations of the same contract is a
maintenance hazard: bug fixes, schema changes, and durability work must be done
twice. The de-fork reconciles them onto the upstream library.

Doing that safely requires an intermediate, reviewable step that:

- introduces the library as an **alternate backend** behind the existing
  `CognitiveMemoryOps` trait, so **no call site changes**;
- keeps the native fork **compiled and default**, so existing behavior and
  durability guarantees are untouched;
- makes the library path **compile and be selectable** for parity testing;
- surfaces every API/behavior gap **loudly and explicitly** (feeding
  [amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85))
  rather than papering over them.

That intermediate step is Phase 2a, and `LibraryCognitiveMemory` is its
deliverable.

---

## Architecture

The stable seam is the `CognitiveMemoryOps` trait
(`src/cognitive_memory/mod.rs`). All ~60 memory call sites depend only on this
trait object. Phase 2a adds one new implementor and selects between
implementors at construction time.

```text
callers (~60) ── Box/Arc<dyn CognitiveMemoryOps> ──┬─ NativeCognitiveMemory    (default, fork — unchanged)
                                                    └─ LibraryCognitiveMemory   (NEW, behind `library-memory`)
                              selected in ooda_loop::bridge_factory::connect_memory()
```

(The diagram omits a third implementor that already exists: `RemoteCognitiveMemory`,
the daemon's IPC client. `connect_memory()` prefers it when a socket is live —
see [Runtime backend selection](#runtime-backend-selection-feature-gated) for how
the library backend slots into that precedence.)

`LibraryCognitiveMemory` is a thin adapter:

```rust
// src/cognitive_memory/library_adapter.rs  (compiled only with `library-memory`)
pub struct LibraryCognitiveMemory {
    inner: std::sync::Mutex<amplihack_memory::CognitiveMemory</* persistent store */>>,
}

impl CognitiveMemoryOps for LibraryCognitiveMemory {
    // each method: lock() → call the library's &mut method → convert types → SimardResult
}
```

Two structural differences between the trait and the library drive the design:

| Difference | Trait (Simard) | Library (`amplihack_memory`) | Adapter resolution |
|---|---|---|---|
| **Receiver** | `&self` + `Send + Sync` | `&mut self` writes | Wrap in `std::sync::Mutex`; lock per op. `PoisonError` → `SimardError`. |
| **Return types** | flat `Cognitive*` DTOs (`memory_cognitive.rs`) | `EpisodicMemory`, `SemanticFact`, … (carry `node_id` + `created_at`) | Thin `From`/converter functions drop/translate fields. |

A process-local `Mutex` is consistent with the native backend, which already
serializes writes with `flock`; the library adapter is therefore a single-writer
like the native path.

---

## Configuration

### Dependency pin

Phase 2a repoints the (previously dead) `amplihack-memory` dependency at the
library commit that contains the persistent store, and enables the library's
`persistent` feature:

```toml
# Cargo.toml
[dependencies]
amplihack-memory = { git = "https://github.com/rysweet/amplihack-memory-lib.git", rev = "11b7dab054c399a603b812eff8941aace70d6e07", features = ["persistent"] }
lbug = "=0.15.3"   # retained — the native backend still uses it
```

- The library's `persistent` feature compiles **LadybugDB from source**, which
  requires a working **CMake + C++ toolchain** and noticeably longer build
  times. This is why the adapter is gated (see below) — default builds must not
  pay this cost.
- Both crates pin `lbug = "=0.15.3"`, so the two `lbug`-backed stores are
  binary-compatible at the dependency level.
- `Cargo.lock` updates to reflect the repointed `rev`. This is an expected,
  in-scope side effect.
- The library exposes two durable features: `ladybug` (the unpublished
  `ladybug-graph-rs` path) and `persistent` (the published `lbug` crate). Phase
  2a uses **`persistent`**, which matches Simard's own `lbug = "=0.15.3"` pin. The
  stale `# TODO: enable ladybug feature …` comment in `Cargo.toml` should be
  removed so it does not imply the wrong feature.

### The `library-memory` cargo feature

Selection is a **compile-time cargo feature**, off by default:

```toml
# Cargo.toml
[features]
library-memory = []
```

```rust
// src/cognitive_memory/mod.rs
#[cfg(feature = "library-memory")]
mod library_adapter;
#[cfg(feature = "library-memory")]
pub use library_adapter::LibraryCognitiveMemory;
```

| Build | Backends compiled | Default backend |
|---|---|---|
| `cargo build` (default) | `NativeCognitiveMemory` only | native |
| `cargo build --features library-memory` | native **and** library adapter | native |

When the feature is **off**, the library adapter does not exist and pulls in no
LadybugDB-from-source build. The default build, the default test suite, and the
daemon are byte-for-byte unaffected.

### Runtime backend selection (feature-gated)

`connect_memory()` already has a precedence order today: if a live daemon socket
exists it returns the IPC client (`RemoteCognitiveMemory`), otherwise it opens
the direct `NativeCognitiveMemory`. Phase 2a inserts the library backend at the
**front** of that order, gated on both the cargo feature and the env var:

1. `library-memory` compiled **and** `SIMARD_COGMEM_BACKEND=library`
   → `LibraryCognitiveMemory` (direct; **bypasses the IPC socket** — there is no
   library IPC server), **else**
2. a live daemon socket exists → `RemoteCognitiveMemory` (unchanged), **else**
3. `NativeCognitiveMemory::open(state_root)` (unchanged default).

| `SIMARD_COGMEM_BACKEND` | Effect (only when built `--features library-memory`) |
|---|---|
| unset / `native` | existing IPC-then-native precedence (default) |
| `library` | `LibraryCognitiveMemory`, bypassing the socket (parity validation only) |

The variable has **no effect** in a default build (the library backend is not
compiled), and the env-reading branch is itself `#[cfg(feature = "library-memory")]`.
Like the other backends, the library path is seeded with bootstrap procedures via
`seed_bootstrap_or_log` at construction. Selecting `library` is for parity testing
and review, not production — see [Known API gaps](#known-api-gaps).

### Store location (and what it must never touch)

`LibraryCognitiveMemory::open(state_root)` constructs the library memory with
`amplihack_memory::CognitiveMemory::open_persistent(path, agent_name)`, where
`path` is a **dedicated sub-path** under the supplied `state_root` (e.g.
`state_root/cognitive`).

- In tests, `state_root` is always a `TempDir`.
- The adapter **never** opens, reads, writes, or migrates the live daemon store
  at `~/.simard/cognitive_memory.ladybug`. Phase 2a performs **no data
  migration**; the native backend continues to own live data.

---

## API reference

`LibraryCognitiveMemory` implements the full `CognitiveMemoryOps` trait by
locking its inner `Mutex` and delegating to the library's high-level
episodic / semantic / procedural / prospective / working / sensory methods,
then converting result types.

### Construction

```rust
// Only available with --features library-memory
let mem = LibraryCognitiveMemory::open(&state_root)?; // open_persistent under the hood
let mem: Box<dyn CognitiveMemoryOps> = Box::new(mem);  // same trait object as native
```

`open(state_root)` calls
`CognitiveMemory::open_persistent(state_root.join("cognitive"), agent_name)`,
where `agent_name` is sourced from the same identity/config the native backend
uses. The library rejects an empty name with `MemoryError::InvalidInput`, so the
adapter must pass a validated, non-empty agent name.

`open` maps `amplihack_memory::MemoryError` → `SimardError`, preserving the
upstream message. Note there is **no `SimardError::Memory` variant today**: Phase
2a either reuses the existing `SimardError::BridgeError(String)` /
`PersistentStoreIo { .. }`, or adds a new `SimardError::Memory(String)` variant.
A new variant must keep the enum's `#[derive(Clone, Debug, Eq, PartialEq)]`
(hence stringify the upstream error) and add a `display` match arm.

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
| `mark_episode_distilled` | — none — | **Gap.** Inherits the trait's safe no-op default; distillation degrades to a no-op under the library backend. |
| `list_undistilled_episodes` | — none — | **Gap.** Overridden to degrade *loudly*: emits a one-time warning (so a disabled backend is distinguishable from a quiet one), then returns empty. |
| `search_episodes_by_keywords` | `get_episodes(.., include_compressed = true)` + filter | Adapter recalls **all** episodes (compressed included, so consolidation sources stay recallable — matching native, whose query has no compressed filter), then filters on case-insensitive `content.contains` and caps at `limit`. |
| `search_episodes_starting_with` | `get_episodes(.., include_compressed = true)` + filter | Adapter recalls all episodes, filters on `content.starts_with`, and pairs each match with the library record's `created_at` to build the `(content, recorded_at)` return. |
| `is_read_only()` | n/a | Always `false` — the library backend is a writer (no read-only constructor). |
| `checkpoint()` | library flush (if present) | No-op when the library exposes no checkpoint. |

### Type conversion

Thin converters translate library records into Simard DTOs
(`src/memory_cognitive.rs`). Each converter maps the fields Simard models
(`content`, `source_label`, `temporal_index`, `node_id`, …), drops fields the
flat DTOs do not carry (e.g. `created_at` — which `search_episodes_starting_with`
instead reads directly to build its `(content, recorded_at)` return), and casts
widths (`priority` i32 → i64):

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
`*`. This matches the native "list everything" behavior.

---

## Documented behavioral divergences

The library is not a drop-in clone of the fork. Where its behavior legitimately
differs, Phase 2a **documents the difference rather than forcing native
semantics into the adapter**. These are the divergences the parity test
tolerates-with-assertions, all feeding
[amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85):

### `check_triggers`

| | Native | Library |
|---|---|---|
| Matching | case-sensitive whole-substring | tokenized, lowercased, keyword-overlap |
| Side effect | read-only | mutates matched status → `"triggered"` |
| Re-firing | re-fires on every call | fires once, then is `"triggered"` |

The parity test asserts the **first-fire** behavior is observably equivalent (a
matching trigger fires once) and explicitly documents the re-fire / case /
tokenization differences as a gap. The adapter does **not** try to emulate the
native re-fire semantics.

### `consolidate_episodes`

The library creates a separate `ConsolidatedEpisode` node (a `con`-prefixed id),
whereas the native backend marks the source `Episode` in place
(`compressed = 1`, `epi`-prefixed id). This changes `episodic_count`. The parity
test asserts that **a consolidation artifact exists and is recallable**,
tolerating the id-scheme and count differences. The adapter passes a
deterministic summarizer closure so the summary content is reproducible.

### `get_statistics`

The library returns `HashMap<String, usize>`; the adapter folds it into the
typed `CognitiveStatistics`. Parity asserts on the fields both backends
populate; keys the library does not emit default to `0` and are noted.

---

## Known API gaps

These trait methods have **no direct library equivalent** at the pinned commit.
The adapter handles each **without panicking**, consistent with the trait's own
contract (every one of these methods ships a documented *safe no-op* default for
backends that lack the feature) and with the approved Phase 2a design:

| Method | Why it matters | Adapter behavior in Phase 2a |
|---|---|---|
| `mark_episode_distilled` | OODA distillation flag (issue #2281, PR-B) | inherit the trait no-op default; distillation degrades to a no-op under the library backend |
| `list_undistilled_episodes` | distillation pass input | override to degrade *loudly*: emit a one-time warning, then return empty so the pass skips |
| `search_episodes_by_keywords` | keyword episode recall (#2281, PR-C) | implement in-adapter: recall **all** episodes via `get_episodes(.., include_compressed = true)`, then filter on case-insensitive `content.contains` (compressed sources stay recallable, matching native) |
| `search_episodes_starting_with` | progress-evidence `since` timestamp gate | implement in-adapter: recall all episodes via `get_episodes(.., include_compressed = true)`, then filter on `content.starts_with`, pairing each with the library record's `created_at` |

A panic (`unimplemented!`) is **deliberately avoided**. The OODA distillation
pass (`list_undistilled_episodes` + `mark_episode_distilled`) and the
progress-evidence gate (`search_episodes_starting_with`) are real call sites that
run through the same `connect_memory()` seam. Panicking there would break the
"no call-site changes / safe integration" guarantee whenever
`SIMARD_COGMEM_BACKEND=library` is set on a live run. The trait already defines a
no-op as the *contractually safe* degradation (not a hollow success), and the
native backend keeps its real implementations untouched. This is still zero-BS:
`list_undistilled_episodes` additionally emits a **one-time runtime warning** when
the gap is hit, so the degradation is loud rather than invisible on the OODA hot
path, and the distillation gap is **documented and tracked upstream**
([amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85)),
not silently hidden. Adding a `distilled` mutation/filter API upstream — and
promoting the in-adapter keyword/prefix filters into the library — is Phase 2b
work.

---

## Parity / conformance test

`src/cognitive_memory/tests_library_parity.rs` runs the **same scenarios**
against both backends, driving each through `Box<dyn CognitiveMemoryOps>` so the
test body is backend-agnostic.

- The **native** branch always runs.
- The **library** branch runs only under `#[cfg(feature = "library-memory")]`.
- All filesystem use goes through `TempDir`. The test never touches `~/.simard`.

Scenarios and assertions:

| Scenario | Asserted equivalence |
|---|---|
| Store + recall episodes | stored episode is recallable; content matches |
| Store + search facts | fact searchable by concept/keyword; confidence filter honored |
| Store + recall procedures | procedure recallable by query; steps preserved |
| Trigger first-fire | a matching prospective fires once on `check_triggers` |
| Persistence across reopen | write → drop → reopen → read round-trip returns the data |

Ids may differ between backends; the test asserts on **counts, content, and
search hits**, not on id strings. The documented divergences above are asserted
in their tolerant form. The distillation gap methods degrade to a no-op under the
library backend (asserted to return empty, **not** to panic); the keyword/prefix
search methods are asserted to honour their `content.contains` / `starts_with`
filter. No trait method panics under either backend, so the default build stays
green.

---

## Examples & tutorials

### Build and test the default (native) configuration

The default build is unchanged and must stay green:

```bash
cargo build
cargo test
```

Neither command compiles the library adapter or LadybugDB-from-source.

### Compile the library backend

```bash
# Requires CMake + a C++ toolchain (LadybugDB is built from source).
# Expect a noticeably longer first build.
cargo build --features library-memory
```

Run the parity test against both backends:

```bash
cargo test --features library-memory tests_library_parity
```

### Select the library backend at runtime (parity validation)

```bash
# Only meaningful in a build that enabled `library-memory`.
SIMARD_COGMEM_BACKEND=library cargo test --features library-memory \
  tests_library_parity::persistence_across_reopen
```

Omitting the variable (or setting `native`) keeps the default native backend.

### Walkthrough: store and recall through the trait

Because both backends share the `CognitiveMemoryOps` trait, application code is
identical regardless of which one `connect_memory()` returns:

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

Switching the underlying backend is a build/config decision
(`--features library-memory` + `SIMARD_COGMEM_BACKEND=library`); the code above
does not change.

### Persistence across reopen

```rust
// state_root is a TempDir in tests — never ~/.simard.
{
    let mem = LibraryCognitiveMemory::open(&state_root)?;
    mem.store_fact("rust", "is a systems language", 0.95, &[], "epi_seed")?;
    mem.checkpoint()?; // flush if the library exposes one; no-op otherwise
} // dropped — store closed

let reopened = LibraryCognitiveMemory::open(&state_root)?;
let hits = reopened.search_facts("systems language", 10, 0.0)?;
assert!(!hits.is_empty());
```

---

## Out of scope — Phase 2b

Phase 2a is intentionally narrow. The following are **not** part of it and are
deferred to a validated, follow-up Phase 2b (after this integration is reviewed
and [amplihack-memory-lib#86](https://github.com/rysweet/amplihack-memory-lib/pull/86)
merges):

- **Deleting the native fork** — `src/cognitive_memory/` and
  `src/memory_consolidation/` are retained and remain the default.
- **Removing `lbug`** — kept; the native backend still uses it.
- **Switching the default backend** — default stays native.
- **Migrating live daemon data** — `~/.simard/cognitive_memory.ladybug` is never
  touched; no migration runs.
- **Closing the API gaps** — the distillation no-op and the in-adapter
  keyword/prefix filters above are surfaced for upstream
  ([amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85)),
  not promoted into the library in Phase 2a.

**Rollback** is trivial: build without `--features library-memory` (the default),
or leave `SIMARD_COGMEM_BACKEND` unset. No code revert is required.

---

## Related

- [Cognitive Memory Architecture](cognitive-memory.md) — the native backend and the 6-type model (canonical)
- [Memory architecture](../memory.md) — operator-level overview
- [Bridge Pattern](bridge-pattern.md) — the backend-agnostic trait-object seam
- [amplihack-memory-lib#86](https://github.com/rysweet/amplihack-memory-lib/pull/86) — the upstream persistent `CognitiveMemory` this adapter targets
- [amplihack-memory-lib#85](https://github.com/rysweet/amplihack-memory-lib/issues/85) — tracking issue for the API gaps and behavioral divergences listed above
