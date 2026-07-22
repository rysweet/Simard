---
title: Governed-roster resolution (identity-scoped state)
description: >
  How Simard's governed ecosystem roster is resolved: durable, install-safe,
  agentically-curated identity-scoped state under the state root, seeded once
  from Simard's built-in identity default. Documents load_governed_roster, the
  identity_state store it reads, why the roster moved out of prompt_assets, and
  how to verify it.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: reference
status: current
issue: 2419
related:
  - ../design/ecosystem-observe.md
  - ./identity-scoped-state.md
  - ./state-root-resolution.md
  - ../fail-open-audit.md
---

# Governed-roster resolution (identity-scoped state)

> **Status: current.** This page is the contract for how the governed ecosystem
> roster is resolved. The roster is **not** a committed framework file anymore.
> It is **Simard's identity-scoped, mutable, deploy-durable state**: it lives
> under the state root at
> `<state_root>/identity_state/simard/governed_repos.toml` and is **seeded once**
> from Simard's built-in identity default (baked into the binary). Every consumer
> — the `ecosystem-observe` rail, the observe-merge-queue reasoner, and the
> CI-health sweep — resolves it through the single loader
> [`load_governed_roster`](../design/ecosystem-observe.md).

The roster *content* contract is unchanged: a list of validated `owner/name`
slugs (with human-readable notes that never reach the agent), including Simard's
own repo. What changed is the **source**: from a committed
`prompt_assets/simard/ecosystem_repos.toml` that install re-installed on every
deploy, to durable identity state that install never touches.

---

## Why the roster moved out of the framework

The roster answers "who does Simard steward?" — that is part of **who Simard
is**, not framework code. As a committed prompt-asset it had a fatal property:
`install` atomically replaces `~/.simard/prompt_assets/` on **every** self-deploy
(`replace_live_prompt_assets`), so any runtime curation of the deployed roster
was **clobbered** by the next deploy. The roster could never be agentically
curated and durably kept.

Moving it to identity-scoped state under the state root fixes this at the root:

- **Durable** — the state root (`~/.simard/state`, `~/.simard/identity_state`, …)
  is *never* rewritten by install; only `prompt_assets/` is. A self-deploy cannot
  clobber Simard's curated roster.
- **Mutable / agentic** — Simard curates the roster through her own reasoning
  (add/remove a governed repo), and the change persists across deploys.
- **Identity-scoped** — the roster is Simard's; the mechanism is generic (see
  [Identity-scoped state](./identity-scoped-state.md)) and holds no "roster"
  concept, so another identity could carry entirely different curated data.

---

## Resolution: seed-on-first-use

`load_governed_roster(state_root)` (in `src/overseer/ecosystem_observe.rs`)
resolves the roster through the generic
[`IdentityStateStore`](./identity-scoped-state.md):

1. **On first use** (no collection file yet), the store writes Simard's identity
   default — `SIMARD_GOVERNED_REPOS_SEED`, an `include_str!` of
   `src/identity/seeds/simard_governed_repos.toml` baked into the binary — durably
   to `<state_root>/identity_state/simard/governed_repos.toml` and returns it.
2. **Thereafter** the on-disk file is the single source of truth; the seed is
   never consulted again. Simard's agentic curation edits are what the loader
   returns.
3. The raw TOML is parsed and validated by `parse_ecosystem_roster` into ordered
   `owner/name` slugs. A roster with **no valid slugs** (empty file, or every slug
   malformed) is an **`Err`**, never a silent empty pass — an empty fleet would
   classify as zero failures and report **green**, the exact false-green the sweep
   and rails exist to prevent.

| Aspect | Value |
|---|---|
| Live roster path | `<state_root>/identity_state/simard/governed_repos.toml` |
| Default state root | `~/.simard` (override with `SIMARD_STATE_ROOT`) |
| Identity | `simard` |
| Collection | `governed_repos` |
| Seed (first use only) | `src/identity/seeds/simard_governed_repos.toml`, embedded via `include_str!` |
| Empty roster | loud `Err` (fail-closed), never an empty pass |

---

## API

Single public loader in `src/overseer/ecosystem_observe.rs`:

```rust
/// Resolve Simard's governed roster from identity-scoped state under
/// `state_root`, seeding it once from her identity default on first use, and
/// return the validated `owner/name` slugs in file order. An empty/invalid
/// roster is a loud `Err`, never a silent empty pass.
pub fn load_governed_roster(state_root: &Path) -> SimardResult<Vec<String>>;
```

It composes two reusable pieces:

- [`IdentityStateStore`](./identity-scoped-state.md) — the generic durable,
  atomic, install-safe, seed-on-first-use store (`load_or_seed_raw`).
- `parse_ecosystem_roster(raw: &str)` — the path-agnostic TOML → validated-slug
  parser shared by every consumer.

The CI-health sweep exposes a hermetic seam,
`ci_health::governed_repos_at(state_root)`, that calls `load_governed_roster`
against an explicit state root; production `ci_health::governed_repos()` passes
`simard_state_root()`.

