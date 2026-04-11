//! Git guardrails — prevent destructive operations on protected repositories.
//!
//! The OODA daemon runs autonomously and can execute git operations. This module
//! ensures it never performs destructive operations (force push, reset --hard,
//! branch -D on main/release) on protected repository paths.

use std::path::Path;

/// Destructive git operations that are always blocked.
const BLOCKED_PATTERNS: &[&str] = &[
    "push --force",
    "push -f",
    "reset --hard",
    "branch -D main",
    "branch -D release",
    "branch -D master",
    "clean -fdx",
    "reflog expire",
    "gc --prune=now --aggressive",
];

/// Check whether `SIMARD_GIT_GUARDRAILS` is enabled (default: enabled).
fn guardrails_enabled() -> bool {
    std::env::var("SIMARD_GIT_GUARDRAILS")
        .map(|v| !matches!(v.as_str(), "0" | "false" | "disabled"))
        .unwrap_or(true)
}

/// Protected repo root paths (from `SIMARD_GIT_PROTECTED_REPOS`, colon-separated).
fn protected_roots() -> Vec<String> {
    std::env::var("SIMARD_GIT_PROTECTED_REPOS")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Returns `Err` with a descriptive message if the proposed git command would
/// violate guardrails. Returns `Ok(())` if the command is safe to execute.
pub fn check_git_safety(workspace: &Path, args: &[&str]) -> Result<(), String> {
    if !guardrails_enabled() {
        return Ok(());
    }

    let cmd_str = args.join(" ");

    // Block globally-destructive patterns regardless of repo path.
    for pattern in BLOCKED_PATTERNS {
        if cmd_str.contains(pattern) {
            return Err(format!(
                "GUARDRAIL BLOCKED: 'git {cmd_str}' matches destructive pattern '{pattern}'. \
                 Destructive git operations are not permitted in autonomous mode."
            ));
        }
    }

    // If workspace is under a protected root, block all write operations
    // except: add, commit, checkout (non-force), branch (create), push (non-force), pull, fetch, stash.
    let ws = workspace.to_string_lossy();
    let roots = protected_roots();
    let is_protected = roots.iter().any(|root| ws.starts_with(root));

    if is_protected {
        let first_arg = args.first().copied().unwrap_or("");
        let safe_commands = [
            "add", "commit", "checkout", "branch", "push", "pull", "fetch",
            "stash", "status", "log", "diff", "show", "tag", "remote",
            "config", "rev-parse",
        ];
        if !safe_commands.contains(&first_arg) {
            return Err(format!(
                "GUARDRAIL BLOCKED: 'git {first_arg}' is not in the safe command list \
                 for protected repo at {ws}. Safe commands: {safe_commands:?}"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn blocks_force_push() {
        let result = check_git_safety(
            &PathBuf::from("/home/user/src/repo"),
            &["push", "--force", "origin", "main"],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("GUARDRAIL BLOCKED"));
    }

    #[test]
    fn blocks_reset_hard() {
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["reset", "--hard", "HEAD~1"],
        );
        assert!(result.is_err());
    }

    #[test]
    fn allows_normal_push() {
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "enabled") };
        unsafe { std::env::remove_var("SIMARD_GIT_PROTECTED_REPOS") };
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["push", "origin", "feature-branch"],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn allows_commit() {
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["commit", "-m", "fix: stuff"],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn disabled_allows_everything() {
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "disabled") };
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["push", "--force", "origin", "main"],
        );
        assert!(result.is_ok());
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "enabled") };
    }

    #[test]
    fn blocks_delete_main_branch() {
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["branch", "-D", "main"],
        );
        assert!(result.is_err());
    }
}
