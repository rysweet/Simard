//! Concurrent dispatch of spawn-path `AdvanceGoal` actions.
//!
//! The Act phase plans up to `cap = scaler.current_max()` `AdvanceGoal`
//! actions per OODA round (one per uncovered incomplete goal). Before this
//! module, every `AdvanceGoal` dispatch serialized on a single global
//! `bridges`+`state` `Mutex` held across the slow goal-action LLM `run_turn`
//! (~30-90s) — so only ~1 engineer started per round even when the plan was
//! parallel.
//!
//! [`dispatch_advance_concurrent`] runs the spawn-path `AdvanceGoal` actions
//! (goals with no live subordinate) concurrently:
//!
//! * Each goal gets its OWN LLM session (via
//!   [`crate::ooda_loop::OrchestratorSessionFactory`]) so the slow `run_turn`
//!   calls run in parallel instead of fighting over one shared session.
//! * The global `state` lock is taken only for SHORT critical sections (read
//!   and atomically claim the goal, then later apply the parsed decision and
//!   record the assignment). It is NEVER held across `run_turn` or the engineer
//!   spawn (git worktree allocation plus detached subprocess).
//! * A per-round claim set guarantees a goal is claimed by exactly one thread
//!   (no double-spawn), and a counting [`Semaphore`] bounds concurrent starts
//!   to the AIMD `cap` (resource-aware, never exceeded).
//!
//! Goals that already have a subordinate (the heartbeat path) and all
//! non-`AdvanceGoal` actions are dispatched by the serialized phase in
//! [`super::dispatch_actions_bounded`]; their behavior is unchanged.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex};

use crate::base_types::BaseTypeSession;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::goal_curation::GoalProgress;
use crate::goal_curation::progress_evidence::ProgressEvidenceChecker;
use crate::ooda_brain::OodaBrain;
use crate::ooda_loop::{
    ActionOutcome, OodaClients, OodaState, OrchestratorSessionFactory, PlannedAction,
};

use super::advance_goal::spawn::{dispatch_spawn_engineer, is_brain_failure_marker, lock_state};
use super::goal_session::{GoalAction, apply_goal_advance_result, build_goal_advance_input};
use super::make_outcome;

/// A simple counting semaphore (std has none) used to cap the number of
/// `AdvanceGoal` dispatches running concurrently to the AIMD `cap`.
///
/// Permits are released on [`Drop`] of the [`SemaphorePermit`], so a panicking
/// dispatch thread still returns its permit and never wedges the round.
pub(super) struct Semaphore {
    permits: Mutex<usize>,
    available: Condvar,
}

impl Semaphore {
    pub(super) fn new(permits: usize) -> Self {
        Self {
            permits: Mutex::new(permits),
            available: Condvar::new(),
        }
    }

    /// Block until a permit is available, then take it. The returned guard
    /// releases the permit when dropped.
    pub(super) fn acquire(&self) -> SemaphorePermit<'_> {
        let mut n = self.permits.lock().unwrap_or_else(|p| p.into_inner());
        while *n == 0 {
            n = self.available.wait(n).unwrap_or_else(|p| p.into_inner());
        }
        *n -= 1;
        SemaphorePermit { sem: self }
    }
}

/// RAII permit; releases its slot back to the [`Semaphore`] on drop.
pub(super) struct SemaphorePermit<'a> {
    sem: &'a Semaphore,
}

impl Drop for SemaphorePermit<'_> {
    fn drop(&mut self) {
        let mut n = self.sem.permits.lock().unwrap_or_else(|p| p.into_inner());
        *n += 1;
        self.sem.available.notify_one();
    }
}

/// Atomically claim `goal_id` for this round. Returns `true` if this caller
/// won the claim (the goal was not already claimed), `false` if another
/// concurrent dispatch already claimed it. The intra-round guard against
/// double-spawning the same goal.
pub(super) fn try_claim(claims: &Mutex<HashSet<String>>, goal_id: &str) -> bool {
    let mut set = claims.lock().unwrap_or_else(|p| p.into_inner());
    set.insert(goal_id.to_string())
}

