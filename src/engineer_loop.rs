use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::base_types::BaseTypeId;
use crate::error::{SimardError, SimardResult};
use crate::evidence::{EvidenceRecord, EvidenceSource, EvidenceStore, FileBackedEvidenceStore};
use crate::goals::{FileBackedGoalStore, GoalRecord, GoalStore};
use crate::handoff::RuntimeHandoffSnapshot;
use crate::memory::{FileBackedMemoryStore, MemoryRecord, MemoryScope, MemoryStore};
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeState, RuntimeTopology};
use crate::sanitization::{objective_metadata, sanitize_terminal_text};
use crate::session::{SessionPhase, SessionRecord, UuidSessionIdGenerator};
use crate::terminal_engineer_bridge::{
    SHARED_EXPLICIT_STATE_ROOT_SOURCE, ScopedHandoffMode, TerminalBridgeContext,
    persist_handoff_artifacts,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoInspection {
    pub workspace_root: PathBuf,
    pub repo_root: PathBuf,
    pub branch: String,
    pub head: String,
    pub worktree_dirty: bool,
    pub changed_files: Vec<String>,
    pub active_goals: Vec<GoalRecord>,
    pub carried_meeting_decisions: Vec<String>,
    pub architecture_gap_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StructuredEditRequest {
    relative_path: String,
    search: String,
    replacement: String,
    verify_contains: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CreateFileRequest {
    relative_path: String,
    content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppendToFileRequest {
    relative_path: String,
    content: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellCommandRequest {
    argv: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GitCommitRequest {
    message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenIssueRequest {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum EngineerActionKind {
    ReadOnlyScan,
    StructuredTextReplace(StructuredEditRequest),
    CargoTest,
    CargoCheck,
    CreateFile(CreateFileRequest),
    AppendToFile(AppendToFileRequest),
    RunShellCommand(ShellCommandRequest),
    GitCommit(GitCommitRequest),
    OpenIssue(OpenIssueRequest),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectedEngineerAction {
    pub label: String,
    pub rationale: String,
    pub argv: Vec<String>,
    pub plan_summary: String,
    pub verification_steps: Vec<String>,
    pub expected_changed_files: Vec<String>,
    kind: EngineerActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutedEngineerAction {
    pub selected: SelectedEngineerAction,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub changed_files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationReport {
    pub status: String,
    pub summary: String,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhaseOutcome {
    Success,
    Failed(String),
    Skipped(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhaseTrace {
    pub name: String,
    pub duration: Duration,
    pub outcome: PhaseOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineerLoopRun {
    pub state_root: PathBuf,
    pub execution_scope: String,
    pub inspection: RepoInspection,
    pub action: ExecutedEngineerAction,
    pub verification: VerificationReport,
    pub terminal_bridge_context: Option<TerminalBridgeContext>,
    pub elapsed_duration: Duration,
    pub phase_traces: Vec<PhaseTrace>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzedAction {
    CreateFile,
    AppendToFile,
    RunShellCommand,
    GitCommit,
    OpenIssue,
    StructuredTextReplace,
    CargoTest,
    ReadOnlyScan,
}

/// Classify an objective string into an action category using keyword matching.
/// Case-insensitive. More specific compound patterns are checked before single
/// keywords so that "run tests" maps to `CargoTest` rather than `RunShellCommand`.
pub fn analyze_objective(objective: &str) -> AnalyzedAction {
    let lower = objective.to_lowercase();

    // Most specific compound patterns first
    if lower.contains("new file") || lower.contains("create") || lower.contains("add file") {
        AnalyzedAction::CreateFile
    } else if lower.contains("append") || lower.contains("add to") {
        AnalyzedAction::AppendToFile
    } else if lower.contains("commit") || lower.contains("save changes") {
        AnalyzedAction::GitCommit
    } else if lower.contains("issue")
        || lower.contains("bug report")
        || lower.contains("feature request")
    {
        AnalyzedAction::OpenIssue
    } else if lower.contains("cargo test")
        || lower.contains("run tests")
        || lower.contains("test suite")
        || lower.contains("run the tests")
    {
        AnalyzedAction::CargoTest
    } else if lower.contains("run") || lower.contains("execute") || lower.contains("check") {
        AnalyzedAction::RunShellCommand
    } else if lower.contains("fix")
        || lower.contains("change")
        || lower.contains("update")
        || lower.contains("replace")
    {
        AnalyzedAction::StructuredTextReplace
    } else if lower.contains("test") {
        AnalyzedAction::CargoTest
    } else {
        AnalyzedAction::ReadOnlyScan
    }
}

fn extract_command_from_objective(objective: &str) -> Option<Vec<String>> {
    let lower = objective.to_lowercase();
    let rest = if let Some(idx) = lower.find("run ") {
        &objective[idx + 4..]
    } else if let Some(idx) = lower.find("execute ") {
        &objective[idx + 8..]
    } else {
        return None;
    };
    let argv: Vec<String> = rest.split_whitespace().map(String::from).collect();
    if argv.is_empty() { None } else { Some(argv) }
}

fn extract_file_path_from_objective(objective: &str) -> Option<String> {
    objective
        .split_whitespace()
        .find(|w| w.contains('/') || (w.contains('.') && w.len() > 2))
        .map(|s| s.to_string())
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

fn select_engineer_action(
    inspection: &RepoInspection,
    objective: &str,
) -> SimardResult<SelectedEngineerAction> {
    let carry_forward_note = if inspection.carried_meeting_decisions.is_empty() {
        String::new()
    } else {
        format!(
            " Shared state root also carries {} meeting decision record{}, so the engineer loop keeps that handoff visible while choosing the next safe repo-native action.",
            inspection.carried_meeting_decisions.len(),
            if inspection.carried_meeting_decisions.len() == 1 {
                ""
            } else {
                "s"
            }
        )
    };

    if let Some(edit_request) = parse_structured_edit_request(objective)? {
        if inspection.worktree_dirty {
            return Err(SimardError::UnsupportedEngineerAction {
                reason: "structured text replacement objectives require a clean git worktree so Simard does not overwrite unrelated local changes".to_string(),
            });
        }
        let relative_path = validate_repo_relative_path(&edit_request.relative_path)?;
        let verify_contains = edit_request.verify_contains.clone();
        return Ok(SelectedEngineerAction {
            label: "structured-text-replace".to_string(),
            rationale: format!(
                "Objective includes explicit edit-file/replace/with/verify-contains directives, so the next honest bounded engineer action is to update '{}' once, then verify the requested text is present and visible through git state.{carry_forward_note}",
                relative_path
            ),
            argv: vec![
                "simard-structured-edit".to_string(),
                relative_path.clone(),
                "replace-once".to_string(),
            ],
            plan_summary: format!(
                "Inspect the clean repo, replace the requested text once in '{}', then verify the file content and git state reflect exactly that bounded local change.",
                relative_path
            ),
            verification_steps: vec![
                format!("confirm '{}' contains '{}'", relative_path, verify_contains),
                format!(
                    "confirm git status reports '{}' as the only changed file",
                    relative_path
                ),
                "confirm carried meeting decisions and active goals stayed stable".to_string(),
            ],
            expected_changed_files: vec![relative_path.clone()],
            kind: EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
                relative_path,
                ..edit_request
            }),
        });
    }

    // Route new action types via keyword analysis before falling through
    // to existing Cargo.toml / .git scan-based fallback.
    //
    // Try LLM-based planning first; fall back to keyword analysis.
    let analyzed = match crate::engineer_plan::plan_objective(objective, inspection) {
        Ok(plan) if !plan.steps().is_empty() => plan.steps()[0].action.clone(),
        _ => analyze_objective(objective),
    };
    match analyzed {
        AnalyzedAction::CreateFile => {
            if let Some(path) = extract_file_path_from_objective(objective) {
                let relative_path = validate_repo_relative_path(&path)?;
                let content = objective
                    .lines()
                    .skip_while(|l| !l.to_lowercase().starts_with("content:"))
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(SelectedEngineerAction {
                    label: "create-file".to_string(),
                    rationale: format!(
                        "Objective requests creating a new file at '{relative_path}'.{carry_forward_note}"
                    ),
                    argv: vec!["simard-create-file".to_string(), relative_path.clone()],
                    plan_summary: format!(
                        "Create file '{}' with the specified content, then verify the file exists.",
                        relative_path
                    ),
                    verification_steps: vec![
                        format!("confirm '{}' exists", relative_path),
                        "confirm file content matches request".to_string(),
                    ],
                    expected_changed_files: vec![relative_path.clone()],
                    kind: EngineerActionKind::CreateFile(CreateFileRequest {
                        relative_path,
                        content,
                    }),
                });
            }
        }
        AnalyzedAction::AppendToFile => {
            if let Some(path) = extract_file_path_from_objective(objective) {
                let relative_path = validate_repo_relative_path(&path)?;
                let content = objective
                    .lines()
                    .skip_while(|l| !l.to_lowercase().starts_with("content:"))
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join("\n");
                return Ok(SelectedEngineerAction {
                    label: "append-to-file".to_string(),
                    rationale: format!(
                        "Objective requests appending content to '{relative_path}'.{carry_forward_note}"
                    ),
                    argv: vec!["simard-append-file".to_string(), relative_path.clone()],
                    plan_summary: format!(
                        "Append content to '{}', then verify the file contains the appended text.",
                        relative_path
                    ),
                    verification_steps: vec![format!(
                        "confirm '{}' contains appended content",
                        relative_path
                    )],
                    expected_changed_files: vec![relative_path.clone()],
                    kind: EngineerActionKind::AppendToFile(AppendToFileRequest {
                        relative_path,
                        content,
                    }),
                });
            }
        }
        AnalyzedAction::RunShellCommand => {
            if let Some(argv) = extract_command_from_objective(objective) {
                let first = argv.first().cloned().unwrap_or_default();
                if SHELL_COMMAND_ALLOWLIST.contains(&first.as_str()) {
                    return Ok(SelectedEngineerAction {
                        label: "run-shell-command".to_string(),
                        rationale: format!(
                            "Objective requests running '{}', which is in the shell allowlist.{carry_forward_note}",
                            argv.join(" ")
                        ),
                        argv: argv.clone(),
                        plan_summary: format!("Execute '{}' and capture output.", argv.join(" ")),
                        verification_steps: vec![format!(
                            "confirm '{}' exits with status 0",
                            argv.join(" ")
                        )],
                        expected_changed_files: Vec::new(),
                        kind: EngineerActionKind::RunShellCommand(ShellCommandRequest { argv }),
                    });
                }
            }
        }
        AnalyzedAction::GitCommit => {
            let message = {
                let lower = objective.to_lowercase();
                if let Some(idx) = lower.find("commit ") {
                    objective[idx + 7..].trim().to_string()
                } else {
                    objective.to_string()
                }
            };
            return Ok(SelectedEngineerAction {
                label: "git-commit".to_string(),
                rationale: format!(
                    "Objective requests committing changes with message: '{}'.{carry_forward_note}",
                    message
                ),
                argv: vec![
                    "git".to_string(),
                    "commit".to_string(),
                    "-m".to_string(),
                    message.clone(),
                ],
                plan_summary: "Stage all changes and create a git commit.".to_string(),
                verification_steps: vec!["confirm HEAD changed (new commit created)".to_string()],
                expected_changed_files: Vec::new(),
                kind: EngineerActionKind::GitCommit(GitCommitRequest { message }),
            });
        }
        AnalyzedAction::OpenIssue => {
            let title = objective.to_string();
            return Ok(SelectedEngineerAction {
                label: "open-issue".to_string(),
                rationale: format!(
                    "Objective requests opening a GitHub issue.{carry_forward_note}"
                ),
                argv: vec![
                    "gh".to_string(),
                    "issue".to_string(),
                    "create".to_string(),
                    "--title".to_string(),
                    title.clone(),
                ],
                plan_summary: "Create a GitHub issue via gh CLI.".to_string(),
                verification_steps: vec!["confirm issue URL is returned in stdout".to_string()],
                expected_changed_files: Vec::new(),
                kind: EngineerActionKind::OpenIssue(OpenIssueRequest {
                    title,
                    body: String::new(),
                    labels: Vec::new(),
                }),
            });
        }
        _ => {} // CargoTest, StructuredTextReplace, ReadOnlyScan fall through to existing logic
    }

    if inspection.repo_root.join("Cargo.toml").is_file() {
        // Only select cargo test/check when the objective explicitly asks for it.
        // Generic words like "verify" are too broad and would misfire.
        let obj_lower = objective.to_lowercase();
        if obj_lower.contains("cargo test")
            || obj_lower.contains("run tests")
            || obj_lower.contains("test suite")
            || obj_lower.contains("run the tests")
        {
            return Ok(SelectedEngineerAction {
                label: "cargo-test".to_string(),
                rationale: format!(
                    "Objective explicitly requests running tests and a Cargo.toml is present, so the next bounded action is to run the test suite and report results.{carry_forward_note}"
                ),
                argv: vec![
                    "cargo".to_string(),
                    "test".to_string(),
                    "--all-features".to_string(),
                    "--locked".to_string(),
                ],
                plan_summary:
                    "Run the full Rust test suite, capture results, and verify the build is healthy."
                        .to_string(),
                verification_steps: vec![
                    "confirm cargo test exits with status 0".to_string(),
                    "confirm test result line reports 0 failures".to_string(),
                    "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                ],
                expected_changed_files: Vec::new(),
                kind: EngineerActionKind::CargoTest,
            });
        }

        if obj_lower.contains("cargo check")
            || obj_lower.contains("compilation check")
            || obj_lower.contains("check compilation")
            || obj_lower.contains("cargo build")
        {
            return Ok(SelectedEngineerAction {
                label: "cargo-check".to_string(),
                rationale: format!(
                    "Objective mentions build/check and a Cargo.toml is present, so the next bounded action is to run cargo check and report compilation status.{carry_forward_note}"
                ),
                argv: vec![
                    "cargo".to_string(),
                    "check".to_string(),
                    "--all-targets".to_string(),
                    "--all-features".to_string(),
                ],
                plan_summary: "Run cargo check to verify the codebase compiles cleanly."
                    .to_string(),
                verification_steps: vec![
                    "confirm cargo check exits with status 0".to_string(),
                    "confirm no compilation errors in output".to_string(),
                    "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                ],
                expected_changed_files: Vec::new(),
                kind: EngineerActionKind::CargoCheck,
            });
        }

        return Ok(SelectedEngineerAction {
            label: "cargo-metadata-scan".to_string(),
            rationale: format!(
                "Detected a Rust workspace via Cargo.toml, so the next honest v1 action is a local argv-only cargo metadata scan that inspects the workspace graph without pretending remote orchestration exists.{carry_forward_note}"
            ),
            argv: vec![
                "cargo".to_string(),
                "metadata".to_string(),
                "--format-version".to_string(),
                "1".to_string(),
                "--no-deps".to_string(),
            ],
            plan_summary:
                "Inspect the repo, query Cargo metadata without mutating files, and verify repo grounding stayed stable."
                    .to_string(),
            verification_steps: vec![
                "confirm cargo metadata returns valid workspace JSON".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                "confirm carried meeting decisions and active goals stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        });
    }

    if inspection.repo_root.join(".git").exists() {
        return Ok(SelectedEngineerAction {
            label: "git-tracked-file-scan".to_string(),
            rationale: format!(
                "No repo-native language manifest was detected, so the loop falls back to a local argv-only scan of tracked files instead of inventing unsupported tooling.{carry_forward_note}"
            ),
            argv: vec![
                "git".to_string(),
                "ls-files".to_string(),
                "--cached".to_string(),
            ],
            plan_summary:
                "Inspect the repo, enumerate tracked files without mutating content, and verify repo grounding stayed stable."
                    .to_string(),
            verification_steps: vec![
                "confirm at least one tracked file is reported".to_string(),
                "confirm repo root, branch, HEAD, and worktree state stayed stable".to_string(),
                "confirm carried meeting decisions and active goals stayed stable".to_string(),
            ],
            expected_changed_files: Vec::new(),
            kind: EngineerActionKind::ReadOnlyScan,
        });
    }

    Err(SimardError::UnsupportedEngineerAction {
        reason: format!(
            "workspace '{}' is repo-grounded but exposes no supported local-first action policy",
            inspection.repo_root.display()
        ),
    })
}

fn execute_engineer_action(
    repo_root: &Path,
    selected: SelectedEngineerAction,
) -> SimardResult<ExecutedEngineerAction> {
    match selected.kind.clone() {
        EngineerActionKind::ReadOnlyScan => {
            let argv = selected.argv.iter().map(String::as_str).collect::<Vec<_>>();
            let output = run_command(repo_root, &argv)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::StructuredTextReplace(edit_request) => {
            let target_path = repo_root.join(&edit_request.relative_path);
            let current = fs::read_to_string(&target_path).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not read '{}' before applying the bounded edit: {error}",
                        target_path.display()
                    ),
                }
            })?;
            let updated = current.replacen(&edit_request.search, &edit_request.replacement, 1);
            if updated == current {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "replacement target was not found in '{}'",
                        edit_request.relative_path
                    ),
                });
            }
            fs::write(&target_path, updated).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not write '{}' after applying the bounded edit: {error}",
                        target_path.display()
                    ),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!(
                    "updated '{}' with one structured replacement",
                    edit_request.relative_path
                ),
                stderr: String::new(),
                changed_files: vec![edit_request.relative_path.clone()],
            })
        }
        EngineerActionKind::CargoTest | EngineerActionKind::CargoCheck => {
            let argv = selected.argv.iter().map(String::as_str).collect::<Vec<_>>();
            let output = run_command(repo_root, &argv)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::CreateFile(ref req) => {
            let relative_path = validate_repo_relative_path(&req.relative_path)?;
            let target_path = repo_root.join(&relative_path);
            if target_path.exists() {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "file '{}' already exists; CreateFile refuses to overwrite",
                        relative_path
                    ),
                });
            }
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "could not create parent directories for '{}': {error}",
                        relative_path
                    ),
                })?;
            }
            fs::write(&target_path, &req.content).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not write '{}': {error}", relative_path),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!("created file '{}'", relative_path),
                stderr: String::new(),
                changed_files: vec![relative_path],
            })
        }
        EngineerActionKind::AppendToFile(ref req) => {
            let relative_path = validate_repo_relative_path(&req.relative_path)?;
            let target_path = repo_root.join(&relative_path);
            if !target_path.exists() {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "file '{}' does not exist; AppendToFile requires an existing file",
                        relative_path
                    ),
                });
            }
            let mut file = OpenOptions::new()
                .append(true)
                .open(&target_path)
                .map_err(|error| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not open '{}' for appending: {error}", relative_path),
                })?;
            file.write_all(req.content.as_bytes()).map_err(|error| {
                SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!("could not append to '{}': {error}", relative_path),
                }
            })?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: 0,
                stdout: format!("appended content to '{}'", relative_path),
                stderr: String::new(),
                changed_files: vec![relative_path],
            })
        }
        EngineerActionKind::RunShellCommand(ref req) => {
            let first = req
                .argv
                .first()
                .ok_or_else(|| SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: "shell command argv is empty".to_string(),
                })?;
            if !SHELL_COMMAND_ALLOWLIST.contains(&first.as_str()) {
                return Err(SimardError::ActionExecutionFailed {
                    action: selected.argv.join(" "),
                    reason: format!(
                        "command '{}' is not in the shell command allowlist {:?}",
                        first, SHELL_COMMAND_ALLOWLIST
                    ),
                });
            }
            let argv_refs: Vec<&str> = req.argv.iter().map(String::as_str).collect();
            let output = run_command(repo_root, &argv_refs)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::GitCommit(ref req) => {
            run_command(repo_root, &["git", "add", "-A"])?;
            let output = run_command(repo_root, &["git", "commit", "-m", &req.message])?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
        EngineerActionKind::OpenIssue(ref req) => {
            let mut argv_owned: Vec<String> = vec![
                "gh".to_string(),
                "issue".to_string(),
                "create".to_string(),
                "--title".to_string(),
                req.title.clone(),
                "--body".to_string(),
                req.body.clone(),
            ];
            for label in &req.labels {
                argv_owned.push("--label".to_string());
                argv_owned.push(label.clone());
            }
            let argv_refs: Vec<&str> = argv_owned.iter().map(String::as_str).collect();
            let output = run_command(repo_root, &argv_refs)?;
            Ok(ExecutedEngineerAction {
                selected,
                exit_code: output.status.code().unwrap_or_default(),
                stdout: sanitize_terminal_text(&output.stdout),
                stderr: sanitize_terminal_text(&output.stderr),
                changed_files: Vec::new(),
            })
        }
    }
}

