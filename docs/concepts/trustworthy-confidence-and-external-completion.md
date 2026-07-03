---
title: "Concept: trustworthy confidence + external-signal completion"
description: Why Simard's brain attaches a usable, calibrated confidence to its Decide and engineer-lifecycle judgments (verbalized confidence + self-consistency), and why goal completion is verified against external signals (merged PR, closed issue, CI, deploy) instead of subordinate self-reports. Together these make the escalation ladder (#2432) and consolidation gate (#2433) trust the right judgments and stop the daemon from archiving unproven work.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: concept
status: design — not yet implemented
related:
  - deploy-aware-done-gate.md
  - prompt-driven-ooda-brain.md
  - progress-evidence-gating.md
  - ../reference/trustworthy-confidence-api.md
  - ../reference/external-signal-completion-gate.md
  - ../reference/completion-evidence-gate-api.md
  - ../howto/interpret-brain-confidence-and-verify-completion.md
  - ../../src/ooda_reasoners/orient.rs
  - ../../src/ooda_reasoners/decide.rs
  - ../../src/ooda_reasoners/judgment_record.rs
  - ../../src/goal_curation/completion_gate.rs
---

# Concept: trustworthy confidence + external-signal completion

> **Status: design specification — not yet implemented (issues
> [#2457](https://github.com/rysweet/Simard/issues/2457) and
> [#2456](https://github.com/rysweet/Simard/issues/2456), both open).**
>
> This document describes the **intended** design and the *why* behind it.
> Nothing here ships yet: the verbalized-confidence fields on
> [`DecideJudgment`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/decide.rs)
> and
> [`EngineerLifecycleDecision`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/mod.rs),
> the `self_consistency` sampler, the `self_metrics::calibration` ECE spine, and
> the external-signal completion ladder in `completion_gate.rs` are all
> **planned, not live**. The precedent it builds on —
> [`OrientJudgment.confidence`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/orient.rs)
> and
> [`ReasonerJudgmentRecord.confidence`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/judgment_record.rs)
> — does exist today. See the
> [trustworthy-confidence API](../reference/trustworthy-confidence-api.md) and the
> [external-signal completion gate](../reference/external-signal-completion-gate.md)
> for the proposed typed surfaces.

> Simard's brain should attach a **usable confidence** to the judgments that
> matter, and refuse to call a goal **done** on a subordinate's word. Confidence
> is earned (verbalized *and* corroborated by self-consistency); completion is
> verified (against merged PRs, closed issues, green CI, and live deploys).

## The two problems this solves

These are two halves of one trust problem: *do not act on a signal more than it
deserves*.

### 1. Decisions had no usable confidence (#2457)

The Orient phase already carries a self-reported `confidence` (see
[`OrientJudgment`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/orient.rs)),
but the two **commitment** phases do not:

- **Decide** (`DecideJudgment`) — which action kind to run this cycle.
- **Engineer-lifecycle** (`EngineerLifecycleDecision`) — whether to reclaim a
  wedged worktree, open a tracking issue, mark a goal blocked, etc.

Downstream machinery that *should* spend more compute on shaky judgments and
trust solid ones has nothing to read:

- The [confidence-gated escalation ladder (#2432)](prompt-driven-ooda-brain.md)
  escalates only on a hard parse-miss — a binary proxy for "low confidence."
- The [consolidation / ISAO reliability gate (#2433)](../reference/cognitive-memory-provenance.md)
  needs a per-judgment confidence to set `CognitiveFact.confidence` honestly.

Without a real number, "I'm 0.55 sure" and "I'm 0.98 sure" are indistinguishable.

### 2. "Done" is a self-report (#2456)

The completion path trusts **artifact existence** — "a commit or a PR exists,
therefore the goal is complete." That is exactly the subordinate-self-report
trust the daemon should not extend: an open PR, a local commit, or an
engineer's "I finished" string is *weak* evidence. Goals can be archived as
`Completed` with red CI, an unmerged PR, or a still-open linked issue.

The [deploy-aware done-gate (#2450)](deploy-aware-done-gate.md) already raised
the bar for *archival* (merged PR + closed issue + verified deploy). #2456 would
extend that same philosophy **upstream** to the moment a goal is first marked
complete, so weak signals never reach the archive gate as `Completed` in the
first place.

## How trustworthy confidence works (#2457)

Two independent signals are combined, cheaply by default and expensively only
when it matters.

1. **Verbalized confidence** — the brain reports a `0.0–1.0` probability
   alongside its choice ("Just-Ask-for-Calibration"). It rides the existing
   wire format: a `CONFIDENCE:` line for the prose-parsed Decide (`DECISION:`
   marker) and engineer-lifecycle (first-word match) phases, and the
   `"confidence"` JSON key for Orient.
2. **Self-consistency** — for **high-stakes / irreversible** judgments with
   budget headroom, the brain is sampled `K` times (default 3) and
   `confidence = modal_count / K`. Agreement *is* the calibration signal; a
   3/3 sweep is trusted, a 1/3/1 split is not.

The result would be recorded on the judgment, surfaced on
[`ReasonerJudgmentRecord.confidence`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/judgment_record.rs)
in cycle reports, fed to the #2432 ladder and the #2433 gate, and scored for
**calibration** (Expected Calibration Error) against the only objective
outcome available: the #2456 verification result.

### Confidence unlocks privilege — so it fails *closed*

Confidence is consumed by gates that *grant* trust (more compute, fact
promotion, fewer re-checks). Therefore a confidence that was **solicited but
could not be trusted** — absent when requested, malformed, or out of `[0,1]` —
collapses to a **low-trust floor**, never to `1.0`. The cheerful
`default_confidence() = 1.0` is reserved for genuinely deterministic and legacy
paths (the deterministic floor brain, and old cycle reports that predate the
field). This is the single most important safety property of the primitive:
**you cannot earn trust by emitting garbage.**

## How external-signal completion works (#2456)

Completion would be decided by a **signal-strength ladder**, not by counting
artifacts:

| Strength | Signals | Effect |
| --- | --- | --- |
| **Strong** | merged PR · closed linked issue · green CI · build/test exit-success · satisfied goal-graph postcondition · deploy-verified ([#2450 gate](deploy-aware-done-gate.md)) | Can complete |
| **Weak** | open PR exists · local commit exists · subordinate "done" text | Never sufficient alone |

- Subordinate claims done **and ≥1 strong signal** ⇒ `Completed`.
- Subordinate claims done, a strong signal is **derivable but refuted** (CI red,
  exit ≠ 0, issue still open) ⇒ recorded `refuted`, routed to
  `OpenTrackingIssue`, **not** completed or archived.
- Subordinate claims done, **no strong signal derivable** ⇒ the honest
  `CompletedUnverified` outcome — held for review, **not archivable**.

> **Shipped vs. design.** Today the gate emits the `unverified_no_signal`
> *outcome* (via `goal_completion_verification`) and keeps such goals
> **blocked-and-retained** (held for review, never archived). The distinct
> persisted `GoalProgress::CompletedUnverified` *state* — which would ripple
> through ~60 exhaustive `match GoalProgress` sites — is kept as forward design;
> see the [gate reference](../reference/external-signal-completion-gate.md).

This would reuse the [`CompletionEvidenceGate`](../reference/completion-evidence-gate-api.md)
(it is the deploy-verified strong signal), rather than duplicating it.

## The shared spine: outcomes calibrate confidence

The two halves would close a loop. The #2456 verification verdict would be the
**ground truth** that scores #2457's confidence:

```
brain judgment ──confidence──▶ self_metrics::calibration (ECE)
       │                                   ▲
       ▼                                   │ realized outcome
   action runs ─▶ external signals ─▶ completion verdict (verified / refuted)
```

- `verified` → realized outcome `1`
- `refuted` → realized outcome `0`
- `unverified_no_signal` / `error` → excluded from calibration (no ground truth)

Expected Calibration Error (`brain_confidence_ece`) over a rolling window tells
operators whether the brain's stated confidence *means* anything. A
well-calibrated brain that says "0.7" is right ~70% of the time.

## What this is **not**

- **Confidence is advisory, never a hard gate on irreversible action.** A high
  confidence number cannot *by itself* complete a goal, archive it, or authorize
  a destructive lifecycle action — those require external evidence (above). This
  prevents a confidently-wrong brain from talking itself into harm.
- **It does not reimplement #2432 or #2433.** Those ladders/gates already exist
  and are merged; #2457 would only *produce and expose* the confidence they
  consume.
- **It does not replace the [deploy-aware done-gate](deploy-aware-done-gate.md).**
  #2456 would extend the same evidence philosophy to the earlier "mark complete"
  decision and *consume* the deploy gate's verdict as one strong signal.

## See also

- [Trustworthy-confidence API reference](../reference/trustworthy-confidence-api.md)
- [External-signal completion gate reference](../reference/external-signal-completion-gate.md)
- [How-to: interpret brain confidence and verify completion](../howto/interpret-brain-confidence-and-verify-completion.md)
- [Concept: deploy-aware done-gate](deploy-aware-done-gate.md)
- [Concept: prompt-driven OODA brain](prompt-driven-ooda-brain.md)