/// Shareable, `Sync` context handed to each concurrent dispatch thread.
struct AdvanceCtx<'a> {
    memory: &'a dyn CognitiveMemoryOps,
    checker: &'a dyn ProgressEvidenceChecker,
    brain: &'a dyn OodaBrain,
    /// Mints a fresh per-goal session so `run_turn` calls run concurrently.
    session_factory: Option<&'a dyn OrchestratorSessionFactory>,
    /// Fallback single session used (under lock, serialized) only when no
    /// `session_factory` is configured — preserves behavior for tests and
    /// non-daemon callers.
    shared_session: &'a Mutex<Option<Box<dyn BaseTypeSession>>>,
    state: &'a Mutex<&'a mut OodaState>,
    claims: &'a Mutex<HashSet<String>>,
    sem: &'a Semaphore,
}

/// Dispatch the spawn-path `AdvanceGoal` actions at `indices` concurrently,
/// writing each outcome into `results[idx]`.
///
/// `max_concurrency` is the AIMD cap (`scaler.current_max()`); at most that
/// many `AdvanceGoal` dispatches run at once. Outcomes are written by original
/// index so the caller can preserve input order.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_advance_concurrent(
    actions: &[PlannedAction],
    indices: &[usize],
    bridges: &mut OodaClients,
    state: &mut OodaState,
    max_concurrency: usize,
    results: &mut [Option<ActionOutcome>],
) {
    // Decompose the `Send + Sync` bridge pieces (everything AdvanceGoal needs
    // except the single non-`Sync` session). Disjoint field borrows.
    let memory: &dyn CognitiveMemoryOps = &*bridges.memory;
    let checker: &dyn ProgressEvidenceChecker = &*bridges.progress_evidence;
    let brain: &dyn OodaBrain = &*bridges.brain;
    let session_factory: Option<&dyn OrchestratorSessionFactory> =
        bridges.session_factory.as_deref();

    // Take ownership of the shared fallback session for the duration of the
    // round so the per-thread context can lend it out by `Box` (avoids tying a
    // `&mut` into the ctx struct). Restored afterwards so daemon shutdown can
    // still close it.
    let shared_session = Mutex::new(bridges.session.take());

    let state_mx = Mutex::new(&mut *state);
    let claims = Mutex::new(HashSet::new());
    let sem = Semaphore::new(max_concurrency.max(1));

    let ctx = AdvanceCtx {
        memory,
        checker,
        brain,
        session_factory,
        shared_session: &shared_session,
        state: &state_mx,
        claims: &claims,
        sem: &sem,
    };
    let ctx_ref = &ctx;

    std::thread::scope(|s| {
        let handles: Vec<(usize, std::thread::ScopedJoinHandle<'_, ActionOutcome>)> = indices
            .iter()
            .map(|&i| {
                let action = &actions[i];
                (
                    i,
                    s.spawn(move || dispatch_advance_goal_concurrent(action, ctx_ref)),
                )
            })
            .collect();

        for (i, handle) in handles {
            // A panicking dispatch thread must not abort the others (Pillar 11
            // honest degradation): surface it as a per-action failure outcome.
            let outcome = handle.join().unwrap_or_else(|_| {
                make_outcome(
                    &actions[i],
                    false,
                    "advance-goal dispatch thread panicked".to_string(),
                )
            });
            results[i] = Some(outcome);
        }
    });

    // Restore the shared session so the daemon can close it on shutdown.
    bridges.session = shared_session
        .into_inner()
        .unwrap_or_else(|p| p.into_inner());
}

/// Dispatch a single spawn-path `AdvanceGoal` action with short state locks
/// and a per-thread LLM session. Never holds the global lock across the slow
/// `run_turn` or the engineer spawn.
fn dispatch_advance_goal_concurrent(action: &PlannedAction, ctx: &AdvanceCtx) -> ActionOutcome {
    // Bound concurrent starts to the AIMD cap. Permit released on drop.
    let _permit = ctx.sem.acquire();

    let goal_id = match &action.goal_id {
        Some(id) => id.clone(),
        None => {
            return make_outcome(action, false, "advance-goal requires a goal_id".to_string());
        }
    };

    // ── Phase 1: short state lock — classify + atomically claim ──────────
    let (goal, prepared_context) = {
        let mut guard = lock_state(ctx.state);

        let Some(goal) = guard
            .active_goals
            .active
            .iter()
            .find(|g| g.id == goal_id)
            .cloned()
        else {
            return make_outcome(
                action,
                false,
                format!("goal '{goal_id}' not found on active board"),
            );
        };

        // Assigned goals take the heartbeat path in the serialized phase. If
        // one is routed here (e.g. a direct call), skip rather than re-spawn.
        if goal.assigned_to.is_some() {
            return make_outcome(
                action,
                true,
                format!("advance skipped: goal '{goal_id}' already has a subordinate"),
            );
        }

        // Status short-circuits + issue-#1911 brain-failure auto-recovery.
        match &goal.status {
            GoalProgress::Blocked(reason) if is_brain_failure_marker(reason) => {
                tracing::info!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    "issue #1911 auto-recovery (concurrent dispatch): clearing brain-failure marker",
                );
                eprintln!(
                    "[simard] OODA auto-recovery: goal '{goal_id}' brain-failure marker cleared (issue #1911)"
                );
                guard.goal_failure_counts.remove(&goal_id);
                if let Some(g) = guard
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = GoalProgress::NotStarted;
                }
            }
            GoalProgress::Blocked(reason) => {
                return make_outcome(
                    action,
                    false,
                    format!("goal '{goal_id}' is blocked: {reason}"),
                );
            }
            GoalProgress::Completed => {
                return make_outcome(
                    action,
                    true,
                    format!("goal '{goal_id}' is already completed"),
                );
            }
            GoalProgress::Proposed => {
                return make_outcome(
                    action,
                    false,
                    format!("goal '{goal_id}' is still proposed — accept it before advancing"),
                );
            }
            GoalProgress::Paused => {
                return make_outcome(
                    action,
                    false,
                    format!("goal '{goal_id}' is paused — resume it before advancing"),
                );
            }
            _ => {}
        }

        // Atomic claim under the state lock: the intra-round guard that stops
        // two concurrent dispatches from double-spawning the same goal.
        if !try_claim(ctx.claims, &goal_id) {
            return make_outcome(
                action,
                true,
                format!(
                    "advance skipped: goal '{goal_id}' already claimed by a concurrent dispatch this round"
                ),
            );
        }

        // Re-snapshot the (possibly recovered) goal + recalled context, then
        // release the lock for the slow LLM call.
        let goal = guard
            .active_goals
            .active
            .iter()
            .find(|g| g.id == goal_id)
            .cloned()
            .unwrap_or(goal);
        let prepared = guard.prepared_context.clone();
        (goal, prepared)
    };

    // ── Phase 2: slow goal-action LLM call — NO global lock held ─────────
    // Acquire a session FIRST so the no-session case fails fast without
    // building the (git/gh-shelling) turn input.
    let run_result = match ctx.session_factory {
        Some(factory) => match factory.open_session() {
            Ok(mut session) => {
                let input = build_goal_advance_input(ctx.memory, prepared_context.as_ref(), &goal);
                let result = session.run_turn(input);
                // Best-effort close; failure to close never masks the turn.
                if let Err(e) = session.close() {
                    tracing::warn!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        error = %e,
                        "closing per-goal advance session failed; turn result preserved",
                    );
                }
                result
            }
            Err(e) => {
                return make_outcome(
                    action,
                    false,
                    format!("goal '{goal_id}' cannot advance: failed to open LLM session: {e}"),
                );
            }
        },
        None => {
            // No factory: fall back to the single shared session under a lock
            // (serialized). Holding this session lock across `run_turn` is the
            // accepted fallback cost; production wires a factory for true
            // concurrency.
            let mut sess_guard = ctx.shared_session.lock().unwrap_or_else(|p| p.into_inner());
            match sess_guard.as_deref_mut() {
                Some(session) => {
                    let input =
                        build_goal_advance_input(ctx.memory, prepared_context.as_ref(), &goal);
                    session.run_turn(input)
                }
                None => {
                    return make_outcome(
                        action,
                        false,
                        format!(
                            "goal '{goal_id}' cannot advance: no LLM session available. Check SIMARD_LLM_PROVIDER and auth config."
                        ),
                    );
                }
            }
        }
    };

    // ── Phase 3: apply the parsed decision (short lock) ──────────────────
    let result = {
        let mut guard = lock_state(ctx.state);
        apply_goal_advance_result(
            action,
            ctx.memory,
            ctx.checker,
            &mut guard.active_goals,
            &goal,
            run_result,
        )
    };

    // ── Engineer spawn — short state critical sections only (the git
    //    worktree allocation + detached subprocess run with NO lock held). ──
    if let Some(GoalAction::SpawnEngineer { task, .. }) = result.action {
        return dispatch_spawn_engineer(action, ctx.state, &goal_id, &task, ctx.brain);
    }

    result.outcome
}

