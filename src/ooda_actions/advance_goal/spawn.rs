//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::identity_composition::max_subordinate_depth;
use crate::ooda_brain::{
    AdmissionGate, BrainJudgmentRecord, EngineerLifecycleDecision, OodaAdmissionBrain, OodaBrain,
    apply_decision_to_state, configured_ceiling_pct, gather_engineer_lifecycle_ctx,
    gather_resource_admission_ctx, judge_and_resolve, push_brain_judgment,
};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};
use std::path::Path;

use crate::ooda_actions::make_outcome;

// ── Issue #1911: brain-failure auto-recovery marker ────────────────────────
//
// The deterministic safeguard in `dispatch_spawn_engineer` writes a
// `GoalProgress::Blocked(reason)` reason after 3 consecutive brain
// failures. To make the persisted marker recoverable without colliding
// with operator-set, scope-blocked, dependency-blocked, or
// subordinate-blocked reasons, the rendered string carries a sentinel-
// bearing prefix that no other Blocked-reason source can produce:
//
//   - U+1F512 LOCK code point (\u{1F512}): not typed by humans.
//   - "OODA-SAFEGUARD" token: a literal authoring marker.
//
// `is_brain_failure_marker` is the single predicate that drives the
// auto-recovery branch in `dispatch_advance_goal` and the bulk-unblock
// scope in `simard goal unblock-all`. All match/render sites for the
// safeguard reason go through these constants.

/// Sentinel prefix of the deterministic brain-failure `Blocked` reason.
pub const BRAIN_FAILURE_BLOCKED_PREFIX: &str = "\u{1F512} [OODA-SAFEGUARD] OODA brain failing for ";

/// Trailing portion of the deterministic brain-failure `Blocked` reason.
pub const BRAIN_FAILURE_BLOCKED_SUFFIX: &str = " consecutive cycles; needs human review";

/// Returns `true` iff `reason` was authored by the deterministic
/// brain-failure safeguard in `dispatch_spawn_engineer`. The predicate
/// gates the issue-#1911 auto-recovery branch and the `unblock-all`
/// bulk-clear so we never override operator-set, scope-blocked,
/// dependency-blocked, or subordinate-blocked reasons.
pub fn is_brain_failure_marker(reason: &str) -> bool {
    reason.starts_with(BRAIN_FAILURE_BLOCKED_PREFIX)
        && reason.contains(BRAIN_FAILURE_BLOCKED_SUFFIX)
}

/// Number of consecutive healthy cycles required before the auto-recovery
/// branch clears a persisted brain-failure marker. Intentionally `1`:
/// the first non-fallback brain signal after a marker-blocked state
/// restores the goal so production lockouts (issue #1911) heal at
/// minimum latency. Bumping this would re-introduce the outage window.
#[allow(dead_code)] // Surface point for future tuning if flapping is observed.
pub const HEALTHY_CYCLES_TO_UNBLOCK: u32 = 1;

