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

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the fallback brain
// that consumers depend on when the LLM bridge returns unparseable JSON or
// otherwise errors. Sibling tests cover the end-to-end behaviour; these pin
// the per-file public contract so coverage tools see #[test]s in this file.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_ctx() -> EngineerLifecycleCtx {
        EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "ship v1".into(),
            cycle_number: 7,
            consecutive_skip_count: 3,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 0,
            in_flight_engineer_count: 1,
            minutes_since_last_update_attempt: u64::MAX,
        }
    }

    #[test]
    fn fallback_always_returns_continue_skipping() {
        let brain = DeterministicFallbackBrain;
        let decision = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        match decision {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(
                    rationale.contains("fallback-brain"),
                    "rationale must mark itself as fallback for diagnostics, got: {rationale}"
                );
            }
            other => panic!("fallback must never escalate; got {other:?}"),
        }
    }

    #[test]
    fn fallback_is_deterministic_across_varied_contexts() {
        // Pin the documented contract: the fallback brain never panics and
        // never returns anything other than ContinueSkipping, regardless of
        // context (the consumer relies on this exact shape after a
        // JSON-parse failure in the LLM bridge).
        let brain = DeterministicFallbackBrain;
        let contexts = [
            EngineerLifecycleCtx {
                failure_count: 0,
                consecutive_skip_count: 0,
                ..sample_ctx()
            },
            EngineerLifecycleCtx {
                failure_count: 99,
                consecutive_skip_count: 99,
                ..sample_ctx()
            },
            EngineerLifecycleCtx {
                sentinel_pid: None,
                worktree_path: PathBuf::new(),
                ..sample_ctx()
            },
            EngineerLifecycleCtx {
                commits_behind: 10_000,
                ..sample_ctx()
            },
        ];
        for ctx in &contexts {
            let d = brain.decide_engineer_lifecycle(ctx).unwrap();
            assert!(
                matches!(d, EngineerLifecycleDecision::ContinueSkipping { .. }),
                "fallback must always emit ContinueSkipping, got {d:?} for ctx {ctx:?}"
            );
        }
    }

    #[test]
    fn fallback_rationale_is_stable_across_calls() {
        // Determinism guard: downstream judgment-record comparisons rely on
        // a stable rationale (no current time, no random data).
        let brain = DeterministicFallbackBrain;
        let a = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        let b = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        assert_eq!(a, b, "fallback brain must be deterministic");
    }

    #[test]
    fn fallback_returns_ok_never_err() {
        // The fallback brain is the safety floor: it must never surface an
        // Err that could bubble up and stall the OODA loop. This is the
        // entire reason it exists.
        let brain = DeterministicFallbackBrain;
        let r = brain.decide_engineer_lifecycle(&sample_ctx());
        assert!(r.is_ok(), "fallback brain must never return Err");
    }
}
