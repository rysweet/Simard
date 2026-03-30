use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

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
enum EngineerActionKind {
    ReadOnlyScan,
    StructuredTextReplace(StructuredEditRequest),
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
pub struct EngineerLoopRun {
    pub state_root: PathBuf,
    pub execution_scope: String,
    pub inspection: RepoInspection,
    pub action: ExecutedEngineerAction,
    pub verification: VerificationReport,
    pub terminal_bridge_context: Option<TerminalBridgeContext>,
}

pub fn run_local_engineer_loop(
    workspace_root: impl AsRef<Path>,
    objective: &str,
    topology: RuntimeTopology,
    state_root: impl Into<PathBuf>,
) -> SimardResult<EngineerLoopRun> {
    let state_root = state_root.into();
    let inspection = inspect_workspace(workspace_root.as_ref(), &state_root)?;
    let terminal_bridge_context = TerminalBridgeContext::load_from_state_root(
        &state_root,
        SHARED_EXPLICIT_STATE_ROOT_SOURCE,
    )?;
    let selected_action = select_engineer_action(&inspection, objective)?;
    let action = execute_engineer_action(&inspection.repo_root, selected_action)?;
    let verification = verify_engineer_action(&inspection, &action, &state_root)?;
    persist_engineer_loop_artifacts(
        &state_root,
        topology,
        objective,
        &inspection,
        &action,
        &verification,
        terminal_bridge_context.as_ref(),
    )?;

    Ok(EngineerLoopRun {
        state_root,
        execution_scope: EXECUTION_SCOPE.to_string(),
        inspection,
        action,
        verification,
        terminal_bridge_context,
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

    if inspection.repo_root.join("Cargo.toml").is_file() {
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

    if post.head != inspection.head {
        return Err(SimardError::VerificationFailed {
            reason: format!("HEAD changed from '{}' to '{}'", inspection.head, post.head),
        });
    }
    checks.push(format!("repo-head={}", post.head));

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
        EngineerActionKind::ReadOnlyScan => {
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
        EngineerActionKind::StructuredTextReplace(_) => {
            if !post.worktree_dirty {
                return Err(SimardError::VerificationFailed {
                    reason: "bounded edit succeeded but the repo still appears clean".to_string(),
                });
            }
            if post.changed_files != action.selected.expected_changed_files {
                return Err(SimardError::VerificationFailed {
                    reason: format!(
                        "bounded edit changed unexpected files: expected {:?}, got {:?}",
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
            search = Some(non_empty_objective_value("replace", value)?);
        } else if let Some(value) = trimmed.strip_prefix("with:") {
            saw_edit_directive = true;
            replacement = Some(non_empty_objective_value("with", value)?);
        } else if let Some(value) = trimmed.strip_prefix("verify-contains:") {
            saw_edit_directive = true;
            verify_contains = Some(non_empty_objective_value("verify-contains", value)?);
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
    command.args(args).current_dir(cwd);
    for key in CLEARED_GIT_ENV_VARS {
        command.env_remove(key);
    }
    let output = command
        .output()
        .map_err(|error| SimardError::ActionExecutionFailed {
            action: argv.join(" "),
            reason: error.to_string(),
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
    use super::{parse_status_paths, parse_structured_edit_request, validate_repo_relative_path};

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
}
