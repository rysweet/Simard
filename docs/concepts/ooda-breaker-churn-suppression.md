---
title: The OODA breaker quarantines terminal UNCLEAR-CRITERIA goals and dedups reblock issues
description: >
  Why the OODA no-progress breaker no longer churns on UNCLEAR-CRITERIA goals —
  ~33 open `ooda-stuck` issues and ~44 open `recurring_goal_reblock in
  simard::overseer` stewardship issues, with ~13 stuck + ~8 reblock filed in a
  single day. Two additive fixes end the residual churn the admission-path work
  (PRs #4939/#4941) does not cover: (1) a terminal-quarantine rung
  (`NoProgressResolution::QuarantineTerminal`) that stops re-scheduling and
  re-filing a goal once it trips `UNCLEAR-CRITERIA` after the bounded guided
  retry, keyed on a durable, injection-safe quarantine marker; and (2)
  reblock-issue signature stabilization so recurrences of the same root cause
  collapse to one stewardship issue instead of one per re-observation. Both are
  additive; clear-criteria goals behave identically. Quarantine is reversible.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./no-progress-root-cause-resolution.md
  - ./no-progress-terminal-investigation.md
  - ./no-progress-breaker-storm-suppression.md
  - ./ooda-reinvestigate-blocked-goals.md
  - ./overseer-root-cause-why.md
  - ./steerable-ooda-daemon.md
  - ../reference/ooda-breaker-churn-suppression-api.md
  - ../reference/no-progress-breaker-storm-suppression-api.md
  - ../howto/quarantine-and-recover-an-unclear-ooda-goal.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# The OODA breaker quarantines terminal UNCLEAR-CRITERIA goals and dedups reblock issues

> **Status: implemented.** The terminal-quarantine rung
> (`NoProgressResolution::QuarantineTerminal`, the `resolution_for_why`
> surfaced-failure parameter, and the durable quarantine marker) lives in
> `src/goal_curation/no_progress_breaker.rs`; the side effects (block, mark,
> and — the single churn-stopping change — the re-schedule exclusion in
> `reinvestigate_bare_blocked_goals`) live in the curate-phase adapter
> `src/ooda_loop/no_progress.rs` (this handler **replaces** the adapter's prior
> inline escalate-at-limit branch). The reblock-issue signature stabilization is
> the new `fold_volatile_goal_ids` helper applied inside `problem_to_run_brief`
> in `src/overseer/observer.rs`, upstream of the existing
> `dedup::failure_signature`; the exported `root_cause_signature` is **not** the
> reblock dedup key and is left untouched. For exact types and functions see the
> [churn-suppression API reference](../reference/ooda-breaker-churn-suppression-api.md).

## The defect

In a single day the daemon accumulated **~33 open `ooda-stuck` issues** and
**~44 open `recurring_goal_reblock in simard::overseer` stewardship issues**,
with roughly **13 stuck + 8 reblock filed in the last 24 hours** — e.g. a run of
issues all titled:

```
OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)
```

This is **not** dozens of independent stuck goals. It is a small population of
goals whose done-criteria are structurally **unmeasurable** (`UNCLEAR-CRITERIA`)
looping through the breaker forever, plus the Overseer re-observing the same
re-block condition each cycle and filing a fresh stewardship issue every time.

The [issue-storm suppression](./no-progress-breaker-storm-suppression.md) work
already made a *single* escalation idempotent via the durable suppression
marker. But suppression stops **re-filing**; it does not stop the goal from
being **re-scheduled**. An `UNCLEAR-CRITERIA` goal that has exhausted the bounded
guided-retry ladder was still picked up by the re-investigation pass every cycle,
re-classified, re-surfaced, and re-escalated — and the Overseer, watching that
re-block, filed a new `recurring_goal_reblock` issue each time because its
dedup signature drifted with volatile goal identifiers.

Two coupled faults:

1. **No terminal rung.** After the guided retry, a permanently-unclear goal had
   nowhere to land. It kept cycling through `SurfaceInvestigationFailure` /
   re-escalation instead of being parked **terminally** and removed from the
   schedule.
2. **Unstable reblock signature.** The Overseer's `recurring_goal_reblock`
   stewardship issue was keyed on a signature that embedded volatile identifiers
   (`simard-identity-*`, `goal-<n>`), so each recurrence looked "new" and filed
   its own issue instead of collapsing onto the existing one.

> This scope is deliberately **disjoint** from the goal-admission hardening in
> PRs #4939 (centralized admission gate) and #4941 (declarative standing seed
> goals). Those own *which goals are admitted*; this owns the *residual churn*
> after a goal is already stuck — re-scheduling and reblock-issue dedup only.

## Fix 1 — a terminal quarantine rung

