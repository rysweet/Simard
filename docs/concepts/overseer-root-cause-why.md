---
title: "Overseer root-cause (\"WHY\") principle — ask why, don't just patch the symptom"
description: >
  How the acting Overseer, on EVERY detected Problem, first determines WHY the problem
  occurred — a structured root-cause analysis synthesized from the evidence signals, the
  observed telemetry, and cognitive-memory recall of prior same-signature occurrences —
  before (or as part of) choosing an action. The chosen action targets the root cause when
  possible; a symptom-only mitigation is explicitly labelled as such, with the root cause
  recorded as unaddressed and surfaced in the activity feed and operator notifications —
  never silently patched. Covers the antipattern it eliminates (blindly re-unblocking a
  perpetual goal every cycle instead of asking why it keeps getting blocked), the always-on
  mandatory contract, the deterministic multi-candidate analyzer (G3), the memory
  recall+store loop (G2, amplihack-memory-lib), the deduped root-cause escalation, and the
  additive OODA-loop integration.
last_updated: 2026-07-06
review_schedule: as-needed
owner: simard
doc_type: concept
status: design — not yet implemented
related:
  - ../design/overseer.md
  - ../reference/overseer-root-cause-why-api.md
  - ../reference/overseer-recurrence-dead-band-escalation-api.md
  - ../howto/configure-overseer-root-cause-why.md
  - ../howto/configure-overseer-recurrence-escalation.md
  - ./overseer-goal-board-health.md
  - ./perpetual-goal-no-progress-exemption.md
  - ../reference/overseer-goal-board-health-api.md
  - ../reference/overseer-activity-feed.md
  - ../reference/cognitive-memory-ranked-episodic-recall.md
  - ../reference/no-progress-breaker-api.md
---

# Overseer root-cause ("WHY") principle

