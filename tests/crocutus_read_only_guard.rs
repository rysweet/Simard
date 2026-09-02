//! Crocutus read-only guardrail — outside-in proof of the #1 non-negotiable
//! guardrail (issue #1) plus the identity-isolation contract that lets a second
//! identity share Simard's binary.
//!
//! These tests are written FIRST (TDD). They exercise Simard's PUBLIC surface
//! only — no Crocutus code is copied. Crocutus is meant to be a distinct
//! *identity* over this same shared codebase, so the observe-only floor and the
//! state-root-relative goal board MUST hold here.
//!
//! ## What is proven
//!
//! 1. **Fail-closed observe-only floor.** For an observe-only identity, EVERY
//!    mutating command against a target repo / AzDO project is refused —
//!    `git push`, `commit`, `branch`, PR creation (`az repos pr create` /
//!    `gh pr create`), work-item edits (`az boards work-item update`), ACL
//!    changes, and raw HTTP writes (`az rest --method POST`, `curl -d`). Reads
//!    are permitted. Ambiguity fails closed.
//!
//! 2. **Identity isolation.** The goal board is resolved *relative to the state
//!    root*, so a Crocutus home (`~/.crocutus`) yields a goal board distinct
//!    from Simard's (`~/.simard`) — the two identities do not share state.

use std::path::{Path, PathBuf};

use simard::read_only_guard::{
    OBSERVE_ONLY_ENV, check_observe_only, check_observe_only_git, command_is_read,
    guard_observe_only, is_write_command, observe_only_enabled,
};

/// The exhaustive set of TARGET-mutating command shapes that Crocutus must
/// never be able to run against the hyenas repos or their AzDO project.
///
/// If a NEW write path is added to the framework, add it here first (it must be
/// refused) — this list is the executable specification of "changes nothing,
/// anywhere".
const MUST_BLOCK: &[&[&str]] = &[
    // --- git: commit / push / branch / history rewrite ---
    &["git", "push", "origin", "main"],
    &["git", "push", "--force", "origin", "main"],
    &["git", "push", "--force-with-lease", "origin", "main"],
    &["git", "commit", "-m", "hygiene fix"],
    &["git", "commit", "--amend", "--no-edit"],
    &["git", "add", "-A"],
    &["git", "merge", "feature"],
    &["git", "rebase", "-i", "main"],
    &["git", "reset", "--hard", "HEAD~1"],
    &["git", "cherry-pick", "deadbeef"],
    &["git", "revert", "deadbeef"],
    &["git", "checkout", "-b", "hygiene"],
    &["git", "switch", "-c", "hygiene"],
    &["git", "branch", "hygiene"],
    &["git", "branch", "-D", "stale-branch"],
    &["git", "tag", "v9.9.9"],
    &["git", "tag", "-d", "v1"],
    &["git", "remote", "add", "mirror", "https://x"],
    &["git", "remote", "set-url", "origin", "https://x"],
    &["git", "config", "user.email", "x@y.z"],
    &["git", "pull", "origin", "main"],
    &["git", "am", "patch.mbox"],
    &["git", "apply", "patch.diff"],
    &["git", "update-ref", "refs/heads/main", "deadbeef"],
    // leading git globals must not smuggle a write past the guard
    &["git", "-C", "/clone", "push", "origin", "main"],
    // --- Azure DevOps: PRs, work items, policies, ACLs ---
    &["az", "repos", "pr", "create", "--title", "x"],
    &["az", "repos", "pr", "update", "--id", "1"],
    &[
        "az", "repos", "pr", "set-vote", "--id", "1", "--vote", "approve",
    ],
    &["az", "repos", "pr", "reviewer", "add", "--id", "1"],
    &["az", "repos", "ref", "create", "--name", "refs/heads/x"],
    &["az", "repos", "ref", "delete", "--name", "refs/heads/x"],
    &[
        "az",
        "repos",
        "import",
        "create",
        "--git-source-url",
        "https://x",
    ],
    &["az", "repos", "policy", "create"],
    &["az", "boards", "work-item", "create", "--type", "Bug"],
    &["az", "boards", "work-item", "update", "--id", "1"],
    &["az", "boards", "work-item", "delete", "--id", "1"],
    &["az", "boards", "work-item", "relation", "add", "--id", "1"],
    &["az", "pipelines", "run", "--name", "ci"],
    &["az", "pipelines", "create", "--name", "ci"],
    &["az", "devops", "security", "permission", "update"],
    &["az", "devops", "security", "group", "membership", "add"],
    // az rest raw writes
    &[
        "az",
        "rest",
        "--method",
        "POST",
        "--url",
        "https://dev.azure.com/x",
    ],
    &[
        "az",
        "rest",
        "-m",
        "patch",
        "--url",
        "https://dev.azure.com/x",
    ],
    &[
        "az",
        "rest",
        "--url",
        "https://dev.azure.com/x",
        "--body",
        "@payload.json",
    ],
    // --- GitHub CLI writes (Crocutus must not act on GitHub either) ---
    &["gh", "pr", "create"],
    &["gh", "pr", "merge", "1"],
    &["gh", "issue", "create"],
    &["gh", "issue", "comment", "1", "--body", "x"],
    &["gh", "api", "-X", "POST", "/repos/x/pulls"],
    &["gh", "api", "-f", "title=x", "/repos/x/issues"],
    // --- raw HTTP writes ---
    &["curl", "-X", "POST", "https://dev.azure.com/x"],
    &["curl", "-d", "@payload", "https://dev.azure.com/x"],
    &["curl", "-XPUT", "https://dev.azure.com/x"],
    &["wget", "--post-data", "a=b", "https://dev.azure.com/x"],
];