/// Lock the shared OODA state, recovering from a poisoned lock instead of
/// panicking.
///
/// Concurrent `AdvanceGoal` dispatch shares `&mut OodaState` behind a
/// `Mutex`. If one dispatch thread panics while holding the lock, the lock
/// becomes poisoned; recovering the guard (rather than `.expect()`-ing) keeps
/// the remaining engineer spawns — and the daemon — alive (Pillar 11: honest
/// degradation, no cycle-wide abort from one failed action).
pub(crate) fn lock_state<'g, 'a>(
    state: &'g Mutex<&'a mut OodaState>,
) -> std::sync::MutexGuard<'g, &'a mut OodaState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Spawn a subordinate engineer for a goal that the LLM picked
/// `spawn_engineer` for, then mutate the active board to record the
/// assignment.
///
/// Takes the shared state behind a `Mutex` and holds it only for short
/// critical sections (assignment re-check, goal lookup, status writeback).
/// The slow work — target-repo resolution, git worktree allocation, and the
/// detached subprocess spawn — runs WITHOUT the lock held, so multiple
/// engineers start concurrently within one OODA round (bounded by the AIMD
/// cap upstream).
///
/// Honours `SIMARD_SUBORDINATE_DEPTH` vs. `SIMARD_MAX_SUBORDINATE_DEPTH`
/// so a recursing supervisor does not spawn forever.
///
/// `admission` is the RESOURCE-AWARE admission reasoner consulted on the
/// fresh-spawn path (after the count cap, before worktree allocation): it
/// weighs disk / build-cache / load and decides ADMIT / DEFER / RECLAIM-FIRST,
/// guarded by a deterministic disk hard-rail. `repo_root` locates the
/// disk-health reclaim recipe invoked on RECLAIM-FIRST.
pub fn dispatch_spawn_engineer(
    action: &PlannedAction,
    state: &Mutex<&mut OodaState>,
    goal_id: &str,
    task: &str,
    brain: &dyn OodaBrain,
    admission: &dyn OodaAdmissionBrain,
    repo_root: &Path,
) -> ActionOutcome {
    // ── Simard #3125: deterministic read-only spawn rail (L2 defense) ────────
    //
    // The single write-bearing chokepoint the Act phase funnels through. When
    // the active identity's posture is read-only (an OBSERVER identity such as
    // Crocutus), hard-block BEFORE any assignment re-check, admission gate,
    // worktree allocation, or subprocess spawn — so no write-bearing engineer
    // is ever dispatched and no AI credits are burned on work the read_only
    // guard would block anyway. This is defense-in-depth beneath the
    // observe-only Act branch (L1): even if control reaches here, the write is
    // refused. Fail-closed: the posture is read STRAIGHT off the shared state,
    // and a read-only posture is a benign skip (`success == true`) so the
    // 3-strikes brain-failure safeguard is never tripped by an observer.
    {
        let guard = lock_state(state);
        if !guard.write_authority.may_dispatch_engineers() {
            eprintln!(
                "[simard] spawn_engineer BLOCKED for goal '{goal_id}': identity posture is {} (deny-by-default: only read-write may dispatch engineers) — no write-bearing engineer dispatched (issue #3125)",
                guard.write_authority
            );
            return make_outcome(
                action,
                true,
                format!(
                    "spawn_engineer skipped: identity posture is read-only (observe-only) for goal '{goal_id}' — no engineer dispatched"
                ),
            );
        }
    }

    // Re-check assignment under a short exclusive state lock to prevent a
    // double-spawn race (two cycles/threads parsing spawn_engineer for the
    // same goal). The per-round claim set in the dispatcher is the primary
    // intra-round guard; this is cross-round defense-in-depth.
    {
        let guard = lock_state(state);
        if let Some(g) = guard.active_goals.active.iter().find(|g| g.id == goal_id)
            && g.assigned_to.is_some()
        {
            let assigned = g.assigned_to.as_deref().unwrap_or("?").to_string();
            return make_outcome(
                action,
                true,
                format!(
                    "spawn_engineer skipped: goal '{goal_id}' already assigned to subordinate '{assigned}'",
                ),
            );
        }
    }

    // Defense-in-depth (issue #1227): check the on-disk engineer-worktrees
    // directory for any live worktree already pursuing this goal. The
    // `assigned_to` board check above can miss in-flight engineers if the
    // daemon was restarted between spawn and goal-status writeback (the
    // engineer subprocess survives systemd unit restart). Without this
    // check, we burn a second LLM session on the same goal.
    //
    // Issue #1266: instead of unconditionally returning success=true (which
    // clears the failure counter and makes FAILURE_PENALTY useless), consult
    // the prompt-driven brain. The brain reasons about whether to keep
    // skipping, reclaim, deprioritize, file an issue, or block the goal.
    // Resolve the engineer-worktree state root ONCE and reuse it for both the
    // in-flight re-attach check below and the resource-admission gate further
    // down. `engineer_worktree_state_root()` is a pure env-read + path-join, so
    // this is a clarity win rather than a hot-path saving, but it removes a
    // duplicate computation and keeps a single source of truth for the root.
    let engineer_state_root = engineer_worktree_state_root();
    if let Some(live) = find_live_engineer_for_goal(&engineer_state_root, goal_id) {
        let ctx = {
            let guard = lock_state(state);
            gather_engineer_lifecycle_ctx(&guard, &engineer_state_root, goal_id, &live)
        };
        // NO FALLBACK: brain.decide_engineer_lifecycle Err must surface as a
        // visible cycle failure (operator constraint, issue #1711, #1748).
        // Silent ContinueSkipping on brain Err was the bug: it cleared the
        // failure counter and made the system look healthy while the brain
        // was completely broken. We now propagate the error loudly:
        //   - log at ERROR severity with full context
        //   - return success=false outcome (cycle.rs auto-bumps
        //     state.goal_failure_counts[goal_id])
        //   - if the failure count crosses the deterministic safeguard
        //     threshold (3 consecutive cycles), file a tracking issue and
        //     mark the goal Blocked. NOTE: this is a deterministic safeguard
        //     enforced by simard, NOT a brain decision — the brain is broken
        //     and cannot be trusted to make decisions about itself.
        let decision = match brain.decide_engineer_lifecycle(&ctx) {
            Ok(d) => d,
            Err(e) => {
                let prior_failures = lock_state(state)
                    .goal_failure_counts
                    .get(goal_id)
                    .copied()
                    .unwrap_or(0);
                tracing::error!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    error = %e,
                    prior_consecutive_failures = prior_failures,
                    "brain.decide_engineer_lifecycle FAILED — surfacing as cycle failure (NO silent continue_skipping fallback per issue #1711)",
                );
                eprintln!(
                    "[simard] BRAIN FAILURE goal={} error={} prior_consecutive_failures={}",
                    goal_id, e, prior_failures
                );

                // Deterministic safeguard: with this failure the count
                // becomes prior_failures + 1. Trigger at >= 3.
                if prior_failures >= 2 {
                    let new_count = prior_failures + 1;
                    let title = format!(
                        "OODA brain failing on goal '{}' ({} consecutive cycles)",
                        goal_id, new_count
                    );
                    let body = format!(
                        "The OODA brain has failed to produce an engineer-lifecycle decision \
                         for goal `{}` for {} consecutive cycles.\n\n\
                         Latest error:\n```\n{}\n```\n\n\
                         Simard has marked this goal Blocked pending human review. \
                         Inspect the brain logs and the relevant prompt asset \
                         (`prompt_assets/simard/ooda_brain.md`) before reactivating.\n\n\
                         Triggered by deterministic safeguard in \
                         `src/ooda_actions/advance_goal/spawn.rs` (issue #1711).",
                        goal_id, new_count, e
                    );
                    // Mark goal Blocked deterministically. Issue #1911:
                    // the rendered reason uses the sentinel constants so
                    // the `dispatch_advance_goal` auto-recovery branch
                    // and the `simard goal unblock-all` CLI bulk-clear
                    // can identify safeguard-authored markers and
                    // distinguish them from operator-set, scope-blocked,
                    // dependency-blocked, or subordinate-blocked reasons.
                    {
                        let mut guard = lock_state(state);
                        if let Some(g) = guard
                            .active_goals
                            .active
                            .iter_mut()
                            .find(|g| g.id == goal_id)
                        {
                            g.status = crate::goal_curation::GoalProgress::Blocked(format!(
                                "{BRAIN_FAILURE_BLOCKED_PREFIX}{new_count}{BRAIN_FAILURE_BLOCKED_SUFFIX}"
                            ));
                        }
                    }
                    // File tracking issue. Failure to file is logged but
                    // does NOT swallow the original brain failure.
                    match std::process::Command::new("gh")
                        .args([
                            "issue",
                            "create",
                            "--title",
                            &title,
                            "--body",
                            &body,
                            "--label",
                            "ooda-stuck",
                        ])
                        .output()
                    {
                        Ok(out) if out.status.success() => {
                            eprintln!(
                                "[simard] DETERMINISTIC SAFEGUARD: goal '{}' marked Blocked + tracking issue filed",
                                goal_id
                            );
                        }
                        Ok(out) => {
                            tracing::error!(
                                target: "simard::ooda_brain",
                                goal = %goal_id,
                                stderr = %String::from_utf8_lossy(&out.stderr),
                                "deterministic safeguard: gh issue create FAILED (goal still marked Blocked)",
                            );
                        }
                        Err(io_err) => {
                            tracing::error!(
                                target: "simard::ooda_brain",
                                goal = %goal_id,
                                error = %io_err,
                                "deterministic safeguard: gh process spawn FAILED (goal still marked Blocked)",
                            );
                        }
                    }
                }

                return make_outcome(
                    action,
                    false,
                    format!(
                        "brain failure: {} (prior consecutive failures: {})",
                        e, prior_failures
                    ),
                );
            }
        };
        push_brain_judgment(BrainJudgmentRecord::from_engineer_lifecycle(
            goal_id,
            &decision,
            false,
            crate::ooda_brain::prompt_store::current_version(crate::ooda_brain::ACT_PROMPT_NAME),
        ));
        // Issue #1911 — Site 2 reset.
        // A non-fallback `Ok(decision)` from `decide_engineer_lifecycle`
        // proves the brain is healthy for this goal. Belt-and-suspenders:
        // even though `cycle.rs` will reset the counter when the resulting
        // outcome is `success`, some lifecycle decisions (e.g.
        // `OpenTrackingIssue`) intentionally return `success=false`. Reset
        // here so the safeguard threshold doesn't keep advancing.
        //
        // `apply_lifecycle_decision` performs the slow reclaim/issue side
        // effects under the lock; this is the rare cross-restart branch
        // (a live on-disk engineer for an unassigned goal), not the common
        // spawn path, so brief lock contention here is acceptable.
        let mut guard = lock_state(state);
        guard.goal_failure_counts.remove(goal_id);
        return apply_lifecycle_decision(action, &mut guard, goal_id, &live, decision);
    }

    // Recursion guard. Default current depth = 0 (top-level supervisor).
    let current_depth: u32 = std::env::var("SIMARD_SUBORDINATE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let depth_limit = max_subordinate_depth();
    if depth_limit < u32::MAX && current_depth >= depth_limit {
        eprintln!(
            "[simard] spawn_engineer DENIED for goal '{goal_id}': depth {current_depth} >= limit {depth_limit}"
        );
        return make_outcome(
            action,
            false,
            format!(
                "spawn_engineer denied for goal '{goal_id}': subordinate depth {current_depth} >= configured limit {depth_limit}"
            ),
        );
    }

    // ── RESOURCE-AWARE admission gate (fresh-spawn only) ─────────────────
    //
    // The AIMD scaler bounds engineer COUNT; nothing above this point weighs
    // DISK / build-cache / system load. On a busy host that gap let 40+ cargo
    // build caches pile up and drove disk to 91% → ENOSPC killed recipes.
    // Before allocating another worktree, gather the resource picture and ask
    // the admission brain to reason ADMIT / DEFER / RECLAIM-FIRST — repeated
    // structured thought at every admission, following the same
    // gather→reason→apply pattern as the engineer-lifecycle brain above. The
    // *intelligence* lives in the recipe/prompt; the only deterministic code
    // here is a THIN disk hard-rail inside `judge_and_resolve` that blocks
    // admission when disk% is known to be at/over the ceiling, regardless of
    // what the brain decided (irreversible ENOSPC must never be reachable).
    //
    // Placed AFTER the count cap + depth guard and BEFORE worktree allocation
    // (and before target-repo resolution) so a DEFER grows nothing. Both
    // dispatch call sites inherit this gate. The live-engineer re-attach branch
    // above is exempt — it allocates no new resources.
    {
        let ceiling = configured_ceiling_pct();
        let admission_ctx = gather_resource_admission_ctx(&engineer_state_root, ceiling);
        match judge_and_resolve(admission, &admission_ctx) {
            Ok(AdmissionGate::Proceed) => {
                // Admitted — emit an observability record and fall through to
                // the normal spawn path below.
                push_brain_judgment(BrainJudgmentRecord::from_admission(
                    goal_id,
                    "admit",
                    "resources healthy; admitting fresh engineer",
                    "",
                ));
            }
            Ok(AdmissionGate::Defer { reason }) => {
                // Benign skip: NO worktree, NO failure-count bump (success=true,
                // see cycle.rs). The goal is simply retried next cycle. A
                // resource defer is neither progress nor failure.
                tracing::info!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    reason = %reason,
                    "resource-aware admission: DEFER (benign skip — no worktree, no failure)",
                );
                push_brain_judgment(BrainJudgmentRecord::from_admission(
                    goal_id, "defer", &reason, "",
                ));
                return make_outcome(
                    action,
                    true,
                    format!("deferred: resource pressure: {reason}"),
                );
            }
            Ok(AdmissionGate::Reclaim { reason }) => {
                // Reclaim disk first (reuse the existing disk-health recipe),
                // then DEFER this cycle and re-evaluate next cycle. Still a
                // benign skip (no worktree, no failure-count bump). A reclaim
                // failure is logged but never turned into a cycle failure — we
                // defer regardless so a deferred cycle grows nothing.
                tracing::warn!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    reason = %reason,
                    "resource-aware admission: RECLAIM-FIRST (running disk-health reclaim, then defer)",
                );
                push_brain_judgment(BrainJudgmentRecord::from_admission(
                    goal_id, "reclaim", &reason, "",
                ));
                match crate::disk_health::run_disk_health_check(
                    repo_root,
                    &engineer_state_root,
                    None,
                ) {
                    Ok(report) => {
                        tracing::info!(
                            target: "simard::ooda_brain",
                            goal = %goal_id,
                            "disk-health reclaim completed; deferring this cycle to re-evaluate next cycle: {}",
                            report.summary(),
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "simard::ooda_brain",
                            goal = %goal_id,
                            error = %e,
                            "disk-health reclaim recipe failed (still deferring this cycle)",
                        );
                    }
                }
                return make_outcome(action, true, format!("deferred: reclaim-first: {reason}"));
            }
            Err(e) => {
                // NO FALLBACK: a broken admission brain must surface as a
                // visible cycle failure, never a silent phantom admit that
                // could fill the disk (mirrors the lifecycle NO-FALLBACK
                // contract). success=false → cycle.rs bumps the failure count.
                tracing::error!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    error = %e,
                    "resource-aware admission brain FAILED — surfacing as cycle failure (NO silent admit)",
                );
                eprintln!("[simard] RESOURCE-ADMISSION BRAIN FAILURE goal={goal_id} error={e}");
                return make_outcome(
                    action,
                    false,
                    format!("resource-admission brain failure: {e}"),
                );
            }
        }
    }

    let agent_name = build_engineer_name(goal_id);

    // Issue #2359 (BUG 1): route the engineer to the goal's TARGET repo, not
    // the daemon's own working directory. Resolve the goal's `repo` slug to a
    // local git repo path; `None`/"Simard" => the daemon's checkout. A
    // missing or invalid target repo is a hard failure — we NEVER silently
    // fall back to Simard (the original bug), which would open PRs in the
    // wrong repository.
    let goal_repo_slug = lock_state(state)
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .and_then(|g| g.repo.clone());
    let parent_repo = match super::repo_resolver::resolve_goal_repo(goal_repo_slug.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            let target = goal_repo_slug.as_deref().unwrap_or("Simard");
            let reason = format!(
                "target repo '{target}' could not be resolved: {e}; clone it under ~/src/ or correct the goal's repo, then `simard goal unblock {goal_id}`"
            );
            eprintln!("[simard] spawn_engineer FAILED for goal '{goal_id}': {reason}");
            // Fail loud: mark the goal Blocked (a plain operator-set block — no
            // OODA-SAFEGUARD sentinel, so `goal unblock-all` won't clear it)
            // rather than silently editing Simard. No worktree, no assignment;
            // the goal is parked until the operator makes the repo available.
            {
                let mut guard = lock_state(state);
                if let Some(g) = guard
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = crate::goal_curation::GoalProgress::Blocked(reason.clone());
                }
            }
            return make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': {reason}"),
            );
        }
    };

    // Allocate a per-engineer git worktree (issue #1197) so concurrent
    // engineers never share the same checkout. The worktree lives under
    // `<state_root>/engineer-worktrees/` and is cleaned up when the
    // subordinate is reaped (or via Drop as a safety net).
    let state_root = engineer_worktree_state_root();
    let worktree = match crate::engineer_worktree::EngineerWorktree::allocate(
        &parent_repo,
        &state_root,
        goal_id,
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!(
                "[simard] spawn_engineer FAILED for goal '{goal_id}': worktree allocation: {e}"
            );
            return make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': worktree allocation: {e}"),
            );
        }
    };
    let worktree_path = worktree.path().to_path_buf();

    // Name the resolved target repo in the engineer's objective so the agent's
    // plan and any explicit `gh` commands operate against the correct repo
    // (issue #2359, BUG 1). The worktree's git remote is already authoritative
    // for implicit `gh pr` calls; this makes the target explicit in the prompt.
    let target_repo_label = goal_repo_slug.as_deref().unwrap_or("Simard");
    let engineer_task = format!(
        "{task}\n\n[target repo: {target_repo_label} — work in this worktree at {}; open any PRs against this repo, not Simard]",
        worktree_path.display()
    );

    let config = SubordinateConfig {
        agent_name: agent_name.clone(),
        goal: engineer_task,
        role: AgentRole::Engineer,
        worktree_path,
        current_depth,
    };

    // Freshness gate (issue #439): ensure the installed amplihack-rs is current
    // before launching the engineer, so it runs on the LATEST recipes,
    // recipe-runner, and SDK adapters (a stale bundle once carried per-step
    // agent timeouts that killed working steps). The gate is serialized +
    // TTL-deduped across a burst of spawners, so it never rebuilds redundantly.
    // A failed update is surfaced (warn/error log + `amplihack_update_failure`
    // metric), and by default we still spawn on the last-known-good install —
    // honest, surfaced degradation, not a silent fallback. Under
    // `SIMARD_REQUIRE_FRESH_AMPLIHACK=1` a failed update blocks this spawn with
    // an explicit error instead. The just-created worktree guard drops (cleans
    // up) on the block path.
    let freshness = crate::amplihack_freshness_gate::ensure_amplihack_fresh();
    if !freshness.should_spawn() {
        return make_outcome(
            action,
            false,
            format!(
                "engineer spawn for goal '{goal_id}' blocked: `amplihack update` failed and \
                 SIMARD_REQUIRE_FRESH_AMPLIHACK=1 requires a fresh amplihack-rs install",
            ),
        );
    }

    match spawn_subordinate(&config) {
        Ok(handle) => {
            // Record the assignment + worktree ownership under one short
            // critical section so subsequent cycles take the heartbeat path
            // instead of re-spawning, and the reaper can clean up the
            // worktree after the subordinate exits (Drop is the safety net).
            {
                let mut guard = lock_state(state);
                if let Some(g) = guard
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.assigned_to = Some(agent_name.clone());
                }
                guard
                    .engineer_worktrees
                    .insert(goal_id.to_string(), worktree);
            }
            // WS-2: persist the tmux session into the dashboard registry so
            // the Recent Actions feed can render Attach deep-links. Failures
            // are logged but never block subagent execution.
            if !handle.session_name.is_empty() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let record = crate::subagent_sessions::SubagentSession {
                    agent_id: agent_name.clone(),
                    session_name: handle.session_name.clone(),
                    host: "local".to_string(),
                    pid: handle.pid,
                    created_at: now,
                    ended_at: None,
                    goal_id: goal_id.to_string(),
                };
                if let Err(e) = crate::subagent_sessions::record_spawn(record) {
                    tracing::warn!(
                        target: "simard::subagent_sessions",
                        agent = %agent_name,
                        session = %handle.session_name,
                        error = %e,
                        "failed to persist subagent session registry entry; spawn proceeds",
                    );
                }
            }

            eprintln!(
                "[simard] spawn_engineer dispatched: goal='{goal_id}', agent='{agent_name}', pid={}",
                handle.pid,
            );
            make_outcome(
                action,
                true,
                format!(
                    "spawn_engineer dispatched: agent='{agent_name}', task='{}' (goal '{goal_id}', pid={})",
                    truncate_for_log(task),
                    handle.pid,
                ),
            )
        }
        Err(e) => {
            // Explicitly cleanup the worktree we just allocated; Drop is the
            // safety net but explicit cleanup gives observable failure logs.
            if let Err(ce) = worktree.cleanup() {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    goal = %goal_id,
                    error = %ce,
                    "explicit worktree cleanup after spawn failure failed",
                );
            }
            eprintln!("[simard] spawn_engineer FAILED for goal '{goal_id}': {e}");
            make_outcome(
                action,
                false,
                format!("spawn_engineer failed for goal '{goal_id}': {e}"),
            )
        }
    }
}