The [root-cause ladder](./no-progress-root-cause-resolution.md) already routes a
stall down self-resolving rungs and, at the bottom, spawns **one** guided
engineer then surfaces a bounded investigation gap
(`SURFACED_INVESTIGATION_FAILURE_LIMIT`, see
[terminal investigation](./no-progress-terminal-investigation.md)). Quarantine is
the **missing terminal rung after that bound**: the rung that says "this goal has
exhausted every machine-resolvable and guided path; stop spending cycles on it."

### The variant

`NoProgressResolution::QuarantineTerminal` is a new terminal variant of the pure
resolution enum. It:

- carries the `surfaced_count` (the number of consecutive evidence-less surfaced
  failures that drove the goal here) as **real evidence** — it never renders
  `evidence=[(none)]`, preserving the
  [never-empty-evidence invariant](./no-progress-terminal-investigation.md);
- reports `is_terminal() == true`, so callers know no further rung follows.

`resolution_for_why` gains **one additive trailing parameter**,
`surfaced_failures` — its existing `(consecutive, why, guided_retry_used)`
parameters are unchanged. On the evidence-less terminal rung (an
`UNCLEAR-CRITERIA` / `GENUINELY-STUCK` WHY where `guided_retry_used` is already
true and the guided investigation produced no evidence), once
`surfaced_failures >= SURFACED_INVESTIGATION_FAILURE_LIMIT` (3) it returns
`QuarantineTerminal` instead of surfacing yet another investigation gap. Every
other class, and the same rung **below** the threshold, is unchanged — quarantine
is strictly the top rung of an already-bounded ladder, reached only after the
guided engineer has run and the surfaced-failure bound has been hit.

Crucially, this **replaces** the limit decision that today lives inline in the
curate-phase adapter: the current `SurfaceInvestigationFailure` handler escalates
at the bound via `surfaced_failure_escalation_issue`. That escalate-at-limit
branch is removed and replaced by the `QuarantineTerminal` handling below, so a
bounded-out goal quarantines instead of escalating (never both).

### The durable, injection-safe quarantine marker

Quarantine reuses the durable `WipRef` marker infrastructure already used for
[storm suppression](./no-progress-breaker-storm-suppression.md), with its own
kind:

- `WipRef.kind = "ooda-breaker-quarantine"` — a novel kind, so every other
  `wip_refs` consumer (`has_derivable_signal`, `stuck_evidence`,
  `artifact_evidence`, the stale-assignment sweep) ignores it via its `_ => None`
  fall-through. The marker is inert to completion/liveness logic; only the
  quarantine predicate and the re-schedule filter read it.
- `WipRef.ref_id` is a **fixed sentinel constant** — **never** derived from goal
  text — so a goal description can never smuggle content into the marker or forge
  another goal's quarantine.

`apply_resolution_side_effects` handles `QuarantineTerminal` by (a) setting the
goal `Blocked` with a WHY-bearing reason (the marker prefix + the surfaced-count
evidence) and (b) writing the quarantine marker **idempotently** through the
existing atomic, single-writer goal-board save path. Marker writes **fail
closed**: if the marker cannot be persisted, no terminal claim is made and the
goal retries next cycle, never silently dropping the quarantine.

### The single churn-stopping change: exclude quarantined goals from re-scheduling

`reinvestigate_bare_blocked_goals` — the pass that sweeps blocked goals back into
investigation each cycle — now **excludes any goal carrying the quarantine
marker**. This is the one change that actually stops the churn: a quarantined
goal is no longer selected, re-classified, re-surfaced, or re-escalated. It sits
`Blocked` + quarantined until a human intervenes.

```text
UNCLEAR-CRITERIA stall
        │  (guided engineer runs once; surfaced-failure gap bounded at 3)
        ▼
surfaced_failures reaches SURFACED_INVESTIGATION_FAILURE_LIMIT
        │
        ▼
resolution_for_why → QuarantineTerminal(surfaced_count)
        │
        ├─ Blocked (WHY + surfaced-count evidence, never (none))
        ├─ durable quarantine marker written (idempotent, fail-closed)
        └─ reinvestigate_bare_blocked_goals SKIPS it forever after
                 └─ no re-schedule → no re-classify → no re-file → churn stops
```

## Fix 2 — stabilize the reblock-issue signature

`recurring_goal_reblock in simard::overseer` is the **observed** `dedup_key` /
issue-title text the Overseer emits when it re-observes a goal being re-blocked
— not a code constant. That stewardship issue is deduplicated through the
existing **failure-signature** path, not through `root_cause_signature`:

```text
observer::problem_to_run_brief → OrchestratorRunBrief { failure_kind: problem.dedup_key, error_text }
                               → stewardship::failure_signature(failure_kind, error_text)
```

