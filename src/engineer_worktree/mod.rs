//! Per-engineer git worktree isolation (issue #1197).
//!
//! Allocates a dedicated git worktree under `<state_root>/engineer-worktrees/`
//! for each spawned engineer subprocess, so concurrent engineers never share
//! the same git working directory. This eliminates the
//! "worktree state changed during a non-mutating local engineer action"
//! verification race that was preventing the OODA daemon from shipping PRs.
//!
//! See `docs/reference/engineer-worktree-isolation.md` for the full contract.
//!
//! # Cargo target dir isolation (issue #1697)
//!
//! Each engineer subprocess inherits a per-worktree `CARGO_TARGET_DIR`
//! computed by [`crate::agent_supervisor::tmux::compute_tmux_env`]. The
//! default is `<HOME>/.cargo-targets/<worktree-basename>`, configurable
//! via the `SIMARD_CARGO_TARGETS_ROOT` env var. The basename is unique
//! per engineer (it embeds the goal id and a per-allocation suffix), so
//! two concurrent engineers never collide on cargo's build lock and never
//! corrupt each other's incremental output.
//!
//! Routing target dirs to a single shared root prevents the disk-fill
//! incident where each of N engineer worktrees grew its own ~7 GB `target/`
//! inside the worktree itself (8 worktrees ⇒ ~60 GB lost).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::OnceLock;

/// Process-wide lock serializing mutating `git worktree` commands against
/// the parent repository. Git's `.git/worktrees/` registry is not safe to
/// mutate concurrently from the same parent (observed: "failed to read
/// .git/worktrees/<other>/commondir: Success" under parallel `worktree add`).
fn worktree_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

use crate::error::SimardError;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;
#[cfg(test)]
mod tests_more;
#[cfg(test)]
mod tests_reaping_safety;

/// Subdirectory under the supervisor state root that holds all engineer worktrees.
pub const WORKTREES_SUBDIR: &str = "engineer-worktrees";

/// Filename of the per-worktree liveness sentinel (issue #1213). Contains the
/// PID of the process that allocated the worktree, plus its starttime read
/// from `/proc/<pid>/stat` field 22 (issue #1238). The starttime guards
/// against the daemon-restart-with-recycled-PID race: after a daemon restart,
/// the new daemon's PID is unrelated to the old one, but Linux can recycle
/// PIDs over time. Recording (PID, starttime) lets us distinguish "the
/// original claimant is still running" from "a different process happens
/// to occupy that PID slot now."
///
/// File format (line-separated, trailing newline tolerated):
///   line 1: `<pid>` (decimal i32, required)
///   line 2: `<starttime>` (u64 jiffies from /proc/<pid>/stat field 22,
///           optional — absent in pre-#1238 sentinels)
pub const ENGINEER_CLAIM_FILE: &str = ".simard-engineer-claim";

mod claim;
mod discovery;
mod precommit;
use claim::{claim_is_live, format_engineer_claim, read_engineer_claim_full};
pub use claim::{is_pid_alive_public, read_pid_starttime_public};
pub(crate) use discovery::goal_id_from_worktree_dir;
pub use discovery::{
    LiveEngineerWorktree, live_claimed_engineers, live_claimed_engineers_in_worktrees,
};

/// Maximum length of a `goal_id` accepted by [`EngineerWorktree::allocate`].
///
/// The cap is bounded by:
/// 1. ext4 NAME_MAX = 255 bytes per path segment (the worktree directory
///    is `engineer-worktrees/<goal-id>-<suffix>/`, where suffix is ~16 bytes).
/// 2. Git ref names accept long segments; the branch is
///    `engineer/<goal-id>-<suffix>`, leaving the same headroom.
///
/// 200 leaves comfortable headroom for both. The previous value of 64 was
/// unnecessarily conservative and caused legitimate dashboard-supplied
/// goals (whose IDs are slug-derived from the description) to be rejected
/// at engineer-dispatch time. See issue #1861 and the truncation logic in
/// [`crate::goals::goal_slug`], which now also caps slugs at
/// [`crate::goals::GOAL_SLUG_MAX_LEN`] (56 bytes) so well-formed slugs
/// always fit even after callers prepend a prefix.
pub const MAX_GOAL_ID_LEN: usize = 200;

/// A per-engineer git worktree.
///
/// Construct via [`EngineerWorktree::allocate`]. The worktree is registered
/// in the parent repository under a fresh `engineer/<goal-id>-<suffix>`
/// branch and lives at `<state_root>/engineer-worktrees/<goal-id>-<suffix>/`.
///
/// Cleanup is idempotent and runs either via [`EngineerWorktree::cleanup`]
/// (the explicit, observable path) or via [`Drop`] (the safety-net path).
#[derive(Debug)]

