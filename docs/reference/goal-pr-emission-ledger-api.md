---
title: Goal-PR emission ledger API reference
description: >
  The typed surface that makes done-gate PR emission idempotent per goal: the
  `goal_pr_emissions` SQLite table in the typed-OODA store (schema v2), its
  parameterized ledger API (`record_goal_pr_emission` upsert +
  `find_open_goal_pr_emission` lookup), the `goal_dedup_key` /
  `find_open_pr_for_goal` bricks, the `GoalPrRef` DTO, the
  `PrGhClient::list_open_goal_prs` detection seam with its back-compatible
  default impl, the per-cycle open-PR cache, and the third
  `dispatch_spawn_engineer` guard. Documents the forward-only schema migration,
  the fail-open reconciliation contract, the `Simard-Goal-Key:` trailer format,
  and the injectable fakes used for hermetic tests.
last_updated: 2026-07-18
review_schedule: as-needed
owner: simard
doc_type: reference
related:
  - ../concepts/idempotent-done-gate-pr-emission.md
  - ./typed-ooda-goal-session-rails.md
  - ./cross-repo-merge-authority.md
  - ./spawn-agent-for-goal.md
  - ../howto/diagnose-duplicate-done-gate-prs.md
---

# Goal-PR emission ledger API reference

This reference documents the typed surface behind
[idempotent done-gate PR emission](../concepts/idempotent-done-gate-pr-emission.md).
All examples are illustrative signatures; the authoritative source is
[`src/typed_ooda/`](https://github.com/rysweet/Simard/tree/main/src/typed_ooda),
[`src/ooda_actions/advance_goal/goal_dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/goal_dedup.rs),
and
[`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs).

## The `goal_pr_emissions` ledger table (schema v2)

The durable, authoritative record of every done-gate PR Simard has emitted,
persisted in the typed-OODA store (`outcomes.sqlite3`) alongside
`terminal_outcomes`, `engineer_claims`, and the other typed-OODA tables.

```sql
CREATE TABLE IF NOT EXISTS goal_pr_emissions (
    goal_key     TEXT PRIMARY KEY,   -- goal_dedup_key(id, repo): 16 lowercase hex
    goal_id      TEXT NOT NULL,
    repo         TEXT NOT NULL,
    pr_number    INTEGER NOT NULL,
    pr_url       TEXT NOT NULL,
    head_ref     TEXT NOT NULL,
    state        TEXT NOT NULL,       -- 'open' | 'merged' | 'closed' | 'superseded'
    created_at   INTEGER NOT NULL,    -- epoch millis
    updated_at   INTEGER NOT NULL,    -- epoch millis
    UNIQUE(repo, pr_number)
);
CREATE INDEX IF NOT EXISTS idx_goal_pr_emissions_open
    ON goal_pr_emissions(goal_key) WHERE state = 'open';
```

Key points:

- **The primary key is `goal_key`, a goal-identity key — deliberately *not*
  named `claim_key`.** It equals `goal_dedup_key(goal_id, repo)` (below). The
  distinct name avoids colliding with `engineer_claims.claim_key`, which is a
  semantically different (engineer-claim) key: the two tables must never be
  conflated. This table is **never** row-deleted; a completed PR transitions
  `state` instead — so an emission outlives the engineer that opened it (unlike
  `engineer_claims`, which is `DELETE`d on termination).
- **`UNIQUE(repo, pr_number)`** makes the upsert TOCTOU-safe: a concurrent
  re-record of the same PR conflicts rather than duplicating.
- The **partial open-index** makes `find_open_goal_pr_emission(goal_key)` an
  indexed point-lookup on the only rows that matter for the guard.

### Migration: v1 → v2 (forward-only, zero backfill)

The typed-OODA `initialize` migration bumps `SCHEMA_VERSION` from `1` to `2`.
Inside the existing `Immediate` transaction it runs
`CREATE TABLE IF NOT EXISTS goal_pr_emissions` + the open-index, then
`PRAGMA user_version = 2`.

- **Forward-only.** A store already at `user_version > SCHEMA_VERSION` is
  rejected (`rusqlite::Error::InvalidQuery`), matching the existing guard.
- **Idempotent.** Re-running `initialize` on a v2 store is a no-op (the
  early-return on `version == SCHEMA_VERSION`).
- **No backfill.** Pre-existing open PRs are *not* imported; they are picked up
  lazily by the advisory `gh` reconciliation (below), which then records them.

## Ledger API

Both calls use parameterized `params![]` binding — no string interpolation —
matching the existing `ledger.rs` idioms, and return the typed-OODA
`CapabilityResult` error type used by the rest of the ledger surface (e.g.
`release_engineer_claim`, `register_actor_session`).

### `record_goal_pr_emission`

```rust
/// Upsert an emission row. Called by the engineer PR-emission contract after
/// `gh pr create`, and by the advisory reconciliation when it adopts a
/// pre-existing PR. Idempotent on `goal_key` via ON CONFLICT.
pub fn record_goal_pr_emission(
    &self,
    goal_key: &str,           // goal_dedup_key(goal_id, repo)
    goal_id: &str,
    repo: &str,
    pr_number: u32,
    pr_url: &str,
    head_ref: &str,
    state: EmissionState,     // Open | Merged | Closed | Superseded
    now_millis: i64,
) -> CapabilityResult<()>;
```

Implemented as:

```sql
INSERT INTO goal_pr_emissions
    (goal_key, goal_id, repo, pr_number, pr_url, head_ref, state, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
ON CONFLICT(goal_key) DO UPDATE SET
    pr_number = excluded.pr_number,
    pr_url    = excluded.pr_url,
    head_ref  = excluded.head_ref,
    state     = excluded.state,
    updated_at = excluded.updated_at;
```

### `find_open_goal_pr_emission`

```rust
/// Indexed point-lookup of the OPEN emission for a goal-key, if any.
/// The primary (authoritative) guard consulted by dispatch_spawn_engineer.
pub fn find_open_goal_pr_emission(
    &self,
    goal_key: &str,
) -> CapabilityResult<Option<GoalPrEmission>>;
```

```rust
pub struct GoalPrEmission {
    pub goal_key: String,
    pub goal_id: String,
    pub repo: String,
    pub pr_number: u32,
    pub pr_url: String,
    pub head_ref: String,
    pub state: EmissionState,
}
```

## The `goal_dedup` brick

New module
[`src/ooda_actions/advance_goal/goal_dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/goal_dedup.rs).
Total, no-panic, no I/O — mirrors
[`src/stewardship/dedup.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/dedup.rs).

### `goal_dedup_key`

```rust
/// Stable, one-way goal-identity key: the first 16 lowercase-hex chars of
/// sha256(goal_id + "\n" + repo). Never derived from the goal title.
pub fn goal_dedup_key(goal_id: &str, repo: &str) -> String;
```

The `repo` argument is the goal's routed target repo (the same value used by
[goal target-repo routing](./goal-target-repo-routing.md)); a `None` route
resolves to the Simard repo before keying, so the key is stable regardless of
how the route is expressed.

