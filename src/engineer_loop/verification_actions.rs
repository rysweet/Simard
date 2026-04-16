use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::error::{SimardError, SimardResult};

use super::execution::run_command;
use super::types::{
    AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
    RepoInspection, StructuredEditRequest,
};

pub(crate) fn verify_cargo_test(
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

pub(crate) fn verify_cargo_check(action: &ExecutedEngineerAction, checks: &mut Vec<String>) {
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

pub(crate) fn verify_create_file(
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

pub(crate) fn verify_append_to_file(
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

pub(crate) fn verify_open_issue(
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

pub(crate) fn build_verification_summary(action: &ExecutedEngineerAction) -> String {
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

pub(crate) fn verify_structured_text_replace(
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

pub(crate) fn verify_cargo_metadata(
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
    use crate::engineer_loop::types::{
        AppendToFileRequest, CreateFileRequest, EngineerActionKind, ExecutedEngineerAction,
        GitCommitRequest, SelectedEngineerAction, ShellCommandRequest, StructuredEditRequest,
    };
    use std::path::PathBuf;

    fn make_action(
        kind: EngineerActionKind,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> ExecutedEngineerAction {
        ExecutedEngineerAction {
            selected: SelectedEngineerAction {
                label: "test-action".to_string(),
                rationale: "test".to_string(),
                argv: vec![],
                plan_summary: "test plan".to_string(),
                verification_steps: vec![],
                expected_changed_files: vec![],
                kind,
            },
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            changed_files: vec![],
        }
    }

    fn dummy_selected(kind: EngineerActionKind) -> SelectedEngineerAction {
        SelectedEngineerAction {
            label: "test-action".into(),
            rationale: "testing".into(),
            argv: vec!["test".into()],
            plan_summary: "plan".into(),
            verification_steps: vec![],
            expected_changed_files: vec![],
            kind,
        }
    }

    fn dummy_executed(
        kind: EngineerActionKind,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> ExecutedEngineerAction {
        ExecutedEngineerAction {
            selected: dummy_selected(kind),
            exit_code,
            stdout: stdout.into(),
            stderr: stderr.into(),
            changed_files: vec![],
        }
    }

    #[test]
    fn verify_cargo_test_passing() {
        let action = make_action(
            EngineerActionKind::CargoTest,
            0,
            "test result: ok. 5 passed; 0 failed",
            "",
        );
        let mut checks = vec![];
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c == "cargo-test-result-present=true"));
        assert!(checks.iter().any(|c| c == "cargo-test-passed=true"));
    }

    #[test]
    fn verify_cargo_test_failing() {
        let action = make_action(
            EngineerActionKind::CargoTest,
            101,
            "",
            "test result: FAILED. 1 passed; 2 failed",
        );
        let mut checks = vec![];
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c == "cargo-test-passed=false"));
    }

    #[test]
    fn verify_cargo_test_no_output_nonzero_exit() {
        let action = make_action(EngineerActionKind::CargoTest, 1, "", "");
        let mut checks = vec![];
        let result = verify_cargo_test(&action, &mut checks);
        assert!(result.is_err());
    }

    #[test]
    fn verify_cargo_test_no_output_zero_exit() {
        let action = make_action(EngineerActionKind::CargoTest, 0, "", "");
        let mut checks = vec![];
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("exit 0")));
    }

    #[test]
    fn verify_cargo_check_success() {
        let action = make_action(EngineerActionKind::CargoCheck, 0, "", "");
        let mut checks = vec![];
        verify_cargo_check(&action, &mut checks);
        assert!(checks.iter().any(|c| c == "cargo-check-passed=true"));
    }

    #[test]
    fn verify_cargo_check_failure_with_errors() {
        let action = make_action(
            EngineerActionKind::CargoCheck,
            1,
            "",
            "error[E0308]: mismatched types\nerror: aborting",
        );
        let mut checks = vec![];
        verify_cargo_check(&action, &mut checks);
        assert!(
            checks
                .iter()
                .any(|c| c.contains("cargo-check-passed=false") && c.contains("errors=2"))
        );
    }

    #[test]
    fn verify_create_file_success() {
        let dir = std::env::temp_dir().join("simard_test_create_file");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("hello.txt");
        fs::write(&file_path, "hello world").unwrap();

        let inspection = RepoInspection {
            workspace_root: dir.clone(),
            repo_root: dir.clone(),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = CreateFileRequest {
            relative_path: "hello.txt".to_string(),
            content: "hello world".to_string(),
        };
        let mut checks = vec![];
        verify_create_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("file-exists")));
        assert!(checks.iter().any(|c| c == "file-content-matches=true"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_create_file_missing() {
        let dir = std::env::temp_dir().join("simard_test_create_file_missing");
        let _ = fs::create_dir_all(&dir);

        let inspection = RepoInspection {
            workspace_root: dir.clone(),
            repo_root: dir.clone(),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = CreateFileRequest {
            relative_path: "nonexistent.txt".to_string(),
            content: "content".to_string(),
        };
        let mut checks = vec![];
        let result = verify_create_file(&inspection, &req, &mut checks);
        assert!(result.is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_append_to_file_success() {
        let dir = std::env::temp_dir().join("simard_test_append");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("log.txt"), "line1\nAPPENDED DATA\nline3").unwrap();

        let inspection = RepoInspection {
            workspace_root: dir.clone(),
            repo_root: dir.clone(),
            branch: "main".to_string(),
            head: "abc123".to_string(),
            worktree_dirty: false,
            changed_files: vec![],
            active_goals: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = AppendToFileRequest {
            relative_path: "log.txt".to_string(),
            content: "APPENDED DATA".to_string(),
        };
        let mut checks = vec![];
        verify_append_to_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("file-contains-appended")));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_open_issue_without_url() {
        let action = make_action(EngineerActionKind::ReadOnlyScan, 0, "no url here", "");
        let mut checks = vec![];
        let result = verify_open_issue(&action, &mut checks);
        assert!(result.is_err());
    }

    #[test]
    fn build_verification_summary_cargo_test() {
        let action = make_action(EngineerActionKind::CargoTest, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("cargo test"));
        assert!(summary.contains("passed"));
    }

    #[test]
    fn build_verification_summary_cargo_check() {
        let action = make_action(EngineerActionKind::CargoCheck, 1, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("cargo check"));
        assert!(summary.contains("failed"));
    }

    #[test]
    fn build_verification_summary_read_only() {
        let action = make_action(EngineerActionKind::ReadOnlyScan, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("local-only engineer action"));
    }

    #[test]
    fn verify_cargo_metadata_invalid_json() {
        let mut checks = vec![];
        let result = verify_cargo_metadata(Path::new("/fake"), "not json", &mut checks);
        assert!(result.is_err());
    }

    #[test]
    fn verify_cargo_metadata_missing_workspace_root() {
        let json = r#"{"packages": [{"name": "foo"}]}"#;
        let mut checks = vec![];
        let result = verify_cargo_metadata(Path::new("/fake"), json, &mut checks);
        assert!(result.is_err());
    }

    #[test]
    fn verify_cargo_metadata_empty_packages() {
        let cwd = std::env::current_dir().unwrap();
        let canonical = fs::canonicalize(&cwd).unwrap();
        let json = serde_json::json!({
            "workspace_root": canonical.to_str().unwrap(),
            "packages": []
        })
        .to_string();
        let mut checks = vec![];
        let result = verify_cargo_metadata(&canonical, &json, &mut checks);
        assert!(result.is_err());
    }

    #[test]
    fn verify_cargo_test_passed() {
        let action = dummy_executed(
            EngineerActionKind::CargoTest,
            0,
            "test result: ok. 5 passed",
            "",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("cargo-test-passed=true")));
    }

    #[test]
    fn verify_cargo_test_failed() {
        let action = dummy_executed(
            EngineerActionKind::CargoTest,
            1,
            "test result: FAILED. 1 passed",
            "",
        );
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("cargo-test-passed=false")));
    }

    #[test]
    fn verify_cargo_test_no_output_exit_zero() {
        let action = dummy_executed(EngineerActionKind::CargoTest, 0, "", "");
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(
            checks
                .iter()
                .any(|c| c.contains("cargo-test-passed=true (exit 0)"))
        );
    }

    #[test]
    fn verify_cargo_test_no_output_nonzero_errors() {
        let action = dummy_executed(EngineerActionKind::CargoTest, 1, "", "");
        let mut checks = Vec::new();
        assert!(verify_cargo_test(&action, &mut checks).is_err());
    }

    #[test]
    fn verify_cargo_check_passed() {
        let action = dummy_executed(EngineerActionKind::CargoCheck, 0, "", "");
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks.iter().any(|c| c.contains("cargo-check-passed=true")));
    }

    #[test]
    fn verify_cargo_check_failed_counts_errors() {
        let action = dummy_executed(
            EngineerActionKind::CargoCheck,
            1,
            "",
            "error[E0001]: something\nerror[E0002]: another\nwarning: foo",
        );
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks.iter().any(|c| c.contains("errors=2")));
    }

    #[test]
    fn verify_open_issue_with_url() {
        let action = dummy_executed(
            EngineerActionKind::ReadOnlyScan,
            0,
            "https://github.com/org/repo/issues/42",
            "",
        );
        let mut checks = Vec::new();
        verify_open_issue(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("issue-url-present=true")));
    }

    #[test]
    fn verify_open_issue_without_url_errors() {
        let action = dummy_executed(EngineerActionKind::ReadOnlyScan, 0, "no url here", "");
        let mut checks = Vec::new();
        assert!(verify_open_issue(&action, &mut checks).is_err());
    }

    #[test]
    fn build_summary_read_only() {
        let action = dummy_executed(EngineerActionKind::ReadOnlyScan, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("test-action"));
        assert!(summary.contains("repo grounding"));
    }

    #[test]
    fn build_summary_cargo_test() {
        let action = dummy_executed(EngineerActionKind::CargoTest, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("cargo test"));
        assert!(summary.contains("passed"));
    }

    #[test]
    fn build_summary_cargo_check_failed() {
        let action = dummy_executed(EngineerActionKind::CargoCheck, 1, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("failed"));
    }

    #[test]
    fn build_summary_create_file() {
        let kind = EngineerActionKind::CreateFile(CreateFileRequest {
            relative_path: "new.rs".into(),
            content: "fn main() {}".into(),
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("new.rs"));
    }

    #[test]
    fn build_summary_append_to_file() {
        let kind = EngineerActionKind::AppendToFile(AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "entry".into(),
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("log.txt"));
    }

    #[test]
    fn build_summary_git_commit() {
        let kind = EngineerActionKind::GitCommit(GitCommitRequest {
            message: "fix bug".into(),
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("GitCommit"));
    }

    #[test]
    fn build_summary_open_issue() {
        let kind = EngineerActionKind::OpenIssue(crate::engineer_loop::types::OpenIssueRequest {
            title: "bug".into(),
            body: "desc".into(),
            labels: vec![],
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("OpenIssue"));
    }

    #[test]
    fn build_summary_structured_text_replace() {
        let kind = EngineerActionKind::StructuredTextReplace(StructuredEditRequest {
            relative_path: "src/main.rs".into(),
            search: "old".into(),
            replacement: "new".into(),
            verify_contains: "new".into(),
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("src/main.rs"));
    }

    #[test]
    fn build_summary_shell_command() {
        let kind = EngineerActionKind::RunShellCommand(ShellCommandRequest {
            argv: vec!["ls".into()],
        });
        let action = dummy_executed(kind, 0, "", "");
        let summary = build_verification_summary(&action);
        assert!(summary.contains("RunShellCommand"));
    }

    #[test]
    fn verify_cargo_metadata_valid_json() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let canonical_root = std::fs::canonicalize(&repo_root).unwrap();
        let json = serde_json::json!({
            "workspace_root": canonical_root.to_string_lossy(),
            "packages": [{"name": "simard"}]
        });
        let mut checks = Vec::new();
        verify_cargo_metadata(&canonical_root, &json.to_string(), &mut checks).unwrap();
        assert!(
            checks
                .iter()
                .any(|c| c.starts_with("metadata-workspace-root="))
        );
        assert!(checks.iter().any(|c| c.starts_with("metadata-packages=")));
    }

    #[test]
    fn verify_cargo_metadata_invalid_json_errors() {
        let mut checks = Vec::new();
        assert!(verify_cargo_metadata(Path::new("/tmp"), "not json", &mut checks).is_err());
    }

    #[test]
    fn verify_cargo_metadata_missing_workspace_root_errors() {
        let mut checks = Vec::new();
        let json = r#"{"packages": []}"#;
        assert!(verify_cargo_metadata(Path::new("/tmp"), json, &mut checks).is_err());
    }

    #[test]
    fn verify_cargo_metadata_empty_packages_errors() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let canonical_root = std::fs::canonicalize(&repo_root).unwrap();
        let json = serde_json::json!({
            "workspace_root": canonical_root.to_string_lossy(),
            "packages": []
        });
        let mut checks = Vec::new();
        assert!(verify_cargo_metadata(&canonical_root, &json.to_string(), &mut checks).is_err());
    }

    // --- verify_create_file with tempdir ---

    #[test]
    fn verify_create_file_succeeds_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");
        std::fs::write(&file_path, "hello world").unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            head_sha: "abc123".into(),
            branch: "main".into(),
            dirty_paths: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = CreateFileRequest {
            relative_path: "hello.txt".into(),
            content: "hello world".into(),
        };
        let mut checks = Vec::new();
        verify_create_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("file-exists=hello.txt")));
        assert!(checks.iter().any(|c| c.contains("file-content-matches=true")));
    }

    #[test]
    fn verify_create_file_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            head_sha: "abc123".into(),
            branch: "main".into(),
            dirty_paths: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = CreateFileRequest {
            relative_path: "nonexistent.txt".into(),
            content: "data".into(),
        };
        let mut checks = Vec::new();
        assert!(verify_create_file(&inspection, &req, &mut checks).is_err());
    }

    #[test]
    fn verify_create_file_wrong_content_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("hello.txt");
        std::fs::write(&file_path, "wrong content").unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            head_sha: "abc123".into(),
            branch: "main".into(),
            dirty_paths: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = CreateFileRequest {
            relative_path: "hello.txt".into(),
            content: "expected content".into(),
        };
        let mut checks = Vec::new();
        assert!(verify_create_file(&inspection, &req, &mut checks).is_err());
    }

    // --- verify_append_to_file with tempdir ---

    #[test]
    fn verify_append_to_file_succeeds_when_content_present() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("log.txt");
        std::fs::write(&file_path, "old line\nnew entry\n").unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            head_sha: "abc123".into(),
            branch: "main".into(),
            dirty_paths: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "new entry".into(),
        };
        let mut checks = Vec::new();
        verify_append_to_file(&inspection, &req, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("file-contains-appended=log.txt")));
    }

    #[test]
    fn verify_append_to_file_missing_content_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("log.txt");
        std::fs::write(&file_path, "old line only\n").unwrap();
        let inspection = RepoInspection {
            repo_root: dir.path().to_path_buf(),
            head_sha: "abc123".into(),
            branch: "main".into(),
            dirty_paths: vec![],
            carried_meeting_decisions: vec![],
            architecture_gap_summary: String::new(),
        };
        let req = AppendToFileRequest {
            relative_path: "log.txt".into(),
            content: "missing entry".into(),
        };
        let mut checks = Vec::new();
        assert!(verify_append_to_file(&inspection, &req, &mut checks).is_err());
    }

    // --- verify_cargo_metadata: workspace root mismatch ---

    #[test]
    fn verify_cargo_metadata_workspace_root_mismatch_errors() {
        let json = serde_json::json!({
            "workspace_root": "/some/other/path",
            "packages": [{"name": "demo"}]
        });
        let mut checks = Vec::new();
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let canonical_root = std::fs::canonicalize(&repo_root).unwrap();
        assert!(verify_cargo_metadata(&canonical_root, &json.to_string(), &mut checks).is_err());
    }

    // --- verify_cargo_check: zero errors stderr ---

    #[test]
    fn verify_cargo_check_failed_zero_error_lines() {
        let action = dummy_executed(EngineerActionKind::CargoCheck, 1, "", "warning: unused");
        let mut checks = Vec::new();
        verify_cargo_check(&action, &mut checks);
        assert!(checks.iter().any(|c| c.contains("errors=0")));
    }

    // --- verify_open_issue: github.com without https prefix ---

    #[test]
    fn verify_open_issue_bare_github_domain() {
        let mut action = dummy_executed(EngineerActionKind::ReadOnlyScan, 0, "", "");
        action.stdout = "Created issue at github.com/org/repo/issues/7".to_string();
        let mut checks = Vec::new();
        verify_open_issue(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("issue-url-present=true")));
    }

    // --- verify_cargo_test: result in stderr (compiler output) ---

    #[test]
    fn verify_cargo_test_result_in_stderr() {
        let mut action = dummy_executed(EngineerActionKind::CargoTest, 0, "", "");
        action.stderr = "test result: ok. 12 passed; 0 failed; 0 ignored".to_string();
        let mut checks = Vec::new();
        verify_cargo_test(&action, &mut checks).unwrap();
        assert!(checks.iter().any(|c| c.contains("cargo-test-result-present=true")));
    }
}