pub struct EngineerWorktree {
    path: PathBuf,
    branch: String,
    parent_repo: PathBuf,
    /// Canonicalized `<state_root>/engineer-worktrees/`. Used by cleanup
    /// paths to assert the target dir is contained inside the managed
    /// root before any `fs::remove_dir_all` (defense against bugs that
    /// could let `path` drift outside of the worktrees root).
    worktrees_root_canonical: PathBuf,
    cleaned: AtomicBool,
}

/// Result of a startup sweep over `<state_root>/engineer-worktrees/`.
#[derive(Debug, Default)]
pub struct SweepReport {
    /// Directories that were physically removed because they were not
    /// registered with the parent repository.
    pub removed_orphan_dirs: Vec<PathBuf>,
    /// Directories that were unregistered with the parent repo but skipped
    /// because their `.simard-engineer-claim` sentinel named a live PID
    /// (issue #1213). Useful for diagnostics and tests.
    pub skipped_live_dirs: Vec<PathBuf>,
    /// Directories skipped because a live process has its current working
    /// directory inside them (issue #2553, LIVE_CWD guard). Includes the
    /// fail-closed case where the liveness probe could not answer.
    pub skipped_live_cwd_dirs: Vec<PathBuf>,
    /// Directories skipped because they are a git worktree carrying
    /// uncommitted / unpushed / unverifiable work (issue #2553, WORK_STATE
    /// guard).
    pub skipped_dirty_dirs: Vec<PathBuf>,
    /// The reason each removed directory was reaped, paired 1:1 (and in the
    /// same order) with [`SweepReport::removed_orphan_dirs`]. Makes every
    /// deletion observable and attributable (issue #2553).
    pub removal_reasons: Vec<(PathBuf, RemovalReason)>,
}

impl SweepReport {
    /// True when the sweep did something worth logging: it removed an orphan,
    /// or a safety guard kept a directory that was otherwise an orphan
    /// candidate (LIVE_CWD or WORK_STATE). A pure LIVE_CLAIM skip is routine
    /// steady-state behaviour and does not, on its own, count as noteworthy.
    pub fn is_noteworthy(&self) -> bool {
        !self.removed_orphan_dirs.is_empty()
            || !self.skipped_live_cwd_dirs.is_empty()
            || !self.skipped_dirty_dirs.is_empty()
    }

    /// One-line `(kept N live-claim, N live-cwd, N with work)` summary of the
    /// directories the guards preserved. Shared by the daemon's boot-time and
    /// periodic sweep log lines so both stay in sync.
    pub fn kept_summary(&self) -> String {
        format!(
            "(kept {} live-claim, {} live-cwd, {} with work)",
            self.skipped_live_dirs.len(),
            self.skipped_live_cwd_dirs.len(),
            self.skipped_dirty_dirs.len(),
        )
    }
}

