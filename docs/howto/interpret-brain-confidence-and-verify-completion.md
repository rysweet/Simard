---
title: How to interpret brain confidence and verify goal completion
description: Operator runbook for the trustworthy-confidence primitive (#2457) and the external-signal completion gate (#2456) — read per-judgment confidence in cycle reports, check brain calibration (ECE) and the goal_false_completion_rate, understand the hold-for-review handling of unverified completions, and tune the self-consistency / high-stakes / verification knobs.
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: howto
status: partially implemented
related:
  - ../concepts/trustworthy-confidence-and-external-completion.md
  - ../reference/trustworthy-confidence-api.md
  - ../reference/external-signal-completion-gate.md
  - ../howto/diagnose-a-rejected-goal-completion.md
  - ../reference/completion-evidence-gate-api.md
---

# How to interpret brain confidence and verify goal completion

> **Status: partially implemented (issues
> [#2457](https://github.com/rysweet/Simard/issues/2457) and
> [#2456](https://github.com/rysweet/Simard/issues/2456), both open).**
>
> This runbook documents the operator interface for the
> trustworthy-confidence primitive
> ([`src/ooda_brain/confidence.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_brain/confidence.rs) —
> **shipped**, exported from `crate::ooda_brain`; the self-consistency vote lives
> in that same file, not a separate `self_consistency.rs`) and the external-signal
> completion gate in
> [`src/goal_curation/completion_gate.rs`](https://github.com/rysweet/Simard/blob/main/src/goal_curation/completion_gate.rs).
> The `goal_completion_verification` and `goal_false_completion_rate` metrics are
> emitted live by the gate today. What is **not yet wired**: the live brain does
> not yet *populate* per-judgment `confidence` from a verbalized score, so
> `brain_confidence_ece` and the cycle-report `confidence` fields below describe
> the **target** interface and stay at their placeholder values until the
> verbalized-confidence solicitation lands. No-signal completions are **held for
> review** (blocked, not archived) today; the distinct `completed-unverified`
> persisted state is forward design (see the
> [gate reference](../reference/external-signal-completion-gate.md)). See the
> [concept](../concepts/trustworthy-confidence-and-external-completion.md) for
> the why.

You want to know **how much to trust** a brain decision, or **why a goal did not
complete**. This guide shows where these numbers *will* live and which knobs
*will* tune them once the feature ships.

## 1. Read per-judgment confidence in a cycle report

Once shipped, every Decide and engineer-lifecycle judgment will record a real
confidence. Cycle reports live under `~/.simard/cycle_reports/cycle_*.json`:

```bash
jq '.brain_judgments[] | {phase, decision, confidence, fallback}' \
  ~/.simard/cycle_reports/cycle_*.json | tail -20
```

```json
{ "phase": "decide",  "decision": "advance_goal",          "confidence": 0.82, "fallback": false }
{ "phase": "act",     "decision": "reclaim_and_redispatch", "confidence": 0.67, "fallback": false }
{ "phase": "act",     "decision": "continue_skipping",      "confidence": 0.0,  "fallback": true  }
```

How to read it:

- **`confidence: 0.82`** — verbalized confidence (or, for high-stakes judgments,
  `modal_count / K` from self-consistency).
- **`0.67`** — a 2/3 self-consistency split on an irreversible lifecycle action:
  the brain agreed twice out of three samples.
- **`confidence: 0.0` with `fallback: true`** — the brain *fell back* to the
  deterministic mapping after an LLM failure, or a solicited confidence was
  missing/malformed. Per the
  [fail-closed policy](../reference/trustworthy-confidence-api.md#default-policy),
  an *un-trustworthy* confidence reads as `0.0`, never `1.0`. Treat it as "the
  brain could not stand behind this."

> A value of exactly `1.0` means either a genuinely deterministic decision or a
> legacy record predating the field — **not** "maximally confident LLM."

## 2. Check whether confidence *means* anything (calibration)

A confidence number is only useful if it is calibrated. The brain scores its own
Expected Calibration Error against the [completion verdict](#4-understand-why-a-goal-did-not-complete):

```bash
jq -r 'select(.metric_name=="brain_confidence_ece") | "\(.timestamp) ECE=\(.value)"' \
  ~/.simard/metrics/metrics.jsonl | tail -10
```

- **Lower is better.** ECE near `0.0` means "when the brain says 0.7, it is right
  ~70% of the time."
- A rising ECE means stated confidence is drifting from reality — inspect recent
  prompts or model changes.
- ECE is computed over a rolling window of 50 verifiable judgments with 10 bins
  (see the [reference](../reference/trustworthy-confidence-api.md#calibration-expected-calibration-error)).
  `unverified_no_signal` / `error` completions carry no ground truth and are
  excluded.

## 3. Tune the confidence knobs

| Goal | Knob | Default | Effect |
| --- | --- | --- | --- |
| Spend more on shaky high-stakes calls | `SIMARD_BRAIN_SELF_CONSISTENCY_K` | `3` | Higher K = finer-grained confidence, more cost. `1` disables sampling. |
| Change what counts as "high-stakes" (Decide) | `SIMARD_BRAIN_HIGH_STAKES_URGENCY` | `0.8` | Lower = more decisions sampled. |
| Stop ECE recording | `SIMARD_BRAIN_CONFIDENCE_CALIBRATION` | `on` | `off` keeps confidence, skips the metric. |

The sampler **never exceeds the daily/weekly budget** (`SIMARD_DAILY_BUDGET_USD`
/ `SIMARD_WEEKLY_BUDGET_USD`): under budget pressure it transparently degrades to
`K = 1` rather than skipping the decision.

```bash
# Be stricter: sample 5x and treat urgency ≥ 0.6 as high-stakes.
SIMARD_BRAIN_SELF_CONSISTENCY_K=5 SIMARD_BRAIN_HIGH_STAKES_URGENCY=0.6 simard daemon
```

## 4. Understand why a goal did not complete

The [external-signal completion gate](../reference/external-signal-completion-gate.md)
refuses to mark a goal `Completed` on a self-report alone. A goal you expected to
be done can land in one of three states:

| You see | Meaning | What to do |
| --- | --- | --- |
| Goal stays active with a blocker | A strong signal was **refuted** (CI red, exit ≠ 0, issue still open). A tracking issue is opened. | Fix the failing signal; see [diagnose a rejected goal completion](./diagnose-a-rejected-goal-completion.md). |
| Status `completed-unverified` | Subordinate claimed done, but **no strong signal** corroborates it. Held for review, **not archived**. | Provide a strong signal (merge the PR, land green CI) or close it out manually. |
| Status `completed` | A strong signal (merged PR / closed issue / green CI / verified deploy) corroborated the claim. | Nothing — it will archive normally. |

Find unverified completions on the board:

```bash
simard goal-curation read | grep -i 'completed-unverified'
```

## 5. Watch the false-completion rate

```bash
jq -r 'select(.metric_name=="goal_false_completion_rate") | "\(.timestamp) rate=\(.value)"' \
  ~/.simard/metrics/metrics.jsonl | tail -10

# Per-event verdicts (verified / refuted / unverified_no_signal / error):
jq -r 'select(.metric_name=="goal_completion_verification") | "\(.context)"' \
  ~/.simard/metrics/metrics.jsonl | tail -20
```

`goal_false_completion_rate = refuted / (verified + refuted)` over the last 50
*checkable* completions. A non-zero rate means subordinates are claiming done on
work that external signals contradict — exactly the behaviour this gate exists to
catch. A rate trending up warrants tightening engineer prompts or review.

## 6. Roll back the completion ladder (emergency only)

If the stricter gate is blocking legitimate completions during an incident, you
can restore the legacy artifact-existence behaviour **without a redeploy**:

```bash
SIMARD_COMPLETION_VERIFICATION=lenient simard daemon   # legacy behaviour
SIMARD_COMPLETION_VERIFICATION=off      simard daemon   # disable the ladder entirely
```

Prefer `lenient` over `off`, and revert as soon as the incident clears — `strict`
is the safe default that keeps unproven work out of the archive.

## See also

- [Concept: trustworthy confidence + external-signal completion](../concepts/trustworthy-confidence-and-external-completion.md)
- [Trustworthy-confidence API reference](../reference/trustworthy-confidence-api.md)
- [External-signal completion gate reference](../reference/external-signal-completion-gate.md)
- [How to diagnose a rejected goal completion](./diagnose-a-rejected-goal-completion.md)
