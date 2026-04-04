use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{SimardError, SimardResult};

use super::execution::run_command;
use super::inspect_workspace;
use super::types::{
    AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
    RepoInspection, StructuredEditRequest, VerificationReport,
};

fn verify_grounding_stable(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    state_root: &Path,
    checks: &mut Vec<String>,
) -> SimardResult<RepoInspection> {
    let post = inspect_workspace(&inspection.repo_root, state_root)?;

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
    Ok(post)
}

fn verify_worktree_state(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    post: &RepoInspection,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
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
            reason: "carried meeting decision memory changed during a non-mutating local engineer action".to_string(),
        });
    }
    checks.push(format!(
        "carried-meeting-decisions={}",
        post.carried_meeting_decisions.len()
    ));
    Ok(())
}

fn verify_kind_specific(
    inspection: &RepoInspection,
    action: &ExecutedEngineerAction,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    match &action.selected.kind {
        EngineerActionKind::ReadOnlyScan => match action.selected.label.as_str() {
            "cargo-metadata-scan" => {
                verify_cargo_metadata(&inspection.repo_root, &action.stdout, checks)?
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
            checks,
        )?,
        EngineerActionKind::CargoTest => verify_cargo_test(action, checks)?,
        EngineerActionKind::CargoCheck => verify_cargo_check(action, checks),
        EngineerActionKind::CreateFile(req) => verify_create_file(inspection, req, checks)?,
        EngineerActionKind::AppendToFile(req) => verify_append_to_file(inspection, req, checks)?,
        EngineerActionKind::RunShellCommand(_) => {
            checks.push(format!("shell-command-exit-code={}", action.exit_code));
        }
        EngineerActionKind::GitCommit(_) => {
            checks.push("git-commit-created=true".to_string());
        }
        EngineerActionKind::OpenIssue(_) => verify_open_issue(action, checks)?,
    }
    Ok(())
}

fn verify_cargo_test(
    action: &ExecutedEngineerAction,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
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
    Ok(())
}

fn verify_cargo_check(action: &ExecutedEngineerAction, checks: &mut Vec<String>) {
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

fn verify_create_file(
    inspection: &RepoInspection,
    req: &CreateFileRequest,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    let target_path = inspection.repo_root.join(&req.relative_path);
    if !target_path.exists() {
        return Err(SimardError::VerificationFailed {
            reason: format!(
                "file '{}' does not exist after CreateFile",
                req.relative_path
            ),
        });
    }
    let content =
        fs::read_to_string(&target_path).map_err(|error| SimardError::VerificationFailed {
            reason: format!(
                "could not read '{}' to verify content: {error}",
                req.relative_path
            ),
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
    Ok(())
}

fn verify_append_to_file(
    inspection: &RepoInspection,
    req: &AppendToFileRequest,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    let target_path = inspection.repo_root.join(&req.relative_path);
    let content =
        fs::read_to_string(&target_path).map_err(|error| SimardError::VerificationFailed {
            reason: format!(
                "could not read '{}' to verify appended content: {error}",
                req.relative_path
            ),
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
    Ok(())
}

fn verify_open_issue(
    action: &ExecutedEngineerAction,
    checks: &mut Vec<String>,
) -> SimardResult<()> {
    if action.stdout.contains("https://github.com/") || action.stdout.contains("github.com") {
        checks.push("issue-url-present=true".to_string());
    } else {
        return Err(SimardError::VerificationFailed {
            reason: "gh issue create did not return an issue URL in stdout".to_string(),
        });
    }
    Ok(())
}

fn build_verification_summary(action: &ExecutedEngineerAction) -> String {
    match &action.selected.kind {
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
    }
}

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

    let mut checks = Vec::new();
    let post = verify_grounding_stable(inspection, action, state_root, &mut checks)?;
    verify_worktree_state(inspection, action, &post, &mut checks)?;
    verify_kind_specific(inspection, action, &mut checks)?;

    Ok(VerificationReport {
        status: "verified".to_string(),
        summary: build_verification_summary(action),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use super::super::types::{
        AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
        GitCommitRequest, OpenIssueRequest, RepoInspection, SelectedEngineerAction,
        ShellCommandRequest, StructuredEditRequest,
    };

    fn make_inspection() -> RepoInspection {
        RepoInspection {
            workspace_root: PathBuf::from("/fake/workspace"),
            repo_root: PathBuf::from("/fake/repo"),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: Vec::new(),
            active_goals: Vec::new(),
            carried_meeting_decisions: Vec::new(),
            architecture_gap_summary: String::new(),
        }
    }

    fn make_selected(label: &str, kind: EngineerActionKind) -> SelectedEngineerAction {
        SelectedEngineerAction {
            label: label.to_string(),
            rationale: "test".to_string(),
            argv: vec!["test".to_string()],
            plan_summary: "test".to_string(),
            verification_steps: Vec::new(),
            expected_changed_files: Vec::new(),
            kind,
        }
    }

    fn make_executed(
        label: &str,
        kind: EngineerActionKind,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> ExecutedEngineerAction {
        ExecutedEngineerAction {
            selected: make_selected(label, kind),
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            changed_files: Vec::new(),
        }
    }

    // --- verify_cargo_test ---

    #[test]
    fn cargo_test_pass_with_result_line() {
        let action = make_executed(
            "cargo-test",
            EngineerActionKind::CargoTest,
            0,
            "test result: ok. 10 passed; 0 failed",
            "",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.contains(&"cargo-test-result-present=true".to_string()));
        assert!(checks.contains(&"cargo-test-passed=true".to_string()));
    }

    #[test]
    fn cargo_test_fail_with_result_line() {
        let action = make_executed(
            "cargo-test",
            EngineerActionKind::CargoTest,
            1,
            "test result: FAILED. 5 passed; 2 failed",
            "",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.contains(&"cargo-test-passed=false".to_string()));
    }

    #[test]
    fn cargo_test_result_in_stderr_also_detected() {
        let action = make_executed(
            "cargo-test",
            EngineerActionKind::CargoTest,
            0,
            "",
            "test result: ok. 3 passed",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.contains(&"cargo-test-result-present=true".to_string()));
    }

    #[test]
    fn cargo_test_no_output_exit_zero_still_passes() {
        let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 0, "", "");
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("cargo-test-passed=true")));
    }

    #[test]
    fn cargo_test_no_output_nonzero_fails() {
        let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
        let mut checks = Vec::new();
        let err = verify_cargo_test(&action, &mut checks).unwrap_err();
        assert!(err.to_string().contains("no recognizable test result"));
    }

    #[test]
    fn cargo_test_exit_nonzero_with_result_marks_failed() {
        let action = make_executed(
            "cargo-test",
            EngineerActionKind::CargoTest,
            101,
            "test result: ok. 10 passed; 0 failed",
            "",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        // Non-zero exit code overrides "ok" in output
        assert!(checks.contains(&"cargo-test-passed=false".to_string()));
    }

    // --- verify_cargo_check ---

    #[test]
    fn cargo_check_pass() {
        let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 0, "", "");
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks.contains(&"cargo-check-passed=true".to_string()));
    }

    #[test]
    fn cargo_check_fail_counts_error_lines() {
        let action = make_executed(
            "cargo-check",
            EngineerActionKind::CargoCheck,
            1,
            "",
            "error[E0308]: mismatched types\nerror: aborting due to previous error",
        );
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks[0].contains("cargo-check-passed=false"));
        assert!(checks[0].contains("errors=2"));
    }

    #[test]
    fn cargo_check_fail_zero_errors_in_stderr() {
        let action = make_executed(
            "cargo-check",
            EngineerActionKind::CargoCheck,
            1,
            "",
            "warning: unused variable\n",
        );
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks[0].contains("errors=0"));
    }

    // --- verify_open_issue ---

    #[test]
    fn open_issue_with_github_url() {
        let action = make_executed(
            "open-issue",
            EngineerActionKind::OpenIssue(OpenIssueRequest {
                title: "test".into(),
                body: String::new(),
                labels: Vec::new(),
            }),
            0,
            "https://github.com/user/repo/issues/42",
            "",
        );
        let mut checks = Vec::new();
        verify_open_issue(&action, &mut checks).unwrap();
        assert!(checks.contains(&"issue-url-present=true".to_string()));
    }

    #[test]
    fn open_issue_with_github_dot_com() {
        let action = make_executed(
            "open-issue",
            EngineerActionKind::OpenIssue(OpenIssueRequest {
                title: "t".into(),
                body: String::new(),
                labels: Vec::new(),
            }),
            0,
            "Created issue at github.com/repo/issues/1",
            "",
        );
        let mut checks = Vec::new();
        verify_open_issue(&action, &mut checks).unwrap();
        assert!(checks.contains(&"issue-url-present=true".to_string()));
    }

    #[test]
    fn open_issue_without_url_fails() {
        let action = make_executed(
            "open-issue",
            EngineerActionKind::OpenIssue(OpenIssueRequest {
                title: "t".into(),
                body: String::new(),
                labels: Vec::new(),
            }),
            0,
            "no url here",
            "",
        );
        let mut checks = Vec::new();
        let err = verify_open_issue(&action, &mut checks).unwrap_err();
        assert!(err.to_string().contains("did not return an issue URL"));
    }

    // --- build_verification_summary ---

    #[test]
    fn summary_read_only_scan() {
        let action = make_executed("my-scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let s = build_verification_summary(&action);
        assert!(s.contains("my-scan"));
        assert!(s.contains("Verified local-only"));
    }

    #[test]
    fn summary_cargo_test_pass() {
        let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 0, "", "");
        assert!(build_verification_summary(&action).contains("passed"));
    }

    #[test]
    fn summary_cargo_test_fail() {
        let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
        assert!(build_verification_summary(&action).contains("failed"));
    }

    #[test]
    fn summary_cargo_check_pass() {
        let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 0, "", "");
        assert!(build_verification_summary(&action).contains("succeeded"));
    }

    #[test]
    fn summary_cargo_check_fail() {
        let action = make_executed("cargo-check", EngineerActionKind::CargoCheck, 1, "", "");
        assert!(build_verification_summary(&action).contains("failed"));
    }

    #[test]
    fn summary_structured_text_replace_mentions_path() {
        let action = make_executed(
            "edit",
            EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
                relative_path: "src/lib.rs".into(),
                search: "a".into(),
                replacement: "b".into(),
                verify_contains: "b".into(),
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("src/lib.rs"));
    }

    #[test]
    fn summary_create_file_mentions_path() {
        let action = make_executed(
            "create-file",
            EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "foo.txt".into(),
                content: "c".into(),
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("foo.txt"));
    }

    #[test]
    fn summary_append_to_file_mentions_path() {
        let action = make_executed(
            "append",
            EngineerActionKind::AppendToFile(AppendToFileRequest {
                relative_path: "log.txt".into(),
                content: "c".into(),
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("log.txt"));
    }

    #[test]
    fn summary_run_shell_command() {
        let action = make_executed(
            "run",
            EngineerActionKind::RunShellCommand(ShellCommandRequest {
                argv: vec!["cargo".into(), "fmt".into()],
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("RunShellCommand"));
    }

    #[test]
    fn summary_git_commit() {
        let action = make_executed(
            "git-commit",
            EngineerActionKind::GitCommit(GitCommitRequest {
                message: "m".into(),
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("GitCommit"));
    }

    #[test]
    fn summary_open_issue() {
        let action = make_executed(
            "open-issue",
            EngineerActionKind::OpenIssue(OpenIssueRequest {
                title: "t".into(),
                body: String::new(),
                labels: Vec::new(),
            }),
            0,
            "",
            "",
        );
        assert!(build_verification_summary(&action).contains("OpenIssue"));
    }

    // --- verify_engineer_action: non-zero exit code early rejection ---

    #[test]
    fn verify_action_nonzero_exit_code_rejected() {
        let inspection = make_inspection();
        let action = make_executed("cargo-test", EngineerActionKind::CargoTest, 1, "", "");
        let state_root = tempfile::tempdir().unwrap();
        let err = verify_engineer_action(&inspection, &action, state_root.path()).unwrap_err();
        assert!(err.to_string().contains("exited with code 1"));
    }

    // --- verify_kind_specific ---

    #[test]
    fn kind_specific_read_only_unknown_label_rejected() {
        let action = make_executed("unknown-scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let mut checks = Vec::new();
        let err = verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap_err();
        assert!(err.to_string().contains("verification rules are missing"));
    }

    #[test]
    fn kind_specific_git_tracked_file_scan_empty_fails() {
        let action = make_executed(
            "git-tracked-file-scan",
            EngineerActionKind::ReadOnlyScan,
            0,
            "",
            "",
        );
        let mut checks = Vec::new();
        let err = verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap_err();
        assert!(err.to_string().contains("no tracked files"));
    }

    #[test]
    fn kind_specific_git_tracked_file_scan_with_files_ok() {
        let action = make_executed(
            "git-tracked-file-scan",
            EngineerActionKind::ReadOnlyScan,
            0,
            "README.md\nsrc/lib.rs\n",
            "",
        );
        let mut checks = Vec::new();
        verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
        assert!(checks.contains(&"tracked-files-present=true".to_string()));
    }

    #[test]
    fn kind_specific_shell_command_records_exit_code() {
        let action = make_executed(
            "run-shell-command",
            EngineerActionKind::RunShellCommand(ShellCommandRequest {
                argv: vec!["cargo".into(), "fmt".into()],
            }),
            0,
            "",
            "",
        );
        let mut checks = Vec::new();
        verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
        assert!(checks.contains(&"shell-command-exit-code=0".to_string()));
    }

    #[test]
    fn kind_specific_git_commit_records_created() {
        let action = make_executed(
            "git-commit",
            EngineerActionKind::GitCommit(GitCommitRequest {
                message: "m".into(),
            }),
            0,
            "",
            "",
        );
        let mut checks = Vec::new();
        verify_kind_specific(&make_inspection(), &action, &mut checks).unwrap();
        assert!(checks.contains(&"git-commit-created=true".to_string()));
    }

    // --- verify_create_file ---

    #[test]
    fn create_file_correct_content_passes() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();
        let req = CreateFileRequest {
            relative_path: "test.txt".into(),
            content: "hello".into(),
        };
        let mut checks = Vec::new();
        verify_create_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.contains(&"file-exists=test.txt".to_string()));
        assert!(checks.contains(&"file-content-matches=true".to_string()));
    }

    #[test]
    fn create_file_missing_fails() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        let req = CreateFileRequest {
            relative_path: "nonexistent.txt".into(),
            content: "x".into(),
        };
        let mut checks = Vec::new();
        let err = verify_create_file(&inspection, &req, &mut checks).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn create_file_content_mismatch_fails() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        std::fs::write(dir.path().join("test.txt"), "wrong").unwrap();
        let req = CreateFileRequest {
            relative_path: "test.txt".into(),
            content: "expected".into(),
        };
        let mut checks = Vec::new();
        let err = verify_create_file(&inspection, &req, &mut checks).unwrap_err();
        assert!(err.to_string().contains("content does not match"));
    }

    // --- verify_append_to_file ---

    #[test]
    fn append_to_file_contains_content_passes() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        std::fs::write(dir.path().join("log.txt"), "old\nappended text\n").unwrap();
        let req = AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "appended text".into(),
        };
        let mut checks = Vec::new();
        verify_append_to_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.contains(&"file-contains-appended=log.txt".to_string()));
    }

    #[test]
    fn append_to_file_missing_content_fails() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        std::fs::write(dir.path().join("log.txt"), "only old\n").unwrap();
        let req = AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "missing text".into(),
        };
        let mut checks = Vec::new();
        let err = verify_append_to_file(&inspection, &req, &mut checks).unwrap_err();
        assert!(err.to_string().contains("does not contain the appended"));
    }

    #[test]
    fn append_to_file_nonexistent_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            ..make_inspection()
        };
        let req = AppendToFileRequest {
            relative_path: "missing.txt".into(),
            content: "x".into(),
        };
        let mut checks = Vec::new();
        let err = verify_append_to_file(&inspection, &req, &mut checks).unwrap_err();
        assert!(err.to_string().contains("could not read"));
    }

    // --- verify_worktree_state ---

    #[test]
    fn worktree_state_read_only_changed_rejected() {
        let inspection = make_inspection();
        let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let mut post = make_inspection();
        post.worktree_dirty = true;
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(err.to_string().contains("worktree state changed"));
    }

    #[test]
    fn worktree_state_read_only_stable_ok() {
        let inspection = make_inspection();
        let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let post = make_inspection();
        let mut checks = Vec::new();
        verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("worktree-dirty=")));
    }

    #[test]
    fn worktree_state_mutating_still_clean_rejected() {
        let inspection = make_inspection();
        let mut action = make_executed(
            "create-file",
            EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "foo.txt".into(),
                content: "c".into(),
            }),
            0,
            "",
            "",
        );
        action.selected.expected_changed_files = vec!["foo.txt".into()];
        action.changed_files = vec!["foo.txt".into()];
        let post = make_inspection(); // worktree_dirty=false
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(err.to_string().contains("still appears clean"));
    }

    #[test]
    fn worktree_state_mutating_unexpected_files_rejected() {
        let inspection = make_inspection();
        let mut action = make_executed(
            "create-file",
            EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "foo.txt".into(),
                content: "c".into(),
            }),
            0,
            "",
            "",
        );
        action.selected.expected_changed_files = vec!["foo.txt".into()];
        action.changed_files = vec!["foo.txt".into()];
        let mut post = make_inspection();
        post.worktree_dirty = true;
        post.changed_files = vec!["bar.txt".into()];
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(err.to_string().contains("changed unexpected files"));
    }

    #[test]
    fn worktree_state_mutating_action_reported_wrong_files() {
        let inspection = make_inspection();
        let mut action = make_executed(
            "create-file",
            EngineerActionKind::CreateFile(CreateFileRequest {
                relative_path: "foo.txt".into(),
                content: "c".into(),
            }),
            0,
            "",
            "",
        );
        action.selected.expected_changed_files = vec!["foo.txt".into()];
        action.changed_files = vec!["other.txt".into()]; // mismatch
        let mut post = make_inspection();
        post.worktree_dirty = true;
        post.changed_files = vec!["foo.txt".into()];
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(
            err.to_string()
                .contains("executed action reported changed files")
        );
    }

    #[test]
    fn worktree_state_goals_changed_rejected() {
        use crate::goals::{GoalRecord, GoalStatus};
        use crate::session::{SessionId, SessionPhase};
        use uuid::Uuid;

        let inspection = make_inspection();
        let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let mut post = make_inspection();
        post.active_goals = vec![GoalRecord {
            slug: "test".into(),
            title: "Test".into(),
            rationale: "r".into(),
            status: GoalStatus::Active,
            priority: 1,
            owner_identity: "o".into(),
            source_session_id: SessionId::from_uuid(Uuid::nil()),
            updated_in: SessionPhase::Preparation,
        }];
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(err.to_string().contains("active goal set changed"));
    }

    #[test]
    fn worktree_state_meeting_decisions_changed_rejected() {
        let inspection = make_inspection();
        let action = make_executed("scan", EngineerActionKind::ReadOnlyScan, 0, "", "");
        let mut post = make_inspection();
        post.carried_meeting_decisions = vec!["new decision".into()];
        let mut checks = Vec::new();
        let err = verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap_err();
        assert!(
            err.to_string()
                .contains("carried meeting decision memory changed")
        );
    }

    #[test]
    fn worktree_state_git_commit_records_dirty_status() {
        let inspection = make_inspection();
        let action = make_executed(
            "git-commit",
            EngineerActionKind::GitCommit(GitCommitRequest {
                message: "m".into(),
            }),
            0,
            "",
            "",
        );
        let post = make_inspection();
        let mut checks = Vec::new();
        verify_worktree_state(&inspection, &action, &post, &mut checks).unwrap();
        assert!(
            checks
                .iter()
                .any(|c| c.contains("worktree-dirty-after-commit="))
        );
    }

    // --- verify_cargo_metadata ---

    #[test]
    fn cargo_metadata_invalid_json_fails() {
        let mut checks = Vec::new();
        let err =
            verify_cargo_metadata(Path::new("/fake"), "not json at all", &mut checks).unwrap_err();
        assert!(err.to_string().contains("not valid JSON"));
    }

    #[test]
    fn cargo_metadata_missing_workspace_root_fails() {
        let json = r#"{"packages": []}"#;
        let mut checks = Vec::new();
        let err = verify_cargo_metadata(Path::new("/fake"), json, &mut checks).unwrap_err();
        assert!(err.to_string().contains("workspace_root"));
    }

    #[test]
    fn cargo_metadata_missing_packages_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let json = format!(r#"{{"workspace_root": "{}"}}"#, root.display());
        let mut checks = Vec::new();
        let err = verify_cargo_metadata(&root, &json, &mut checks).unwrap_err();
        assert!(err.to_string().contains("packages"));
    }

    #[test]
    fn cargo_metadata_empty_packages_fails() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let json = format!(
            r#"{{"workspace_root": "{}", "packages": []}}"#,
            root.display()
        );
        let mut checks = Vec::new();
        let err = verify_cargo_metadata(&root, &json, &mut checks).unwrap_err();
        assert!(err.to_string().contains("empty package list"));
    }

    #[test]
    fn cargo_metadata_valid_passes() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let json = format!(
            r#"{{"workspace_root": "{}", "packages": [{{"name": "test"}}]}}"#,
            root.display()
        );
        let mut checks = Vec::new();
        verify_cargo_metadata(&root, &json, &mut checks).unwrap();
        assert!(
            checks
                .iter()
                .any(|c| c.contains("metadata-workspace-root="))
        );
        assert!(checks.iter().any(|c| c.contains("metadata-packages=1")));
    }

    #[test]
    fn cargo_metadata_wrong_workspace_root_fails() {
        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();
        let root1 = std::fs::canonicalize(dir1.path()).unwrap();
        let root2 = std::fs::canonicalize(dir2.path()).unwrap();
        let json = format!(
            r#"{{"workspace_root": "{}", "packages": [{{"name": "x"}}]}}"#,
            root2.display()
        );
        let mut checks = Vec::new();
        let err = verify_cargo_metadata(&root1, &json, &mut checks).unwrap_err();
        assert!(err.to_string().contains("instead of"));
    }
}