impl EngineerWorktree {
    /// Allocate a fresh git worktree for an engineer pursuing `goal_id`.
    ///
    /// Branches off the parent repository's current `main` HEAD. **Fails
    /// loud** if `main` cannot be resolved or if `goal_id` is not a safe
    /// identifier — there is no fallback to `HEAD`, per the repo's
    /// no-fallback convention.
    pub fn allocate(
        parent_repo: &Path,
        state_root: &Path,
        goal_id: &str,
    ) -> Result<Self, SimardError> {
        // 0. Validate goal_id at the boundary. Rejects path traversal,
        //    git ref-injection, and oversized inputs before they hit
        //    the filesystem or git ref namespace.
        validate_goal_id(goal_id).map_err(|reason| SimardError::ActionExecutionFailed {
            action: format!("engineer_worktree::allocate(goal={goal_id:?})"),
            reason,
        })?;

        // 0b. Disk-pressure precheck (issue #1697 follow-up). Refuses to
        //     allocate when the filesystem hosting `state_root` is below
        //     half the configured threshold (default 20 GiB). The OODA
        //     cycle treats `Refuse` as a transient failure and should
        //     run `simard worktree-gc --apply` before retrying. The
        //     check stat()s the closest existing ancestor of `state_root`
        //     so it works even if the worktrees subdir has not been
        //     created yet on a fresh install.
        //
        //     Disabled in three cases:
        //       (a) `cfg(test)` — the in-crate unit tests run against a
        //           tempdir on whatever filesystem the CI runner happens
        //           to have; we can't gate them on 20 GiB free, and the
        //           disk_pressure module has its own dedicated tests.
        //       (b) `SIMARD_DISK_PRESSURE_DISABLE=1` — escape hatch for
        //           constrained CI runners and one-off operator probes.
        //       (c) `SIMARD_DISK_PRESSURE_MIN_FREE_GB=0` is handled
        //           inside `configured_min_free_gb()` (it falls back to
        //           the default, not to disabled).
        if !cfg!(test) && std::env::var("SIMARD_DISK_PRESSURE_DISABLE").as_deref() != Ok("1") {
            let probe_target = first_existing_ancestor(state_root)
                .unwrap_or_else(|| std::path::PathBuf::from("/"));
            match crate::disk_pressure::check_with_default_threshold(&probe_target) {
                Ok(report) if report.should_refuse() => {
                    return Err(SimardError::ActionExecutionFailed {
                        action: format!("engineer_worktree::allocate(goal={goal_id})"),
                        reason: report.refuse_message(),
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    // statvfs failure is non-fatal: log loud and proceed.
                    // Better to over-allocate during a stat outage than
                    // to block the OODA cycle on a syscall hiccup.
                    tracing::warn!(
                        target: "simard::engineer_worktree",
                        error = %e,
                        path = %probe_target.display(),
                        "disk_pressure stat failed; proceeding without precheck",
                    );
                }
            }
        }

        // 1. Resolve the parent repo's `main` HEAD. No fallback.
        let main_sha = git_capture(parent_repo, &["rev-parse", "main"]).map_err(|reason| {
            SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!(
                    "cannot resolve `main` in {}: {reason}",
                    parent_repo.display()
                ),
            }
        })?;
        let main_sha = main_sha.trim();
        if !is_valid_sha40(main_sha) {
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!(
                    "`git rev-parse main` returned non-40-hex output {main_sha:?} in {}",
                    parent_repo.display()
                ),
            });
        }

        // 2. Build a unique suffix.
        let suffix = unique_suffix();
        let dir_name = format!("{goal_id}-{suffix}");
        let worktrees_root = state_root.join(WORKTREES_SUBDIR);
        let dir = worktrees_root.join(&dir_name);
        let branch = format!("engineer/{dir_name}");

        // 3. Ensure the worktrees root exists with mode 0700 on Unix.
        //    Worktrees may transiently hold credentials or .env files;
        //    do not expose them to other local users.
        create_worktrees_root(&worktrees_root).map_err(|e| SimardError::ActionExecutionFailed {
            action: format!("engineer_worktree::allocate(goal={goal_id})"),
            reason: format!(
                "cannot create worktrees root {}: {e}",
                worktrees_root.display()
            ),
        })?;

        // Canonicalize the worktrees root once now that it exists. Used by
        // cleanup_inner / the failure-recovery path below to refuse any
        // `remove_dir_all` whose canonical path is not contained here.
        let worktrees_root_canonical =
            worktrees_root
                .canonicalize()
                .map_err(|e| SimardError::ActionExecutionFailed {
                    action: format!("engineer_worktree::allocate(goal={goal_id})"),
                    reason: format!(
                        "cannot canonicalize worktrees root {}: {e}",
                        worktrees_root.display()
                    ),
                })?;

