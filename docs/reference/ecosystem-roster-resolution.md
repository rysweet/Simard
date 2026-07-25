---
title: Stewarded-roster resolution
description: >
  How the ecosystem-observe / merge-queue / ci-health rails resolve Simard's
  stewarded roster: an identity-curated, deploy-durable collection at
  <state_root>/identity-state/simard/stewarded_repos.toml, seeded once (install-
  first then in-tree) from prompt_assets/simard/identity/stewarded_repos.seed.toml.
  Documents resolve_curated_path, load_or_seed, resolve_roster_seed_path, the
  fail-closed-on-empty / skip-malformed loader contract in load_stewarded_roster,
  the (repo_root, state_root) wiring contract, and how to verify it.
last_updated: 2026-07-23
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ../design/ecosystem-observe.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ./recipe-brain-api.md
  - ../fail-open-audit.md
  - ./state-root-resolution.md
---

# Stewarded-roster resolution

> **Status: current.** This page is the contract for how the
> `ecosystem-observe`, merge-queue-reasoning, and ci-health rails find Simard's
> stewarded roster. The roster is **not** a committed framework file any more —
> it is an **identity-curated, deploy-durable collection** owned at
> `<state_root>/identity-state/simard/stewarded_repos.toml`. On first use it is
> **seeded** from the committed identity data
> `prompt_assets/simard/identity/stewarded_repos.seed.toml` (resolved install-
> first, then in-tree), persisted to the state root, and from then on the durable
> curated copy is the single source of truth.

The rail is otherwise unchanged: it loads the roster, applies the Overseer
cadence, spawns `recipe-runner-rs` on `ecosystem-observe.yaml`, and forwards the
agent's opaque result. See [Ecosystem Observe](../design/ecosystem-observe.md)
for the end-to-end chain. This page documents **only** roster resolution and the
loader / wiring contract around it.

---

## Why identity-curated durable state

Every Simard self-deploy re-installs `prompt_assets/` from the repo. When the
roster lived in a committed framework file
(`prompt_assets/simard/ecosystem_repos.toml`), any runtime edit Simard made to
steward a new repo was **clobbered** on the next deploy — she could not durably
steward her own roster.

The fix moves the roster out of the framework tree and into **identity-scoped
mutable state** that Simard OWNS:

- The durable roster lives under the **state root**, which `install` **never**
  overwrites (unlike `prompt_assets/`). Curation survives every self-deploy.
- The committed seed is consulted **only once** — on first use, before any
  durable file exists. After the first seed the persisted copy is authoritative;
  the seed is never read again.
- Adding or removing a stewarded repo is an agentic `add_item` / `remove_item`
  edit against the `stewarded_repos` collection, not a code or framework-file
  change.

The state root resolves via
[`state_root::simard_state_root`](./state-root-resolution.md):
`SIMARD_STATE_ROOT` → `SIMARD_HOME` → `~/.simard`.

---

## Two resolution paths

There are two distinct resolutions, and they must not be confused:

1. **The durable roster** — where the curated collection is read from and
   written to. Resolved by `resolve_curated_path` under the state root. This is
   the single source of truth on every run after first use.
2. **The first-use seed** — the committed identity data used to populate the
   durable roster the very first time. Resolved by `resolve_roster_seed_path`
   **install-first, then in-tree**, preserving the #2419 stale-deploy fix (a
   deployed daemon whose `repo_root` is a stale source checkout still finds the
   seed installed under `~/.simard`).

### Durable-roster path (`resolve_curated_path`)

```
<state_root>/identity-state/<identity>/<collection>.toml
```

For Simard's roster: `<state_root>/identity-state/simard/stewarded_repos.toml`.

`resolve_curated_path(collection, identity, state_root_override)` in
`src/identity_curated_state.rs`:

- `<state_root>` is `state_root_override` when provided (a test seam), otherwise
  `state_root::simard_state_root()`.
- `identity-state` is a compile-time constant segment
  (`IDENTITY_STATE_SUBDIR`), never derived from env/argv/file contents.
- `<identity>` and `<collection>` are validated as clean single segments
  (`[A-Za-z0-9._-]`, non-empty, no leading `-`, no `..`, no separators). A
  non-simple name is rejected with `SimardError::InvalidStateRoot`
  (path-traversal prevention). The resolver performs **no I/O**.

### Seed path (`resolve_roster_seed_path`)

The seed resolver returns the first existing candidate, in order:

1. **Install location** —
   `<home>/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml`
   (`<home>` is `home_override` when provided, otherwise `dirs::home_dir()`).
2. **In-tree checkout** —
   `<repo_root>/prompt_assets/simard/identity/stewarded_repos.seed.toml`.
3. **None** — neither candidate is a file (a first-use load then fails loud with
   `PromptAssetMissing`; see the loader contract below).

