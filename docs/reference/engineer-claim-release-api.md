---
title: "Reference: Engineer-Claim Release & Reclaim API"
description: >
  The capability-handler API and admission-gate contract that make an
  engineer_claims row a releasable liveness lease. Covers release_engineer_claim
  (idempotent DELETE by claim_key), the EngineerLiveness injection point used by
  the reclaim path, the claim_key format, the record_action reclaim retry, error
  semantics, and worked examples.
last_updated: 2026-07-15
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/engineer-claim-liveness-lease.md
  - ooda-capability-api.md
  - typed-ooda-goal-session-rails.md
---

# Reference: Engineer-Claim Release & Reclaim API

> **Status: implemented.** Present-tense description of shipped behaviour.
> Primary source:
> [`src/typed_ooda/ledger.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/ledger.rs).
> Conceptual overview:
> [Engineer-Claim Liveness Lease](../concepts/engineer-claim-liveness-lease.md).

## The `engineer_claims` table

Declared in
[`src/typed_ooda/schema.rs`](https://github.com/rysweet/Simard/blob/main/src/typed_ooda/schema.rs).
Unchanged by this feature — **no new column, no `SCHEMA_VERSION` bump, no
migration**:

```sql
CREATE TABLE IF NOT EXISTS engineer_claims (
    claim_key  TEXT PRIMARY KEY,
    outcome_id TEXT NOT NULL UNIQUE,
    request_id TEXT NOT NULL,
    FOREIGN KEY(outcome_id) REFERENCES terminal_outcomes(outcome_id)
);
```

The row is a **lease**: it now has a deterministic release path and is
reclaimable when its engineer is not alive.

### `claim_key` format

```
claim_key = "{owner}/{repo}:{goal_id}"
```

- Reconstructed and equality-checked **server-side** in `record_action`
  (`spawn.claim_key` must equal `"{owner}/{repo}:{goal_id}"`, else
  `InvalidArgument`). A caller-supplied key is never trusted.
- The **same** formula is used at release time to target the exact row.
- `outcome_id` on the row is the **SpawnEngineer action's** outcome — *not* the
  later termination outcome. Release therefore keys on `claim_key`, never on a
  termination `outcome_id`.

## API: `CapabilityHandler::release_engineer_claim`

```rust
/// Release (delete) the engineer claim for `claim_key`.
///
/// Idempotent: deleting a claim that does not exist is success.
/// Runs in its own immediate transaction. Fail-visible: a real SQL error
/// is returned as `Err`, never swallowed.
pub fn release_engineer_claim(&self, claim_key: &str) -> CapabilityResult<()>;
```

Semantics:

| Condition | Result |
|---|---|
| Row exists and is deleted | `Ok(())` |
| Row does not exist (0 rows affected) | `Ok(())` — idempotent |
| Underlying SQL / persistence failure | `Err(CapabilityError { code: PersistenceFailed, .. })` |

Implementation contract:

- Single private SQL owner: `DELETE FROM engineer_claims WHERE claim_key = ?1`
  via `params![]` (no string interpolation).
- Owns an **immediate transaction** so a concurrent reclaim/insert cannot
  interleave a partial state.
- **No** wall-clock logic. Release is unconditional for the given key.

### Call sites (all engineer-termination paths)

`release_engineer_claim` is invoked from the single deterministic chokepoint
every termination path flows through:

```
cleanup_engineer_worktree_for_goal(state: &mut OodaState, goal_id: &str)
    └─ reconstruct claim_key = "{owner}/{repo}:{goal_id}"
    └─ open CapabilityHandler from typed_ooda::ledger_path(state_root)
    └─ handler.release_engineer_claim(&claim_key)      // idempotent
```

Because cleanup runs on success, failure, blocked, crash, and zombie-reap
(six call sites in `subordinate.rs`), the claim is released on all of them. A
release failure is **logged and surfaced**, never silently ignored. The handler
opened here for release **never invokes** `is_claim_live`, so its
`EngineerLiveness` provider is immaterial on this path — the default provider
is sufficient.

## Admission gate: liveness-verified reclaim in `record_action`

When `record_action` records a `SpawnEngineer` action it inserts the claim:

```sql
INSERT INTO engineer_claims(claim_key, outcome_id, request_id) VALUES (?1, ?2, ?3)
```

On a `claim_key` `PRIMARY KEY` constraint violation the gate no longer rejects
blindly. It branches on **liveness of the existing claim**:

```text
INSERT hits PRIMARY KEY constraint on claim_key
        │
        ▼
is_claim_live(claim_key)?
   ├── true  ── keep AdmissionRejected  ("engineer claim is already active: {claim_key}")
   └── false ── DELETE stale row + retry INSERT once, in the SAME transaction
                    ├── retry ok        ── spawn admitted
                    └── still constrained ── AdmissionRejected
```

- Reclaim is **targeted** (only the colliding `claim_key`), not a global sweep.
- Reclaim happens **inside the spawn transaction** → no zero-claim window and no
  TOCTOU gap that could admit two engineers.
- A **live** claim always keeps the rejection (fail-closed) — the
  single-active-claim invariant is preserved.
- **No `outcome_id UNIQUE` collision on retry.** The retried INSERT carries the
  *new* spawn's freshly-generated `outcome_id`, so it never clashes with the
  stale row's value. The reclaim `DELETE` removes only the `engineer_claims`
  child row; the referenced historical `terminal_outcomes` row is left intact
  (the FK is child→parent, and deleting the claim does not cascade to the
  outcome). Prior engineer history is therefore preserved.

### `EngineerLiveness` injection

`CapabilityHandler` carries a boxed liveness provider so the ledger does not
depend on `ooda_actions` (avoiding a dependency cycle):

```rust
/// Authoritative liveness signal for an engineer claim.
pub trait EngineerLiveness: Send + Sync {
    /// True iff `claim_key`'s engineer is actually alive right now.
    fn is_claim_live(&self, claim_key: &str) -> bool;
}
```

| Context | Provider |
|---|---|
| Production daemon | Wraps `find_live_engineer_for_goal(state_root, goal_id)` → sentinel scan of `<state_root>/engineer-worktrees/*/.simard-engineer-claim` + `is_pid_alive_public(pid)` **with the process start-time guard** against recycled PIDs. |
| Tests | A mock returning a scripted `bool` — no real processes spawned. |
| Legacy / default constructors | A behaviour-preserving default so existing call paths compile unchanged. |

Liveness of the **sentinel PID is authoritative**. Reclaim fires strictly on
positive proof of death (PID absent **or** start-time mismatch). There is no
elapsed-time invalidation.

> **Fail-closed on scan error (must-hold contract).** `is_claim_live` must
> return `true` for *any* outcome that is not **provable death**. A transient
> failure to enumerate the worktree directory (e.g. `read_dir` returns `Err`
> because the path is momentarily unreadable) is **not** proof of death and
> **must not** be reported as dead — otherwise a live engineer could be wrongly
> reclaimed, violating the single-active-claim invariant. Note that the raw
> `find_live_engineer_for_goal` helper returns `None` on a `read_dir` error
> (indistinguishable from "no live engineer found"), so the production provider
> must **not** map a bare `None` to *dead*. It must treat only a *successful*
> scan that finds no matching, alive, start-time-verified sentinel as death;
> any scan/IO error resolves to **live** (fail-closed).

> **`claim_key` → `goal_id` reconstruction.** The trait receives the full
> `claim_key` (`{owner}/{repo}:{goal_id}`), while the underlying
> `find_live_engineer_for_goal(state_root, goal_id)` is keyed on `goal_id`
> alone (it prefix-matches worktree directories `<state_root>/engineer-worktrees/{goal_id}-*`).
> The production provider therefore parses `goal_id` back out of the
> `claim_key` (the segment after the first `:`) and captures `state_root` at
> construction. This is sound because `goal_id` is unique within a single
> daemon's `state_root`; the SQL claim's `{owner}/{repo}` prefix and the
> filesystem liveness scan address the same engineer.

## Error semantics (fail-visible)

| Path | On error |
|---|---|
| `release_engineer_claim` DELETE fails | return `Err`; caller logs at error and surfaces it |
| Reclaim DELETE / retry INSERT fails with a non-constraint SQL error | propagated as `PersistenceFailed` `Err` |
| Live claim collision | `AdmissionRejected` (expected, not an error) |

Nothing on the release or reclaim path swallows a failure.

## Examples

### Release on termination (deterministic)

```rust
// Inside cleanup_engineer_worktree_for_goal, reached by every exit path.
let claim_key = format!("{}/{}:{}", repo.owner, repo.name, goal_id);
if let Err(err) = handler.release_engineer_claim(&claim_key) {
    // Fail-visible: surfaced, never swallowed.
    tracing::error!(%claim_key, error = %err, "failed to release engineer claim");
}
```

### Reclaiming an orphaned claim (host reboot killed the engineer)

```text
# Claim row survives, but its sentinel PID is gone.
spawn engineer for rysweet/Simard:harden-backups
  → INSERT hits PRIMARY KEY on "rysweet/Simard:harden-backups"
  → is_claim_live("rysweet/Simard:harden-backups") == false   (dead PID)
  → DELETE stale row, retry INSERT  → admitted
```

### Duplicate spawn blocked while engineer is alive

```text
engineer #1 running for rysweet/Simard:harden-backups   (sentinel PID alive)
spawn engineer #2 for the same goal
  → INSERT hits PRIMARY KEY
  → is_claim_live(...) == true
  → AdmissionRejected: "engineer claim is already active: rysweet/Simard:harden-backups"
```

## Regression coverage

Tests live alongside the code (`src/typed_ooda/ledger.rs` unit tests;
`src/ooda_actions/advance_goal/*` for the integration/liveness paths):

1. **Core regression** — spawn engineer → record terminal outcome → spawn again
   for the **same goal succeeds** (claim was released).
2. **Stale reclaim** — a claim whose engineer PID/sentinel is dead does **not**
   block a new spawn.
3. **Single-active preserved** — a claim whose engineer is genuinely alive
   **still blocks** a duplicate concurrent spawn.
4. **Idempotent release** — releasing a non-existent / already-released claim
   returns `Ok(())`.
5. **Targeted delete** — release/reclaim touches only the matching `claim_key`.
6. **Fail-closed on ambiguous liveness** — when the liveness provider cannot
   prove death (scan/IO error), the claim is treated as live and the duplicate
   spawn is **still blocked** (no wrongful reclaim).

Required gates (merge blockers, not observed results): `cargo fmt`,
`cargo clippy -D warnings`, and `cargo test` must pass, and the SQL
release/reclaim paths must carry no `unwrap`/`expect`.

## Related

- [Engineer-Claim Liveness Lease](../concepts/engineer-claim-liveness-lease.md)
- [OODA Capability API](ooda-capability-api.md)
- [Typed OODA Goal-Session Deterministic Rails](typed-ooda-goal-session-rails.md)
