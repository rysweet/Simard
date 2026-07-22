---
title: Identity-curated state
description: >
  A generic framework mechanism for identity-scoped, mutable, deploy-durable
  data. Each identity owns named datasets of ordered, value-deduped (value, note)
  items under the durable state root that install never overwrites. Documents the
  CuratedItem / CuratedList / CuratedDataStore API, the on-disk schema, the
  seed-on-first-use contract, and the path-traversal and atomic-write guarantees.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ./ecosystem-roster-resolution.md
  - ./state-root-resolution.md
  - ../concepts/identity-scoped-cognition.md
  - ../concepts/pluggable-identity.md
---

# Identity-curated state

> **Status: current.** This page is the contract for `identity_curated_state`
> (`src/identity_curated_state.rs`): the framework's **generic** mechanism for
> data an identity curates about herself — durable across self-deploys, mutable
> at runtime, seeded from the identity.

An identity is more than the framework she runs on. Like the example identities
(a Gastronome carries menus and events), Simard carries the set of repositories
she stewards. That "who am I" data should be *hers to curate*, not a committed
framework file that every self-deploy clobbers. This module is the answer: a
small, generic store where each identity owns named **datasets** of curated
items. The framework knows nothing about "rosters" or "menus" — it stores ordered,
value-deduped lists of `(value, note)` pairs and lets the caller give them
meaning.

The [governed-repo roster](./ecosystem-roster-resolution.md) is *one* dataset
(`governed_repos` for the `simard` identity) built on this mechanism.

---

## Why generic

The governed roster's original home — a git-tracked
`prompt_assets/simard/ecosystem_repos.toml` with Rust bound to it — could not be
durable: `install` re-installs `prompt_assets/` from the repo on every
self-deploy, clobbering runtime edits. The fix generalizes rather than
special-cases: instead of a bespoke "roster store", the framework provides a
**generic identity-scoped mutable-data store**. Any identity can own any named
dataset. Adding a menu for a Gastronome, a watched-handle list for a research
identity, or a second dataset for Simard needs **no new code** — just a new
`(identity, dataset)` pair and a seed.

---

## Storage layout

```
<state_root>/state/identity_state/<identity>/<dataset>.toml
```

`<state_root>` is resolved by [state-root resolution](./state-root-resolution.md)
(`SIMARD_STATE_ROOT`, else `~/.simard`). Because `install` replaces only
`~/.simard/{bin,prompt_assets,systemd}` and **never** the state root, every
identity's curated datasets survive every self-deploy.

---

## Schema

A dataset is a TOML `CuratedList`: a `schema_version` plus an ordered array of
`[[item]]` tables, each an opaque `value` and a human-readable `note`.

```toml
schema_version = 1

[[item]]
value = "rysweet/Simard"
note = "Orchestrator / self-improving engineering identity"

[[item]]
value = "rysweet/amplihack-rs"
note = "Core framework"
```

- **`schema_version`** (`u32`, currently `1`) — defaults to the current version
  when absent from a file, so older on-disk datasets load forward-compatibly.
- **`value`** (`String`) — the opaque identifying value (a repo slug, a dish
  name…). Deduped: adding a duplicate `value` is a no-op.
- **`note`** (`String`) — a human-readable description the identity may curate.
  Never interpreted by the store.

Items keep insertion order; `from_items` collapses duplicate `value`s to the
first occurrence.

---

## API

`src/identity_curated_state.rs`. All types are `pub`.

### `CuratedItem`

```rust
pub struct CuratedItem { pub value: String, pub note: String }

impl CuratedItem {
    pub fn new(value: impl Into<String>, note: impl Into<String>) -> Self;
}
```

### `CuratedList`

```rust
pub struct CuratedList { /* schema_version + ordered items */ }

impl CuratedList {
    pub fn new() -> Self;                                             // empty
    pub fn from_items(items: impl IntoIterator<Item = CuratedItem>) -> Self; // dedup by value
    pub fn values(&self) -> Vec<String>;                             // ordered values
    pub fn contains(&self, value: &str) -> bool;
    pub fn add(&mut self, value: impl Into<String>, note: impl Into<String>) -> bool; // false if dup
    pub fn remove(&mut self, value: &str) -> bool;                   // true if removed
    pub fn is_empty(&self) -> bool;
    pub fn len(&self) -> usize;
}
```

### `CuratedDataStore`

