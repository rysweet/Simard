---
title: The no-progress breaker suppresses its own issue storm
description: >
  Why the OODA no-progress breaker no longer auto-files ~15 identical
  `OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)`
  tracking issues in ~2 days. Explains the observed self-amplifying loop — a
  failed `gh` issue link left the escalated goal "untracked", so the idempotence
  guard re-filed the same issue every cycle — and the two additive fixes: a
  durable, restart-surviving suppression marker written BEFORE and independent of
  the `gh` link (the primary storm-stopper), and a terminal-rung criteria
  derivation that stops sweeping derivable-criteria goals into UNCLEAR-CRITERIA
  (the secondary correctness fix). Both are additive; clear-criteria goals behave
  identically.
last_updated: 2026-07-24
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./no-progress-root-cause-resolution.md
  - ./no-progress-terminal-investigation.md
  - ./perpetual-goal-no-progress-exemption.md
  - ./gap-scan-backoff-dedup.md
  - ../reference/no-progress-breaker-storm-suppression-api.md
  - ../reference/no-progress-root-cause-resolution-api.md
  - ../reference/no-progress-breaker-api.md
  - ../howto/diagnose-a-no-progress-breaker-issue-storm.md
  - ../howto/unblock-stuck-ooda-goals.md
---

# The no-progress breaker suppresses its own issue storm

> **Status: implemented.** The durable suppression marker
> (`NO_PROGRESS_SUPPRESSION_MARKER_KIND`), the storm-safe `escalate_with_tracking_issue`,
> the additive `is_breaker_tracking_ref` recognition, and the terminal-rung
> `derive_criteria` helper live in `src/ooda_loop/no_progress.rs`. For the exact
> types and functions see the
> [issue-storm suppression API reference](../reference/no-progress-breaker-storm-suppression-api.md).

## The defect

Over roughly two days the daemon auto-filed **~15 identical** tracking issues,
all sharing one title:

```
OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)
```

That volume is not fifteen independent stuck goals — it is **one** stuck-goal
population re-filing the **same** issue every cycle. The breaker was behaving as
designed at the classification layer (a genuinely-unclear goal *should* escalate
once) but its escalation side effect had two coupled faults: a **broken dedup**
that turned a single stall into a storm, and a **misclassification** that widened
which goals entered that path.

### Fault 1 — the dedup was coupled to a best-effort `gh` link

The [root-cause escalation](./no-progress-root-cause-resolution.md) files a `gh`
tracking issue and links it back to the goal as a `WipRef` so the done-criteria
become measurable. Idempotence was keyed on the presence of that **linked**
tracking ref. But the link is only written when `file_issue()` returns
`Some(FiledIssue)` — which requires both the `gh` call to succeed **and** its
output URL to parse to a bare issue number:

```text
escalate → file_issue() → parse gh URL → Some → link written  → suppressed ✔
                                    └──── None → NO ref written → NOT suppressed ✘
                                                 → next cycle sees "untracked"
                                                 → files the SAME issue again
                                                 → … every cycle → STORM
```

The `UNCLEAR-CRITERIA` population is exactly the one most exposed to this: those
goals have **no** tracked PR/issue by definition, so if the `gh` link ever
failed to land, the goal stayed "untracked" forever and the breaker re-filed on
every subsequent cycle. The in-memory `NoProgressTracker` state that might have
remembered "already escalated" resets on the daemon's periodic exec-reload, so it
could not durably stop the loop either. The storm is a
[self-amplifying loop](./gap-scan-backoff-dedup.md) of the same shape the
Overseer gap-scan backoff addresses — a safeguard observing a condition forever
instead of acting on it once.

### Fault 2 — derivable-criteria goals were misclassified

The terminal rung of the deterministic reasoner splits an unresolved stall by
whether the goal still references open work: open artifacts ⇒ `GENUINELY-STUCK`;
**no** tracked artifact ⇒ `UNCLEAR-CRITERIA` (done-criteria structurally
unmeasurable). But some goals with no *tracked* artifact still have criteria
**derivable from their own description** — a named repo/module, a "PR merged"
phrasing, a measurable threshold. Those were being swept into `UNCLEAR-CRITERIA`
and flagged as unmeasurable when in fact a concrete criterion could be derived.
That both widened the storm-prone population and mislabeled the diagnosis.

## The fix: suppress durably, link best-effort; derive before defaulting

The two faults are coupled — the dedup failure is what turns *any* single stuck
goal into a *storm*, while the misclassification only widens which goals enter
the path — so the primary fix is the **storm-stopper** and the secondary is the
**correctness tightening**.

### Primary — a durable suppression marker, decoupled from linking

Escalation is split into **suppress-then-link**:

1. **Suppress first, durably.** Before `file_issue()` is even attempted, the
   breaker writes a durable **suppression marker** `WipRef`
   (`kind = NO_PROGRESS_SUPPRESSION_MARKER_KIND`, a fixed sentinel `ref_id`) to
   the goal-board store through the existing atomic, single-writer save path, and
   sets the goal `Blocked`. The idempotence guard recognizes this marker
   (`is_breaker_tracking_ref` now matches it), so the goal is suppressed
   **whether or not** any `gh` link ever lands, and the marker **survives a
   restart** because it lives on the durable goal board, not in the resettable
   in-memory tracker.
2. **Link best-effort second.** The breaker then attempts `file_issue()`. On
   success it **upgrades the bare marker in place** to the linked tracking ref
   (so the done-criteria still become measurable) — never appending a second
   `WipRef`. On failure it does nothing further: the goal is already Blocked and
   suppressed, so **no re-file happens next cycle**.

