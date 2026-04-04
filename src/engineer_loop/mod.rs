mod execution;
mod review_persist;
mod selection;
mod types;
mod verification;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::{SimardError, SimardResult};
use crate::goals::{FileBackedGoalStore, GoalStore};
use crate::memory::{FileBackedMemoryStore, MemoryScope, MemoryStore};
use crate::runtime::RuntimeTopology;
use crate::terminal_engineer_bridge::{SHARED_EXPLICIT_STATE_ROOT_SOURCE, TerminalBridgeContext};

use execution::{
    execute_engineer_action, parse_status_paths, run_command, trimmed_stdout,
    trimmed_stdout_allow_empty,
};
use review_persist::{persist_engineer_loop_artifacts, run_optional_review};
use selection::select_engineer_action;
use verification::verify_engineer_action;

// Re-export all public items so `crate::engineer_loop::X` still works.
pub use types::{
    AnalyzedAction, EngineerLoopRun, ExecutedEngineerAction, PhaseOutcome, PhaseTrace,
    RepoInspection, SelectedEngineerAction, VerificationReport, analyze_objective,
};

const ENGINEER_IDENTITY: &str = "simard-engineer";
const ENGINEER_BASE_TYPE: &str = "terminal-shell";
const EXECUTION_SCOPE: &str = "local-only";
const MAX_CARRIED_MEETING_DECISIONS: usize = 3;
const GIT_COMMAND_TIMEOUT_SECS: u64 = 60;
const CARGO_COMMAND_TIMEOUT_SECS: u64 = 120;
const SHELL_COMMAND_ALLOWLIST: &[&str] = &["cargo", "git", "gh", "rustfmt", "clippy"];