/// Resolve the supervisor state root for engineer worktrees.
///
/// Honors `SIMARD_STATE_ROOT` then falls back to `$HOME/.simard`, matching
/// the supervisor's own resolution to keep all per-engineer state in a
/// single discoverable tree.
fn engineer_worktree_state_root() -> std::path::PathBuf {
    std::env::var("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/home/azureuser".to_string());
            std::path::PathBuf::from(home).join(".simard")
        })
}

/// Scan `<state_root>/engineer-worktrees/` for any directory whose name
/// starts with `<goal_id>-` and whose `.simard-engineer-claim` sentinel
/// names a live PID. Returns the first such path, or None if no live
/// engineer is currently pursuing this goal.
///
/// This is a defense-in-depth check used by `dispatch_spawn_engineer`
/// to prevent duplicate engineer subprocesses on the same goal across
/// daemon restarts (see issue #1227). Stateless: relies only on the
/// on-disk worktree dir and the per-worktree PID sentinel introduced
/// by issue #1213.
pub fn find_live_engineer_for_goal(
    state_root: &std::path::Path,
    goal_id: &str,
) -> Option<std::path::PathBuf> {
    let worktrees_root = state_root.join(crate::engineer_worktree::WORKTREES_SUBDIR);
    let entries = std::fs::read_dir(&worktrees_root).ok()?;
    let prefix = format!("{goal_id}-");
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(&prefix) {
            continue;
        }
        let claim_path = path.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
        let raw = match std::fs::read_to_string(&claim_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Sentinel format (issue #1238): `<pid>\n<starttime>\n` (starttime
        // optional for backwards compat with pre-#1238 sentinels).
        let mut lines = raw.lines();
        let pid: i32 = match lines.next().and_then(|s| s.trim().parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let recorded_starttime: Option<u64> = lines.next().and_then(|s| s.trim().parse().ok());
        if !crate::engineer_worktree::is_pid_alive_public(pid) {
            continue;
        }
        // Starttime guard: if the sentinel records a starttime, it must
        // still match the live process. Mismatch → recycled PID, treat as
        // dead. Pre-#1238 sentinels have no starttime → fall back to PID-only.
        if let Some(recorded) = recorded_starttime {
            match crate::engineer_worktree::read_pid_starttime_public(pid) {
                Some(current) if current == recorded => {}
                _ => continue,
            }
        }
        return Some(path);
    }
    None
}

/// Build a unique subordinate agent name for a goal.
///
/// The epoch suffix prevents collisions when a goal's previous engineer
/// died and a fresh one needs to be spawned in the same process.
fn build_engineer_name(goal_id: &str) -> String {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("engineer-{goal_id}-{epoch}")
}

/// Truncate a user-derived string for safe inclusion in outcome detail / logs.
fn truncate_for_log(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// Apply a brain decision at the engineer-lifecycle skip site (issue #1266).
///
/// Wraps the pure state mutation in `apply_decision_to_state` with the IO
/// side effects each variant requires: numeric kill of the sentinel pid +
/// `git worktree remove` for `ReclaimAndRedispatch`, `gh issue create` for
/// `OpenTrackingIssue`. `success` is `true` only for `ContinueSkipping` so
/// every other branch lets the existing FAILURE_PENALTY engage in the next
/// orient phase (see `src/ooda_loop/orient.rs:12`).
fn apply_lifecycle_decision(
    action: &PlannedAction,
    state: &mut OodaState,
    goal_id: &str,
    live_worktree: &std::path::Path,
    decision: EngineerLifecycleDecision,
) -> ActionOutcome {
    let success = matches!(
        decision,
        EngineerLifecycleDecision::ContinueSkipping { .. }
            | EngineerLifecycleDecision::ConsiderSelfUpdate { .. }
    );

    if let EngineerLifecycleDecision::ReclaimAndRedispatch { .. } = &decision {
        if let Some(pid) = read_sentinel_pid(live_worktree)
            && let Err(e) = numeric_kill(pid)
        {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                pid,
                error = %e,
                "reclaim_and_redispatch: failed to kill engineer pid",
            );
        }
        if let Err(e) = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(live_worktree)
            .status()
        {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                worktree = %live_worktree.display(),
                error = %e,
                "reclaim_and_redispatch: git worktree remove failed",
            );
        }
    }

    if let EngineerLifecycleDecision::OpenTrackingIssue { title, body, .. } = &decision {
        let result = std::process::Command::new("gh")
            .args([
                "issue",
                "create",
                "--title",
                title,
                "--body",
                body,
                "--label",
                "ooda-stuck",
            ])
            .status();
        if let Err(e) = result {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                error = %e,
                "open_tracking_issue: gh issue create failed",
            );
        }
    }

    if let EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } = &decision {
        // Re-validate the safety predicate at action time. The engineer-
        // lifecycle brain is invoked while AT LEAST ONE engineer (the one
        // we are inspecting) is live for this goal — calling
        // `simard safe-update` now would block in the drain phase. Honour
        // the choice as a recorded judgment but defer the actual update.
        //
        // Future PR: a non-engineer-lifecycle brain site can call this same
        // dispatch path when `count_live_engineer_claims == 0`, at which
        // point the act-phase will spawn the orchestrator.
        let state_root = engineer_worktree_state_root();
        let live = crate::ooda_brain::count_live_engineer_claims(&state_root);
        if live > 0 {
            tracing::info!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                live_engineer_count = live,
                rationale = %rationale,
                "consider_self_update deferred: engineers in flight",
            );
        } else {
            // Spawn `simard safe-update` as a detached child process so
            // the daemon can finish the current cycle cleanly while the
            // orchestrator drives drain → snapshot → pretest → swap+exec
            // independently. We deliberately do NOT call
            // `safe_update::SafeUpdateOrchestrator::run()` inline because
            // its `swap` phase exec()s into the new binary — replacing
            // the still-running cycle mid-flight would corrupt state.
            let bin =
                std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("simard"));
            let result = std::process::Command::new(bin)
                .arg("safe-update")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn();
            match result {
                Ok(child) => tracing::info!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    pid = child.id(),
                    rationale = %rationale,
                    "consider_self_update: spawned `simard safe-update` (detached)",
                ),
                Err(e) => tracing::warn!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    error = %e,
                    "consider_self_update: failed to spawn `simard safe-update`",
                ),
            }
        }
    }

    let detail = apply_decision_to_state(&decision, state, goal_id);
    make_outcome(action, success, detail)
}

