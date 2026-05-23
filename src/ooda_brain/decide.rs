//! Prompt-driven brain for the OODA **Decide** phase (extends the pattern
//! established for the engineer-lifecycle skip branch in PR #1458).
//!
//! Today the Decide phase performs one judgment site per priority: mapping a
//! `Priority` (goal_id + reason + urgency) to an [`ActionKind`]. Historically
//! that mapping was a deterministic match-arm on `goal_id` prefix. This module
//! reframes it as a prompt-driven decision so the routing rules live in
//! `prompt_assets/simard/ooda_decide.md` and can be iterated without code
//! changes.
//!
//! Per the standing architectural mandate, the daemon never depends on LLM
//! availability for Decide: [`DeterministicFallbackDecideBrain`] preserves the
//! pre-#1458 mapping bit-for-bit and is the floor when no LLM is configured.

use super::prompt_store;
use super::rustyclawd::LlmSubmitter;
use crate::error::{SimardError, SimardResult};
use crate::ooda_loop::ActionKind;
use crate::ooda_loop::SyntheticPriorityKind;

const ADAPTER_TAG: &str = "ooda-decide-brain";

/// Prompt asset name. The on-disk file is read fresh per call via
/// [`prompt_store::global`]; the compile-time embedded baseline is
/// preserved in [`prompt_store::embedded_fallback`] so the daemon never
/// fails because a prompt file is missing.
pub const PROMPT_NAME: &str = "ooda_decide.md";

// ---------------------------------------------------------------------------
// Context fed to the brain
// ---------------------------------------------------------------------------

/// Read-only view of one priority entry that the Decide brain judges. Mirrors
/// the per-priority columns that the deterministic match-arm consumed
/// (`goal_id`, `urgency`, `reason`) so a fallback impl can reproduce the
/// existing routing exactly.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DecideContext {
    pub goal_id: String,
    pub urgency: f64,
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Judgment: tagged enum the LLM emits as `{"choice":"...","rationale":"..."}`
// ---------------------------------------------------------------------------

/// What action kind the brain decided this priority maps to. Tagged on
/// `choice` for forward-compatibility; unknown tags fail to parse and the
/// caller falls back to the deterministic mapping.
///
/// Variants intentionally cover every [`ActionKind`] (not just the five
/// emitted today) so the prompt can evolve to route to currently-unused
/// kinds (`research_query`, `run_gym_eval`, `build_skill`, `launch_session`)
/// without touching this file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum DecideJudgment {
    AdvanceGoal { rationale: String },
    RunImprovement { rationale: String },
    ConsolidateMemory { rationale: String },
    ResearchQuery { rationale: String },
    RunGymEval { rationale: String },
    BuildSkill { rationale: String },
    LaunchSession { rationale: String },
    PollDeveloperActivity { rationale: String },
    ExtractIdeas { rationale: String },
}

impl DecideJudgment {
    /// Project the tagged judgment back onto the existing [`ActionKind`]
    /// enum so the Decide phase can keep emitting `PlannedAction` values
    /// unchanged.
    pub fn action_kind(&self) -> ActionKind {
        match self {
            Self::AdvanceGoal { .. } => ActionKind::AdvanceGoal,
            Self::RunImprovement { .. } => ActionKind::RunImprovement,
            Self::ConsolidateMemory { .. } => ActionKind::ConsolidateMemory,
            Self::ResearchQuery { .. } => ActionKind::ResearchQuery,
            Self::RunGymEval { .. } => ActionKind::RunGymEval,
            Self::BuildSkill { .. } => ActionKind::BuildSkill,
            Self::LaunchSession { .. } => ActionKind::LaunchSession,
            Self::PollDeveloperActivity { .. } => ActionKind::PollDeveloperActivity,
            Self::ExtractIdeas { .. } => ActionKind::ExtractIdeas,
        }
    }

    pub fn rationale(&self) -> &str {
        match self {
            Self::AdvanceGoal { rationale }
            | Self::RunImprovement { rationale }
            | Self::ConsolidateMemory { rationale }
            | Self::ResearchQuery { rationale }
            | Self::RunGymEval { rationale }
            | Self::BuildSkill { rationale }
            | Self::LaunchSession { rationale }
            | Self::PollDeveloperActivity { rationale }
            | Self::ExtractIdeas { rationale } => rationale,
        }
    }
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Single-decision-site trait for the Decide phase. Sync on purpose to match
/// [`super::OodaBrain`] — the LLM-backed impl bridges to async internally so
/// callers do not need a runtime.
pub trait OodaDecideBrain: Send + Sync {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment>;
}

// ---------------------------------------------------------------------------
// Deterministic fallback — preserves pre-#1458 behaviour bit-for-bit
// ---------------------------------------------------------------------------

/// Fallback impl that mirrors the deterministic match-arm previously inlined
/// in `src/ooda_loop/decide.rs`. This is the safety floor: when no LLM is
/// configured, the daemon's Decide phase behaves identically to its
/// pre-prompt-driven self.
#[derive(Debug, Default)]
pub struct DeterministicFallbackDecideBrain;

impl OodaDecideBrain for DeterministicFallbackDecideBrain {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let rationale = "fallback-brain: prefix-routed".to_string();
        // Route synthetic priorities via the typed enum (single source of
        // truth in `ooda_loop::priority_kind`). Real goal_ids — and any
        // unrecognized synthetic — fall through to AdvanceGoal.
        let judgment = match SyntheticPriorityKind::from_synthetic_id(ctx.goal_id.as_str()) {
            Some(SyntheticPriorityKind::ConsolidateMemory) => {
                DecideJudgment::ConsolidateMemory { rationale }
            }
            Some(SyntheticPriorityKind::RunImprovement) => {
                DecideJudgment::RunImprovement { rationale }
            }
            Some(SyntheticPriorityKind::PollDeveloperActivity) => {
                DecideJudgment::PollDeveloperActivity { rationale }
            }
            Some(SyntheticPriorityKind::ExtractIdeas) => DecideJudgment::ExtractIdeas { rationale },
            Some(SyntheticPriorityKind::EvalWatchdog) | None => {
                DecideJudgment::AdvanceGoal { rationale }
            }
        };
        Ok(judgment)
    }
}

