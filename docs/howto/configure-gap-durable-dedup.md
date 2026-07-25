---
title: Configure and verify gap dedup
description: >
  How to operate and verify the Overseer's stable-signature gap dedup for
  workstream-gap notifications — confirm a recurring gap is deduped to one
  operator notification within a running daemon, read the
  overseer::gap_scan flagged/suppressed logs, understand that a daemon restart
  resets the in-process gate, and check the bounded GapCategory taxonomy that
  keeps gap signatures stable and dedupable.
last_updated: 2026-07-25
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-gap-durable-dedup.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-backoff-gate-api.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ./review-overseer-workstream-gaps.md
  - ./configure-overseer-gap-scan-backoff.md
  - ./file-stewardship-issues-from-orchestrator-runs.md
---

# Configure and verify gap dedup

The Overseer flags uncovered backlog work — uncovered goals, high-signal open
issues, and unaddressed telemetry anomalies — on its recurring gap-scan and
**notifies the operator** (email + Signal) about it. This rail makes the gap's
dedup signature a **stable, content-addressed** slug (instead of a per-run hash),
so a recurring gap is deduped to **one notification within a running daemon**
rather than re-notified every tick. This is the root-cause fix behind the
near-duplicate `[stewardship] workstream_gap:*` noise (observed on e.g. #4671,
#4680, #4685).

For the data model, signature grammar, and guarantees, see the
[gap-filing dedup reference](../reference/overseer-gap-durable-dedup.md).

> **Scope.** The gap-notification path dedupes via the **in-process**
> `WhisperGate` and notifies the operator; it does **not** create GitHub issues
> and is **not restart-safe on its own** — a daemon restart resets the gate. A
> durable, GitHub-sourced cross-process check is scoped as follow-on work and is
> **not** wired on this path yet; see the reference doc's *Future work* section.

## What changed for operators

- **Before:** the gap signature was derived per run
  (`originating-run: overseer-<hash>`), so every restart/re-run minted a fresh
  key and the in-process gate could not collapse a recurring gap → the scan
  re-notified a near-duplicate gap every tick.
- **Now:** the signature is a **stable, content-addressed** slug, so the
  in-process gate collapses a recurring gap to **one notification per dedup
  window** for the life of the daemon.
- **Restart behaviour:** the in-process gate is memory-resident, so a restart
  resets it and the first post-restart tick may re-notify. Cross-restart dedup
  awaits the durable check (future work).

There is **nothing to turn on** — stable-signature dedup is always active on the
gap path whenever the Overseer runs. The existing knobs still apply:

| Env var | What it does | Default |
|---|---|---|
| `SIMARD_OVERSEER_GAP_SCAN` | Falsey (`0`/`false`/`no`/`off`) turns the gap-scan off entirely | on |
| `SIMARD_OVERSEER_GAP_SCAN_EVERY_N` | Run the scan every *Nth* tick | `1` |

See [configure the gap-scan backoff](./configure-overseer-gap-scan-backoff.md)
for the in-process dedup window.

## Prerequisites

- The acting Overseer is enabled (`SIMARD_OVERSEER_ENABLED` unset or truthy).
- An operator notifier is configured (email and/or Signal) so gap notifications
  have somewhere to go.

## Verify: a recurring gap is deduped within a run

This is the core acceptance check for the workstream-gap dedup rail.

1. With a standing, uncovered gap present, let the Overseer run one gap-scan tick
   and confirm it notifies **once** (a `flagged>=1` info line):

   ```text
   INFO overseer::gap_scan flagged=1 suppressed=0
        overseer recorded uncovered backlog work and notified the operator
   ```

2. On the **next** tick within the dedup window, confirm the **same** gap is
   suppressed rather than re-notified — the count moves to `suppressed`:

   ```text
   DEBUG overseer::gap_scan flagged=0 suppressed=1
         overseer gap-scan: every observed gap is within the dedup window (suppressed)
   ```

3. `flagged=0 suppressed=1` for the recurring gap is the proof the stable
   signature deduped it. (A daemon restart between steps 1 and 2 resets the gate
   and may re-notify — that is expected until the durable check lands.)

## Read the logs

The gap path emits structured `tracing` + OTel only (no `print!`/`println!`), on
`target: "overseer::gap_scan"`:

| Field | Meaning |
|---|---|
| `flagged` | Fresh gaps notified this tick |
| `suppressed` | Gaps dropped by the in-process gate or by a malformed signature |
| `dispatched` / `all_sent` | Operator-notification delivery status |

## Malformed signatures are dropped (injection defense)

A gap whose signature is not a valid restricted slug
(`^[a-z0-9][a-z0-9:_#.\-/]{0,200}$`) is **dropped at the filing seam** and counted
as suppressed — it never reaches an operator notification:

```text
WARN overseer::gap_scan category="goal"
     overseer gap-scan: dropping a gap with a malformed dedup signature
     (outside the bounded taxonomy)
```

This is deliberate: signatures come from trusted identifiers only, so a malformed
one signals a bug or an injection attempt, not a real gap.

## The bounded taxonomy (why signatures are now stable)

Duplicates used to slip through because free-form titles drifted between ticks.
Each gap resolves to a bounded `GapCategory` variant with a stable slug that
anchors the signature:

| Gap kind | `GapCategory` | Signature prefix |
|---|---|---|
| Uncovered p1/p2 goal | `GoalUncovered` | `goal:<goal_id>` |
| High-signal open issue | `IssueUncovered` | `issue:<repo>#<n>` |
| Unaddressed anomaly | `AnomalyUnaddressed` | `anomaly:<slug>` |

`GapCategory` is a closed enum of exactly these three kinds, so a gap's signature
is stable across ticks. The fix did not add kinds — it made the signature a
stable, content-addressed slug (instead of a per-run hash) so the in-process gate
recognises the same gap across ticks. This change is additive.

## Common pitfalls

- **A duplicate notification appeared right after a restart.** Expected — the
  in-process gate is reset on restart. Cross-restart dedup is future work.
- **Two notifications for the "same" gap.** Confirm the gaps carry the **same**
  signature. If the signatures differ, the gap resolved to two distinct keys
  (e.g. two different goal ids) — correct behaviour, not a dedup miss.
- **A gap I expected was never notified.** Check for a `dropping a gap with a
  malformed dedup signature` WARN — a signature outside the bounded taxonomy is
  dropped by design.

## See also

- [Gap-filing dedup reference](../reference/overseer-gap-durable-dedup.md)
  — signature grammar, the in-process flow, and the scoped durable follow-on.
- [Review the Overseer's workstream gaps](./review-overseer-workstream-gaps.md)
  — where the gaps surface and how to respond.
- [Gap-scan dedup & exponential backoff](../concepts/gap-scan-backoff-dedup.md)
  — the in-process gate this stable signature feeds.
- [File stewardship issues from orchestrator runs](./file-stewardship-issues-from-orchestrator-runs.md)
  — the sibling loop whose durable dedup flow the future gap check would mirror.