/// Read the sentinel pid file written by the engineer-worktree allocator.
/// Returns `None` if the file is missing or unparseable.
fn read_sentinel_pid(worktree: &std::path::Path) -> Option<i32> {
    let claim = worktree.join(crate::engineer_worktree::ENGINEER_CLAIM_FILE);
    let raw = std::fs::read_to_string(claim).ok()?;
    raw.lines().next()?.trim().parse().ok()
}

/// Numeric SIGTERM via `libc::kill`. Per repo shell policy and the #1266
/// spec we never shell out to name-based process terminators.
fn numeric_kill(pid: i32) -> std::io::Result<()> {
    // SAFETY: libc::kill is FFI but the call is well-defined for any i32.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

// ─────────────────────────── Tests ───────────────────────────
//
// Unit coverage for the deterministic *private* helpers in this module that
// previously had none: `build_engineer_name`, `truncate_for_log`, and
// `read_sentinel_pid`. Being module-private, these can only be exercised from
// an in-file `#[cfg(test)]` block. Every test is hermetic — pure string logic
// or a `tempfile` sentinel — and never mutates process-global state, so they
// are safe under cargo's default parallel runner.
//
// The public surface (`is_brain_failure_marker`, `dispatch_spawn_engineer`,
// and `find_live_engineer_for_goal` including its starttime-guard branches) is
// covered by `src/ooda_actions/tests_advance_goal.rs`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineer_worktree::ENGINEER_CLAIM_FILE;
    use std::fs;
    use std::path::Path;

    // ── build_engineer_name ────────────────────────────────────────────────

    #[test]
    fn build_engineer_name_has_prefix_and_epoch_suffix() {
        let name = build_engineer_name("improve-test-coverage");
        let suffix = name
            .strip_prefix("engineer-improve-test-coverage-")
            .expect("name must start with `engineer-<goal>-`");
        // The trailing component is a unix-epoch second count.
        suffix
            .parse::<u64>()
            .expect("epoch suffix must parse as u64");
    }

    #[test]
    fn build_engineer_name_handles_empty_goal_id() {
        // Degenerate but must not panic: empty goal id yields the double dash.
        let name = build_engineer_name("");
        assert!(
            name.starts_with("engineer--"),
            "empty goal id should render `engineer--<epoch>`, got {name:?}"
        );
        name.strip_prefix("engineer--")
            .and_then(|s| s.parse::<u64>().ok())
            .expect("epoch suffix must parse even with empty goal id");
    }

    // ── truncate_for_log ───────────────────────────────────────────────────

    #[test]
    fn truncate_for_log_passes_short_strings_through_unchanged() {
        let s = "a short, safe log line";
        assert_eq!(truncate_for_log(s), s);
    }

    #[test]
    fn truncate_for_log_passes_exactly_max_length_unchanged() {
        // 256 ASCII bytes is the inclusive boundary — must be returned as-is
        // with no ellipsis appended.
        let s = "x".repeat(256);
        let out = truncate_for_log(&s);
        assert_eq!(out, s);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn truncate_for_log_truncates_and_appends_ellipsis_for_ascii() {
        let s = "y".repeat(300);
        let out = truncate_for_log(&s);
        assert!(out.ends_with('…'), "truncated output must end with U+2026");
        // 256 retained bytes + 3-byte ellipsis.
        assert_eq!(out.len(), 256 + '…'.len_utf8());
        assert_eq!(out.chars().count(), 257);
    }

    #[test]
    fn truncate_for_log_respects_utf8_char_boundaries() {
        // '€' is 3 bytes; byte index 256 falls in the middle of a char, so the
        // cut must back off to byte 255 (85 whole chars) rather than panic.
        let s = "€".repeat(100);
        assert_eq!(s.len(), 300);
        let out = truncate_for_log(&s);
        assert!(out.ends_with('…'));
        // 85 retained '€' chars + the ellipsis. The result is valid UTF-8 by
        // construction (it is a `String`); the assertion guards the off-by-one
        // boundary back-off in the loop.
        assert_eq!(out.chars().count(), 86);
        assert_eq!(out, format!("{}…", "€".repeat(85)));
    }

    // ── read_sentinel_pid ──────────────────────────────────────────────────

    fn write_claim(dir: &Path, contents: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(ENGINEER_CLAIM_FILE), contents).unwrap();
    }

    #[test]
    fn read_sentinel_pid_reads_first_line_with_starttime() {
        let tmp = tempfile::tempdir().unwrap();
        write_claim(tmp.path(), "12345\n9876543\n");
        assert_eq!(read_sentinel_pid(tmp.path()), Some(12345));
    }

    #[test]
    fn read_sentinel_pid_reads_pid_only_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        write_claim(tmp.path(), "777\n");
        assert_eq!(read_sentinel_pid(tmp.path()), Some(777));
    }

    #[test]
    fn read_sentinel_pid_trims_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        write_claim(tmp.path(), "  42  \n");
        assert_eq!(read_sentinel_pid(tmp.path()), Some(42));
    }

    #[test]
    fn read_sentinel_pid_missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_sentinel_pid(tmp.path()), None);
    }

    #[test]
    fn read_sentinel_pid_empty_or_unparseable_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        write_claim(tmp.path(), "");
        assert_eq!(read_sentinel_pid(tmp.path()), None);

        let tmp2 = tempfile::tempdir().unwrap();
        write_claim(tmp2.path(), "not-a-pid\n0\n");
        assert_eq!(read_sentinel_pid(tmp2.path()), None);
    }
}
