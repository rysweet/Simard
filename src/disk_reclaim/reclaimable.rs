//! Deterministic reclaimable-set enumeration (issue #4809).
//!
//! Routine disk reclaim previously sourced candidates **only** from the LLM
//! `disk-reclaim.yaml` recipe. When the agent failed to nominate the real
//! space hogs, the executor routed everything to "skip for review" and removed
//! nothing — logging `freed 0 bytes, 0 paths removed, N skipped for review` on
//! every cycle while `%-used` climbed monotonically toward 100%.
//!
//! This module adds a **deterministic Rust floor**: [`reclaimable_targets`]
//! always proposes the known-safe, regenerable artifacts routine reclaim never
//! touched — the idle `self-deploy-target` build tree (and the shared state-root
//! build caches) and stale engineer worktrees. The agentic proposal remains
//! **additive** on top, and — critically — every candidate this module proposes
//! is still re-vetted by [`super::guard::vet_candidate`] at the syscall boundary,
//! identically to an LLM candidate. There is no "trusted internal" shortcut.
//!
//! Snapshot / backup / corruption-quarantine directories are **out of scope**
//! here: they are owned solely by
//! [`crate::cognitive_threads::threads::maintenance`] (`MaintenanceThread`) with
//! its own keep-N floors, so the two subsystems never race or double-count.
//!
//! See `docs/reference/disk-reclaim-deterministic-enumeration.md` for the full
//! contract.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::self_deploy::source_prep::SELF_DEPLOY_TARGET_DIRNAME;
use crate::worktree_gc::LiveProcessProbe;

use super::candidate::{CandidateKind, ReclaimCandidate};

/// Env var: minimum idle age (days) before an idle build tree
/// (`self-deploy-target` / `cargo-target` / `shared-target`) is proposed.
pub const BUILD_IDLE_DAYS_ENV: &str = "SIMARD_DISK_RECLAIM_BUILD_IDLE_DAYS";

/// Env var: minimum idle age (days) before a stale engineer worktree is
/// proposed (still subject to the dirty/unpushed/PR-state vetoes at vet time).
pub const WORKTREE_IDLE_DAYS_ENV: &str = "SIMARD_DISK_RECLAIM_WORKTREE_IDLE_DAYS";

/// Default (and safe floor) for [`BUILD_IDLE_DAYS_ENV`]. Conservative: build
/// trees are fully regenerable by `cargo build`.
pub const DEFAULT_BUILD_IDLE_DAYS: u64 = 1;

/// Default (and safe floor) for [`WORKTREE_IDLE_DAYS_ENV`].
pub const DEFAULT_WORKTREE_IDLE_DAYS: u64 = 7;

/// Read [`BUILD_IDLE_DAYS_ENV`], defaulting to [`DEFAULT_BUILD_IDLE_DAYS`].
///
/// Defensive clamping is a **safety property**, not a convenience: a `0`,
/// empty, or unparseable value must **never** be interpreted as "purge now" —
/// it clamps back to the safe default floor.
pub fn build_idle_days_from_env() -> u64 {
    idle_days_from_env(BUILD_IDLE_DAYS_ENV, DEFAULT_BUILD_IDLE_DAYS)
}

/// Read [`WORKTREE_IDLE_DAYS_ENV`], defaulting to [`DEFAULT_WORKTREE_IDLE_DAYS`]
/// with the same defensive clamping as [`build_idle_days_from_env`].
pub fn worktree_idle_days_from_env() -> u64 {
    idle_days_from_env(WORKTREE_IDLE_DAYS_ENV, DEFAULT_WORKTREE_IDLE_DAYS)
}

/// Shared env parse + defensive clamp. A `0`, empty, non-numeric, or negative
/// value is a misconfiguration that must **never** collapse the idle window to
/// "purge now"; it clamps back to `default` (the safe floor).
fn idle_days_from_env(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&days| days > 0)
        .unwrap_or(default)
}

