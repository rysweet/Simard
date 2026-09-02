mod agent_spawn;
pub(crate) mod execution;
pub(crate) mod review_persist;
mod types;

#[cfg(test)]
mod tests_agent_spawn;
#[cfg(test)]
mod tests_bounded_memory;
#[cfg(test)]
mod tests_goal_records_migration;
#[cfg(test)]
mod tests_mod;
#[cfg(test)]
mod tests_mod_more;
#[cfg(test)]
mod tests_mod_most;
#[cfg(test)]
mod tests_review_persist;
#[cfg(test)]
mod tests_review_persist_extra;
#[cfg(test)]
mod tests_types;
#[cfg(test)]
mod tests_types_extra;
#[cfg(test)]
mod tests_types_inline;

#[cfg(test)]
mod tests_checkpoint;
#[cfg(test)]
mod tests_claim_sentinel;
#[cfg(test)]
mod tests_meeting_decisions;
#[cfg(test)]
mod tests_resume;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::base_types::BaseTypeId;
use crate::engineer_handoff::{EngineerHandoffContext, SHARED_EXPLICIT_STATE_ROOT_SOURCE};
use crate::error::{SimardError, SimardResult};
use crate::runtime::RuntimeTopology;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};

use execution::{
    parse_status_paths, run_command, run_command_allow_failure, trimmed_stdout,
    trimmed_stdout_allow_empty,
};

// Re-export all public items so `crate::engineer_loop::X` still works.
pub use types::{
    AnalyzedAction, EngineerActionKind, EngineerLoopRun, ExecutedEngineerAction, ExecutionPlan,
    PhaseOutcome, PhaseTrace, RepoInspection, SelectedEngineerAction, SessionCheckpoint,
    SessionErrorReflection, SessionSummary, VerificationReport, analyze_objective,
};

// Phase-entry-point re-exports for the recipe-driven engineer loop (Phase 2 rebuild).
// These let `simard-engineer-step` (in src/bin/) drive each phase via JSON IPC.
pub use agent_spawn::spawn_agent_for_goal;
use review_persist::persist_artifacts_with_session;
pub use review_persist::{
    persist_engineer_loop_artifacts, persist_error_reflection, run_optional_review,
};

// Test-visible re-exports for the integration regression suite that pins the
// Copilot subprocess permission contract (issue #1717,
// `tests/engineer_copilot_permissions.rs`). These helpers are otherwise
// internal to the engineer loop. Kept under `#[doc(hidden)]` so they do not
// appear in user-facing rustdoc and are not treated as a stable surface.
#[doc(hidden)]
pub use agent_spawn::{AgentKind, engineer_argv, run_engineer_subprocess};

pub(crate) const ENGINEER_IDENTITY: &str = "simard-engineer";
pub(crate) const ENGINEER_BASE_TYPE: &str = "terminal-shell";
pub(crate) const EXECUTION_SCOPE: &str = "local-only";
pub(crate) const MAX_CARRIED_MEETING_DECISIONS: usize = 3;
/// Per-scope cap on the number of meeting-related `MemoryRecord` entries that
/// may remain on disk in `memory_records.json` after the engineer loop
/// persists artifacts. When a scope exceeds this cap, the oldest records
/// (FIFO by `(created_at, key)` ascending, with `None` timestamps treated
/// as oldest) are evicted to bring the scope back to `MAX_PERSISTED_MEETING_MEMORY`.
///
/// Currently applied to `MemoryScope::Decision` and `MemoryScope::SessionSummary`
/// only (see `review_persist::persist_engineer_loop_artifacts`). Other scopes
/// — including `SessionScratch` — are intentionally unbounded by this cap.
pub(crate) const MAX_PERSISTED_MEETING_MEMORY: usize = 32;
pub(crate) const GIT_COMMAND_TIMEOUT_SECS: u64 = 60;
pub(crate) const CARGO_COMMAND_TIMEOUT_SECS: u64 = 120;

pub(crate) const CLEARED_GIT_ENV_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

/// Decide whether a loaded checkpoint should be resumed for `objective`.
///
/// A checkpoint is resumable only when it belongs to the same objective (the
/// goal being advanced) AND sits at a mid-session phase boundary that was
/// written before the session completed. The objective match prevents a stale
/// checkpoint left behind by a different goal from contaminating this run.
pub(crate) fn should_resume(checkpoint: &SessionCheckpoint, objective: &str) -> bool {
    checkpoint.objective == objective && checkpoint.is_resumable()
}

