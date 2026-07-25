//! Best-effort native git-hook enrollment for fresh engineer worktrees.
//!
//! When a per-engineer worktree is allocated, wire `core.hooksPath` to the
//! repo's committed `hooks/` directory so the engineer's local commits are
//! gated by the same formatting, lint, and test fences that CI runs (#1641,
//! #1581, #1607, #1608, #1629, #1558, #1499 and several other PRs all failed
//! CI because the engineer never ran the hooks locally before pushing).
//!
//! This replaces the former Python `pre-commit` framework install (#3181):
//! Simard is a pure-Rust daemon, so the committed `hooks/pre-commit` and
//! `hooks/pre-push` shell out to `cargo` directly and there is no Python
//! runtime dependency. A single `core.hooksPath` setting installs both stages
//! (git dispatches by hook filename); the relative `hooks` path resolves to
//! each worktree's own committed hooks at hook-run time.
//!
//! **Non-fatal**: an absent `hooks/` directory or a non-zero `git config`
//! exit are logged at WARN and the worktree allocation still succeeds. The
//! hooks are a productivity improvement, not a correctness requirement —
//! engineers can still produce valid commits without them, and CI will catch
//! anything they miss.
//!
//! **Security**: follows the same `env_clear()` + selective re-injection
//! pattern as [`crate::engineer_worktree::sweep::git_capture`] so a hostile
//! environment cannot hijack the subprocess via `LD_PRELOAD` or similar.

use std::path::Path;
use std::process::Command;

/// Wire the committed native git hooks into a freshly-allocated worktree by
/// pointing `core.hooksPath` at the repo's `hooks/` directory.
///
/// Returns `Ok(true)` if the hooks path was configured, `Ok(false)` if the
/// operation was skipped (the committed `hooks/` directory is absent), and
/// `Err(reason)` only if the `git config` subprocess could not be spawned or
/// exited non-zero. Callers in production treat all outcomes as best-effort
/// and never propagate the error.
pub fn install_hooks(worktree: &Path) -> Result<bool, String> {
    // Skip if the committed native hooks aren't present in this checkout.
    let hooks_dir = worktree.join("hooks");
    if !hooks_dir.join("pre-commit").is_file() || !hooks_dir.join("pre-push").is_file() {
        return Ok(false);
    }

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(worktree)
        .args(["config", "core.hooksPath", "hooks"])
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

    #[test]
    fn install_hooks_skips_when_hooks_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let result = install_hooks(dir.path()).unwrap();
        assert!(
            !result,
            "expected skip (Ok(false)) when the committed hooks/ directory is absent"
        );
    }

    #[test]
    fn install_hooks_wires_core_hookspath_in_real_git_repo() {
        let dir = tempfile::tempdir().unwrap();

        // Initialize a real git repo so `git config` has somewhere to write.
        let git = crate::util::spawn_retry::retry_spawn_sync(|| {
            Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(dir.path())
                .status()
        })
        .unwrap();
        assert!(git.success(), "git init failed");

        // Materialize the committed hooks so the presence check passes.
        let hooks = dir.path().join("hooks");
        fs::create_dir_all(&hooks).unwrap();
        fs::write(hooks.join("pre-commit"), "#!/usr/bin/env bash\ncargo fmt\n").unwrap();
        fs::write(hooks.join("pre-push"), "#!/usr/bin/env bash\ncargo test\n").unwrap();

        let result = install_hooks(dir.path()).unwrap();
        assert!(
            result,
            "expected install_hooks to wire core.hooksPath (Ok(true))"
        );

        // Verify git now points at the committed hooks directory.
        let out = crate::util::spawn_retry::retry_spawn_sync(|| {
            Command::new("git")
                .args(["config", "--get", "core.hooksPath"])
                .current_dir(dir.path())
                .output()
        })
        .unwrap();
        assert!(
            out.status.success(),
            "git config --get core.hooksPath failed"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "hooks",
            "core.hooksPath should be wired to the committed hooks/ directory"
        );
    }
}