/// The containment roots the enumerator needs `allow_roots` to include so that
/// the *contents* of the build trees are reclaimable through the guard.
///
/// Returns the **specific** `<state_root>/self-deploy-target` directory (the
/// shared `cargo-target/`, `shared-target/`, and `engineer-worktrees/` roots are
/// already in [`super::allow_roots`]). This is an **explicit closed set of leaf
/// directories** — it must never resolve to bare `$HOME`, a bare `state_root`,
/// or any parent that would also contain the live `cognitive`/`.wal`/`.shadow`
/// store (snapshot backups live *directly* under `state_root`, so `state_root`
/// itself is never an allow-root).
pub fn build_tree_roots(state_root: &Path) -> Vec<PathBuf> {
    vec![state_root.join(SELF_DEPLOY_TARGET_DIRNAME)]
}

/// The full set of build-tree roots the enumerator scans for idle, regenerable
/// contents. This is the union of the containment-widening root
/// ([`build_tree_roots`], i.e. `self-deploy-target`) and the shared cargo caches
/// (`cargo-target`, `shared-target`) that already live in [`super::allow_roots`].
/// All three are fully regenerable by `cargo build`; none is ever the live
/// `cognitive` store or a snapshot/backup directory.
fn scannable_build_roots(state_root: &Path) -> Vec<PathBuf> {
    let mut roots = build_tree_roots(state_root);
    roots.push(state_root.join("cargo-target"));
    roots.push(state_root.join("shared-target"));
    roots
}

/// Immediate children of `dir`, sorted for deterministic output. Returns an
/// empty vec if `dir` is not a readable directory.
fn sorted_children(dir: &Path) -> Vec<PathBuf> {
    let mut children: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
        Err(_) => Vec::new(),
    };
    children.sort();
    children
}

/// Whether `path` has been idle for at least `idle_days` relative to `now`.
///
/// Fail-closed: any inability to read the mtime, or an mtime that is *newer*
/// than `now` (age would be negative), reports **not idle** so a candidate is
/// never proposed on an unverifiable age. This is the safe bias — an
/// unreclaimed idle tree costs one more cycle; a wrongly-reclaimed active tree
/// destroys work.
fn is_idle(path: &Path, now: SystemTime, idle_days: u64) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    match now.duration_since(mtime) {
        Ok(age) => age >= Duration::from_secs(idle_days.saturating_mul(86_400)),
        Err(_) => false,
    }
}

/// Testable core of the enumerator (dependency-injected for hermetic tests).
///
/// Proposes — never deletes — two categories of safe, regenerable artifacts:
///   1. **Idle build-tree contents:** for each build-tree root
///      (`self-deploy-target`, `cargo-target`, `shared-target`) that exists, is
///      not referenced by a live PID (per `live`), and is idle (root mtime older
///      than `build_idle_days` relative to `now`), each immediate child entry is
///      proposed as a `stale_build_cache` candidate. Children (not the roots
///      themselves) are proposed because the guard's `is_safe_to_delete` requires
///      a candidate to be **strictly inside** an allow-root.
///   2. **Stale engineer worktrees:** each idle child of
///      `<state_root>/engineer-worktrees` (idle beyond `worktree_idle_days`, not
///      live) is proposed as a `tracked_worktree` candidate; the dirty/unpushed/
///      unknown-PR vetoes still run at vet time.
///
/// Never proposes live cognitive state (`cognitive`, `cognitive.wal`,
/// `cognitive.shadow`) or any snapshot/backup/corruption-quarantine directory —
/// those are owned by `MaintenanceThread`.
pub fn enumerate_reclaimable(
    state_root: &Path,
    live: &dyn LiveProcessProbe,
    now: SystemTime,
    build_idle_days: u64,
    worktree_idle_days: u64,
) -> Vec<ReclaimCandidate> {
    let mut out = Vec::new();

    // 1. Idle build-tree contents. Propose each immediate child (never the root
    //    itself — the guard refuses a candidate that equals an allow-root). The
    //    root must be idle and free of any live PID before we touch its contents.
    for root in scannable_build_roots(state_root) {
        if !root.is_dir() {
            continue;
        }
        if live.worktree_has_live_process(&root) {
            continue;
        }
        if !is_idle(&root, now, build_idle_days) {
            continue;
        }
        for child in sorted_children(&root) {
            out.push(ReclaimCandidate {
                path: child,
                kind: CandidateKind::StaleBuildCache,
                parent_repo: None,
                reason: Some("deterministic floor: idle build-tree cache".to_string()),
                est_bytes: None,
            });
        }
    }

    // 2. Stale engineer worktrees. Each idle child of the engineer-worktrees
    //    root is proposed as a tracked_worktree so the guard re-runs the
    //    dirty/unpushed/unknown-PR vetoes at vet time — the enumerator only
    //    narrows on age + liveness, never on git state.
    let worktrees_root = state_root.join("engineer-worktrees");
    for child in sorted_children(&worktrees_root) {
        if !child.is_dir() {
            continue;
        }
        if live.worktree_has_live_process(&child) {
            continue;
        }
        if !is_idle(&child, now, worktree_idle_days) {
            continue;
        }
        out.push(ReclaimCandidate {
            path: child,
            kind: CandidateKind::TrackedWorktree,
            parent_repo: None,
            reason: Some("deterministic floor: stale engineer worktree".to_string()),
            est_bytes: None,
        });
    }

    out
}

