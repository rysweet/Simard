//! Prompt-driven brain for the OODA **Orient** phase — the THIRD prompt-driven
//! OODA brain, completing the round (act + decide + orient). Companion modules:
//! - [`super::rustyclawd`] — engineer-lifecycle brain (PR #1458).
//! - [`super::decide`]     — decide-phase routing brain (PR #1469).
//!
//! Decision site: per-goal **failure-penalty demotion**. The Orient phase
//! computes a base urgency from goal status + environmental boosts; this
//! brain judges how aggressively to demote that urgency given the goal's
//! recent failure history. Historically that was a single deterministic
//! formula (`urgency - 0.2 * failure_count`, clamped). This module reframes
//! it as a prompt-driven judgment so the demotion policy lives in
//! `prompt_assets/simard/ooda_orient.md` and can be iterated without code
//! changes.
//!
//! Per the standing architectural mandate, the daemon never depends on LLM
//! availability for Orient: [`DeterministicFallbackOrientBrain`] preserves
//! the pre-#1469 formula bit-for-bit and is the floor when no LLM is
//! configured *or* when the LLM-backed brain returns an invalid judgment
//! (e.g. attempts to escalate above `base_urgency`).

use super::prompt_store;
use super::rustyclawd::LlmSubmitter;
use crate::error::{SimardError, SimardResult};

const ADAPTER_TAG: &str = "ooda-orient-brain";

/// Per-failure penalty in the deterministic floor. Mirrors the
/// `FAILURE_PENALTY_PER_CONSECUTIVE` constant historically inlined in
/// `src/ooda_loop/orient.rs`. Five failures drive any goal's urgency to 0.
pub const FAILURE_PENALTY_PER_CONSECUTIVE: f64 = 0.2;

/// Prompt asset name. Loaded fresh per call from disk (with embedded
/// fallback) so prompt edits take effect on the next OODA cycle.
pub const PROMPT_NAME: &str = "ooda_orient.md";

// ---------------------------------------------------------------------------
// Context fed to the brain
// ---------------------------------------------------------------------------

/// Read-only view of one priority entry whose failure-penalty demotion the
/// Orient brain judges. Mirrors the inputs the deterministic formula
/// consumed (`base_urgency`, `failure_count`) plus identifying context
/// (`goal_id`, `base_reason`) so a fallback impl can reproduce the existing
/// behaviour exactly.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrientContext {
    pub goal_id: String,
    pub base_urgency: f64,
    pub base_reason: String,
    pub failure_count: u32,
}

// ---------------------------------------------------------------------------
// Judgment: struct (not enum) — single decision site emits a numeric demotion
// ---------------------------------------------------------------------------

/// What the brain decided about failure-penalty demotion for a single goal.
/// Schema mirrors `prompt_assets/simard/ooda_orient.md`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct OrientJudgment {
    /// Final urgency in `[0, 1]`. Validated to be `≤ base_urgency` by
    /// [`OrientJudgment::validate`].
    pub adjusted_urgency: f64,
    /// Rationale string the daemon attaches to the priority's `reason`.
    pub rationale: String,
    /// Brain's self-reported confidence. Optional in the wire format
    /// (defaults to 1.0 when absent so the deterministic fallback's
    /// "always confident" output round-trips cleanly).
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    /// Convenience field; daemon recomputes if absent.
    #[serde(default)]
    pub demotion_applied: f64,
}

fn default_confidence() -> f64 {
    1.0
}

