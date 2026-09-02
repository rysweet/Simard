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
    // Observe-only floor (issue #1, Crocutus): a read-only identity sets
    // `SIMARD_OBSERVE_ONLY=1`; under it, EVERY mutating git verb (push, commit,
    // branch, tag, merge, rebase, reset, checkout -b, am, apply, update-ref, …)
    // is refused here at the shared write seam. This runs BEFORE the
    // `guardrails_enabled()` gate on purpose: the observe-only guarantee must
    // hold even if `SIMARD_GIT_GUARDRAILS` is disabled — fail closed. It is a
    // no-op for the engineer identity (env unset).
    crate::read_only_guard::guard_observe_only_git(args)?;

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
            "add",
            "commit",
            "checkout",
            "branch",
            "push",
            "pull",
            "fetch",
            "stash",
            "status",
            "log",
            "diff",
            "show",
            "tag",
            "remote",
            "config",
            "rev-parse",
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
    use std::sync::Mutex;

    // Tests in this module mutate process-global SIMARD_GIT_* env vars.
    // Cargo runs unit tests in parallel by default, so a Mutex is used
    // to serialize them and prevent the disabled-everywhere flag from
    // leaking into block_* tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn reset_env() {
        unsafe {
            std::env::remove_var("SIMARD_GIT_GUARDRAILS");
            std::env::remove_var("SIMARD_GIT_PROTECTED_REPOS");
            // Ensure the observe-only floor does not leak between tests.
            std::env::remove_var(crate::read_only_guard::OBSERVE_ONLY_ENV);
        }
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_force_push() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(
            &PathBuf::from("/home/user/src/repo"),
            &["push", "--force", "origin", "main"],
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("GUARDRAIL BLOCKED"));
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_reset_hard() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["reset", "--hard", "HEAD~1"]);
        let err = result.expect_err("reset --hard must be blocked");
        assert!(
            err.contains("GUARDRAIL BLOCKED"),
            "unexpected message: {err}"
        );
        assert!(
            err.contains("reset --hard"),
            "message must name the matched pattern: {err}"
        );
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn allows_normal_push() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["push", "origin", "feature-branch"],
        );
        assert!(result.is_ok());
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn allows_commit() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["commit", "-m", "fix: stuff"]);
        assert!(result.is_ok());
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn disabled_allows_everything() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "disabled") };
        let result = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["push", "--force", "origin", "main"],
        );
        assert!(result.is_ok());
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_delete_main_branch() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["branch", "-D", "main"]);
        let err = result.expect_err("branch -D main must be blocked");
        assert!(
            err.contains("GUARDRAIL BLOCKED"),
            "unexpected message: {err}"
        );
        assert!(
            err.contains("branch -D main"),
            "message must name the matched pattern: {err}"
        );
    }

    // --- Additional blocked-pattern coverage (each destructive pattern) ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_force_push_short_flag() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["push", "-f", "origin", "main"],
        )
        .expect_err("push -f must be blocked");
        assert!(
            err.contains("GUARDRAIL BLOCKED") && err.contains("push -f"),
            "{err}"
        );
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_clean_fdx() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(&PathBuf::from("/tmp/repo"), &["clean", "-fdx"])
            .expect_err("clean -fdx must be blocked");
        assert!(err.contains("clean -fdx"), "{err}");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_reflog_expire() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["reflog", "expire", "--all", "--expire=now"],
        )
        .expect_err("reflog expire must be blocked");
        assert!(err.contains("reflog expire"), "{err}");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_gc_prune_aggressive() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(
            &PathBuf::from("/tmp/repo"),
            &["gc", "--prune=now", "--aggressive"],
        )
        .expect_err("gc --prune=now --aggressive must be blocked");
        assert!(err.contains("gc --prune=now --aggressive"), "{err}");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_delete_release_branch() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(&PathBuf::from("/tmp/repo"), &["branch", "-D", "release"])
            .expect_err("branch -D release must be blocked");
        assert!(err.contains("branch -D release"), "{err}");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn blocks_delete_master_branch() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let err = check_git_safety(&PathBuf::from("/tmp/repo"), &["branch", "-D", "master"])
            .expect_err("branch -D master must be blocked");
        assert!(err.contains("branch -D master"), "{err}");
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn allows_delete_feature_branch() {
        // Only main/release/master deletions are globally blocked; a normal
        // feature-branch deletion must pass when the repo is not protected.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["branch", "-D", "feature-x"]);
        assert!(result.is_ok(), "{result:?}");
    }

    // --- guardrails_enabled() toggle variants ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn flag_zero_disables_guardrails() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "0") };
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["push", "--force"]);
        assert!(result.is_ok(), "0 must disable guardrails: {result:?}");
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn flag_false_disables_guardrails() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "false") };
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["reset", "--hard"]);
        assert!(result.is_ok(), "false must disable guardrails: {result:?}");
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn flag_unrecognized_value_keeps_guardrails_enabled() {
        // Any value other than 0/false/disabled leaves guardrails ON.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_GUARDRAILS", "1") };
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &["push", "--force"]);
        assert!(
            result.is_err(),
            "an unrecognized flag value must NOT disable guardrails: {result:?}"
        );
        reset_env();
    }

    // --- Protected-repo enforcement (the previously untested branch) ---

    const PROT_ROOT: &str = "/srv/simard-protected";

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn protected_repo_blocks_command_outside_safe_list() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", PROT_ROOT) };
        let ws = PathBuf::from(format!("{PROT_ROOT}/checkout"));
        let err = check_git_safety(&ws, &["rebase", "-i", "HEAD~3"])
            .expect_err("rebase in a protected repo must be blocked");
        assert!(err.contains("GUARDRAIL BLOCKED"), "{err}");
        assert!(
            err.contains("rebase") && err.contains("safe command list"),
            "message must name the rejected command + safe-list context: {err}"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn protected_repo_allows_each_safe_command() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", PROT_ROOT) };
        let ws = PathBuf::from(format!("{PROT_ROOT}/checkout"));
        // A representative sample across the safe list, including the boundary
        // entries (`add` first, `rev-parse` last).
        for args in [
            &["add", "."][..],
            &["commit", "-m", "msg"][..],
            &["checkout", "-b", "feature"][..],
            &["status"][..],
            &["rev-parse", "HEAD"][..],
        ] {
            let result = check_git_safety(&ws, args);
            assert!(
                result.is_ok(),
                "safe command {args:?} must be allowed in a protected repo: {result:?}"
            );
        }
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn unprotected_repo_allows_command_outside_safe_list() {
        // The safe-list restriction applies ONLY to protected roots. A workspace
        // outside every protected root may run any (non-globally-destructive) cmd.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", PROT_ROOT) };
        let ws = PathBuf::from("/home/user/some-other-repo");
        let result = check_git_safety(&ws, &["rebase", "-i", "HEAD~3"]);
        assert!(
            result.is_ok(),
            "rebase outside protected roots must be allowed: {result:?}"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn protected_repos_matches_any_of_multiple_colon_separated_roots() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe {
            std::env::set_var(
                "SIMARD_GIT_PROTECTED_REPOS",
                "/opt/first-root:/srv/second-root",
            )
        };
        // Workspace lives under the SECOND configured root.
        let ws = PathBuf::from("/srv/second-root/repo");
        let err = check_git_safety(&ws, &["merge", "other"])
            .expect_err("merge under a protected root must be blocked");
        assert!(
            err.contains("GUARDRAIL BLOCKED") && err.contains("merge"),
            "{err}"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn protected_repos_ignores_empty_colon_entries() {
        // Leading/trailing/duplicate colons must be filtered, not treated as a
        // root that matches every path.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", format!("::{PROT_ROOT}::")) };
        // A path that does NOT start with the real root must be treated as
        // unprotected (an empty "" root would otherwise prefix-match everything).
        let outside = PathBuf::from("/home/user/repo");
        assert!(
            check_git_safety(&outside, &["rebase"]).is_ok(),
            "empty entries must not make every path protected"
        );
        // The real root still matches.
        let inside = PathBuf::from(format!("{PROT_ROOT}/x"));
        assert!(
            check_git_safety(&inside, &["rebase"]).is_err(),
            "the non-empty root must still protect its subtree"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn empty_protected_repos_env_protects_nothing() {
        // Empty env => protected_roots() is empty => no repo is protected.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", "") };
        let result = check_git_safety(&PathBuf::from("/anything/at/all"), &["rebase"]);
        assert!(result.is_ok(), "{result:?}");
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn global_destructive_pattern_beats_protected_safe_list() {
        // A globally-destructive pattern is rejected by the pattern check BEFORE
        // the protected-repo safe-list check, so the reported reason is the
        // destructive-pattern message even inside a protected repo.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", PROT_ROOT) };
        let ws = PathBuf::from(format!("{PROT_ROOT}/checkout"));
        let err = check_git_safety(&ws, &["push", "--force", "origin", "main"])
            .expect_err("force push must be blocked");
        assert!(
            err.contains("destructive pattern"),
            "the global-pattern reason must win: {err}"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn empty_args_in_protected_repo_are_blocked() {
        // `args.first()` is None => first_arg == "" which is not in the safe
        // list => blocked. Exercises the empty-args fallthrough.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var("SIMARD_GIT_PROTECTED_REPOS", PROT_ROOT) };
        let ws = PathBuf::from(format!("{PROT_ROOT}/checkout"));
        let result = check_git_safety(&ws, &[]);
        assert!(
            result.is_err(),
            "an empty git invocation in a protected repo must be blocked: {result:?}"
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn empty_args_in_unprotected_repo_are_allowed() {
        // No protected root + no destructive pattern => empty args are Ok.
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        let result = check_git_safety(&PathBuf::from("/tmp/repo"), &[]);
        assert!(result.is_ok(), "{result:?}");
    }

    // ── Observe-only floor (issue #1, Crocutus) ─────────────────────────────
    // With SIMARD_OBSERVE_ONLY set, the shared git write seam refuses every
    // mutating verb — even ordinary push/commit that the engineer identity is
    // allowed to run — and even when SIMARD_GIT_GUARDRAILS is disabled.

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn observe_only_blocks_ordinary_push_and_commit() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var(crate::read_only_guard::OBSERVE_ONLY_ENV, "1") };
        for args in [
            vec!["push", "origin", "feature-branch"],
            vec!["commit", "-m", "hygiene fix"],
            vec!["checkout", "-b", "hygiene"],
            vec!["tag", "v1.0"],
            vec!["merge", "feature"],
        ] {
            let result = check_git_safety(&PathBuf::from("/tmp/clone"), &args);
            assert!(
                result.is_err(),
                "observe-only must refuse the write `git {}`",
                args.join(" ")
            );
            assert!(result.unwrap_err().contains("GUARDRAIL BLOCKED"));
        }
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn observe_only_still_allows_reads() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe { std::env::set_var(crate::read_only_guard::OBSERVE_ONLY_ENV, "1") };
        for args in [
            vec!["fetch", "--all", "--prune"],
            vec!["log", "--oneline", "-20"],
            vec!["status"],
            vec!["branch", "-a"],
            vec!["for-each-ref", "refs/remotes"],
        ] {
            assert!(
                check_git_safety(&PathBuf::from("/tmp/clone"), &args).is_ok(),
                "observe-only must permit the read `git {}`",
                args.join(" ")
            );
        }
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn observe_only_floor_holds_even_when_git_guardrails_disabled() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        unsafe {
            std::env::set_var("SIMARD_GIT_GUARDRAILS", "disabled");
            std::env::set_var(crate::read_only_guard::OBSERVE_ONLY_ENV, "1");
        }
        // Even with the destructive-op guardrails switched off, the observe-only
        // floor must fail closed on a push (defense in depth).
        assert!(
            check_git_safety(&PathBuf::from("/tmp/clone"), &["push", "origin", "main"]).is_err()
        );
        reset_env();
    }

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn engineer_identity_unaffected_by_floor_when_env_unset() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset_env();
        // Env unset ⇒ the observe-only floor is a no-op; ordinary push allowed.
        assert!(
            check_git_safety(&PathBuf::from("/tmp/repo"), &["push", "origin", "feature"]).is_ok()
        );
    }
}
