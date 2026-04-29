//! Deterministic fallback brain — preserves today's behaviour bit-for-bit
//! when no LLM is configured (no API key, subprocess unavailable, etc.).

use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::error::SimardResult;

/// Always returns `ContinueSkipping`. This is exactly what the unconditional
/// skip branch in `dispatch_spawn_engineer` did before issue #1266, so a
/// daemon falling back to this brain behaves identically to the pre-#1266
/// daemon: no panics, no escalation, no surprises.
#[derive(Debug, Default)]
pub struct DeterministicFallbackBrain;

impl OodaBrain for DeterministicFallbackBrain {
    fn decide_engineer_lifecycle(
        &self,
        _ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        Ok(EngineerLifecycleDecision::ContinueSkipping {
            rationale: "fallback-brain: rustyclawd unavailable".to_string(),
        })
    }
}
