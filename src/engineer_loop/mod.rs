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
mod tests_meeting_decisions;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::runtime::RuntimeTopology;
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::terminal_engineer_bridge::{SHARED_EXPLICIT_STATE_ROOT_SOURCE, TerminalBridgeContext};

use execution::{parse_status_paths, run_command, trimmed_stdout, trimmed_stdout_allow_empty};

// Re-export all public items so `crate::engineer_loop::X` still works.
pub use types::{
    AnalyzedAction, EngineerActionKind, EngineerLoopRun, ExecutedEngineerAction, PhaseOutcome,
    PhaseTrace, RepoInspection, SelectedEngineerAction, SessionErrorReflection, VerificationReport,
    analyze_objective,
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

pub fn run_local_engineer_loop(
    workspace_root: impl AsRef<Path>,
    objective: &str,
    topology: RuntimeTopology,
    state_root: impl Into<PathBuf>,
) -> SimardResult<EngineerLoopRun> {
    let loop_start = Instant::now();
    let state_root = state_root.into();
    let mut phase_traces = Vec::new();

    // Create a SessionRecord to track the session through the spec's
    // SessionPhase state machine (issue #2100). The session starts at Intake
    // and advances through Preparation → Planning → Execution → Reflection →
    // Persistence → Complete, mirroring RuntimeKernel::execute_session.
    let session_ids = UuidSessionIdGenerator;
    let mut session = SessionRecord::new(
        crate::identity::OperatingMode::Engineer,
        objective.to_string(),
        BaseTypeId::new(ENGINEER_BASE_TYPE),
        &session_ids,
    );
    let session_id_str = session.id.to_string();

    // --- SessionPhase::Intake ---
    // Normalize the request, detect mode, and identify workspace context.

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
    let analyzed = analyze_objective(objective);
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

    // --- SessionPhase::Preparation ---
    // Gather current state, constraints, and existing memory relevant to the task.
    session.advance(SessionPhase::Preparation)?;

    let phase_start = Instant::now();
    let terminal_bridge_context =
        TerminalBridgeContext::load_from_state_root(&state_root, SHARED_EXPLICIT_STATE_ROOT_SOURCE);
    match &terminal_bridge_context {
        Ok(_) => {
            phase_traces.push(PhaseTrace {
                name: "load-bridge-context".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "load-bridge-context".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    let terminal_bridge_context = match terminal_bridge_context {
        Ok(ctx) => ctx,
        Err(e) => {
            let _ = session.advance(SessionPhase::Failed);
            persist_error_reflection(
                &state_root,
                &SessionErrorReflection {
                    objective: objective.to_string(),
                    failed_phase: "load-bridge-context".to_string(),
                    error_message: e.to_string(),
                    phase_traces: phase_traces.clone(),
                    session_id: Some(session_id_str.clone()),
                },
            );
            return Err(e);
        }
    };

    // --- SessionPhase::Planning ---
    // Produce a bounded plan sized to the task.
    session.advance(SessionPhase::Planning)?;

    let phase_start = Instant::now();
    let agent_prompt = agent_spawn::build_agent_prompt(objective, &inspection);
    phase_traces.push(PhaseTrace {
        name: "agent-prompt-build".to_string(),
        duration: phase_start.elapsed(),
        outcome: PhaseOutcome::Success,
    });

    // --- SessionPhase::Execution ---
    // Perform shell actions, file changes, and tool calls while recording evidence.
    session.advance(SessionPhase::Execution)?;

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

    let verification = VerificationReport {
        status: "agent-completed".to_string(),
        summary: action.stdout.clone(),
        checks: vec![],
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
        terminal_bridge_context.as_ref(),
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

    Ok(EngineerLoopRun {
        state_root,
        execution_scope: EXECUTION_SCOPE.to_string(),
        inspection,
        action,
        verification,
        terminal_bridge_context,
        elapsed_duration: loop_start.elapsed(),
        phase_traces,
        session_record: Some(session),
    })
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
    let changed_files = parse_status_paths(&status_output.stdout);
    let worktree_dirty = !changed_files.is_empty();
    let active_goals = {
        // Issue #1590 follow-up: read goals through the
        // `CognitiveMemoryGoalStore` so this probe sees the same
        // records the runtime persists via `RuntimePorts.goal_store`
        // (the previous `load_goal_board` path queried a different
        // fact concept and missed every put through the goal store).
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
