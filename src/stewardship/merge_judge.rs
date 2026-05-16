//! Agentic merge-readiness judge.
//!
//! Replaces the previous hardcoded heading/byte/bracket gate in
//! [`super::merge_authority`]. The deterministic gate now only owns objective
//! checks (mergeable, CI green, base-branch allow-list, repo allow-list); this
//! module owns the **judgment** half — "does the PR body satisfy the
//! merge-ready skill?" — by delegating to a prompt-driven agent.
//!
//! ## Why agentic
//!
//! The previous gate encoded an array of literal heading strings, a byte-count
//! threshold, and a `<...>` placeholder heuristic. All three were brittle: a
//! legitimate `<placeholder>` inside real prose poisoned the bracket check; a
//! rename in the skill template drifted the heading set; substantive evidence
//! shorter than the byte threshold was rejected mechanically. The criteria
//! the gate enforces are inherently a **judgment call** that belongs in a
//! prompt at `~/.copilot/skills/merge-ready/SKILL.md`, not in Rust constants.
//!
//! ## Architecture
//!
//! - [`MergeJudge`] — the trait, one synchronous method per PR.
//! - [`LlmMergeJudge`] — production impl. Renders
//!   `prompt_assets/simard/merge_readiness_judge.md`, submits via the same
//!   [`LlmSubmitter`] seam the OODA brains use, parses the JSON verdict.
//! - [`RefusingMergeJudge`] — fallback when no LLM is configured. Always
//!   refuses with a "judge unavailable" reason; never re-implements the old
//!   heuristic gate. This is intentional: we never want a silent fall-back to
//!   brittle string matching.
//! - [`build_merge_judge`] — production constructor; resolves an LLM provider
//!   or returns the refusing fallback.
//!
//! The judge never decides whether to merge — it only decides whether the
//! evidence satisfies the skill. The deterministic gate in
//! [`super::merge_authority`] still owns the actual `gh pr merge` call and the
//! objective preconditions.

use crate::error::{SimardError, SimardResult};
use crate::ooda_brain::prompt_store;
use crate::ooda_brain::{LlmSubmitter, SessionLlmSubmitter};
use crate::session_builder::LlmProvider;

use super::merge_authority::PrSnapshot;

const ADAPTER_TAG: &str = "merge-readiness-judge";
pub const PROMPT_NAME: &str = "merge_readiness_judge.md";

/// Truncate `s` to 2 KiB for inclusion in error messages / logs. Same shape
/// as `ooda_brain::rustyclawd::truncate_for_log`, duplicated here so this
/// module does not depend on a private helper across module boundaries.
fn truncate_for_log(s: &str) -> String {
    const MAX_LEN: usize = 2048;
    if s.len() <= MAX_LEN {
        s.to_string()
    } else {
        // Walk back to a char boundary so we never split a UTF-8 codepoint.
        let mut end = MAX_LEN;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…[truncated {} bytes]", &s[..end], s.len() - end)
    }
}

/// Verdict tags emitted by the judge prompt. Lower-snake-case matches the
/// JSON contract documented in `prompt_assets/simard/merge_readiness_judge.md`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every skill criterion is present and substantive.
    Ready,
    /// At least one criterion is missing or thin. See `blockers`.
    NotReady,
    /// The judge could not form a verdict — e.g. PR body truncated. Treated
    /// the same as `NotReady` at the call site so the merge does not proceed.
    Unclear,
}

/// One actionable issue the judge identified.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Blocker {
    pub section: String,
    pub severity: String,
    pub observation: String,
    pub fix: String,
}

/// Structured verdict the judge returns. `blockers` is `Some` when verdict is
/// `NotReady`; `None` (or empty) otherwise.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JudgeOutcome {
    pub verdict: Verdict,
    pub rationale: String,
    #[serde(default)]
    pub blockers: Vec<Blocker>,
}

impl JudgeOutcome {
    /// Convenience: one-line human-readable summary for log/refusal messages.
    pub fn summary(&self) -> String {
        match self.verdict {
            Verdict::Ready => format!("merge-readiness-judge: ready — {}", self.rationale),
            Verdict::NotReady => {
                let mut parts = Vec::new();
                for b in &self.blockers {
                    parts.push(format!("{} ({}): {}", b.section, b.severity, b.observation));
                }
                if parts.is_empty() {
                    format!("merge-readiness-judge: not_ready — {}", self.rationale)
                } else {
                    format!(
                        "merge-readiness-judge: not_ready — {}; blockers: {}",
                        self.rationale,
                        parts.join("; ")
                    )
                }
            }
            Verdict::Unclear => format!("merge-readiness-judge: unclear — {}", self.rationale),
        }
    }
}