The dedup key is now **durable goal identity**, not "did the `gh` URL parse". A
failed link degrades to "Blocked + suppressed, no linked issue" — no worse than
before the linkage existed, and crucially **not** a re-filing loop.

```text
3 no-action cycles → escalate goal G
        │
        ├─ write durable suppression marker + Blocked   (idempotent, restart-safe)
        │        └─ G is now suppressed regardless of what gh does
        │
        └─ file_issue()
                 ├─ Some(issue) → upgrade marker → linked tracking ref (done-criteria measurable)
                 └─ None        → leave bare marker (Blocked + suppressed, NO re-file)
```

### Secondary — derive criteria before defaulting to UNCLEAR-CRITERIA

At the terminal rung, before an empty-evidence stall defaults to
`UNCLEAR-CRITERIA`, the reasoner calls a pure, total `derive_criteria(goal)`
helper. If it can derive checkable criteria from the goal's own
description/artifacts, the goal proceeds as `GENUINELY-STUCK` (and gets its one
guided engineer with real evidence) instead of being flagged structurally
unmeasurable. The helper is **conservative**: anything it cannot positively
derive returns `None`, preserving the exact legacy `UNCLEAR-CRITERIA` behavior
for genuinely-unclear goals (the synthetic `simard-identity-*` goals). It never
returns empty evidence and never opens a new unbounded re-investigation loop —
the existing `SURFACED_INVESTIGATION_FAILURE_LIMIT` bound still governs the rung.

## Graceful degradation for a truly-unclear goal

A goal that really has no derivable, checkable criteria still reaches exactly
**one** deduplicated breaker outcome: a single filing/annotation, then the goal
sits `Blocked` + suppressed. It is not re-investigated into a storm every cycle,
and — because suppression no longer depends on the `gh` link — it does not
re-file even when issue-link parsing fails. The breaker still surfaces the stall
(via the durable Blocked marker and its WHY), it just does so **once**.

!!! note "Deliberate trade-off — a bare marker is never re-linked"
    Suppression is intentionally prioritized over eventual linking. If the very
    first `file_issue()` fails (e.g. a `gh` outage), the goal keeps a **bare**
    suppression marker and later cycles short-circuit before retrying the link —
    so its done-criteria never become measurable and it stays `Blocked` +
    suppressed but **unlinked, permanently**. This is by design: re-attempting the
    link every cycle is exactly the re-filing storm this feature kills. The
    recovery path is manual (remove the marker; see the runbook). A future
    "re-link on the next cycle only while the marker is still bare" enhancement is
    deliberately out of scope so the storm guarantee stays unconditional. See the
    [API trade-off note](../reference/no-progress-breaker-storm-suppression-api.md#deliberate-trade-off-a-bare-marker-is-never-re-linked).

## Why not backoff, title-level dedup, or an in-memory guard?

- **In-memory guard alone** — insufficient. The `NoProgressTracker` resets on the
  daemon's periodic exec-reload, so a restart re-opens the loop. Suppression must
  be **durable**, hence the goal-board marker.
- **Title/fleet-level dedup** — deliberately **not** added. All ~15 issues share
  a title, but that is a symptom of per-goal re-filing, not many goals colliding.
  Per-durable-goal-id suppression stops the observed storm without risking
  suppression of legitimately-distinct filings that happen to share a title.
- **Exponential backoff** — the right tool for a genuinely *recurring* gap (see
  [gap-scan backoff](./gap-scan-backoff-dedup.md)), but here the correct count is
  **one** filing per stuck goal, ever — a hard idempotent cap, not a rate limit.

## How this relates to the sibling gates

The suppression marker sits **inside** the existing escalation side effect
(`escalate_with_tracking_issue`), shared by every breaker escalation path
(on-transition, [re-investigation](../reference/no-progress-reinvestigation-api.md),
and the bounded [surfaced-failure escalation](./no-progress-terminal-investigation.md)).
It does not change *which* goals escalate — the
[perpetual exemption](./perpetual-goal-no-progress-exemption.md) still runs first,
and the [root-cause ladder](./no-progress-root-cause-resolution.md) still decides
the rung. It only makes the escalation **idempotent and restart-surviving**, and
lets `derive_criteria` sharpen the terminal diagnosis.

## What an operator sees now

- A genuinely-stuck, unclear goal produces **one** `ooda-stuck` tracking issue —
  not ~15 duplicates — even if the `gh` link fails or the daemon restarts.
- The goal shows `Blocked` with a breaker suppression marker in its `wip_refs`;
  on a successful filing that marker is the linked tracking issue.
- A goal whose criteria are derivable from its own description is investigated as
  `GENUINELY-STUCK` rather than flagged `UNCLEAR-CRITERIA`.

See the [how-to: diagnose a no-progress breaker issue storm](../howto/diagnose-a-no-progress-breaker-issue-storm.md)
for confirming suppression and closing the duplicate issues, and the
[issue-storm suppression API reference](../reference/no-progress-breaker-storm-suppression-api.md)
for the exact types.

## See also

- [Issue-storm suppression API reference](../reference/no-progress-breaker-storm-suppression-api.md) — the marker, the storm-safe escalation, and `derive_criteria`.
- [The no-progress breaker explains WHY and self-resolves before escalating](./no-progress-root-cause-resolution.md) — the ladder whose escalation this makes idempotent.
- [The terminal no-progress stall never parks a goal with empty evidence](./no-progress-terminal-investigation.md) — the sibling terminal-rung guard and its re-investigation bound.
- [Gap-scan dedup & exponential backoff](./gap-scan-backoff-dedup.md) — the Overseer's sibling self-amplifying-loop fix.
- [How-to: diagnose a no-progress breaker issue storm](../howto/diagnose-a-no-progress-breaker-issue-storm.md) — the operator runbook.