`failure_signature` SHA-256s `failure_kind` **verbatim** and normalizes only
`error_text`. So when `problem.dedup_key` embeds **volatile** identifiers —
synthetic `simard-identity-*` goal ids and positional `goal-<n>` slugs — the
`failure_kind` (and therefore the signature) drifts every cycle, and each
re-observation files its own issue.

> Note: the Overseer's exported `root_cause_signature` helper has **no
> non-test caller** keying this issue; it is *not* the reblock dedup key and is
> left untouched by this fix.

The fix folds those volatile tokens to stable placeholders **before**
`problem.dedup_key` becomes `failure_kind`, inside `problem_to_run_brief`, via a
**new** pure helper `fold_volatile_goal_ids`. It is deliberately named to avoid
the pre-existing private `dedup::normalize_for_signature` (the message
UUID-redactor `failure_signature` already uses internally — a different function
in a different module). The fold is conservative: it only rewrites the known
volatile id shapes and leaves everything else byte-for-byte, so distinct root
causes still get distinct signatures. Every recurrence of the same underlying
re-block cause then collapses onto **one** stewardship issue, mirroring the
Overseer's existing [failure-signature dedup](./overseer-root-cause-why.md).

## Reversibility — quarantine is a park, not a grave

Quarantine is **terminal for the daemon, reversible for a human**. When an
operator un-blocks the goal with the single-id escape hatch
(`simard goal unblock <goal-id>`, or by giving it a checkable finish condition),
the quarantine marker is cleared and the goal earns a **fresh** bounded window:
the surfaced-failure counter and quarantine state reset, so it is re-investigated
like any newly-unblocked goal rather than re-quarantining immediately.
`simard goal unblock-all` is deliberately scoped to the brain-failure safeguard
marker and does **not** mass-clear quarantines — quarantine is a considered
terminal state, so clearing it is an explicit per-goal decision. See
[Quarantine and recover an unclear OODA goal](../howto/quarantine-and-recover-an-unclear-ooda-goal.md).

The right permanent fix for a quarantined goal is almost always to **make its
done-criteria machine-checkable** (name a specific issue that must be observed
`CLOSED`, a PR observed `MERGED`, or a file/command whose output the done-gate
can verify) — or to drop it if it is out of scope.

## Why not just lengthen the surfaced-failure bound?

- **Raising `SURFACED_INVESTIGATION_FAILURE_LIMIT`** only delays the storm; a
  *permanently* unclear goal re-investigates forever at any finite bound. The
  correct terminal count is a hard **stop**, not a larger rate.
- **Deleting the goal automatically** would be destructive and irreversible — a
  transient misclassification would silently lose real work. Quarantine parks the
  goal visibly and reversibly instead.
- **Title-level issue dedup** was rejected for the same reason as in
  [storm suppression](./no-progress-breaker-storm-suppression.md#why-not-backoff-title-level-dedup-or-an-in-memory-guard):
  a shared title is a *symptom* of per-goal re-filing, not many goals colliding.
  Root-cause-signature dedup collapses genuine recurrences without risking
  suppression of legitimately-distinct filings.

## What an operator sees now

- A permanently-unclear goal produces **one** `ooda-stuck` escalation, then sits
  `Blocked` with a `ooda-breaker-quarantine` marker in its `wip_refs` — it is not
  re-investigated or re-escalated every cycle.
- Recurrences of the same re-block root cause collapse onto **one**
  `recurring_goal_reblock in simard::overseer` stewardship issue instead of one
  per cycle.
- Clear-criteria goals, and unclear goals still inside the guided-retry ladder,
  behave exactly as before — quarantine is strictly the terminal rung.

## Interaction with the admission-path work (PRs #4939/#4941)

This feature edits shared OODA-core files, so it lands **one-at-a-time in the
`ooda-core` sequence group** and deliberately does **not** touch the goal-admission
gate owned by PRs #4939/#4941. Merging those PRs relieves the *inflow* of
unclear goals; this feature terminally parks the *residue* that still gets
through and stops the reblock-issue churn. The two are complementary and
non-overlapping.

## See also

- [Churn-suppression API reference](../reference/ooda-breaker-churn-suppression-api.md) — the variant, the marker, the resolution signature, and the re-schedule filter.
- [The no-progress breaker suppresses its own issue storm](./no-progress-breaker-storm-suppression.md) — the durable-marker infrastructure quarantine reuses.
- [The terminal no-progress stall never parks a goal with empty evidence](./no-progress-terminal-investigation.md) — the surfaced-failure bound quarantine sits above.
- [Overseer Root-Cause (WHY) Principle](./overseer-root-cause-why.md) — the signature dedup this stabilizes.
- [Quarantine and recover an unclear OODA goal](../howto/quarantine-and-recover-an-unclear-ooda-goal.md) — the operator runbook and reversal path.
