//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use std::collections::HashSet;

use chrono::Utc;

use crate::agent_supervisor::{HeartbeatStatus, check_heartbeat};
use crate::goal_curation::progress_evidence::EvidenceDecision;
use crate::goal_curation::{
    GoalProgress, clear_goal_assignment, save_goal_board, update_goal_progress,
    update_goal_progress_with_evidence,
};
use crate::ooda_loop::{ActionOutcome, OodaClients, OodaState, PlannedAction};
use crate::subagent_sessions;

use crate::ooda_actions::make_outcome;

/// Advance a goal that has a subordinate assigned by checking heartbeat
/// and validating output artifacts.
pub fn advance_goal_with_subordinate(
    action: &PlannedAction,
    memories: &mut OodaClients,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
) -> ActionOutcome {
    // Build a minimal handle for heartbeat checking. The worktree path is
    // taken from the OODA-owned EngineerWorktree (issue #1197) when
    // available so artifact validation looks at the engineer's own scope,
    // not the parent checkout. Falls back to "." for legacy/manual paths
    // that pre-date worktree isolation.
    let worktree_path = state
        .engineer_worktrees
        .get(goal_id)
        .map(|w| w.path().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let handle = crate::agent_supervisor::SubordinateHandle {
        pid: 0,
        agent_name: sub_name.to_string(),
        goal: goal_id.to_string(),
        worktree_path,
        spawn_time: 0,
        retry_count: 0,
        killed: false,
        session_name: String::new(),
    };

    match check_heartbeat(&handle, &*memories.memory) {
        Ok(HeartbeatStatus::Alive { phase, .. }) => {
            // Check if subordinate reported completion with an outcome.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*memories.memory)
                && progress.outcome.is_some()
            {
                // Subordinate claims completion — validate artifacts.
                return validate_subordinate_completion(
                    action,
                    &*memories.progress_evidence,
                    &*memories.memory,
                    state,
                    goal_id,
                    sub_name,
                    &progress,
                );
            }

            // Subordinate is alive and still working.
            //
            // Route the 50% heartbeat bump through
            // `update_goal_progress_with_evidence` (issue #1967): an
            // alive engineer is NOT evidence of progress. If the reviewer
            // cannot confirm meaningful progress since the last update,
            // the gate Rejects and the prior percent is preserved.
            let new_progress = GoalProgress::InProgress { percent: 50 };
            match update_goal_progress_with_evidence(
                &mut state.active_goals,
                goal_id,
                new_progress,
                &*memories.progress_evidence,
                &*memories.memory,
                Utc::now(),
            ) {
                Ok(EvidenceDecision::Accept { .. }) => {}
                Ok(EvidenceDecision::Reject { reason: rej }) => {
                    eprintln!(
                        "[simard] OODA subordinate heartbeat REJECTED 50% bump for \
                         goal '{goal_id}' (subordinate '{sub_name}' alive, no \
                         commits/PRs yet): {rej}"
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[simard] OODA advance_goal FAILED to persist InProgress for \
                         goal '{goal_id}': {e}"
                    );
                    return make_outcome(
                        action,
                        false,
                        format!(
                            "subordinate '{sub_name}' alive (phase={phase}) but \
                             persisting InProgress for goal '{goal_id}' failed: {e}"
                        ),
                    );
                }
            }
            make_outcome(
                action,
                true,
                format!(
                    "subordinate '{sub_name}' alive (phase={phase}), goal '{goal_id}' in-progress"
                ),
            )
        }
        Ok(HeartbeatStatus::Stale { seconds_since }) => {
            // Subordinate is stale — check if it left behind any artifacts
            // before marking as failed.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*memories.memory)
                && progress.outcome.is_some()
            {
                return validate_subordinate_completion(
                    action,
                    &*memories.progress_evidence,
                    &*memories.memory,
                    state,
                    goal_id,
                    sub_name,
                    &progress,
                );
            }

            eprintln!(
                "[simard] WARNING: subordinate '{sub_name}' stale ({seconds_since}s) \
                 with no completion outcome — clearing assignment so goal '{goal_id}' \
                 can be re-dispatched"
            );
            // Clear the assignment so dispatch_advance_goal re-enters the session
            // path and can spawn a fresh engineer on the next OODA cycle.
            if let Err(e) = clear_goal_assignment(&mut state.active_goals, goal_id) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to clear assignment for \
                     goal '{goal_id}': {e}"
                );
            } else if let Err(e) = save_goal_board(&state.active_goals, &*memories.memory) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist goal board after \
                     clearing stale assignment for goal '{goal_id}': {e}"
                );
            }
            cleanup_engineer_worktree_for_goal(state, goal_id);
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' stale ({seconds_since}s) with no artifacts, \
                     goal '{goal_id}' assignment cleared for re-dispatch"
                ),
            )
        }
        Ok(HeartbeatStatus::Dead) => {
            // Subordinate is dead — check if it produced anything before dying.
            if let Ok(Some(progress)) =
                crate::agent_goal_assignment::poll_progress(sub_name, &*memories.memory)
            {
                if progress.outcome.is_some() {
                    return validate_subordinate_completion(
                        action,
                        &*memories.progress_evidence,
                        &*memories.memory,
                        state,
                        goal_id,
                        sub_name,
                        &progress,
                    );
                }
                // Subordinate reported progress but no outcome — silent exit.
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' died without reporting \
                     an outcome — last phase='{}', last action='{}', \
                     exit_status={:?}, commits={}, prs={}",
                    progress.phase,
                    progress.last_action,
                    progress.exit_status,
                    progress.commits_produced,
                    progress.prs_produced,
                );
            } else {
                eprintln!(
                    "[simard] WARNING: subordinate '{sub_name}' is dead with no progress \
                     reports at all — it may have exited immediately without doing any work"
                );
            }

            if let Err(e) = clear_goal_assignment(&mut state.active_goals, goal_id) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to clear assignment for \
                     goal '{goal_id}': {e}"
                );
                cleanup_engineer_worktree_for_goal(state, goal_id);
                return make_outcome(
                    action,
                    false,
                    format!(
                        "subordinate '{sub_name}' exited with no artifacts and \
                         clearing assignment for goal '{goal_id}' failed: {e}"
                    ),
                );
            }
            if let Err(e) = save_goal_board(&state.active_goals, &*memories.memory) {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist goal board after \
                     clearing dead assignment for goal '{goal_id}': {e}"
                );
            }
            // Reap the per-engineer worktree (issue #1197).
            cleanup_engineer_worktree_for_goal(state, goal_id);
            make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' exited with no output artifacts, \
                     goal '{goal_id}' assignment cleared for re-dispatch"
                ),
            )
        }
        Err(e) => make_outcome(
            action,
            false,
            format!("heartbeat check failed for subordinate '{sub_name}': {e}"),
        ),
    }
}

