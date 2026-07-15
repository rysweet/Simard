---
title: "Reference: Stale-Engineer-Claim Reaper API"
description: >
  The API contract for the periodic engineer-claim reaper: the
  list_engineer_claims ledger method, the reap_stale_claims sweep orchestrator,
  the ClaimLivenessProbe seam and its ClaimLiveness/DeadReason verdict, the
  OrphanWorktreeCleanup seam, the SIMARD_CLAIM_REAP_* config resolvers
  (*_from(lookup) pattern), the Overseer wiring, fail-closed / fail-visible
  semantics, and the regression test list.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/stale-engineer-claim-reaper.md
  - ./engineer-claim-release-api.md
  - ./engineer-worktree-sweep-safety.md
  - ./overseer-tick-details.md
  - ../operations/claim-reaper-kill-switch.md
---

# Reference: Stale-Engineer-Claim Reaper API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs).
> Conceptual overview:
> [Stale-Engineer-Claim Reaper](../concepts/stale-engineer-claim-reaper.md).

## Overview

The reaper is a pure orchestrator over three injectable seams:

- a **ledger handle** that lists and releases claims
  (`CapabilityHandler`, [`ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs)),
- a **`ClaimLivenessProbe`** that returns a rich liveness verdict per claim, and
- an **`OrphanWorktreeCleanup`** that removes an orphaned worktree directory
  through the existing guarded-removal primitive.

Injecting the last two makes the whole sweep hermetically testable with fakes —
no real processes, no real `gh`, no real filesystem required.

## Ledger: `list_engineer_claims`

New read-only method beside
[`release_engineer_claim`](./engineer-claim-release-api.md), added to
`CapabilityHandler` in
[`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs):

```rust
/// Return every claim_key currently held in the engineer_claims table.
///
/// Read-only full scan of a tiny table (cap 24). Uses the existing
/// prepare/query_map pattern with params![]. No schema change, no
/// SCHEMA_VERSION bump.
pub fn list_engineer_claims(&self) -> CapabilityResult<Vec<String>>;
```

| Condition | Result |
|---|---|
| Table has rows | `Ok(vec_of_claim_keys)` |
| Table empty | `Ok(vec![])` |
| Underlying SQL failure | `Err(CapabilityError { code: PersistenceFailed, .. })` — surfaced, never swallowed |

Backing SQL: `SELECT claim_key FROM engineer_claims`. The `engineer_claims`
schema is **unchanged** by this feature.

## Sweep: `reap_stale_claims`

The single public orchestrator, in
[`src/overseer/claim_reaper.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/claim_reaper.rs):

```rust
/// Sweep ALL engineer claims and reclaim those whose engineer is provably dead.
///
/// Independent of per-goal polling. Fail-closed (unknown/fresh => never reaped),
/// fail-visible (one [simard] line per reclaim). Per-claim errors are contained:
/// a single bad entry can never abort the sweep.
///
/// Returns the number of claims reclaimed this sweep.
pub fn reap_stale_claims(
    handler: &CapabilityHandler,
    probe: &dyn ClaimLivenessProbe,
    cleanup: &dyn OrphanWorktreeCleanup,
    enabled: bool,
    stale_secs: u64,
) -> usize;
```

Algorithm:

```text
if !enabled: return 0                      // kill switch: no work at all

for claim_key in handler.list_engineer_claims():        // errors surfaced, sweep continues
    match probe.assess(&claim_key):
        Live                              => skip
        Dead { NoWorktree,      .. }      => reclaim(claim_key, reason="no-worktree",     age=n/a)
        Dead { HeartbeatStale, age }
            if age > stale_secs           => reclaim(claim_key, reason="heartbeat-stale", age)
        Dead { HeartbeatStale, .. }       => skip        // within threshold => still Live-ish
    // reclaim = release_engineer_claim(claim_key) + cleanup.remove(goal_id)
    // each reclaim emits exactly one [simard] fail-visible line
```

- **No `.unwrap()`/`.expect()` on any I/O or SQL path.**
- **Per-claim containment:** an error assessing or reclaiming one claim is
  logged and the loop continues to the next claim.
- **Reclaim chokepoints only:** ledger `DELETE` via `release_engineer_claim`;
  worktree removal via the `OrphanWorktreeCleanup` seam. No hand-rolled SQL, no
  `--admin`.

## Liveness seam: `ClaimLivenessProbe`

`EngineerLiveness::is_claim_live(&self, claim_key) -> bool` cannot carry a reason
or an age, both of which fail-visible logging requires. The reaper introduces a
richer, reaper-local seam. `EngineerLiveness` / `WorktreeEngineerLiveness` are
**untouched**.

```rust
/// Reason a claim's engineer is judged dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadReason {
    /// No engineer-worktree directory maps to this claim's goal_id.
    NoWorktree,
    /// A worktree exists but its newest-file mtime is stale.
    HeartbeatStale,
}

/// Rich liveness verdict for a single claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimLiveness {
    /// Engineer is (or may be) alive — never reclaim. Also the fail-closed
    /// result for any scan/IO uncertainty.
    Live,
    /// Engineer is provably dead. `age_secs` is the newest-file idle age for
    /// HeartbeatStale, or None for NoWorktree.
    Dead { reason: DeadReason, age_secs: Option<u64> },
}

