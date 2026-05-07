mod agent_spawn;
pub(crate) mod execution;
pub(crate) mod review_persist;
mod types;

#[cfg(test)]
mod tests_agent_spawn;
#[cfg(test)]
mod tests_agent_spawn_contract;
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

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goals::{FileBackedGoalStore, GoalStore};
use crate::runtime::RuntimeTopology;
use crate::terminal_engineer_bridge::{SHARED_EXPLICIT_STATE_ROOT_SOURCE, TerminalBridgeContext};

use execution::{parse_status_paths, run_command, trimmed_stdout, trimmed_stdout_allow_empty};

// Re-export all public items so `crate::engineer_loop::X` still works.
pub use types::{
    AnalyzedAction, EngineerActionKind, EngineerLoopRun, ExecutedEngineerAction, PhaseOutcome,
    PhaseTrace, RepoInspection, SelectedEngineerAction, VerificationReport, analyze_objective,
};

// Phase-entry-point re-exports for the recipe-driven engineer loop (Phase 2 rebuild).
// These let `simard-engineer-step` (in src/bin/) drive each phase via JSON IPC.
pub use agent_spawn::spawn_agent_for_goal;
pub use review_persist::{persist_engineer_loop_artifacts, run_optional_review};

pub(crate) const ENGINEER_IDENTITY: &str = "simard-engineer";
pub(crate) const ENGINEER_BASE_TYPE: &str = "terminal-shell";
pub(crate) const EXECUTION_SCOPE: &str = "local-only";
pub(crate) const MAX_CARRIED_MEETING_DECISIONS: usize = 3;
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
            return Err(inspection.unwrap_err());
        }
    };

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
    let terminal_bridge_context = terminal_bridge_context?;

    // Phase: agent-prompt-build
    let phase_start = Instant::now();
    let agent_prompt = agent_spawn::build_agent_prompt(objective, &inspection);
    phase_traces.push(PhaseTrace {
        name: "agent-prompt-build".to_string(),
        duration: phase_start.elapsed(),
        outcome: PhaseOutcome::Success,
    });

    // Phase: agent-spawn — open session and start background thread
    let phase_start = Instant::now();
    let rx = agent_spawn::start_agent_session(agent_prompt, &inspection.repo_root);
    let rx = match rx {
        Ok(rx) => {
            phase_traces.push(PhaseTrace {
                name: "agent-spawn".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
            rx
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "agent-spawn".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
            return Err(e);
        }
    };

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
            return Err(e);
        }
    };

    let verification = VerificationReport {
        status: "agent-completed".to_string(),
        summary: action.stdout.clone(),
        checks: vec![],
    };

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
    review_result?;

    let phase_start = Instant::now();
    let persist_result = persist_engineer_loop_artifacts(
        &state_root,
        topology,
        objective,
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
    persist_result?;

    Ok(EngineerLoopRun {
        state_root,
        execution_scope: EXECUTION_SCOPE.to_string(),
        inspection,
        action,
        verification,
        terminal_bridge_context,
        elapsed_duration: loop_start.elapsed(),
        phase_traces,
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
    let active_goals =
        FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?.active_top_goals(5)?;
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
