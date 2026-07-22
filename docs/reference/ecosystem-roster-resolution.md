---
title: Governed-roster resolution
description: >
  How Simard resolves her governed-repo roster from identity-scoped, mutable,
  deploy-durable curated state instead of a committed framework file. Documents
  the identity_curated_state store, resolve_governed_roster, the seed-on-first-use
  contract shared by the ecosystem-observe rail, the merge-queue reasoner, and the
  ci-health sweep, and how to curate and verify the roster.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ../design/ecosystem-observe.md
  - ./identity-curated-state.md
  - ./ooda-engineer-lifecycle-recipe.md
  - ../fail-open-audit.md
  - ./state-root-resolution.md
---

# Governed-roster resolution

> **Status: current.** This page is the contract for how Simard resolves the
> **governed-repo roster** — the set of `owner/name` repos she stewards. The
> roster is no longer a committed framework file (`ecosystem_repos.toml`, now
> retired). It is Simard's **identity-scoped, mutable, deploy-durable curated
> state**: a named dataset under the durable state root that `install` never
> overwrites, seeded once from Simard's identity and curated agentically
> thereafter.

The roster is data. It is resolved once per Overseer build and handed — as the
same `Vec<String>` — to every consumer: the `ecosystem-observe` rail, the
`observe-merge-queue` reasoner, and the CI-health sweep. There is **exactly one
source of truth**. See [Ecosystem Observe](../design/ecosystem-observe.md) for
the end-to-end agentic chain and
[Identity-curated state](./identity-curated-state.md) for the generic storage
mechanism.

---

## Why identity-curated state (not a committed file)

The roster used to live in `prompt_assets/simard/ecosystem_repos.toml`, a
git-tracked file that Rust loaded, parsed, and `include_str!`'d. That coupling
had a fatal flaw for a self-stewarding agent: **every self-deploy re-installs
`prompt_assets/` from the repo**, clobbering any runtime edit. Simard could not
durably add or remove a governed repo — her curation would be overwritten on the
next deploy.

Moving the roster into identity-scoped curated state fixes this at the root:

- **Durable.** `install` overwrites only `~/.simard/{bin,prompt_assets,systemd}`
  and **never** the state root. The roster lives under
  `<state_root>/state/identity_state/…`, so a self-deploy leaves it untouched.
- **Mutable.** Simard curates the roster agentically (add/remove a stewarded
  repo); the durable copy is authoritative after first use.
- **Seeded from identity.** On first use the roster is seeded from Simard's
  identity — her default roster (the repos she stewards out of the box), or a
  named identity's declared `target_repos`. The seed is *who Simard is*, like her
  default seed goals; it is not a config file.
- **Generic.** The mechanism is not roster-specific: any identity can own any
  named dataset (repos for Simard, menus for a Gastronome). See
  [Identity-curated state](./identity-curated-state.md).

---

## Storage layout

```
<state_root>/state/identity_state/<identity>/<dataset>.toml
```

The governed roster is identity `simard`, dataset `governed_repos`:

```
<state_root>/state/identity_state/simard/governed_repos.toml
```

`<state_root>` is resolved by [`state-root resolution`](./state-root-resolution.md)
(`SIMARD_STATE_ROOT`, else `~/.simard`). The file is a TOML `CuratedList`:

```toml
schema_version = 1

[[item]]
value = "rysweet/Simard"
note = "Orchestrator / self-improving engineering identity (steward of this roster)"

[[item]]
value = "rysweet/amplihack-rs"
note = "Core framework — skills, workflows, recipes, hooks, CLI, fleet"
# … one [[item]] per stewarded repo
```

`value` is the `owner/name` slug; `note` is a human-readable description Simard
may curate. See the [curated-state schema](./identity-curated-state.md#schema).

---

## Resolution

`resolve_governed_roster(store, identity, seed)` (in
`src/overseer/ecosystem_observe.rs`) is the single resolver:

1. **Load or seed.** `store.load_or_seed(identity, "governed_repos", seed)` reads
   the durable dataset. If it is absent (first use), the `seed` is written to disk
   and returned; thereafter the **durable curated copy** is authoritative — a
   later resolve (as on a redeploy) returns Simard's edits, never the seed.
2. **Validate.** Each item's `value` is checked as a clean `owner/name` slug
   (`is_valid_slug`): exactly two non-empty segments, only `[A-Za-z0-9._-]`, no
   whitespace, no `..`, no leading `-`, no shell metacharacters. A malformed slug
   is **skipped with a logged warning** — it never reaches the agent's `gh` calls.
3. **Fail loud on empty.** An empty roster — whether the dataset was empty or
   every slug was malformed — is an **`Err`**, never a silent empty pass. An empty
   fleet would classify as zero failures and report the ecosystem **green**; the
   resolver refuses to produce that false-green.

### Seed selection

`governed_roster_seed_for(identity_name, target_repos)` chooses the identity and
seed from the active identity's cognition:

| Active identity | Seed |
|---|---|
| A **named** identity with non-empty `target_repos` | That identity owns its own `governed_repos` dataset, seeded from its declared `target_repos`. |
| No identity, or a named identity with no `target_repos` | Simard (`simard`), seeded from her baked default roster (`default_simard_roster_seed`). |

This is the generic mechanism at work: a non-Simard identity stewards its own
declared scope; Simard stewards her default roster.

---

## Wiring contract (`build_overseer`)

`build_overseer` (in `src/overseer/wiring.rs`) resolves the roster **once** and
shares it:

| Step | Behavior |
|---|---|
| Resolve | `resolve_governed_roster_for_build(identity_name, target_repos)` opens the durable store (`CuratedDataStore::resolve()`), picks the seed, and resolves. |
| `Ok(roster)` | The same `Vec<String>` is cloned into the ecosystem-observe rail and the merge-queue reasoner (each also needs a live recipe-runner to wire). |
| `Err(_)` | Emit a fail-visible `WARN` on `simard::ecosystem_observe` and skip the roster-bound rails this build — the daemon **never panics** and **never silently degrades**. |

The CI-health sweep resolves the **same** durable dataset independently
(`governed_repos()` → `resolve_governed_roster` on `CuratedDataStore::resolve()`),
so the sweep and the Overseer rails can never disagree about who is governed.

The daemon passes the active identity's `identity_name` + `target_repos` (captured
from `state.identity_cognition` before the tick spawns) into `build_overseer`.

