//! AdvanceGoal dispatch — routing, subordinate heartbeat, and session-based advancement.

use std::path::Path;
use std::process::Output;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::agent_roles::AgentRole;
use crate::agent_supervisor::{SubordinateConfig, spawn_subordinate};
use crate::identity_composition::max_subordinate_depth;
use crate::ooda_brain::{
    BrainJudgmentRecord, EngineerLifecycleDecision, OodaBrain, apply_decision_to_state,
    gather_engineer_lifecycle_ctx, push_brain_judgment,
};
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};
use crate::stewardship::gh_client::{LabelDisposition, OODA_STUCK_LABEL, ensure_label};

use crate::ooda_actions::make_outcome;

use super::admission;
use super::resource_admission;

/// Build the engineer-lifecycle tracking-issue `gh issue create` argv, attaching
/// `--label ooda-stuck` only when the label was ensured
/// ([`LabelDisposition::Attach`]). A degraded [`LabelDisposition::Omit`] drops
/// it so the issue is still filed (issue #4474).
fn ooda_stuck_issue_argv<'a>(
    title: &'a str,
    body: &'a str,
    label: &LabelDisposition,
) -> Vec<&'a str> {
    let mut argv = vec!["issue", "create", "--title", title, "--body", body];
    argv.extend(label.label_args(OODA_STUCK_LABEL));
    argv
}

/// Inspect a completed `gh issue create` [`Output`], returning `Some(note)` with
/// a bounded, lossy-decoded stderr when the process exited non-zero, or `None`
/// on success. Replaces the prior `.status()` call at the `open_tracking_issue`
/// site that silently swallowed a non-zero exit — e.g. the `ooda-stuck`
/// label-not-found failure of issue #4474.
fn tracking_issue_failure_note(output: &Output) -> Option<String> {
    if output.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Some(format!(
        "`gh issue create` exited {}: {}",
        output.status,
        truncate_for_log(stderr.trim())
    ))
}

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

/// Derive a STABLE goal-session id from a goal id (issue #4197).
///
/// The previous `format!("ooda-{}", Uuid::now_v7())` minted a FRESH session id
/// every tick, so a terminal recorded on one tick — keyed by
/// `(session_id, cycle_id)` — could never be read back on the next, and the goal
/// was perpetually re-surfaced as blocked and re-escalated. Deriving the session
/// id deterministically from the goal identity makes the same goal map to the
/// same session id across ticks and process restarts, so
/// `CapabilityHandler::terminal_for_session` can recognise a completed session.
///
/// The result is always a valid ledger identifier: `ooda-` followed by 32 hex
/// characters of the SHA-256 of the goal id (37 chars total, `[a-z0-9-]`), so it
/// satisfies `validate_identifier` even when the goal id itself contains
/// characters that would need sanitising — and distinct goals never collide onto
/// the same session id (hashing preserves distinctness that naive sanitising
/// would erase).
pub fn derive_session_id(goal_id: &str) -> String {
    let digest = Sha256::digest(goal_id.as_bytes());
    let mut hex = String::with_capacity(32);
    for byte in &digest[..16] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("ooda-{hex}")
}

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

/// Observe-only dispatch floor (issue #1, Crocutus).
///
/// Returns `Some(refusal_outcome)` when the process runs under a read-only
/// identity (`SIMARD_OBSERVE_ONLY` truthy), signalling that no write-bearing
/// engineer may be dispatched; returns `None` for the ordinary engineer
/// identity so dispatch proceeds unchanged. Extracted as a pure function so the
/// short-circuit is unit-testable without constructing a full brain/session.
///
/// The outcome is `success = true`: for an observer, *declining to act* is the
/// correct behaviour, not a failure — it must not bump goal-failure counters or
/// trip the no-progress breaker.
fn observe_only_dispatch_refusal(action: &PlannedAction, goal_id: &str) -> Option<ActionOutcome> {
    if !crate::read_only_guard::observe_only_enabled() {
        return None;
    }
    Some(make_outcome(
        action,
        true,
        format!(
            "observe-only: refused to dispatch a write-bearing engineer for goal '{goal_id}'. \
             This read-only identity proposes repo-hygiene goals but dispatches 0 write \
             actions (no clone-and-push, no PR). Guardrail: SIMARD_OBSERVE_ONLY."
        ),
    ))
}