For ordinary identities the preimage is exactly `goal_id + "\n" + repo`. To keep
the encoding injective, a literal `\` or newline inside either field is
backslash-escaped before joining (an identity no-op for the common,
newline-free case), so a newline embedded in one field can never be confused
with the field boundary.

### The `Simard-Goal-Key:` body trailer

Engineers stamp the key into the PR body as a line-anchored trailer:

```
Simard-Goal-Key: 4f2a9c1e7b3d0a58
```

Parsing contract (attacker-controllable input → total/no-panic):

| Rule | Behaviour |
| --- | --- |
| Prefix match | Exact, line-anchored `^Simard-Goal-Key: ` (case-sensitive). |
| Value validation | Must match `^[0-9a-f]{16}$`; anything else is ignored. |
| Body-size cap | Only the first `MAX_TRAILER_SCAN_BYTES` of the body are scanned. |
| Multiple matches | If two or more distinct valid trailers appear, **ignore all** (ambiguous → no match). |
| No panic | Malformed UTF-8, control chars, and truncation are handled without panicking. |

### `find_open_pr_for_goal`

```rust
/// Given the goal-key and a list of open PRs, return the matching PR by
/// precedence: (1) Simard-Goal-Key body trailer, then (2) head-branch
/// convention `engineer/{goal-key}-...`. Returns None if none match.
pub fn find_open_pr_for_goal<'a>(
    goal_key: &str,
    open_prs: &'a [GoalPrRef],
) -> Option<&'a GoalPrRef>;
```

The head-branch fallback matches on the `engineer/{goal-key}-` prefix with a `-`
boundary guard, so `engineer/4f2a9c1e7b3d0a58-...` matches but
`engineer/4f2a9c1e7b3d0a58ff-...` does not.

## `GoalPrRef` DTO

```rust
/// Purpose-scoped view of an open PR carrying its body, used only by
/// goal-emission reconciliation.
pub struct GoalPrRef {
    pub number: u32,
    pub url: String,
    pub head_ref_name: String,
    pub body: String,
}
```

A separate DTO is required because the existing
[`OpenPrSummary`](./cross-repo-merge-authority.md) (shared with the dashboard and
self-merge sensor) carries **no** `body` field, and widening it would bloat those
unrelated fetches.

## `PrGhClient::list_open_goal_prs` (detection seam)

A new method on the existing `PrGhClient` trait
([`src/stewardship/merge_authority.rs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs)),
with a **back-compatible default impl** so every existing fake keeps compiling
unchanged:

```rust
pub trait PrGhClient {
    // ... existing methods ...

    /// List open PRs (with bodies) for goal-emission reconciliation.
    /// Default returns Ok(vec![]) so existing fakes need no update;
    /// RealPrGhClient overrides it.
    fn list_open_goal_prs(&self, _repo: &str, _limit: u32)
        -> SimardResult<Vec<GoalPrRef>>
    {
        Ok(Vec::new())
    }
}
```

`RealPrGhClient` runs:

```
gh pr list --repo <repo> --state open --label simard-autonomous \
   --limit <limit> --json number,url,headRefName,body
```

