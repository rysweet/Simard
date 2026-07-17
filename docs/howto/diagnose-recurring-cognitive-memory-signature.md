---
title: Diagnose a recurring "seen 2× in cognitive memory" signature
description: >
  How to read, verify, and resolve the Overseer's `recurring signature seen 2× in
  cognitive memory (overseer-obs:…)` signal — why the `overseer-obs:` prefix means
  a self-observation loop, how the D1/D2/D3 fixes (#4128) stop the Overseer
  re-observing its own emissions and re-parking already-done goals, and how to
  confirm bare-blocked goals are re-investigated against live GitHub state.
last_updated: 2026-07-16
review_schedule: as-needed
owner: simard
doc_type: howto
related:
  - ../reference/overseer-self-observation-stability.md
  - ../reference/overseer-memory-recall-api.md
  - ../reference/overseer-workstream-gap-scan.md
  - ../reference/overseer-backoff-gate-api.md
  - ../reference/no-progress-reinvestigation-api.md
  - ./reinvestigate-bare-blocked-goals.md
  - ./review-overseer-workstream-gaps.md
  - ./configure-overseer-gap-scan-backoff.md
  - ../concepts/gap-scan-backoff-dedup.md
  - ../design/overseer.md
---

# Diagnose a recurring "seen 2× in cognitive memory" signature

If the Overseer activity feed shows a signal like:

```
recurring signature seen 2× in cognitive memory
(overseer-obs:goal:blocked:fix-agent-kgpacks-rs-issue-17-ws2-int8-pq-embed-7f5afcca|…)
```

this guide explains what it means, why it is (or is not) a real problem, and how
the stability fixes shipped in issue #4128 resolve it. For the full contract, see
the [self-observation stability reference](../reference/overseer-self-observation-stability.md).

## Read the signature first

The `RecurringSignature` payload is a `|`-joined list of problem dedup keys. The
**prefix** tells you the origin:

- **`overseer-obs:` prefix** → the recalled episode is one the Overseer *itself*
  wrote back. Before #4128 this meant the Overseer was **observing its own
  observation** — a self-referential loop. After #4128, D1's emission-hygiene
  filter drops these at the write boundary, so a fresh `overseer-obs:*`
  recurrence should **no longer** appear. If you still see one, the daemon is
  running a pre-#4128 binary — redeploy.
- **`goal:blocked:…` keys** (inside the join) → the signature is composed of
  goals parked in a bare `[OODA-SAFEGUARD] … needs human review` block. This is
  the fingerprint of the second defect: done work re-parked as stuck.

## Why "2×" is expected, not a counter bug

The recurrence count comes from recall of prior episodes. The dedup
[`WhisperGate`](../reference/overseer-memory-recall-api.md) is **per-process**, so
a daemon restart honestly re-emits a still-unresolved observation exactly once —
giving `2×` against a static, unresolved set. This is an honest re-observation,
**not** a storage or dedup bug. The fix removes the *cause* of the recurrence, not
the honest count.

## Verify the fix is doing its job

### 1. The self-observation loop is broken (D1)

Confirm no `RecurringSignature` carries an `overseer-obs:` **origin**. The
Overseer no longer feeds its own write-back episodes into its recall pass. Genuine
engineer/OODA failure signatures still recur and escalate normally.

### 2. Bare-blocked goals are re-investigated against live GitHub (D2)

For each goal in the signature (e.g. `fix-agent-kgpacks-rs-issue-17-…`), check the
live GitHub state:

- If the **issue is CLOSED** and the **PR is MERGED**, the goal's work is *done*.
  With D2a, the WHY-gate now re-queries GitHub through a **fresh API call** for any
  bare-blocked goal — independent of whether this cycle produced completion
  evidence — and transitions the goal **out** of `Blocked` (auto-complete /
  archive) instead of re-parking it.
- Re-investigation is **evidence-driven**: it never auto-unblocks on recall alone.
  Only fresh CLOSED/MERGED evidence clears a block, and each goal is
  re-investigated at most once per parked state (see the
  [no-progress re-investigation how-to](./reinvestigate-bare-blocked-goals.md)).

The recurrence count stays **stable** across cycles because D2b upserts a
saturating count into the occurrence fact rather than growing `recall.len()` every
tick — so the escalation reflects distinct occurrences, not how often the Overseer
looked.

### 3. Workstream gaps actually close (D3)

If the signature includes `workstream-gap` keys, confirm each distinct gap is now
keyed per-signature (`workstream-gap:<sig>`) and receives **one** idempotent
covering launch or filed issue through the shared `gate()` — no notify-only
dead-end, and no launch/issue spam. See
[review the Overseer's workstream gaps](./review-overseer-workstream-gaps.md).

### 4. Duplicate gap-cover work is backed off, not re-fired (#4186)

Per-signature keying (D3) stops *distinct* gaps from sharing a slot, but it does
not by itself stop the **same** still-open gap from being acted on again next
tick. The gap-scan act path now runs an in-process exponential-backoff
duplicate-suppression gate
([BackoffGate reference](../reference/overseer-backoff-gate-api.md)):

- **Exponential `BackoffGate`.** The first occurrence of a gap signature is
  surfaced immediately; each later occurrence within the current window is
  suppressed, and the window **doubles** each admit (900s → 1800s → 1h → …
  capped at 24h), resetting after a long silence. So a persistent gap is covered
  **once** and then rate-limited, not re-filed every tick — this is what ends the
  seven duplicate *"Cover uncovered backlog workstream(s)"* issues
  ([#4190](https://github.com/rysweet/Simard/issues/4190) …
  [#4206](https://github.com/rysweet/Simard/issues/4206)). A cross-process
  open-issue equivalence check (for the cold-start/restart case) is planned as
  future work, not part of this change.

The dedup **key is signature-stable**, so *"seen 3×"* and *"seen 4×"* collapse to
one key: the honest recurrence count (above) is preserved, only the *duplicate
action* is suppressed. Confirm the healthy steady state via the tick's held-plan
reason — a covered gap is held with *"an equivalent coverage was launched
recently (backoff window)"*, **not** a new issue. See
[configure gap-scan backoff](./configure-overseer-gap-scan-backoff.md) to tune
the window.

## If a bare-blocked goal is genuinely done but still parked

1. Confirm the daemon is on a post-#4128 binary.
2. Confirm `SIMARD_OVERSEER_GOAL_HEALTH` is enabled (it feeds bare-blocked goals
   into re-investigation) and the no-progress re-investigation rail is on.
3. Confirm GitHub actually reports the issue CLOSED / PR MERGED — D2 requires
   **fresh API evidence**, not recall.

## What you should *not* do

- **Do not** "fix" the 2× by widening the dedup window — that suppresses an honest
  signal instead of removing its cause. (This is distinct from the #4186 gap-cover
  `BackoffGate`, which suppresses duplicate *actions* on an already-covered gap
  while leaving the honest recurrence count intact — see
  [gap-scan dedup & backoff](../concepts/gap-scan-backoff-dedup.md).)
- **Do not** re-implement the kgpacks-rs int8-PQ-embed work to "unblock" #17 — the
  work is already merged; the defect is the *safeguard that mis-read done as
  stuck*, which #4128 fixes.
- **Do not** open Gate A re-investigation without the paired count-in-content
  upsert — that latches escalation forever (see the
  [reference](../reference/overseer-self-observation-stability.md#safety-invariants)).
