---
title: wip-ref liveness reconcile API reference
description: Reference for the #4428 wip_refs liveness reconciliation — the two per-cycle pruning prongs that make ActiveGoal::has_live_in_flight_ref()'s liveness precondition true before the never-idle breaker classifies. Prong 1 drops session/branch/engineer refs for a dead tmux session in sweep_stale_assignments_with_sessions; Prong 2 prunes merged/closed PR refs via a pure prune_merged_pr_refs over an in-memory open-PR set fetched once per cycle through the existing PrGhClient::list_open_prs path (no new git/gh shell parse). Fail-open: unparseable ref_id is kept, a fetch error skips PR pruning that cycle.
last_updated: 2026-07-22
review_schedule: as-needed
owner: simard
doc_type: reference
status: implemented
related:
  - ../concepts/research-goal-never-idle.md
  - ./research-goal-never-idle-rail-api.md
  - ./standing-research-goal-novelty-directive-api.md
  - ./no-progress-breaker-api.md
  - ../howto/keep-the-research-goal-never-idle.md
  - ../../src/ooda_loop/cycle.rs
  - ../../src/ooda_loop/no_progress.rs
  - ../../src/goal_curation/types.rs
  - ../../src/stewardship/merge_authority.rs
---

# wip-ref liveness reconcile API reference

> **Status: implemented.** The two per-cycle liveness prongs live in
> [`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
> (Prong 1 sweep + Prong 2 fetch/invocation) and the pure prune core in
> [`src/ooda_loop/no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/no_progress.rs)
> — the same module that houses the round-1 `classify_standing_idle` rail.
> The open-PR set is read through the existing
> [`PrGhClient::list_open_prs`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs)
> path — **no new git/gh shell parse is added**. The guarded precondition is
> [`ActiveGoal::has_live_in_flight_ref()`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/types.rs),
> whose behaviour is **unchanged** (still IO-free); #4428 only guarantees its
> input, `wip_refs`, is liveness-reconciled first.