/// Read-only command shapes Crocutus MUST be able to run so it can actually
/// observe the target repos.
const MUST_ALLOW: &[&[&str]] = &[
    &[
        "git",
        "clone",
        "https://dev.azure.com/acs-mdash/acs-mdash/_git/hyenas",
    ],
    &["git", "fetch", "--all", "--prune"],
    &["git", "log", "--oneline", "-20"],
    &["git", "status"],
    &["git", "diff", "origin/main...HEAD"],
    &["git", "show", "HEAD:README.md"],
    &["git", "ls-remote", "--heads", "origin"],
    &[
        "git",
        "for-each-ref",
        "--sort=-committerdate",
        "refs/remotes",
    ],
    &["git", "branch", "-a"],
    &["git", "branch", "--merged", "origin/main"],
    &["git", "tag", "-l"],
    &["git", "rev-parse", "HEAD"],
    &["git", "config", "--get", "remote.origin.url"],
    &["git", "-C", "/clone", "log"],
    &["az", "repos", "list", "--project", "acs-mdash"],
    &["az", "repos", "pr", "list", "--status", "active"],
    &["az", "boards", "work-item", "show", "--id", "1"],
    &["az", "repos", "ref", "list", "--repository", "hyenas"],
    &[
        "az",
        "rest",
        "--method",
        "GET",
        "--url",
        "https://dev.azure.com/x",
    ],
    &["az", "rest", "--url", "https://dev.azure.com/x"],
    &["gh", "pr", "list"],
    &["gh", "api", "/repos/x/pulls"],
    &["curl", "https://dev.azure.com/x"],
    &["curl", "-sSL", "-X", "GET", "https://dev.azure.com/x"],
];

#[test]
fn every_mutating_command_is_refused() {
    for argv in MUST_BLOCK {
        let result = check_observe_only(argv);
        assert!(
            result.is_err(),
            "GUARDRAIL HOLE: observe-only identity was allowed to run a mutating \
             command {argv:?} — this could change the target repos. Must fail closed."
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("GUARDRAIL BLOCKED"),
            "block message for {argv:?} must carry the GUARDRAIL BLOCKED marker, got: {msg}"
        );
        assert!(
            is_write_command(argv),
            "is_write_command must agree that {argv:?} is a write"
        );
    }
}

#[test]
fn every_read_command_is_permitted() {
    for argv in MUST_ALLOW {
        assert!(
            check_observe_only(argv).is_ok(),
            "observe-only identity must be able to READ: {argv:?} was refused, \
             which would prevent Crocutus from observing the target repos."
        );
        assert!(
            command_is_read(argv),
            "command_is_read must agree that {argv:?} is a read"
        );
    }
}

#[test]
fn git_arg_convention_matches_full_argv() {
    // The git-args-without-"git" convention (mirroring git_guardrails) must
    // agree with the full-argv path.
    assert!(check_observe_only_git(&["push", "origin", "main"]).is_err());
    assert!(check_observe_only_git(&["fetch", "origin"]).is_ok());
    assert!(check_observe_only_git(&["commit", "-m", "x"]).is_err());
    assert!(check_observe_only_git(&["log"]).is_ok());
}

#[test]
fn env_gate_defaults_off_and_can_be_forced_on() {
    // With the env unset, the env-gated entry point does not enforce (Simard,
    // the engineering identity, is free to write). The always-on check still
    // refuses. The Crocutus deployment sets OBSERVE_ONLY_ENV to force the gate.
    //
    // We only READ the env default here (do not mutate process-global env in a
    // parallel integration test): default must be "not observe-only".
    assert!(
        !observe_only_enabled(),
        "{OBSERVE_ONLY_ENV} must default to disabled so the engineer identity is unaffected"
    );
    // Env-gated path is a no-op when disabled …
    assert!(guard_observe_only(&["git", "push", "origin", "main"]).is_ok());
    // … but the always-on path (used by the Crocutus identity) always refuses.
    assert!(check_observe_only(&["git", "push", "origin", "main"]).is_err());
}

#[test]
fn identity_isolation_goal_board_is_state_root_relative() {
    // Crocutus runs with its own home (~/.crocutus) via SIMARD_STATE_ROOT.
    // The goal board is resolved relative to that root, so the two identities'
    // goal boards are DISTINCT files — no shared mutable state.
    let crocutus_home: PathBuf = Path::new("/home/agent/.crocutus").to_path_buf();
    let simard_home: PathBuf = Path::new("/home/agent/.simard").to_path_buf();

    let crocutus_board = simard::goal_board_store::store_path(&crocutus_home);
    let simard_board = simard::goal_board_store::store_path(&simard_home);

    assert_ne!(
        crocutus_board, simard_board,
        "Crocutus and Simard must not share a goal board"
    );
    assert!(
        crocutus_board.starts_with(&crocutus_home),
        "Crocutus goal board must live under its own home, got {crocutus_board:?}"
    );
    assert!(
        crocutus_board.to_string_lossy().contains(".crocutus"),
        "Crocutus goal board path must be scoped to .crocutus, got {crocutus_board:?}"
    );
    assert!(
        !crocutus_board.to_string_lossy().contains(".simard"),
        "Crocutus goal board must not touch the Simard home, got {crocutus_board:?}"
    );
}
