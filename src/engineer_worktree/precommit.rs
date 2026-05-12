//! Best-effort `pre-commit install` for fresh engineer worktrees.
//!
//! When a per-engineer worktree is allocated, install the repo's pre-commit
//! hooks into it so the engineer's local commits are gated by the same
//! formatting, lint, and test fences that CI runs (#1641, #1581, #1607,
//! #1608, #1629, #1558, #1499 and several other PRs all failed CI on the
//! `pre-commit` job because the engineer never ran the hooks locally before
//! pushing).
//!
//! **Non-fatal**: a missing `pre-commit` binary, an absent `.pre-commit-config.yaml`,
//! or a non-zero exit from `pre-commit install` are all logged at WARN and
//! the worktree allocation still succeeds. The hooks are a productivity
//! improvement, not a correctness requirement — engineers can still produce
//! valid commits without them, and CI will catch anything they miss.
//!
//! **Security**: follows the same `env_clear()` + selective re-injection
//! pattern as [`crate::engineer_worktree::sweep::git_capture`] so a hostile
//! environment cannot hijack the subprocess via `LD_PRELOAD`,
//! `PRE_COMMIT_HOME`, or similar.

use std::path::Path;
use std::process::Command;

/// Install pre-commit hooks into a freshly-allocated worktree.
///
/// Returns `Ok(true)` if hooks were installed, `Ok(false)` if the operation
/// was skipped (no config, no binary), and `Err(reason)` only if the
/// subprocess could not be spawned at all. Callers in production treat all
/// outcomes as best-effort and never propagate the error.
pub fn install_hooks(worktree: &Path) -> Result<bool, String> {
    // Skip if the repo doesn't use pre-commit.
    let cfg = worktree.join(".pre-commit-config.yaml");
    if !cfg.exists() {
        return Ok(false);
    }

    // Skip if the pre-commit binary isn't on PATH. We don't want to fail
    // worktree allocation just because a developer hasn't `pip install`'d
    // pre-commit in this environment.
    if !pre_commit_on_path() {
        return Ok(false);
    }

    let mut cmd = Command::new("pre-commit");
    cmd.arg("install")
        .arg("--install-hooks")
        .current_dir(worktree)
        .env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("spawn pre-commit install: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "pre-commit install exited with {} in {}: {}",
            output.status,
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(true)
}

/// Return `true` if `pre-commit` is resolvable on `PATH`.
fn pre_commit_on_path() -> bool {
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    for dir in std::env::split_paths(&path) {
        for candidate in ["pre-commit", "pre-commit.exe"] {
            let p = dir.join(candidate);
            if p.is_file() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn install_hooks_skips_when_config_missing() {
        let dir = tempfile::tempdir().unwrap();
        let result = install_hooks(dir.path()).unwrap();
        assert!(
            !result,
            "expected skip (Ok(false)) when .pre-commit-config.yaml is absent"
        );
    }

    #[test]
    fn install_hooks_skips_silently_when_binary_missing() {
        let dir = tempfile::tempdir().unwrap();
        // Create a config so we go past the first early return.
        fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();

        // Save and clear PATH so the binary lookup fails.
        let saved_path = std::env::var_os("PATH");
        // SAFETY: tests run sequentially in this module's scope; restored below.
        unsafe {
            std::env::set_var("PATH", "/nonexistent-dir-for-precommit-test");
        }

        let result = install_hooks(dir.path());

        // Restore PATH before any assertions so a panic doesn't leak the change.
        if let Some(p) = saved_path {
            unsafe {
                std::env::set_var("PATH", p);
            }
        } else {
            unsafe {
                std::env::remove_var("PATH");
            }
        }

        assert!(
            !result.unwrap(),
            "expected skip (Ok(false)) when pre-commit binary is not on PATH"
        );
    }

    #[test]
    fn install_hooks_succeeds_in_real_git_repo_with_real_pre_commit() {
        // Skip when the test environment has no pre-commit binary — the
        // production callsite treats that as Ok(false) too.
        if !pre_commit_on_path() {
            eprintln!("skipping: pre-commit not on PATH");
            return;
        }

        let dir = tempfile::tempdir().unwrap();

        // Initialize a real git repo so pre-commit has somewhere to install
        // the hook script.
        let git = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(git.success(), "git init failed");

        // Minimal valid config — empty repos list is accepted by pre-commit.
        fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();

        let result = install_hooks(dir.path()).unwrap();
        assert!(result, "expected install_hooks to install (Ok(true))");

        // Verify the hook script actually appeared.
        let hook = dir.path().join(".git").join("hooks").join("pre-commit");
        assert!(
            hook.exists(),
            "expected .git/hooks/pre-commit to exist after install_hooks"
        );
    }
}