- Invoked via `Command::new("gh").args(&[...])` — **direct exec, no shell**, so
  no new shell-injection surface.
- The `simard-autonomous` label is a **filter, not authorization**: a match must
  *also* satisfy the goal-key trailer or the `engineer/{goal-key}-` branch
  before it counts. The label alone never suppresses emission.
- Reuses the ambient `gh` auth (existing repo scope); no new credentials.

## Per-cycle open-PR cache

`OodaState` gains an `open_pr_cache` keyed on the durable cycle counter
(`OodaState.cycle_count: u32`), so at most **one** `gh pr list` runs per repo
per OODA cycle regardless of how many goals advance:

```rust
// src/ooda_loop/types.rs
pub struct OodaState {
    // ...
    /// (cycle_count, repo) -> reconciled open PRs. Invalidated when the
    /// cycle counter advances.
    pub open_pr_cache: HashMap<(u32, String), Vec<GoalPrRef>>,
}
```

## The dispatch guard

The third guard in
[`dispatch_spawn_engineer`](https://github.com/rysweet/Simard/blob/main/src/ooda_actions/advance_goal/spawn.rs),
inserted **after** the live-worktree guard (~L291) and **before** any worktree
allocation:

```rust
let goal_key = goal_dedup_key(goal_id, &routed_repo);

// (1) Primary, authoritative: the durable ledger.
if let Some(existing) = ledger.find_open_goal_pr_emission(&goal_key)? {
    tracing::info!(
        target: "ooda::done_gate",
        goal_id, repo = %existing.repo, pr = existing.pr_number, key = %goal_key,
        "spawn skipped: open done-gate PR already tracked for goal",
    );
    return make_outcome(action, true, /* idempotent no-op */ ...);
}

// (2) Secondary, advisory: reconcile against live open PRs (fail-open).
match reconcile_open_prs(gh, &routed_repo, cycle_count, &mut state.open_pr_cache) {
    Ok(open_prs) => {
        if let Some(pr) = find_open_pr_for_goal(&goal_key, &open_prs) {
            // self-heal: adopt the pre-existing PR into the ledger, then skip.
            ledger.record_goal_pr_emission(
                &goal_key, goal_id, &routed_repo,
                pr.number, &pr.url, &pr.head_ref_name,
                EmissionState::Open, now_millis(),
            )?;
            return make_outcome(action, true, /* idempotent no-op */ ...);
        }
    }
    Err(e) => {
        // FAIL-OPEN: ledger already holds the primary guarantee.
        tracing::warn!(
            target: "ooda::done_gate", goal_id, repo = %routed_repo, error = %e,
            "open-PR reconciliation failed; proceeding on ledger guard only",
        );
    }
}

// (3) No open PR for this goal -> dispatch exactly one engineer.
```

- On a guard hit the outcome is `success = true` — declining to act is the
  correct behaviour, matching the two existing guards.
- Logs carry only PR number + repo + the 16-hex key — never a token or raw PR
  body; control chars are stripped.

## Engineer PR-emission contract

After the guard passes and the engineer opens its PR:

1. The engineer task string (`spawn.rs` ~L642) and
   [`prompts/engineer_system.md`](https://github.com/rysweet/Simard/blob/main/prompts/engineer_system.md)
   instruct the subprocess to stamp `Simard-Goal-Key: <key>` into the PR body and
   name the head branch `engineer/{key}-<slug>`.
2. After `gh pr create` succeeds, the emission is recorded via
   `record_goal_pr_emission(..., EmissionState::Open, ...)`.

## Testing seams

| Seam | Fake |
| --- | --- |
| `PrGhClient::list_open_goal_prs` | `FakePrGhClient` seeds a fixed `Vec<GoalPrRef>` (or an `Err` to exercise fail-open). |
| Typed-OODA ledger | In-memory (`:memory:`) SQLite store; no filesystem. |
| Cycle cache | Driven directly by advancing `cycle_count`. |

Coverage (all hermetic, trait-injection):

- `goal_dedup.rs` — key stability, trailer parsing (valid/invalid/multiple/oversized/non-UTF-8), branch-boundary matching.
- `ledger.rs` — v1→v2 migration idempotency, upsert ON CONFLICT, `UNIQUE(repo, pr_number)`, open-index lookup.
- `spawn.rs` — guard hit on ledger, guard hit on reconciliation (with self-heal record), fail-open on lister `Err`, **distinct-goal PRs still dispatch**.
- `tests/done_gate_dedup_integration.rs` — two consecutive OODA cycles with engineer termination + an open PR ⇒ exactly **one** dispatch / one PR.

## Related

- [Concept: idempotent done-gate PR emission](../concepts/idempotent-done-gate-pr-emission.md)
- [How to diagnose duplicate done-gate PRs](../howto/diagnose-duplicate-done-gate-prs.md)
- [Typed OODA goal-session deterministic rails](./typed-ooda-goal-session-rails.md)
- [Cross-repo merge authority](./cross-repo-merge-authority.md) — `PrGhClient` / `OpenPrSummary`.
- [Spawn an agent for a goal](./spawn-agent-for-goal.md) — the dispatch path the guard sits in.