impl OrientJudgment {
    /// Reject judgments that escalate (`adjusted_urgency > base_urgency`),
    /// produce out-of-range values, or contain non-finite floats. Callers
    /// fall back to the deterministic floor on rejection so a misbehaving
    /// LLM cannot inflate priorities.
    pub fn validate(&self, base_urgency: f64) -> Result<(), String> {
        if !self.adjusted_urgency.is_finite() {
            return Err(format!(
                "adjusted_urgency must be finite, got {}",
                self.adjusted_urgency
            ));
        }
        if !(0.0..=1.0).contains(&self.adjusted_urgency) {
            return Err(format!(
                "adjusted_urgency {} out of [0, 1]",
                self.adjusted_urgency
            ));
        }
        // Allow tiny FP slack so a brain echoing base_urgency exactly does
        // not trip on rounding.
        if self.adjusted_urgency > base_urgency + 1e-9 {
            return Err(format!(
                "adjusted_urgency {} > base_urgency {} (escalation forbidden)",
                self.adjusted_urgency, base_urgency
            ));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Single-decision-site trait for the Orient phase. Sync on purpose to match
/// [`super::OodaBrain`] and [`super::OodaDecideBrain`] — the LLM-backed impl
/// bridges to async internally so callers do not need a runtime.
pub trait OodaOrientBrain: Send + Sync {
    /// Judge the demotion for one goal with at least one consecutive failure.
    /// Implementations must guarantee the returned judgment passes
    /// [`OrientJudgment::validate`] against `ctx.base_urgency`; callers
    /// re-validate defensively.
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment>;
}

// ---------------------------------------------------------------------------
// Deterministic fallback — preserves pre-#1469 behaviour bit-for-bit
// ---------------------------------------------------------------------------

/// Fallback impl that mirrors the deterministic failure-penalty formula
/// previously inlined in `src/ooda_loop/orient.rs`. This is the safety
/// floor: when no LLM is configured (or the LLM-backed brain returns an
/// invalid judgment) the daemon's Orient phase behaves identically to its
/// pre-prompt-driven self.
#[derive(Debug, Default)]
pub struct DeterministicFallbackOrientBrain;

impl DeterministicFallbackOrientBrain {
    /// Pure helper exposed so the wire-in code path can reuse the exact
    /// formula on per-call brain errors without re-instantiating the
    /// brain.
    pub fn compute(ctx: &OrientContext) -> OrientJudgment {
        let penalty = FAILURE_PENALTY_PER_CONSECUTIVE * ctx.failure_count as f64;
        let adjusted = (ctx.base_urgency - penalty).max(0.0);
        OrientJudgment {
            adjusted_urgency: adjusted,
            rationale: format!(
                "{count} consecutive failure(s) → urgency {base:.2} − {penalty:.2}",
                count = ctx.failure_count,
                base = ctx.base_urgency,
                penalty = penalty,
            ),
            confidence: 1.0,
            demotion_applied: ctx.base_urgency - adjusted,
        }
    }
}

impl OodaOrientBrain for DeterministicFallbackOrientBrain {
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        Ok(Self::compute(ctx))
    }
}

// ---------------------------------------------------------------------------
// LLM-backed brain (mirrors RustyClawdDecideBrain shape from PR #1469)
// ---------------------------------------------------------------------------

/// LLM-backed Orient brain. Generic over [`LlmSubmitter`] so tests can swap
/// in a canned-response stub without touching production wiring. The daemon
/// uses [`build_rustyclawd_orient_brain`] to construct one wired to a real
/// session.
pub struct RustyClawdOrientBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdOrientBrain<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    /// Render the prompt with the context. Loaded fresh per call so prompt
    /// edits take effect on the next OODA cycle (see [`prompt_store`]).
    pub fn render_prompt(&self, ctx: &OrientContext) -> String {
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{goal_id}", &ctx.goal_id)
            .replace("{base_urgency}", &format!("{:.3}", ctx.base_urgency))
            .replace("{base_reason}", &ctx.base_reason)
            .replace("{failure_count}", &ctx.failure_count.to_string())
    }
}

impl<S: LlmSubmitter> OodaOrientBrain for RustyClawdOrientBrain<S> {
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        let prompt = self.render_prompt(ctx);
        let raw = self.submitter.submit(&prompt)?;
        let judgment = parse_judgment_from_response(&raw).map_err(|reason| {
            SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason,
            }
        })?;
        judgment.validate(ctx.base_urgency).map_err(|reason| {
            SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason,
            }
        })?;
        Ok(judgment)
    }
}

