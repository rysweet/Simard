pub(crate) mod execution;
pub(crate) mod review_persist;
pub(crate) mod selection;
mod types;
pub(crate) mod verification;
mod verification_actions;

#[cfg(test)]
mod tests_execution;
#[cfg(test)]
mod tests_execution_extra;
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
mod tests_selection;
#[cfg(test)]
mod tests_selection_extra;
#[cfg(test)]
mod tests_selection_inline_a;
#[cfg(test)]
mod tests_selection_inline_b;
#[cfg(test)]
mod tests_types;
#[cfg(test)]
mod tests_types_extra;
#[cfg(test)]
mod tests_types_inline;
#[cfg(test)]
mod tests_verification;
#[cfg(test)]
mod tests_verification_actions;
#[cfg(test)]
mod tests_verification_extra;
#[cfg(test)]
mod tests_verification_more;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goals::{FileBackedGoalStore, GoalStore};
use crate::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
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
pub use execution::execute_engineer_action;
pub use review_persist::{persist_engineer_loop_artifacts, run_optional_review};
pub use selection::select_engineer_action;
pub use verification::verify_engineer_action;

pub(crate) const ENGINEER_IDENTITY: &str = "simard-engineer";
pub(crate) const ENGINEER_BASE_TYPE: &str = "terminal-shell";
pub(crate) const EXECUTION_SCOPE: &str = "local-only";
pub(crate) const MAX_CARRIED_MEETING_DECISIONS: usize = 3;
pub(crate) const GIT_COMMAND_TIMEOUT_SECS: u64 = 60;
pub(crate) const CARGO_COMMAND_TIMEOUT_SECS: u64 = 120;
pub(crate) const SHELL_COMMAND_ALLOWLIST: &[&str] = &[
    // Mutating / build / VCS — produce real work
    "cargo", "git", "gh", "rustfmt", "clippy",
    // Read-only inspection — safe for engineers to use during planning
    "ls", "cat", "grep", "rg", "find", "wc", "head", "tail", "jq",
];

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

    let phase_start = Instant::now();
    let selected_action = select_engineer_action(&inspection, objective);
    match &selected_action {
        Ok(_) => {
            phase_traces.push(PhaseTrace {
                name: "select".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "select".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    let selected_action = selected_action?;

    let phase_start = Instant::now();
    let action = execute_engineer_action(&inspection.repo_root, selected_action);
    match &action {
        Ok(_) => {
            phase_traces.push(PhaseTrace {
                name: "execute".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "execute".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    let action = action?;

    let phase_start = Instant::now();
    let verification = verify_engineer_action(&inspection, &action, &state_root);
    match &verification {
        Ok(_) => {
            phase_traces.push(PhaseTrace {
                name: "verify".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Success,
            });
        }
        Err(e) => {
            phase_traces.push(PhaseTrace {
                name: "verify".to_string(),
                duration: phase_start.elapsed(),
                outcome: PhaseOutcome::Failed(e.to_string()),
            });
        }
    }
    let verification = verification?;

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

pub(crate) fn load_carried_meeting_decisions(state_root: &Path) -> SimardResult<Vec<String>> {
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let mut carried = memory_store
        .list(MemoryScope::Decision)?
        .into_iter()
        .filter_map(|record| match is_meeting_decision_record(&record.value) {
            true => Some(record.value),
            false => None,
        })
        .collect::<Vec<_>>();

    // Also check for unprocessed meeting handoff artifacts.
    let handoff_dir = crate::meeting_facilitator::default_handoff_dir();
    match crate::meeting_facilitator::load_meeting_handoff(&handoff_dir) {
        Ok(Some(handoff)) if !handoff.processed => {
            for d in &handoff.decisions {
                carried.push(format!(
                    "meeting handoff — {}: {} (rationale: {})",
                    handoff.topic, d.description, d.rationale,
                ));
            }
            for a in &handoff.action_items {
                carried.push(format!(
                    "meeting handoff — {} action: {} (owner: {}, priority: {})",
                    handoff.topic, a.description, a.owner, a.priority,
                ));
            }
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!(
                "[simard] warning: failed to load meeting handoff from '{}': {e}",
                handoff_dir.display()
            );
        }
    }

    if carried.len() > MAX_CARRIED_MEETING_DECISIONS {
        carried = carried.split_off(carried.len() - MAX_CARRIED_MEETING_DECISIONS);
    }

    Ok(carried)
}

pub(crate) fn is_meeting_decision_record(value: &str) -> bool {
    [
        "agenda=",
        "updates=",
        "decisions=",
        "risks=",
        "next_steps=",
        "open_questions=",
        "goals=",
    ]
    .into_iter()
    .all(|fragment| value.contains(fragment))
}

pub(crate) fn architecture_gap_summary(repo_root: &Path) -> SimardResult<String> {
    let architecture_path = repo_root.join("Specs/ProductArchitecture.md");
    let probe_path = repo_root.join("src/bin/simard_operator_probe.rs");
    let runtime_contracts_path = repo_root.join("docs/reference/runtime-contracts.md");

    let architecture_exists = architecture_path.is_file();
    let probe_exists = probe_path.is_file();
    let operator_surface = if probe_exists {
        let source = fs::read_to_string(&probe_path).map_err(|error| SimardError::ArtifactIo {
            path: probe_path.clone(),
            reason: error.to_string(),
        })?;
        if source.contains("\"engineer-loop-run\"") {
            "operator probe already exposes engineer-loop-run".to_string()
        } else if source.contains("\"terminal-run\"") {
            "operator probe exposes terminal-run but not a repo-grounded engineer-loop-run"
                .to_string()
        } else {
            "operator probe exists but does not yet expose a terminal engineer loop".to_string()
        }
    } else {
        "operator probe file is missing".to_string()
    };

    let review_trace = if runtime_contracts_path.is_file() {
        "runtime contracts docs mention operator/runtime public surfaces and prior spec reconciliation"
    } else {
        "runtime contracts docs are absent, so gap trace comes only from code and architecture files"
    };

    Ok(match architecture_exists {
        true => format!(
            "Compared current operator/runtime surfaces against Specs/ProductArchitecture.md: the architecture requires inspect -> act -> verify -> persist in engineer mode, and {operator_surface}; {review_trace}."
        ),
        false => format!(
            "Repository is missing Specs/ProductArchitecture.md, so the gap trace falls back to current operator/runtime surfaces only; {operator_surface}; {review_trace}."
        ),
    })
}
