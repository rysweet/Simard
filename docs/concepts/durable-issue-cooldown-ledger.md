---
title: The durable issue-cooldown ledger stops the OODA-core auto-issue storm
description: >
  Why the OODA-core self-monitoring flooded its own issue tracker with ~20
  duplicate auto-issues in 24h — 9 identical `ooda-stuck` UNCLEAR-CRITERIA issues
  for one goal, 5 `recurring_goal_reblock` stewardship issues, and 6
  `workstream_gap:issue` stewardship issues — and the additive fix: a durable,
  restart-surviving `IssueCooldownLedger` keyed by `(finding_kind, subject)` that
  the auto-issue filers consult before opening an issue (comment-and-throttle on
  an existing open match, never re-file). All changes are additive; clear,
  non-recurring findings behave identically.
last_updated: 2026-07-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: implemented
related:
  - ./gap-scan-backoff-dedup.md
  - ../reference/issue-cooldown-ledger-api.md
  - ../reference/whisper-gate-backoff-api.md
  - ../reference/overseer-gap-scan-durable-dedup.md
---

# The durable issue-cooldown ledger stops the OODA-core auto-issue storm

> **Status.** The durable `IssueCooldownLedger`
> (`src/overseer/issue_cooldown.rs`), its config knobs
> (`src/overseer/config.rs`), and the reused `WhisperGate` window math
> (`src/overseer/guardrails.rs`) are **implemented and unit-tested**. Routing the
> three OODA-core filers through the ledger and the defensive `merge_boards`
> `wip_refs` union are the additive follow-up integration (issue #4930). For the
> exact types and functions see the
> [issue-cooldown ledger API reference](../reference/issue-cooldown-ledger-api.md).

## The defect

Over roughly one day the self-monitoring daemon auto-filed **~20 duplicate**
tracking issues, in three clusters that all trace to the same root cause — a
finding re-filed **every OODA cycle** instead of once:

- **9 identical `ooda-stuck` UNCLEAR-CRITERIA issues** for the *single* goal
  `move-the-governed-repo-roster-out-of-framework-a8f57a50`
  (#4930, #4927, #4922, #4915, #4912, #4905, #4897, #4891, #4890) fired by the
  no-progress breaker.
- **5 `recurring_goal_reblock` stewardship issues**
  (#4925, #4919, #4908, #4902, #4893).
- **6 `workstream_gap:issue` stewardship issues**
  (#4921, #4920, #4910, #4909, #4894, #4888).

That volume is not twenty independent findings — it is **three** findings each
re-filed once per cycle. This is the same self-amplifying shape the
[gap-scan backoff](./gap-scan-backoff-dedup.md) already addresses: a safeguard
that *observes a condition forever* instead of *acting on it once*.

### Why the existing dedup did not stop it

Each path already had a dedup mechanism — the storm is a **regression / wiring
gap**, not a missing feature. Two coupled faults let duplicates through anyway:

1. **In-memory-only backoff resets on exec-reload.** The `WhisperGate` and the
   in-process strike maps live in the `Overseer` / `NoProgressTracker` structs.
   The daemon periodically **exec-reloads** to pick up new binaries; every reload
   constructs fresh gates with empty state, so the *first* cycle after a reload
   re-fires every still-open finding.
2. **Whole-goal last-writer-wins drops the durable marker.** The OODA loop and
   the Overseer both write the goal board. `merge_boards` reconciled a persisted
   snapshot with an in-flight one by taking the **whole goal** from the last
   writer (LWW), so a suppression-marker `WipRef` written by the other client
   could be clobbered — the next cycle then saw an "untracked" goal and re-filed.

The net effect: the durable markers were real but not *durable enough* across
(a) process reloads and (b) cross-client merge races, and the in-memory backoff
could not bridge the gap because it reset on every reload.

## The fix: one durable cooldown ledger

The additive fix is a single durable dedup layer that lives **outside** any
in-process struct and **outside** the goal-board snapshot: the
[`IssueCooldownLedger`](../reference/issue-cooldown-ledger-api.md). It is keyed
by `(finding_kind, subject)` and backed by a standalone cognitive-memory fact
namespace (`overseer:issue-cooldown`), so it is:

- **reconstructed from durable memory on every exec-reload / restart** — closing
  fault (1); a fresh ledger over the same memory still throttles an in-window
  key; and
- **immune to the goal-board `merge_boards` LWW** — closing fault (2), because
  the ledger fact is not part of the merged snapshot at all.

Before opening an issue a filer consults `allow_emit(key, now)`:

- **`Emit`** — no prior durable emit, or the backoff window has elapsed → file
  once, then `record_emit` advances the durable timestamp and strike count.
- **`Throttle`** — inside the window → do **not** file; `note_still_observed`
  adds a short "still observed" annotation to the ONE canonical tracking issue.

The window reuses the existing `WhisperGate` exponential backoff verbatim
(`6h → 12h → 24h`, capped), with the base **floored at one full OODA cycle** so
the same `(goal, finding)` can never re-file every cycle — the storm's defining
symptom. A still-open finding is never *permanently* silenced: after the cap it
re-surfaces at least daily.

Distinct findings keep independent keys, so rate-limiting one goal's `ooda-stuck`
finding never silences a *different* goal or a *different* finding kind — a
genuinely new problem still surfaces promptly.

### Why this is safe

- **Additive.** Absent the ledger every filer keeps its prior per-path dedup;
  nothing breaks. The ledger is opt-out via `SIMARD_OVERSEER_ISSUE_COOLDOWN`.
- **Fail-open.** A memory-read error yields `Emit`, so a storage hiccup can never
  permanently suppress a genuinely new finding.
- **No sensitive data.** The durable fact holds only
  `{ last_emit_secs, strikes, issue_number }` — never an issue body or token.
- **Injection-safe keys.** The untrusted subject is reduced to `[a-z0-9_]`, so
  goal/gap text can never inject a `gh --search` qualifier.

## Defense-in-depth: `merge_boards` field-level `wip_refs` union (follow-up)

As a secondary guard for the existing per-goal durable markers, `merge_boards`
is intended to **union** the two `wip_refs` lists for a goal present in both
boards (de-duplicated by `(kind, ref_id)`) instead of dropping one side under
whole-goal LWW. Only `wip_refs` becomes a field-level union; every other goal
field keeps its prior merge, so the change stays narrowly scoped. This is tracked
as an additive follow-up under issue #4930.

## See also

- [Issue-cooldown ledger API](../reference/issue-cooldown-ledger-api.md) — the types and methods.
- [Gap-Scan Dedup & Backoff](./gap-scan-backoff-dedup.md) — the sibling in-process backoff this makes durable.
- [WhisperGate Exponential-Backoff API](../reference/whisper-gate-backoff-api.md) — the window math reused here.