/// Extract a JSON object from the LLM response (LLMs sometimes wrap JSON in
/// prose / markdown fences) and parse it as a judgment. On failure the error
/// embeds the **full raw response text** (truncated for log safety) — see
/// issue #1711.
fn parse_judgment_from_response(raw: &str) -> Result<OrientJudgment, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "orient brain returned an empty response (raw_response={:?})",
            raw
        ));
    }
    let candidate = if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && end >= start
    {
        &stripped[start..=end]
    } else {
        return Err(format!(
            "orient brain response had no JSON object; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        ));
    };
    serde_json::from_str::<OrientJudgment>(candidate).map_err(|e| {
        format!(
            "orient-brain-parse-error: {e}; payload={candidate}; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        )
    })
}

// ---------------------------------------------------------------------------
// Production constructor
// ---------------------------------------------------------------------------

/// Production constructor mirroring [`super::build_rustyclawd_decide_brain`].
/// Returns `Err` if no LLM provider is configured; callers must fall back
/// to [`DeterministicFallbackOrientBrain`] so the daemon's Orient phase
/// behaves identically to its pre-prompt-driven self when LLM access is
/// unavailable.
pub fn build_rustyclawd_orient_brain() -> SimardResult<Box<dyn OodaOrientBrain>> {
    let provider = crate::session_builder::LlmProvider::resolve()?;
    let submitter = super::rustyclawd::SessionLlmSubmitter::new(provider);
    Ok(Box::new(RustyClawdOrientBrain::new(submitter)))
}

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the JSON-parse
// fallback parser the RustyClawd Orient bridge depends on. Sibling
// `orient_tests.rs` covers the public API end-to-end; these inline tests
// pin the private `parse_judgment_from_response` for the four shapes
// `parse_failure::record_parse_failure` was added to surface in #1933.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ----- (a) well-formed JSON pass-through -----------------------------
    #[test]
    fn parse_well_formed_json_returns_judgment() {
        let raw = r#"{"adjusted_urgency":0.4,"rationale":"transient","confidence":0.9}"#;
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        assert_eq!(j.rationale, "transient");
        assert!((j.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn parse_well_formed_json_defaults_confidence_when_absent() {
        let raw = r#"{"adjusted_urgency":0.3,"rationale":"x"}"#;
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert!((j.confidence - 1.0).abs() < 1e-9);
    }

    // ----- (b) JSON with surrounding prose — fallback parser must salvage -
    #[test]
    fn parse_salvages_json_wrapped_in_prose() {
        let raw = "Reasoning:\n```json\n{\"adjusted_urgency\":0.0,\"rationale\":\"chronic\",\"confidence\":0.95}\n```\nDone.";
        let j = parse_judgment_from_response(raw).expect("must salvage");
        assert!(j.adjusted_urgency.abs() < 1e-9);
        assert_eq!(j.rationale, "chronic");
    }

    #[test]
    fn parse_salvages_json_with_trailing_prose_only() {
        let raw =
            r#"{"adjusted_urgency":0.2,"rationale":"ok","confidence":1.0}  -- and that's all"#;
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert!((j.adjusted_urgency - 0.2).abs() < 1e-9);
    }

    // ----- (c) completely unparseable returns Err, never panics ----------
    #[test]
    fn parse_unparseable_returns_structured_error_with_raw_body() {
        let raw = "totally not json at all";
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        // Anti-regression for #1711.
        assert!(
            err.contains(raw),
            "error must embed raw response (issue #1711), got: {err}"
        );
    }

    #[test]
    fn parse_empty_response_returns_empty_error() {
        let err = parse_judgment_from_response("").expect_err("must Err");
        assert!(
            err.to_lowercase().contains("empty"),
            "empty-response error must mention emptiness, got: {err}"
        );
    }

    #[test]
    fn parse_whitespace_only_response_treated_as_empty() {
        let err = parse_judgment_from_response("   \n\t  ").expect_err("must Err");
        assert!(
            err.to_lowercase().contains("empty"),
            "whitespace-only must be treated as empty, got: {err}"
        );
    }

    #[test]
    fn parse_malformed_json_with_braces_returns_parse_error() {
        let raw = r#"{"adjusted_urgency":0.4,"rationale":}"#;
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("orient-brain-parse-error"),
            "malformed JSON must surface a structured serde error tag: {err}"
        );
        assert!(err.contains(raw), "raw_response must be embedded: {err}");
    }

    #[test]
    fn parse_missing_required_field_returns_parse_error() {
        // Schema guard: parses as JSON but lacks `adjusted_urgency`.
        let raw = r#"{"rationale":"forgot the urgency field"}"#;
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("orient-brain-parse-error"),
            "missing required field must surface serde error: {err}"
        );
    }
}