/// Production entry point: the single shared deterministic enumerator consumed by
/// **both** routine reclaim (additively, next to LLM proposals) and
/// `emergency_cleanup`, so the two paths can never diverge. Wires production
/// defaults — real `/proc` liveness, real mtimes/now, and the env-configured idle
/// windows — into [`enumerate_reclaimable`].
pub fn reclaimable_targets(state_root: &Path) -> Vec<ReclaimCandidate> {
    let live = crate::worktree_gc::ProcfsLiveProcessProbe::new();
    enumerate_reclaimable(
        state_root,
        &live,
        SystemTime::now(),
        build_idle_days_from_env(),
        worktree_idle_days_from_env(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_reclaim::candidate::CandidateKind;
    use crate::self_deploy::source_prep::SELF_DEPLOY_TARGET_DIRNAME;
    use crate::worktree_gc::liveness::FakeLiveProcessProbe;
    use serial_test::serial;
    use std::time::Duration;
    use tempfile::TempDir;

    // ---- helpers ------------------------------------------------------

    fn mkdir(base: &Path, rel: &str) -> PathBuf {
        let p = base.join(rel);
        std::fs::create_dir_all(&p).expect("create dir");
        p
    }

    fn write_file(dir: &Path, name: &str, bytes: usize) {
        std::fs::write(dir.join(name), vec![0u8; bytes]).expect("write file");
    }

    /// A `now` far enough in the future that freshly-created dirs read as idle.
    fn future_now(days: u64) -> SystemTime {
        SystemTime::now() + Duration::from_secs(days * 86_400 + 3_600)
    }

    fn clear_env() {
        // SAFETY: guarded by #[serial(disk_reclaim_env)]; restored per test.
        unsafe {
            std::env::remove_var(BUILD_IDLE_DAYS_ENV);
            std::env::remove_var(WORKTREE_IDLE_DAYS_ENV);
        }
    }

    // ---- build_tree_roots (pure) --------------------------------------

    #[test]
    fn build_tree_roots_include_self_deploy_target() {
        let sr = Path::new("/home/azureuser/.simard");
        let roots = build_tree_roots(sr);
        assert!(
            roots.contains(&sr.join(SELF_DEPLOY_TARGET_DIRNAME)),
            "build_tree_roots must widen containment to the self-deploy-target \
             build tree so routine reclaim can free its contents; got {roots:?}",
        );
    }

    #[test]
    fn build_tree_roots_never_span_bare_state_root_or_home() {
        let sr = Path::new("/home/azureuser/.simard");
        let roots = build_tree_roots(sr);
        assert!(
            !roots.contains(&sr.to_path_buf()),
            "must never add bare state_root — snapshot backups live directly under it",
        );
        assert!(
            !roots.contains(&PathBuf::from("/home/azureuser")),
            "must never span $HOME",
        );
        assert!(
            !roots.iter().any(|r| r.as_os_str().is_empty()),
            "must never contain an empty path",
        );
    }

    // ---- env knob parsing + defensive clamping ------------------------

    #[test]
    #[serial(disk_reclaim_env)]
    fn idle_day_knobs_default_when_unset() {
        clear_env();
        assert_eq!(build_idle_days_from_env(), DEFAULT_BUILD_IDLE_DAYS);
        assert_eq!(worktree_idle_days_from_env(), DEFAULT_WORKTREE_IDLE_DAYS);
    }

    #[test]
    #[serial(disk_reclaim_env)]
    fn idle_day_knobs_read_valid_values() {
        clear_env();
        // SAFETY: serialized; cleared below.
        unsafe {
            std::env::set_var(BUILD_IDLE_DAYS_ENV, "3");
            std::env::set_var(WORKTREE_IDLE_DAYS_ENV, "14");
        }
        assert_eq!(build_idle_days_from_env(), 3);
        assert_eq!(worktree_idle_days_from_env(), 14);
        clear_env();
    }

    #[test]
    #[serial(disk_reclaim_env)]
    fn idle_day_knob_zero_empty_or_garbage_clamps_to_safe_floor() {
        // A misconfigured 0 / empty / non-numeric value must NEVER mean
        // "purge everything now" — it clamps to the safe default floor.
        for bad in ["0", "", "not-a-number", "-5"] {
            clear_env();
            // SAFETY: serialized; cleared each iteration.
            unsafe {
                std::env::set_var(BUILD_IDLE_DAYS_ENV, bad);
                std::env::set_var(WORKTREE_IDLE_DAYS_ENV, bad);
            }
            assert_eq!(
                build_idle_days_from_env(),
                DEFAULT_BUILD_IDLE_DAYS,
                "build idle knob={bad:?} must clamp to the safe floor, never purge",
            );
            assert_eq!(
                worktree_idle_days_from_env(),
                DEFAULT_WORKTREE_IDLE_DAYS,
                "worktree idle knob={bad:?} must clamp to the safe floor, never purge",
            );
        }
        clear_env();
    }

    // ---- build-tree enumeration ---------------------------------------

    #[test]
    fn idle_build_tree_contents_are_proposed_strictly_inside_the_root() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let sdt = mkdir(sr, SELF_DEPLOY_TARGET_DIRNAME);
        let child = mkdir(sr, &format!("{SELF_DEPLOY_TARGET_DIRNAME}/debug"));
        write_file(&child, "artifact.o", 4096);

        let live = FakeLiveProcessProbe::default();
        let cands = enumerate_reclaimable(sr, &live, future_now(3), 1, 7);

        // The candidate must be strictly INSIDE self-deploy-target (a child),
        // never the root itself, because the guard's is_safe_to_delete refuses a
        // candidate that equals an allow-root.
        assert!(
            cands
                .iter()
                .any(|c| c.path.starts_with(&sdt) && c.path != sdt),
            "an idle self-deploy-target must yield a reclaimable candidate \
             strictly inside it (its contents), not the root dir; got {cands:?}",
        );
    }

    #[test]
    fn live_build_tree_is_withheld() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let sdt = mkdir(sr, SELF_DEPLOY_TARGET_DIRNAME);
        let child = mkdir(sr, &format!("{SELF_DEPLOY_TARGET_DIRNAME}/debug"));
        write_file(&child, "artifact.o", 4096);

        let live = FakeLiveProcessProbe::default();
        live.mark_live(sdt.clone());

        let cands = enumerate_reclaimable(sr, &live, future_now(3), 1, 7);
        assert!(
            !cands.iter().any(|c| c.path.starts_with(&sdt)),
            "a self-deploy-target with a live PID must NOT be enumerated",
        );
    }

    #[test]
    fn sub_idle_window_build_tree_is_withheld() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let sdt = mkdir(sr, SELF_DEPLOY_TARGET_DIRNAME);
        let child = mkdir(sr, &format!("{SELF_DEPLOY_TARGET_DIRNAME}/debug"));
        write_file(&child, "artifact.o", 4096);

        let live = FakeLiveProcessProbe::default();
        // `now` == real now: the freshly-created tree is age ~0 < 1 idle day.
        let cands = enumerate_reclaimable(sr, &live, SystemTime::now(), 1, 7);
        assert!(
            !cands.iter().any(|c| c.path.starts_with(&sdt)),
            "a build tree younger than the idle window must NOT be enumerated",
        );
    }

    // ---- engineer-worktree enumeration --------------------------------

    #[test]
    fn idle_engineer_worktree_is_proposed_as_tracked_worktree() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let wt = mkdir(sr, "engineer-worktrees/wt-eng-1");
        write_file(&wt, "scratch", 2048);

        let live = FakeLiveProcessProbe::default();
        let cands = enumerate_reclaimable(sr, &live, future_now(8), 1, 7);

        let hit = cands.iter().find(|c| c.path == wt);
        assert!(
            hit.is_some(),
            "an idle engineer worktree (strictly inside the engineer-worktrees \
             allow-root) must be proposed; got {cands:?}",
        );
        assert_eq!(
            hit.unwrap().kind,
            CandidateKind::TrackedWorktree,
            "worktrees must be proposed with the tracked_worktree kind so the \
             guard re-runs the dirty/unpushed/PR-state vetoes",
        );
    }

    #[test]
    fn live_or_fresh_worktree_is_withheld() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let live_wt = mkdir(sr, "engineer-worktrees/wt-live");
        let fresh_wt = mkdir(sr, "engineer-worktrees/wt-fresh");
        write_file(&live_wt, "a", 16);
        write_file(&fresh_wt, "a", 16);

        let live = FakeLiveProcessProbe::default();
        live.mark_live(live_wt.clone());

        // future now makes both "old"; liveness must still withhold wt-live.
        let cands = enumerate_reclaimable(sr, &live, future_now(8), 1, 7);
        assert!(
            !cands.iter().any(|c| c.path == live_wt),
            "a worktree with a live PID must NOT be enumerated",
        );

        // real now: wt-fresh is younger than the 7-day worktree idle window.
        let cands_fresh = enumerate_reclaimable(sr, &live, SystemTime::now(), 1, 7);
        assert!(
            !cands_fresh.iter().any(|c| c.path == fresh_wt),
            "a worktree younger than the idle window must NOT be enumerated",
        );
    }

    // ---- live-state / maintenance-owned exclusion ---------------------

    #[test]
    fn live_cognitive_and_snapshot_state_is_never_enumerated() {
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        // Live store + maintenance-owned snapshot/backup/corrupt dirs, each with
        // content, placed directly under state_root.
        let protected = [
            mkdir(sr, "cognitive"),
            mkdir(sr, "cognitive.wal"),
            mkdir(sr, "cognitive.shadow"),
            mkdir(sr, "cognitive.snapshot-123"),
            mkdir(sr, "cognitive.corrupt-9"),
            mkdir(sr, "backup-1"),
            mkdir(sr, "verified-backup-2"),
        ];
        for p in &protected {
            write_file(p, "data", 4096);
        }

        let live = FakeLiveProcessProbe::default();
        let cands = enumerate_reclaimable(sr, &live, future_now(30), 1, 7);

        for c in &cands {
            for pd in &protected {
                assert!(
                    !c.path.starts_with(pd),
                    "the enumerator must NEVER propose live/snapshot/backup state \
                     ({pd:?}); those are owned by MaintenanceThread. Got {:?}",
                    c.path,
                );
            }
        }
    }

    // ---- deterministic floor ------------------------------------------

    #[test]
    fn deterministic_floor_is_non_empty_under_the_stale_scenario() {
        // The regression this whole change fixes: with idle regenerable
        // artifacts present, routine reclaim must have a non-empty deterministic
        // candidate set even with ZERO agent proposals — no more perpetual
        // "0 bytes, 0 paths removed".
        let tmp = TempDir::new().unwrap();
        let sr = tmp.path();
        let child = mkdir(sr, &format!("{SELF_DEPLOY_TARGET_DIRNAME}/debug"));
        write_file(&child, "artifact.o", 8192);
        let wt = mkdir(sr, "engineer-worktrees/wt-stale");
        write_file(&wt, "scratch", 8192);

        let live = FakeLiveProcessProbe::default();
        let cands = enumerate_reclaimable(sr, &live, future_now(30), 1, 7);

        assert!(
            !cands.is_empty(),
            "the deterministic floor must propose the idle self-deploy-target \
             contents and the stale worktree even with no agent input",
        );
    }
}