/// Trait every merge judge implements. Synchronous on purpose to match the
/// OODA brain pattern — the LLM-backed impl bridges to async internally.
pub trait MergeJudge: Send + Sync {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome>;
}

// ─────────────────────────── Refusing fallback ──────────────────────────────

/// Fallback judge used when no LLM provider is configured. Always returns a
/// `NotReady` verdict so callers refuse the merge with an actionable reason.
///
/// This is intentionally **not** a re-implementation of the old hardcoded
/// heuristic — the whole point of the refactor is that brittle string
/// matching never runs again. If the daemon cannot reach an LLM, merges
/// require operator intervention.
pub struct RefusingMergeJudge;

impl MergeJudge for RefusingMergeJudge {
    fn judge(&self, _pr: u32, _repo: &str, _snapshot: &PrSnapshot) -> SimardResult<JudgeOutcome> {
        Ok(JudgeOutcome {
            verdict: Verdict::NotReady,
            rationale: "merge-readiness-judge is not configured (no LLM provider). \
                Configure SIMARD_LLM_PROVIDER (or ~/.simard/config.toml) and retry, \
                or merge manually after a human review."
                .to_string(),
            blockers: vec![Blocker {
                section: "judge-availability".to_string(),
                severity: "high".to_string(),
                observation: "No LLM provider is configured for the merge-readiness judge."
                    .to_string(),
                fix: "Set up an LLM provider for Simard, or perform a manual merge-ready review."
                    .to_string(),
            }],
        })
    }
}

// ─────────────────────────── LLM-backed judge ───────────────────────────────

/// LLM-backed merge judge. Production wires it via [`build_merge_judge`].
pub struct LlmMergeJudge<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> LlmMergeJudge<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    fn render_prompt(&self, pr_number: u32, repo: &str, snapshot: &PrSnapshot) -> String {
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{pr_number}", &pr_number.to_string())
            .replace("{repo}", repo)
            .replace("{pr_body}", &snapshot.body)
    }
}

impl<S: LlmSubmitter> MergeJudge for LlmMergeJudge<S> {
    fn judge(
        &self,
        pr_number: u32,
        repo: &str,
        snapshot: &PrSnapshot,
    ) -> SimardResult<JudgeOutcome> {
        let prompt = self.render_prompt(pr_number, repo, snapshot);
        let raw = self.submitter.submit(&prompt)?;
        parse_judge_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }
}

/// Extract a JSON object from the LLM response (LLMs sometimes wrap it in
/// prose or markdown fences) and parse it as a [`JudgeOutcome`]. On failure
/// the error embeds the truncated raw response so operators can diagnose.
pub fn parse_judge_response(raw: &str) -> Result<JudgeOutcome, String> {
    let stripped = raw.trim();
    if stripped.is_empty() {
        return Err(format!(
            "merge-readiness-judge returned an empty response (raw_response={:?})",
            raw
        ));
    }
    // Same strategy the decide brain uses: find the outermost {...} span.
    let candidate = match (stripped.find('{'), stripped.rfind('}')) {
        (Some(start), Some(end)) if end >= start => &stripped[start..=end],
        _ => {
            return Err(format!(
                "merge-readiness-judge response had no JSON object; raw_response={:?}",
                truncate_for_log(raw)
            ));
        }
    };
    serde_json::from_str::<JudgeOutcome>(candidate).map_err(|e| {
        format!(
            "merge-readiness-judge-parse-error: {e}; payload={candidate}; raw_response={:?}",
            truncate_for_log(raw)
        )
    })
}

// ─────────────────────────── Production constructor ─────────────────────────

/// Build the production merge judge. Resolves an LLM provider via the same
/// `LlmProvider::resolve` path the OODA brains use. Falls back to
/// [`RefusingMergeJudge`] when no provider is configured.
pub fn build_merge_judge() -> Box<dyn MergeJudge> {
    match LlmProvider::resolve() {
        Ok(provider) => {
            let submitter = SessionLlmSubmitter::new(provider);
            Box::new(LlmMergeJudge::new(submitter))
        }
        Err(_) => Box::new(RefusingMergeJudge),
    }
}

