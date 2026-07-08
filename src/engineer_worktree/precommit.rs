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

    /// Initialize a minimal real git repo so `git config` has somewhere to
    /// persist `core.hooksPath`.
    fn git_init(dir: &Path) {
        let ok = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .status()
            .expect("[simard] git init should run")
            .success();
        assert!(ok, "[simard] git init failed in {}", dir.display());
    }

    /// Read a local git config value, or `None` if unset.
    fn git_config_get(dir: &Path, key: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["config", "--local", "--get", key])
            .current_dir(dir)
            .output()
            .expect("[simard] git config should run");
        if out.status.success() {
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            None
        }
    }

    /// Contract: when the repo has NO committed `hooks/` directory there is
    /// nothing to wire — install_hooks skips (Ok(false)) and leaves
    /// `core.hooksPath` unset. It must never require `.pre-commit-config.yaml`
    /// or the Python `pre-commit` binary.
    #[test]
    fn install_hooks_skips_when_no_committed_hooks_dir() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());

        let result = install_hooks(dir.path()).unwrap();
        assert!(
            !result,
            "[simard] expected Ok(false) skip when repo has no committed hooks/ dir"
        );
        assert!(
            git_config_get(dir.path(), "core.hooksPath").is_none(),
            "[simard] core.hooksPath must stay unset when there is no hooks/ dir"
        );
    }

    /// Contract: when the repo ships committed native hooks (`hooks/pre-commit`),
    /// install_hooks wires the worktree's `core.hooksPath` to that directory so
    /// local commits run the same cargo gates as CI — with no Python framework.
    #[test]
    fn install_hooks_wires_core_hooks_path_to_committed_hooks() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());

        // Simulate the repo's committed native hooks.
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(
            hooks.join("pre-commit"),
            "#!/usr/bin/env bash\ncargo fmt --check\n",
        )
        .unwrap();

        let result = install_hooks(dir.path()).unwrap();
        assert!(
            result,
            "[simard] expected Ok(true) when committed hooks/ dir is present"
        );

        let configured = git_config_get(dir.path(), "core.hooksPath")
            .expect("[simard] core.hooksPath should be set after install_hooks");
        assert_eq!(
            configured, "hooks",
            "[simard] core.hooksPath must point at the committed hooks/ dir"
        );
    }

    /// Contract: installing the native hooks must not depend on a
    /// `.pre-commit-config.yaml` file (that was the Python framework's config).
    /// A repo with committed hooks but no such file must still wire hooksPath.
    #[test]
    fn install_hooks_does_not_require_pre_commit_config_yaml() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());

        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "#!/usr/bin/env bash\ncargo fmt\n").unwrap();
        assert!(
            !dir.path().join(".pre-commit-config.yaml").exists(),
            "[simard] test precondition: no .pre-commit-config.yaml present"
        );

        let result = install_hooks(dir.path()).unwrap();
        assert!(
            result,
            "[simard] native hook install must not require .pre-commit-config.yaml"
        );
    }
}