fn verify_engineer_action(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    state_root: &Path,
) -> SimardResult<VerificationReport> {
    if action.exit_code != 0 {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "selected action '{}' exited with code {}",
                action.selected.label, action.exit_code
            ),
        });
    }

    let post = inspect_workspace(&inspection.repo_root, state_root)?;
    let mut checks = Vec::new();

    if post.repo_root != inspection.repo_root {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "repo root changed from '{}' to '{}'",
                inspection.repo_root.display(),
                post.repo_root.display()
            ),
        });
    }
    checks.push(format!("repo-root={}", post.repo_root.display()));

    match &action.selected.kind {
        EngineerActionKind::GitCommit(_) => {
            if post.head == inspection.head {
                return Err(SimardError::VerificationFailed {
                    reason: "HEAD did not change after git commit".to_string(),
                });
            }
            checks.push(format!("repo-head-changed={}", post.head));
        }
        _ => {
            if post.head != inspection.head {
                return Err(SimardError::VerificationFailed {
                    reason: format!("HEAD changed from '{}' to '{}'", inspection.head, post.head),
                });
            }
            checks.push(format!("repo-head={}", post.head));
        }
    }

    if post.branch != inspection.branch {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "branch changed from '{}' to '{}'",
                inspection.branch, post.branch
            ),
        });
    }
    checks.push(format!("repo-branch={}", post.branch));

    match &action.selected.kind {
        EngineerActionKind::ReadOnlyScan
        | EngineerActionKind::CargoTest
        | EngineerActionKind::CargoCheck
        | EngineerActionKind::RunShellCommand(_)
        | EngineerActionKind::OpenIssue(_) => {
            if post.worktree_dirty != inspection.worktree_dirty
                || post.changed_files != inspection.changed_files
            {
                return Err(SimardError::VerificationFailed {
                    reason: "worktree state changed during a non-mutating local engineer action"
                        .to_string(),
                });
            }
            checks.push(format!("worktree-dirty={}", post.worktree_dirty));
            checks.push("changed-files-after-action=<none>".to_string());
        }
        EngineerActionKind::StructuredTextReplace(_)
        | EngineerActionKind::CreateFile(_)
        | EngineerActionKind::AppendToFile(_) => {
            if !post.worktree_dirty {
                return Err(SimardError::VerificationFailed {
                    reason: "file-mutating action succeeded but the repo still appears clean"
                        .to_string(),
                });
            }
            if post.changed_files != action.selected.expected_changed_files {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "file-mutating action changed unexpected files: expected {:?}, got {:?}",
                        action.selected.expected_changed_files, post.changed_files
                    ),
                });
            }
            if action.changed_files != action.selected.expected_changed_files {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "executed action reported changed files {:?}, expected {:?}",
                        action.changed_files, action.selected.expected_changed_files
                    ),
                });
            }
            checks.push(format!("worktree-dirty={}", post.worktree_dirty));
            checks.push(format!(
                "changed-files-after-action={}",
                post.changed_files.join(", ")
            ));
        }
        EngineerActionKind::GitCommit(_) => {
            checks.push(format!(
                "worktree-dirty-after-commit={}",
                post.worktree_dirty
            ));
        }
    }
    if post.active_goals != inspection.active_goals {
        return Err(SimardError::VerificationFailed {
            reason: "active goal set changed during a non-mutating local engineer action"
                .to_string(),
        });
    }
    checks.push(format!("active-goals={}", post.active_goals.len()));

    if post.carried_meeting_decisions != inspection.carried_meeting_decisions {
        return Err(SimardError::VerificationFailed {
            reason: "carried meeting decision memory changed during a non-mutating local engineer action"
                .to_string(),
        });
    }
    checks.push(format!(
        "carried-meeting-decisions={}",
        post.carried_meeting_decisions.len()
    ));

    match &action.selected.kind {
        EngineerActionKind::ReadOnlyScan => match action.selected.label.as_str() {
            "cargo-metadata-scan" => {
                verify_cargo_metadata(&inspection.repo_root, &action.stdout, &mut checks)?
            }
            "git-tracked-file-scan" => {
                if action.stdout.lines().next().is_none() {
                    return Err(SimardError::VerificationFailed {
                        reason: "git tracked-file scan returned no tracked files".to_string(),
                    });
                }
                checks.push("tracked-files-present=true".to_string());
            }
            other => {
                return Err(SimardError::VerificationFailed {
                    reason: format!("verification rules are missing for selected action '{other}'"),
                });
            }
        },
        EngineerActionKind::StructuredTextReplace(edit_request) => verify_structured_text_replace(
            &inspection.repo_root,
            edit_request,
            &action.stdout,
            &mut checks,
        )?,
        EngineerActionKind::CargoTest => {
            // Verify test output contains a test result summary line
            let combined = format!("{}\n{}", action.stdout, action.stderr);
            if combined.contains("test result:") {
                checks.push("cargo-test-result-present=true".to_string());
                if combined.contains("FAILED") || action.exit_code != 0 {
                    checks.push("cargo-test-passed=false".to_string());
                } else {
                    checks.push("cargo-test-passed=true".to_string());
                }
            } else if action.exit_code == 0 {
                checks.push("cargo-test-result-present=false (no test output)".to_string());
                checks.push("cargo-test-passed=true (exit 0)".to_string());
            } else {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "cargo test exited with code {} and produced no recognizable test result summary",
                        action.exit_code
                    ),
                });
            }
        }
        EngineerActionKind::CargoCheck => {
            if action.exit_code == 0 {
                checks.push("cargo-check-passed=true".to_string());
            } else {
                let error_count = action
                    .stderr
                    .lines()
                    .filter(|l| l.starts_with("error"))
                    .count();
                checks.push(format!("cargo-check-passed=false (errors={})", error_count));
            }
        }
        EngineerActionKind::CreateFile(req) => {
            let target_path = inspection.repo_root.join(&req.relative_path);
            if !target_path.exists() {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "file '{}' does not exist after CreateFile",
                        req.relative_path
                    ),
                });
            }
            let content = fs::read_to_string(&target_path).map_err(|error| {
                SimardError::VerificationFailed {
                    reason: format!(
                        "could not read '{}' to verify content: {error}",
                        req.relative_path
                    ),
                }
            })?;
            if content != req.content {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "file '{}' content does not match expected content",
                        req.relative_path
                    ),
                });
            }
            checks.push(format!("file-exists={}", req.relative_path));
            checks.push("file-content-matches=true".to_string());
        }
        EngineerActionKind::AppendToFile(req) => {
            let target_path = inspection.repo_root.join(&req.relative_path);
            let content = fs::read_to_string(&target_path).map_err(|error| {
                SimardError::VerificationFailed {
                    reason: format!(
                        "could not read '{}' to verify appended content: {error}",
                        req.relative_path
                    ),
                }
            })?;
            if !content.contains(&req.content) {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "file '{}' does not contain the appended content",
                        req.relative_path
                    ),
                });
            }
            checks.push(format!("file-contains-appended={}", req.relative_path));
        }
        EngineerActionKind::RunShellCommand(_) => {
            checks.push(format!("shell-command-exit-code={}", action.exit_code));
        }
        EngineerActionKind::GitCommit(_) => {
            checks.push("git-commit-created=true".to_string());
        }
        EngineerActionKind::OpenIssue(_) => {
            if action.stdout.contains("https://github.com/") || action.stdout.contains("github.com")
            {
                checks.push("issue-url-present=true".to_string());
            } else {
                return Err(SimardError::VerificationFailed {
                    reason: "gh issue create did not return an issue URL in stdout".to_string(),
                });
            }
        }
    }

    Ok(VerificationReport {
        status: "verified".to_string(),
        summary: match &action.selected.kind {
            EngineerActionKind::ReadOnlyScan => format!(
                "Verified local-only engineer action '{}' against stable repo grounding, unchanged worktree state, and explicit repo-native action checks.",
                action.selected.label
            ),
            EngineerActionKind::StructuredTextReplace(edit_request) => format!(
                "Verified bounded local engineer edit '{}' by checking '{}' for the requested content, confirming the expected git-visible file change, and preserving stable repo grounding.",
                action.selected.label, edit_request.relative_path
            ),
            EngineerActionKind::CargoTest => format!(
                "Verified cargo test action '{}': exit_code={}, test suite {}.",
                action.selected.label,
                action.exit_code,
                if action.exit_code == 0 {
                    "passed"
                } else {
                    "failed"
                }
            ),
            EngineerActionKind::CargoCheck => format!(
                "Verified cargo check action '{}': compilation {}.",
                action.selected.label,
                if action.exit_code == 0 {
                    "succeeded"
                } else {
                    "failed"
                }
            ),
            EngineerActionKind::CreateFile(req) => format!(
                "Verified CreateFile action '{}': file '{}' exists with expected content.",
                action.selected.label, req.relative_path
            ),
            EngineerActionKind::AppendToFile(req) => format!(
                "Verified AppendToFile action '{}': file '{}' contains appended content.",
                action.selected.label, req.relative_path
            ),
            EngineerActionKind::RunShellCommand(_) => format!(
                "Verified RunShellCommand action '{}': exit_code={}.",
                action.selected.label, action.exit_code
            ),
            EngineerActionKind::GitCommit(_) => format!(
                "Verified GitCommit action '{}': HEAD advanced to new commit.",
                action.selected.label
            ),
            EngineerActionKind::OpenIssue(_) => format!(
                "Verified OpenIssue action '{}': issue URL present in output.",
                action.selected.label
            ),
        },
        checks,
    })
}

