//! Prompt-driven OODA brain for high-leverage decision sites (issue #1266).
//!
//! Establishes the pattern at the engineer-lifecycle skip branch in
//! `ooda_actions::advance_goal::spawn::dispatch_spawn_engineer`. Future PRs
//! migrate observe/orient/decide/curate/review to the same prompt-driven
//! shape (see PR description).
//!
//! Module split (per #1266 400-LOC cap):
//!   - `mod.rs`     — public surface: trait, types, re-exports, `apply_decision_to_state`.
//!   - `fallback.rs`— `DeterministicFallbackBrain` (preserves today's behavior).
//!   - `rustyclawd.rs` — `RustyClawdBrain` + `LlmSubmitter` + `build_rustyclawd_brain`.
//!   - `context.rs` — `gather_engineer_lifecycle_ctx` + `redact_secrets`.

use crate::error::SimardResult;
use crate::ooda_loop::OodaState;
use std::path::PathBuf;

mod context;
mod decide;
mod fallback;
mod judgment_record;
mod orient;
pub mod parse_failure;
pub mod prompt_store;
mod recipe_brain;
mod rustyclawd;
mod sanitize;

#[cfg(test)]
mod decide_tests;
#[cfg(test)]
mod orient_tests;
#[cfg(test)]
mod prompt_store_tests;
#[cfg(test)]
mod tests;

pub use context::{count_live_engineer_claims, gather_engineer_lifecycle_ctx, redact_secrets};
pub use decide::{
    DecideContext, DecideJudgment, DeterministicFallbackDecideBrain, OodaDecideBrain,
    PROMPT_NAME as DECIDE_PROMPT_NAME,
};
pub use fallback::DeterministicFallbackBrain;
pub use judgment_record::{
    BrainJudgmentRecord, BrainPhase, clear as clear_brain_judgments, push as push_brain_judgment,
    take_all as take_brain_judgments, with_cycle_scope as with_brain_judgment_scope,
};
pub use orient::{
    DeterministicFallbackOrientBrain, FAILURE_PENALTY_PER_CONSECUTIVE, OodaOrientBrain,
    OrientContext, OrientJudgment, PROMPT_NAME as ORIENT_PROMPT_NAME, RustyClawdOrientBrain,
    build_rustyclawd_orient_brain,
};
pub use parse_failure::ParseFailureRecord;
pub use recipe_brain::RecipeBrain;
/// Backward-compatible type aliases (issue #2132).
pub type RecipeDecideBrain = RecipeBrain;
pub type RecipeEngineerLifecycleBrain = RecipeBrain;
pub type RecipeOrientBrain = RecipeBrain;
pub use rustyclawd::{
    LlmSubmitter, PROMPT_NAME as ACT_PROMPT_NAME, RustyClawdBrain, SessionLlmSubmitter,
    build_rustyclawd_brain,
};

// ---------------------------------------------------------------------------
// Context fed to the brain
// ---------------------------------------------------------------------------

/// All read-only context the brain needs to decide what to do about a goal
/// that already has a live engineer worktree. Best-effort: any field may be
/// defaulted if the underlying source is missing — the brain reasons about
/// partial context.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EngineerLifecycleCtx {
    pub goal_id: String,
    pub goal_description: String,
    pub cycle_number: u32,
    pub consecutive_skip_count: u32,
    pub failure_count: u32,
    pub worktree_path: PathBuf,
    pub worktree_mtime_secs_ago: u64,
    pub sentinel_pid: Option<i32>,
    pub last_engineer_log_tail: String,
    /// How many commits the running binary's embedded git SHA is behind
    /// `origin/main` HEAD (best-effort `git rev-list` count). 0 if equal,
    /// missing, or unparseable. Used by the `consider_self_update` doctrine.
    #[serde(default)]
    pub commits_behind: u32,
    /// How many engineer worktrees currently have a live `.simard-engineer-claim`
    /// heartbeat (alive sentinel pid). Includes the worktree under inspection.
    /// `consider_self_update` is unsafe to act on while this is > 1 (or > 0
    /// from a non-engineer-lifecycle site) because the safe-update drain phase
    /// would block on the in-flight engineer.
    #[serde(default)]
    pub in_flight_engineer_count: u32,
    /// Minutes since the last safe-update attempt (success or failure).
    /// `u64::MAX` means "never attempted on this host". Compared against
    /// `safe_update::UpdateConfig::min_minutes_since_last_attempt` (default 30).
    #[serde(default = "default_minutes_since_last_update")]
    pub minutes_since_last_update_attempt: u64,
}