/// Cleanup the per-goal engineer worktree owned by the OODA state.
///
/// Called from terminal paths (subordinate completed, dead, or stale-failed)
/// so the worktree dir + branch are reaped within one OODA cycle of the
/// engineer's exit. Idempotent — missing entries are silently a no-op.
fn cleanup_engineer_worktree_for_goal(state: &mut OodaState, goal_id: &str) {
    if let Some(worktree) = state.engineer_worktrees.remove(goal_id)
        && let Err(e) = worktree.cleanup()
    {
        tracing::warn!(
            target: "simard::engineer_worktree",
            goal = %goal_id,
            error = %e,
            "engineer worktree cleanup failed; Drop will run as a safety net",
        );
        // worktree drops here; if cleanup() already ran the swap guard
        // ensures Drop is a no-op.
    }
    // Release-on-termination (issue #4094): deterministically free the
    // typed-OODA engineer claim so the goal can spawn again. This chokepoint is
    // reached by every engineer exit path (success, failure, blocked, crash,
    // zombie-reap), co-located with the worktree-sentinel drop above. The
    // release is idempotent and fail-visible.
    release_engineer_claim_for_goal(state, goal_id);
}

/// Delete the `engineer_claims` lease row for `goal_id` on engineer
/// termination. Reconstructs the deterministic `claim_key`
/// (`{owner}/{repo}:{goal_id}`) from the goal's stored repo slug and issues the
/// idempotent [`CapabilityHandler::release_engineer_claim`]. Fail-visible: a
/// failure to open the ledger or delete the row is logged at error and never
/// silently swallowed (the stale-claim reclaim gate is the safety net).
fn release_engineer_claim_for_goal(state: &OodaState, goal_id: &str) {
    let repo_slug = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .and_then(|g| g.repo.as_deref());
    let repository = crate::typed_ooda::RepositoryRef::from_goal_slug(repo_slug);
    let claim_key = format!("{}/{}:{}", repository.owner, repository.name, goal_id);

    let ledger_path = crate::typed_ooda::ledger_path(
        &crate::ooda_actions::advance_goal::spawn::typed_ooda_state_root(),
    );
    let policy = crate::typed_ooda::CapabilityPolicy::new("engineer-claim-release");
    let handler = match crate::typed_ooda::CapabilityHandler::open(&ledger_path, policy) {
        Ok(handler) => handler,
        Err(error) => {
            tracing::error!(
                target: "simard::engineer_claim",
                goal = %goal_id,
                claim_key = %claim_key,
                error = %error,
                "failed to open ledger to release engineer claim on termination",
            );
            return;
        }
    };
    if let Err(error) = handler.release_engineer_claim(&claim_key) {
        tracing::error!(
            target: "simard::engineer_claim",
            goal = %goal_id,
            claim_key = %claim_key,
            error = %error,
            "failed to release engineer claim on termination",
        );
    }
}

