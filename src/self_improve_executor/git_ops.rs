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
    let output = Command::new("git")
        .args(["checkout", "--", "."])
        .current_dir(workspace)
        .output()
        .map_err(|e| SimardError::GitCommandFailed {
            command: "git checkout -- .".into(),
            reason: e.to_string(),
        })?;

    if !output.status.success() {
        return Err(SimardError::GitCommandFailed {
            command: "git checkout -- .".into(),
            reason: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    Ok(())
}