fn verify_structured_text_replace(
    repo_root: &Path,
    edit_request: &StructuredEditRequest,
    action_stdout: &str,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    let target_path = repo_root.join(&edit_request.relative_path);
    let current =
        fs::read_to_string(&target_path).map_err(|error| SimardError::VerificationFailed {
            reason: format!(
                "could not read '{}' while verifying the bounded edit: {error}",
                target_path.display()
            ),
        })?;
    if !current.contains(&edit_request.verify_contains) {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "'{}' does not contain required verification text '{}'",
                edit_request.relative_path, edit_request.verify_contains
            ),
        });
    }
    checks.push(format!(
        "verify-contains={}::{}",
        edit_request.relative_path, edit_request.verify_contains
    ));

    let diff = run_command(
        repo_root,
        &["git", "diff", "--", edit_request.relative_path.as_str()],
    )?;
    if diff.stdout.trim().is_empty() {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "git diff returned no visible change for '{}'",
                edit_request.relative_path
            ),
        });
    }
    if !diff.stdout.contains(&edit_request.replacement)
        && !diff.stdout.contains(&edit_request.verify_contains)
    {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "git diff for '{}' did not contain the replacement or verification text",
                edit_request.relative_path
            ),
        });
    }
    checks.push(format!("git-diff-visible={}", edit_request.relative_path));

    if !action_stdout.contains(&edit_request.relative_path) {
        return Err(SimardError::VerificationFailed {
            reason: "structured edit action output did not identify the changed file".to_string(),
        });
    }
    checks.push("action-output-identifies-changed-file=true".to_string());
    Ok(())
}

