//! Repo-invariant regression guard for the self-deploy drift root cause (#4914).
//!
//! The self-deploy checkout failed **every** Overseer cycle for hours because a
//! tracked file in the managed clone — canonically
//! `.github/hooks/amplihack-hooks.json` — was left locally modified, so every
//! `git checkout --detach <merged sha>` aborted with:
//!
//! ```text
//! error: Your local changes to the following files would be overwritten by
//!         checkout: .github/hooks/amplihack-hooks.json
//! ```
//!
//! The behavioural repair (a gated `reset_source_tree` scrub before checkout)
//! lives in `src/self_deploy/source_prep.rs` and is covered by
//! `src/self_deploy/tests_source_prep.rs`. This file locks the *other* half of
//! the fix as a durable CI invariant: the hooks manifest (and every script it
//! authorises) MUST stay **git-tracked** and MUST NOT drift in a clean checkout.
//!
//! Why an invariant test rather than another unit test:
//!
//!   * **Untracking / gitignoring the manifest is NOT the fix.** The manifest
//!     and hook scripts stay tracked for review and supply-chain integrity
//!     (SR-P1-2). If a future change moves `.github/hooks/` out of version
//!     control to "solve" the drift, this test goes red and names why.
//!   * **A clean checkout must be drift-free.** The manifest writer is
//!     write-if-changed: regenerating an identical manifest is a no-op, so a
//!     fresh `git checkout` of `main` leaves `.github/hooks/` pristine. If the
//!     manifest starts drifting again (an unconditional rewrite reappears), a
//!     fresh CI checkout will show the file modified and this test goes red —
//!     exactly the signal that was missing while #4914 burned Overseer cycles.
//!
//! See `docs/reference/self-deploy-drift-resilient-checkout.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Repo root — `CARGO_MANIFEST_DIR` is the crate root, which is the git work
/// tree top level for this repo.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const HOOKS_DIR: &str = ".github/hooks";
const MANIFEST_REL: &str = ".github/hooks/amplihack-hooks.json";

/// Run `git <args>` in the repo root, returning `(success, stdout)`. A spawn
/// failure (git absent) returns `None` so the caller can skip rather than fail
/// spuriously in a git-less packaging/vendored build.
fn git(root: &Path, args: &[&str]) -> Option<(bool, String)> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// Whether `root` is inside a real git work tree. Guards the whole suite so a
/// non-git build (packaged crate, offline vendored source) skips instead of
/// failing on the absence of `.git`.
fn is_git_work_tree(root: &Path) -> bool {
    matches!(
        git(root, &["rev-parse", "--is-inside-work-tree"]),
        Some((true, ref s)) if s.trim() == "true"
    )
}

#[test]
fn hooks_manifest_and_scripts_are_git_tracked() {
    let root = repo_root();
    if !is_git_work_tree(&root) {
        eprintln!(
            "skipping hooks_manifest_and_scripts_are_git_tracked: {} is not a git work tree",
            root.display()
        );
        return;
    }

    // The manifest must physically exist and be tracked. `git ls-files
    // --error-unmatch` exits non-zero for an untracked/ignored/missing path, so
    // this fails red the moment the manifest is untracked or gitignored — the
    // NOT-the-fix path (SR-P1-2).
    assert!(
        root.join(MANIFEST_REL).exists(),
        "{MANIFEST_REL} must exist on disk (the tracked self-deploy hooks manifest)"
    );
    let (tracked, _) = git(&root, &["ls-files", "--error-unmatch", "--", MANIFEST_REL])
        .expect("git must be runnable in a work tree");
    assert!(
        tracked,
        "{MANIFEST_REL} must be git-tracked — untracking/gitignoring it is NOT the #4914 fix; \
         the manifest stays tracked for review and supply-chain integrity (SR-P1-2)"
    );

    // Every hook script the manifest authorises must be tracked too: at least
    // the manifest plus one hook script under .github/hooks/ must be listed by
    // `git ls-files`, so the directory can never silently become untracked.
    let (ok, listed) =
        git(&root, &["ls-files", "--", HOOKS_DIR]).expect("git ls-files must run in a work tree");
    assert!(ok, "git ls-files {HOOKS_DIR} must succeed");
    let tracked_files: Vec<&str> = listed.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        tracked_files.iter().any(|f| *f == MANIFEST_REL),
        "the manifest must appear in `git ls-files {HOOKS_DIR}`"
    );
    assert!(
        tracked_files.len() >= 2,
        "{HOOKS_DIR} must track the manifest AND its hook scripts (found only {tracked_files:?})"
    );
}

#[test]
fn hooks_dir_has_no_untracked_drift_in_a_clean_checkout() {
    let root = repo_root();
    if !is_git_work_tree(&root) {
        eprintln!(
            "skipping hooks_dir_has_no_untracked_drift_in_a_clean_checkout: {} is not a git work tree",
            root.display()
        );
        return;
    }

    // A fresh checkout of `main` must leave .github/hooks/ pristine. Any
    // untracked (and not gitignored) file here is a drift source that can
    // re-wedge the self-deploy checkout, exactly the #4914 failure mode.
    let (ok, others) = git(
        &root,
        &[
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            HOOKS_DIR,
        ],
    )
    .expect("git ls-files --others must run in a work tree");
    assert!(ok, "git ls-files --others {HOOKS_DIR} must succeed");
    let untracked: Vec<&str> = others.lines().filter(|l| !l.is_empty()).collect();
    assert!(
        untracked.is_empty(),
        "no untracked files may live under {HOOKS_DIR} in a clean checkout (drift source for #4914); \
         found: {untracked:?}"
    );
}