fn default_minutes_since_last_update() -> u64 {
    u64::MAX
}

// ---------------------------------------------------------------------------
// Decision: tagged enum the LLM emits as JSON `{"choice":"...","rationale":"..."}`
// ---------------------------------------------------------------------------

/// What the brain decided to do. Matches the JSON schema in
/// `prompt_assets/simard/ooda_brain.md`. Tagged on `choice` for
/// forward-compatibility (unknown tags fail to parse → caller falls back to
/// `ContinueSkipping`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "choice", rename_all = "snake_case")]
pub enum EngineerLifecycleDecision {
    /// Engineer is healthy / making progress. No-op this cycle.
    ContinueSkipping { rationale: String },
    /// Worktree is wedged. Tear it down and respawn with extra context.
    ReclaimAndRedispatch {
        rationale: String,
        #[serde(default)]
        redispatch_context: String,
    },
    /// Goal is consuming budget without progress. Bump failure count so the
    /// existing FAILURE_PENALTY in `orient.rs` demotes it next cycle.
    Deprioritize { rationale: String },
    /// Worth a human eyeball. Queue a tracking issue.
    OpenTrackingIssue {
        rationale: String,
        title: String,
        body: String,
    },
    /// Cannot proceed without external input. Mark goal blocked.
    MarkGoalBlocked { rationale: String, reason: String },
    /// The running binary is meaningfully behind `origin/main` and conditions
    /// look right for a safe-update. The brain only emits this after weighing
    /// the four-part doctrine documented in `prompt_assets/simard/ooda_brain.md`:
    ///
    /// 1. `commits_behind >= UpdateConfig::min_commits_since_build` (default 3)
    /// 2. `in_flight_engineer_count == 0` (or ≤1 from this site, since we
    ///    are inspecting one engineer when this brain runs)
    /// 3. `minutes_since_last_update_attempt >= min_minutes_since_last_attempt`
    ///    (default 30 — backoff to avoid thrash)
    /// 4. The current goal's engineer is healthy enough to be safely paused
    ///
    /// The act-phase dispatcher re-validates the safety predicate before
    /// invoking `simard safe-update`; if it cannot run safely the choice is
    /// recorded as deferred (success-equivalent, no state mutation).
    ConsiderSelfUpdate { rationale: String },
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Single-decision-site trait. Sync on purpose: the act-phase dispatcher is
/// sync, and the LLM-backed impl bridges to async internally so callers do
/// not see a runtime requirement.
pub trait OodaBrain: Send + Sync {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision>;
}

// ---------------------------------------------------------------------------
// Pure side-effect application (state mutation only — no IO)
// ---------------------------------------------------------------------------

/// Apply a brain decision to OODA state and return the human-readable detail
/// string the caller should attach to the resulting `ActionOutcome`.
///
/// Pure-state: does NOT kill processes, remove worktrees, or shell out to
/// `gh`. Those side effects live in `ooda_actions::advance_goal::spawn` so
/// this helper stays unit-testable without process spawning.
pub fn apply_decision_to_state(
    decision: &EngineerLifecycleDecision,
    state: &mut OodaState,
    goal_id: &str,
) -> String {
    match decision {
        EngineerLifecycleDecision::ContinueSkipping { rationale } => {
            format!("brain: continue_skipping ({rationale})")
        }
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale,
            redispatch_context,
        } => {
            // Clear the in-state assignment so the next cycle re-spawns. The
            // caller still needs to perform the kill / `git worktree remove`
            // IO outside this pure helper.
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.assigned_to = None;
            }
            state.engineer_worktrees.remove(goal_id);
            if redispatch_context.is_empty() {
                format!("brain: reclaim_and_redispatch ({rationale})")
            } else {
                format!(
                    "brain: reclaim_and_redispatch ({rationale}); redispatch_context={redispatch_context}"
                )
            }
        }
        EngineerLifecycleDecision::Deprioritize { rationale } => {
            // Bump the failure counter ourselves so even though the cycle
            // post-processor will see success=false and increment again, we
            // still get a visible bump on this very cycle (defends against
            // future refactors of cycle.rs that might not auto-increment).
            let entry = state
                .goal_failure_counts
                .entry(goal_id.to_string())
                .or_insert(0);
            *entry = entry.saturating_add(1);
            format!("brain: deprioritized ({rationale})")
        }
        EngineerLifecycleDecision::OpenTrackingIssue {
            rationale, title, ..
        } => {
            // The actual `gh issue create` shell-out happens in spawn.rs;
            // here we just return the descriptive detail string.
            format!("brain: open_tracking_issue title='{title}' ({rationale})")
        }
        EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason } => {
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = crate::goal_curation::GoalProgress::Blocked(reason.clone());
            }
            format!("brain: mark_goal_blocked ({rationale}); reason={reason}")
        }
        EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
            // Pure-state helper: the brain has emitted the choice but the
            // act-phase dispatcher decides whether to actually invoke
            // `simard safe-update` based on the live in-flight predicate.
            // We do NOT mutate state here — the failure-counter / blocked
            // status logic is irrelevant to a self-update decision.
            format!("brain: consider_self_update ({rationale})")
        }
    }
}

