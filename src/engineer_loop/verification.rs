use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{SimardError, SimardResult};

use super::execution::run_command;
use super::inspect_workspace;
use super::types::{
    EngineerActionKind, ExecutedEngineerAction, RepoInspection, StructuredEditRequest,
    VerificationReport,
};

pub(crate) fn verify_engineer_action(
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