// ─────────────────────────── Tests ──────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> PrSnapshot {
        PrSnapshot {
            body: "some body".into(),
            mergeable: "MERGEABLE".into(),
            review_decision: "APPROVED".into(),
            checks: vec![],
            base_ref_name: "main".into(),
        }
    }

    #[test]
    fn refusing_judge_returns_not_ready_with_actionable_blocker() {
        let j = RefusingMergeJudge;
        let out = j.judge(1, "rysweet/Simard", &snap()).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
        assert!(out.rationale.contains("not configured"));
        assert_eq!(out.blockers.len(), 1);
        assert_eq!(out.blockers[0].section, "judge-availability");
        assert_eq!(out.blockers[0].severity, "high");
    }

    #[test]
    fn parse_response_accepts_bare_json() {
        let raw = r#"{"verdict":"ready","rationale":"all six sections substantive"}"#;
        let out = parse_judge_response(raw).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
        assert_eq!(out.rationale, "all six sections substantive");
        assert!(out.blockers.is_empty());
    }

    #[test]
    fn parse_response_extracts_json_from_prose() {
        let raw = "Here is my verdict:\n\n```\n{\"verdict\":\"not_ready\",\"rationale\":\"Quality-audit is one line\",\"blockers\":[{\"section\":\"Quality-audit\",\"severity\":\"high\",\"observation\":\"thin\",\"fix\":\"add cycles\"}]}\n```\n\nLet me know.";
        let out = parse_judge_response(raw).unwrap();
        assert_eq!(out.verdict, Verdict::NotReady);
        assert_eq!(out.blockers.len(), 1);
        assert_eq!(out.blockers[0].section, "Quality-audit");
    }

    #[test]
    fn parse_response_unclear_verdict() {
        let raw = r#"{"verdict":"unclear","rationale":"body looked truncated"}"#;
        let out = parse_judge_response(raw).unwrap();
        assert_eq!(out.verdict, Verdict::Unclear);
    }

    #[test]
    fn parse_response_rejects_empty() {
        let err = parse_judge_response("").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn parse_response_rejects_no_json() {
        let err = parse_judge_response("I don't have a JSON for you today.").unwrap_err();
        assert!(err.contains("no JSON object"));
    }

    #[test]
    fn parse_response_rejects_malformed_json() {
        // Has both braces so the outer span is non-empty, but the contents
        // are not valid JSON — must hit the parse-error path, not the
        // "no JSON object" path.
        let err = parse_judge_response("{ verdict: ready, this is not real json }").unwrap_err();
        assert!(err.contains("parse-error"), "got: {err}");
    }

    #[test]
    fn summary_ready_includes_rationale() {
        let out = JudgeOutcome {
            verdict: Verdict::Ready,
            rationale: "all six substantive".into(),
            blockers: vec![],
        };
        let s = out.summary();
        assert!(s.contains("ready"));
        assert!(s.contains("all six substantive"));
    }

    #[test]
    fn summary_not_ready_enumerates_blockers() {
        let out = JudgeOutcome {
            verdict: Verdict::NotReady,
            rationale: "two sections thin".into(),
            blockers: vec![
                Blocker {
                    section: "Quality-audit".into(),
                    severity: "high".into(),
                    observation: "thin".into(),
                    fix: "add cycles".into(),
                },
                Blocker {
                    section: "CI".into(),
                    severity: "medium".into(),
                    observation: "no link".into(),
                    fix: "add URL".into(),
                },
            ],
        };
        let s = out.summary();
        assert!(s.contains("not_ready"));
        assert!(s.contains("Quality-audit (high): thin"));
        assert!(s.contains("CI (medium): no link"));
    }

    /// Smoke test for the LLM-backed path using a stub submitter, no actual
    /// LLM dependency. Mirrors the OODA brain test pattern.
    struct StubSubmitter {
        canned: String,
    }
    impl LlmSubmitter for StubSubmitter {
        fn submit(&self, _prompt: &str) -> SimardResult<String> {
            Ok(self.canned.clone())
        }
    }

    #[test]
    fn llm_judge_round_trips_a_ready_verdict() {
        let stub = StubSubmitter {
            canned: r#"{"verdict":"ready","rationale":"all six substantive"}"#.into(),
        };
        let judge = LlmMergeJudge::new(stub);
        let out = judge.judge(1500, "rysweet/Simard", &snap()).unwrap();
        assert_eq!(out.verdict, Verdict::Ready);
    }

    #[test]
    fn llm_judge_propagates_parse_failures_as_adapter_errors() {
        let stub = StubSubmitter {
            canned: "model refused to respond".into(),
        };
        let judge = LlmMergeJudge::new(stub);
        let err = judge.judge(1500, "rysweet/Simard", &snap()).unwrap_err();
        match err {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, "merge-readiness-judge");
                assert!(reason.contains("no JSON object"));
            }
            other => panic!("expected AdapterInvocationFailed, got {other:?}"),
        }
    }
}