        // 4. `git worktree add -b <branch> <dir> <main_sha>` — serialized
        //    against the parent repo because git's worktree registry races.
        let dir_str = dir.to_string_lossy();
        let result = {
            let _guard = worktree_mutation_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            git_capture(
                parent_repo,
                &["worktree", "add", "-b", &branch, &dir_str, main_sha],
            )
        };
        if let Err(reason) = result {
            // Best-effort cleanup of any partial state before failing loud.
            // Each failure is logged at WARN — never silently swallowed.
            // The dir-removal is gated on the canonical-prefix check so a
            // future bug that lets `dir` drift outside the worktrees root
            // cannot escalate to out-of-root deletion.
            if dir.exists() {
                match assert_under_root(&dir, &worktrees_root_canonical) {
                    Ok(safe_dir) => {
                        if let Err(e) = fs::remove_dir_all(&safe_dir) {
                            tracing::warn!(
                                target: "simard::engineer_worktree",
                                error = %e,
                                worktree = %safe_dir.display(),
                                "failed to clean up partial worktree dir after `git worktree add` failure",
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "simard::engineer_worktree",
                            error = %e,
                            worktree = %dir.display(),
                            "refusing to remove partial worktree dir: not contained in canonical worktrees root",
                        );
                    }
                }
            }
            if let Err(e) = git_capture(parent_repo, &["worktree", "prune"]) {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    "git worktree prune failed during allocate-failure recovery",
                );
            }
            if let Err(e) = git_capture(parent_repo, &["branch", "-D", &branch]) {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    branch = %branch,
                    "best-effort branch delete failed during allocate-failure recovery",
                );
            }
            return Err(SimardError::ActionExecutionFailed {
                action: format!("engineer_worktree::allocate(goal={goal_id})"),
                reason: format!("`git worktree add` failed: {reason}"),
            });
        }

        // 4b. Exclude the Simard-managed claim sentinel from `git status` in
        //     the target repo via the worktree's git exclude file (issue
        //     #2621). Belt-and-suspenders with the `inspect_workspace`
        //     sentinel filter: Simard drops `.simard-engineer-claim` into
        //     every engineer worktree but must NOT depend on each *target*
        //     repo gitignoring Simard's private infra file. Without this, an
        //     external governed repo that doesn't gitignore the sentinel makes
        //     `inspect_workspace().worktree_dirty == true`, so the engineer-
        //     loop pre-mutation guard aborts every mutating engineer before
        //     the coding agent is ever spawned — an infinite dispatch loop.
        //     Fail-loud-but-non-fatal: log at WARN and continue; the
        //     `inspect_workspace` filter still hides the sentinel if this
        //     fails.
        //
        //     Serialized under `worktree_mutation_lock` (the same lock guarding
        //     `git worktree add`): `git rev-parse --git-path info/exclude`
        //     resolves to the *shared common* git dir for linked worktrees, so
        //     concurrent allocations against the same parent repo would
        //     otherwise race on a non-atomic read-modify-write of that shared
        //     file and could clobber the repo's pre-existing exclude entries.
        let exclude_result = {
            let _guard = worktree_mutation_lock()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            exclude_engineer_claim(&dir)
        };
        if let Err(e) = exclude_result {
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                worktree = %dir.display(),
                "failed to add engineer-claim sentinel to git exclude; inspect_workspace filter still hides it",
            );
        }

        // 5. Write the per-worktree liveness sentinel (issue #1213, refined
        //    in #1238). If the sweep ever runs against this worktree while
        //    git's registration is transiently missing, the live PID +
        //    starttime guard prevents the cwd-deletion-under-the-engineer's-
        //    feet bug. Recording starttime alongside the PID closes the
        //    daemon-restart-with-recycled-PID race.
        let claim_path = dir.join(ENGINEER_CLAIM_FILE);
        let claim_pid = std::process::id();
        if let Err(e) = fs::write(&claim_path, format_engineer_claim(claim_pid)) {
            // Sentinel write failure is non-fatal: the AtomicBool guard plus
            // the existing canonical-prefix safety still protect us. Log loud
            // so the regression is visible.
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                claim = %claim_path.display(),
                "failed to write engineer-claim sentinel; sweep falls back to git-registration check only",
            );
        }

        // 6. Best-effort native git-hook enrollment so engineer commits run
        //    the same fmt/clippy/test fences locally that CI runs. Several
        //    merged and pending PRs (#1641, #1581, #1607, #1608, #1629, #1558,
        //    #1499) failed CI on the `pre-commit` job because the engineer never
        //    ran the hooks locally before pushing. Wires `core.hooksPath` to the
        //    committed Python-free `hooks/` directory (#3181). Fail-loud-but-
        //    non-fatal: log at WARN and continue; CI is still the source of truth.
        match precommit::install_hooks(&dir) {
            Ok(true) => {
                tracing::info!(
                    target: "simard::engineer_worktree",
                    worktree = %dir.display(),
                    "native git hooks enrolled in engineer worktree (core.hooksPath -> hooks)",
                );
            }
            Ok(false) => {
                tracing::debug!(
                    target: "simard::engineer_worktree",
                    worktree = %dir.display(),
                    "native git-hook enrollment skipped (committed hooks/ directory absent)",
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "simard::engineer_worktree",
                    error = %e,
                    worktree = %dir.display(),
                    "native git-hook enrollment failed; engineer commits will not be locally gated (CI still gates the merge)",
                );
            }
        }

        Ok(Self {
            path: dir,
            branch,
            parent_repo: parent_repo.to_path_buf(),
            worktrees_root_canonical,
            cleaned: AtomicBool::new(false),
        })
    }

    /// Path to the worktree on disk.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Name of the branch checked out in this worktree.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Remove the worktree, prune its registration, delete its branch.
    ///
    /// Idempotent — second and subsequent calls are `Ok(())` no-ops.
    /// Returns the first hard error encountered (canonical-prefix guard
    /// rejection or filesystem failure on the worktree dir). Best-effort
    /// git registry/branch failures are logged but do not propagate, so
    /// a partially-cleaned worktree still drives the call to a result.
    pub fn cleanup(&self) -> Result<(), SimardError> {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        cleanup_inner(
            &self.parent_repo,
            &self.path,
            &self.branch,
            &self.worktrees_root_canonical,
        )
    }
}