---

## Curating the roster

Adding or removing a stewarded repo is a **data** change on the durable dataset —
no code change, and it survives redeploys. Simard curates it agentically; an
operator can also edit it directly on a deployed host:

```bash
# The durable governed roster (deploy-safe; install never overwrites it).
$EDITOR ~/.simard/state/identity_state/simard/governed_repos.toml
```

Add or remove an `[[item]]` block. The next Overseer tick that builds the rail —
and the next CI-health sweep — pick it up. Because the state root is never
overwritten by `install`, the edit is durable across self-deploys.

> **The retired file.** `prompt_assets/simard/ecosystem_repos.toml` no longer
> exists and is no longer read. Editing a copy of it has no effect.

---

## Verifying resolution

```bash
# 1. Confirm the durable roster exists (seeded on first use).
test -f ~/.simard/state/identity_state/simard/governed_repos.toml && echo "roster present"

# 2. Inspect the governed repos.
grep '^value' ~/.simard/state/identity_state/simard/governed_repos.toml

# 3. Watch the daemon journal for a clean wire.
journalctl --user -u simard -f | grep -i ecosystem-observe
#   Expect NO "governed roster NOT resolved".
#   Expect the ecosystem-observe recipe to run on cadence.
```

See [Watch Overseer activity](../howto/watch-overseer-activity.md) for the
broader journal-tailing workflow.

---

## Testing

Hermetic unit tests use a `tempfile::tempdir()`-rooted `CuratedDataStore`, so they
never touch the real state root:

| Test (module) | Expectation |
|---|---|
| `default_seed_lists_the_ten_stewarded_repos` (`ecosystem_observe`) | Simard's default seed is the 10 stewarded repos, excluding the deprecated Python `rysweet/amplihack`. |
| `resolve_seeds_then_returns_validated_slugs` | First resolve seeds the dataset and returns the validated roster; the dataset is now on disk. |
| `resolve_returns_curated_edits_not_the_seed` | An agentic add/remove survives — a later resolve returns the curated copy, never the seed (proves deploy-durable mutability). |
| `resolve_skips_malformed_slugs_but_keeps_valid` | Malformed slugs are skipped; valid ones kept in order. |
| `resolve_all_invalid_is_error_not_empty_pass` | An all-invalid roster is an `Err`, never a silent empty pass. |
| `seed_for_*` | Seed selection: default Simard vs. a named identity's `target_repos`. |
| `governed_roster_*` (`ci_health`) | The sweep resolves the same durable dataset as the ecosystem-observe rail (single source of truth). |
| `identity_curated_state::*` | The generic store: round-trip, seed-on-first-use, curated-copy-after-mutation, per-identity isolation, path-traversal rejection. |

Gates: full `cargo test` (not `--lib` only) and
`cargo clippy --all-targets -- -D warnings` pass. No `--no-verify`, no `--admin`.

---

## Security notes

- **Fixed dataset segments.** The identity and dataset names are compile-time
  constants; the on-disk path is never constructed from env, argv, or file
  contents. `dataset_path` additionally rejects any segment containing a path
  separator or `..` (path-traversal prevention).
- **Slug validation before `gh`.** Every resolved slug is validated as a clean
  `owner/name` pair; a malformed slug is dropped, so nothing that could reach a
  shell survives into the agent's `gh` calls.
- **Atomic writes.** The store persists via a temp file + rename, so a crash
  mid-write never leaves a truncated roster.
- **Fail loud, never false-green.** An empty/all-invalid roster is an error, not
  an empty sweep — the resolver refuses to report a false-green ecosystem.
- **Never panics.** Resolution errors are surfaced as `Err` and logged
  fail-visibly; the daemon keeps running with the roster-bound rails skipped.
- **State root is operator-controlled**, consistent with the
  [state-root resolution](./state-root-resolution.md) threat model.

---

## See also

- [Ecosystem Observe](../design/ecosystem-observe.md) — the full agentic
  OBSERVE→BRIEF chain and the roster's role as an allowlist.
- [Identity-curated state](./identity-curated-state.md) — the generic
  identity-scoped mutable-data mechanism this roster is built on.
- [State-root resolution](./state-root-resolution.md) — how `<state_root>` is
  resolved and its shared threat model.
- [Fail-open audit](../fail-open-audit.md) — the fail-visible / fail-open
  posture this rail preserves.
- [Watch Overseer activity](../howto/watch-overseer-activity.md) — how to
  confirm the rail wires on the deployed daemon.