/// Select the single **live** subagent-session row for `goal_id`, if any.
///
/// The registry retains ended rows (one per retry) for up to its retention
/// window, and their recorded `pid`s can be recycled by the OS onto unrelated
/// live processes. Because [`kill_subordinate`](crate::agent_supervisor::kill_subordinate)
/// signals by `pid` only, the reaper must target the live row
/// (`ended_at.is_none()`, newest `created_at`) so a stale/recycled pid is never
/// SIGTERM'd. Returns `None` when the registry holds no live row for the goal
/// (e.g. all rows ended/GC'd, or a test env with no tmux) — the caller then
/// skips the SIGTERM but still runs the authoritative worktree cleanup.
fn select_live_session<'a>(
    registry: &'a subagent_sessions::Registry,
    goal_id: &str,
) -> Option<&'a subagent_sessions::SubagentSession> {
    registry
        .sessions
        .iter()
        .filter(|s| s.goal_id == goal_id && s.ended_at.is_none())
        .max_by_key(|s| s.created_at)
}

/// Cheap in-memory predicate: does any in-flight engineer's goal appear in
/// `tombstones`? Pure `HashSet` lookups over the (typically tiny)
/// `engineer_worktrees` map — no allocation, no I/O.
///
/// The daemon uses this to skip the per-cycle subagent-registry disk read +
/// JSON parse entirely on the common steady-state path where nothing needs
/// reaping. It is the single source of truth for the tombstone-only reap
/// predicate, so the guard here and the victim selection in
/// [`reap_engineers_for_tombstoned_goals`] can never drift apart.
pub fn has_tombstoned_engineer(state: &OodaState, tombstones: &HashSet<String>) -> bool {
    state
        .engineer_worktrees
        .keys()
        .any(|goal_id| tombstones.contains(goal_id.as_str()))
}