```rust
pub struct CuratedDataStore { /* root dir */ }

impl CuratedDataStore {
    pub fn resolve() -> Self;                        // <state_root>/state/identity_state
    pub fn with_root(root: impl Into<PathBuf>) -> Self; // test seam (tempdir)
    pub fn root(&self) -> &Path;

    pub fn dataset_path(&self, identity: &str, dataset: &str) -> SimardResult<PathBuf>;
    pub fn load(&self, identity: &str, dataset: &str) -> SimardResult<Option<CuratedList>>;
    pub fn save(&self, identity: &str, dataset: &str, list: &CuratedList) -> SimardResult<()>;
    pub fn load_or_seed(&self, identity: &str, dataset: &str, seed: &CuratedList)
        -> SimardResult<CuratedList>;
}
```

| Method | Contract |
|---|---|
| `resolve` | Production store under the durable state root. |
| `with_root` | Store rooted at an explicit dir — the hermetic test seam. |
| `dataset_path` | Validates both segments (see [Security](#security)) then returns `<root>/<identity>/<dataset>.toml`. |
| `load` | `Ok(Some(list))`, or `Ok(None)` when absent. An unreadable or malformed file is an **`Err`** — never silently treated as absent, so a caller never re-seeds over corrupt curation. |
| `save` | Persists **atomically** (temp file + `rename`), creating the identity dir if needed. |
| `load_or_seed` | The **seeding-from-identity** primitive: writes `seed` on first use and returns it; every later call returns the *curated durable copy*, so add/remove edits are never reverted to the seed by a redeploy. |

---

## Seed-on-first-use contract

`load_or_seed` is the crux of "seeded from identity, curated agentically,
durable":

1. **First use** — the dataset is absent. The seed (e.g. an identity's declared
   data) is written to disk and returned.
2. **Every later use** — the durable file is authoritative. The returned list is
   the identity's *curated* copy; the seed is ignored.
3. **A self-deploy** replaces `prompt_assets/` and the binary but **not** the
   state root, so step 2's curated copy survives — the whole point.

```rust
let store = CuratedDataStore::resolve();
let seed = CuratedList::from_items([CuratedItem::new("rysweet/Simard", "steward")]);

// First tick: seeds and persists.
let list = store.load_or_seed("simard", "governed_repos", &seed)?;

// Curate agentically, then persist.
let mut list = store.load("simard", "governed_repos")?.unwrap();
list.add("rysweet/new-repo", "freshly stewarded");
list.remove("rysweet/azlin");
store.save("simard", "governed_repos", &list)?;

// A later resolve (even after a redeploy) returns the curated copy, not the seed.
```

---

## Security

- **Segment validation (path traversal).** `dataset_path` validates both the
  `identity` and `dataset` segments: each must be non-empty, not `.` or `..`,
  contain no path separator (`/` or `\`), and no NUL. A caller-supplied name can
  never escape the store root. `load`/`save`/`load_or_seed` all route through it.
- **Atomic writes.** `save` writes a `*.toml.tmp` sibling then `rename`s it into
  place, so a crash mid-write never leaves a truncated or partially-serialized
  dataset.
- **Fail loud on corruption.** A malformed or unreadable dataset is a
  [`SimardError::PersistentStoreIo`] `Err`, not a silent `None` — the caller
  decides, and never blindly re-seeds over corrupt curation.
- **Deploy-durable by construction.** The store roots under the state root, which
  `install` never overwrites — see
  [state-root resolution](./state-root-resolution.md).

---

## Testing

Hermetic unit tests in `src/identity_curated_state.rs` use `with_root` against a
`tempfile::tempdir()`, so they never touch the real state root:

| Test | Expectation |
|---|---|
| `save_then_load_round_trips` | A saved list loads back identically. |
| `load_absent_dataset_is_none_not_error` | A missing dataset is `Ok(None)`. |
| `load_corrupt_dataset_is_error_not_none` | A malformed file is an `Err`. |
| `load_or_seed_persists_seed_on_first_use` | First use writes and returns the seed. |
| `load_or_seed_returns_curated_copy_not_seed_after_mutation` | Later use returns the curated copy. |
| `from_items_dedups_by_value_preserving_first_order` | Duplicate values collapse to the first. |
| `add_is_idempotent_by_value` / `remove_reports_whether_present` | Mutation semantics. |
| `datasets_are_isolated_per_identity` | Two identities' datasets do not collide. |
| `dataset_path_rejects_traversal_segments` | `..`/separator segments are rejected. |
| `schema_version_defaults_when_absent_from_file` | Forward-compatible load. |

---

## See also

- [Governed-roster resolution](./ecosystem-roster-resolution.md) — the first
  consumer: Simard's stewarded-repo roster as a `governed_repos` dataset.
- [State-root resolution](./state-root-resolution.md) — how `<state_root>` is
  resolved and why `install` never overwrites it.
- [Identity-scoped cognition](../concepts/identity-scoped-cognition.md) — how an
  identity's declared data (seed goals, target repos) projects into cognition.
- [Pluggable identity](../concepts/pluggable-identity.md) — the identity model
  this state is scoped to.
