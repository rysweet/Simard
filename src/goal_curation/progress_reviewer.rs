//! LLM-backed [`ProgressEvidenceChecker`] — production implementation
//! (replaced the former `DefaultProgressEvidenceChecker` in PR #2007).
//!
//! Per user direction (post-PR #1970 review): "the progress assessment
//! should be an llm reviewing the problem, the plan, and the progress
//! against the plan, thats all." This module implements exactly that —
//! no git introspection, no `gh pr list` shellouts, no Rust state-machine
//! gate. A single LLM call reads `{problem, plan, prior_pct, claimed_pct,
//! wip_summary}` and returns `{verdict, rationale}`.
//!
//! The trait stays in [`super::progress_evidence`]; this module only
//! provides a new implementation. The kill-switch
//! [`super::progress_evidence::NoopProgressEvidenceChecker`] continues to
//! work via `SIMARD_PROGRESS_EVIDENCE=off`.
//!
//! Prompt template: `prompt_assets/simard/progress_assessment_reviewer.md`.
//! Response shape: `{"verdict": "accept"|"reject", "rationale": "..."}`.
//!
//! Failure handling: on LLM transport error, JSON parse error, or empty
//! response, the reviewer **accepts** with a diagnostic rationale. The
//! purpose of this gate is to catch hallucinated progress jumps — not to
//! block goals on LLM infrastructure issues.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::progress_evidence::{EvidenceDecision, ProgressEvidenceChecker};
use super::types::ActiveGoal;
use crate::ooda_brain::LlmSubmitter;
use crate::ooda_brain::prompt_store;

pub const PROMPT_NAME: &str = "progress_assessment_reviewer.md";
const ADAPTER_TAG: &str = "progress-assessment-reviewer";

/// Max chars retained from the LLM rationale before the gate truncates.
/// The prompt asks for one short sentence; this is a safety net.
const RATIONALE_MAX_CHARS: usize = 240;

/// JSON contract from the prompt template. Kept private — callers only
/// see the `EvidenceDecision` returned by [`ProgressEvidenceChecker::check`].
#[derive(Debug, Deserialize)]
struct ReviewerResponse {
    verdict: String,
    rationale: String,
}

/// LLM-backed checker. Generic over `LlmSubmitter` so tests can swap in a
/// canned-response stub without any global state.
pub struct LlmReviewerProgressChecker<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> LlmReviewerProgressChecker<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    fn render_prompt(&self, goal: &ActiveGoal, prior_pct: u32, claimed_pct: u32) -> String {
        let plan = goal
            .current_activity
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        let wip_summary = render_wip_summary(goal);
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{goal_id}", &goal.id)
            .replace("{problem}", &goal.description)
            .replace("{plan}", &plan)
            .replace("{prior_pct}", &prior_pct.to_string())
            .replace("{claimed_pct}", &claimed_pct.to_string())
            .replace("{wip_summary}", &wip_summary)
    }
}

impl<S: LlmSubmitter> ProgressEvidenceChecker for LlmReviewerProgressChecker<S> {
    fn check(
        &self,
        goal: &ActiveGoal,
        old_percent: u32,
        new_percent: u32,
        _since: DateTime<Utc>,
    ) -> EvidenceDecision {
        // Downward self-correction is always accepted (per prompt rules).
        if new_percent <= old_percent {
            return EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: downward / no-change ({old_percent} -> {new_percent}) auto-accepted"
                ),
            };
        }

        let prompt = self.render_prompt(goal, old_percent, new_percent);
        let raw = match self.submitter.submit(&prompt) {
            Ok(r) => r,
            Err(e) => {
                // Infra failure — accept with diagnostic so we don't block
                // goals on LLM transport hiccups.
                return EvidenceDecision::Accept {
                    reason: format!(
                        "{ADAPTER_TAG}: LLM submit failed ({e}); accepting to avoid blocking goal"
                    ),
                };
            }
        };

        match parse_reviewer_response(&raw) {
            Ok(parsed) => decision_from_response(parsed),
            Err(parse_err) => EvidenceDecision::Accept {
                reason: format!(
                    "{ADAPTER_TAG}: parse error ({parse_err}); accepting to avoid blocking goal"
                ),
            },
        }
    }
}