const CLEARED_GIT_ENV_VARS: &[&str] = &[
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

fn inspect_workspace(workspace_root: &Path, state_root: &Path) -> SimardResult<RepoInspection> {
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

fn load_carried_meeting_decisions(state_root: &Path) -> SimardResult<Vec<String>> {
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

fn is_meeting_decision_record(value: &str) -> bool {
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

fn architecture_gap_summary(repo_root: &Path) -> SimardResult<String> {
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

#[cfg(test)]
mod tests {
    use super::execution::execute_engineer_action;
    use super::execution::parse_status_paths;
    use super::types::{
        AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind,
        SelectedEngineerAction, ShellCommandRequest, analyze_objective,
        parse_structured_edit_request, validate_repo_relative_path,
    };

    #[test]
    fn git_status_paths_strip_status_prefixes() {
        let paths =
            parse_status_paths(" M src/lib.rs\nA  tests/engineer_loop.rs\n?? docs/index.md\n");
        assert_eq!(
            paths,
            vec![
                "src/lib.rs".to_string(),
                "tests/engineer_loop.rs".to_string(),
                "docs/index.md".to_string()
            ]
        );
    }

    #[test]
    fn structured_edit_request_requires_complete_directives() {
        let error = parse_structured_edit_request("edit-file: docs/demo.txt\nreplace: before\n")
            .expect_err("incomplete structured edit directives should fail");
        assert!(
            error
                .to_string()
                .contains("structured edit objectives must include non-empty"),
            "error should explain the missing directives: {error}"
        );
    }

    #[test]
    fn structured_edit_paths_must_stay_repo_relative() {
        let error = validate_repo_relative_path("../outside.txt")
            .expect_err("parent escapes should be rejected");
        assert!(
            error.to_string().contains("must not escape"),
            "error should explain the rejected path: {error}"
        );
    }

    // ---- analyze_objective keyword mapping tests ----

    #[test]
    fn analyze_objective_create_file() {
        assert_eq!(
            analyze_objective("create a new config file"),
            AnalyzedAction::CreateFile
        );
    }

    #[test]
    fn analyze_objective_new_file() {
        assert_eq!(
            analyze_objective("new file for the project"),
            AnalyzedAction::CreateFile
        );
    }

    #[test]
    fn analyze_objective_add_file() {
        assert_eq!(
            analyze_objective("add file to the project"),
            AnalyzedAction::CreateFile
        );
    }

    #[test]
    fn analyze_objective_append() {
        assert_eq!(
            analyze_objective("append log entry"),
            AnalyzedAction::AppendToFile
        );
    }

    #[test]
    fn analyze_objective_add_to() {
        assert_eq!(
            analyze_objective("add to the changelog"),
            AnalyzedAction::AppendToFile
        );
    }

    #[test]
    fn analyze_objective_run_shell_command() {
        assert_eq!(
            analyze_objective("run cargo fmt"),
            AnalyzedAction::RunShellCommand
        );
    }

    #[test]
    fn analyze_objective_execute_command() {
        assert_eq!(
            analyze_objective("execute rustfmt on main.rs"),
            AnalyzedAction::RunShellCommand
        );
    }

    #[test]
    fn analyze_objective_git_commit() {
        assert_eq!(
            analyze_objective("commit the changes"),
            AnalyzedAction::GitCommit
        );
    }

    #[test]
    fn analyze_objective_save_changes() {
        assert_eq!(
            analyze_objective("save changes to the repo"),
            AnalyzedAction::GitCommit
        );
    }

    #[test]
    fn analyze_objective_open_issue() {
        assert_eq!(
            analyze_objective("open an issue for the bug"),
            AnalyzedAction::OpenIssue
        );
    }

    #[test]
    fn analyze_objective_bug_report() {
        assert_eq!(
            analyze_objective("file a bug report"),
            AnalyzedAction::OpenIssue
        );
    }

    #[test]
    fn analyze_objective_feature_request() {
        assert_eq!(
            analyze_objective("submit a feature request"),
            AnalyzedAction::OpenIssue
        );
    }

    #[test]
    fn analyze_objective_fix_maps_to_structured_edit() {
        assert_eq!(
            analyze_objective("fix the typo in README"),
            AnalyzedAction::StructuredTextReplace
        );
    }

    #[test]
    fn analyze_objective_update_maps_to_structured_edit() {
        assert_eq!(
            analyze_objective("update the version number"),
            AnalyzedAction::StructuredTextReplace
        );
    }

    #[test]
    fn analyze_objective_cargo_test() {
        assert_eq!(
            analyze_objective("test the parser module"),
            AnalyzedAction::CargoTest
        );
    }

    #[test]
    fn analyze_objective_run_tests_maps_to_cargo_test() {
        assert_eq!(
            analyze_objective("run tests for the project"),
            AnalyzedAction::CargoTest
        );
    }

    #[test]
    fn analyze_objective_default_fallback() {
        assert_eq!(
            analyze_objective("unknown gibberish"),
            AnalyzedAction::ReadOnlyScan
        );
    }

    #[test]
    fn analyze_objective_is_case_insensitive() {
        assert_eq!(
            analyze_objective("CREATE a New File"),
            AnalyzedAction::CreateFile
        );
        assert_eq!(
            analyze_objective("RUN cargo fmt"),
            AnalyzedAction::RunShellCommand
        );
    }

    // ---- CreateFile path validation tests ----

    #[test]
    fn create_file_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "create-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-create-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "../../../etc/passwd".to_string(),
                content: "malicious".to_string(),
            }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("path traversal should be rejected");
        assert!(
            error.to_string().contains("must not escape"),
            "error should mention traversal: {error}"
        );
    }

    #[test]
    fn create_file_rejects_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("existing.txt"), "content").unwrap();
        let selected = SelectedEngineerAction {
            label: "create-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-create-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "existing.txt".to_string(),
                content: "new".to_string(),
            }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("overwriting existing file should be rejected");
        assert!(
            error.to_string().contains("already exists"),
            "error should explain the rejection: {error}"
        );
    }

    #[test]
    fn create_file_succeeds_with_valid_path() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "create-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-create-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: vec!["src/new.rs".to_string()],
            kind: EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "src/new.rs".to_string(),
                content: "fn main() {}".to_string(),
            }),
        };
        let result = execute_engineer_action(dir.path(), selected).unwrap();
        assert_eq!(result.exit_code, 0);
        let written = std::fs::read_to_string(dir.path().join("src/new.rs")).unwrap();
        assert_eq!(written, "fn main() {}");
    }

    // ---- AppendToFile validation tests ----

    #[test]
    fn append_to_file_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "append-to-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-append-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
                relative_path: "../../../etc/shadow".to_string(),
                content: "malicious".to_string(),
            }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("path traversal should be rejected");
        assert!(
            error.to_string().contains("must not escape"),
            "error should mention traversal: {error}"
        );
    }

    #[test]
    fn append_to_file_rejects_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "append-to-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-append-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
                relative_path: "missing.txt".to_string(),
                content: "append this".to_string(),
            }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("appending to nonexistent file should fail");
        assert!(
            error.to_string().contains("does not exist"),
            "error should explain: {error}"
        );
    }

    #[test]
    fn append_to_file_succeeds_with_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("log.txt"), "line1\n").unwrap();
        let selected = SelectedEngineerAction {
            label: "append-to-file".to_string(),
            rationale: "test".to_string(),
            argv: vec!["simard-append-file".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: vec!["log.txt".to_string()],
            kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
                relative_path: "log.txt".to_string(),
                content: "line2\n".to_string(),
            }),
        };
        let result = execute_engineer_action(dir.path(), selected).unwrap();
        assert_eq!(result.exit_code, 0);
        let content = std::fs::read_to_string(dir.path().join("log.txt")).unwrap();
        assert!(content.contains("line1\n"));
        assert!(content.contains("line2\n"));
    }

    // ---- RunShellCommand allowlist tests ----

    #[test]
    fn run_shell_command_rejects_non_allowlisted_command() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "run-shell-command".to_string(),
            rationale: "test".to_string(),
            argv: vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::RunShellCommand(ShellCommandRequest {
                argv: vec!["rm".to_string(), "-rf".to_string(), "/".to_string()],
            }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("non-allowlisted command should be rejected");
        assert!(
            error.to_string().contains("allowlist"),
            "error should mention allowlist: {error}"
        );
    }

    #[test]
    fn run_shell_command_rejects_empty_argv() {
        let dir = tempfile::tempdir().unwrap();
        let selected = SelectedEngineerAction {
            label: "run-shell-command".to_string(),
            rationale: "test".to_string(),
            argv: Vec::new(),
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::RunShellCommand(ShellCommandRequest { argv: Vec::new() }),
        };
        let error = execute_engineer_action(dir.path(), selected)
            .expect_err("empty argv should be rejected");
        assert!(
            error.to_string().contains("empty"),
            "error should explain: {error}"
        );
    }

    // ---- is_meeting_decision_record tests ----

    #[test]
    fn is_meeting_decision_record_positive() {
        let value = "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
        assert!(super::is_meeting_decision_record(value));
    }

    #[test]
    fn is_meeting_decision_record_missing_field() {
        // Missing "goals="
        let value =
            "agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none";
        assert!(!super::is_meeting_decision_record(value));
    }

    #[test]
    fn is_meeting_decision_record_empty_string() {
        assert!(!super::is_meeting_decision_record(""));
    }

    #[test]
    fn is_meeting_decision_record_partial_match() {
        let value = "agenda=stuff decisions=yes";
        assert!(!super::is_meeting_decision_record(value));
    }

    // ---- Additional types tests ----

    #[test]
    fn validate_repo_relative_path_absolute_rejected() {
        let err = validate_repo_relative_path("/etc/passwd")
            .expect_err("absolute paths should be rejected");
        assert!(err.to_string().contains("must stay relative"));
    }

    #[test]
    fn validate_repo_relative_path_empty_rejected() {
        let err = validate_repo_relative_path("").expect_err("empty paths should be rejected");
        assert!(err.to_string().contains("must identify a file"));
    }

    #[test]
    fn validate_repo_relative_path_dot_only_rejected() {
        let err = validate_repo_relative_path(".").expect_err("dot-only paths should be rejected");
        assert!(err.to_string().contains("must identify a file"));
    }

    #[test]
    fn validate_repo_relative_path_normalizes_dot_segments() {
        let result = validate_repo_relative_path("./src/./lib.rs").unwrap();
        assert_eq!(result, "src/lib.rs");
    }

    #[test]
    fn validate_repo_relative_path_simple_valid() {
        let result = validate_repo_relative_path("src/main.rs").unwrap();
        assert_eq!(result, "src/main.rs");
    }

    // ---- parse_structured_edit_request tests ----

    #[test]
    fn structured_edit_complete_request_parses() {
        let obj =
            "edit-file: src/lib.rs\nreplace: old_text\nwith: new_text\nverify-contains: new_text";
        let result = parse_structured_edit_request(obj).unwrap().unwrap();
        assert_eq!(result.relative_path, "src/lib.rs");
        assert_eq!(result.search, "old_text");
        assert_eq!(result.replacement, "new_text");
        assert_eq!(result.verify_contains, "new_text");
    }

    #[test]
    fn structured_edit_no_directives_returns_none() {
        let obj = "just a regular objective with no edit directives";
        assert!(parse_structured_edit_request(obj).unwrap().is_none());
    }

    #[test]
    fn structured_edit_empty_field_value_rejected() {
        let obj = "edit-file:   \nreplace: old\nwith: new\nverify-contains: new";
        let err = parse_structured_edit_request(obj).unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn structured_edit_unescape_newlines_and_tabs() {
        let obj = "edit-file: f.rs\nreplace: a\\nb\nwith: c\\td\nverify-contains: c\\td";
        let result = parse_structured_edit_request(obj).unwrap().unwrap();
        assert_eq!(result.search, "a\nb");
        assert_eq!(result.replacement, "c\td");
    }

    // ---- extract_command_from_objective tests ----

    #[test]
    fn extract_command_run_keyword() {
        let cmd = super::types::extract_command_from_objective("run cargo fmt").unwrap();
        assert_eq!(cmd, vec!["cargo", "fmt"]);
    }

    #[test]
    fn extract_command_execute_keyword() {
        let cmd = super::types::extract_command_from_objective("execute git status").unwrap();
        assert_eq!(cmd, vec!["git", "status"]);
    }

    #[test]
    fn extract_command_no_keyword_returns_none() {
        assert!(super::types::extract_command_from_objective("please do something").is_none());
    }

    #[test]
    fn extract_command_empty_after_keyword_returns_none() {
        assert!(super::types::extract_command_from_objective("run   ").is_none());
    }

    // ---- extract_file_path_from_objective tests ----

    #[test]
    fn extract_file_path_with_slash() {
        let path = super::types::extract_file_path_from_objective("create src/lib.rs now").unwrap();
        assert_eq!(path, "src/lib.rs");
    }

    #[test]
    fn extract_file_path_with_dot_extension() {
        let path =
            super::types::extract_file_path_from_objective("modify config.toml please").unwrap();
        assert_eq!(path, "config.toml");
    }

    #[test]
    fn extract_file_path_no_path_returns_none() {
        assert!(super::types::extract_file_path_from_objective("do something").is_none());
    }

    #[test]
    fn extract_file_path_short_dot_word_skipped() {
        // Words like "a." are too short (len <= 2) to be considered paths
        assert!(super::types::extract_file_path_from_objective("fix a bug").is_none());
    }

    // ---- constants tests ----

    #[test]
    fn engineer_identity_constant() {
        assert_eq!(super::ENGINEER_IDENTITY, "simard-engineer");
    }

    #[test]
    fn engineer_base_type_constant() {
        assert_eq!(super::ENGINEER_BASE_TYPE, "terminal-shell");
    }

    #[test]
    fn execution_scope_constant() {
        assert_eq!(super::EXECUTION_SCOPE, "local-only");
    }

    #[test]
    fn max_carried_meeting_decisions_is_reasonable() {
        let m = super::MAX_CARRIED_MEETING_DECISIONS;
        assert!(m > 0, "must be positive, got {m}");
        assert!(m <= 10, "must be <= 10, got {m}");
    }

    #[test]
    fn shell_command_allowlist_contains_expected_commands() {
        for cmd in &["cargo", "git", "gh", "rustfmt", "clippy"] {
            assert!(
                super::SHELL_COMMAND_ALLOWLIST.contains(cmd),
                "allowlist should contain {cmd}"
            );
        }
    }

    #[test]
    fn shell_command_allowlist_excludes_dangerous_commands() {
        for cmd in &["rm", "sudo", "chmod", "chown", "dd", "mkfs"] {
            assert!(
                !super::SHELL_COMMAND_ALLOWLIST.contains(cmd),
                "allowlist should not contain {cmd}"
            );
        }
    }

    #[test]
    fn cleared_git_env_vars_is_nonempty() {
        assert!(!super::CLEARED_GIT_ENV_VARS.is_empty());
        assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_DIR"));
        assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_WORK_TREE"));
        assert!(super::CLEARED_GIT_ENV_VARS.contains(&"GIT_INDEX_FILE"));
    }

    #[test]
    fn git_command_timeout_is_reasonable() {
        let t = super::GIT_COMMAND_TIMEOUT_SECS;
        assert!(t >= 10, "git timeout too low: {t}");
        assert!(t <= 300, "git timeout too high: {t}");
    }

    #[test]
    fn cargo_command_timeout_is_reasonable() {
        let t = super::CARGO_COMMAND_TIMEOUT_SECS;
        assert!(t >= 30, "cargo timeout too low: {t}");
        assert!(t <= 600, "cargo timeout too high: {t}");
    }

    // ---- architecture_gap_summary tests ----

    #[test]
    fn architecture_gap_summary_no_architecture_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("missing Specs/ProductArchitecture.md"));
    }

    #[test]
    fn architecture_gap_summary_with_architecture_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
        std::fs::write(
            dir.path().join("Specs/ProductArchitecture.md"),
            "# Architecture",
        )
        .unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("Specs/ProductArchitecture.md"));
        assert!(result.contains("engineer mode"));
    }

    #[test]
    fn architecture_gap_summary_with_probe_engineer_loop_run() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("src/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(
            bin_dir.join("simard_operator_probe.rs"),
            r#"fn main() { "engineer-loop-run" }"#,
        )
        .unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("engineer-loop-run"));
    }

    #[test]
    fn architecture_gap_summary_with_probe_terminal_run() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("src/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(
            bin_dir.join("simard_operator_probe.rs"),
            r#"fn main() { "terminal-run" }"#,
        )
        .unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("terminal-run"));
    }

    #[test]
    fn architecture_gap_summary_with_probe_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("src/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("simard_operator_probe.rs"), "fn main() {}").unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("does not yet expose"));
    }

    #[test]
    fn architecture_gap_summary_with_runtime_contracts_docs() {
        let dir = tempfile::tempdir().unwrap();
        let docs_dir = dir.path().join("docs/reference");
        std::fs::create_dir_all(&docs_dir).unwrap();
        std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("runtime contracts docs mention"));
    }

    #[test]
    fn architecture_gap_summary_without_runtime_contracts_docs() {
        let dir = tempfile::tempdir().unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("runtime contracts docs are absent"));
    }

    #[test]
    fn architecture_gap_summary_all_files_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Specs")).unwrap();
        std::fs::write(dir.path().join("Specs/ProductArchitecture.md"), "# Arch").unwrap();
        let bin_dir = dir.path().join("src/bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(
            bin_dir.join("simard_operator_probe.rs"),
            r#"fn main() { "engineer-loop-run" }"#,
        )
        .unwrap();
        let docs_dir = dir.path().join("docs/reference");
        std::fs::create_dir_all(&docs_dir).unwrap();
        std::fs::write(docs_dir.join("runtime-contracts.md"), "# Contracts").unwrap();
        let result = super::architecture_gap_summary(dir.path()).unwrap();
        assert!(result.contains("Specs/ProductArchitecture.md"));
        assert!(result.contains("engineer-loop-run"));
        assert!(result.contains("runtime contracts docs mention"));
    }

    // ---- is_meeting_decision_record additional tests ----

    #[test]
    fn is_meeting_decision_record_fields_in_different_order() {
        let value = "goals=win open_questions=none next_steps=go risks=low decisions=yes updates=things agenda=stuff";
        assert!(super::is_meeting_decision_record(value));
    }

    #[test]
    fn is_meeting_decision_record_with_extra_content() {
        let value = "prefix agenda=stuff updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win suffix";
        assert!(super::is_meeting_decision_record(value));
    }

    #[test]
    fn is_meeting_decision_record_missing_agenda() {
        let value =
            "updates=things decisions=yes risks=low next_steps=go open_questions=none goals=win";
        assert!(!super::is_meeting_decision_record(value));
    }

    #[test]
    fn is_meeting_decision_record_only_agenda() {
        assert!(!super::is_meeting_decision_record("agenda=stuff"));
    }

    // ---- parse_status_paths additional tests ----

    #[test]
    fn parse_status_paths_empty_input() {
        let paths = parse_status_paths("");
        assert!(paths.is_empty());
    }

    #[test]
    fn parse_status_paths_whitespace_only() {
        let paths = parse_status_paths("   \n  \n");
        assert!(paths.is_empty());
    }

    #[test]
    fn parse_status_paths_single_modification() {
        let paths = parse_status_paths(" M src/main.rs\n");
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    fn parse_status_paths_untracked_files() {
        let paths = parse_status_paths("?? new_file.txt\n");
        assert_eq!(paths, vec!["new_file.txt"]);
    }

    #[test]
    fn parse_status_paths_added_file() {
        let paths = parse_status_paths("A  added.rs\n");
        assert_eq!(paths, vec!["added.rs"]);
    }

    #[test]
    fn parse_status_paths_deleted_file() {
        let paths = parse_status_paths(" D removed.rs\n");
        assert_eq!(paths, vec!["removed.rs"]);
    }

    #[test]
    fn parse_status_paths_multiple_mixed_statuses() {
        let paths =
            parse_status_paths(" M modified.rs\nA  added.rs\n?? untracked.txt\n D deleted.rs\n");
        assert_eq!(paths.len(), 4);
        assert!(paths.contains(&"modified.rs".to_string()));
        assert!(paths.contains(&"added.rs".to_string()));
        assert!(paths.contains(&"untracked.txt".to_string()));
        assert!(paths.contains(&"deleted.rs".to_string()));
    }

    // ---- extract_command_from_objective additional tests ----

    #[test]
    fn extract_command_with_multiple_args() {
        let cmd = super::types::extract_command_from_objective("run cargo test --lib").unwrap();
        assert_eq!(cmd[0], "cargo");
        assert!(cmd.len() >= 2);
    }

    #[test]
    fn extract_command_case_insensitive() {
        let cmd = super::types::extract_command_from_objective("RUN cargo fmt").unwrap();
        assert_eq!(cmd[0], "cargo");
    }

    // ---- extract_file_path_from_objective additional tests ----

    #[test]
    fn extract_file_path_nested_directory() {
        let path =
            super::types::extract_file_path_from_objective("create src/engineer_loop/types.rs now")
                .unwrap();
        assert!(path.contains('/'));
    }

    #[test]
    fn extract_file_path_toml_extension() {
        let path = super::types::extract_file_path_from_objective("update Cargo.toml").unwrap();
        assert_eq!(path, "Cargo.toml");
    }

    // ---- validate_repo_relative_path additional tests ----

    #[test]
    fn validate_repo_relative_path_nested_dirs() {
        let result = validate_repo_relative_path("src/engineer_loop/mod.rs").unwrap();
        assert_eq!(result, "src/engineer_loop/mod.rs");
    }

    #[test]
    fn validate_repo_relative_path_double_dot_mid_path_rejected() {
        let err = validate_repo_relative_path("src/../../../etc/passwd")
            .expect_err("parent traversal should be rejected");
        assert!(err.to_string().contains("must not escape"));
    }

    #[test]
    fn validate_repo_relative_path_with_dot_prefix() {
        let result = validate_repo_relative_path("./src/main.rs").unwrap();
        assert_eq!(result, "src/main.rs");
    }
}