fn verify_cargo_metadata(
    repo_root: &Path,
    stdout: &str,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    let payload: Value =
        serde_json::from_str(stdout).map_err(|error| SimardError::VerificationFailed {
            reason: format!("cargo metadata output was not valid JSON: {error}"),
        })?;
    let workspace_root = payload
        .get("workspace_root")
        .and_then(Value::as_str)
        .ok_or_else(|| SimardError::VerificationFailed {
            reason: "cargo metadata output did not include workspace_root".to_string(),
        })?;
    let workspace_root =
        fs::canonicalize(workspace_root).map_err(|error| SimardError::VerificationFailed {
            reason: format!("cargo metadata workspace_root could not be canonicalized: {error}"),
        })?;
    if workspace_root != repo_root {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "cargo metadata reported workspace_root '{}' instead of '{}'",
                workspace_root.display(),
                repo_root.display()
            ),
        });
    }
    checks.push(format!(
        "metadata-workspace-root={}",
        workspace_root.display()
    ));

    let packages = payload
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| SimardError::VerificationFailed {
            reason: "cargo metadata output did not include packages".to_string(),
        })?;
    if packages.is_empty() {
        return Err(SimardError::VerificationFailed {
            reason: "cargo metadata reported an empty package list".to_string(),
        });
    }
    checks.push(format!("metadata-packages={}", packages.len()));
    Ok(())
}