/// Injection point for filesystem/heartbeat probing. Fakes implement this in
/// tests; production scans the worktree tree.
pub trait ClaimLivenessProbe: Send + Sync {
    fn assess(&self, claim_key: &str) -> ClaimLiveness;
}
```

### Production probe

`WorktreeClaimLivenessProbe` (captures `state_root` and `stale_secs` at
construction):

1. Parse `goal_id` from `claim_key` with `split_once(':')` (segment after the
   first `:`), matching the existing `is_claim_live` precedent at
   [`src/ooda_actions/advance_goal/typed_goal_session.rs:490`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/typed_goal_session.rs).
   `owner`, `repo`, and `goal_id` contain no `:`, so first- and last-colon
   splits are equivalent; a malformed key with no `:` is treated as live
   (fail-closed).
2. Scan `<state_root>/engineer-worktrees/` and match the directory whose
   `goal_id_from_worktree_dir(dir) == goal_id`
   ([`src/engineer_worktree/discovery.rs`](https://github.com/rysweet/Simard/blob/main/src/engineer_worktree/discovery.rs),
   promoted to `pub(crate)`; total, non-panicking parse).
3. Verdict:
   - Root read succeeds, **no matching dir** → `Dead { NoWorktree, None }`.
   - Matching dir found → compute **newest-file mtime**; `age = now - mtime`.
     `Dead { HeartbeatStale, Some(age) }` (the sweep then compares `age` to the
     threshold).
   - Fresh newest file → `Live`.
   - **Root unreadable / any I/O error** → `Live` (**fail-closed**; an
     unreadable root is *not* proof of death, so a live engineer is never
     reclaimed).

> **Fail-closed contract (must hold).** Only a *successful* scan that proves the
> engineer is gone (missing dir) or provably idle (readable newest-file mtime
> older than threshold) yields a `Dead` verdict. Every ambiguous outcome resolves
> to `Live`. This mirrors the `EngineerLiveness` fail-closed rule.

## Cleanup seam: `OrphanWorktreeCleanup`

Wraps the guarded worktree removal as an `OodaState`-free primitive so the sweep
(which runs in the Overseer tick, without an `OodaState`) can reuse it and tests
can stub it:

```rust
/// Remove the orphaned engineer-worktree directory for `goal_id`, if present.
///
/// Routes through the existing assert_under_root + remove_dir_all guard —
/// see the worktree-reaping safety guards. Idempotent: a missing directory is
/// success. Never constructs a delete path from claim_key; deletes only a
/// directory discovered on disk under <state_root>/engineer-worktrees/.
pub trait OrphanWorktreeCleanup: Send + Sync {
    fn remove(&self, goal_id: &str) -> std::io::Result<()>;
}
```

The production impl reuses the same `assert_under_root` (canonicalize +
`starts_with`) guard documented in
[Worktree Reaping Safety Guards](./engineer-worktree-sweep-safety.md), so a
corrupt goal id / path-traversal attempt can never escape the worktree root.

## Config resolvers (`*_from(lookup)` pattern)

Added to
[`src/overseer/config.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/config.rs),
mirroring `gap_scan_*_from`:

```rust
pub const SIMARD_CLAIM_REAP_ENABLED_ENV: &str = "SIMARD_CLAIM_REAP_ENABLED";
pub const SIMARD_CLAIM_REAP_STALE_SECS_ENV: &str = "SIMARD_CLAIM_REAP_STALE_SECS";
pub const DEFAULT_CLAIM_REAP_STALE_SECS: u64 = 1800; // 30 minutes

/// Enabled by default. Disabled only when SIMARD_CLAIM_REAP_ENABLED is an
/// explicit falsey value (0/false/no/off). Unset/empty/garbage => enabled.
pub fn claim_reap_enabled_from(lookup: impl Fn(&str) -> Option<String>) -> bool;
pub fn claim_reap_enabled() -> bool; // production: reads std::env

/// Stale-idle threshold in seconds. Default 1800. Unset/empty/unparseable/zero
/// => the safe default (never a 0-second threshold that would mass-reclaim).
pub fn claim_reap_stale_secs_from(lookup: impl Fn(&str) -> Option<String>) -> u64;
pub fn claim_reap_stale_secs() -> u64; // production: reads std::env
```