// ---------------------------------------------------------------------------
// LLM-backed brain (mirrors the RustyClawdBrain shape from PR #1458)
// ---------------------------------------------------------------------------

/// LLM-backed Decide brain. Generic over [`LlmSubmitter`] so tests can swap
/// in a canned-response stub without touching production wiring. The daemon
/// uses [`build_rustyclawd_decide_brain`] to construct one wired to a real
/// session.
pub struct RustyClawdDecideBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdDecideBrain<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    /// Render the prompt with the context. Loaded fresh per call so prompt
    /// edits take effect on the next OODA cycle (see [`prompt_store`]).
    pub fn render_prompt(&self, ctx: &DecideContext) -> String {
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{goal_id}", &ctx.goal_id)
            .replace("{urgency}", &format!("{:.3}", ctx.urgency))
            .replace("{reason}", &ctx.reason)
    }
}

impl<S: LlmSubmitter> OodaDecideBrain for RustyClawdDecideBrain<S> {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let prompt = self.render_prompt(ctx);
        let raw = self.submitter.submit(&prompt)?;
        parse_judgment_from_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }
}

/// Extract a JSON object from the LLM response (LLMs sometimes wrap JSON in
/// prose / markdown fences) and parse it as a judgment. On failure the error
/// embeds the **full raw response text** (truncated for log safety) so
/// operators can diagnose the model behaviour — this replaces the legacy
/// `got N bytes` byte-count format that originated the issue #1711 bug.
fn parse_judgment_from_response(raw: &str) -> Result<DecideJudgment, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "decide brain returned an empty response (raw_response={:?})",
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
            "decide brain response had no JSON object; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        ));
    };
    serde_json::from_str::<DecideJudgment>(candidate).map_err(|e| {
        format!(
            "decide-brain-parse-error: {e}; payload={candidate}; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        )
    })
}

// ---------------------------------------------------------------------------
// Production constructor
// ---------------------------------------------------------------------------

/// Production constructor mirroring [`super::build_rustyclawd_brain`].
/// Returns `Err` if no LLM provider is configured; callers must fall back
/// to [`DeterministicFallbackDecideBrain`] so the daemon's Decide phase
/// behaves identically to its pre-prompt-driven self when LLM access is
/// unavailable.
pub fn build_rustyclawd_decide_brain() -> SimardResult<Box<dyn OodaDecideBrain>> {
    let provider = crate::session_builder::LlmProvider::resolve()?;
    let submitter = super::rustyclawd::SessionLlmSubmitter::new(provider);
    Ok(Box::new(RustyClawdDecideBrain::new(submitter)))
}

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the JSON-parse
// fallback parser the RustyClawd Decide bridge depends on. Sibling
// `decide_tests.rs` covers the end-to-end public API; these inline tests
// pin the private `parse_judgment_from_response` for the four shapes
// `parse_failure::record_parse_failure` was added to surface in #1933.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_loop::ActionKind;

    // ----- (a) well-formed JSON pass-through -----------------------------
    #[test]
    fn parse_well_formed_json_returns_judgment() {
        let raw = r#"{"choice":"advance_goal","rationale":"go"}"#;
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        assert_eq!(j.rationale(), "go");
    }

    // ----- (b) JSON with surrounding prose — fallback parser must salvage -
    #[test]
    fn parse_salvages_json_wrapped_in_prose() {
        // Both leading and trailing prose around the object. The
        // `find('{') .. rfind('}')` salvage must extract the JSON body.
        let raw = "Here's my answer:\n```json\n{\"choice\":\"run_gym_eval\",\"rationale\":\"low score\"}\n```\nThanks!";
        let j = parse_judgment_from_response(raw).expect("must salvage JSON in prose");
        assert_eq!(j.action_kind(), ActionKind::RunGymEval);
    }

    #[test]
    fn parse_salvages_json_with_trailing_prose_only() {
        // The parser slices from the first `{` to the last `}`, so trailing
        // commentary after the closing brace must not break the salvage.
        let raw = r#"{"choice":"consolidate_memory","rationale":"compaction"}  -- thanks"#;
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    // ----- (c) completely unparseable returns Err, never panics ----------
    #[test]
    fn parse_unparseable_returns_structured_error_with_raw_body() {
        let raw = "totally not json at all";
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        // Anti-regression for #1711: the error must embed the raw response
        // so operators can diagnose. (`raw` has no `{` so it hits the
        // "no JSON object" branch, not the serde-error branch.)
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
        // Hits the serde-error branch (has `{` and `}` but invalid JSON).
        let raw = r#"{"choice":"advance_goal","rationale":}"#;
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("decide-brain-parse-error"),
            "malformed JSON must surface a structured serde error tag, got: {err}"
        );
        // #1711: full raw must be embedded.
        assert!(err.contains(raw), "raw_response must be embedded: {err}");
    }

    #[test]
    fn parse_unknown_choice_tag_returns_error() {
        // Schema guard: unrecognised `choice` tags must fail to parse so the
        // consumer falls back to the deterministic mapping.
        let raw = r#"{"choice":"do_a_barrel_roll","rationale":"why not"}"#;
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("decide-brain-parse-error"),
            "unknown variant must surface serde-tagged error: {err}"
        );
    }
}