/// Reap every in-flight engineer whose goal has been tombstoned
/// (removed via `simard goal remove` or completed via `simard goal complete`).
///
/// Runs once per OODA cycle, right after the goal board is reloaded and
/// tombstone-filtered. State-driven and tombstone-gated: an engineer is reaped
/// iff its `goal_id` is in `tombstones`. Never a wall-clock timeout — a healthy
/// engineer whose goal is still on the board is never touched.
///
/// Two independent, idempotent steps per victim:
///   1. best-effort SIGTERM via a registry-recovered `SubordinateHandle`
///      (`kill_subordinate`, ESRCH-tolerant, never SIGKILL);
///   2. always `cleanup_engineer_worktree_for_goal` (removes the map entry,
///      runs the guarded worktree `.cleanup()`/Drop, releases the claim).
///
/// Per-victim errors are contained and logged; the reconciliation never aborts
/// the OODA cycle. Returns the `goal_id`s reaped this cycle (empty if none), so
/// the caller can log exactly which orphaned engineers were terminated.
pub fn reap_engineers_for_tombstoned_goals(
    state: &mut OodaState,
    tombstones: &HashSet<String>,
    registry: &subagent_sessions::Registry,
) -> Vec<String> {
    // Cheap gate: bail before any allocation when no in-flight engineer's goal
    // is tombstoned (the overwhelmingly common case). Same predicate the daemon
    // uses to skip the registry disk-load, so behaviour cannot drift.
    if !has_tombstoned_engineer(state, tombstones) {
        return Vec::new();
    }

    // Collect victims first (owned) so the subsequent `&mut state` reaping never
    // overlaps a borrow of `state.engineer_worktrees`. Predicate is
    // tombstone-only: absence from the active board is deliberately NOT a
    // trigger, so Blocked/Paused/backlog/completion-pending goals (never
    // tombstoned) keep their engineers.
    let victims: Vec<String> = state
        .engineer_worktrees
        .keys()
        .filter(|goal_id| tombstones.contains(goal_id.as_str()))
        .cloned()
        .collect();

    for goal_id in &victims {
        // Step 1 — best-effort graceful SIGTERM via the LIVE registry row.
        //
        // The registry retains ended rows (per retry) for up to its retention
        // window, and their recorded `pid`s can be recycled by the OS onto
        // unrelated live processes. `kill_subordinate` signals by `pid` only, so
        // we must target the live row (`ended_at.is_none()`, newest
        // `created_at`) — never a stale/recycled pid. A miss skips only the
        // SIGTERM; Step 2 cleanup is authoritative and always runs.
        if let Some(session) = select_live_session(registry, goal_id) {
            let worktree_path = state
                .engineer_worktrees
                .get(goal_id)
                .map(|w| w.path().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            // Transient targeting handle: only `pid`/`session_name` matter to
            // `kill_subordinate`; the remaining fields are inert placeholders.
            let mut handle = crate::agent_supervisor::SubordinateHandle {
                pid: session.pid,
                agent_name: session.agent_id.clone(),
                goal: goal_id.clone(),
                worktree_path,
                spawn_time: session.created_at.max(0) as u64,
                retry_count: 0,
                killed: false,
                session_name: session.session_name.clone(),
            };
            match crate::agent_supervisor::kill_subordinate(&mut handle) {
                Ok(()) => {
                    tracing::info!(
                        target: "simard::engineer_reaper",
                        goal = %goal_id,
                        pid = session.pid,
                        "sent SIGTERM to in-flight engineer for tombstoned goal",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "simard::engineer_reaper",
                        goal = %goal_id,
                        pid = session.pid,
                        error = %e,
                        "SIGTERM to reaped engineer failed; worktree cleanup still runs",
                    );
                }
            }
        } else {
            tracing::debug!(
                target: "simard::engineer_reaper",
                goal = %goal_id,
                "no live registry row for tombstoned goal; skipping SIGTERM, cleanup still runs",
            );
        }

        // Step 2 — authoritative cleanup (ALWAYS runs, idempotent): removes the
        // map entry, runs the guarded worktree `.cleanup()`/Drop, and releases
        // the engineer claim.
        cleanup_engineer_worktree_for_goal(state, goal_id);
    }

    victims
}

/// Validate that a subordinate's claimed completion produced real artifacts.
///
/// If the subordinate reports success but has zero commits and zero PRs,
/// the action is marked as failed so the OODA cycle can retry with a
/// different approach.
pub fn validate_subordinate_completion(
    action: &PlannedAction,
    checker: &dyn crate::goal_curation::progress_evidence::ProgressEvidenceChecker,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    state: &mut OodaState,
    goal_id: &str,
    sub_name: &str,
    progress: &crate::agent_goal_assignment::SubordinateProgress,
) -> ActionOutcome {
    let has_artifacts = progress.has_artifacts();
    let outcome_text = progress.outcome.as_deref().unwrap_or("unknown");

    if has_artifacts {
        // Route the Completed write through the progress-evidence gate
        // (issue #1967) for audit-trail consistency. Rule 1 (commit on
        // engineer branch) is satisfied by definition here because the
        // subordinate produced commits; the gate Accepts and stamps
        // `last_progress_update_at`.
        let new_progress = GoalProgress::Completed;
        match update_goal_progress_with_evidence(
            &mut state.active_goals,
            goal_id,
            new_progress,
            checker,
            memory,
            Utc::now(),
        ) {
            Ok(EvidenceDecision::Accept { .. }) => {}
            Ok(EvidenceDecision::Reject { reason: rej }) => {
                // Unexpected — subordinate produced artifacts so rule 1
                // should match. Log and fall through; the percent stays
                // where it was, but we still treat the action as
                // successful since the engineer did produce output.
                eprintln!(
                    "[simard] OODA validate_subordinate_completion: gate REJECTED \
                     Completed for goal '{goal_id}' despite artifacts: {rej}"
                );
            }
            Err(e) => {
                eprintln!(
                    "[simard] OODA advance_goal FAILED to persist Completed for \
                     goal '{goal_id}': {e}"
                );
                return make_outcome(
                    action,
                    false,
                    format!(
                        "subordinate '{sub_name}' produced {} commit(s) and {} PR(s) for \
                         goal '{goal_id}' but persisting Completed failed: {e}",
                        progress.commits_produced, progress.prs_produced,
                    ),
                );
            }
        }
        eprintln!(
            "[simard] subordinate '{sub_name}' completed goal '{goal_id}' — \
             {} commit(s), {} PR(s), outcome='{outcome_text}'",
            progress.commits_produced, progress.prs_produced,
        );
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            true,
            format!(
                "subordinate '{sub_name}' completed goal '{goal_id}' with \
                 {} commit(s) and {} PR(s)",
                progress.commits_produced, progress.prs_produced,
            ),
        )
    } else {
        // Subordinate claims success but produced nothing — this is the
        // silent exit bug (issue #905). Mark as failed for retry.
        //
        // `Blocked(reason)` is in the bypass set for the progress-evidence
        // gate (it does not increase the percent) so we keep the direct
        // `update_goal_progress` call here.
        eprintln!(
            "[simard] WARNING: subordinate '{sub_name}' reported outcome \
             '{outcome_text}' for goal '{goal_id}' but produced 0 commits \
             and 0 PRs — marking as failed for OODA retry"
        );
        if let Err(e) = update_goal_progress(
            &mut state.active_goals,
            goal_id,
            GoalProgress::Blocked(format!(
                "subordinate '{sub_name}' exited with outcome '{outcome_text}' \
                 but produced no commits or PRs"
            )),
        ) {
            eprintln!(
                "[simard] OODA advance_goal FAILED to persist Blocked for \
                 goal '{goal_id}': {e}"
            );
            cleanup_engineer_worktree_for_goal(state, goal_id);
            return make_outcome(
                action,
                false,
                format!(
                    "subordinate '{sub_name}' claimed '{outcome_text}' for goal '{goal_id}' \
                     but produced no artifacts and persisting Blocked failed: {e}"
                ),
            );
        }
        // Reap the per-engineer worktree (issue #1197).
        cleanup_engineer_worktree_for_goal(state, goal_id);
        make_outcome(
            action,
            false,
            format!(
                "subordinate '{sub_name}' claimed '{outcome_text}' for goal '{goal_id}' \
                 but produced 0 commits and 0 PRs — action failed, eligible for retry"
            ),
        )
    }
}

