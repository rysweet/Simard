//! Trustworthy-confidence primitive for the OODA brain (issue #2457).
//!
//! The brain decisions that gate everything — [`DecideJudgment`] and
//! [`EngineerLifecycleDecision`] — historically carried only a `rationale`;
//! only [`super::OrientJudgment`] had a native `confidence`. The escalation
//! ladder (#2432) and the consolidation gate (#2433) both *assume* a usable
//! confidence signal that the code did not produce. This module supplies that
//! signal, drawing on two results that survive the #2455 critique:
//!
//! - **Verbalized confidence** (*Just Ask for Calibration*, EMNLP 2023,
//!   [2305.14975]): ask the model to *state* a 0–1 probability; for RLHF
//!   models this is better-calibrated than token log-probs. Represented here by
//!   the [`JudgedDecision`] / [`JudgedLifecycle`] wrappers, which attach a
//!   validated `confidence` to the existing tagged-enum judgments **without
//!   changing their wire shape** (`#[serde(flatten)]` keeps the JSON flat:
//!   `{"choice":..,"rationale":..,"confidence":..}`).
//! - **Self-consistency** (*Self-Consistency*, ICLR 2023, [2203.11171]): sample
//!   K decisions and majority-vote; the agreement fraction is a confidence
//!   proxy that needs no external feedback. Implemented by
//!   [`self_consistency_vote`], bounded by [`SELF_CONSISTENCY_K`] and gated to
//!   high-stakes/irreversible decisions ([`should_self_consistency_sample`],
//!   [`is_irreversible_lifecycle`]) so the K× cost stays confined.
//!
//! Calibration is measured against realized outcomes with [`CalibrationWindow`]
//! (rolling expected-calibration-error over [`ECE_WINDOW`] samples and
//! [`ECE_BINS`] equal-width bins) — the honest check on whether a stated
//! confidence means anything on Simard's own outcomes.
//!
//! Everything here is pure and dependency-light so it is exhaustively
//! unit-testable and free of the LLM/runtime coupling of the brain itself.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::{DecideJudgment, EngineerLifecycleDecision};

// ---------------------------------------------------------------------------
// Tunable constants (named, not magic numbers)
// ---------------------------------------------------------------------------

/// A priority is "high-stakes" — and thus eligible for the K× self-consistency
/// vote — when its urgency is at or above this threshold. There is no discrete
/// `Priority` *tier* in the codebase (`Priority` is a struct with `urgency:
/// f64`), so the gate is a numeric threshold rather than a tier name.
pub const HIGH_STAKES_URGENCY: f64 = 0.8;

/// Number of independent samples drawn for a self-consistency vote. `3` avoids
/// the binary-tie pathology of an even K while keeping the cost multiplier
/// bounded. Degrades to `1` (single verbalized confidence) when the budget has
/// no headroom — see [`effective_k`].
pub const SELF_CONSISTENCY_K: usize = 3;

/// Rolling window length over which the expected-calibration-error is computed.
pub const ECE_WINDOW: usize = 50;

/// Number of equal-width confidence bins used by the ECE computation.
pub const ECE_BINS: usize = 10;

/// Metric name under which the rolling ECE is emitted via
/// [`crate::self_metrics::record_metric`].
pub const ECE_METRIC: &str = "brain_confidence_ece";

// ---------------------------------------------------------------------------
// Confidence scalars
// ---------------------------------------------------------------------------

/// The default confidence used when a wire payload omits the field. `1.0`
/// mirrors [`super::OrientJudgment`] so the deterministic fallback's
/// "always confident" output round-trips cleanly and old JSON keeps parsing.
///
/// **Trust boundary.** `default_confidence` is the cheerful default used only
/// where high confidence is genuinely warranted: deserializing a trusted/legacy
/// record that predates the field, and the *configured* deterministic floor
/// brain (always-confident by construction). It must **never** be returned for
/// a confidence that was *solicited* from an LLM but came back absent or
/// malformed — that path must fail closed to [`LOW_TRUST_CONFIDENCE`]. See
/// [`confidence_or_low_trust`].
pub fn default_confidence() -> f64 {
    1.0
}