// ---------------------------------------------------------------------------
// Inline tests (issue #1979 — per-source-file coverage of the public surface
// declared here. The sibling `tests.rs` file covers brain dispatch end-to-end;
// these inline tests pin the contracts declared in *this* file so coverage
// tools see #[test]s alongside the public surface, including the
// `BrainPhase` serde round-trip that the `parse_failure::counters()` map
// uses as a HashMap key.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod inline_tests_1979 {
    use super::*;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::ooda_brain::BrainPhase;
    use crate::ooda_loop::OodaState;

    fn state_with_active_goal(id: &str) -> OodaState {
        let mut board = GoalBoard::default();
        board.active.push(ActiveGoal {
            id: id.to_string(),
            description: "test".to_string(),
            priority: 1,
            status: GoalProgress::NotStarted,
            assigned_to: Some("engineer-a".to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
        OodaState::new(board)
    }

    // ----- BrainPhase serde round-trip -----------------------------------
    // The map `parse_failure::counters(): (BrainPhase, goal_id) -> u32` keys
    // on `BrainPhase`. An incorrect Hash/Eq/serde impl silently re-buckets
    // failures across phases — these tests pin the round-trip so an
    // accidental rename or representation change is caught early.

    #[test]
    fn brain_phase_serializes_as_lowercase() {
        assert_eq!(serde_json::to_string(&BrainPhase::Act).unwrap(), "\"act\"");
        assert_eq!(
            serde_json::to_string(&BrainPhase::Decide).unwrap(),
            "\"decide\""
        );
        assert_eq!(
            serde_json::to_string(&BrainPhase::Orient).unwrap(),
            "\"orient\""
        );
    }

    #[test]
    fn brain_phase_round_trips_through_json() {
        for &phase in &[BrainPhase::Act, BrainPhase::Decide, BrainPhase::Orient] {
            let s = serde_json::to_string(&phase).unwrap();
            let back: BrainPhase = serde_json::from_str(&s).unwrap();
            assert_eq!(phase, back);
        }
    }

    #[test]
    fn brain_phase_distinct_variants_are_not_equal() {
        // Guard against a future refactor that accidentally collapses
        // variants — counters() would re-bucket all phases together.
        assert_ne!(BrainPhase::Act, BrainPhase::Decide);
        assert_ne!(BrainPhase::Decide, BrainPhase::Orient);
        assert_ne!(BrainPhase::Act, BrainPhase::Orient);
    }

    // ----- apply_decision_to_state — branches sibling tests do not pin --

    #[test]
    fn apply_decision_continue_skipping_does_not_mutate_state() {
        let mut state = state_with_active_goal("g1");
        let before_assigned = state.active_goals.active[0].assigned_to.clone();
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ContinueSkipping {
                rationale: "hb ok".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("continue_skipping"));
        assert!(detail.contains("hb ok"));
        assert_eq!(state.active_goals.active[0].assigned_to, before_assigned);
    }

    #[test]
    fn apply_decision_reclaim_clears_assignment_and_worktree() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale: "wedged 7h".into(),
                redispatch_context: "retry with extra ctx".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("reclaim_and_redispatch"));
        assert!(detail.contains("retry with extra ctx"));
        assert!(state.active_goals.active[0].assigned_to.is_none());
        // worktree map remove is best-effort even when entry is absent.
        assert!(!state.engineer_worktrees.contains_key("g1"));
    }

    #[test]
    fn apply_decision_reclaim_omits_context_marker_when_empty() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::ReclaimAndRedispatch {
                rationale: "wedged".into(),
                redispatch_context: String::new(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("reclaim_and_redispatch"));
        assert!(
            !detail.contains("redispatch_context="),
            "empty redispatch_context must NOT be appended; got: {detail}"
        );
    }

    #[test]
    fn apply_decision_deprioritize_bumps_failure_counter() {
        let mut state = state_with_active_goal("g1");
        let before = state.goal_failure_counts.get("g1").copied().unwrap_or(0);
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::Deprioritize {
                rationale: "chronic".into(),
            },
            &mut state,
            "g1",
        );
        let after = state.goal_failure_counts.get("g1").copied().unwrap_or(0);
        assert_eq!(after, before + 1, "deprioritize must bump failure counter");
        assert!(detail.contains("deprioritized"));
    }

    #[test]
    fn apply_decision_mark_blocked_sets_goal_status() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::MarkGoalBlocked {
                rationale: "human input".into(),
                reason: "needs API key".into(),
            },
            &mut state,
            "g1",
        );
        match &state.active_goals.active[0].status {
            GoalProgress::Blocked(r) => assert_eq!(r, "needs API key"),
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(detail.contains("mark_goal_blocked"));
        assert!(detail.contains("needs API key"));
    }

    #[test]
    fn apply_decision_open_tracking_issue_returns_descriptive_detail() {
        let mut state = state_with_active_goal("g1");
        let detail = apply_decision_to_state(
            &EngineerLifecycleDecision::OpenTrackingIssue {
                rationale: "panic seen".into(),
                title: "engineer panicked".into(),
                body: "see logs".into(),
            },
            &mut state,
            "g1",
        );
        assert!(detail.contains("open_tracking_issue"));
        assert!(detail.contains("engineer panicked"));
        // No state mutation — the actual `gh issue create` lives elsewhere.
        assert!(state.active_goals.active[0].assigned_to.is_some());
    }

    // ----- EngineerLifecycleCtx default minutes_since_last_update --------
    #[test]
    fn lifecycle_ctx_serde_default_minutes_since_last_update_is_max() {
        // When the field is absent from incoming JSON (e.g. older cycle
        // reports), the serde default must be u64::MAX so safe-update's
        // min-gap predicate does not immediately permit an update.
        let json = r#"{
            "goal_id":"g","goal_description":"","cycle_number":0,
            "consecutive_skip_count":0,"failure_count":0,
            "worktree_path":"/tmp","worktree_mtime_secs_ago":0,
            "sentinel_pid":null,"last_engineer_log_tail":""
        }"#;
        let ctx: EngineerLifecycleCtx = serde_json::from_str(json).unwrap();
        assert_eq!(
            ctx.minutes_since_last_update_attempt,
            u64::MAX,
            "missing field must default to 'never attempted' (u64::MAX), not 0"
        );
        // commits_behind / in_flight_engineer_count use plain #[serde(default)]
        // (the type's Default → 0). Pinning so a future serde rename catches.
        assert_eq!(ctx.commits_behind, 0);
        assert_eq!(ctx.in_flight_engineer_count, 0);
    }

    #[test]
    fn lifecycle_ctx_serde_round_trip_preserves_all_fields() {
        let ctx = EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "ship".into(),
            cycle_number: 12,
            consecutive_skip_count: 3,
            failure_count: 1,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 100,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 4,
            in_flight_engineer_count: 2,
            minutes_since_last_update_attempt: 30,
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: EngineerLifecycleCtx = serde_json::from_str(&json).unwrap();
        assert_eq!(back.goal_id, ctx.goal_id);
        assert_eq!(back.cycle_number, ctx.cycle_number);
        assert_eq!(back.sentinel_pid, ctx.sentinel_pid);
        assert_eq!(back.commits_behind, ctx.commits_behind);
        assert_eq!(
            back.minutes_since_last_update_attempt,
            ctx.minutes_since_last_update_attempt
        );
    }
}