// ---------------------------------------------------------------------------
// Tests (issue #4232) — Tombstoned-goal engineer reaper.
//
// These are written against the public contract in
// `docs/reference/tombstoned-goal-engineer-reaper-api.md` and exercise the
// shipped `reap_engineers_for_tombstoned_goals` reconciliation directly.
//
// Contract under test:
//   pub fn reap_engineers_for_tombstoned_goals(
//       state: &mut OodaState,
//       tombstones: &HashSet<String>,
//       registry: &subagent_sessions::Registry,
//   ) -> Vec<String>
//
// Reap predicate is tombstone-ONLY: reap iff goal_id ∈ tombstones. Absence
// from the active board is NOT a predicate. Two independent idempotent steps
// per victim: (1) best-effort SIGTERM via a registry-recovered handle, (2)
// always cleanup the per-goal worktree. Returns the reaped goal_ids.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod reap_tests {
    use std::collections::HashSet;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::reap_engineers_for_tombstoned_goals;
    use crate::engineer_worktree::EngineerWorktree;
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::ooda_loop::OodaState;
    use crate::subagent_sessions;

    // -- fixtures -----------------------------------------------------------

    /// Restore `SIMARD_STATE_ROOT` on drop so the claim-release chokepoint the
    /// reaper reuses (`typed_ooda_state_root()`) stays hermetic to a temp dir.
    struct StateRootGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl StateRootGuard {
        fn set(root: &Path) -> Self {
            let prev = std::env::var_os("SIMARD_STATE_ROOT");
            // SAFETY: tests in this module are serialized via #[serial_test::serial(cognitive_memory)],
            // the canonical group guarding the process-global SIMARD_STATE_ROOT env var.
            unsafe { std::env::set_var("SIMARD_STATE_ROOT", root) };
            Self { prev }
        }
    }

    impl Drop for StateRootGuard {
        fn drop(&mut self) {
            // SAFETY: tests in this module are serialized via #[serial_test::serial(cognitive_memory)],
            // the canonical group guarding the process-global SIMARD_STATE_ROOT env var.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("SIMARD_STATE_ROOT", v) },
                None => unsafe { std::env::remove_var("SIMARD_STATE_ROOT") },
            }
        }
    }

    /// `git` command mirroring production isolation: clear env, re-inject only
    /// PATH + HOME so process-global GIT_DIR/GIT_WORK_TREE cannot poison the
    /// fixture repo.
    fn git_cmd(repo: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo).env_clear();
        if let Ok(p) = std::env::var("PATH") {
            cmd.env("PATH", p);
        }
        if let Ok(h) = std::env::var("HOME") {
            cmd.env("HOME", h);
        }
        cmd
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let out = git_cmd(repo, args).output().expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A parent repo with a committed `main` so `EngineerWorktree::allocate`
    /// (which branches off `main` HEAD) succeeds.
    fn init_parent_repo(dir: &Path) {
        std::fs::create_dir_all(dir).expect("create parent repo dir");
        run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
        run_git(dir, &["config", "user.email", "test@example.com"]);
        run_git(dir, &["config", "user.name", "test"]);
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("README.md"), "seed\n").expect("seed file");
        run_git(dir, &["add", "README.md"]);
        run_git(dir, &["commit", "-m", "seed", "--quiet"]);
    }

    /// A live registry row for `goal_id`. `pid == 0` so `kill_subordinate`
    /// sends no real signal (safe in the test env) but the row still exercises
    /// the registry-join code path. `ended_at: None` marks it live.
    fn live_session_row(goal_id: &str) -> subagent_sessions::SubagentSession {
        subagent_sessions::SubagentSession {
            agent_id: format!("engineer-{goal_id}"),
            session_name: format!("simard-engineer-{goal_id}"),
            host: "localhost".to_string(),
            pid: 0,
            created_at: 1_000,
            ended_at: None,
            goal_id: goal_id.to_string(),
        }
    }

    /// An ENDED registry row for `goal_id` (an earlier retry that already
    /// exited). Carries a distinct, older `created_at` and a non-zero `pid` so
    /// a test can prove the live-row selector never targets this stale pid.
    fn ended_session_row(
        goal_id: &str,
        pid: u32,
        created_at: i64,
    ) -> subagent_sessions::SubagentSession {
        subagent_sessions::SubagentSession {
            agent_id: format!("engineer-{goal_id}-old"),
            session_name: format!("simard-engineer-{goal_id}-old"),
            host: "localhost".to_string(),
            pid,
            created_at,
            ended_at: Some(created_at + 1),
            goal_id: goal_id.to_string(),
        }
    }

    /// Allocate a real per-engineer worktree under `state_root` and register it
    /// in `state.engineer_worktrees` keyed by `goal_id`. Returns the on-disk
    /// worktree path so the test can assert on its (dis)appearance.
    fn attach_engineer(
        state: &mut OodaState,
        parent_repo: &Path,
        state_root: &Path,
        goal_id: &str,
    ) -> std::path::PathBuf {
        let wt = EngineerWorktree::allocate(parent_repo, state_root, goal_id)
            .expect("allocate engineer worktree");
        let path = wt.path().to_path_buf();
        assert!(
            path.is_dir(),
            "freshly allocated worktree must exist on disk"
        );
        state.engineer_worktrees.insert(goal_id.to_string(), wt);
        path
    }

    // -- Test (a): tombstoned goal's engineer IS reaped ---------------------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reaps_engineer_and_cleans_worktree_when_goal_tombstoned() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let goal_id = "gone-goal";

        // The board no longer carries the goal (it was removed/tombstoned),
        // but its engineer is still in-flight and tracked in state.
        let mut state = OodaState::new(GoalBoard::new());
        let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

        let mut tombstones = HashSet::new();
        tombstones.insert(goal_id.to_string());

        let registry = subagent_sessions::Registry {
            sessions: vec![live_session_row(goal_id)],
        };

        let reaped = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);

        assert_eq!(
            reaped,
            vec![goal_id.to_string()],
            "the tombstoned goal's engineer must be reported as reaped"
        );
        assert!(
            !state.engineer_worktrees.contains_key(goal_id),
            "the reaped engineer must be dropped from engineer_worktrees"
        );
        assert!(
            !wt_path.exists(),
            "the reaped engineer's worktree dir must be cleaned from disk: {}",
            wt_path.display()
        );
    }

    // -- Test (b): healthy engineer for a present goal is NOT reaped --------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn does_not_reap_engineer_for_present_active_goal() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let goal_id = "live-goal";

        // Goal is still present and healthy on the active board.
        let mut board = GoalBoard::new();
        board
            .active
            .push(ActiveGoal::new(goal_id, "ship the feature", 1));
        let mut state = OodaState::new(board);
        let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

        // Nothing is tombstoned — the goal is genuinely still present.
        let tombstones: HashSet<String> = HashSet::new();
        let registry = subagent_sessions::Registry {
            sessions: vec![live_session_row(goal_id)],
        };

        let reaped = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);

        assert!(
            reaped.is_empty(),
            "a healthy engineer for a still-present goal must NOT be reaped, got {reaped:?}"
        );
        assert!(
            state.engineer_worktrees.contains_key(goal_id),
            "the healthy engineer must remain tracked in engineer_worktrees"
        );
        assert!(
            wt_path.is_dir(),
            "the healthy engineer's worktree dir must remain on disk: {}",
            wt_path.display()
        );
    }

    // -- Test (c): Blocked-but-present goal's engineer is NOT reaped --------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn does_not_reap_engineer_for_blocked_but_present_goal() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let goal_id = "blocked-goal";

        // Goal is Blocked but STILL on the board — Blocked goals are never
        // tombstoned, so the engineer must survive.
        let mut blocked = ActiveGoal::new(goal_id, "waiting on infra", 1);
        blocked.status = GoalProgress::Blocked("waiting on upstream infra".to_string());
        let mut board = GoalBoard::new();
        board.active.push(blocked);
        let mut state = OodaState::new(board);
        let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

        // Blocked ≠ tombstoned: the tombstone set does not contain the goal.
        let tombstones: HashSet<String> = HashSet::new();
        let registry = subagent_sessions::Registry {
            sessions: vec![live_session_row(goal_id)],
        };

        let reaped = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);

        assert!(
            reaped.is_empty(),
            "a Blocked-but-present goal's engineer must NOT be reaped, got {reaped:?}"
        );
        assert!(
            state.engineer_worktrees.contains_key(goal_id),
            "the Blocked goal's engineer must remain tracked in engineer_worktrees"
        );
        assert!(
            wt_path.is_dir(),
            "the Blocked goal's worktree dir must remain on disk: {}",
            wt_path.display()
        );
    }

    // -- Test (d): reconciliation is idempotent -----------------------------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reaping_is_idempotent_second_cycle_is_a_noop() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let goal_id = "gone-goal";
        let mut state = OodaState::new(GoalBoard::new());
        let _wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

        let mut tombstones = HashSet::new();
        tombstones.insert(goal_id.to_string());
        let registry = subagent_sessions::Registry {
            sessions: vec![live_session_row(goal_id)],
        };

        let first = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);
        assert_eq!(
            first,
            vec![goal_id.to_string()],
            "first cycle reaps the engineer"
        );

        // Same tombstone set on the next cycle: the entry is already gone, so
        // the reconciliation is a no-op and reports nothing reaped.
        let second = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);
        assert!(
            second.is_empty(),
            "a second reconciliation with the same tombstones must be a no-op, got {second:?}"
        );
        assert!(!state.engineer_worktrees.contains_key(goal_id));
    }

    // -- Test (e): registry miss still cleans the worktree ------------------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reaps_worktree_even_when_registry_has_no_live_row() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let goal_id = "gone-goal";
        let mut state = OodaState::new(GoalBoard::new());
        let wt_path = attach_engineer(&mut state, parent.path(), state_dir.path(), goal_id);

        let mut tombstones = HashSet::new();
        tombstones.insert(goal_id.to_string());

        // Registry has NO row for the goal (e.g. no tmux / already GC'd): the
        // SIGTERM step is skipped, but the authoritative worktree cleanup must
        // still run because the two steps are independent and idempotent.
        let registry = subagent_sessions::Registry { sessions: vec![] };

        let reaped = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);

        assert_eq!(
            reaped,
            vec![goal_id.to_string()],
            "cleanup-only reap (registry miss) must still report the goal as reaped"
        );
        assert!(
            !state.engineer_worktrees.contains_key(goal_id),
            "worktree map entry must be removed even on registry miss"
        );
        assert!(
            !wt_path.exists(),
            "worktree dir must be cleaned even when the kill step is skipped: {}",
            wt_path.display()
        );
    }

    // -- Test (f): only the tombstoned engineer is reaped, others survive ---

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn reaps_only_tombstoned_engineer_and_leaves_others_running() {
        let parent = tempdir().expect("tempdir");
        let state_dir = tempdir().expect("tempdir");
        init_parent_repo(parent.path());
        let _guard = StateRootGuard::set(state_dir.path());

        let gone = "gone-goal";
        let kept = "kept-goal";

        // `kept` is still present on the active board; `gone` was tombstoned.
        let mut board = GoalBoard::new();
        board.active.push(ActiveGoal::new(kept, "keep shipping", 1));
        let mut state = OodaState::new(board);
        let gone_path = attach_engineer(&mut state, parent.path(), state_dir.path(), gone);
        let kept_path = attach_engineer(&mut state, parent.path(), state_dir.path(), kept);

        let mut tombstones = HashSet::new();
        tombstones.insert(gone.to_string());

        let registry = subagent_sessions::Registry {
            sessions: vec![live_session_row(gone), live_session_row(kept)],
        };

        let reaped = reap_engineers_for_tombstoned_goals(&mut state, &tombstones, &registry);

        assert_eq!(
            reaped,
            vec![gone.to_string()],
            "exactly the tombstoned engineer must be reaped"
        );
        assert!(!state.engineer_worktrees.contains_key(gone));
        assert!(
            !gone_path.exists(),
            "tombstoned engineer's worktree must be gone"
        );

        assert!(
            state.engineer_worktrees.contains_key(kept),
            "the still-present goal's engineer must survive"
        );
        assert!(
            kept_path.is_dir(),
            "the surviving engineer's worktree dir must remain: {}",
            kept_path.display()
        );
    }

    // -- Test (d): live-row selection ignores ended rows --------------------
    //
    // PID-reuse safety: the registry may hold several rows per goal_id — mostly
    // ended retries whose recorded pids the OS can recycle. `kill_subordinate`
    // signals by pid only, so the reaper must target the LIVE row
    // (`ended_at.is_none()`, newest `created_at`), never a stale/recycled pid.
    // This asserts the selector directly (no real signal is sent).

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn selects_live_registry_row_and_ignores_ended_rows() {
        let goal_id = "gone-goal";
        const STALE_PID: u32 = 999_999; // a pid we must NEVER target

        // Two ended retries (older, with a stale/recycled pid) plus one live
        // row (`ended_at: None`, newest `created_at`, pid 0).
        let registry = subagent_sessions::Registry {
            sessions: vec![
                ended_session_row(goal_id, STALE_PID, 100),
                ended_session_row(goal_id, STALE_PID, 500),
                live_session_row(goal_id), // ended_at: None, created_at: 1_000
            ],
        };

        let selected =
            super::select_live_session(&registry, goal_id).expect("a live row must be selected");

        assert!(
            selected.ended_at.is_none(),
            "the selected row must be the LIVE one (ended_at: None)"
        );
        assert_eq!(
            selected.pid, 0,
            "the live row's pid must be selected, never the stale/recycled ended pid"
        );
        assert_ne!(
            selected.pid, STALE_PID,
            "the stale/recycled ended pid must never be targeted for SIGTERM"
        );

        // With only ended rows present, the selector returns None so the reaper
        // skips SIGTERM entirely (cleanup remains authoritative).
        let ended_only = subagent_sessions::Registry {
            sessions: vec![
                ended_session_row(goal_id, STALE_PID, 100),
                ended_session_row(goal_id, STALE_PID, 500),
            ],
        };
        assert!(
            super::select_live_session(&ended_only, goal_id).is_none(),
            "with no live row, no session may be selected (SIGTERM is skipped)"
        );
    }
}