/// The fail-closed floor. Returned whenever a confidence was *solicited* from an
/// LLM but could not be trusted: absent when the prompt asked for it,
/// unparseable, non-finite, or outside `[0, 1]`.
///
/// Confidence is consumed by gates that *unlock* privilege (extra compute, fact
/// promotion, fewer re-verifications), so a missing-or-malformed value must
/// degrade to the **least** privileged number, never to `1.0`. You cannot earn
/// trust by emitting garbage.
pub const LOW_TRUST_CONFIDENCE: f64 = 0.0;

/// Reject a confidence that is non-finite or outside `[0, 1]`. Callers treat a
/// rejection the same way they treat any other invalid judgment: fall back to
/// the deterministic floor rather than trust a malformed signal.
pub fn validate_confidence(confidence: f64) -> Result<(), String> {
    if !confidence.is_finite() {
        return Err(format!("confidence must be finite, got {confidence}"));
    }
    if !(0.0..=1.0).contains(&confidence) {
        return Err(format!("confidence {confidence} out of [0, 1]"));
    }
    Ok(())
}

/// Resolve a *solicited* confidence to a trusted scalar, failing closed. Maps a
/// present-and-valid `[0, 1]` finite value through unchanged, and **any** other
/// case (absent, non-finite, out of range) to [`LOW_TRUST_CONFIDENCE`]. This is
/// the canonical policy for confidences parsed out of an LLM reply: it can never
/// turn a missing or poisoned value into a privilege-granting high confidence.
pub fn confidence_or_low_trust(parsed: Option<f64>) -> f64 {
    match parsed {
        Some(c) if validate_confidence(c).is_ok() => c,
        _ => LOW_TRUST_CONFIDENCE,
    }
}