impl Drop for EngineerWorktree {
    fn drop(&mut self) {
        if self.cleaned.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Err(e) = cleanup_inner(
            &self.parent_repo,
            &self.path,
            &self.branch,
            &self.worktrees_root_canonical,
        ) {
            tracing::warn!(
                target: "simard::engineer_worktree",
                error = %e,
                worktree = %self.path.display(),
                "Drop-path cleanup of engineer worktree returned a hard error",
            );
        }
    }
}
mod cleanup;
pub(crate) use cleanup::assert_under_root;
use cleanup::{cleanup_inner, create_worktrees_root, unique_suffix};
mod sweep;
pub use sweep::{
    RemovalReason, sweep_orphaned_worktrees, sweep_orphaned_worktrees_inner, validate_goal_id,
};
use sweep::{git_capture, is_valid_sha40};

/// Walk up from `path`, returning the first ancestor that exists on
/// disk. Used by the disk-pressure precheck to find a path that
/// `statvfs` can stat — on a fresh install, `state_root` (and the
/// `engineer-worktrees` subdir under it) does not yet exist.
fn first_existing_ancestor(path: &Path) -> Option<std::path::PathBuf> {
    let mut cur: Option<&Path> = Some(path);
    while let Some(p) = cur {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

/// Append an anchored exclude entry for [`ENGINEER_CLAIM_FILE`] to
/// `worktree_dir`'s git exclude file so the Simard-managed sentinel is never
/// reported as an untracked change by `git status` in the target repo (issue
/// #2621).
///
/// The real exclude path is resolved via `git rev-parse --git-path
/// info/exclude`, run *inside the worktree* so git returns the correct path
/// into the linked worktree's git dir (git keeps `info/exclude` in the shared
/// common dir, which is exactly what we want: excluding the sentinel filename
/// is harmless and repo-local — `.git/info/exclude` is never committed).
///
/// The written pattern is **root-anchored** (`/.simard-engineer-claim`): the
/// sentinel is only ever placed at the worktree root, and an unanchored bare
/// filename would (per gitignore semantics) also hide any `subdir/…` file that
/// happens to share the basename — silently dropping a real agent-created file
/// from both `inspect_workspace` and `verify_agent_spawn_artifacts`. Anchoring
/// keeps the exclude semantics identical to the exact-root `strip_claim_sentinel`
/// filter (verified: an anchored entry in the shared exclude, evaluated from a
/// linked worktree, hides only the root sentinel).
///
/// The parent directory and file are created if absent, and the append is
/// idempotent (an exact-line match short-circuits) so repeated allocations
/// against the same parent repo never duplicate the entry.
fn exclude_engineer_claim(worktree_dir: &Path) -> Result<(), String> {
    let raw = git_capture(worktree_dir, &["rev-parse", "--git-path", "info/exclude"])?;
    let rel = raw.trim();
    if rel.is_empty() {
        return Err("`git rev-parse --git-path info/exclude` returned empty output".to_string());
    }
    let candidate = Path::new(rel);
    let exclude_path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        // git resolves the path relative to the worktree cwd it was invoked in.
        worktree_dir.join(candidate)
    };

    if let Some(parent) = exclude_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create exclude parent {}: {e}", parent.display()))?;
    }

    let existing = match fs::read_to_string(&exclude_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", exclude_path.display())),
    };

    // Root-anchored pattern (leading `/`) so only `<worktree>/.simard-engineer-claim`
    // is excluded, never a same-basename file nested in a subdirectory.
    let anchored = format!("/{ENGINEER_CLAIM_FILE}");

    // Idempotent: skip if the sentinel is already excluded (exact-line match,
    // ignoring surrounding whitespace so a hand-edited exclude still matches).
    // A legacy bare `.simard-engineer-claim` line also counts as present so we
    // never stack a duplicate on a worktree written by an earlier build.
    if existing
        .lines()
        .any(|line| line.trim() == anchored || line.trim() == ENGINEER_CLAIM_FILE)
    {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&anchored);
    updated.push('\n');

    fs::write(&exclude_path, updated).map_err(|e| format!("write {}: {e}", exclude_path.display()))
}