| Env | Unset | Explicit valid | Falsey / invalid |
|---|---|---|---|
| `SIMARD_CLAIM_REAP_ENABLED` | `true` (enabled) | honored | `0`/`false`/`no`/`off` → **disabled** |
| `SIMARD_CLAIM_REAP_STALE_SECS` | `1800` | honored (seconds) | empty/garbage/`0` → **`1800`** (safe default) |

The primary off switch is `SIMARD_CLAIM_REAP_ENABLED`. See the
[kill switch operations page](../operations/claim-reaper-kill-switch.md).

## Overseer wiring

The `Overseer` struct gains `state_root: PathBuf` plus the reaper config
(`claim_reap_enabled`, `claim_reap_stale_secs`) and the boxed probe/cleanup
seams. These are wired in `build_overseer`
([`src/overseer/wiring.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/wiring.rs)),
where `state_root` is already in scope, with the production
`WorktreeClaimLivenessProbe` and worktree-cleanup impls. `run_cycle`
([`src/overseer/mod.rs`](https://github.com/rysweet/Simard/blob/main/src/overseer/mod.rs))
calls `reap_stale_claims(...)` synchronously beside
`reconcile_inflight_investigations` — **no new thread**.

## Fail-visible logging

Each reclaim emits exactly one line (the `[simard]` prefix, the `claim_key`,
`age`, and `reason` substrings are stable):

```
[simard] claim-reaper: reclaimed rysweet/Simard:g1 (reason=no-worktree, age=n/a)
[simard] claim-reaper: reclaimed rysweet/Simard:goal-improve-tests (reason=heartbeat-stale, age=5142s)
```

A surfaced (non-fatal) per-claim error is logged and the sweep continues:

```
[simard] claim-reaper: skipped rysweet/Simard:g2 — release failed: <error>
```

## Error semantics (fail-visible)

| Path | On error |
|---|---|
| `list_engineer_claims` SQL fails | `Err` surfaced; the sweep is skipped this tick (logged), retried next tick |
| Probe scan/IO error | resolves to `Live` (fail-closed) — the claim is kept, not an error |
| `release_engineer_claim` DELETE fails | logged; that claim is skipped; sweep continues |
| `OrphanWorktreeCleanup::remove` fails | logged; the ledger row is still released (lease is the source of truth); sweep continues |

Nothing on the reap path swallows a failure silently.

## Regression coverage

Tests live inline in `src/overseer/claim_reaper.rs` (`#[cfg(test)]`) and
`src/overseer/config.rs`, using fake seams:

| Test | Asserts |
|---|---|
| T1 | Claim with **no worktree** ⇒ reaped immediately. |
| T2 | Claim with a **fresh** worktree (mtime ~now) ⇒ **not** reaped. |
| T3 | Claim with a **stale** worktree (mtime > threshold) ⇒ reaped. |
| T4 | Reclaim goes through `release_engineer_claim` (row gone) + worktree cleaned; **no** hand-rolled SQL / `--admin`. |
| T5 | Config `*_from(lookup)`: unset ⇒ default (1800 / enabled); explicit ⇒ honored; falsey ⇒ disabled (no reclaims even with a no-worktree claim); `stale_secs=0`/garbage ⇒ default. |
| T6 | Injected filesystem/heartbeat fakes only — no real processes, no real `gh`. |
| Fail-closed probe | **Root-unreadable** worktree tree ⇒ `Live` ⇒ claim kept. |

Required gates (merge blockers): `cargo fmt`, `cargo clippy -D warnings`, and
`cargo test` must pass; the reap/probe/cleanup paths must carry no
`unwrap`/`expect` on I/O.

## Related

- [Stale-Engineer-Claim Reaper (concept)](../concepts/stale-engineer-claim-reaper.md)
- [Engineer-Claim Release & Reclaim API](./engineer-claim-release-api.md)
- [Worktree Reaping Safety Guards](./engineer-worktree-sweep-safety.md)
- [Claim-Reaper Kill Switch & Tuning](../operations/claim-reaper-kill-switch.md)
- [Overseer Tick Details](./overseer-tick-details.md)