---

## Wiring contract

`build_ecosystem_observer(repo_root, state_root)` and
`build_merge_queue_reasoner(repo_root, state_root)` (in
`src/overseer/wiring.rs`) both call `load_governed_roster(state_root)` and keep
the rail's **fail-visible / fail-open** posture:

| Loader result | Behavior |
|---|---|
| `Ok(roster)` | Wire the rail with the resolved roster. |
| `Err(error)` | Emit a fail-visible `WARN` naming the identity-scoped `state_root` and return `None` — the rail is left unwired and the pass is skipped this build; the daemon never panics and never silently degrades. |

Both builders take `state_root` (already threaded through `build_overseer`), so
the roster resolves against the same durable root the rest of the Overseer uses.

---

## Configuration

There is **no environment variable** for the roster itself (the state root is
configurable via `SIMARD_STATE_ROOT`). The roster is curated data: Simard edits
it agentically, and the edit persists at
`<state_root>/identity_state/simard/governed_repos.toml`.

| Deployment | Where the live roster lives |
|---|---|
| Deployed daemon (`WorkingDirectory=~/.simard`) | `~/.simard/identity_state/simard/governed_repos.toml` |
| Local dev / tests | `<state_root>/identity_state/simard/governed_repos.toml` (tests pass a `tempdir`) |

Changing the fleet Simard *starts* from (the seed) is a one-line data edit to
`src/identity/seeds/simard_governed_repos.toml` — but note the seed only applies
on **first use** in a fresh state root. On an already-seeded daemon, the live
roster is authoritative; re-seeding requires removing the collection file (or
curating it in place).

> **Migration note.** There is no automatic migration from the old
> `~/.simard/prompt_assets/simard/ecosystem_repos.toml`. The embedded seed
> reproduces the previous roster exactly, so a fresh state root seeds to the same
> fleet. Any ad-hoc edits previously made to the (clobbered) deployed
> prompt-asset copy must be re-applied through the new curation path.

---

## Verifying resolution

```bash
# 1. Confirm the live roster resolved/seeded under the state root.
test -f ~/.simard/identity_state/simard/governed_repos.toml && echo "roster present"

# 2. Watch the daemon journal for a clean wire (absence of the failure line).
journalctl --user -u simard -f | grep -i ecosystem-observe
#   Expect NO "ecosystem-observe NOT wired: failed to resolve the governed roster".
#   Expect the ecosystem-observe recipe to run on cadence.
```

See [Watch Overseer activity](../howto/watch-overseer-activity.md) for the
broader journal-tailing workflow.

---

## Testing

Hermetic unit tests (in `src/overseer/ecosystem_observe.rs`,
`src/identity_state/mod.rs`, and `src/ci_health/tests.rs`) use
`tempfile::tempdir()` as the state root so they never touch the ambient
`~/.simard`:

| Test | Expectation |
|---|---|
| `governed_roster_seeds_from_identity_default` | First load returns the embedded fleet |
| `governed_roster_seed_is_persisted_under_state_root` | The seed is written to `identity_state/simard/governed_repos.toml` |
| `governed_roster_returns_agentic_curation_not_seed` | A curated on-disk roster wins over the seed |
| `governed_roster_all_invalid_is_error_not_empty_pass` | An all-invalid roster is a loud `Err` |
| `ci_health … governed_roster_is_exactly_the_identity_seeded_source_of_truth` | The sweep resolves the same roster as the loader (single source of truth) |

Gates: `cargo test` and `cargo clippy --all-targets -- -D warnings` pass.

---

## Security notes

- **Constant identity/collection names.** `SIMARD_ROSTER_IDENTITY` and
  `GOVERNED_REPOS_COLLECTION` are compile-time constants; the store additionally
  validates every identity/collection name as a safe single path segment
  (path-traversal prevention).
- **Atomic, owner-only writes.** Seeding and curation go through
  `persistence::persist_bytes` (temp-write → fsync → atomic rename, `0600`), so a
  reader never sees a torn roster and a crash never leaves a half-written file.
- **Fail-closed on empty.** An empty/invalid roster is an `Err`, never an empty
  pass that would report the fleet green.
- **Logs are paths only.** Fail-visible warnings log the state root / collection
  path, never roster contents or credentials.
- **Never panics.** Resolution errors return `Err`/`None`; the daemon stays up
  (fail-open), consistent with the [state-root resolution](./state-root-resolution.md)
  threat model.

---

## See also

- [Ecosystem Observe](../design/ecosystem-observe.md) — the full agentic
  OBSERVE→BRIEF chain and the roster's role as the stewarded allowlist.
- [Identity-scoped state](./identity-scoped-state.md) — the generic durable,
  install-safe mechanism this roster is the first consumer of.
- [State-root resolution](./state-root-resolution.md) — the durable root and its
  threat model.
- [Fail-open audit](../fail-open-audit.md) — the fail-visible / fail-open posture
  these rails preserve.