pub fn run_local_engineer_loop(
    workspace_root: impl AsRef<Path>,
    objective: &str,
    topology: RuntimeTopology,
    state_root: impl Into<PathBuf>,
) -> SimardResult<EngineerLoopRun> {
    let loop_start = Instant::now();
    let state_root = state_root.into();
    let mut phase_traces = Vec::new();

    // Resume-on-startup (session checkpoint + resume). If a valid checkpoint for
    // THIS objective already exists under the state root, a prior engineer
    // process was interrupted mid-session (crash, restart, or deploy
    // binary-swap). Resume that same session instead of restarting from scratch
    // so no in-progress work is thrown away. Resume is idempotent: every phase
    // whose result was already recorded is reused rather than re-run, which is
    // what stops a completed agent session from being spawned a second time (no
    // double-PR, no duplicate work). A checkpoint whose objective does not match
    // belongs to a different goal and is ignored — this run's own Intake
    // checkpoint overwrites it. Resume complements the deploy-drain goal-requeue:
    // requeue keeps the GOAL safe, this keeps the in-progress SESSION safe.
    let resume: Option<SessionCheckpoint> =
        SessionCheckpoint::load(&state_root).filter(|c| should_resume(c, objective));
    let resumed_at_or_after = |phase: SessionPhase| -> bool {
        resume.as_ref().is_some_and(|c| c.completed_phase >= phase)
    };

    // Restore (resume) or create (fresh) the SessionRecord that tracks the
    // session through the spec's SessionPhase state machine (issue #2100). On
    // resume we keep the ORIGINAL session identity so the resumed run is the
    // same session, not a new one.
    let session_ids = UuidSessionIdGenerator;
    let mut session = match &resume {
        Some(cp) => cp.session_record.clone(),
        None => SessionRecord::new(
            crate::identity::OperatingMode::Engineer,
            objective.to_string(),
            BaseTypeId::new(ENGINEER_BASE_TYPE),
            &session_ids,
        ),
    };
    let session_id_str = session.id.to_string();

    // On resume, hydrate the phase traces the interrupted process recorded and
    // prepend an auditable `resume` marker naming the phase we picked up from.
    if let Some(cp) = &resume {
        phase_traces.push(PhaseTrace {
            name: "resume".to_string(),
            duration: std::time::Duration::ZERO,
            outcome: PhaseOutcome::Skipped(format!(
                "resumed session {} from completed phase {}",
                cp.session_id, cp.completed_phase
            )),
        });
        phase_traces.extend(cp.phase_traces.clone());
    }

    let analyzed = analyze_objective(objective);

    // --- SessionPhase::Intake ---
    // Normalize the request, detect mode, and identify workspace context. A
    // checkpoint is always written at or after Intake, so on resume we reuse the
    // recorded inspection and deliberately skip the pre-mutation dirty-worktree
    // guard: the agent being resumed may have legitimately dirtied the tree.
    let inspection = if let Some(inspection) = resume
        .as_ref()
        .filter(|_| resumed_at_or_after(SessionPhase::Intake))
        .and_then(|c| c.inspection.clone())
    {
        inspection
    } else {
        let phase_start = Instant::now();
        let inspection = inspect_workspace(workspace_root.as_ref(), &state_root);
        let inspection = match &inspection {
            Ok(_) => {
                phase_traces.push(PhaseTrace {
                    name: "inspect".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Success,
                });
                inspection?
            }
            Err(e) => {
                phase_traces.push(PhaseTrace {
                    name: "inspect".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Failed(e.to_string()),
                });
                let err = inspection.unwrap_err();
                let _ = session.advance(SessionPhase::Failed);
                persist_error_reflection(
                    &state_root,
                    &SessionErrorReflection {
                        objective: objective.to_string(),
                        failed_phase: "inspect".to_string(),
                        error_message: err.to_string(),
                        phase_traces: phase_traces.clone(),
                        session_id: Some(session_id_str.clone()),
                    },
                );
                return Err(err);
            }
        };

        // Pre-mutation guard (issue #2082): if the objective implies a mutating
        // action and the working tree has uncommitted changes, abort before
        // spawning the agent. Per spec line 256 the mutating path requires a
        // clean repo.
        if analyzed.is_mutating() && inspection.worktree_dirty {
            let phase_name = "pre-mutation-guard";
            phase_traces.push(PhaseTrace {
                name: phase_name.to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed("dirty worktree".to_string()),
            });
            let err = SimardError::DirtyWorktree {
                changed_files: inspection.changed_files.clone(),
            };
            let _ = session.advance(SessionPhase::Failed);
            persist_error_reflection(
                &state_root,
                &SessionErrorReflection {
                    objective: objective.to_string(),
                    failed_phase: phase_name.to_string(),
                    error_message: err.to_string(),
                    phase_traces: phase_traces.clone(),
                    session_id: Some(session_id_str.clone()),
                },
            );
            return Err(err);
        }

        // Checkpoint after Intake (issue #2095): persist session state so a
        // resuming engineer can skip workspace inspection.
        let checkpoint = SessionCheckpoint {
            session_id: session_id_str.clone(),
            objective: objective.to_string(),
            completed_phase: SessionPhase::Intake,
            inspection: Some(inspection.clone()),
            terminal_handoff_context: None,
            execution_plan: None,
            action: None,
            verification: None,
            session_summary: None,
            phase_traces: phase_traces.clone(),
            session_record: session.clone(),
        };
        if let Err(e) = checkpoint.save(&state_root) {
            tracing::warn!(error = %e, "engineer loop: failed to save intake checkpoint");
        }

        inspection
    };

    // --- SessionPhase::Preparation ---
    // Gather current state, constraints, and existing memory relevant to the task.
    // On resume past Preparation, reuse the memory context the interrupted
    // process already loaded instead of re-running the carryover verification.
    let terminal_handoff_context = if resumed_at_or_after(SessionPhase::Preparation) {
        resume
            .as_ref()
            .and_then(|c| c.terminal_handoff_context.clone())
    } else {
        session.advance(SessionPhase::Preparation)?;

        let phase_start = Instant::now();
        let terminal_handoff_context = EngineerHandoffContext::load_from_state_root(
            &state_root,
            SHARED_EXPLICIT_STATE_ROOT_SOURCE,
        );
        match &terminal_handoff_context {
            Ok(_) => {
                phase_traces.push(PhaseTrace {
                    name: "load-handoff-context".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Success,
                });
            }
            Err(e) => {
                phase_traces.push(PhaseTrace {
                    name: "load-handoff-context".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Failed(e.to_string()),
                });
            }
        }
        let terminal_handoff_context = match terminal_handoff_context {
            Ok(ctx) => ctx,
            Err(e) => {
                let _ = session.advance(SessionPhase::Failed);
                persist_error_reflection(
                    &state_root,
                    &SessionErrorReflection {
                        objective: objective.to_string(),
                        failed_phase: "load-handoff-context".to_string(),
                        error_message: e.to_string(),
                        phase_traces: phase_traces.clone(),
                        session_id: Some(session_id_str.clone()),
                    },
                );
                return Err(e);
            }
        };

        // Goal carryover verification (issue #2092, spec line 665).
        //
        // If a meeting wrote a carryover record to cognitive memory, verify
        // that the engineer session's goal board matches. A drift means
        // goals curated in the meeting may have silently vanished.
        let phase_start = Instant::now();
        match crate::memory_ipc::launch_writer_client(&state_root) {
            Ok(memory) => match crate::goal_curation::load_goal_board(memory.ops()) {
                Ok(board) => {
                    match crate::goal_curation::verify_goal_carryover(&board, memory.ops()) {
                        Ok(crate::goal_curation::CarryoverVerification::Drifted {
                            meeting_id,
                            missing_goal_ids,
                            ..
                        }) => {
                            let msg = format!(
                                "goal carryover drift detected (meeting {meeting_id}): \
                                 goals missing from board: {missing_goal_ids:?}. \
                                 Meeting goals may have been lost due to state-root divergence."
                            );
                            tracing::warn!(
                                meeting_id = %meeting_id,
                                missing = ?missing_goal_ids,
                                "engineer loop: {msg}"
                            );
                            phase_traces.push(PhaseTrace {
                                name: "goal-carryover-verify".to_string(),
                                duration: phase_start.elapsed(),
                                outcome: PhaseOutcome::Failed(msg),
                            });
                        }
                        Ok(crate::goal_curation::CarryoverVerification::Verified {
                            meeting_id,
                            active_goal_count,
                        }) => {
                            tracing::info!(
                                meeting_id = %meeting_id,
                                active_goals = active_goal_count,
                                "engineer loop: goal carryover verified"
                            );
                            phase_traces.push(PhaseTrace {
                                name: "goal-carryover-verify".to_string(),
                                duration: phase_start.elapsed(),
                                outcome: PhaseOutcome::Success,
                            });
                        }
                        Ok(crate::goal_curation::CarryoverVerification::NoRecord) => {
                            tracing::debug!(
                                "engineer loop: no goal carryover record found (first run or no meetings)"
                            );
                            phase_traces.push(PhaseTrace {
                                name: "goal-carryover-verify".to_string(),
                                duration: phase_start.elapsed(),
                                outcome: PhaseOutcome::Success,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "engineer loop: goal carryover verification failed"
                            );
                            phase_traces.push(PhaseTrace {
                                name: "goal-carryover-verify".to_string(),
                                duration: phase_start.elapsed(),
                                outcome: PhaseOutcome::Failed(e.to_string()),
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "engineer loop: could not load goal board for carryover check"
                    );
                    phase_traces.push(PhaseTrace {
                        name: "goal-carryover-verify".to_string(),
                        duration: phase_start.elapsed(),
                        outcome: PhaseOutcome::Failed(e.to_string()),
                    });
                }
            },
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "engineer loop: could not launch memory for carryover check"
                );
                phase_traces.push(PhaseTrace {
                    name: "goal-carryover-verify".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Failed(e.to_string()),
                });
            }
        }

        // Checkpoint after Preparation (issue #2095).
        let checkpoint = SessionCheckpoint {
            session_id: session_id_str.clone(),
            objective: objective.to_string(),
            completed_phase: SessionPhase::Preparation,
            inspection: Some(inspection.clone()),
            terminal_handoff_context: terminal_handoff_context.clone(),
            execution_plan: None,
            action: None,
            verification: None,
            session_summary: None,
            phase_traces: phase_traces.clone(),
            session_record: session.clone(),
        };
        if let Err(e) = checkpoint.save(&state_root) {
            tracing::warn!(error = %e, "engineer loop: failed to save preparation checkpoint");
        }

        terminal_handoff_context
    };

    // --- SessionPhase::Planning ---
    // Produce a bounded plan sized to the task (spec step 3).
    // On resume past Planning, reuse the recorded execution plan; the agent
    // prompt is a pure function of (objective, inspection) so it is rebuilt
    // without side effects when the Execution phase still needs to run.
    let (agent_prompt, execution_plan) = if resumed_at_or_after(SessionPhase::Planning) {
        let plan = resume
            .as_ref()
            .and_then(|c| c.execution_plan.clone())
            .unwrap_or_else(|| form_execution_plan(objective, &analyzed, &inspection));
        (
            agent_spawn::build_agent_prompt(objective, &inspection),
            plan,
        )
    } else {
        session.advance(SessionPhase::Planning)?;

        let phase_start = Instant::now();
        let agent_prompt = agent_spawn::build_agent_prompt(objective, &inspection);
        phase_traces.push(PhaseTrace {
            name: "agent-prompt-build".to_string(),
            duration: phase_start.elapsed(),
            outcome: PhaseOutcome::Success,
        });

        // Form an auditable execution plan as a distinct orchestration primitive.
        let phase_start = Instant::now();
        let execution_plan = form_execution_plan(objective, &analyzed, &inspection);
        phase_traces.push(PhaseTrace {
            name: "plan".to_string(),
            duration: phase_start.elapsed(),
            outcome: PhaseOutcome::Success,
        });

        // Checkpoint after Planning (issue #2095).
        let checkpoint = SessionCheckpoint {
            session_id: session_id_str.clone(),
            objective: objective.to_string(),
            completed_phase: SessionPhase::Planning,
            inspection: Some(inspection.clone()),
            terminal_handoff_context: terminal_handoff_context.clone(),
            execution_plan: Some(execution_plan.clone()),
            action: None,
            verification: None,
            session_summary: None,
            phase_traces: phase_traces.clone(),
            session_record: session.clone(),
        };
        if let Err(e) = checkpoint.save(&state_root) {
            tracing::warn!(error = %e, "engineer loop: failed to save planning checkpoint");
        }

        (agent_prompt, execution_plan)
    };

    // --- SessionPhase::Execution ---
    // Perform shell actions, file changes, and tool calls while recording evidence.
    // On resume, reuse the recorded agent result instead of spawning the agent
    // again. This is the core idempotency guarantee: a completed agent session
    // is never re-run, so resume cannot open a duplicate PR or redo expensive,
    // non-idempotent work.
    let (action, verification) = if let Some((action, verification)) = resume
        .as_ref()
        .and_then(SessionCheckpoint::resumable_execution)
    {
        (action, verification)
    } else {
        // `session.phase` is `Planning` on a fresh run or a resume that stopped
        // at the Planning checkpoint; only advance when we have not already
        // entered Execution (guards a corrupt Execution checkpoint lacking a
        // recorded action).
        if session.phase != SessionPhase::Execution {
            session.advance(SessionPhase::Execution)?;
        }

        // Phase: agent-spawn — start background thread that runs the
        // `amplihack RustyClawd --auto` subprocess. Spawning is infallible
        // here because subprocess errors surface during agent-wait.
        let phase_start = Instant::now();
        let rx = agent_spawn::start_agent_session(agent_prompt, inspection.repo_root.clone());
        phase_traces.push(PhaseTrace {
            name: "agent-spawn".to_string(),
            duration: phase_start.elapsed(),
            outcome: PhaseOutcome::Success,
        });

        // Phase: agent-wait — block until agent session completes
        let phase_start = Instant::now();
        let outcome_summary = agent_spawn::await_agent_session(rx);
        let action = match outcome_summary {
            Ok(summary) => {
                phase_traces.push(PhaseTrace {
                    name: "agent-wait".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Success,
                });
                ExecutedEngineerAction {
                    selected: SelectedEngineerAction {
                        label: "agent-session".to_string(),
                        rationale: format!("Spawned autonomous agent session for: {objective}"),
                        argv: vec![],
                        plan_summary: objective.to_string(),
                        verification_steps: vec![],
                        expected_changed_files: vec![],
                        kind: EngineerActionKind::AgentSession {
                            outcome_summary: summary.clone(),
                        },
                    },
                    exit_code: 0,
                    stdout: summary,
                    stderr: String::new(),
                    changed_files: vec![],
                }
            }
            Err(e) => {
                phase_traces.push(PhaseTrace {
                    name: "agent-wait".to_string(),
                    duration: phase_start.elapsed(),
                    outcome: PhaseOutcome::Failed(e.to_string()),
                });
                let _ = session.advance(SessionPhase::Failed);
                persist_error_reflection(
                    &state_root,
                    &SessionErrorReflection {
                        objective: objective.to_string(),
                        failed_phase: "agent-wait".to_string(),
                        error_message: e.to_string(),
                        phase_traces: phase_traces.clone(),
                        session_id: Some(session_id_str.clone()),
                    },
                );
                return Err(e);
            }
        };

        let verification = verify_agent_spawn_artifacts(&inspection, objective);

        // Checkpoint after Execution (issue #2095): the most critical checkpoint
        // since agent work is expensive and non-idempotent.
        let checkpoint = SessionCheckpoint {
            session_id: session_id_str.clone(),
            objective: objective.to_string(),
            completed_phase: SessionPhase::Execution,
            inspection: Some(inspection.clone()),
            terminal_handoff_context: terminal_handoff_context.clone(),
            execution_plan: Some(execution_plan.clone()),
            action: Some(action.clone()),
            verification: Some(verification.clone()),
            session_summary: None,
            phase_traces: phase_traces.clone(),
            session_record: session.clone(),
        };
        if let Err(e) = checkpoint.save(&state_root) {
            tracing::warn!(error = %e, "engineer loop: failed to save execution checkpoint");
        }

        (action, verification)
    };

    // --- SessionPhase::Reflection ---
    // Compare results against the objective and capture what succeeded/failed.
    session.advance(SessionPhase::Reflection)?;

    // Optional LLM-driven review gate: only runs for mutating actions
    // when an LLM session is available (requires ANTHROPIC_API_KEY).
    let phase_start = Instant::now();
    let review_result = run_optional_review(&inspection, &action);
    match &review_result {
        Ok(()) => {
            phase_traces.push(PhaseTrace {
                name: "review".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "review".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    if let Err(e) = review_result {
        let _ = session.advance(SessionPhase::Failed);
        persist_error_reflection(
            &state_root,
            &SessionErrorReflection {
                objective: objective.to_string(),
                failed_phase: "review".to_string(),
                error_message: e.to_string(),
                phase_traces: phase_traces.clone(),
                session_id: Some(session_id_str.clone()),
            },
        );
        return Err(e);
    }

    // --- SessionPhase::Summarize ---
    // Produce a structured summary of results (spec step 6).
    session.advance(SessionPhase::Summarize)?;

    let phase_start = Instant::now();
    let session_summary = summarize_results(objective, &action, &verification, &inspection);
    phase_traces.push(PhaseTrace {
        name: "summarize".to_string(),
        duration: phase_start.elapsed(),
        outcome: PhaseOutcome::Success,
    });

    // --- SessionPhase::Persistence ---
    // Write session summary, memory updates, and benchmark records.
    session.advance(SessionPhase::Persistence)?;

    let phase_start = Instant::now();
    let persist_result = persist_artifacts_with_session(
        &state_root,
        topology,
        &mut session,
        &inspection,
        &action,
        &verification,
        terminal_handoff_context.as_ref(),
    );
    match &persist_result {
        Ok(()) => {
            phase_traces.push(PhaseTrace {
                name: "persist".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "persist".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    if let Err(e) = persist_result {
        let _ = session.advance(SessionPhase::Failed);
        persist_error_reflection(
            &state_root,
            &SessionErrorReflection {
                objective: objective.to_string(),
                failed_phase: "persist".to_string(),
                error_message: e.to_string(),
                phase_traces: phase_traces.clone(),
                session_id: Some(session_id_str.clone()),
            },
        );
        return Err(e);
    }

    // Session has been advanced to Complete by persist_artifacts_with_session.

    // Clear the checkpoint on successful completion (issue #2095).
    SessionCheckpoint::clear(&state_root);

    Ok(EngineerLoopRun {
        state_root,
        execution_scope: EXECUTION_SCOPE.to_string(),
        inspection,
        plan: Some(execution_plan),
        action,
        verification,
        summary: Some(session_summary),
        terminal_handoff_context,
        elapsed_duration: loop_start.elapsed(),
        phase_traces,
        session_record: Some(session),
    })
}

/// Extract issue/PR numbers referenced in an objective string.
/// Matches patterns like `#1234`, `issue #1234`, `PR #1234`.
fn extract_referenced_numbers(objective: &str) -> Vec<u64> {
    let mut numbers = Vec::new();
    // Simple byte scan for `#` followed by digits.
    let bytes = objective.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start
                && let Ok(n) = objective[start..end].parse::<u64>()
                && !numbers.contains(&n)
            {
                numbers.push(n);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    numbers
}

/// Count status paths that are new relative to a pre-spawn baseline.
fn count_new_status_paths(before: &[String], after: &[String]) -> Vec<String> {
    after
        .iter()
        .filter(|p| !before.contains(p))
        .cloned()
        .collect()
}

/// Drop the Simard-managed claim sentinel (`.simard-engineer-claim`, issue
/// #2621) from a parsed `git status` path list.
///
/// Simard writes this private liveness sentinel into every engineer worktree;
/// it is Simard infra, not a user change, so **every** `git status` consumer in
/// the engineer loop must ignore it. Applied to both `inspect_workspace` (the
/// pre-mutation-guard input) and `verify_agent_spawn_artifacts` (the
/// post-session evidence report) so the sentinel can never surface as a
/// spurious change in either — including the degraded path where the
/// worktree-allocation `.git/info/exclude` append failed (see
/// `engineer_worktree::exclude_engineer_claim`), which is exactly when raw
/// `git status` still lists the sentinel.
fn strip_claim_sentinel(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| path != crate::engineer_worktree::ENGINEER_CLAIM_FILE)
        .collect()
}

/// Synthesize a [`VerificationReport`] from observable side-effects after an
/// agent session completes (issue #1670). Replaces the previous hardcoded
/// `"agent-completed"` status with post-hoc verification that checks:
///
/// 1. New commits since `inspection.head` (via `git rev-list --count`).
/// 2. New/changed files in the worktree (via `git status --short`).
/// 3. Referenced issue/PR metadata (best-effort via `gh`, non-blocking).
///
/// Status is `"verified"` when at least one check finds a measurable artifact;
/// `"unverified"` otherwise.
fn verify_agent_spawn_artifacts(
    inspection: &RepoInspection,
    objective: &str,
) -> VerificationReport {
    let repo_root = &inspection.repo_root;
    let pre_head = &inspection.head;
    let mut checks: Vec<String> = Vec::new();

    // 1. Count new commits since the pre-spawn HEAD.
    let new_commit_count = run_command_allow_failure(
        repo_root,
        &["git", "rev-list", "--count", &format!("{pre_head}..HEAD")],
    )
    .ok()
    .and_then(|out| out.stdout.trim().parse::<u64>().ok())
    .unwrap_or(0);

    if new_commit_count > 0 {
        // Also grab the one-line summaries for the checks list.
        let log_lines = run_command_allow_failure(
            repo_root,
            &["git", "log", "--oneline", &format!("{pre_head}..HEAD")],
        )
        .ok()
        .map(|out| out.stdout.trim().to_string())
        .unwrap_or_default();
        checks.push(format!(
            "git: {new_commit_count} new commit(s) since {pre_head}"
        ));
        if !log_lines.is_empty() {
            // Cap at first 5 lines to keep the report bounded.
            let capped: String = log_lines.lines().take(5).collect::<Vec<_>>().join("; ");
            checks.push(format!("git-log: {capped}"));
        }
    }

    // 2. Detect new/changed files in the worktree.
    let post_status = run_command_allow_failure(
        repo_root,
        &["git", "status", "--short", "--untracked-files=all"],
    )
    .ok()
    // Filter the Simard-managed claim sentinel here too (issue #2621): the
    // pre-spawn baseline (`inspection.changed_files`) is already filtered, so
    // leaving the sentinel in `post_status` would make `count_new_status_paths`
    // (= after \ before) report it as a spurious "new changed file" whenever
    // the `.git/info/exclude` append failed — falsely flipping a no-op session
    // to "verified". Both consumers must strip the sentinel identically.
    .map(|out| strip_claim_sentinel(parse_status_paths(&out.stdout)))
    .unwrap_or_default();

    let new_files = count_new_status_paths(&inspection.changed_files, &post_status);
    if !new_files.is_empty() {
        checks.push(format!(
            "git-status: {} new changed file(s): {}",
            new_files.len(),
            new_files
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    // 3. Best-effort GH metadata probe for referenced issues/PRs.
    let referenced = extract_referenced_numbers(objective);
    for num in referenced.iter().take(3) {
        // Try PR first, then issue. Both are non-blocking.
        if let Some(info) = gh_resource_info(repo_root, "pr", *num) {
            checks.push(format!("gh: PR #{num} {info}"));
        } else if let Some(info) = gh_resource_info(repo_root, "issue", *num) {
            checks.push(format!("gh: issue #{num} {info}"));
        }
    }

    // Determine status: verified if at least one check found a measurable
    // artifact from git (commits or file changes). GH metadata alone is
    // informational and does not flip the status.
    let has_git_evidence = new_commit_count > 0 || !new_files.is_empty();
    let status = if has_git_evidence {
        "verified"
    } else {
        "unverified"
    }
    .to_string();

    let summary = if checks.is_empty() {
        "No observable artifacts detected after agent session".to_string()
    } else {
        checks.join("; ")
    };

    VerificationReport {
        status,
        summary,
        checks,
    }
}

/// Best-effort query of a GH PR or issue. Returns a short info string on
/// success, `None` on any failure (missing `gh` CLI, auth issues, etc.).
fn gh_resource_info(repo_root: &Path, kind: &str, number: u64) -> Option<String> {
    let num_str = number.to_string();
    let result = run_command_allow_failure(
        repo_root,
        &["gh", kind, "view", &num_str, "--json", "state,title"],
    );
    match result {
        Ok(out) => {
            let text = out.stdout.trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        }
        Err(_) => None,
    }
}

/// Form a structured execution plan from the objective and inspection data
/// (spec step 3: "form a short execution plan"). The plan is a separable
/// orchestration primitive that can be audited independently from execution.
fn form_execution_plan(
    objective: &str,
    analyzed: &AnalyzedAction,
    inspection: &RepoInspection,
) -> ExecutionPlan {
    let is_mutating = analyzed.is_mutating();

    let steps = match analyzed {
        AnalyzedAction::ReadOnlyScan => vec![
            "Inspect repository state and gather context".to_string(),
            "Analyze relevant files and code paths".to_string(),
            "Report findings".to_string(),
        ],
        AnalyzedAction::StructuredTextReplace => vec![
            "Identify target files for modification".to_string(),
            "Apply structured text replacements".to_string(),
            "Verify changes compile and pass checks".to_string(),
            "Commit changes".to_string(),
        ],
        AnalyzedAction::CargoTest => vec![
            "Run test suite".to_string(),
            "Collect and report results".to_string(),
        ],
        AnalyzedAction::CreateFile => vec![
            "Create new file with specified content".to_string(),
            "Verify file was created correctly".to_string(),
            "Commit changes".to_string(),
        ],
        AnalyzedAction::AppendToFile => vec![
            "Append content to target file".to_string(),
            "Verify modification".to_string(),
            "Commit changes".to_string(),
        ],
        AnalyzedAction::RunShellCommand => vec![
            "Execute shell command".to_string(),
            "Capture and report output".to_string(),
        ],
        AnalyzedAction::GitCommit => vec![
            "Stage changes".to_string(),
            "Create commit with descriptive message".to_string(),
        ],
        AnalyzedAction::OpenIssue => vec![
            "Compose issue title and body".to_string(),
            "Create issue on repository".to_string(),
        ],
    };

    let risk_level = if !is_mutating {
        "low"
    } else if inspection.worktree_dirty {
        "high"
    } else {
        "medium"
    }
    .to_string();

    ExecutionPlan {
        objective: objective.to_string(),
        steps,
        expected_changed_files: inspection.changed_files.clone(),
        risk_level,
        is_mutating,
    }
}

/// Produce a structured session summary from execution results
/// (spec step 6: "summarize results"). The summary is a separable
/// orchestration primitive that can be audited independently from persistence.
fn summarize_results(
    objective: &str,
    action: &ExecutedEngineerAction,
    verification: &VerificationReport,
    inspection: &RepoInspection,
) -> SessionSummary {
    let outcome = if action.exit_code == 0 && verification.status != "failed" {
        "success"
    } else if action.exit_code == 0 {
        "partial"
    } else {
        "failed"
    }
    .to_string();

    let accomplishment = if action.stdout.len() > 200 {
        // Char-boundary-safe truncation: `&action.stdout[..200]` panics when byte
        // 200 falls inside a multi-byte UTF-8 sequence, and engineer stdout is
        // arbitrary agent output (emoji, box-drawing, accented text) that hits
        // this on the every-action summary path. See util::string_truncate.
        let mut truncated = action.stdout.clone();
        crate::util::string_truncate::truncate_to_char_boundary(&mut truncated, 200);
        truncated.push('…');
        truncated
    } else if action.stdout.is_empty() {
        format!("Completed objective: {objective}")
    } else {
        action.stdout.clone()
    };

    // Extract changed files: prefer the action's tracked changes, fall back
    // to inspection's pre-existing dirty files.
    let changed_files = if action.changed_files.is_empty() {
        inspection.changed_files.clone()
    } else {
        action.changed_files.clone()
    };

    let mut key_decisions = Vec::new();
    key_decisions.push(format!("Action type: {}", action.selected.label));
    if !action.selected.rationale.is_empty() {
        key_decisions.push(format!("Rationale: {}", action.selected.rationale));
    }

    SessionSummary {
        objective: objective.to_string(),
        outcome,
        changed_files,
        key_decisions,
        accomplishment,
    }
}

pub fn inspect_workspace(workspace_root: &Path, state_root: &Path) -> SimardResult<RepoInspection> {
    let workspace_root =
        fs::canonicalize(workspace_root).map_err(|error| SimardError::NotARepo {
            path: workspace_root.to_path_buf(),
            reason: format!("workspace path could not be resolved: {error}"),
        })?;
    let repo_root_output = run_command(&workspace_root, &["git", "rev-parse", "--show-toplevel"])?;
    let repo_root = PathBuf::from(trimmed_stdout(&repo_root_output)?);
    let repo_root = fs::canonicalize(&repo_root).map_err(|error| SimardError::NotARepo {
        path: repo_root,
        reason: format!("git worktree root could not be canonicalized: {error}"),
    })?;

    let branch_output = run_command(&repo_root, &["git", "branch", "--show-current"])?;
    let branch = trimmed_stdout_allow_empty(&branch_output);
    let head = trimmed_stdout(&run_command(&repo_root, &["git", "rev-parse", "HEAD"])?)?;
    let status_output = run_command(
        &repo_root,
        &["git", "status", "--short", "--untracked-files=all"],
    )?;
    // Filter the Simard-managed claim sentinel out of the reported changes
    // (issue #2621). Simard drops `.simard-engineer-claim` into every engineer
    // worktree; it is private infra, not a user change, and must never count
    // toward `worktree_dirty` or the engineer-loop pre-mutation guard would
    // trip on the untracked sentinel in any target repo that doesn't gitignore
    // it — aborting every mutating engineer before the coding agent spawns.
    // Belt-and-suspenders with the `.git/info/exclude` append performed at
    // worktree-allocation time (see `engineer_worktree::exclude_engineer_claim`).
    // Shared with `verify_agent_spawn_artifacts` via `strip_claim_sentinel` so
    // both `git status` consumers ignore the sentinel identically.
    let changed_files = strip_claim_sentinel(parse_status_paths(&status_output.stdout));
    let worktree_dirty = !changed_files.is_empty();
    let active_goals = {
        use crate::goals::GoalStore as _;
        let store = crate::goals::CognitiveMemoryGoalStore::new(state_root.to_path_buf())?;
        store.active_top_goals(5)?
    };
    let carried_meeting_decisions = load_carried_meeting_decisions(state_root)?;

    Ok(RepoInspection {
        workspace_root,
        repo_root: repo_root.clone(),
        branch: if branch.is_empty() {
            "HEAD".to_string()
        } else {
            branch
        },
        head,
        worktree_dirty,
        changed_files,
        active_goals,
        carried_meeting_decisions,
        architecture_gap_summary: architecture_gap_summary(&repo_root)?,
    })
}

mod meeting_decisions;
// re-exported for cfg(test) consumers in engineer_loop/tests_mod_more.rs and tests_mod_most.rs (false-positive of clippy unused_imports on lib pass — see #1405)
#[allow(unused_imports)]
pub(crate) use meeting_decisions::{
    architecture_gap_summary, is_meeting_decision_record, load_carried_meeting_decisions,
};
