---
title: Identity-scoped state (mutable curated data)
description: >
  The generic framework mechanism for durable, install-safe, agentically-curated
  data that belongs to an identity rather than to framework code. Documents
  IdentityStateStore, its on-disk layout, seed-on-first-use, durability/atomicity,
  path-traversal safety, and how Simard's governed roster is its first consumer.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
related:
  - ./ecosystem-roster-resolution.md
  - ./state-root-resolution.md
  - ../design/ecosystem-observe.md
---

# Identity-scoped state (mutable curated data)

> **Status: current.** `IdentityStateStore` (in `src/identity_state/`) is the
> generic mechanism for **mutable curated data that belongs to *who an identity
> is*, not to the framework**. It is durable (lives under the state root, which
> `install` never overwrites), atomic (crash-safe writes), and seeded once from an
> identity's built-in default. The framework stores an opaque per-identity,
> per-collection TOML document and holds **no** knowledge of what any collection
> means.

## Why this exists

An identity should be separable from the framework the way example identities
are. A hypothetical Gastronome identity carries menus and events; Simard carries
the set of repos she stewards. That curated data must be:

- **Mutable** — the identity curates it agentically at runtime (add/remove an
  item through its own reasoning), not by editing a committed source file.
- **Deploy-durable** — it lives under the durable state root, which `install`
  never rewrites. Only `~/.simard/prompt_assets/` is atomically replaced on a
  self-deploy; the state root (`state/`, `identity_state/`, `metrics/`, …) is
  left intact. A committed `prompt_assets/…` file, by contrast, is re-installed
  and **clobbered** on every deploy — the exact failure that made the governed
  roster un-stewardable before this mechanism existed.
- **Generic** — the framework stores an opaque TOML document per
  `(identity, collection)` and never interprets it. Simard stores
  `governed_repos`; another identity could store `menus`. There is **no**
  hardcoded "roster" concept in this layer.

## On-disk layout

Each `(identity, collection)` pair is one TOML file under the state root:

```text
<state_root>/identity_state/<identity>/<collection>.toml
```

For Simard's governed roster that is
`<state_root>/identity_state/simard/governed_repos.toml` (default state root:
`~/.simard`, override with `SIMARD_STATE_ROOT`).

## Seed-on-first-use

On first use a collection is **seeded** once from the identity's default — a seed
string, typically an `include_str!`'d file baked into the binary. Thereafter the
on-disk file is the single source of truth and the seed is **never consulted
again**: the identity owns and mutates its curated copy.

```text
first load  → file absent → write seed durably → return seed
later load  → file present → return curated contents (seed ignored)
save        → atomically replace with new curated contents
```

## API

```rust
pub struct IdentityStateStore { /* rooted at <state_root>/identity_state/ */ }

impl IdentityStateStore {
    pub fn new(state_root: &Path) -> Self;

    // Path for a collection, or Err if identity/collection is not a safe segment.
    pub fn collection_path(&self, identity: &str, collection: &str) -> SimardResult<PathBuf>;

    // Raw seed-on-first-use load / atomic save.
    pub fn load_or_seed_raw(&self, identity: &str, collection: &str, seed: &str) -> SimardResult<String>;
    pub fn save_raw(&self, identity: &str, collection: &str, body: &str) -> SimardResult<()>;

    // Typed convenience over the raw pair (T: serde).
    pub fn load_or_seed<T: DeserializeOwned>(&self, identity: &str, collection: &str, seed_toml: &str) -> SimardResult<T>;
    pub fn save<T: Serialize>(&self, identity: &str, collection: &str, value: &T) -> SimardResult<()>;
}
```

The store is **generic over the document shape**: different identities carry
differently-typed data through the same mechanism (the tests round-trip both a
roster-shaped document and an unrelated `menus` document).

## Durability & safety

- **Atomic writes.** All writes go through `persistence::persist_bytes`
  (temp-write → fsync → atomic rename → parent-dir fsync, owner-only `0600`
  perms), so a reader never sees a torn document and a crash never leaves a
  half-written collection.
- **Path-traversal safety.** Identity and collection names are validated as a
  single safe path segment (`[A-Za-z0-9._-]`, non-empty, not `.`/`..`, no
  separators), so a name can never escape the store root. Invalid names are a
  loud `Err`.
- **Never panics.** I/O and (de)serialization failures return
  `SimardError::PersistentStoreIo` with store/action/path attribution.

## First consumer: Simard's governed roster

`overseer::ecosystem_observe::load_governed_roster(state_root)` is the first
consumer. It calls `load_or_seed_raw("simard", "governed_repos", SEED)` — where
`SEED` is `include_str!("../identity/seeds/simard_governed_repos.toml")` — then
parses/validates the TOML into `owner/name` slugs. See
[Governed-roster resolution](./ecosystem-roster-resolution.md) for that consumer
in full. The `ecosystem-observe` rail, the observe-merge-queue reasoner, and the
CI-health sweep all resolve the roster through that single loader, so the fleet
has exactly one source of truth.

## Adding a new identity-scoped collection

1. Add a seed file under `src/identity/seeds/` and `include_str!` it.
2. Pick a stable `(identity, collection)` name pair (compile-time constants).
3. Resolve it with `IdentityStateStore::new(state_root).load_or_seed*(…)`; write
   curation edits with `save*`.
4. Thread `state_root` to the call site (it is already available throughout the
   Overseer wiring).

No framework change is needed to teach the store about the new collection — it is
opaque to the mechanism.

## Testing

Hermetic unit tests in `src/identity_state/mod.rs` cover seed-on-first-use,
curated-copy-wins-over-seed, per-identity/per-collection namespacing, a generic
typed round-trip for an arbitrary shape, path-traversal rejection, and corrupt
body → typed-load error. They use `tempfile::tempdir()` as the state root, so
they never touch the ambient `~/.simard`.

## See also

- [Governed-roster resolution](./ecosystem-roster-resolution.md) — the first
  consumer.
- [State-root resolution](./state-root-resolution.md) — the durable root this
  store lives under and its threat model.