/// Run the optional LLM-driven review on mutating actions.
///
/// Skips (returns `Ok`) for read-only actions, test/check actions, and when
/// no LLM session is available. Blocks with [`SimardError::ReviewBlocked`]
/// if the review finds high-severity bugs or security issues.
fn run_optional_review(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
) -> SimardResult<()> {
    let is_mutating = matches!(
        action.selected.kind,
        EngineerActionKind::StructuredTextReplace(_)
            | EngineerActionKind::CreateFile(_)
            | EngineerActionKind::AppendToFile(_)
            | EngineerActionKind::GitCommit(_)
    );
    if !is_mutating {
        return Ok(());
    }

    let mut review_session = match crate::review_pipeline::ReviewSession::open() {
        Some(s) => s,
        None => return Ok(()),
    };

    let diff_text = compute_diff_for_review(&inspection.repo_root, &action.selected.kind);
    if diff_text.is_empty() {
        let _ = review_session.close();
        return Ok(());
    }

    let findings =
        crate::review_pipeline::review_diff(&mut review_session, &diff_text, PHILOSOPHY_REVIEW);
    let _ = review_session.close();

    let findings = match findings {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };

    if !crate::review_pipeline::should_commit(&findings) {
        let summary = crate::review_pipeline::summarize_review(&findings);
        return Err(SimardError::ReviewBlocked { summary });
    }

    Ok(())
}

