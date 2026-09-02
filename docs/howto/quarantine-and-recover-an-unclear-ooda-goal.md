---
title: Quarantine and recover an unclear OODA goal
description: >
  Runbook for the terminal-quarantine rung of the OODA no-progress breaker.
  Explains how to recognise a goal that has been terminally quarantined after
  exhausting the guided-retry ladder on an UNCLEAR-CRITERIA classification, why
  quarantine stops the `ooda-stuck` / `recurring_goal_reblock` churn, and how to
  recover a quarantined goal — by giving it a machine-checkable finish condition
  (the durable fix) or by un-blocking it for a fresh bounded window.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/ooda-breaker-churn-suppression.md
  - ../concepts/no-progress-breaker-storm-suppression.md
  - ../concepts/no-progress-terminal-investigation.md
  - ../reference/ooda-breaker-churn-suppression-api.md
  - ../reference/simard-cli.md
  - ./unblock-stuck-ooda-goals.md
  - ./diagnose-a-no-progress-block.md
  - ./diagnose-a-no-progress-breaker-issue-storm.md
---

# Quarantine and recover an unclear OODA goal

## Symptom

Before this fix, a goal whose done-criteria the daemon cannot machine-check
(`UNCLEAR-CRITERIA`) churned: every cycle it was re-investigated, re-surfaced,
and re-escalated. The visible symptoms were a growing pile of duplicate
tracking issues —

```
OODA no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)
```

— and a matching pile of `recurring_goal_reblock in simard::overseer`
stewardship issues, one per cycle.

As of the terminal-quarantine rung, such a goal is parked **once** and then left
alone. It shows on the board as `Blocked` and carries a durable
`ooda-breaker-quarantine` marker.

## Recognise a quarantined goal

```bash
simard goal list
```

A quarantined goal shows `status = Blocked` with a breaker-authored WHY-bearing
reason whose evidence is the re-investigation count itself (never `(none)`):

```text
🔒 [OODA-SAFEGUARD] OODA goal made no shippable progress for 3 consecutive
no-action cycles; why=UNCLEAR-CRITERIA evidence=[re-investigation <goal> (3 consecutive evidence-less investigations)]
```

Inspect its `wip_refs` to confirm the quarantine marker
(`kind = ooda-breaker-quarantine`):

```bash
simard goal show <goal-id>   # look for a wip_ref with kind "ooda-breaker-quarantine"
```

A quarantined goal is **not** re-selected by the re-investigation pass
(`reinvestigate_bare_blocked_goals` skips it), so it stops generating new
`ooda-stuck` and `recurring_goal_reblock` issues immediately.

## What quarantine does and does not mean

- **Terminal for the daemon.** The goal is removed from re-scheduling; the loop
  spends no more cycles on it. This is the change that stops the churn.
- **Reversible for a human.** Quarantine is a park, not a delete. No work is
  lost; the goal remains on the board, visible and recoverable.
- **Reached only at the bottom of a bounded ladder.** A goal is quarantined only
  after it (1) classified `UNCLEAR-CRITERIA`, (2) got its one guided engineer,
  and (3) surfaced an evidence-less investigation gap
  `SURFACED_INVESTIGATION_FAILURE_LIMIT` (3) times. Clear-criteria goals and
  unclear goals still inside the ladder are never quarantined.

## Recover a quarantined goal

### Option A — give it a machine-checkable finish condition (preferred, durable)

A goal is quarantined because the done-gate cannot tell when it is finished. The
durable fix is to replace it with a goal whose completion the daemon can
**observe**: a specific issue that must be `CLOSED`, a specific PR that must be
`MERGED`, or a file/command whose output the done-gate can check.

```bash
# Remove the unclear goal (its work is not lost — you re-express it below).
simard goal remove <goal-id>

# Add a concrete, completable replacement at a chosen priority (1-7).
simard goal add <priority> "module X line coverage >= 80%, PR merged"
```

### Option B — un-block for a fresh bounded window

If you believe the goal was quarantined prematurely (e.g. a transient
misclassification), un-block it. This clears the quarantine marker and **resets**
the surfaced-failure counter, so the goal earns a fresh guided-retry window
rather than re-quarantining immediately:

```bash
# Unconditional single-goal override: clears Blocked + the quarantine marker.
simard goal unblock <goal-id>
```

> `simard goal unblock-all` is scoped to the brain-failure safeguard marker and
> deliberately does **not** mass-clear quarantines — quarantine is a considered
> terminal state, so clearing it is an explicit, per-goal decision via
> `simard goal unblock <goal-id>`.

After un-blocking, restart the daemon (or wait a cycle) and confirm the goal is
re-investigated:

```bash
systemctl --user restart simard-ooda.service
simard goal list
```

If the underlying criteria are still unmeasurable, the goal will re-quarantine
after the same bounded ladder — that is the signal to use Option A instead.

## Verify the churn has stopped

- Duplicate `ooda-stuck` "goal stuck after guided retry (UNCLEAR-CRITERIA)"
  issues stop accumulating; the quarantined goal has exactly one open escalation.
- `recurring_goal_reblock in simard::overseer` stewardship issues collapse to a
  single open issue per root cause (see
  [signature stabilization](../concepts/ooda-breaker-churn-suppression.md#fix-2-stabilize-the-reblock-issue-signature)).
  Close the leftover duplicates by hand once you confirm no new ones appear.

```bash
# Confirm no NEW duplicates are being filed (count should be stable across cycles).
gh issue list --repo rysweet/Simard --search "goal stuck after guided retry (UNCLEAR-CRITERIA)" --state open | wc -l
gh issue list --repo rysweet/Simard --search "recurring_goal_reblock in simard::overseer" --state open | wc -l
```

## Related

- [The OODA breaker quarantines terminal UNCLEAR-CRITERIA goals](../concepts/ooda-breaker-churn-suppression.md) — the design and rationale.
- [Churn-suppression API reference](../reference/ooda-breaker-churn-suppression-api.md) — the exact variant, marker, and re-schedule filter.
- [Diagnose a no-progress breaker issue storm](./diagnose-a-no-progress-breaker-issue-storm.md) — the sibling per-goal filing cap.
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — the general block-clearing runbook.
- [Simard CLI reference: `simard goal`](../reference/simard-cli.md)
