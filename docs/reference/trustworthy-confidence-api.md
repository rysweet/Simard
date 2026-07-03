---
title: Trustworthy-confidence API reference
description: Reference for the brain's trustworthy-confidence primitive — the confidence field on DecideJudgment and EngineerLifecycleDecision, the verbalized-confidence wire format, the default_confidence / LOW_TRUST_CONFIDENCE policy, the self-consistency K-sampler, the calibration (ECE) metric spine, the environment knobs, and the downstream consumers (the #2432 escalation ladder and the #2433 consolidation gate).
last_updated: 2026-06-28
review_schedule: as-needed
owner: simard
doc_type: reference
status: partially implemented
related:
  - ../concepts/trustworthy-confidence-and-external-completion.md
  - ../concepts/prompt-driven-ooda-brain.md
  - ./external-signal-completion-gate.md
  - ./ooda-brain-decision-protocol.md
  - ./ooda-decide-prompt.md
  - ../../src/ooda_reasoners/decide.rs
  - ../../src/ooda_reasoners/mod.rs
  - ../../src/ooda_reasoners/orient.rs
  - ../../src/ooda_reasoners/judgment_record.rs
  - ../../src/self_metrics/mod.rs
---

# Trustworthy-confidence API reference

> **Status: partially implemented (issue [#2457](https://github.com/rysweet/Simard/issues/2457), open).**
>
> **Shipped now** — the trustworthy-confidence *primitive* in
> [`src/ooda_reasoners/confidence.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/confidence.rs)
> (re-exported from `crate::ooda_reasoners`): the fail-closed default policy
> (`default_confidence` / `LOW_TRUST_CONFIDENCE` / `confidence_or_low_trust`),
> `validate_confidence` / `clamp_confidence`; the high-stakes + irreversibility gates
> (`HIGH_STAKES_URGENCY`, `is_high_stakes`, `is_irreversible_lifecycle`,
> `should_self_consistency_sample`, `effective_k`); the self-consistency vote
> (`self_consistency_vote` → `Vote { choice, agreement, modal_count, k }`,
> `SELF_CONSISTENCY_K`, `lifecycle_conservative_rank`); the calibration spine
> (`CalibrationWindow` → `ece()`, `ECE_WINDOW`, `ECE_BINS`, `ECE_METRIC =
> "brain_confidence_ece"`); and the verbalized-confidence carriers
> `JudgedDecision` / `JudgedLifecycle` (see
> [the confidence carrier](#the-confidence-carrier)). All are unit-tested.
>
> **Exists already** (the precedents this builds on):
> [`OrientJudgment::confidence`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/orient.rs)
> with its private `default_confidence() -> f64` and `validate` pattern;
> [`DecideJudgment`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/decide.rs)
> and
> [`EngineerLifecycleDecision`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/mod.rs);
> [`ReasonerJudgmentRecord.confidence: f32`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/judgment_record.rs);
> and
> [`self_metrics::record_metric`](https://github.com/rysweet/Simard/blob/main/src/self_metrics/mod.rs).
>
> **Follow-up (not in this slice):** threading the verbalized/vote confidence
> all the way through the brain trait return types into the live
> `ReasonerJudgmentRecord` and the #2432 escalation ladder, and soliciting a
> `CONFIDENCE:` line in `ooda_decide.md` / `ooda_brain.md`. The carriers and the
> primitive exist so that wiring is a small, additive follow-up; today the
> production parse path still records the deterministic `1.0` / fallback `0.5`
> placeholders.

This reference specifies the typed surface of the trustworthy-confidence
primitive (#2457). For the rationale, see
[trustworthy confidence + external-signal completion](../concepts/trustworthy-confidence-and-external-completion.md).

## Contents

- [The confidence carrier](#the-confidence-carrier)
- [Default policy: `default_confidence` vs `LOW_TRUST_CONFIDENCE`](#default-policy)
- [Verbalized-confidence wire format](#verbalized-confidence-wire-format)
- [Validation](#validation)
- [Self-consistency sampling](#self-consistency-sampling)
- [High-stakes trigger](#high-stakes-trigger)
- [Budget gating](#budget-gating)
- [Calibration: Expected Calibration Error](#calibration-expected-calibration-error)
- [`ReasonerJudgmentRecord` integration](#reasonerjudgmentrecord-integration)
- [Downstream consumers (#2432, #2433)](#downstream-consumers)
- [Environment knobs](#environment-knobs)
- [Compatibility & wire stability](#compatibility-and-wire-stability)

## The confidence carrier

`OrientJudgment` already has a native `confidence: f64`. The two commitment-phase
judgments — `DecideJudgment` and `EngineerLifecycleDecision` — are **internally
tagged** enums (`#[serde(tag = "choice")]`) that today carry only `rationale`.
Rather than edit every variant (and the ~90 match/construction sites across the
live #2432 escalation ladder, which would also force dropping the `Eq` derive),
the shipped primitive attaches confidence with a thin **carrier** that wraps the
existing judgment with `#[serde(flatten)]`:

```rust
// src/ooda_reasoners/confidence.rs — SHIPPED
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgedDecision {
    #[serde(flatten)]
    pub judgment: DecideJudgment,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgedLifecycle {
    #[serde(flatten)]
    pub decision: EngineerLifecycleDecision,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}
```

Each exposes `new(inner, confidence)` and a `validate()` that rejects a
non-finite or out-of-range confidence.

### Why a flatten carrier, not a per-variant field

`#[serde(flatten)]` over an internally-tagged enum keeps the JSON **flat** —
`{"choice":"advance_goal","rationale":"…","confidence":0.8}` — *identical* to what
a per-variant field would emit. This is verified by a round-trip test
(`judged_decision_serializes_flat_with_confidence`).

> An earlier draft of this doc claimed a wrapper would nest the payload as
> `{"choice":{…}}` and break the cycle-report consumers. That is **incorrect**:
> `flatten` does not nest. Because the wire shape is preserved *and* the inner
> enum is untouched, the carrier is strictly more contract-preserving than a
> per-variant field — `DecideJudgment` / `EngineerLifecycleDecision` keep their
> `Eq` derive and every existing `matches!` / destructure / construction site
> compiles unchanged, and the bare inner enum still deserializes from the
> carrier's JSON (`bare_decide_judgment_still_parses_from_judged_wire`).

A per-variant `confidence` field remains a viable future refactor if a phase
needs the value inline on the enum; the carrier is the lower-risk path taken
here.

## Default policy

[`src/ooda_reasoners/confidence.rs`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/confidence.rs)
defines the policy that makes confidence safe to consume:

```rust
// src/ooda_reasoners/confidence.rs — SHIPPED
/// The cheerful default. Returned ONLY for paths where high confidence is
/// genuinely warranted and carries no privilege risk:
///   * the deterministic floor brains (always-confident by construction), and
///   * legacy JSON / cycle reports written before the field existed.
pub fn default_confidence() -> f64 { 1.0 }

/// The fail-closed floor. Returned whenever a confidence was *solicited* from
/// an LLM but could not be trusted: absent when the prompt asked for it,
/// unparseable, non-finite, or outside [0.0, 1.0].
pub const LOW_TRUST_CONFIDENCE: f64 = 0.0;

/// Canonical fail-closed resolver for a solicited confidence: a present, finite,
/// in-range value passes through; anything else degrades to LOW_TRUST_CONFIDENCE.
pub fn confidence_or_low_trust(parsed: Option<f64>) -> f64;
```

> **Critical (SR-1).** Confidence is consumed by gates that *unlock* privilege
> (extra compute, fact promotion, fewer re-verifications). A malformed or
> missing-when-requested confidence must therefore degrade to
> `LOW_TRUST_CONFIDENCE`, **never** to `1.0`. `serde(default = default_confidence)`
> is used only for the *deserialization* of trusted/legacy records; the live LLM
> parsers apply the fail-closed rule above. In particular, a **fallback to the
> deterministic mapping after an LLM parse failure is a low-trust event**
> (`LOW_TRUST_CONFIDENCE`, not `1.0`): `default_confidence() = 1.0` applies only
> when the deterministic brain is the *configured* floor and no LLM was attempted.
> You cannot earn trust by emitting garbage.

## Verbalized-confidence wire format

The primitive does **not** change the phase wire formats; it rides them.

| Phase | Wire | Confidence carrier |
| --- | --- | --- |
| Orient | JSON object | existing `"confidence"` key (unchanged) |
| Decide | `DECISION:` marker + prose ([protocol](./ooda-decide-prompt.md)) | optional `CONFIDENCE: <0..1>` line |
| Engineer-lifecycle | first-word match ([protocol](./ooda-brain-decision-protocol.md)) | optional `CONFIDENCE: <0..1>` line |

Parsing rules for the `CONFIDENCE:` line (Decide / lifecycle):

- **Present and valid** (`0.0 ≤ x ≤ 1.0`, finite) ⇒ used verbatim.
- **Present but invalid** (non-numeric, non-finite, out of range) ⇒
  `LOW_TRUST_CONFIDENCE`, logged as a soft parse warning (not a hard parse
  failure — the choice still stands).
- **Absent** ⇒ `LOW_TRUST_CONFIDENCE` when the prompt solicited it.
  `default_confidence()` applies only on the *configured* deterministic floor
  (no LLM attempted); a *fallback to* the deterministic mapping after an LLM
  parse failure is a low-trust event (`LOW_TRUST_CONFIDENCE`, not `1.0`).

Because the engineer-lifecycle phase matches only the **first word**
([#2144 protocol](./ooda-brain-decision-protocol.md)), its parser additionally
scans subsequent lines for the optional `CONFIDENCE:` line; the first-word match
itself is unchanged.

The prompt wording that solicits the line would be added to
`prompt_assets/simard/ooda_decide.md` and `prompt_assets/simard/ooda_brain.md`
(neither solicits it today) and covered by planned content-pin tests (see
[Compatibility & wire stability](#compatibility-and-wire-stability)).

Example Decide response:

```
DECISION: advance_goal
CONFIDENCE: 0.82
Goal g-2457 has a green CI run and an open PR ready to merge; advancing.
```

## Validation

Each judgment would expose a `validate()` that rejects non-finite or
out-of-range confidence, analogous to `OrientJudgment::validate` (which applies
the same finite / `[0,1]` check to its `adjusted_urgency`). Callers validate
defensively and fall back to the deterministic floor on rejection, so a
misbehaving LLM cannot inject a poisoned confidence:

```rust
pub fn validate(&self) -> Result<(), String> {
    let c = self.confidence();
    if !c.is_finite() || !(0.0..=1.0).contains(&c) {
        return Err(format!("confidence {c} out of [0, 1]"));
    }
    Ok(())
}
```

## Self-consistency sampling

The shipped `self_consistency_vote` in `src/ooda_reasoners/confidence.rs` takes K
already-sampled judgments and returns the modal choice with
`confidence = modal_count / k`.

```rust
// src/ooda_reasoners/confidence.rs — SHIPPED
pub struct Vote<K> {
    /// The winning choice (modal sample; ties broken by `rank`).
    pub choice: K,
    /// modal_count / k, in (0.0, 1.0] — the agreement confidence proxy.
    pub agreement: f64,
    pub modal_count: usize,
    pub k: usize,
}

/// Majority-vote over `samples`; `None` for an empty slice. Ties break toward
/// the highest `rank` (then first-seen) so the result is deterministic.
pub fn self_consistency_vote<K, R>(samples: &[K], rank: R) -> Option<Vote<K>>
where
    K: Eq + Clone,
    R: Fn(&K) -> i64;

/// Canonical `rank` for lifecycle decisions: the more escalating/irreversible
/// the choice, the higher the rank, so a tie resolves the cautious way.
pub fn lifecycle_conservative_rank(decision: &EngineerLifecycleDecision) -> i64;
```

`K = 3` (the default `SELF_CONSISTENCY_K`) yields confidences in `{1/3, 2/3, 1}`
and avoids 1-of-2 ties. The vote requires only `K: Eq + Clone` (an O(K²) count),
so the brain enums need no `Hash` derive — preserving their contract.

> The caller is responsible for drawing the K samples (e.g. K calls to the brain)
> and for the budget gate below; `self_consistency_vote` is the pure tally over
> the results.

### High-stakes trigger

K-sampling is reserved for consequential, hard-to-undo judgments; everything
else takes a single verbalized-confidence call.

- **Decide phase** — `is_high_stakes(urgency)` is true when
  `urgency >= HIGH_STAKES_URGENCY` (default `0.8`).
- **Engineer-lifecycle phase** — `is_irreversible_lifecycle(&decision)` is true
  only for `OpenTrackingIssue`, `ReclaimAndRedispatch`, `MarkGoalBlocked`;
  `ContinueSkipping`, `Deprioritize`, `ConsiderSelfUpdate` take a single call.

`should_self_consistency_sample(urgency, have_budget)` and
`JudgedLifecycle::warrants_self_consistency(urgency, have_budget)` combine these
with the budget gate below.

```rust
pub const HIGH_STAKES_URGENCY: f64 = 0.8;
pub const SELF_CONSISTENCY_K: usize = 3;
```

### Budget gating

Before drawing `K` samples the caller checks remaining budget headroom using the
same guard the OODA cycle already uses
([`cost_tracking::daily_summary`](https://github.com/rysweet/Simard/blob/main/src/cost_tracking.rs)
/ `weekly_summary` against `OodaConfig::daily_budget_usd` / `weekly_budget_usd`).
`effective_k(have_budget_headroom)` returns `SELF_CONSISTENCY_K` with headroom
and **`1`** without — degrading to a single verbalized-confidence call rather
than skipping the decision. Calibration spend never stalls the loop, and a
decision is always produced.

## Calibration: Expected Calibration Error

The shipped `CalibrationWindow` (in `src/ooda_reasoners/confidence.rs`) scores how
*meaningful* the stated confidence is, using the #2456 completion verdict as
ground truth.

```rust
// src/ooda_reasoners/confidence.rs — SHIPPED
pub const ECE_WINDOW: usize = 50; // most-recent samples
pub const ECE_BINS: usize = 10;   // equal-width [0,1] bins
pub const ECE_METRIC: &str = "brain_confidence_ece";

pub struct CalibrationWindow { /* bounded ring of (predicted, outcome) */ }

impl CalibrationWindow {
    pub fn new() -> Self;                       // ECE_WINDOW / ECE_BINS
    pub fn with_params(capacity: usize, bins: usize) -> Self;
    pub fn record(&mut self, predicted: f64, outcome: bool);
    /// Sample-weighted mean over non-empty bins of |avg_confidence − accuracy|.
    /// `None` while empty.
    pub fn ece(&self) -> Option<f64>;
    /// Best-effort emit of the current ECE via record_metric(ECE_METRIC, …).
    pub fn record_ece_metric(&self) -> Option<f64>;
}
```

- Realized outcome is sourced from the
  [external-signal completion gate](./external-signal-completion-gate.md):
  `verified → true`, `refuted → false`. `unverified_no_signal` and `error` carry
  no ground truth and are simply **not recorded**.
- The rolling ECE is emitted via
  `self_metrics::record_metric("brain_confidence_ece", ece, context)`.

A lower ECE is better; `0.0` means the brain's stated confidence exactly tracks
its hit-rate per bin.

## `ReasonerJudgmentRecord` integration

[`ReasonerJudgmentRecord.confidence: f32`](https://github.com/rysweet/Simard/blob/main/src/ooda_reasoners/judgment_record.rs)
already exists today, but is currently populated with **fixed placeholders**
(`1.0` non-fallback / `0.5` fallback, and `0.0` for phases with no native
confidence field). This spec would source it from the judgment's `confidence()`
(cast `f64 → f32`, mirroring the Orient `from_orient` precedent that already
reads `judgment.confidence`); a fallback would then record `LOW_TRUST_CONFIDENCE`
(`0.0`), **not** the current `0.5`. Cycle reports under
`~/.simard/cycle_reports/cycle_*.json` would therefore show the real
per-judgment confidence with no schema change.

## Downstream consumers

#2457 will **produce and expose** confidence; it does not reimplement the
consumers (both already merged):

- **Escalation ladder (#2432).** A low verbalized confidence or low
  self-consistency agreement is the explicit "spend more compute here" signal
  the [bounded escalation ladder](../concepts/prompt-driven-ooda-brain.md)
  consumes — generalizing its prior parse-miss-only trigger.
- **Consolidation / ISAO gate (#2433).** The verbalized score is made available
  so consolidation can set `CognitiveFact.confidence` from the judgment that
  produced a fact, rather than a constant.

The full ladder / gate logic remains owned by #2432 / #2433.

## Environment knobs

| Variable | Default | Meaning |
| --- | --- | --- |
| `SIMARD_BRAIN_SELF_CONSISTENCY_K` | `3` | K for high-stakes self-consistency; `1` disables sampling (verbalized-only). Hard-capped at a small bound to protect budget. |
| `SIMARD_BRAIN_HIGH_STAKES_URGENCY` | `0.8` | Decide-phase urgency threshold above which self-consistency engages. |
| `SIMARD_BRAIN_CONFIDENCE_CALIBRATION` | `on` | `off` skips ECE recording (judgments still carry confidence). |
| `SIMARD_DAILY_BUDGET_USD` | `500` | Existing budget guard the sampler honors before drawing K samples. |
| `SIMARD_WEEKLY_BUDGET_USD` | `2500` | Existing weekly budget guard. |

All knobs are read once per process at config build, alongside the existing
`OodaConfig` env reads.

## Compatibility and wire stability

- **Serde back-compat.** `#[serde(default = default_confidence)]` lets old cycle
  reports and deterministic-fallback JSON (which never wrote `confidence`)
  deserialize unchanged; new records add the flat `"confidence"` key.
- **Prompt content-pins.** Planned content-pin tests would assert the exact
  `CONFIDENCE:` solicitation wording in `ooda_decide.md` / `ooda_brain.md`, so a
  prompt edit that drops the ask fails CI.
- **Determinism preserved.** With `SIMARD_BRAIN_SELF_CONSISTENCY_K=1` and no
  `CONFIDENCE:` line, the brain behaves exactly as before this feature, except
  that the recorded confidence is the honest `LOW_TRUST_CONFIDENCE` for solicited
  LLM paths and `1.0` for the deterministic floor.

## See also

- [Concept: trustworthy confidence + external-signal completion](../concepts/trustworthy-confidence-and-external-completion.md)
- [External-signal completion gate reference](./external-signal-completion-gate.md)
- [OODA Decide prompt reference](./ooda-decide-prompt.md)
- [OODA Brain decision protocol](./ooda-brain-decision-protocol.md)
