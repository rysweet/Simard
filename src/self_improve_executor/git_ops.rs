//! Git helper operations for the self-improvement executor.

use std::path::Path;
use std::process::Command;

use crate::error::{SimardError, SimardResult};

pub(crate) fn git_diff(workspace: &Path) -> SimardResult<String> {
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

    fn init_test_repo(ws: &Path) {
        for (args, label) in [
            (vec!["init"], "git init"),
            (
                vec!["config", "user.email", "test@test.com"],
                "git config email",
            ),
            (vec!["config", "user.name", "Test"], "git config name"),
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(ws)
                .output()
                .unwrap_or_else(|_| panic!("{label}"));
        }
        std::fs::write(ws.join("init.txt"), "init").expect("write init file");
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(ws)
            .output()
            .expect("git add");
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(ws)
            .output()
            .expect("git commit");
    }

    #[test]
    fn git_diff_empty_on_clean_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        init_test_repo(dir.path());
        let diff = git_diff(dir.path()).expect("git diff should succeed");
        assert!(diff.is_empty(), "clean repo should have empty diff");
    }

    #[test]
    fn git_diff_shows_modifications() {
        let dir = tempfile::TempDir::new().unwrap();
        init_test_repo(dir.path());
        std::fs::write(dir.path().join("init.txt"), "modified").expect("write");
        let diff = git_diff(dir.path()).expect("git diff should succeed");
        assert!(diff.contains("modified"), "diff should contain the change");
    }

    #[test]
    fn git_diff_fails_on_non_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = git_diff(dir.path());
        assert!(result.is_err(), "git diff should fail on non-repo");
    }

    #[test]
    fn git_commit_succeeds_with_staged_changes() {
        let dir = tempfile::TempDir::new().unwrap();
        init_test_repo(dir.path());
        std::fs::write(dir.path().join("new.txt"), "content").expect("write");
        git_commit(dir.path(), "test commit").expect("git commit should succeed");
        let log = Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(dir.path())
            .output()
            .expect("git log");
        let log_text = String::from_utf8_lossy(&log.stdout);
        assert!(log_text.contains("test commit"));
    }

    #[test]
    fn git_commit_fails_on_non_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = git_commit(dir.path(), "should fail");
        assert!(result.is_err(), "git commit should fail on non-repo");
    }

    #[test]
    fn rollback_restores_tracked_files() {
        let dir = tempfile::TempDir::new().unwrap();
        init_test_repo(dir.path());
        std::fs::write(dir.path().join("init.txt"), "modified").expect("write");
        rollback(dir.path()).expect("rollback should succeed");
        let contents = std::fs::read_to_string(dir.path().join("init.txt")).expect("read");
        assert_eq!(contents, "init");
    }

    #[test]
    fn rollback_removes_untracked_files() {
        let dir = tempfile::TempDir::new().unwrap();
        init_test_repo(dir.path());
        std::fs::write(dir.path().join("extra.txt"), "untracked").expect("write");
        rollback(dir.path()).expect("rollback should succeed");
        assert!(!dir.path().join("extra.txt").exists());
    }

    #[test]
    fn rollback_fails_on_non_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = rollback(dir.path());
        assert!(result.is_err(), "rollback should fail on non-repo");
    }
}