This reference specifies the API added in issue **#4428**. For the rationale, see
[The standing research goal never idles — an idle cycle is a fault](../concepts/research-goal-never-idle.md#liveness-precondition-wip_refs-are-reconciled-before-the-breaker-4428).
It builds directly on the never-idle rail documented in the
[research-goal never-idle rail API reference](./research-goal-never-idle-rail-api.md):
that rail's `ResearchInFlight` exemption is only sound because the prongs below
make `has_live_in_flight_ref()` reflect true liveness.

## Contents

- [The defect this closes (NEW-1)](#the-defect-this-closes-new-1)
- [The guarded precondition: `has_live_in_flight_ref`](#the-guarded-precondition-has_live_in_flight_ref)
- [Prong 1 — dead-session ref drop](#prong-1-dead-session-ref-drop)
- [Prong 2 — merged/closed PR ref prune](#prong-2-mergedclosed-pr-ref-prune)
- [Cycle ordering](#cycle-ordering)
- [Fail-open decisions](#fail-open-decisions)
- [Guarantees](#guarantees)
- [What is unchanged](#what-is-unchanged)
- [Tests](#tests)
- [See also](#see-also)

## The defect this closes (NEW-1)

The never-idle rail's `ResearchInFlight` exemption
([classify_standing_idle](./research-goal-never-idle-rail-api.md#shared-classifier-classify_standing_idle))
keys on `has_live_in_flight_ref()`, which tests ref **kind**
(`pr` / `branch` / `session` / `engineer`) and — by design — performs **no IO**.
Before #4428 nothing pruned stale refs before the breaker ran, so:

- a standing research goal whose PR **merged** kept its `pr` ref (a merged PR's ref
  was only removed via `roll_to_new_cycle`, which fires only when a goal reaches
  terminal status — and standing research goals are designed to **never** terminate); and
- a goal whose engineer **tmux session died** had `assigned_to` cleared by the
  stale-assignment sweep but its `session` / `branch` refs **left in place**.

Either lingering ref reads as "live", so the goal is classified `ResearchInFlight`
on **every** subsequent no-action cycle: never faulted, never re-oriented, never
logged. It silently idles indefinitely — the #4399 loophole re-opened behind a
narrower gate.

**Fix:** make `wip_refs` reflect only **LIVE** artifacts **before** the breaker
classifies, so the kind-based guard is sound. Two in-memory prongs do this; the
guard itself is untouched.

## The guarded precondition: `has_live_in_flight_ref`

Behaviour is **unchanged** from the round-1 rail (#4399); #4428 adds only a
doc-note recording the precondition the prongs now satisfy.

```rust
impl ActiveGoal {
    /// True if this goal holds a `wip_ref` of a LIVE kind
    /// (`pr` / `branch` / `session` / `engineer`, case-insensitive; `issue` and
    /// unknown kinds are deny-by-default). Reads only in-memory state; performs
    /// NO IO and never panics.
    ///
    /// PRECONDITION (issue #4428): the caller MUST have liveness-reconciled
    /// `wip_refs` THIS cycle before relying on this result, because this function
    /// checks ref *kind*, not artifact liveness. That reconciliation happens in
    /// two places, both BEFORE the no-progress breaker classifies:
    ///   1. `sweep_stale_assignments_with_sessions` (cycle start) drops
    ///      session/branch/engineer refs for a dead tmux session, and
    ///   2. the per-cycle merged-PR reconcile (`prune_merged_pr_refs`, after Act)
    ///      drops `pr` refs whose number is no longer in the open-PR set.
    /// With that precondition met, a lingering merged/dead ref can no longer make
    /// a genuinely-idle research goal read as `ResearchInFlight` forever.
    pub fn has_live_in_flight_ref(&self) -> bool { /* unchanged */ }
}
```

## Prong 1 — dead-session ref drop

**Site:**
[`sweep_stale_assignments_with_sessions`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
(runs at **cycle start**, already before the breaker). Extends the existing
stale-assignment sweep: when it clears a goal's `assigned_to` because the goal's
tmux session is **not** in `live_sessions`, it **also drops** that goal's
`session` / `branch` / `engineer` `wip_refs`.

```rust
/// Cycle-start sweep. For every active goal whose `assigned_to` names a tmux
/// session NOT in `live_sessions`, clear the assignment, reset to NotStarted, AND
/// drop that goal's session/branch/engineer wip_refs — the dead engineer's live
/// working artifacts. `pr` and `issue` refs are KEPT: a PR outlives the session
/// (handled by Prong 2) and an issue is a durable record, not open work.
///
/// No-op when `live_sessions` is empty (unchanged guard) — avoids clearing every
/// assignment when running outside tmux (e.g. CI). Pure/in-memory; no IO.
pub(crate) fn sweep_stale_assignments_with_sessions(
    board: &mut GoalBoard,
    live_sessions: &std::collections::HashSet<String>,
) {
    if live_sessions.is_empty() {
        return;
    }
    const DEAD_KINDS: [&str; 3] = ["session", "branch", "engineer"];
    for goal in board.active.iter_mut() {
        let is_stale = goal
            .assigned_to
            .as_deref()
            .is_some_and(|s| !live_sessions.contains(s));
        if is_stale {
            let session = goal.assigned_to.take().unwrap_or_default();
            eprintln!(
                "[simard] OODA start: cleared stale assignment '{}' for goal '{}' \
                 (dropping dead-session working refs)",
                session, goal.id
            );
            goal.status = GoalProgress::NotStarted;
            goal.wip_refs
                .retain(|w| !DEAD_KINDS.iter().any(|k| w.kind.eq_ignore_ascii_case(k)));
        }
    }
}
```

### Correlation is **by goal**, not by session id

Spawn records only `assigned_to = agent_name`; `session` / `branch` / `engineer`
refs are **not** individually keyed to a session id. So correlation is by goal: the
working refs on the goal being swept for a dead session belong to that dead
engineer, and are dropped together. `pr` / `issue` refs are always kept — a PR can
outlive the engineer session and is Prong 2's responsibility.

## Prong 2 — merged/closed PR ref prune

Two parts: a **pure** prune core (unit-testable, IO-free) and a thin **prod-only**
wrapper that fetches the open-PR set once per cycle.

### Pure core — `prune_merged_pr_refs`

```rust
/// Pure, IO-free. For each ACTIVE goal, retain a `pr` wip_ref IFF its parsed
/// number is in `open_prs`; non-`pr` refs are always kept. A `pr` ref whose id
/// does not parse is KEPT (see fail-open) and a warning is logged. Returns the
/// pruned (goal_id, ref_id) pairs for logging/assertions.
///
/// `open_prs` is the set of currently-open PR numbers (from `list_open_prs`).
/// An empty set means "no open PRs" and correctly prunes every `pr` ref.
pub(crate) fn prune_merged_pr_refs(
    board: &mut GoalBoard,
    open_prs: &std::collections::HashSet<u32>,
) -> Vec<(String, String)> {
    let mut pruned = Vec::new();
    for goal in board.active.iter_mut() {
        let goal_id = goal.id.clone();
        goal.wip_refs.retain(|w| {
            if !w.kind.eq_ignore_ascii_case("pr") {
                return true; // non-PR refs untouched
            }
            match w.ref_id.trim_start_matches('#').parse::<u32>() {
                Ok(n) => {
                    let live = open_prs.contains(&n);
                    if !live {
                        pruned.push((goal_id.clone(), w.ref_id.clone()));
                    }
                    live // prune (drop) when not in the open set
                }
                Err(_) => {
                    // fail-open: unparseable id → keep the ref, warn.
                    tracing::warn!(
                        target: "simard::ooda", goal = %goal_id, ref_id = %w.ref_id,
                        "pr wip_ref id did not parse as u32 — keeping ref (not pruning)"
                    );
                    true
                }
            }
        });
    }
    pruned
}
```

- **`ref_id` parse rule:** `ref_id.trim_start_matches('#').parse::<u32>()` — the
  established repo normalization (a `pr` ref id is a bare number, occasionally
  `#`-prefixed) compared against
  [`OpenPrSummary.number: u32`](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs).
- **Scope:** ACTIVE (non-terminal) goals only; backlog/archived are not touched.

### Prod wrapper — fetch the open set once per cycle

```rust
/// Prod-only. Fetch the open-PR set once via the existing PrGhClient path and
/// feed it to the pure `prune_merged_pr_refs`. Fail-open: on a fetch error,
/// surface it (non-fatal) and prune NOTHING this cycle. On Ok, build the number
/// set and prune. NO new git/gh shell parse — reuses `list_open_prs`.
fn reconcile_merged_prs(board: &mut GoalBoard, client: &dyn PrGhClient, repo: &str) {
    match client.list_open_prs(repo, OPEN_PR_RECONCILE_LIMIT) {
        Ok(open) => {
            let set: std::collections::HashSet<u32> =
                open.iter().map(|s| s.number).collect();
            let pruned = prune_merged_pr_refs(board, &set);
            for (goal_id, ref_id) in pruned {
                tracing::info!(
                    target: "simard::ooda", goal = %goal_id, ref_id = %ref_id,
                    "pruned merged/closed PR wip_ref (not in open-PR set)"
                );
            }
        }
        Err(e) => {
            eprintln!(
                "[simard] merged-PR reconcile: list_open_prs failed — skipping PR \
                 prune this cycle (fail-open): {e}"
            );
        }
    }
}
```

Production wires the concrete `RealPrGhClient`; tests call the **pure**
`prune_merged_pr_refs` directly with an in-memory `HashSet<u32>`, so no unit test
touches the network or `gh`.

#### `OPEN_PR_RECONCILE_LIMIT` — must never truncate

`list_open_prs(repo, limit)` forwards `limit` to `gh pr list --limit`, which **caps**
the result set. This is a **completeness-critical** input: the pure prune treats the
returned set as authoritative, so a `pr` ref whose PR is genuinely open but **beyond
the limit** would be absent from the set and wrongly pruned — wiping a live in-flight
ref, i.e. the exact round-1 finding-#1 (F1) regression the fail-open rules elsewhere
prevent. Fail-open does **not** cover a silently truncated `Ok(...)`.

Therefore `OPEN_PR_RECONCILE_LIMIT` MUST be set high enough that the repo's open-PR
count can never reach it (e.g. `1000` — far above any realistic simultaneous-open-PR
count for this repo). Define it as a module `const OPEN_PR_RECONCILE_LIMIT: u32` next
to `reconcile_merged_prs` in `cycle.rs`. If a future repo could plausibly approach the
cap, the wrapper must page until the listing is exhausted before pruning; until then
the high constant is the guarantee. This invariant is load-bearing: an accidental low
limit reintroduces F1.

## Cycle ordering

Both prongs are **load-bearing on ordering** — they must complete before the
no-progress breaker classifies (`apply_no_progress_breaker*`):

```
cycle start
  └─ sweep_stale_assignments_with_sessions   ← Prong 1 (dead-session refs dropped)
  … Orient / Decide / Act …
  └─ reconcile_merged_prs                     ← Prong 2 (merged PR refs pruned)
  └─ apply_no_progress_breaker* → classify_standing_idle / has_live_in_flight_ref
      (now sees only LIVE refs)
```

Prong 1 already sits at cycle start. Prong 2 is invoked **after the Act phase but
before** the breaker sites so a PR merged during this very cycle is pruned before
its goal is classified.

## Fail-open decisions

The reconcile must never wipe a **live** ref (that reintroduces the round-1 F1
regression), so every ambiguous case errs toward **keeping** the ref:

| Situation | Behaviour | Why |
| --- | --- | --- |
| `pr` `ref_id` fails to parse as `u32` | **Keep** the ref, `warn` | Malformed-but-possibly-live; never drop on a guess. |
| `list_open_prs` returns `Err` | **Skip** PR pruning this cycle (prune nothing), surface the error | Wiping a live PR ref → round-1 bug; skipping merely delays a merged-PR fault by a cycle or two, never forever. |
| `list_open_prs` returns `Ok([])` | **Prune all** `pr` refs | Genuinely no open PRs ⇒ every `pr` ref is stale (correct). |
| ref kind is `session`/`branch`/`engineer` on a live-session goal | **Keep** | Only Prong 1 (dead session) removes those. |
| ref kind is `issue` | **Keep** (both prongs) | Durable record, not open work; already deny-by-default in the liveness guard. |

The asymmetry is deliberate: a **delayed** fault costs a cycle; a **wrongly wiped**
live ref costs correctness.

## Guarantees

- **Precondition satisfied.** After both prongs run, every remaining `pr` /
  `session` / `branch` / `engineer` ref on an active goal corresponds to a live
  artifact (open PR / live tmux session), so `has_live_in_flight_ref()` — and thus
  the `ResearchInFlight` exemption — is sound.
- **No new brittle parse.** Prong 2 reuses `PrGhClient::list_open_prs`; no new
  `git`/`gh` shell invocation or ad-hoc output parsing is introduced.
- **Pure, testable core.** `prune_merged_pr_refs` and the Prong-1 retain are
  IO-free, total, and panic-free; unit tests exercise them with in-memory sets.
- **Fail-open.** The reconcile can only ever remove a dead/merged ref, never a
  live one — round-1 finding #1 (preserve a live in-flight ref) cannot regress —
  **provided** `OPEN_PR_RECONCILE_LIMIT` never truncates the open-PR listing (see
  [`OPEN_PR_RECONCILE_LIMIT` — must never truncate](#open_pr_reconcile_limit-must-never-truncate)).
  Err and unparse are covered; a silently truncated `Ok(...)` is the one way a live
  ref could be wiped, so the limit is set well above the repo's open-PR count.
- **Once per cycle.** The open-PR set is fetched at most once per cycle and shared
  by the pure prune, so "never idle" cannot become a tight PR-listing loop.
- **In-memory.** Both prongs mutate the in-memory board; the change is persisted by
  the cycle's normal commit path (no extra write).

## What is unchanged

- **`has_live_in_flight_ref()` body** — still IO-free, deny-by-default on kind; only
  a doc-note is added recording the #4428 precondition.
- **The never-idle rail** — `classify_standing_idle`, `apply_standing_idle`,
  `StandingIdle` / `ResearchInFlight`, the `research_idle_faults` field, and both
  breaker sites are unchanged. See the
  [rail API reference](./research-goal-never-idle-rail-api.md).
- **`roll_to_new_cycle`** — untouched; it still `wip_refs.clear()`s on re-orient.
  #4428 specifically does **not** use the "N idle cycles then fault regardless"
  approach for open PRs, because faulting a long-in-review open PR would call
  `roll_to_new_cycle` and wipe the live ref (the round-1 F1 bug). A live PR ref
  survives the prune and is never faulted while open.
- **`PrGhClient` / `OpenPrSummary` / `list_open_prs`** — reused as-is; no signature
  change. See the
  [merge-authority source](https://github.com/rysweet/Simard/blob/main/src/stewardship/merge_authority.rs).
- **Benign exemption** for non-research standing goals and the base breaker —
  unchanged.

## Tests

All hermetic — no network, no live `gh`; Prong 2 tests drive the **pure**
`prune_merged_pr_refs` with an in-memory `HashSet<u32>`. Placement follows the
round-1 rail: the pure-core / classifier cases live in
[`src/ooda_loop/tests_no_progress.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/tests_no_progress.rs)
(alongside the existing `classify_standing_idle` tests), and the Prong-1 sweep
cases extend the inline `mod tests` in
[`src/ooda_loop/cycle.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_loop/cycle.rs)
(next to the existing `sweep_stale_assignments_with_sessions` tests):

- **(a) Merged-PR fault** — a research goal whose ONLY `wip_ref` is a merged/closed
  PR (number **not** in the open set): after `prune_merged_pr_refs`, a NO-ACTION
  cycle classifies as **`ResearchFault`** — counted in `research_idle_faults`,
  re-oriented, stays active, **never `Blocked`** and `fired() == false`.
  *(RED before #4428: the stale `pr` ref made it `ResearchInFlight` forever.)*
- **(b) Dead-session ref drop** — a research goal with a `session`/`branch`
  `wip_ref` whose tmux session is DEAD: `sweep_stale_assignments_with_sessions`
  (given a `live_sessions` set excluding it) drops the ref, so the next NO-ACTION
  cycle **faults + re-orients** (not `ResearchInFlight`-forever).
  *(RED before #4428: the sweep cleared `assigned_to` but left the ref.)*
- **(c) Open-PR regression guard** — a research goal with a genuinely-OPEN PR ref
  (its number **in** the open set) **survives** `prune_merged_pr_refs`: still
  `ResearchInFlight`, `wip_refs` **and** `assigned_to` preserved, **not** faulted
  (round-1 finding #1 intact).
- **Fail-open unit tests** — an unparseable `pr` `ref_id` is **kept** (not pruned);
  an `Ok([])` open set prunes all `pr` refs; a non-`pr` (`issue`) ref is never
  pruned by Prong 2.

## See also

- [Concept: the standing research goal never idles](../concepts/research-goal-never-idle.md)
  — the liveness precondition section motivating these prongs.
- [Research-goal never-idle rail API reference](./research-goal-never-idle-rail-api.md)
  — the `ResearchInFlight` exemption these prongs make sound.
- [How to keep the research goal never idle](../howto/keep-the-research-goal-never-idle.md).
- [No-progress breaker API reference](./no-progress-breaker-api.md) — the base
  breaker the rail extends.