> **Status: design — not yet implemented** (issue
> [#2635](https://github.com/rysweet/Simard/issues/2635)). This document is the
> **binding design specification** for the root-cause principle — it describes the
> feature we are about to build, not code that already ships. The `RootCause` /
> `CauseCandidate` model in `signal.rs`, the pure `root_cause::analyze` analyzer,
> the `Remediation` classification in `intervention.rs`, the `Overseer::with_memory`
> recall+store seam, the `problem_entries` activity-feed rendering, and the
> `overseer::root_cause` traces are the **contract the implementing PR will add** to
> `src/overseer/`; the implementation and this documentation land in the **same
> pull request**. Until then, the signatures here are the specification. See the
> [root-cause API reference](../reference/overseer-root-cause-why-api.md) for the
> exact symbols and the
> [configure-and-observe how-to](../howto/configure-overseer-root-cause-why.md)
> for the operator surface.

The acting [Overseer](../design/overseer.md) runs a meta-OODA loop:
**Observe → Orient → Decide → Act**. Orient folds the raw
[`Signal`](../reference/overseer-goal-board-health-api.md)s from one Observe pass
into ranked, deduplicated [`Problem`](../reference/overseer-root-cause-why-api.md#problemwhy)s;
Decide picks an [`Intervention`](../reference/overseer-goal-board-health-api.md);
Act executes the admitted ones. Without this principle, Decide would jump straight
from a `Problem` to an `Intervention` — fixing **what** it saw without ever
recording **why** it happened.

The **root-cause principle** makes one thing mandatory and always-on:

> Whenever the Overseer detects a Problem it MUST first determine **WHY** the
> problem occurred — a structured root-cause analysis — before (or as part of)
> acting. The chosen action MUST target the root cause when possible. If only a
> symptom-level mitigation is available, the Overseer MUST explicitly label it as
> a symptom-mitigation and record that the root cause remains **unaddressed**,
> and surface that — never silently patch it.

This is the operator's principle verbatim: *"when there is a problem it should
always ask WHY the problem occurred, not just try to fix it."*

## The antipattern it eliminates

The canonical failure this closes is **symptom-patching a perpetual goal**. The
continuous self-research goal is a standing/perpetual goal (see
[perpetual-goal no-progress exemption](./perpetual-goal-no-progress-exemption.md)).
When it is bursty, the OODA no-progress safeguard can false-park it "needs human
review". [Overseer goal-board health](./overseer-goal-board-health.md) taught the
Overseer to self-heal that false park with the exact `simard goal unblock`
operation.

But an `UnblockGoal` on its own is a **symptom fix**. If the Overseer just
re-unblocks the same perpetual goal every cycle, it never asks the real question:
*why does this goal keep getting blocked?* The three plausible root causes are all
distinct and demand different remedies:

- **(a) parked by the no-progress safeguard** (a false park) — the goal is fine,
  the safeguard fired too early → correct the safeguard state / re-tag;
- **(b) not tagged perpetual** — the goal is missing its standing tag, so the
  safeguard treats it as a normal goal → apply the perpetual tag;
- **(c) starved by higher-priority work** — the goal never gets scheduled → the
  root cause is scheduling starvation, which the Overseer escalates.

Blindly unblocking treats (a), (b) and (c) identically and forever. The
root-cause principle forces the Overseer to name the WHY, fix the actual cause
when it can, and — when the same signature keeps recurring and the Overseer
cannot fix it in-loop — **escalate the root cause** (a deduped issue describing
the cause, not the symptom) instead of re-unblocking on a loop.

## What "determining WHY" means

For each `Problem`, Orient now attaches a structured
[`RootCause`](../reference/overseer-root-cause-why-api.md#rootcause):

- a **ranked list of `CauseCandidate`s** — each a stable, human-readable cause
  `label`, a `likelihood`, and the `evidence` (signal / telemetry / recall
  references) it rests on;
- a **`primary_rationale`** — the single human-readable one-line WHY;
- a **`confidence`** (`Low | Medium | High`);
- a **`source`** (`Telemetry | MemoryRecall | Both`) — whether the WHY came from
  this pass's telemetry, from recalled prior occurrences, or both;
- a **`recurrence`** count — how many prior same-signature occurrences the
  cognitive memory recalled (`0` when none or when memory is unavailable).

`RootCause` renders (via `Display`) to one canonical line, e.g.:

```
perpetual goal parked by no-progress safeguard (false park) (confidence: High, source: Both, seen 4× before)
```

That single string is what appears in the traces, the activity feed, and the
operator notification, so the WHY the Overseer reasoned about is exactly the WHY
a human reads.

### Structured, not a single brittle heuristic (G3)

The WHY is produced by a **pure, deterministic, multi-candidate analyzer**
(`root_cause::analyze`). It weighs several evidence-linked candidates per
`ProblemKind` — for a distill-failure spike, for example, it ranks
`schema/format drift`, `model regression`, and `upstream payload change` by the
magnitude of `distill_fail_pct` rather than hard-coding one answer. This
satisfies guideline **G3** (prefer structured reasoning over a brittle
single-heuristic patch) while staying **hermetic**: there is **no in-loop LLM
call**, so the analysis is fully unit-testable with a hand-built `ObservedState`.

### Memory recall makes the WHY accumulate (G2)

The analyzer is enriched by the Overseer's cognitive memory
(amplihack-memory-lib, **G2**). On each problem the Overseer:

1. **Recalls** prior same-signature occurrences (read-only, non-reinforcing
   `search_facts` / `recall_facts_ranked` keyed on the `Problem.dedup_key`) to
   see what caused this problem before and how it turned out — this promotes
   matching candidates and sets the `recurrence` count and `source`.
2. **Stores** this occurrence's `{signature, primary cause, action, outcome}`
   after acting (best-effort `store_fact`), so the next occurrence's recall is
   richer.

Recall in the "reports what WOULD be done" `run_cycle` phase is strictly
read-only; all writes are deferred to the act/tick phase, preserving
`run_cycle`'s no-mutation contract.

When cognitive memory is **unavailable** — tests without a memory handle, or a
recall/store error — the analyzer degrades gracefully to telemetry-only
reasoning (`source = Telemetry`, `recurrence = 0`). That degrade is **logged via
`tracing`, never silent** (guideline: no silent fallbacks).

## Root cause vs. symptom: always labelled, never silent

Decide classifies every planned action with a
[`Remediation`](../reference/overseer-root-cause-why-api.md#remediation-classification-interventionrs):

| Situation | `class` | `root_cause_addressed` | `unaddressed_note` |
|---|---|---|---|
| Action fixes the primary cause (e.g. a first-time false-park → `UnblockGoal`; a systemic cause → `FileIssue`/`EscalateBlockedGoal`) | `RootCause` | `true` | `None` |
| No fix is needed because the "cause" is a **deliberate** operator or dependency block, correctly respected by a no-op `Report` | `Acknowledged` | `true` | `None` |
| Action only mitigates the symptom and leaves the primary cause live (e.g. a *repeated* `UnblockGoal` on a recurring re-block, a `Report` on genuinely-degraded process-health) | `SymptomMitigation` | `false` | `Some("<primary cause label>")` |

A `SymptomMitigation` always carries the surfaced note naming the still-live root
cause, so a symptom patch can never hide. This is the mechanical enforcement of
"never silently patch".

`Acknowledged` is the neutral, *non-alarming* outcome reserved for a **deliberate**
block: an operator or a real dependency has intentionally blocked the goal, so the
correct action is a respectful no-op and there is nothing to escalate. It counts as
`root_cause_addressed` and is **never** counted as a symptom-mitigation, so an
intentional block does not inflate the `symptom_mitigations` counter and does not
raise the "root cause unaddressed" feed alarm. This is what keeps the "never
silently patch" guarantee from **crying wolf** over blocks that are working as
intended.

### Blocked-perpetual-goal routing (the operator's exact ask)

`decide_blocked_goal` now routes on the recalled **recurrence** instead of
re-unblocking every cycle:

| Condition | Action | Remediation |
|---|---|---|
| perpetual & no-progress marker & **recurrence < N** | `UnblockGoal { …, why }` — one-off false-park correction | `RootCause` |
| perpetual & no-progress marker & **recurrence ≥ N** (keeps getting re-parked) | `FileIssue` describing the **root cause**, deduped on the root-cause signature | `RootCause` (escalates the cause) |
| genuine `needs_review` block | `EscalateBlockedGoal { …, why }` (operator, both channels) | `RootCause` |
| plain operator/dependency block | `Report` | `Acknowledged` (respectful no-op; the deliberate block is the recorded, addressed cause — **not** flagged as a symptom) |

`N` is the recurrence threshold (default 3, aligned with the
[no-progress breaker](../reference/no-progress-breaker-api.md) semantics). The
recurring path is the operator's exact ask: **stop re-unblocking; ask why it
keeps getting blocked and file the root cause, deduped.**

## Surfacing the WHY

The WHY is surfaced everywhere the Overseer's activity is visible:

- **Activity feed** — each tick's
  [`OverseerActivityRecord`](../reference/overseer-activity-feed.md) carries a
  `problem_entries` vector; each entry renders **problem + WHY + action +
  whether it addressed the root cause, acknowledged a deliberate block, or only
  patched the symptom** (`[root-cause]` / `[acknowledged]` / `[symptom]`). When any
  genuine symptom-mitigation occurred, `humanize_tick` appends
  `"(N symptom-mitigation(s), root cause unaddressed)"`; `[acknowledged]` entries
  are never counted in that summary. See
  [how to watch what the Overseer is doing](../howto/watch-overseer-activity.md).
- **Per-tick counters** — the `OverseerTickReport` gains scalar counters
  (`root_cause_analyses`, `symptom_mitigations`, `root_causes_addressed`), each
  emitted as a `tracing` key.
- **Operator notifications** — `OperatorNotification::goal_blocked_with_why`
  includes the WHY line in the message body, so an escalation reaches a human
  *with* the diagnosed cause, not just the symptom.
- **Deduped escalation** — when a root cause is a recurring systemic defect the
  Overseer cannot fix in-loop, it notifies the operator with the **root cause**
  through the per-goal escalation gate. It does not create another GitHub issue.

## Always-on, mandatory — no opt-out

Root-cause analysis is **always-on**. There is no flag to disable it, because the
principle is mandatory ("ALWAYS ask WHY"). The only thing conditioned on
availability is the **memory-recall enrichment**, which degrades gracefully when
cognitive memory is absent (telemetry-only WHY, logged). The acting paths that
the WHY feeds (goal-board self-heal / escalation) are still governed by the
existing [`SIMARD_OVERSEER_GOAL_HEALTH`](./overseer-goal-board-health.md) gate;
the analysis itself is not gated.

## Invariants

- **Every** detected `Problem` carries a populated `why` by the time Decide runs
  — not just goal problems.
- The WHY is derived from real evidence (signals + telemetry + recall), never
  fabricated; an unknown cause is an honest `unknown-cause` candidate at
  `Confidence::Low`, `source = Telemetry`.
- A symptom-only action is **always** labelled `SymptomMitigation` with the live
  root cause surfaced — the Overseer cannot silently patch.
- A **deliberate** operator/dependency block is labelled `Acknowledged` (addressed),
  **never** `SymptomMitigation` — an intentional block is never counted as a symptom
  mitigation and never raises a false "root cause unaddressed" alarm.
- `run_cycle` stays free of Simard mutations (recall is read-only; stores are
  deferred to act/tick).
- The analysis is deterministic and hermetic (no in-loop LLM), so behavior is
  reproducible in tests.
- Everything is **additive** — no existing Overseer type, field, or test is
  renamed or removed; new serialized fields default cleanly.

## See also

- [Overseer root-cause API reference](../reference/overseer-root-cause-why-api.md)
  — the exact types, fields, and functions.
- [Configure and observe the Overseer root-cause principle](../howto/configure-overseer-root-cause-why.md)
  — the operator surface and end-to-end verification.
- [Overseer goal-board health](./overseer-goal-board-health.md) — the self-heal /
  escalate actions the WHY now routes.
- [Standing/perpetual goals are exempt from the no-progress hard-block](./perpetual-goal-no-progress-exemption.md)
  — why a perpetual goal must never be parked, the root cause the Overseer names.
- [Ranked episodic recall & memory reinforcement](../reference/cognitive-memory-ranked-episodic-recall.md)
  — the amplihack-memory-lib recall the WHY reuses.
