//! Wire committed native git hooks into fresh engineer worktrees (#3181).
//!
//! When a per-engineer worktree is allocated, point its `core.hooksPath` at the
//! repo's committed `hooks/` directory so the engineer's local commits and
//! pushes are gated by the same formatting, lint, and test fences that CI runs
//! (#1641, #1581, #1607, #1608, #1629, #1558, #1499 and several other PRs all
//! failed CI because the engineer never ran the hooks locally before pushing).
//!
//! Simard is a pure-Rust daemon: the hooks shell out to `cargo` directly and
//! there is no Python `pre-commit` framework, no `.pre-commit-config.yaml`, and
//! no `pip install` — see `hooks/pre-commit` and `hooks/pre-push`.
//!
//! **Non-fatal**: a repo without a committed `hooks/` directory simply skips
//! (`Ok(false)`); only a failed `git config` invocation returns `Err`. The
//! hooks are a productivity improvement, not a correctness requirement — CI is
//! still the source of truth.
//!
//! **Security**: follows the same `env_clear()` + selective re-injection
//! pattern as [`crate::engineer_worktree::sweep::git_capture`] so a hostile
//! environment cannot hijack the `git config` subprocess via `LD_PRELOAD` or
//! similar.

use std::path::Path;
use std::process::Command;

/// Wire committed native git hooks into a freshly-allocated worktree.
///
/// Sets `core.hooksPath` to the repo's committed `hooks/` directory when it
/// ships a `hooks/pre-commit` hook. Returns `Ok(true)` when the path was wired,
/// `Ok(false)` when the operation was skipped (no committed `hooks/` dir), and
/// `Err(reason)` only if `git config` could not be run. Callers in production
/// treat all outcomes as best-effort and never propagate the error.
pub fn install_hooks(worktree: &Path) -> Result<bool, String> {
    // Skip if the repo doesn't ship committed native hooks. `hooks/pre-commit`
    // is the sentinel: without it there is nothing to wire.
    if !worktree.join("hooks").join("pre-commit").is_file() {
        return Ok(false);
    }

    // Point git at the committed hooks/ dir (relative to the worktree root).
    // `--local` scopes the setting to this worktree's config.
    let mut cmd = Command::new("git");
    cmd.args(["config", "--local", "core.hooksPath", "hooks"])
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
        .map_err(|e| format!("spawn git config core.hooksPath: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "git config core.hooksPath exited with {} in {}: {}",
            output.status,
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(true)
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
