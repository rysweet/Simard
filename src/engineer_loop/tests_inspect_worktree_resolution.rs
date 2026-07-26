//! TDD (Step 7) — FAILING tests pinning the engineer-inspect worktree
//! resolution fix (issue #4744).
//!
//! Problem 1 (`process:engineer_inspect_false_reap`): the engineer-loop inspect
//! phase probed a synthetic, non-repository `/tmp` path, `git` returned exit 128
//! (`fatal: not a git repository`), the inspection surfaced
//! [`SimardError::NotARepo`], and a healthy-but-idle engineer was **false-stale
//! reaped** — discarding whole engineering loops (goal-board blocker `7f5afcca`).
//!
//! The fix (see `docs/reference/engineer-inspect-worktree-resolution.md`):
//!   1. resolve the engineer's REAL worktree at the probe seam via the new
//!      `engineer_worktree::claim::resolve_engineer_worktree(claim_key)`; and
//!   2. add an additive `SimardError::MissingWorktree { claim_key, expected_path }`
//!      so a genuinely-absent worktree is a DISTINCT, fail-closed signal — never
//!      conflated with `NotARepo` for a live-but-idle engineer.
//!
//! Invariant: **a valid engineer worktree never yields `NotARepo`.**
//!
//! These tests reference the TARGET API (`SimardError::MissingWorktree` and
//! `resolve_engineer_worktree`) and MUST fail to compile / fail against the
//! current tree. They go GREEN only once the #4744 fix lands.

use std::path::{Path, PathBuf};
use std::process::Command;

use serial_test::serial;
use tempfile::tempdir;

use super::inspect_workspace;
use super::{ENGINEER_CLAIM_KEY_ENV, effective_workspace_root};
use crate::engineer_worktree::claim::resolve_engineer_worktree;
use crate::error::SimardError;

/// RAII guard that pins `SIMARD_STATE_ROOT` for a test and restores the prior
/// value on drop. Env mutation is `unsafe` under edition 2024; every consumer
/// is `#[serial(cognitive_memory)]`, so no concurrent test races the process
/// env while the guard is live.
struct StateRootEnvGuard {
    prev: Option<String>,
}

impl StateRootEnvGuard {
    fn set(path: &Path) -> Self {
        let prev = std::env::var("SIMARD_STATE_ROOT").ok();
        // SAFETY: serialized via #[serial(cognitive_memory)]; single-threaded.
        unsafe { std::env::set_var("SIMARD_STATE_ROOT", path) };
        Self { prev }
    }
}

impl Drop for StateRootEnvGuard {
    fn drop(&mut self) {
        // SAFETY: see StateRootEnvGuard::set — serialized env restore.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
                None => std::env::remove_var("SIMARD_STATE_ROOT"),
            }
        }
    }
}

/// Guard that pins/clears the engineer-claim-key env for a test and restores it.
struct ClaimKeyEnvGuard {
    prev: Option<String>,
}

impl ClaimKeyEnvGuard {
    fn set(value: Option<&str>) -> Self {
        let prev = std::env::var(ENGINEER_CLAIM_KEY_ENV).ok();
        // SAFETY: serialized via #[serial(cognitive_memory)]; single-threaded.
        unsafe {
            match value {
                Some(v) => std::env::set_var(ENGINEER_CLAIM_KEY_ENV, v),
                None => std::env::remove_var(ENGINEER_CLAIM_KEY_ENV),
            }
        }
        Self { prev }
    }
}

impl Drop for ClaimKeyEnvGuard {
    fn drop(&mut self) {
        // SAFETY: see ClaimKeyEnvGuard::set — serialized env restore.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(ENGINEER_CLAIM_KEY_ENV, v),
                None => std::env::remove_var(ENGINEER_CLAIM_KEY_ENV),
            }
        }
    }
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A real engineer worktree: an initialised git repo with a seed commit, exactly
/// like an allocated `<state_root>/engineer-worktrees/<eng>` tree.
fn init_worktree(dir: &Path) {
    git(dir, &["init", "--initial-branch=main", "--quiet"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("README.md"), "seed\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-m", "seed", "--quiet"]);
}

/// A VALID engineer worktree must inspect to `Ok(..)` — never `NotARepo`. This
/// is the core #4744 invariant: an idle engineer with a real worktree is never
/// misreported as "not a repo" and therefore never false-stale reaped.
#[test]
#[serial(cognitive_memory)]
fn inspect_on_valid_worktree_is_not_not_a_repo() {
    let dir = tempdir().unwrap();
    init_worktree(dir.path());
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    let inspection = inspect_workspace(dir.path(), &state_root);

    assert!(
        !matches!(inspection, Err(SimardError::NotARepo { .. })),
        "a valid engineer worktree must NEVER yield NotARepo (issue #4744); got: {inspection:?}"
    );
    let inspection = inspection.expect("valid worktree must inspect Ok");
    assert!(
        inspection.repo_root.exists(),
        "resolved repo_root must be the real, existing worktree"
    );
}

/// `MissingWorktree` is a DISTINCT, additive variant — it must not be equal to,
/// nor match, `NotARepo`. This pins the "genuinely-absent worktree is not a
/// not-a-repo false positive" contract structurally.
#[test]
fn missing_worktree_is_distinct_from_not_a_repo() {
    let missing = SimardError::MissingWorktree {
        claim_key: "engineer:goal-7f5afcca".to_string(),
        expected_path: PathBuf::from("/state/engineer-worktrees/eng-7f5afcca"),
    };
    let not_a_repo = SimardError::NotARepo {
        path: PathBuf::from("/tmp/synthetic"),
        reason: "fatal: not a git repository".to_string(),
    };

    assert_ne!(
        missing, not_a_repo,
        "MissingWorktree and NotARepo must be distinct outcomes"
    );
    assert!(
        !matches!(missing, SimardError::NotARepo { .. }),
        "an absent worktree must not be classified as NotARepo (issue #4744)"
    );
    // Its Display must be log-safe: name the claim + expected path, no secrets,
    // no raw subprocess output.
    let rendered = missing.to_string();
    assert!(
        rendered.contains("engineer:goal-7f5afcca"),
        "MissingWorktree Display should name the claim key; got: {rendered}"
    );
}

