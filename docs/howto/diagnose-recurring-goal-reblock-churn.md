---
title: Diagnose recurring goal-reblock churn
description: >
  Operator runbook for the goal-reblock backoff & stewardship-dedup rail
  (#4817 / #4828): how to recognise the "GoalHygiene ... blocked (N no-action
  cycle(s))" relaunch storm and the duplicate stewardship issues, confirm the
  rail is suppressing relaunches and folding issues into one, read the state,
  tune the shared backoff window, and clear a genuinely stuck block.
last_updated: 2026-07-27
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../concepts/goal-reblock-backoff-dedup.md
  - ../reference/goal-reblock-backoff-api.md
  - ../reference/overseer-backoff-gate-api.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ./unblock-stuck-ooda-goals.md
  - ./configure-overseer-gap-scan-backoff.md
---

# Diagnose recurring goal-reblock churn

The Overseer suppresses per-cycle **relaunch** of an already-blocked goal and
files **one** stewardship issue per block (see
[goal-reblock backoff & stewardship dedup](../concepts/goal-reblock-backoff-dedup.md)).
This runbook helps you confirm the rail is doing its job and act when a block is
genuinely stuck.

## Recognise the symptom

Before this rail (#4817 / #4828) the daemon log showed the **same** blocked goal
relaunched every ~15 minutes for hours:

```
GoalHygiene goal <id> blocked (0 no-action cycle(s))
... held: per-cycle launch cap reached
GoalHygiene goal <id> blocked (0 no-action cycle(s))   # next cycle, identical
... held: per-cycle launch cap reached
```

…together with a stream of near-identical `recurring_goal_reblock` stewardship
issues (the pattern that produced #4817 and #4828).

With the rail in place you should instead see the relaunch **suppressed** after
the first observation, and a **single** open stewardship issue for the goal.

## Confirm the rail is working

1. **One issue, not many.** Check for duplicate stewardship issues for the same
   goal:

   ```bash
   gh issue list --repo rysweet/Simard --state open \
     --search 'recurring_goal_reblock in:title,body' --limit 50
   ```

   You should find **one** open issue per blocked goal. Multiple open issues for
   the *same* goal means the signature is not stable — see
   [signature debugging](#signature-not-folding) below.

2. **Relaunch suppressed.** In the daemon log, after the first
   `GoalHygiene goal <id> blocked` you should see the relaunch held by the
   backoff (not by the launch cap) on subsequent cycles, e.g.
   `goal-reblock backoff: <id> suppressed (window <secs>s)`. The `held:
   per-cycle launch cap reached` line should no longer recur for that goal every
   cycle.

3. **Re-admit on clear.** When the underlying block clears, the goal should
   re-admit within one base window and the workstream relaunch/close normally.

## Read the backoff / dedup state

The suppression key is `overseer-obs:goal:blocked:{goal_id}` and it uses the
shared Overseer backoff window (see
[BackoffGate reference](../reference/overseer-backoff-gate-api.md#configuration)).
The stewardship signature is stable per goal — the
`consecutive_no_action` counter is kept in the issue body/title only, not in the
hashed signature.

## Tune the backoff window

The goal-reblock gate reuses the shared `SIMARD_OVERSEER_BACKOFF_*` window
configuration (same knobs as the gap-scan rail — see
[configure Overseer gap-scan backoff](./configure-overseer-gap-scan-backoff.md)):

| Env var | Default | Effect |
| ------- | ------- | ------ |
| `SIMARD_OVERSEER_BACKOFF_BASE_SECS` | (shared default) | base suppression window after the first observation |
| `SIMARD_OVERSEER_BACKOFF_MULTIPLIER` | (shared default) | growth per re-hit (`≥ 2`) |
| `SIMARD_OVERSEER_BACKOFF_MAX_SECS` | (shared default) | cap on the window |

Apply via a systemd drop-in and restart the daemon. A longer base window quiets
a persistently-blocked goal further between retries; a shorter one retries a
recoverable goal sooner.

## <a id="signature-not-folding"></a>Duplicate issues still appearing

If you see more than one open stewardship issue for the **same** goal:

1. Open two of the duplicates and compare their `stewardship-signature: <sig>`
   footer. If the signatures **differ**, some volatile text is still leaking into
   the signature input — most likely a counter or id the redaction does not yet
   cover. This is a `normalize_for_signature` gap
   (`src/stewardship/dedup.rs`), not an operator misconfiguration; file it with
   the two issue bodies attached.
2. If the signatures **match** but both issues are open, the dedup **read** may
   be stale (both were filed before the first became visible) — close the older
   duplicate; the rail will reuse the survivor going forward.

## Clear a genuinely stuck block

Suppressing the *relaunch churn* does not fix the *underlying block*. If the
single stewardship issue shows a goal that truly needs intervention, resolve the
block itself — see [unblock stuck OODA goals](./unblock-stuck-ooda-goals.md).
Once the block clears, the goal-reblock gate re-admits automatically and the
stewardship issue can be closed.

## See also

- [Goal-reblock backoff & stewardship dedup](../concepts/goal-reblock-backoff-dedup.md) — the rationale.
- [Goal-reblock backoff reference](../reference/goal-reblock-backoff-api.md) — the typed API.
- [Unblock stuck OODA goals](./unblock-stuck-ooda-goals.md) — resolving the underlying block.
