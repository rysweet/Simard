---
title: Progress-evidence gating
description: How Simard refuses to bump a goal's completion percentage unless a verifiable git artifact backs the claim.
last_updated: 2026-05-21
owner: simard
doc_type: concept
related:
  - goal-board-corruption-guards.md
  - ../reference/progress-evidence-api.md
  - ../operations/progress-evidence-kill-switch.md
  - ../howto/diagnose-rejected-progress-claims.md
  - ../../src/goal_curation/progress_evidence.rs
  - ../../src/goal_curation/operations.rs
---

# Progress-evidence gating

## The problem this solves

Before issue [#1967](https://github.com/rysweet/Simard/issues/1967) the OODA
brain's *decide* phase could mark a goal `STATUS: ACHIEVED` and bump its
`GoalProgress::InProgress { percent }` value without any change in the
underlying repository. The brain's text reasoning was treated as ground truth
and persisted directly onto the goal board.

The result, observed live on the production daemon between 2026-05-19 23:06Z
(merge of [#1968](https://github.com/rysweet/Simard/pull/1968)) and 2026-05-21
03:19Z, was a 28-hour cascade of fictional progress: several active goals
drifted upward by 10–40 percentage points despite **zero** new PRs on
`rysweet/Simard` over the same window. The exact per-goal numbers used to
calibrate the test fixtures are pulled from the cognitive-memory store at
implementation time (see acceptance criterion §10.3 in the design); they
are not enumerated here to avoid this concept doc going stale the moment
the daemon is restarted.

Engineer subprocesses spawned, ran, and exited without producing branches or
PRs — but the brain still believed they had made progress, recorded that
belief on the goal board, and surfaced it on the operator dashboard. This
made every downstream signal Simard produced — the dashboard, the meeting
summaries, the priority ordering — quietly false.

The single guiding principle of the fix is:

> **No progress increase without a verifiable git artifact since the last
> update.**

## The three accepted forms of evidence

A proposed increase from `old_percent` to `new_percent` for goal `G` is
accepted if **any one** of the following is true since `G`'s last accepted
update (`G.last_progress_update_at`):

1. **Local commit on an engineer branch.** At least one commit exists on a
   branch matching `engineer/{slug(G.id)}-*` with author-date `>= since`.
2. **PR referencing the goal.** At least one PR (any state) on
   `rysweet/Simard` whose title or body contains either the goal slug or any
   `ref_id` listed in `G.wip_refs`, created at or after `since`.
3. **Merged PR closing a wip-ref issue.** At least one PR with
   `state="MERGED"` and `mergedAt >= since` whose body matches a
   `Closes/Fixes/Resolves #NNNN` clause where `NNNN` is an issue in
   `G.wip_refs`.

If none match, the gate rejects the update, the prior percent is kept, and
the rejection is recorded as a cognitive-memory episode (see
[Audit trail](#audit-trail)).

### Why "any-of" rather than "all-of"

Real progress takes many shapes. A long-running spike may produce commits on
a personal branch before a PR exists. A documentation-only fix may land as a
PR with no engineer branch. A bug closed by an external contributor may merge
without ever touching an engineer branch. Requiring all three signals
simultaneously would block legitimate work; requiring any one of them blocks
only the failure mode we observed — brain output with **zero** corresponding
artifacts.

### What does *not* count as evidence

- Brain text that asserts work was done.
- Engineer subprocess exit codes (an engineer can exit 0 without producing
  output).
- Heartbeat freshness (an engineer can be alive and idle).
- Memory episodes claiming progress (memory is downstream of the gate, not
  upstream).

(Dashboard operator overrides are not "non-evidence" — they are a separate
intentional bypass mechanism. See [Bypass set](#bypass-set--when-the-gate-is-not-consulted) and [Scope and exceptions](#scope-and-exceptions).)

## Bypass set — when the gate is not consulted

The gate guards **percent increases**. It is bypassed for:

- **Non-increase transitions.** Any update where `new_percent <=
  old_percent`. Decreases and same-value writes are always allowed; they
  cannot inflate the dashboard.
- **`Blocked(reason)` transitions.** Blocking a goal keeps its percent at
  the prior value and adds a reason string — this is non-fictional by
  definition.
- **`NotStarted` resets.** Used by `clear_goal_assignment` and similar
  operator paths to wipe state.
- **`Completed` after artifact verification.** The
  `advance_goal/subordinate.rs` Completed path is already guarded by
  `SubordinateProgress::has_artifacts()`. It is still routed through the
  gate for audit consistency, but rule (1) — commit on engineer branch —
  is satisfied by definition, so the gate is a no-op in practice.

## Where the gate lives

The gate is a single trait — `ProgressEvidenceChecker` — and a single
façade function — `update_goal_progress_with_evidence` — both in
`src/goal_curation/`. **Four** OODA-loop call sites that previously called
`update_goal_progress` directly are rewired to go through the façade:

```
ooda_actions/goal_session/advance.rs:57    (assess_only_outcome)
ooda_actions/goal_session/advance.rs:243   (pre-spawn percent bump)
ooda_actions/advance_goal/subordinate.rs:56   (heartbeat → 50%)
ooda_actions/advance_goal/subordinate.rs:223  (Completed after artifacts)
```

A fifth caller — `ooda_actions/advance_goal/subordinate.rs:262` — sets
`Blocked(reason)`, which is in the [bypass set](#bypass-set--when-the-gate-is-not-consulted)
(`Blocked` keeps the prior percent and cannot inflate the dashboard). It
intentionally continues to call `update_goal_progress` directly.

A grep for `update_goal_progress(` in production code therefore returns
exactly **three** direct call sites after the fix: (a) the façade itself,
(b) the dashboard `PUT /api/goals/<id>/progress` handler (an intentional
operator override), and (c) the `Blocked`-path bypass at
`subordinate.rs:262`. Any new direct caller introduced after #1967 must
be justified explicitly (bypass-set membership or operator override).

For the trait shape and exact function signatures, see the
[Progress-evidence API reference](../reference/progress-evidence-api.md).

## Sourcing `since` — the "last update" timestamp

The gate compares evidence to a `since: DateTime<Utc>` timestamp. To remain
useful on legacy on-disk boards that predate this change, `since` is sourced
via a three-step fallback chain:

1. **Primary.** `ActiveGoal.last_progress_update_at` — a new
   `Option<DateTime<Utc>>` field that is set by the gate itself on every
   `Accept`. Goals created after #1967 will have this populated.
2. **Fallback — memory scan.** If the field is `None`, the gate searches
   cognitive memory for the most recent episode whose content starts with
   `"goal progress accepted: "` and contains the goal id. The episode's
   timestamp is used.
3. **Last resort — process start.** If neither of the above is available,
   the gate uses the daemon's process-start timestamp (a `OnceLock` set at
   boot). This guarantees the gate is never a silent open door on a fresh
   daemon.

The schema change to `ActiveGoal` is purely additive
(`#[serde(default, skip_serializing_if = "Option::is_none")]`) — existing
JSON goal boards and fixtures continue to deserialize without migration.

## Audit trail

The gate emits one cognitive-memory episode per non-bypass decision.

**On `Accept`** (low importance, 0.4):

```
goal progress accepted: 64%→72% on improve-simard-dashboard
  — evidence: commit a1b2c3d on engineer/improve-simard-dashboard-2026-05-21 at 2026-05-21T02:14:08Z
```

**On `Reject`** (higher importance, 0.7) — the prefix is exact and stable
so dashboards and consolidation jobs can match it:

```
brain hallucination detected: rejected progress 35%→75% on enhance-simard-meeting-experience
  — no git evidence since last update: no commits on engineer/enhance-simard-meeting-experience-*, no PRs referencing goal, no merged PRs closing #1951 since 2026-05-19T23:06:48Z
```

These episodes flow through the existing cognitive-memory pipeline:

- Greppable on the daemon's stderr (memory writes log there).
- Searchable via `bridges.memory.search("brain hallucination detected")`.
- Surfaced on the operator dashboard via `POST /api/memory/search` with
  `{"query":"brain hallucination detected"}` (the dashboard's memory
  search box uses this endpoint).
- Eligible for consolidation — if the same rejection recurs the
  consolidator promotes it to a semantic memory ("Simard frequently
  hallucinates progress on goal X").

This is the "brain-failure surfacing" piece called out in the
`improve-simard-dashboard` goal: rejections are first-class observable
events, not silent suppressions.

## Scope and exceptions

| Surface | Gated? | Rationale |
|---|---|---|
| OODA decide-phase bumps | **Yes** | This is the meta-bug origin. |
| OODA pre-spawn percent bump | **Yes** | Same path — claims must match artifacts. |
| Subordinate heartbeat (50%) | **Yes** | "Engineer is alive" is not evidence of work. |
| Subordinate Completed (post-artifacts) | Yes (routed; always Accepts) | Routed for audit-trail consistency. |
| `Blocked(reason)` transitions | No (bypass) | Non-increase; cannot inflate dashboard. |
| `NotStarted` resets | No (bypass) | Decrease; cannot inflate dashboard. |
| Decreases & equal-value writes | No (bypass) | Same. |
| Dashboard `PUT /api/goals/<id>/progress` | **No** | Intentional operator override; documented as such. |
| `gh` / `git` not available on daemon host | Gate Rejects | Treat tooling absence as "no evidence". See [kill switch](../operations/progress-evidence-kill-switch.md). |

## Performance

The gate fires only on progress-**increase** attempts — typically a handful
per OODA cycle, not per cycle wall-clock. Each fire executes at most one
`git for-each-ref` + one `git log` + one `gh pr list`. On a quiet repo the
combined wall-time is well under the existing OODA cycle budget. A
per-cycle in-memory cache for `gh pr list` results is reserved for v2 if
profiling identifies it as hot.

## Related work

- [#1582](https://github.com/rysweet/Simard/issues/1582) — Goal board
  corruption guards. Same family of failure (LLM output → goal-board
  damage), different surface (id hallucination vs. percent hallucination).
- [#1951](https://github.com/rysweet/Simard/issues/1951) — Meeting
  experience epic. The "tell Simard what you need and have her actually do
  it" workflow depends on progress claims being truthful.
- [#1957](https://github.com/rysweet/Simard/issues/1957) — Dashboard
  brain-failure surfacing. The hallucination episodes documented above are
  the data source for that surface.
- [#1968](https://github.com/rysweet/Simard/pull/1968) — State-root and
  lock-vs-corruption fixes shipped immediately before #1967 was filed.
