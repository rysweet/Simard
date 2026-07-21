---
title: Ecosystem-roster resolution
description: >
  How the ecosystem-observe rail, the merge-queue reasoner, and the ci-health
  sweep resolve Simard's stewarded roster from identity-scoped, mutable,
  deploy-durable curated state (<state_root>/identity/<id>/curated/
  stewarded_repos.toml) — seeded once from Simard's identity default and curated
  through `simard roster`. Documents resolve_stewarded_roster,
  resolve_daemon_stewarded_roster, the add/remove mutation API, the generic
  identity_state mechanism, and how to verify it.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
related:
  - ../design/ecosystem-observe.md
  - ./ci-health-sweep.md
  - ./state-root-resolution.md
  - ../ecosystem-map.md
---

# Ecosystem-roster resolution

> **Status: current.** The stewarded roster — the sibling repos Simard's
> Overseer observes, whose merge queue she reasons over, and whose CI the
> `ci-health` sweep polls — is **identity-scoped, mutable, deploy-durable
> state**, not a committed framework file. It lives under the durable state root
> at `<state_root>/identity/<identity>/curated/stewarded_repos.toml`, is seeded
> once from Simard's identity default, and is thereafter curated through the
> `simard roster` CLI (or Simard's own reasoning). All three stewards resolve
> the **same** curated document — one source of truth.

## Why identity-scoped curated state, not a committed file

The roster is part of *who Simard is* — the fleet she stewards — so it belongs
to her **identity**, not to the framework code. Baking it into a committed
`prompt_assets/simard/ecosystem_repos.toml` had two problems:

1. **Not agentically mutable at runtime.** Simard could not durably add or drop a
   governed repo through her own reasoning; changing the fleet required a code
   change and a redeploy.
2. **Clobbered by self-deploy.** `install` / `self-deploy` rewrites the binary,
   the systemd units, and the installed `~/.simard/prompt_assets/` tree. Any
   runtime edit to a roster living under `prompt_assets/` would be overwritten by
   the next deploy.

The durable state root is never touched by `install` (it only writes the binary,
units, and `~/.simard/prompt_assets/`). So a roster stored under
`<state_root>/identity/<id>/curated/` survives every redeploy: **deploy-durable
by construction.**

## The generic mechanism: `identity_state`

The roster is one instance of a GENERIC capability: arbitrary named TOML
documents persisted per identity. The `crate::identity_state` module knows
nothing about "repos":

```
<state_root>/identity/<identity>/curated/<key>.toml
```

- `store_curated(state_root, identity, key, toml)` — atomic temp-write + rename.
- `load_curated(state_root, identity, key)` — returns the stored TOML, if any.
- `curated_exists(...)`, `curated_data_path(...)`, `sanitize_component(...)`.

The same mechanism could hold a different identity's different curated data (for
example a hypothetical *Gastronome* identity's `menus` document) with no code
change — the framework provides the mechanism, the identity provides the data.

## Resolving the roster

`crate::overseer::ecosystem_observe` layers the roster-typed view on top:

- **`default_simard_roster_seed_toml() -> &'static str`** — Simard's identity
  default: the baked stewarded roster (10 slugs), used ONLY to initialise the
  curated store on first use (mirrors `DEFAULT_SEED_GOALS`).
- **`resolve_stewarded_roster(state_root, identity, seed_toml) -> Result<Vec<String>, String>`**
  — load the curated document; if absent, seed it from `seed_toml`; parse +
  validate to a `Vec` of clean `owner/name` slugs.
- **`resolve_daemon_stewarded_roster(state_root)`** — resolves the identity +
  seed from the environment via `daemon_identity_and_seed()`
  (`SIMARD_IDENTITY` unset ⇒ `simard` + baked seed; set ⇒ that identity's own
  slug + EMPTY seed, so a non-Simard identity does not inherit Simard's repos).

### Fail-loud invariant

An empty roster is always an `Err`, never a silent empty pass. An empty repo
list would classify as zero actionable failures and report the whole fleet
**green** — the exact false-green the sweep exists to prevent. `remove` refuses
to delete the last repo; a malformed seed is validated before it is ever
persisted.

## Mutating the roster

Simard curates her fleet through the `simard roster` CLI, which wraps the
mutation API:

```bash
simard roster list                        # print the resolved roster
simard roster add rysweet/new-repo "note" # add a repo (idempotent)
simard roster remove rysweet/old-repo     # remove a repo (refuses last)
```

Under the hood:

- **`add_stewarded_repo(state_root, identity, seed, slug, note)`** — seeds if
  absent, appends the validated slug, persists atomically. Idempotent; rejects a
  malformed slug.
- **`remove_stewarded_repo(state_root, identity, seed, slug)`** — removes the
  slug, persists atomically. Idempotent; refuses to empty the roster.

Both return a `RosterMutation { changed, roster, summary }`.

## One source of truth for every steward

| Steward | Entry point | Resolves via |
|---|---|---|
| `ecosystem-observe` rail | `build_ecosystem_observer` | `resolve_daemon_stewarded_roster(state_root)` |
| merge-queue reasoner | `build_merge_queue_reasoner` | `resolve_daemon_stewarded_roster(state_root)` |
| `ci-health` sweep | `ci_health::governed_repos` | `resolve_stewarded_roster(state_root, "simard", seed)` |

Because all three read the same `<state_root>/identity/simard/curated/
stewarded_repos.toml`, a repo Simard adds is swept, reasoned over, and observed
on the next cycle — with no second hardcoded list to drift out of sync.

## Verifying

```bash
# The resolved roster the daemon reads (seeds on first use):
simard roster list

# The durable, deploy-safe location:
ls "$SIMARD_STATE_ROOT/identity/simard/curated/stewarded_repos.toml"
# (defaults to ~/.simard/state/... when SIMARD_STATE_ROOT is unset)

# Add and confirm durability across a (simulated) redeploy: the state root is
# never rewritten by install, so the entry persists.
simard roster add rysweet/example "trial"
simard roster list | grep rysweet/example
```

## Honoured environment

- **`SIMARD_STATE_ROOT`** — the durable state root
  (`crate::state_root::simard_state_root`); defaults to `~/.simard/state`.
- **`SIMARD_IDENTITY`** — selects the identity whose curated roster is resolved;
  unset means Simard herself (seeded from her identity default).
