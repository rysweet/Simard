//! LLM-backed brain — submits the rendered prompt via a `LlmSubmitter`
//! abstraction. Production wires the real RustyClawd session; tests wire a
//! canned-response stub.

use super::prompt_store;
use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::base_types::BaseTypeTurnInput;
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::{LlmProvider, SessionBuilder};

/// Embedded prompt — compile-time fallback. The runtime brain reads from
/// disk via [`prompt_store::global`] so prompt edits take effect on the
/// next OODA cycle without restarting the daemon (PR #1474 follow-up).
/// This constant is retained as documentation of the embedded baseline; the
/// authoritative copy lives in [`prompt_store::embedded_fallback`].
const PROMPT_NAME: &str = "ooda_brain.md";

const ADAPTER_TAG: &str = "ooda-brain";

/// Thin seam over whatever subprocess/HTTP path the rustyclawd adapter uses.
/// Production wires the real adapter via `RustyClawdSessionSubmitter`; tests
/// wire a canned-response stub without touching production wiring.
pub trait LlmSubmitter: Send + Sync {
    fn submit(&self, rendered_prompt: &str) -> SimardResult<String>;
}

/// LLM-backed brain. Construct via `build_rustyclawd_brain` in production so
/// callers do not need to know the adapter type. The submitter is generic so
/// tests can swap in a stub.
pub struct RustyClawdBrain<S: LlmSubmitter> {
    submitter: S,
}

impl<S: LlmSubmitter> RustyClawdBrain<S> {
    pub fn new(submitter: S) -> Self {
        Self { submitter }
    }

    /// Render the prompt with the context. Loads the prompt fresh per call
    /// via [`prompt_store::global`] so on-disk edits take effect on the
    /// next OODA cycle. Falls back to the embedded baseline when no file
    /// exists at the resolved path.
    pub fn render_prompt(&self, ctx: &EngineerLifecycleCtx) -> String {
        let sentinel = ctx
            .sentinel_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        prompt_store::global()
            .load(PROMPT_NAME)
            .replace("{goal_id}", &ctx.goal_id)
            .replace("{goal_description}", &ctx.goal_description)
            .replace("{cycle_number}", &ctx.cycle_number.to_string())
            .replace(
                "{consecutive_skip_count}",
                &ctx.consecutive_skip_count.to_string(),
            )
            .replace("{failure_count}", &ctx.failure_count.to_string())
            .replace("{worktree_path}", &ctx.worktree_path.display().to_string())
            .replace(
                "{worktree_mtime_secs_ago}",
                &ctx.worktree_mtime_secs_ago.to_string(),
            )
            .replace("{sentinel_pid}", &sentinel)
            .replace("{last_engineer_log_tail}", &ctx.last_engineer_log_tail)
    }
}

impl<S: LlmSubmitter> OodaBrain for RustyClawdBrain<S> {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        let prompt = self.render_prompt(ctx);
        let raw = self.submitter.submit(&prompt)?;
        parse_decision_from_response(&raw).map_err(|reason| SimardError::AdapterInvocationFailed {
            base_type: ADAPTER_TAG.to_string(),
            reason,
        })
    }
}

/// Extract a JSON object from the LLM response (LLMs sometimes wrap JSON in
/// prose / markdown fences) and parse it as a decision.
fn parse_decision_from_response(raw: &str) -> Result<EngineerLifecycleDecision, String> {
    // Strip markdown fences if present.
    let stripped = raw.trim();
    let candidate = if let Some(start) = stripped.find('{')
        && let Some(end) = stripped.rfind('}')
        && end >= start
    {
        &stripped[start..=end]
    } else {
        return Err(format!(
            "no JSON object found in LLM response (got {} bytes)",
            raw.len()
        ));
    };
    serde_json::from_str::<EngineerLifecycleDecision>(candidate)
        .map_err(|e| format!("brain-parse-error: {e}; payload={candidate}"))
}

// ---------------------------------------------------------------------------
// Production constructor
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Production submitter — opens a fresh BaseTypeSession per submit() call.
// ---------------------------------------------------------------------------

/// Production [`LlmSubmitter`]: opens a fresh [`BaseTypeSession`] via
/// [`SessionBuilder`] for each `submit()` call, runs one turn, and returns
/// the LLM response text (`outcome.execution_summary` per the
/// `BaseTypeSession` contract — see `engineer_plan::plan_objective` for the
/// canonical example).
///
/// **Why per-call session:** the engineer-lifecycle skip branch only fires
/// when an engineer is already alive (rare). Per-call session open mirrors
/// `engineer_plan` / `review_pipeline` and avoids threading
/// `Arc<Mutex<Box<dyn BaseTypeSession>>>` through `OodaBridges`. If profiling
/// later shows session-open cost dominating, swap to a cached session
/// without changing the `LlmSubmitter` trait.
///
/// **Resilience:** session-open or `run_turn` failures propagate as
/// `SimardError::AdapterInvocationFailed`. The `dispatch_spawn_engineer`
/// caller (see `ooda_actions/advance_goal/spawn.rs`) maps any brain error
/// to `ContinueSkipping`, preserving pre-#1266 behaviour on transient
/// adapter failure. We deliberately do **not** retry inside the submitter:
/// the OODA loop already iterates once per cycle, so a retry-on-failure
/// loop here would compound latency on every skipped cycle.
pub struct SessionLlmSubmitter {
    provider: LlmProvider,
}

impl SessionLlmSubmitter {
    pub fn new(provider: LlmProvider) -> Self {
        Self { provider }
    }
}

impl LlmSubmitter for SessionLlmSubmitter {
    fn submit(&self, rendered_prompt: &str) -> SimardResult<String> {
        let mut session = SessionBuilder::new(OperatingMode::Orchestrator, self.provider)
            .node_id("ooda-brain")
            .address("ooda-brain://local")
            .adapter_tag(ADAPTER_TAG)
            .open()
            .map_err(|reason| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("session open failed: {reason}"),
            })?;

        let outcome = session
            .run_turn(BaseTypeTurnInput::objective_only(
                rendered_prompt.to_string(),
            ))
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: ADAPTER_TAG.to_string(),
                reason: format!("run_turn failed: {e}"),
            });

        // Best-effort close — never mask the run_turn error if close also
        // fails. Closing on the error path mirrors `engineer_plan`.
        let _ = session.close();

        outcome.map(|o| o.execution_summary)
    }
}

/// Production constructor. Resolves the configured [`LlmProvider`] and
/// returns a [`RustyClawdBrain`] wired to a [`SessionLlmSubmitter`].
///
/// Returns `Err` if no LLM provider is configured (no env var, no
/// `~/.simard/config.toml`, no API credentials). Callers — currently
/// `operator_commands_ooda::daemon` — must fall back to
/// `DeterministicFallbackBrain` so the daemon behaves identically to the
/// pre-#1266 daemon when LLM access is unavailable.
///
/// Provider resolution does **not** open a session here: the
/// `SessionLlmSubmitter` opens one per `submit()` call (see its docstring
/// for rationale). This keeps daemon startup fast and avoids holding an
/// LLM connection open for the rare engineer-lifecycle decision path.
pub fn build_rustyclawd_brain() -> SimardResult<Box<dyn OodaBrain>> {
    let provider = LlmProvider::resolve()?;
    let submitter = SessionLlmSubmitter::new(provider);
    Ok(Box::new(RustyClawdBrain::new(submitter)))
}
