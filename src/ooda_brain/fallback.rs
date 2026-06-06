//! Deterministic lifecycle brain — preserves today's behaviour bit-for-bit
//! when no LLM is configured (no API key, subprocess unavailable, etc.).

use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::error::SimardResult;

/// Always returns `ContinueSkipping`. This is exactly what the unconditional
/// skip branch in `dispatch_spawn_engineer` did before issue #1266. When no
/// LLM brain is configured, the daemon uses this deterministic implementation.
#[derive(Debug, Default)]
pub struct DeterministicLifecycleBrain;

impl OodaBrain for DeterministicLifecycleBrain {
    fn decide_engineer_lifecycle(
        &self,
        _ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        Ok(EngineerLifecycleDecision::ContinueSkipping {
            rationale: "deterministic-brain: no LLM configured".to_string(),
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
        let brain = DeterministicLifecycleBrain;
        let decision = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        match decision {
            EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                assert!(
                    rationale.contains("deterministic-brain"),
                    "rationale must identify as deterministic brain, got: {rationale}"
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
        let brain = DeterministicLifecycleBrain;
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
        let brain = DeterministicLifecycleBrain;
        let a = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        let b = brain.decide_engineer_lifecycle(&sample_ctx()).unwrap();
        assert_eq!(a, b, "fallback brain must be deterministic");
    }

    #[test]
    fn fallback_returns_ok_never_err() {
        // The fallback brain is the safety floor: it must never surface an
        // Err that could bubble up and stall the OODA loop. This is the
        // entire reason it exists.
        let brain = DeterministicLifecycleBrain;
        let r = brain.decide_engineer_lifecycle(&sample_ctx());
        assert!(r.is_ok(), "fallback brain must never return Err");
    }
}