/// Clamp any float into `[0, 1]`, mapping NaN to `0.0`. Used where a usable
/// (if conservative) number is preferable to an error — e.g. deriving a
/// `Fact.confidence` for the consolidation gate (#2433).
pub fn clamp_confidence(confidence: f64) -> f64 {
    if confidence.is_nan() {
        0.0
    } else {
        confidence.clamp(0.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// High-stakes / irreversibility gating
// ---------------------------------------------------------------------------

/// Whether a priority's urgency clears the [`HIGH_STAKES_URGENCY`] threshold.
pub fn is_high_stakes(urgency: f64) -> bool {
    urgency.is_finite() && urgency >= HIGH_STAKES_URGENCY
}

/// Whether an engineer-lifecycle decision is irreversible / escalating, and so
/// worth the extra self-consistency samples even when the priority urgency is
/// not itself high. Reclaiming a worktree, opening a tracking issue, and
/// marking a goal blocked are all hard to walk back; merely continuing to skip
/// or deprioritizing are cheap and reversible.
pub fn is_irreversible_lifecycle(decision: &EngineerLifecycleDecision) -> bool {
    matches!(
        decision,
        EngineerLifecycleDecision::OpenTrackingIssue { .. }
            | EngineerLifecycleDecision::ReclaimAndRedispatch { .. }
            | EngineerLifecycleDecision::MarkGoalBlocked { .. }
    )
}

/// Whether to spend the K× self-consistency vote on this decision: only when it
/// is consequential (high-stakes urgency) **and** the budget has headroom. With
/// no headroom we still decide — just with a single verbalized-confidence
/// sample (see [`effective_k`]).
pub fn should_self_consistency_sample(urgency: f64, have_budget_headroom: bool) -> bool {
    is_high_stakes(urgency) && have_budget_headroom
}

/// The effective sample count given budget headroom: the full
/// [`SELF_CONSISTENCY_K`] when there is room, otherwise `1` (degrade to a
/// single sample, never to *no* decision).
pub fn effective_k(have_budget_headroom: bool) -> usize {
    if have_budget_headroom {
        SELF_CONSISTENCY_K
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// Self-consistency vote
// ---------------------------------------------------------------------------

/// Result of a self-consistency vote over K samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Vote<K> {
    /// The winning choice (modal sample; ties broken by the caller's rank).
    pub choice: K,
    /// Vote agreement = `modal_count / k` in `[0, 1]`. This is the confidence
    /// proxy that needs no external feedback.
    pub agreement: f64,
    /// Number of samples that agreed on [`Vote::choice`].
    pub modal_count: usize,
    /// Total number of samples considered.
    pub k: usize,
}

/// Majority-vote over `samples`, returning the modal choice and the agreement
/// fraction as confidence. `None` for an empty slice.
///
/// Ties (multiple choices sharing the maximum count) are broken
/// deterministically: the choice with the highest `rank` wins, and among equal
/// ranks the one that appeared **first** in `samples` wins. Pass a `rank` that
/// scores the more conservative / more escalating option higher so a tie
/// resolves the safe way rather than arbitrarily.
pub fn self_consistency_vote<K, R>(samples: &[K], rank: R) -> Option<Vote<K>>
where
    K: Eq + Clone,
    R: Fn(&K) -> i64,
{
    if samples.is_empty() {
        return None;
    }
    // Count occurrences by linear scan. K is tiny here (K ≤ SELF_CONSISTENCY_K),
    // so an O(n²) count avoids requiring `K: Hash` — keeping the brain decision
    // enums free of any extra derive (contract preservation).
    let count_of = |target: &K| -> usize { samples.iter().filter(|s| *s == target).count() };
    let max_count = samples.iter().map(&count_of).max().unwrap_or(0);

    // First-seen order index for stable tie-breaking among equal ranks.
    let first_index = |target: &K| -> i64 {
        samples
            .iter()
            .position(|s| s == target)
            .unwrap_or(usize::MAX) as i64
    };

    // Among the modal choices, pick the highest `(rank, earliest-first-seen)`.
    // A composite key sidesteps the "last maximum wins" rule of `max_by`:
    // higher rank wins; on equal rank the smaller first-seen index wins
    // (negated so "earlier" sorts larger). Duplicate modal samples share a key,
    // so the choice is deterministic regardless of which copy is returned.
    let choice = samples
        .iter()
        .filter(|s| count_of(s) == max_count)
        .max_by_key(|s| (rank(s), -first_index(s)))
        .cloned()?;

    Some(Vote {
        agreement: max_count as f64 / samples.len() as f64,
        modal_count: max_count,
        k: samples.len(),
        choice,
    })
}

// ---------------------------------------------------------------------------
// Verbalized-confidence wrappers (native confidence on the brain judgments)
// ---------------------------------------------------------------------------

/// A [`DecideJudgment`] carrying a brain-stated confidence. `#[serde(flatten)]`
/// keeps the wire shape identical to the bare judgment plus a sibling
/// `confidence` field, so old prompts/JSON (no `confidence`) still parse and the
/// bare [`DecideJudgment`] still deserializes from the same bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgedDecision {
    #[serde(flatten)]
    pub judgment: DecideJudgment,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

impl JudgedDecision {
    pub fn new(judgment: DecideJudgment, confidence: f64) -> Self {
        Self {
            judgment,
            confidence,
        }
    }

    /// Validate the attached confidence (finite, `[0, 1]`).
    pub fn validate(&self) -> Result<(), String> {
        validate_confidence(self.confidence)
    }
}

/// An [`EngineerLifecycleDecision`] carrying a brain-stated confidence. Same
/// flatten/back-compat properties as [`JudgedDecision`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JudgedLifecycle {
    #[serde(flatten)]
    pub decision: EngineerLifecycleDecision,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

impl JudgedLifecycle {
    pub fn new(decision: EngineerLifecycleDecision, confidence: f64) -> Self {
        Self {
            decision,
            confidence,
        }
    }

    /// Validate the attached confidence (finite, `[0, 1]`).
    pub fn validate(&self) -> Result<(), String> {
        validate_confidence(self.confidence)
    }

    /// Whether this decision warrants the K× self-consistency vote given the
    /// driving priority urgency and the budget headroom.
    pub fn warrants_self_consistency(&self, urgency: f64, have_budget_headroom: bool) -> bool {
        have_budget_headroom
            && (is_high_stakes(urgency) || is_irreversible_lifecycle(&self.decision))
    }
}

// ---------------------------------------------------------------------------
// Calibration: rolling expected-calibration-error (ECE)
// ---------------------------------------------------------------------------

/// A bounded rolling window of `(predicted_confidence, realized_outcome)` pairs
/// used to compute the expected-calibration-error of the brain's stated
/// confidences. `realized_outcome` is the objective ground truth — for the
/// completion gate (#2456) this is `verified → true`, `refuted → false`;
/// signal-less outcomes are simply not recorded.
#[derive(Clone, Debug)]
pub struct CalibrationWindow {
    samples: VecDeque<(f64, bool)>,
    capacity: usize,
    bins: usize,
}

impl Default for CalibrationWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl CalibrationWindow {
    /// A window with the module defaults ([`ECE_WINDOW`], [`ECE_BINS`]).
    pub fn new() -> Self {
        Self::with_params(ECE_WINDOW, ECE_BINS)
    }

    /// A window with explicit capacity and bin count. Both are clamped to at
    /// least 1 so the structure is always usable.
    pub fn with_params(capacity: usize, bins: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            capacity: capacity.max(1),
            bins: bins.max(1),
        }
    }

    /// Record one prediction/outcome pair. The predicted confidence is clamped
    /// into `[0, 1]`; the oldest sample is evicted once the window is full.
    pub fn record(&mut self, predicted: f64, outcome: bool) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples
            .push_back((clamp_confidence(predicted), outcome));
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Expected calibration error over equal-width bins: the sample-weighted
    /// mean of `|mean_confidence − accuracy|` per bin. `None` while empty.
    /// `0.0` means perfectly calibrated; `1.0` is the worst case.
    pub fn ece(&self) -> Option<f64> {
        let total = self.samples.len();
        if total == 0 {
            return None;
        }
        let mut bin_conf_sum = vec![0.0_f64; self.bins];
        let mut bin_hit_sum = vec![0.0_f64; self.bins];
        let mut bin_count = vec![0_usize; self.bins];

        for &(conf, outcome) in &self.samples {
            // conf is already clamped to [0,1]; map 1.0 into the last bin.
            let mut idx = (conf * self.bins as f64).floor() as usize;
            if idx >= self.bins {
                idx = self.bins - 1;
            }
            bin_conf_sum[idx] += conf;
            bin_hit_sum[idx] += if outcome { 1.0 } else { 0.0 };
            bin_count[idx] += 1;
        }

        let mut ece = 0.0;
        for b in 0..self.bins {
            let n = bin_count[b];
            if n == 0 {
                continue;
            }
            let mean_conf = bin_conf_sum[b] / n as f64;
            let accuracy = bin_hit_sum[b] / n as f64;
            ece += (n as f64 / total as f64) * (mean_conf - accuracy).abs();
        }
        Some(ece)
    }

    /// Emit the current ECE via [`crate::self_metrics::record_metric`] under
    /// [`ECE_METRIC`]. Best-effort and a no-op while the window is empty;
    /// returns the value that was recorded (if any).
    pub fn record_ece_metric(&self) -> Option<f64> {
        let ece = self.ece()?;
        let _ = crate::self_metrics::record_metric(
            ECE_METRIC,
            ece,
            &format!("n={} bins={}", self.len(), self.bins),
        );
        Some(ece)
    }
}

/// Convenience: the inverse-of-correct rate over realized outcomes — the share
/// of recorded samples whose outcome was `false`. With outcomes sourced from
/// the #2456 gate (`refuted → false`) this is the false-completion rate the
/// issue asks to trend down.
pub fn false_outcome_rate(outcomes: &[bool]) -> Option<f64> {
    if outcomes.is_empty() {
        return None;
    }
    let bad = outcomes.iter().filter(|o| !**o).count();
    Some(bad as f64 / outcomes.len() as f64)
}

/// Tie-break rank for an [`EngineerLifecycleDecision`]: the more escalating /
/// irreversible the action, the higher the rank, so a self-consistency tie
/// resolves toward the safer (more cautious) choice. Provided as the canonical
/// `rank` argument to [`self_consistency_vote`] for lifecycle decisions.
pub fn lifecycle_conservative_rank(decision: &EngineerLifecycleDecision) -> i64 {
    match decision {
        EngineerLifecycleDecision::MarkGoalBlocked { .. } => 5,
        EngineerLifecycleDecision::OpenTrackingIssue { .. } => 4,
        EngineerLifecycleDecision::ReclaimAndRedispatch { .. } => 3,
        EngineerLifecycleDecision::ConsiderSelfUpdate { .. } => 2,
        EngineerLifecycleDecision::Deprioritize { .. } => 1,
        EngineerLifecycleDecision::ContinueSkipping { .. } => 0,
    }
}

#[cfg(test)]
mod tests;