/// Classify whether an `AdvanceGoal` action is a spawn candidate (goal exists
/// and is currently unassigned) and therefore belongs in the concurrent phase.
///
/// Goals with a live subordinate are routed to the serialized phase so their
/// heartbeat-check behavior is unchanged. Missing-goal / missing-id actions go
/// to the concurrent phase, which surfaces the appropriate failure outcome.
pub(super) fn is_concurrent_advance_candidate(action: &PlannedAction, state: &OodaState) -> bool {
    match &action.goal_id {
        Some(goal_id) => match state.active_goals.active.iter().find(|g| &g.id == goal_id) {
            Some(goal) => goal.assigned_to.is_none(),
            None => true,
        },
        None => true,
    }
}

#[cfg(test)]
fn active_goal_for_test(id: &str) -> crate::goal_curation::ActiveGoal {
    crate::goal_curation::ActiveGoal {
        parent_goal_id: None,
        repo: None,
        id: id.to_string(),
        description: format!("goal {id}"),
        priority: 1,
        status: GoalProgress::NotStarted,
        assigned_to: None,
        current_activity: None,
        wip_refs: vec![],
        last_progress_update_at: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Semaphore ───────────────────────────────────────────────────────

    #[test]
    fn semaphore_bounds_concurrent_holders() {
        let sem = Arc::new(Semaphore::new(2));
        let peak = Arc::new(AtomicUsize::new(0));
        let live = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|s| {
            for _ in 0..8 {
                let sem = Arc::clone(&sem);
                let peak = Arc::clone(&peak);
                let live = Arc::clone(&live);
                s.spawn(move || {
                    let _permit = sem.acquire();
                    let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    live.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });

        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "semaphore must never allow more than its permit count concurrently; peak={}",
            peak.load(Ordering::SeqCst)
        );
    }

    // ── try_claim ───────────────────────────────────────────────────────

    #[test]
    fn try_claim_is_atomic_exactly_one_winner_under_race() {
        let claims = Arc::new(Mutex::new(HashSet::new()));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let winners = Arc::new(AtomicUsize::new(0));

        std::thread::scope(|s| {
            for _ in 0..16 {
                let claims = Arc::clone(&claims);
                let barrier = Arc::clone(&barrier);
                let winners = Arc::clone(&winners);
                s.spawn(move || {
                    // All threads race to claim the SAME goal at once.
                    barrier.wait();
                    if try_claim(&claims, "shared-goal") {
                        winners.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(
            winners.load(Ordering::SeqCst),
            1,
            "exactly one thread may claim a goal id"
        );
    }

    #[test]
    fn try_claim_distinct_goals_all_succeed() {
        let claims = Mutex::new(HashSet::new());
        assert!(try_claim(&claims, "a"));
        assert!(try_claim(&claims, "b"));
        assert!(!try_claim(&claims, "a"), "second claim of 'a' must fail");
    }

    // ── is_concurrent_advance_candidate ─────────────────────────────────

    #[test]
    fn unassigned_goal_is_concurrent_candidate() {
        let mut board = crate::goal_curation::GoalBoard::new();
        crate::goal_curation::add_active_goal(&mut board, active_goal_for_test("g1")).unwrap();
        let state = OodaState::new(board);
        let action = PlannedAction {
            kind: crate::ooda_loop::ActionKind::AdvanceGoal,
            goal_id: Some("g1".to_string()),
            description: "advance".to_string(),
        };
        assert!(is_concurrent_advance_candidate(&action, &state));
    }

    #[test]
    fn assigned_goal_is_not_concurrent_candidate() {
        let mut board = crate::goal_curation::GoalBoard::new();
        let mut goal = active_goal_for_test("g1");
        goal.assigned_to = Some("engineer-x".to_string());
        crate::goal_curation::add_active_goal(&mut board, goal).unwrap();
        let state = OodaState::new(board);
        let action = PlannedAction {
            kind: crate::ooda_loop::ActionKind::AdvanceGoal,
            goal_id: Some("g1".to_string()),
            description: "advance".to_string(),
        };
        assert!(!is_concurrent_advance_candidate(&action, &state));
    }
}