const PHILOSOPHY_REVIEW: &str = "Ruthless simplicity. No unnecessary abstractions. \
    Modules under 400 lines. Every public function tested. \
    Clippy clean. No panics in library code.";

fn compute_diff_for_review(repo_root: &Path, kind: &EngineerActionKind) -> String {
    let args: &[&str] = match kind {
        EngineerActionKind::GitCommit(_) => &["git", "diff", "HEAD~1", "HEAD"],
        _ => &["git", "diff"],
    };
    match Command::new(args[0])
        .args(&args[1..])
        .current_dir(repo_root)
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    }
}

fn persist_engineer_loop_artifacts(
    state_root: &Path,
    topology: RuntimeTopology,
    objective: &str,
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    verification: &VerificationReport,
    terminal_bridge_context: Option<&TerminalBridgeContext>,
) -> SimardResult<()> {
    let memory_store = FileBackedMemoryStore::try_new(state_root.join("memory_records.json"))?;
    let evidence_store =
        FileBackedEvidenceStore::try_new(state_root.join("evidence_records.json"))?;

    let session_ids = UuidSessionIdGenerator;
    let mut session = SessionRecord::new(
        crate::identity::OperatingMode::Engineer,
        objective.to_string(),
        BaseTypeId::new(ENGINEER_BASE_TYPE),
        &session_ids,
    );
    session.advance(SessionPhase::Preparation)?;

    let scratch_key = format!("{}-engineer-loop-scratch", session.id);
    memory_store.put(MemoryRecord {
        key: scratch_key.clone(),
        scope: MemoryScope::SessionScratch,
        value: objective_metadata(objective),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Preparation,
    })?;
    session.attach_memory(scratch_key);

    session.advance(SessionPhase::Planning)?;
    session.advance(SessionPhase::Execution)?;
    let evidence_details = vec![
        format!("repo-root={}", inspection.repo_root.display()),
        format!("repo-branch={}", inspection.branch),
        format!("repo-head={}", inspection.head),
        format!("worktree-dirty={}", inspection.worktree_dirty),
        format!(
            "changed-files={}",
            if inspection.changed_files.is_empty() {
                "<none>".to_string()
            } else {
                inspection.changed_files.join(", ")
            }
        ),
        format!(
            "active-goals={}",
            if inspection.active_goals.is_empty() {
                "<none>".to_string()
            } else {
                inspection
                    .active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        format!(
            "carried-meeting-decisions={}",
            if inspection.carried_meeting_decisions.is_empty() {
                "<none>".to_string()
            } else {
                inspection.carried_meeting_decisions.join(" || ")
            }
        ),
        format!("architecture-gap={}", inspection.architecture_gap_summary),
        format!("execution-scope={EXECUTION_SCOPE}"),
        format!("selected-action={}", action.selected.label),
        format!("action-plan={}", action.selected.plan_summary),
        format!(
            "action-verification-steps={}",
            action.selected.verification_steps.join(" || ")
        ),
        format!("selected-action-rationale={}", action.selected.rationale),
        format!("action-command={}", action.selected.argv.join(" ")),
        "action-status=success".to_string(),
        format!("action-exit-code={}", action.exit_code),
        format!(
            "changed-files-after-action={}",
            if action.changed_files.is_empty() {
                "<none>".to_string()
            } else {
                action.changed_files.join(", ")
            }
        ),
        format!("verification-status={}", verification.status),
        format!("verification-summary={}", verification.summary),
    ];
    let mut evidence_details = evidence_details;
    if let Some(terminal_bridge_context) = terminal_bridge_context {
        evidence_details.extend(terminal_bridge_context.engineer_evidence_details());
    }

    for (index, detail) in evidence_details.into_iter().enumerate() {
        let evidence_id = format!("{}-engineer-loop-evidence-{}", session.id, index + 1);
        evidence_store.record(EvidenceRecord {
            id: evidence_id.clone(),
            session_id: session.id.clone(),
            phase: SessionPhase::Execution,
            detail,
            source: EvidenceSource::Runtime,
        })?;
        session.attach_evidence(evidence_id);
    }

    session.advance(SessionPhase::Reflection)?;
    session.advance(SessionPhase::Persistence)?;

    let summary_key = format!("{}-engineer-loop-summary", session.id);
    memory_store.put(MemoryRecord {
        key: summary_key.clone(),
        scope: MemoryScope::SessionSummary,
        value: format!(
            "engineer-loop-summary | repo-root={} | repo-branch={} | worktree-dirty={} | active-goals={} | carried-meeting-decisions={} | selected-action={} | verification-status={} | execution-scope={EXECUTION_SCOPE}",
            inspection.repo_root.display(),
            inspection.branch,
            inspection.worktree_dirty,
            if inspection.active_goals.is_empty() {
                "<none>".to_string()
            } else {
                inspection
                    .active_goals
                    .iter()
                    .map(GoalRecord::concise_label)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            inspection.carried_meeting_decisions.len(),
            action.selected.label,
            verification.status
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Persistence,
    })?;
    session.attach_memory(summary_key);

    let decision_key = format!("{}-engineer-loop-decision", session.id);
    memory_store.put(MemoryRecord {
        key: decision_key.clone(),
        scope: MemoryScope::Decision,
        value: format!(
            "engineer-loop-decision | carried-meeting-decisions={} | {} | {}",
            inspection.carried_meeting_decisions.len(),
            action.selected.rationale,
            verification.summary
        ),
        session_id: session.id.clone(),
        recorded_in: SessionPhase::Persistence,
    })?;
    session.attach_memory(decision_key);

    session.advance(SessionPhase::Complete)?;

    let handoff = RuntimeHandoffSnapshot {
        exported_state: RuntimeState::Ready,
        identity_name: ENGINEER_IDENTITY.to_string(),
        selected_base_type: BaseTypeId::new(ENGINEER_BASE_TYPE),
        topology,
        source_runtime_node: RuntimeNodeId::local(),
        source_mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        session: Some(session.redacted_for_handoff()),
        memory_records: memory_store.list_for_session(&session.id)?,
        evidence_records: evidence_store.list_for_session(&session.id)?,
        copilot_submit_audit: None,
    };
    persist_handoff_artifacts(state_root, ScopedHandoffMode::Engineer, &handoff)?;
    Ok(())
}

fn parse_structured_edit_request(objective: &str) -> SimardResult<Option<StructuredEditRequest>> {
    let mut relative_path = None;
    let mut search = None;
    let mut replacement = None;
    let mut verify_contains = None;
    let mut saw_edit_directive = false;

    for line in objective.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("edit-file:") {
            saw_edit_directive = true;
            relative_path = Some(non_empty_objective_value("edit-file", value)?);
        } else if let Some(value) = trimmed.strip_prefix("replace:") {
            saw_edit_directive = true;
            search = Some(unescape_edit_value(&non_empty_objective_value(
                "replace", value,
            )?));
        } else if let Some(value) = trimmed.strip_prefix("with:") {
            saw_edit_directive = true;
            replacement = Some(unescape_edit_value(&non_empty_objective_value(
                "with", value,
            )?));
        } else if let Some(value) = trimmed.strip_prefix("verify-contains:") {
            saw_edit_directive = true;
            verify_contains = Some(unescape_edit_value(&non_empty_objective_value(
                "verify-contains",
                value,
            )?));
        }
    }

    if !saw_edit_directive {
        return Ok(None);
    }

    match (relative_path, search, replacement, verify_contains) {
        (Some(relative_path), Some(search), Some(replacement), Some(verify_contains)) => {
            Ok(Some(StructuredEditRequest {
                relative_path,
                search,
                replacement,
                verify_contains,
            }))
        }
        _ => Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit objectives must include non-empty edit-file:, replace:, with:, and verify-contains: lines".to_string(),
        }),
    }
}

fn non_empty_objective_value(field: &str, value: &str) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: format!("structured edit objective field '{field}' cannot be empty"),
        });
    }
    Ok(trimmed.to_string())
}