/// Resolving a claim whose worktree directory does not exist on disk must return
/// the distinct `MissingWorktree` signal — never a bare `NotARepo` or a synthetic
/// `/tmp` default the inspect phase would then probe.
#[test]
#[serial(cognitive_memory)]
fn inspect_resolves_engineer_worktree_not_synthetic_tmp() {
    let state = tempdir().unwrap();
    // An empty state root: the managed engineer-worktrees dir holds no worktree
    // for this claim, so resolution must report it as genuinely missing.
    let prev = std::env::var("SIMARD_STATE_ROOT").ok();
    // SAFETY: this test is `#[serial(cognitive_memory)]`, so no other test
    // mutates or reads process env concurrently while it runs.
    unsafe { std::env::set_var("SIMARD_STATE_ROOT", state.path()) };

    let resolved = crate::engineer_worktree::claim::resolve_engineer_worktree("engineer:goal-abc");

    // SAFETY: see the set_var above — serialized, single-threaded env restore.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("SIMARD_STATE_ROOT", v),
            None => std::env::remove_var("SIMARD_STATE_ROOT"),
        }
    }

    assert!(
        matches!(resolved, Err(SimardError::MissingWorktree { .. })),
        "an absent engineer worktree must resolve to MissingWorktree, not a \
         synthetic /tmp path nor NotARepo (issue #4744); got: {resolved:?}"
    );
}

/// The full #4744 chain: a valid, idle engineer worktree (clean, no new files)
/// still inspects cleanly and is NOT reported as producing-nothing/NotARepo —
/// closing the false-stale-reap path. Idleness alone must never redden inspect.
#[test]
#[serial(cognitive_memory)]
fn valid_idle_engineer_is_not_false_stale_reaped() {
    let dir = tempdir().unwrap();
    init_worktree(dir.path());
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();

    // No new work since the seed commit — a genuinely IDLE (but live) engineer.
    let inspection = inspect_workspace(dir.path(), &state_root)
        .expect("an idle-but-valid worktree must inspect Ok, never NotARepo");

    assert!(
        !inspection.worktree_dirty,
        "an idle engineer's clean worktree must report clean (idle != dead)"
    );
    assert!(
        inspection.changed_files.is_empty(),
        "an idle engineer has no changed files; got {:?}",
        inspection.changed_files
    );
}

/// The happy path of the resolver: a real, on-disk engineer worktree under the
/// managed `<state_root>/engineer-worktrees/` root resolves to that exact,
/// canonicalized directory — the path the inspect phase then probes (never a
/// synthetic `/tmp` default).
#[test]
#[serial(cognitive_memory)]
fn resolve_engineer_worktree_returns_real_managed_worktree() {
    let state = tempdir().unwrap();
    // Allocator dir shape: `<goal-id>-<epoch_secs>-<hex6>`.
    let worktree = state
        .path()
        .join("engineer-worktrees")
        .join("advance-parity-f29bb15c-1783168109-a1b2c3");
    let inner = worktree.join("repo");
    std::fs::create_dir_all(&inner).unwrap();
    init_worktree(&inner);

    let _guard = StateRootEnvGuard::set(state.path());
    let resolved = resolve_engineer_worktree("engineer:advance-parity-f29bb15c")
        .expect("a real managed worktree must resolve to Ok");

    assert_eq!(
        resolved,
        std::fs::canonicalize(&worktree).unwrap(),
        "resolver must return the canonicalized managed worktree dir"
    );
}

/// With no claim-key env set, the loop's worktree selection preserves legacy
/// behaviour: the caller-supplied path is used verbatim (no resolution).
#[test]
#[serial(cognitive_memory)]
fn effective_workspace_root_uses_supplied_when_claim_env_unset() {
    let _claim = ClaimKeyEnvGuard::set(None);
    let supplied = PathBuf::from("/some/caller/supplied/worktree");

    let effective = effective_workspace_root(&supplied)
        .expect("with no claim env, the supplied path is used unchanged");

    assert_eq!(
        effective, supplied,
        "claim-env unset must not rewrite the supplied workspace root"
    );
}

/// When the harness names the claim but its worktree is genuinely absent, the
/// loop fails loudly with the distinct `MissingWorktree` signal instead of
/// probing the (possibly synthetic) supplied path into a `NotARepo`. This is the
/// core #4744 fail-closed guarantee at the loop seam.
#[test]
#[serial(cognitive_memory)]
fn effective_workspace_root_missing_claim_is_missing_worktree_not_synthetic() {
    let state = tempdir().unwrap();
    let _state_guard = StateRootEnvGuard::set(state.path());
    let _claim = ClaimKeyEnvGuard::set(Some("engineer:goal-does-not-exist"));

    // The supplied path is exactly the kind of synthetic non-repo path that
    // previously caused the false-stale reap — it must NOT be probed.
    let synthetic = PathBuf::from("/tmp/simard-engineer-loop-not-a-repo-123456");
    let effective = effective_workspace_root(&synthetic);

    assert!(
        matches!(effective, Err(SimardError::MissingWorktree { .. })),
        "a named-but-absent claim must fail with MissingWorktree, never probe a \
         synthetic /tmp path (issue #4744); got: {effective:?}"
    );
}