| Candidate | Path | Chosen when |
|---|---|---|
| Install (preferred) | `~/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml` | file exists |
| In-tree (fallback) | `<repo_root>/prompt_assets/simard/identity/stewarded_repos.seed.toml` | install absent, file exists |
| None | — | neither exists |

> Both resolvers **probe the filesystem only** (`is_file()`) — no shell, no
> `Command`, no network — and never panic. The seed relative dir
> (`ROSTER_SEED_RELDIR = "prompt_assets/simard/identity"`) and filename
> (`ROSTER_SEED_FILENAME = "stewarded_repos.seed.toml"`) are compile-time
> constants.

---

## API

### Generic identity-curated-state mechanism

Public API in `src/identity_curated_state.rs` (domain-agnostic — it knows
nothing about repos; another identity could carry menus or tickers):

```rust
pub const IDENTITY_STATE_SUBDIR: &str = "identity-state";
pub const DEFAULT_IDENTITY: &str = "simard";

/// One curated item. For the `stewarded_repos` collection, `key` is an
/// `owner/name` GitHub slug and `note` is a human label — but the mechanism
/// stores strings and nothing more.
pub struct CuratedItem { pub key: String, pub note: String }

/// A collection: `schema_version = N` plus a `[[item]]` array of key/note.
pub struct CuratedCollection { /* schema_version, items: Vec<CuratedItem> */ }
// CuratedCollection::keys() -> Vec<String>  (ordered keys)

/// Resolve <state_root>/identity-state/<identity>/<collection>.toml. Rejects
/// non-simple names with InvalidStateRoot. No I/O.
pub fn resolve_curated_path(
    collection: &str, identity: &str, state_root_override: Option<&Path>,
) -> SimardResult<PathBuf>;

/// Load the durable collection. Ok(None) on first use (no file yet); a corrupt
/// file is Err (never silently empty).
pub fn load(
    collection: &str, identity: &str, state_root_override: Option<&Path>,
) -> SimardResult<Option<CuratedCollection>>;

/// Atomic write (sibling .tmp then rename); creates dirs.
pub fn save(
    collection: &str, identity: &str, data: &CuratedCollection,
    state_root_override: Option<&Path>,
) -> SimardResult<()>;

/// Load the durable file, or on FIRST USE invoke `seed`, persist it, and return
/// it. After the first seed the persisted copy is authoritative — the seed is
/// never consulted again, so add/remove edits survive re-installs/re-deploys.
pub fn load_or_seed<F>(
    collection: &str, identity: &str, seed: F, state_root_override: Option<&Path>,
) -> SimardResult<CuratedCollection>
where F: FnOnce() -> SimardResult<CuratedCollection>;

/// Agentic curation primitives: upsert-by-key (preserving order) / remove-by-key,
/// then persist.
pub fn add_item(/* collection, identity, item, state_root_override */) -> SimardResult<CuratedCollection>;
pub fn remove_item(/* collection, identity, key, state_root_override */) -> SimardResult<CuratedCollection>;

/// SIMARD_IDENTITY when non-blank, else "simard".
pub fn active_identity() -> String;
```

Errors use `SimardError::PersistentStoreIo { store, action, path, reason }` (with
`store = "identity_curated_state"`) for I/O and `SimardError::InvalidStateRoot
{ path, reason }` for a rejected name.

### Roster loader (`src/overseer/ecosystem_observe.rs`)

```rust
/// The identity-scoped collection name for the stewarded roster.
pub const STEWARDED_REPOS_COLLECTION: &str = "stewarded_repos";

/// THE single source of truth for the roster. Routes through
/// identity_curated_state::load_or_seed: if the durable
/// <state_root>/identity-state/<identity>/stewarded_repos.toml exists it is used
/// verbatim; otherwise the collection is seeded from the committed seed file
/// (resolved install-first then in-tree via resolve_roster_seed_path),
/// persisted, and returned. Each item `key` is validated as `owner/name`;
/// malformed slugs are SKIPPED with a logged warning (never reach `gh`); an
/// EMPTY roster (no valid slugs) is an ERROR (fail-closed).
pub fn load_stewarded_roster(
    repo_root: &Path,
    identity: &str,
    state_root_override: Option<&Path>,
    home_override: Option<&Path>,
) -> SimardResult<Vec<String>>;

/// Convenience wrapper used by ci_health: repo_root = CARGO_MANIFEST_DIR,
/// identity = active_identity(), state_root/home = None (env-resolved).
pub fn load_stewarded_roster_from_env() -> SimardResult<Vec<String>>;

/// Still validates `owner/name`.
fn is_valid_slug(slug: &str) -> bool;
```

Seed-resolution consts and helper:

```rust
const ROSTER_SEED_FILENAME: &str = "stewarded_repos.seed.toml";
const ROSTER_SEED_RELDIR: &str = "prompt_assets/simard/identity";

/// Resolve the seed install-first, then in-tree. `home_override` keeps tests
/// hermetic; production passes None.
fn resolve_roster_seed_path(repo_root: &Path, home_override: Option<&Path>) -> Option<PathBuf>;
```

> **Removed symbols.** The old model is gone — do not reference `RosterFile`,
> `RosterEntry`, `load_ecosystem_roster`, `parse_ecosystem_roster`,
> `resolve_ecosystem_roster_path`, or `ECOSYSTEM_ROSTER_FILENAME`. The committed
> `ecosystem_repos.toml` and its `include_str!` compile-time embed in ci_health
> are deleted.

### Parameters (`load_stewarded_roster`)

| Parameter | Meaning |
|---|---|
| `repo_root` | Source checkout root — used only for the in-tree **seed** fallback on first use. |
| `identity` | The curated-state identity (usually `active_identity()`, `"simard"`). Selects `identity-state/<identity>/`. |
| `state_root_override` | Test seam for the durable roster location; production passes `None` (env-resolved state root). |
| `home_override` | Test seam for the install-first **seed** location; production passes `None`. |

---

## Loader contract (fail-closed / skip-malformed)

`load_stewarded_roster` validates the collection's item keys before any reach
`gh`:

| Condition | Behavior |
|---|---|
| Valid `owner/name` slug | Kept, in file order. |
| Malformed slug | **Skipped with a `WARN`** on target `overseer::ecosystem_observe` (never reaches `gh`). |
| No valid slugs (empty result) | **Error** — fail-closed. The caller skips the observation tick and fabricates no Problems; an empty fleet is never silently reported healthy. |
| Corrupt durable file | `Err` from `load` (`PersistentStoreIo` with `action = "parse"`) — never a phantom empty set. |
| First use, seed missing at both candidates | `Err` (`PromptAssetMissing`) from the seed loader. |

This preserves the rail's established **fail-visible / fail-closed** posture: a
fault yields "nothing actionable," never a fabricated brief.

---

## Wiring contract

The consumers now read the identity-curated roster (single source of truth):

| Consumer | Call |
|---|---|
| `build_ecosystem_observer(repo_root, state_root)` (`src/overseer/wiring.rs`) | `load_stewarded_roster(repo_root, active_identity(), Some(state_root), None)` |
| `build_merge_queue_reasoner(repo_root, state_root)` (`src/overseer/wiring.rs`) | `load_stewarded_roster(repo_root, active_identity(), Some(state_root), None)` |
| `ci_health::governed_repos()` (`src/ci_health/mod.rs`) | `load_stewarded_roster_from_env()` at **runtime** (the old `include_str!` compile-time embed is gone) |

Both `build_*` builders now take `(repo_root, state_root)`. On a load error they
emit the fail-visible `WARN` (logging the resolved `state_root`) and leave the
rail unwired for that build — the daemon **never panics** and **never silently
degrades**.

---

## Configuration

There is **no new environment variable** for the roster path itself; the durable
roster location follows the state root
([`SIMARD_STATE_ROOT` → `SIMARD_HOME` → `~/.simard`](./state-root-resolution.md)).
`SIMARD_IDENTITY` selects the identity (blank/unset ⇒ `simard`).

| What | Where | How it resolves |
|---|---|---|
| Durable roster (single source of truth) | `<state_root>/identity-state/simard/stewarded_repos.toml` | `resolve_curated_path`; `install` never overwrites it |
| First-use seed — deployed daemon | `~/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml` | `resolve_roster_seed_path` install candidate (preferred) |
| First-use seed — source checkout | `<repo_root>/prompt_assets/simard/identity/stewarded_repos.seed.toml` | in-tree fallback |

To **curate** the roster (add/remove a stewarded repo), use the `add_item` /
`remove_item` primitives against the `stewarded_repos` collection — the edit is
written to the durable state root and survives the next deploy. Editing the
committed **seed** only affects a *fresh* identity that has not yet seeded its
roster.

---

## Examples

### First use on a deployed daemon (seed → durable)

The durable roster does not exist yet, so the rail seeds it from the installed
seed file and persists it under the state root:

```
# Seed present (install-first); durable roster absent → seeded on first tick.
$ ls ~/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml
/home/azureuser/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml

# After the first ecosystem-observe tick, the durable roster exists:
$ ls ~/.simard/identity-state/simard/stewarded_repos.toml
/home/azureuser/.simard/identity-state/simard/stewarded_repos.toml
```

### Later runs (durable copy wins, survives deploy)

Once the durable file exists it is used verbatim; the seed is never consulted
again. A self-deploy re-installs `prompt_assets/` but leaves the state root
untouched, so Simard's curation persists:

```
# Simard stewards a new repo agentically (add_item upserts stewarded_repos):
#   -> ~/.simard/identity-state/simard/stewarded_repos.toml now lists it.
# A subsequent self-deploy re-installs prompt_assets/ but NOT the state root.
# The next tick reads the curated durable roster — the new repo is still there.
```

### Fail-closed on an empty roster

If the durable roster (or seed) yields no valid `owner/name` slugs,
`load_stewarded_roster` returns an error, the builder logs the fail-visible
`WARN`, and the observation pass is skipped — the daemon keeps running and
reports nothing rather than fabricating a green fleet.

---

## Verifying resolution

On a deployed host, confirm the durable roster exists (or that the seed is in
place for first use) and that the journal shows a clean wire after the next tick:

```bash
# 1. The durable, curated roster (single source of truth).
test -f ~/.simard/identity-state/simard/stewarded_repos.toml \
  && echo "durable roster present"

# 2. The first-use seed (only consulted before the durable file exists).
test -f ~/.simard/prompt_assets/simard/identity/stewarded_repos.seed.toml \
  && echo "seed present"

# 3. Watch the daemon journal for a clean wire.
journalctl --user -u simard -f | grep -i ecosystem-observe
#   Expect the ecosystem-observe recipe to run on cadence,
#   and NO "failed to load stewarded roster" line.
```

See [Watch Overseer activity](../howto/watch-overseer-activity.md) for the
broader journal-tailing workflow.

---

## Testing

The mechanism and the roster loader are covered by hermetic unit tests that use
`tempfile::tempdir()` plus `state_root_override` / `home_override`, so they never
touch the real `~/.simard`:

| Test | Setup | Expectation |
|---|---|---|
| Path layout | `resolve_curated_path("stewarded_repos", "simard", root)` | `<root>/identity-state/simard/stewarded_repos.toml` |
| Rejects traversal | `identity` / `collection` = `..`, `a/b`, `""`, `-lead` | `Err(InvalidStateRoot)` |
| Missing is None | No durable file | `load` → `Ok(None)` (first use), never an error |
| Seed once, then own | `load_or_seed` seeds, `add_item` edits, `load_or_seed` again | Returns the CURATED copy; the seed closure is never re-consulted |
| Corrupt is loud | Invalid TOML at the durable path | `load` → `Err` (never phantom empty) |
| Seed install-first (bug #2419) | `repo_root` lacks the seed but `home_override/.simard/…` has it | `resolve_roster_seed_path` returns the install path |
| Fail-closed on empty | Roster with no valid slugs | `load_stewarded_roster` → `Err` |

The #2419 regression is preserved by the **seed** resolver: a `repo_root`
without the seed must still resolve the installed copy on first use.

Gates: full `cargo test` (not `--lib` only) and
`cargo clippy --all-targets -- -D warnings` pass. No `--no-verify`, no `--admin`.

---

## Security notes

- **Constant path segments.** `IDENTITY_STATE_SUBDIR`, `ROSTER_SEED_RELDIR`, and
  `ROSTER_SEED_FILENAME` are compile-time constants — never constructed from env,
  argv, or file contents (path-traversal prevention).
- **Name validation.** `resolve_curated_path` rejects any `identity` /
  `collection` that is not a clean single segment (`InvalidStateRoot`), so a
  hostile value can never traverse out of the identity-state tree.
- **Slug validation.** `is_valid_slug` restricts each roster key to a clean
  `owner/name`; malformed slugs are skipped with a warning and never reach `gh`
  (which is invoked argv-only, `-R <slug>`).
- **Pure filesystem probes.** Both resolvers run no shell, no `Command`, and no
  network.
- **Atomic writes.** `save` writes a sibling `.tmp` then renames, so a crash
  never leaves a half-written durable roster.
- **Fail-loud, not fail-silent.** A corrupt durable file, a missing first-use
  seed, and an empty roster are all errors — never a phantom empty set that would
  report the fleet green.
- **Logs are paths only.** Fail-visible warnings log resolved paths, never file
  contents or credentials.
- **State root is operator-controlled.** The durable roster and the seed roots
  are all operator-controlled on the single-user host, consistent with the
  [state-root resolution](./state-root-resolution.md) threat model. The residual
  TOCTOU/symlink window between `is_file()` and load is LOW/accepted there.

---

## See also

- [Ecosystem Observe](../design/ecosystem-observe.md) — the full agentic
  OBSERVE→BRIEF chain and the roster's role as an allowlist. This page covers
  only roster resolution and the loader contract.
- [State-root resolution](./state-root-resolution.md) — the durable state root
  the identity-curated roster lives under, and its shared threat model.
- [Fail-open audit](../fail-open-audit.md) — the fail-visible / fail-closed
  posture this rail preserves.
- [Watch Overseer activity](../howto/watch-overseer-activity.md) — how to
  confirm the rail wires on the deployed daemon.