/// Deterministic spawn rail (#3125) — the thin, pure, default-DENY predicate the
/// agentic Act cognition runs behind.
///
/// Returns `true` only for a definitively *writing* posture (`Full` /
/// `ScopedWrite`). Semantics:
///
/// - `None` — no identity is resolved. This deterministically resolves to
///   `Full` (Simard's own default), so Simard is unaffected and spawns proceed.
/// - `Some(Full | ScopedWrite)` — a writing identity: spawn permitted.
/// - `Some(ReadOnly)` — a bounded observer: spawn denied.
///
/// An *unresolved* posture under a named identity must be encoded by the caller
/// as `Some(IdentityAuthority::read_only())` so it too denies — the rail never
/// spawns when authority is uncertain (fail-closed). There is no wall-clock
/// timeout and no fallback-to-dispatch anywhere on this path.
///
/// This mirrors the existing `dispatch_spawn_engineer` pattern of an agentic
/// brain decision paired with a deterministic safeguard: here the safeguard is
/// this predicate rather than the 3-strikes counter.
pub fn posture_permits_spawn(authority: Option<&crate::identity::IdentityAuthority>) -> bool {
    match authority {
        None => true,
        Some(a) => a.permits_spawn(),
    }
}

/// Cognition-level observe-only refusal (#3125). Returns `Some(outcome)` when the
/// resolved identity's write-authority posture forbids dispatching a
/// write-bearing engineer, so the Act phase takes the observe-only branch
/// *before* ever reaching the shipped `observe_only_dispatch_refusal` floor.
///
/// The outcome is `success = true`: for a read-only observer, *declining to
/// dispatch* is the correct behaviour — it must not bump goal-failure counters
/// or trip the no-progress breaker. Records which identity/posture refused.
fn posture_observe_only_refusal(
    action: &PlannedAction,
    goal_id: &str,
    authority: Option<&crate::identity::IdentityAuthority>,
    identity_name: Option<&str>,
) -> Option<ActionOutcome> {
    if posture_permits_spawn(authority) {
        return None;
    }
    let who = identity_name.unwrap_or("identity");
    let posture = authority
        .map(|a| a.posture.to_string())
        .unwrap_or_else(|| "read-only".to_string());
    Some(make_outcome(
        action,
        true,
        format!(
            "observe-only (cognition): {who} write-authority posture is '{posture}', so goal \
             '{goal_id}' takes the observe-only branch — proposes/observes on its own \
             target-scoped board and dispatches 0 engineer(s). No write-bearing engineer is \
             spawned. Rail: posture_permits_spawn."
        ),
    ))
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
/// `repo_root` is the DAEMON's own repository root (from `OodaClients`), used by
/// the resource-admission gate's `reclaim_first` path to locate the disk-health
/// recipe (issue #2706) — distinct from the goal's resolved target repo.
pub fn dispatch_spawn_engineer(
    action: &PlannedAction,
    state: &Mutex<&mut OodaState>,
    goal_id: &str,
    task: &str,
    brain: &dyn OodaBrain,
    repo_root: &Path,
) -> ActionOutcome {
    // ── Cognition-level observe-only rail (#3125) ───────────────────────────
    // Defense in depth ABOVE the shipped write-primitive floor below. If the
    // resolved identity's write-authority posture does not permit a
    // write-bearing engineer (posture = read-only, or an unresolved posture
    // under a named identity, encoded fail-closed), take the observe-only branch
    // BEFORE any brain decision, worktree, or subprocess: the identity observes
    // and proposes on its own target-scoped board and dispatches 0 engineers.
    // A read-only identity therefore never even *reaches* the env floor for a
    // write-bearing action, saving the credits the brain would burn deciding to
    // spawn. No identity (None) resolves to `full`, so Simard is unaffected.
    {
        let (authority, identity_name) = {
            let guard = lock_state(state);
            (
                guard.identity_cognition.authority.clone(),
                guard.identity_cognition.identity_name.clone(),
            )
        };
        if let Some(refusal) = posture_observe_only_refusal(
            action,
            goal_id,
            authority.as_ref(),
            identity_name.as_deref(),
        ) {
            eprintln!(
                "[simard] Act: observe-only posture ({}) — refusing engineer dispatch for goal '{goal_id}', dispatched 0 engineer(s)",
                identity_name.as_deref().unwrap_or("identity")
            );
            return refusal;
        }
    }

    // ── Observe-only floor (issue #1, Crocutus) ─────────────────────────────
    // A read-only identity (SIMARD_OBSERVE_ONLY=1) is a bounded OBSERVER: it may
    // reason about goals but must never dispatch a write-bearing engineer, which
    // would clone-and-push/PR against a target repo. Short-circuit BEFORE any
    // worktree is allocated or subprocess launched — fail closed. This is the
    // capability layer that makes "proposes goals, changes nothing anywhere"
    // structural, not merely prompt-deep. The engineer identity (env unset) is
    // unaffected.
    if let Some(refusal) = observe_only_dispatch_refusal(action, goal_id) {
        return refusal;
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
    let state_root_inflight = engineer_worktree_state_root();
    if let Some(live) = find_live_engineer_for_goal(&state_root_inflight, goal_id) {
        let ctx = {
            let guard = lock_state(state);
            gather_engineer_lifecycle_ctx(&guard, &state_root_inflight, goal_id, &live)
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
                    // does NOT swallow the original brain failure. Self-heal the
                    // `ooda-stuck` label first; degrade to no label on failure
                    // so the escalation still files (issue #4474).
                    let disposition = ensure_label(OODA_STUCK_LABEL);
                    if let LabelDisposition::Omit { reason } = &disposition {
                        tracing::warn!(
                            target: "simard::ooda_brain",
                            goal = %goal_id,
                            label = OODA_STUCK_LABEL,
                            reason = %reason,
                            "deterministic safeguard: label unensurable; filing tracking issue without it",
                        );
                    }
                    let argv = ooda_stuck_issue_argv(&title, &body, &disposition);
                    match std::process::Command::new("gh").args(&argv).output() {
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

    // Issue #2690: dependency/overlap-aware engineer ADMISSION gate. Runs at the
    // spawn/admission decision point — reached only for a genuinely NEW engineer
    // on a DIFFERENT goal (same-goal single-flight was enforced by the live-
    // engineer branch above; the depth guard and repo-resolve are already past).
    // It reasons about the FILE-FOOTPRINT overlap between this candidate goal and
    // the in-flight engineers and decides Admit / Defer / SerializeAfter. A THIN
    // deterministic exact-path rail blocks a CERTAIN collision regardless of the
    // brain; a brain error fails OPEN (admits). Gather runs OFF the state lock
    // (best-effort `gh`/`git`), so we snapshot the goal under a short lock first.
    //
    // `Defer` reuses the benign spawn-skip outcome shape (`success=true`, no
    // worktree, `goal_failure_counts` untouched) — retried naturally next cycle.
    // `SerializeAfter` threads a rebase-after hint into the engineer `task`.
    let engineer_task_base = {
        let goal_snapshot = lock_state(state)
            .active_goals
            .active
            .iter()
            .find(|g| g.id == goal_id)
            .cloned();
        match goal_snapshot {
            Some(goal) => {
                let state_root_admission = engineer_worktree_state_root();
                match admission::run_admission_gate(
                    &state_root_admission,
                    &goal,
                    &parent_repo,
                    task,
                    brain,
                ) {
                    admission::AdmissionOutcome::Defer { detail } => {
                        eprintln!(
                            "[simard] spawn_engineer deferred for goal '{goal_id}': {detail}"
                        );
                        return make_outcome(action, true, detail);
                    }
                    admission::AdmissionOutcome::Admit { task: augmented } => augmented,
                }
            }
            // Goal vanished from the board between the earlier checks and here —
            // nothing to admit against; fall through with the base task
            // (worktree allocation will surface any real inconsistency).
            None => task.to_string(),
        }
    };
    let task = engineer_task_base.as_str();

    // Issue #2706: resource-aware engineer ADMISSION gate. AFTER the overlap gate
    // and BEFORE worktree allocation. Spawning another engineer allocates a git
    // worktree and runs parallel `cargo` builds; this gate weighs the HOST
    // resource picture (disk %, build-cache / worktree count, load average,
    // in-flight engineers) and decides Admit / Defer / ReclaimFirst. It augments
    // the upstream AIMD COUNT control with resource ADMISSION: count-control is
    // blind to the disk that piled-up parallel builds consume (the 91% ENOSPC
    // incident). A THIN deterministic disk-ceiling rail BLOCKS a spawn past a
    // configurable ceiling regardless of the brain (the ENOSPC guard); a brain
    // error fails CLOSED (defers). The gate runs OFF the state lock.
    //
    // `Defer`/`ReclaimFirst` reuse the benign spawn-skip outcome (`success=true`,
    // no worktree, `goal_failure_counts` untouched) — retried naturally next
    // cycle. `ReclaimFirst` runs the disk-reclaim capability HERE (in the caller,
    // which owns the daemon `repo_root` the reclaim recipe needs — the reclaim
    // recipe belongs to Simard, not the goal's resolved target repo), then defers.
    {
        let state_root_resource = engineer_worktree_state_root();
        match resource_admission::run_resource_admission_gate(
            &state_root_resource,
            goal_id,
            brain,
            &crate::disk_pressure::RealDiskStatProvider,
        ) {
            resource_admission::ResourceAdmissionOutcome::Admit => {}
            resource_admission::ResourceAdmissionOutcome::Defer { detail } => {
                eprintln!(
                    "[simard] spawn_engineer resource-deferred for goal '{goal_id}': {detail}"
                );
                return make_outcome(action, true, detail);
            }
            resource_admission::ResourceAdmissionOutcome::ReclaimFirst { detail } => {
                // Best-effort reclaim; a reclaim error is warn-logged, never a
                // cycle failure (issue #2706). `repo_root` is the DAEMON's repo
                // (locates the reclaim recipe's in-tree fallback), not the goal's
                // resolved target repo. Drives the agentic disk-reclaim capability
                // (issue #2704): dry-run + human-review unless the operator opts
                // into `SIMARD_DISK_RECLAIM_DAEMON_APPLY=1`.
                let mode = crate::disk_reclaim::daemon_apply_from_env();
                let target_pct = crate::disk_reclaim::reclaim_pct_from_env();
                if let Err(e) = crate::disk_reclaim::run_disk_reclaim(
                    repo_root,
                    &state_root_resource,
                    None,
                    mode,
                    target_pct,
                    crate::disk_reclaim::ReclaimSource::Daemon,
                ) {
                    tracing::warn!(
                        target: "simard::ooda_brain",
                        goal = %goal_id,
                        error = %e,
                        "resource-admission reclaim_first: disk reclaim failed; deferring anyway",
                    );
                }
                eprintln!("[simard] spawn_engineer reclaim-first for goal '{goal_id}': {detail}");
                return make_outcome(action, true, detail);
            }
        }
    }

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

/// Resolve the typed-OODA ledger state root.
///
/// Single source of truth for the directory that contains the typed-OODA
/// SQLite ledger. Both the spawn-admission path (`typed_goal_session::run`,
/// which opens the ledger for `record_action`) and the engineer-termination
/// release path (`subordinate::cleanup_engineer_worktree_for_goal`) resolve the
/// ledger through this helper, so a released claim always targets the exact
/// ledger the admission gate inserted it into. Lives in `spawn` (compiled in
/// both test and non-test builds) because `typed_goal_session` is
/// `#[cfg(not(test))]`.
pub(crate) fn typed_ooda_state_root() -> std::path::PathBuf {
    std::env::var_os("SIMARD_STATE_ROOT")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("SIMARD_HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("."))
                .join(".simard")
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
        // Self-heal the `ooda-stuck` label before filing; degrade to no label
        // on failure so the tracking issue is still opened (issue #4474).
        let disposition = ensure_label(OODA_STUCK_LABEL);
        if let LabelDisposition::Omit { reason } = &disposition {
            tracing::warn!(
                target: "simard::ooda_brain",
                goal = %goal_id,
                label = OODA_STUCK_LABEL,
                reason = %reason,
                "open_tracking_issue: label unensurable; filing tracking issue without it",
            );
        }
        let argv = ooda_stuck_issue_argv(title, body, &disposition);
        // `.output()` (not `.status()`) so a non-zero exit — e.g. the historic
        // label-not-found failure — is captured and surfaced, never swallowed.
        match std::process::Command::new("gh").args(&argv).output() {
            Ok(out) => {
                if let Some(note) = tracking_issue_failure_note(&out) {
                    tracing::warn!(
                        target: "simard::ooda_brain",
                        goal = %goal_id,
                        detail = %note,
                        "open_tracking_issue: gh issue create failed",
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "simard::ooda_brain",
                    goal = %goal_id,
                    error = %e,
                    "open_tracking_issue: gh issue create failed to spawn",
                );
            }
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

    // ── observe_only_dispatch_refusal (issue #1, Crocutus) ──────────────────
    // These tests mutate the process-global OBSERVE_ONLY_ENV var; they carry the
    // `cognitive_memory` serial key so env mutation is never concurrent with an
    // env read (enforced by test_support::serial_guard).

    fn advance_action(goal_id: &str) -> PlannedAction {
        PlannedAction {
            kind: crate::ooda_loop::ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.to_string()),
            description: format!("advance {goal_id}"),
        }
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn observe_only_refuses_engineer_dispatch_when_enabled() {
        unsafe {
            std::env::set_var(crate::read_only_guard::OBSERVE_ONLY_ENV, "1");
        }
        let action = advance_action("tidy-stale-branches");
        let refusal = observe_only_dispatch_refusal(&action, "tidy-stale-branches");
        unsafe {
            std::env::remove_var(crate::read_only_guard::OBSERVE_ONLY_ENV);
        }
        let outcome = refusal.expect("read-only identity must refuse engineer dispatch");
        // Declining to act is correct behaviour, not a failure.
        assert!(
            outcome.success,
            "observer refusal must not count as a failure"
        );
        assert!(
            outcome.detail.contains("observe-only")
                && outcome.detail.contains("0 write")
                && outcome.detail.contains("SIMARD_OBSERVE_ONLY"),
            "refusal detail must be explicit and auditable, got: {}",
            outcome.detail
        );
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn engineer_identity_dispatches_normally_when_env_unset() {
        unsafe {
            std::env::remove_var(crate::read_only_guard::OBSERVE_ONLY_ENV);
        }
        let action = advance_action("ship-feature");
        assert!(
            observe_only_dispatch_refusal(&action, "ship-feature").is_none(),
            "engineer identity (env unset) must not be short-circuited"
        );
    }

    // ── #3125: cognition-level observe-only rail (posture_permits_spawn) ──────
    // Pure-function coverage plus an end-to-end proof that dispatch_spawn_engineer
    // takes the observe-only branch under a read-only posture WITHOUT ever
    // consulting the brain or spawning a worktree. These are hermetic (no env, no
    // subprocess) — the read-only rail returns before any of that.

    use crate::identity::{IdentityAuthority, WritePosture};

    fn authority(posture: WritePosture) -> IdentityAuthority {
        IdentityAuthority {
            posture,
            ..IdentityAuthority::default()
        }
    }

    #[test]
    fn posture_permits_spawn_default_deny_matrix() {
        // No identity resolves deterministically to `full` => Simard spawns.
        assert!(posture_permits_spawn(None));
        // A writing posture permits spawn.
        assert!(posture_permits_spawn(Some(&authority(WritePosture::Full))));
        assert!(posture_permits_spawn(Some(&authority(
            WritePosture::ScopedWrite
        ))));
        // A read-only posture NEVER permits spawn (fail-closed cognition rail).
        assert!(!posture_permits_spawn(
            Some(&IdentityAuthority::read_only())
        ));
        assert!(!posture_permits_spawn(Some(&authority(
            WritePosture::ReadOnly
        ))));
    }

    #[test]
    fn posture_observe_only_refusal_none_for_writing_postures() {
        let action = advance_action("observe-hyenas");
        assert!(
            posture_observe_only_refusal(&action, "observe-hyenas", None, None).is_none(),
            "no identity (full) must not take the observe-only branch"
        );
        assert!(
            posture_observe_only_refusal(
                &action,
                "observe-hyenas",
                Some(&authority(WritePosture::Full)),
                Some("simard-engineer"),
            )
            .is_none(),
            "a full identity must not take the observe-only branch"
        );
    }

    #[test]
    fn posture_observe_only_refusal_records_read_only_branch() {
        let action = advance_action("observe-hyenas");
        let outcome = posture_observe_only_refusal(
            &action,
            "observe-hyenas",
            Some(&IdentityAuthority::read_only()),
            Some("crocutus"),
        )
        .expect("read-only posture must take the observe-only branch");
        // Declining to dispatch is correct behaviour, not a failure.
        assert!(
            outcome.success,
            "observer refusal must not count as failure"
        );
        assert!(
            outcome.detail.contains("observe-only (cognition)")
                && outcome.detail.contains("crocutus")
                && outcome.detail.contains("read-only")
                && outcome.detail.contains("0 engineer")
                && outcome.detail.contains("posture_permits_spawn"),
            "refusal detail must name the identity, posture, and rail; got: {}",
            outcome.detail
        );
    }

    /// A brain that panics on any decision — proves the read-only rail
    /// short-circuits BEFORE any (credit-spending) brain reasoning.
    struct PanicBrain;

    impl crate::ooda_brain::OodaBrain for PanicBrain {
        fn decide_engineer_lifecycle(
            &self,
            _ctx: &crate::ooda_brain::EngineerLifecycleCtx,
        ) -> crate::error::SimardResult<crate::ooda_brain::EngineerLifecycleDecision> {
            panic!("read-only cognition rail must not consult the brain");
        }

        fn decide_per_goal_cycle(
            &self,
            _ctx: &crate::ooda_brain::PerGoalCycleCtx,
        ) -> crate::error::SimardResult<crate::ooda_brain::PerGoalAction> {
            panic!("read-only cognition rail must not consult the brain");
        }
    }

    #[test]
    fn dispatch_spawn_engineer_read_only_cognition_never_spawns_or_reasons() {
        let cognition = crate::ooda_loop::IdentityCognition {
            identity_name: Some("crocutus".to_string()),
            seed_goals: Vec::new(),
            target_repos: vec!["hyenas".to_string()],
            authority: Some(IdentityAuthority::read_only()),
        };
        let mut state = OodaState::new(crate::goal_curation::GoalBoard::new())
            .with_identity_cognition(cognition);
        let state_mx = std::sync::Mutex::new(&mut state);
        let action = advance_action("observe-hyenas-branch-hygiene");
        let repo_root = tempfile::tempdir().unwrap();

        // If the cognition rail works, PanicBrain is never called and no
        // worktree/subprocess is launched.
        let outcome = dispatch_spawn_engineer(
            &action,
            &state_mx,
            "observe-hyenas-branch-hygiene",
            "propose repo-hygiene goals for hyenas",
            &PanicBrain,
            repo_root.path(),
        );

        assert!(
            outcome.success,
            "observe-only dispatch must be a success outcome, got: {}",
            outcome.detail
        );
        assert!(
            outcome.detail.contains("observe-only (cognition)")
                && outcome.detail.contains("0 engineer"),
            "read-only identity must take the observe-only branch; got: {}",
            outcome.detail
        );
        // No engineer was assigned or worktree registered.
        assert!(state.engineer_worktrees.is_empty());
        assert!(
            state
                .active_goals
                .active
                .iter()
                .all(|g| g.assigned_to.is_none())
        );
    }

    #[test]
    fn identity_cognition_default_permits_spawn_simard_unchanged() {
        // The default carrier (no identity) must permit spawn so Simard's Act
        // phase is byte-for-byte unchanged.
        let cognition = crate::ooda_loop::IdentityCognition::default();
        assert!(cognition.permits_spawn());
        assert!(cognition.authority.is_none());
        let action = advance_action("ship-feature");
        assert!(
            posture_observe_only_refusal(
                &action,
                "ship-feature",
                cognition.authority.as_ref(),
                cognition.identity_name.as_deref(),
            )
            .is_none(),
            "Simard (no identity) must never take the observe-only branch"
        );
    }

    // ── ooda-stuck label self-heal + exit-capture (issue #4474) ────────────
    //
    // Both `gh issue create` sites in this module hardcoded `--label ooda-stuck`.
    // When that label does not exist in the repo, `gh` exits non-zero. These
    // tests pin two contract points of the fix:
    //   1. The label is attached only when it was ensured; otherwise the issue
    //      is still filed (degraded, no label) so the goal is escalated.
    //   2. The `open_tracking_issue` site must inspect the process `Output`
    //      (was `.status()`, which swallowed non-zero exits) and surface a
    //      bounded failure note — never silently drop the failure.
    mod ooda_stuck_label_self_heal {
        use super::super::{ooda_stuck_issue_argv, tracking_issue_failure_note};
        use crate::stewardship::gh_client::{LabelDisposition, OODA_STUCK_LABEL};
        use std::os::unix::process::ExitStatusExt;
        use std::process::{ExitStatus, Output};

        #[test]
        fn argv_carries_label_when_ensured() {
            let argv = ooda_stuck_issue_argv("t", "b", &LabelDisposition::Attach);
            assert!(
                argv.windows(2).any(|w| w == ["--label", OODA_STUCK_LABEL]),
                "an ensured label must be attached",
            );
        }

        #[test]
        fn argv_drops_label_when_degraded_but_still_files() {
            let argv = ooda_stuck_issue_argv(
                "t",
                "b",
                &LabelDisposition::Omit {
                    reason: "no perms".into(),
                },
            );
            assert!(!argv.contains(&"--label"));
            assert_eq!(
                &argv[..6],
                &["issue", "create", "--title", "t", "--body", "b"],
                "the issue must still be filed without the label",
            );
        }

        #[test]
        fn success_output_produces_no_failure_note() {
            let out = Output {
                status: ExitStatus::from_raw(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
            assert!(
                tracking_issue_failure_note(&out).is_none(),
                "a successful `gh issue create` must not emit a failure note",
            );
        }

        #[test]
        fn nonzero_exit_is_surfaced_not_swallowed() {
            // Regression for the latent second bug: the site used `.status()` and
            // only logged the spawn `Err`, silently swallowing a non-zero exit
            // (e.g. the `ooda-stuck` label-not-found failure). `.output()` must
            // capture stderr and surface it in the note.
            let out = Output {
                status: ExitStatus::from_raw(256),
                stdout: Vec::new(),
                stderr: b"could not add label: 'ooda-stuck' not found".to_vec(),
            };
            let note = tracking_issue_failure_note(&out)
                .expect("a non-zero exit must produce a surfaced failure note");
            assert!(
                note.contains("ooda-stuck"),
                "the note must include the gh stderr, got {note:?}",
            );
        }

        #[test]
        fn failure_note_is_bounded_for_huge_stderr() {
            let out = Output {
                status: ExitStatus::from_raw(256),
                stdout: Vec::new(),
                stderr: vec![b'x'; 100_000],
            };
            let note = tracking_issue_failure_note(&out).unwrap();
            assert!(
                note.len() <= 4096,
                "the failure note must be bounded to prevent log flooding, was {}",
                note.len(),
            );
        }
    }
}
