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
    SafeUpdate { rationale: String },
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
            Self::SafeUpdate { .. } => ActionKind::SafeUpdate,
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
            | Self::ExtractIdeas { rationale }
            | Self::SafeUpdate { rationale } => rationale,
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
            Some(SyntheticPriorityKind::SafeUpdate) => DecideJudgment::SafeUpdate { rationale },
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

/// Parse the brain response using the `DECISION:` marker protocol.
/// Replaces the JSON `find('{')..rfind('}')` parser (issue #1980).
///
/// Expected format:
/// ```text
/// DECISION: advance_goal
/// The goal should proceed because the evidence supports it.
/// ```
///
/// The first non-blank line must contain `DECISION: <action_kind>`.
/// The rest of the response is the rationale.
fn parse_judgment_from_response(raw: &str) -> Result<DecideJudgment, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "decide brain returned an empty response (raw_response={:?})",
            raw
        ));
    }

    // Extract DECISION marker from first non-blank line
    let first_line = stripped.lines().find(|l| !l.trim().is_empty());
    let first_line = match first_line {
        Some(l) => l.trim(),
        None => {
            return Err(format!(
                "decide brain response had no non-blank lines; raw_response={:?}",
                super::rustyclawd::truncate_for_log_pub(raw)
            ));
        }
    };

    // Check for DECISION: prefix (case-insensitive)
    if first_line.len() < "decision:".len()
        || !first_line[.."decision:".len()].eq_ignore_ascii_case("decision:")
    {
        return Err(format!(
            "decide brain response missing DECISION: marker on first line; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        ));
    }

    let after_marker = first_line["decision:".len()..].trim();
    let choice = after_marker.split_whitespace().next().ok_or_else(|| {
        format!(
            "decide brain DECISION: marker present but no variant token; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        )
    })?;

    // Remainder after the first line is the rationale
    let rationale = stripped
        .split_once('\n')
        .map(|(_, r)| r.trim())
        .unwrap_or("")
        .to_string();
    let rationale = if rationale.is_empty() {
        "(no rationale provided)".to_string()
    } else {
        rationale
    };

    match choice {
        "advance_goal" => Ok(DecideJudgment::AdvanceGoal { rationale }),
        "run_improvement" => Ok(DecideJudgment::RunImprovement { rationale }),
        "consolidate_memory" => Ok(DecideJudgment::ConsolidateMemory { rationale }),
        "research_query" => Ok(DecideJudgment::ResearchQuery { rationale }),
        "run_gym_eval" => Ok(DecideJudgment::RunGymEval { rationale }),
        "build_skill" => Ok(DecideJudgment::BuildSkill { rationale }),
        "launch_session" => Ok(DecideJudgment::LaunchSession { rationale }),
        "poll_developer_activity" => Ok(DecideJudgment::PollDeveloperActivity { rationale }),
        "extract_ideas" => Ok(DecideJudgment::ExtractIdeas { rationale }),
        "safe_update" => Ok(DecideJudgment::SafeUpdate { rationale }),
        _ => Err(format!(
            "decide brain DECISION variant `{choice}` is not recognized; raw_response={:?}",
            super::rustyclawd::truncate_for_log_pub(raw)
        )),
    }
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

    // ----- DECISION marker parser (issue #1980) --------------------------

    #[test]
    fn parse_decision_marker_advance_goal() {
        let raw = "DECISION: advance_goal\nThe goal should proceed.";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        assert!(j.rationale().contains("should proceed"));
    }

    #[test]
    fn parse_decision_marker_run_gym_eval() {
        let raw = "DECISION: run_gym_eval\nLow score warrants re-evaluation.";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::RunGymEval);
    }

    #[test]
    fn parse_decision_marker_consolidate_memory() {
        let raw = "DECISION: consolidate_memory\nMemory compaction needed.";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
    }

    #[test]
    fn parse_decision_marker_safe_update() {
        let raw = "DECISION: safe_update\nDivergence >= 3, conditions met.";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
        assert!(j.rationale().contains("conditions met"));
    }

    #[test]
    fn parse_decision_marker_case_insensitive() {
        let raw = "decision: advance_goal\ngo";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn parse_decision_marker_skips_leading_blank_lines() {
        let raw = "\n\nDECISION: advance_goal\ngo";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
    }

    #[test]
    fn parse_decision_marker_no_rationale_defaults() {
        let raw = "DECISION: advance_goal";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        assert_eq!(j.rationale(), "(no rationale provided)");
    }

    #[test]
    fn parse_decision_marker_multiline_rationale() {
        let raw = "DECISION: advance_goal\nLine 1 of reasoning.\nLine 2 of reasoning.";
        let j = parse_judgment_from_response(raw).expect("must parse");
        assert!(j.rationale().contains("Line 1"));
        assert!(j.rationale().contains("Line 2"));
    }

    // ----- Error cases ---------------------------------------------------

    #[test]
    fn parse_empty_response_returns_error() {
        let err = parse_judgment_from_response("").expect_err("must Err");
        assert!(err.to_lowercase().contains("empty"));
    }

    #[test]
    fn parse_whitespace_only_returns_error() {
        let err = parse_judgment_from_response("   \n\t  ").expect_err("must Err");
        assert!(err.to_lowercase().contains("empty"));
    }

    #[test]
    fn parse_no_decision_marker_returns_error() {
        let raw = "I think we should advance the goal.";
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("DECISION:"),
            "error should mention missing marker: {err}"
        );
    }

    #[test]
    fn parse_unknown_variant_returns_error() {
        let raw = "DECISION: do_a_barrel_roll\nwhy not";
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("do_a_barrel_roll"),
            "error should include the bad variant: {err}"
        );
    }

    #[test]
    fn parse_json_input_rejected_without_marker() {
        // Pure JSON (the old format) should now fail — no DECISION marker
        let raw = r#"{"choice":"advance_goal","rationale":"go"}"#;
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("DECISION:"),
            "JSON without DECISION marker should be rejected (issue #1980): {err}"
        );
    }

    #[test]
    fn parse_json_wrapped_in_prose_rejected_without_marker() {
        let raw = "Here's my answer:\n```json\n{\"choice\":\"run_gym_eval\",\"rationale\":\"low score\"}\n```\nThanks!";
        let err = parse_judgment_from_response(raw).expect_err("must Err");
        assert!(
            err.contains("DECISION:"),
            "JSON-in-prose without DECISION marker should be rejected (issue #1980): {err}"
        );
    }

    // ----- All action kinds roundtrip ------------------------------------

    #[test]
    fn all_action_kinds_parse_from_decision_marker() {
        let variants = vec![
            ("advance_goal", ActionKind::AdvanceGoal),
            ("run_improvement", ActionKind::RunImprovement),
            ("consolidate_memory", ActionKind::ConsolidateMemory),
            ("research_query", ActionKind::ResearchQuery),
            ("run_gym_eval", ActionKind::RunGymEval),
            ("build_skill", ActionKind::BuildSkill),
            ("launch_session", ActionKind::LaunchSession),
            ("poll_developer_activity", ActionKind::PollDeveloperActivity),
            ("extract_ideas", ActionKind::ExtractIdeas),
            ("safe_update", ActionKind::SafeUpdate),
        ];
        for (variant, expected_kind) in variants {
            let raw = format!("DECISION: {variant}\ntest rationale");
            let j = parse_judgment_from_response(&raw)
                .unwrap_or_else(|e| panic!("variant {variant} failed: {e}"));
            assert_eq!(
                j.action_kind(),
                expected_kind,
                "variant {variant} wrong kind"
            );
        }
    }
}
