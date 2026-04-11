//! Git helper operations for the self-improvement executor.

use std::path::Path;
use std::process::Command;

use crate::error::{SimardError, SimardResult};
use crate::git_guardrails;

pub(crate) fn git_diff(workspace: &Path) -> SimardResult<String> {
    git_guardrails::check_git_safety(workspace, &["diff", "HEAD"]).map_err(|e| {
        SimardError::GitCommandFailed {
            command: "git diff HEAD".into(),
            reason: e,
        }
    })?;
    let output = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git diff HEAD".into(),
            reason: e.to_string(),
        })?;

    if !output.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git diff HEAD".into(),
            reason: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn git_commit(workspace: &Path, message: &str) -> SimardResult<()> {
    git_guardrails::check_git_safety(workspace, &["add", "-A"]).map_err(|e| {
        SimardError::GitCommandFailed {
            command: "git add -A".into(),
            reason: e,
        }
    })?;
    git_guardrails::check_git_safety(workspace, &["commit", "-m", message]).map_err(|e| {
        SimardError::GitCommandFailed {
            command: "git commit".into(),
            reason: e,
        }
    })?;
    let add_output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git add -A".into(),
            reason: e.to_string(),
        })?;

    if !add_output.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git add -A".into(),
            reason: String::from_utf8_lossy(&add_output.stderr).into_owned(),
        });
    }

    let commit_output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git commit".into(),
            reason: e.to_string(),
        })?;

    if !commit_output.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git commit".into(),
            reason: String::from_utf8_lossy(&commit_output.stderr).into_owned(),
        });
    }

    Ok(())
}

pub(crate) fn rollback(workspace: &Path) -> SimardResult<()> {
    git_guardrails::check_git_safety(workspace, &["checkout", "--", "."]).map_err(|e| {
        SimardError::GitCommandFailed {
            command: "git checkout -- .".into(),
            reason: e,
        }
    })?;
    git_guardrails::check_git_safety(workspace, &["clean", "-fd"]).map_err(|e| {
        SimardError::GitCommandFailed {
            command: "git clean -fd".into(),
            reason: e,
        }
    })?;
    // Restore modified tracked files.
    let checkout = Command::new("git")
        .args(["checkout", "--", "."])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git checkout -- .".into(),
            reason: e.to_string(),
        })?;

    if !checkout.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git checkout -- .".into(),
            reason: String::from_utf8_lossy(&checkout.stderr).into_owned(),
        });
    }

    // Remove untracked files/dirs created by plan steps.
    let clean = Command::new("git")
        .args(["clean", "-fd"])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git clean -fd".into(),
            reason: e.to_string(),
        })?;

    if !clean.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git clean -fd".into(),
            reason: String::from_utf8_lossy(&clean.stderr).into_owned(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Set up a temporary git repository with an initial commit.
    fn init_temp_repo() -> tempfile::TempDir {
        let tmp = tempdir().unwrap();
        let ws = tmp.path();
        Command::new("git")
            .args(["init"])
            .current_dir(ws)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(ws)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(ws)
            .output()
            .unwrap();
        std::fs::write(ws.join("init.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(ws)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(ws)
            .output()
            .unwrap();
        tmp
    }

    // ── git_diff ────────────────────────────────────────────────────

    #[test]
    fn git_diff_clean_repo_empty_diff() {
        let tmp = init_temp_repo();
        let diff = git_diff(tmp.path()).unwrap();
        assert!(diff.is_empty(), "Clean repo should have empty diff");
    }

    #[test]
    fn git_diff_with_changes() {
        let tmp = init_temp_repo();
        std::fs::write(tmp.path().join("init.txt"), "changed").unwrap();
        let diff = git_diff(tmp.path()).unwrap();
        assert!(diff.contains("changed"), "Diff should show the change");
    }

    #[test]
    fn git_diff_nonexistent_dir() {
        let result = git_diff(Path::new("/nonexistent/repo"));
        assert!(result.is_err());
    }

    // ── git_commit ──────────────────────────────────────────────────

    #[test]
    fn git_commit_with_staged_changes() {
        let tmp = init_temp_repo();
        std::fs::write(tmp.path().join("new.txt"), "content").unwrap();
        let result = git_commit(tmp.path(), "add new file");
        assert!(result.is_ok());
        // Verify commit was made
        let log = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let log_text = String::from_utf8_lossy(&log.stdout);
        assert!(log_text.contains("add new file"));
    }

    #[test]
    fn git_commit_nothing_to_commit_fails() {
        let tmp = init_temp_repo();
        // Nothing changed, so git commit should fail
        let result = git_commit(tmp.path(), "empty commit");
        assert!(result.is_err());
    }

    #[test]
    fn git_commit_nonexistent_dir() {
        let result = git_commit(Path::new("/nonexistent/repo"), "msg");
        assert!(result.is_err());
    }

    // ── rollback ────────────────────────────────────────────────────

    #[test]
    fn rollback_restores_modified_files() {
        let tmp = init_temp_repo();
        let file = tmp.path().join("init.txt");
        std::fs::write(&file, "modified").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "modified");

        rollback(tmp.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello");
    }

    #[test]
    fn rollback_removes_untracked_files() {
        let tmp = init_temp_repo();
        let untracked = tmp.path().join("untracked.txt");
        std::fs::write(&untracked, "junk").unwrap();
        assert!(untracked.exists());

        rollback(tmp.path()).unwrap();
        assert!(!untracked.exists());
    }

    #[test]
    fn rollback_clean_repo_is_noop() {
        let tmp = init_temp_repo();
        let result = rollback(tmp.path());
        assert!(result.is_ok());
    }

    #[test]
    fn rollback_nonexistent_dir() {
        let result = rollback(Path::new("/nonexistent/repo"));
        assert!(result.is_err());
    }
}