fn render_wip_summary(goal: &ActiveGoal) -> String {
    if goal.wip_refs.is_empty() {
        return String::new();
    }
    use std::fmt::Write;
    let mut s = String::new();
    for (i, w) in goal.wip_refs.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "{:?}", w);
    }
    s
}

fn decision_from_response(r: ReviewerResponse) -> EvidenceDecision {
    let trimmed = r.rationale.trim();
    let rationale = {
        let mut chars = trimmed.chars();
        let prefix: String = chars.by_ref().take(RATIONALE_MAX_CHARS).collect();
        if chars.next().is_some() {
            prefix + "…"
        } else {
            prefix
        }
    };
    let verdict_lc = r.verdict.trim().to_ascii_lowercase();
    if verdict_lc == "accept" {
        EvidenceDecision::Accept {
            reason: format!("{ADAPTER_TAG}: accept — {rationale}"),
        }
    } else if verdict_lc == "reject" {
        EvidenceDecision::Reject {
            reason: format!("{ADAPTER_TAG}: reject — {rationale}"),
        }
    } else {
        // Unknown verdict string — accept with diagnostic.
        EvidenceDecision::Accept {
            reason: format!(
                "{ADAPTER_TAG}: unknown verdict {:?}; accepting to avoid blocking goal",
                r.verdict
            ),
        }
    }
}

/// Parse the LLM response into a [`ReviewerResponse`].
///
/// Strategy mirrors `merge_judge::parse_judge_response`:
///   1. Try parsing the trimmed input as-is.
///   2. Look inside fenced code blocks.
///   3. Scan for the first brace-balanced `{...}` that parses cleanly.
///   4. Fall back to outermost-brace strategy.
fn parse_reviewer_response(raw: &str) -> Result<ReviewerResponse, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "{ADAPTER_TAG} returned an empty response (raw={:?})",
            raw
        ));
    }
    // 1. as-is
    if let Ok(parsed) = serde_json::from_str::<ReviewerResponse>(stripped) {
        return Ok(parsed);
    }
    // 2. fenced blocks
    for candidate in extract_fenced_blocks(stripped) {
        if let Ok(parsed) = serde_json::from_str::<ReviewerResponse>(candidate.trim()) {
            return Ok(parsed);
        }
    }
    // 3. brace-balanced spans
    for candidate in extract_balanced_objects(stripped) {
        if let Ok(parsed) = serde_json::from_str::<ReviewerResponse>(candidate) {
            return Ok(parsed);
        }
    }
    // 4. outermost braces fallback
    if let (Some(first), Some(last)) = (stripped.find('{'), stripped.rfind('}'))
        && first < last
        && let Ok(parsed) = serde_json::from_str::<ReviewerResponse>(&stripped[first..=last])
    {
        return Ok(parsed);
    }
    Err(format!(
        "{ADAPTER_TAG} response had no parseable JSON object; raw={:?}",
        truncate_for_err(raw)
    ))
}

fn extract_fenced_blocks(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut remainder = s;
    while let Some(open) = remainder.find("```") {
        let after_open = &remainder[open + 3..];
        let body_start = after_open.find('\n').map(|n| n + 1).unwrap_or(0);
        let body_region = &after_open[body_start..];
        if let Some(close) = body_region.find("```") {
            out.push(&body_region[..close]);
            let consumed = open + 3 + body_start + close + 3;
            if consumed >= remainder.len() {
                break;
            }
            remainder = &remainder[consumed..];
        } else {
            break;
        }
    }
    out
}