fn unescape_edit_value(value: &str) -> String {
    value.replace("\\n", "\n").replace("\\t", "\t")
}

fn validate_repo_relative_path(relative_path: &str) -> SimardResult<String> {
    let path = Path::new(relative_path);
    if path.is_absolute() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit target paths must stay relative to the selected repo"
                .to_string(),
        });
    }

    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SimardError::UnsupportedEngineerAction {
                    reason: "structured edit target paths must not escape the selected repo"
                        .to_string(),
                });
            }
        }
    }

    if normalized.is_empty() {
        return Err(SimardError::UnsupportedEngineerAction {
            reason: "structured edit target paths must identify a file under the selected repo"
                .to_string(),
        });
    }

    Ok(normalized.join("/"))
}

struct CommandOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn timeout_for_command(argv: &[&str]) -> Duration {
    if argv.first().is_some_and(|cmd| *cmd == "cargo") {
        Duration::from_secs(CARGO_COMMAND_TIMEOUT_SECS)
    } else {
        Duration::from_secs(GIT_COMMAND_TIMEOUT_SECS)
    }
}

fn run_command(cwd: &Path, argv: &[&str]) -> SimardResult<CommandOutput> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| SimardError::ActionExecutionFailed {
            action: "<empty>".to_string(),
            reason: "argv command list cannot be empty".to_string(),
        })?;
    if argv
        .iter()
        .any(|segment| segment.is_empty() || segment.contains('\n') || segment.contains('\r'))
    {
        return Err(SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: "argv-only command segments must be non-empty single-line values".to_string(),
        });
    }

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in CLEARED_GIT_ENV_VARS {
        command.env_remove(key);
    }
    let mut child = command
        .spawn()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: error.to_string(),
        })?;

    let deadline = Instant::now() + timeout_for_command(argv);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(SimardError::CommandTimeout {
                        action: argv.join(" "),
                        timeout_secs: timeout_for_command(argv).as_secs(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                return Err(SimardError::ActionExecutionFailed {
                    action: argv.join(" "),
                    reason: format!("failed to poll child process: {error}"),
                });
            }
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: format!("failed to collect child output: {error}"),
        })?;

    if !output.status.success() {
        let stderr = sanitize_terminal_text(&String::from_utf8_lossy(&output.stderr));
        let stdout = sanitize_terminal_text(&String::from_utf8_lossy(&output.stdout));
        let reason = if stderr.trim().is_empty() {
            format!(
                "command exited with status {} and stdout='{}'",
                output.status,
                stdout.trim()
            )
        } else {
            format!(
                "command exited with status {} and stderr='{}'",
                output.status,
                stderr.trim()
            )
        };
        let error = if argv.starts_with(&["git", "rev-parse", "--show-toplevel"]) {
            SimardError::NotARepo {
                path: cwd.to_path_buf(),
                reason,
            }
        } else {
            SimardError::ActionExecutionFailed {
                action: argv.join(" "),
                reason,
            }
        };
        return Err(error);
    }

    Ok(CommandOutput {
        status: output.status,
        stdout: sanitize_terminal_text(&String::from_utf8_lossy(&output.stdout)),
        stderr: sanitize_terminal_text(&String::from_utf8_lossy(&output.stderr)),
    })
}

fn trimmed_stdout(output: &CommandOutput) -> SimardResult<String> {
    let trimmed = output.stdout.trim();
    if trimmed.is_empty() {
        return Err(SimardError::VerificationFailed {
            reason: "expected a non-empty command result while inspecting repo state".to_string(),
        });
    }

    Ok(trimmed.to_string())
}

fn trimmed_stdout_allow_empty(output: &CommandOutput) -> String {
    output.stdout.trim().to_string()
}

fn parse_status_paths(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.len() > 3 {
                line[3..].trim().to_string()
            } else {
                line.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        AnalyzedAction, AppendToFileRequest, CreateFileRequest, EngineerActionKind,
        SelectedEngineerAction, ShellCommandRequest, analyze_objective, execute_engineer_action,
        parse_status_paths, parse_structured_edit_request, validate_repo_relative_path,
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
}