fn extract_balanced_objects(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut depth: i32 = 0;
            let mut in_str = false;
            let mut escape = false;
            let mut j = i;
            while j < bytes.len() {
                let c = bytes[j];
                if in_str {
                    if escape {
                        escape = false;
                    } else if c == b'\\' {
                        escape = true;
                    } else if c == b'"' {
                        in_str = false;
                    }
                } else if c == b'"' {
                    in_str = true;
                } else if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        spans.push(&s[i..=j]);
                        i = j;
                        break;
                    }
                }
                j += 1;
            }
        }
        i += 1;
    }
    spans
}

fn truncate_for_err(s: &str) -> String {
    const MAX: usize = 400;
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        prefix + "…"
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{SimardError, SimardResult};
    use crate::goal_curation::types::GoalProgress;

    struct StubSubmitter {
        response: SimardResult<String>,
    }

    impl LlmSubmitter for StubSubmitter {
        fn submit(&self, _rendered_prompt: &str) -> SimardResult<String> {
            match &self.response {
                Ok(s) => Ok(s.clone()),
                Err(e) => Err(SimardError::AdapterInvocationFailed {
                    base_type: ADAPTER_TAG.to_string(),
                    reason: format!("{e}"),
                }),
            }
        }
    }

    fn goal_with_activity(activity: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            id: "test-goal".to_string(),
            description: "do the thing".to_string(),
            priority: 1,
            status: GoalProgress::InProgress { percent: 10 },
            assigned_to: None,
            current_activity: activity.map(String::from),
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn downward_move_is_always_accepted_without_llm_call() {
        // Submitter that would panic if called — proves the early-return path.
        struct PanickingSubmitter;
        impl LlmSubmitter for PanickingSubmitter {
            fn submit(&self, _: &str) -> SimardResult<String> {
                panic!("LLM should not be called for downward moves");
            }
        }
        let c = LlmReviewerProgressChecker::new(PanickingSubmitter);
        let g = goal_with_activity(None);
        match c.check(&g, 80, 50, now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("downward"));
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn no_change_is_accepted_without_llm_call() {
        struct PanickingSubmitter;
        impl LlmSubmitter for PanickingSubmitter {
            fn submit(&self, _: &str) -> SimardResult<String> {
                panic!("LLM should not be called for no-change");
            }
        }
        let c = LlmReviewerProgressChecker::new(PanickingSubmitter);
        let g = goal_with_activity(None);
        assert!(matches!(
            c.check(&g, 60, 60, now()),
            EvidenceDecision::Accept { .. }
        ));
    }

    #[test]
    fn llm_accept_response_yields_accept_decision() {
        let stub = StubSubmitter {
            response: Ok(r#"{"verdict": "accept", "rationale": "matches plan"}"#.to_string()),
        };
        let c = LlmReviewerProgressChecker::new(stub);
        let g = goal_with_activity(Some("working on it"));
        match c.check(&g, 30, 45, now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("matches plan"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept"),
        }
    }

    #[test]
    fn llm_reject_response_yields_reject_decision() {
        let stub = StubSubmitter {
            response: Ok(r#"{"verdict": "reject", "rationale": "no plan, big jump"}"#.to_string()),
        };
        let c = LlmReviewerProgressChecker::new(stub);
        let g = goal_with_activity(None);
        match c.check(&g, 5, 90, now()) {
            EvidenceDecision::Reject { reason } => {
                assert!(reason.contains("no plan"), "got: {reason}");
            }
            EvidenceDecision::Accept { .. } => panic!("expected reject"),
        }
    }

    #[test]
    fn llm_submit_failure_falls_back_to_accept() {
        // Use a dedicated submitter that returns a real SimardError variant
        // (StubSubmitter takes Ok(String), not Err - keep tests honest).
        struct ErrSubmitter;
        impl LlmSubmitter for ErrSubmitter {
            fn submit(&self, _: &str) -> SimardResult<String> {
                Err(SimardError::AdapterInvocationFailed {
                    base_type: "x".into(),
                    reason: "transport timeout".into(),
                })
            }
        }
        let c = LlmReviewerProgressChecker::new(ErrSubmitter);
        let g = goal_with_activity(None);
        match c.check(&g, 10, 20, now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("LLM submit failed"), "got: {reason}");
                assert!(reason.contains("transport timeout"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on infra failure"),
        }
    }

    #[test]
    fn parse_error_falls_back_to_accept() {
        let stub = StubSubmitter {
            response: Ok("this is not json at all".to_string()),
        };
        let c = LlmReviewerProgressChecker::new(stub);
        let g = goal_with_activity(None);
        match c.check(&g, 10, 20, now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("parse error"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on parse failure"),
        }
    }

    #[test]
    fn unknown_verdict_falls_back_to_accept() {
        let stub = StubSubmitter {
            response: Ok(r#"{"verdict": "maybe", "rationale": "shrug"}"#.to_string()),
        };
        let c = LlmReviewerProgressChecker::new(stub);
        let g = goal_with_activity(None);
        match c.check(&g, 10, 20, now()) {
            EvidenceDecision::Accept { reason } => {
                assert!(reason.contains("unknown verdict"), "got: {reason}");
            }
            EvidenceDecision::Reject { .. } => panic!("expected accept on unknown verdict"),
        }
    }

    #[test]
    fn parser_handles_fenced_json() {
        let raw = "Here is my verdict:\n```json\n{\"verdict\":\"reject\",\"rationale\":\"hallucinated\"}\n```\nDone.";
        let parsed = parse_reviewer_response(raw).expect("parse ok");
        assert_eq!(parsed.verdict, "reject");
        assert_eq!(parsed.rationale, "hallucinated");
    }

    #[test]
    fn parser_handles_brace_balanced_inside_prose() {
        let raw = "Brain says: {\"verdict\":\"accept\",\"rationale\":\"ok\"} and that's that.";
        let parsed = parse_reviewer_response(raw).expect("parse ok");
        assert_eq!(parsed.verdict, "accept");
    }

    #[test]
    fn parser_rejects_empty_response() {
        assert!(parse_reviewer_response("").is_err());
        assert!(parse_reviewer_response("   ").is_err());
    }

    #[test]
    fn render_prompt_substitutes_all_placeholders() {
        let stub = StubSubmitter {
            response: Ok(r#"{"verdict":"accept","rationale":"ok"}"#.to_string()),
        };
        let c = LlmReviewerProgressChecker::new(stub);
        let mut g = goal_with_activity(Some("a plan that says things"));
        g.id = "my-goal-id".into();
        g.description = "a problem description".into();
        let rendered = c.render_prompt(&g, 25, 40);
        assert!(rendered.contains("my-goal-id"), "missing goal_id");
        assert!(
            rendered.contains("a problem description"),
            "missing problem"
        );
        assert!(rendered.contains("a plan that says things"), "missing plan");
        assert!(rendered.contains("25"), "missing prior_pct");
        assert!(rendered.contains("40"), "missing claimed_pct");
        // No literal placeholder leftovers
        assert!(
            !rendered.contains("{goal_id}"),
            "unsubstituted goal_id placeholder"
        );
        assert!(
            !rendered.contains("{problem}"),
            "unsubstituted problem placeholder"
        );
        assert!(
            !rendered.contains("{plan}"),
            "unsubstituted plan placeholder"
        );
        assert!(
            !rendered.contains("{prior_pct}"),
            "unsubstituted prior_pct placeholder"
        );
        assert!(
            !rendered.contains("{claimed_pct}"),
            "unsubstituted claimed_pct placeholder"
        );
        assert!(
            !rendered.contains("{wip_summary}"),
            "unsubstituted wip_summary placeholder"
        );
    }
}
