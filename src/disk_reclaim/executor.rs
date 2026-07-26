//! The disposer: `exec_reclaim`.
//!
//! Sorts candidates largest-first, re-vets **every** one through
//! [`vet_candidate`] at the syscall boundary, and performs the reclamation
//! primitive — stopping once the partition is back under `target_pct` so it
//! removes the minimum necessary. The delete primitive lives behind the
//! [`PathRemover`] seam so tests never touch real system paths.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::disk_pressure::check::{DiskStatProvider, used_pct};
use crate::worktree_gc::under_any_root;

use super::ReclaimMode;
use super::candidate::{CandidateKind, ReclaimCandidate};
use super::guard::{GuardContext, ReclaimPrimitive, RejectReason, Verdict, vet_candidate};

/// A path the executor removed (or would remove, in dry-run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovedPath {
    pub path: PathBuf,
    pub kind: CandidateKind,
    pub bytes: u64,
    pub primitive: ReclaimPrimitive,
}

/// A candidate a rail refused — the human-review list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedPath {
    pub path: PathBuf,
    pub kind: CandidateKind,
    pub reject_reason: RejectReason,
}

/// An individual removal that failed (does not abort the run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReclaimFailure {
    pub path: PathBuf,
    pub error: String,
}

/// The structured outcome of one reclamation run. Telemetry and the
/// `--report-json` output derive from this same report, so they never disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReclaimReport {
    pub mode: ReclaimMode,
    pub used_pct_before: u8,
    pub used_pct_after: u8,
    pub target_pct: u8,
    pub bytes_freed: u64,
    pub removed: Vec<RemovedPath>,
    pub would_remove: Vec<RemovedPath>,
    pub skipped: Vec<SkippedPath>,
    pub failures: Vec<ReclaimFailure>,
}

impl ReclaimReport {
    /// Whether any reclamation actually happened.
    pub fn reclaim_performed(&self) -> bool {
        self.bytes_freed > 0 || !self.removed.is_empty()
    }

    /// Daemon one-liner summary.
    pub fn summary(&self) -> String {
        format!(
            "disk reclaim: {}% -> {}% used, freed {} bytes, {} paths removed, {} skipped for review",
            self.used_pct_before,
            self.used_pct_after,
            self.bytes_freed,
            self.removed.len(),
            self.skipped.len(),
        )
    }
}

/// Seam for the actual destructive primitive. Production shells `git` / `rm`;
/// tests record calls and remove only tempdirs.
pub trait PathRemover {
    fn remove(&self, primitive: ReclaimPrimitive, path: &Path) -> Result<(), String>;
}

/// Production remover. Re-asserts allow-root containment immediately before the
/// unlink (TOCTOU defense), invokes `git` with `env_clear` + argument vectors +
/// `--` separators, and rejects leading-dash paths. No `--admin`, no
/// `--no-verify`.
pub struct RealPathRemover {
    pub parent_repo: PathBuf,
    pub allow_roots: Vec<PathBuf>,
}

impl PathRemover for RealPathRemover {
    fn remove(&self, primitive: ReclaimPrimitive, path: &Path) -> Result<(), String> {
        let path_str = path.to_str().ok_or_else(|| "non-utf8 path".to_string())?;
        if path_str.starts_with('-') {
            return Err(format!("refusing leading-dash path: {path_str}"));
        }
        // TOCTOU re-assert: containment must still hold at the syscall boundary.
        if !under_any_root(path, &self.allow_roots) {
            return Err(format!(
                "refusing removal — {path_str} is not under any allow-root",
            ));
        }
        match primitive {
            ReclaimPrimitive::GitWorktreeRemoveForce => {
                let _ = git_hardened(&self.parent_repo, &["worktree", "prune"]);
                git_hardened(
                    &self.parent_repo,
                    &["worktree", "remove", "--force", "--", path_str],
                )?;
                let _ = git_hardened(&self.parent_repo, &["worktree", "prune"]);
                Ok(())
            }
            ReclaimPrimitive::RemoveDir => {
                let canon = path
                    .canonicalize()
                    .map_err(|e| format!("cannot canonicalize {path_str}: {e}"))?;
                if !under_any_root(&canon, &self.allow_roots) {
                    return Err(format!(
                        "refusing rm -rf — canonical {} is not under any allow-root",
                        canon.display()
                    ));
                }
                std::fs::remove_dir_all(&canon)
                    .map_err(|e| format!("rm -rf {} failed: {e}", canon.display()))
            }
        }
    }
}

/// Env-cleared git invocation (only `PATH`/`HOME` survive). Blocks `GIT_*` /
/// `LD_PRELOAD` hijacking; argument vectors only, no shell.
fn git_hardened(repo: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("git {args:?} spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Read the home-partition `%-used` via the injectable provider. On any error,
/// returns `100` (fail-closed: never falsely report "under threshold").
fn read_used_pct(disk: &dyn DiskStatProvider, path: &Path) -> u8 {
    disk.stat(path)
        .ok()
        .and_then(|s| used_pct(&s))
        .map(|p| p.round().clamp(0.0, 100.0) as u8)
        .unwrap_or(100)
}

/// The disposer. Sorts largest-first, re-vets each candidate, and performs the
/// reclamation up to `target_pct`.
///
/// - **dry-run**: full vetting, zero destructive ops; allowed candidates go to
///   `would_remove`, the whole list is processed so the report is complete.
/// - **apply**: performs the primitive via `remover`, stops once under
///   `target_pct` (minimum-necessary reclamation).
#[allow(clippy::too_many_arguments)]
pub fn exec_reclaim(
    mut candidates: Vec<ReclaimCandidate>,
    ctx: &GuardContext<'_>,
    mode: ReclaimMode,
    target_pct: u8,
    disk: &dyn DiskStatProvider,
    disk_path: &Path,
    remover: &dyn PathRemover,
) -> ReclaimReport {
    let used_before = read_used_pct(disk, disk_path);

    // Largest-first by fresh measurement — reclaim the biggest wins first.
    // `sort_by_cached_key` evaluates the (expensive `du`) key exactly once per
    // candidate — O(n) subprocess spawns — instead of `sort_by_key`'s
    // O(n·log n) re-evaluations. The ordering is identical.
    candidates.sort_by_cached_key(|c| std::cmp::Reverse(ctx.measurer.measure(&c.path)));

    let mut report = ReclaimReport {
        mode,
        used_pct_before: used_before,
        used_pct_after: used_before,
        target_pct,
        bytes_freed: 0,
        removed: Vec::new(),
        would_remove: Vec::new(),
        skipped: Vec::new(),
        failures: Vec::new(),
    };

    for candidate in candidates {
        // In apply mode, stop as soon as we are under the target — removes the
        // minimum necessary. Dry-run always processes the full list.
        if mode == ReclaimMode::Apply {
            let current = read_used_pct(disk, disk_path);
            report.used_pct_after = current;
            if current <= target_pct {
                break;
            }
        }

        match vet_candidate(&candidate, ctx) {
            Verdict::Reject { reason } => report.skipped.push(SkippedPath {
                path: candidate.path.clone(),
                kind: candidate.kind,
                reject_reason: reason,
            }),
            Verdict::Allow { primitive, bytes } => {
                let entry = RemovedPath {
                    path: candidate.path.clone(),
                    kind: candidate.kind,
                    bytes,
                    primitive,
                };
                match mode {
                    ReclaimMode::DryRun => report.would_remove.push(entry),
                    ReclaimMode::Apply => match remover.remove(primitive, &candidate.path) {
                        Ok(()) => {
                            report.bytes_freed = report.bytes_freed.saturating_add(bytes);
                            report.removed.push(entry);
                        }
                        Err(error) => report.failures.push(ReclaimFailure {
                            path: candidate.path.clone(),
                            error,
                        }),
                    },
                }
            }
        }
    }

    report.used_pct_after = read_used_pct(disk, disk_path);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk_pressure::check::DiskStat;
    use crate::worktree_gc::liveness::FakeLiveProcessProbe;
    use std::cell::RefCell;
    use std::path::Path;
    use tempfile::TempDir;

    use super::super::guard::{
        ProtectedDenySet, SizeMeasurer, TrackedWorktreeProbe, WorktreeVerdict,
    };

    // ---- test doubles -------------------------------------------------

    /// A disk provider that returns a scripted sequence of `%-used` values.
    /// Each `stat` call consumes the next value; the last repeats.
    struct ScriptedDisk {
        pcts: RefCell<Vec<u8>>,
    }
    impl ScriptedDisk {
        fn new(pcts: Vec<u8>) -> Self {
            Self {
                pcts: RefCell::new(pcts),
            }
        }
    }
    impl DiskStatProvider for ScriptedDisk {
        fn stat(&self, _path: &Path) -> Result<DiskStat, std::io::Error> {
            let mut v = self.pcts.borrow_mut();
            let pct = if v.len() > 1 { v.remove(0) } else { v[0] };
            // total=100 units, free chosen so used_pct == pct.
            Ok(DiskStat {
                free_bytes: (100 - pct as u64),
                total_bytes: 100,
            })
        }
    }

    /// Records every remove call; actually removes the tempdir so free space
    /// bookkeeping in real runs would be exercised (here only recorded).
    #[derive(Default)]
    struct RecordingRemover {
        calls: RefCell<Vec<(ReclaimPrimitive, PathBuf)>>,
    }
    impl PathRemover for RecordingRemover {
        fn remove(&self, primitive: ReclaimPrimitive, path: &Path) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push((primitive, path.to_path_buf()));
            Ok(())
        }
    }

    struct AllowAllWtProbe;
    impl TrackedWorktreeProbe for AllowAllWtProbe {
        fn assess(&self, _worktree: &Path) -> WorktreeVerdict {
            WorktreeVerdict::Reclaimable
        }
    }

    /// Measures by a fixed map keyed on path.
    #[derive(Default)]
    struct MapMeasurer(RefCell<std::collections::HashMap<PathBuf, u64>>);
    impl MapMeasurer {
        fn set(&self, p: &Path, b: u64) {
            self.0.borrow_mut().insert(p.to_path_buf(), b);
        }
    }
    impl SizeMeasurer for MapMeasurer {
        fn measure(&self, p: &Path) -> u64 {
            *self.0.borrow().get(p).unwrap_or(&0)
        }
    }

    /// An allow-root tempdir plus N child dirs inside it.
    struct Env {
        _root: TempDir,
        allow_roots: Vec<PathBuf>,
        children: Vec<PathBuf>,
    }
    impl Env {
        fn new(names: &[&str]) -> Self {
            let root = TempDir::new().unwrap();
            let mut children = Vec::new();
            for n in names {
                let c = root.path().join(n);
                std::fs::create_dir_all(&c).unwrap();
                children.push(c);
            }
            Self {
                allow_roots: vec![root.path().to_path_buf()],
                _root: root,
                children,
            }
        }
    }

    fn orphan(path: &Path) -> ReclaimCandidate {
        ReclaimCandidate {
            path: path.to_path_buf(),
            kind: CandidateKind::OrphanDir,
            parent_repo: None,
            reason: None,
            est_bytes: None,
        }
    }

    fn ctx<'a>(
        env: &'a Env,
        protected: &'a ProtectedDenySet,
        live: &'a FakeLiveProcessProbe,
        wt: &'a AllowAllWtProbe,
        measurer: &'a MapMeasurer,
    ) -> GuardContext<'a> {
        GuardContext {
            allow_roots: &env.allow_roots,
            build_cache_leaves: &[],
            protected,
            live_probe: live,
            wt_probe: wt,
            measurer,
        }
    }

    // ---- behavior -----------------------------------------------------

    #[test]
    fn dry_run_performs_zero_destructive_ops() {
        let env = Env::new(&["a", "b"]);
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = AllowAllWtProbe;
        let measurer = MapMeasurer::default();
        measurer.set(&env.children[0], 10);
        measurer.set(&env.children[1], 20);
        let disk = ScriptedDisk::new(vec![90]);
        let remover = RecordingRemover::default();
        let guard = ctx(&env, &protected, &live, &wt, &measurer);

        let cands = vec![orphan(&env.children[0]), orphan(&env.children[1])];
        let report = exec_reclaim(
            cands,
            &guard,
            ReclaimMode::DryRun,
            85,
            &disk,
            Path::new("/home"),
            &remover,
        );

        assert!(remover.calls.borrow().is_empty(), "dry-run must not delete");
        assert_eq!(report.would_remove.len(), 2);
        assert_eq!(report.bytes_freed, 0);
        assert_eq!(report.used_pct_before, 90);
        assert_eq!(report.used_pct_after, 90, "dry-run leaves usage unchanged");
        assert!(!report.reclaim_performed());
    }

    #[test]
    fn candidates_are_processed_largest_first() {
        let env = Env::new(&["small", "big", "medium"]);
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = AllowAllWtProbe;
        let measurer = MapMeasurer::default();
        measurer.set(&env.children[0], 10);
        measurer.set(&env.children[1], 9000);
        measurer.set(&env.children[2], 300);
        let disk = ScriptedDisk::new(vec![90]); // never drops below target → no stop
        let remover = RecordingRemover::default();
        let guard = ctx(&env, &protected, &live, &wt, &measurer);

        let cands = vec![
            orphan(&env.children[0]),
            orphan(&env.children[1]),
            orphan(&env.children[2]),
        ];
        let report = exec_reclaim(
            cands,
            &guard,
            ReclaimMode::DryRun,
            85,
            &disk,
            Path::new("/home"),
            &remover,
        );

        let order: Vec<&PathBuf> = report.would_remove.iter().map(|r| &r.path).collect();
        assert_eq!(
            order,
            vec![&env.children[1], &env.children[2], &env.children[0]],
            "largest-first ordering (big, medium, small)",
        );
    }

    #[test]
    fn apply_stops_once_under_target() {
        let env = Env::new(&["big", "medium", "small"]);
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = AllowAllWtProbe;
        let measurer = MapMeasurer::default();
        measurer.set(&env.children[0], 9000);
        measurer.set(&env.children[1], 300);
        measurer.set(&env.children[2], 10);
        // stat calls, in order: used_before=90; loop-top before big=90
        // (>85 → process big); loop-top before medium=80 (<=85 → stop); the
        // final post-loop read repeats the last value (80).
        let disk = ScriptedDisk::new(vec![90, 90, 80]);
        let remover = RecordingRemover::default();
        let guard = ctx(&env, &protected, &live, &wt, &measurer);

        let cands = vec![
            orphan(&env.children[0]),
            orphan(&env.children[1]),
            orphan(&env.children[2]),
        ];
        let report = exec_reclaim(
            cands,
            &guard,
            ReclaimMode::Apply,
            85,
            &disk,
            Path::new("/home"),
            &remover,
        );

        assert_eq!(remover.calls.borrow().len(), 1, "only the largest removed");
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].path, env.children[0]);
        assert_eq!(report.bytes_freed, 9000);
        assert!(report.reclaim_performed());
        assert_eq!(report.used_pct_before, 90);
        assert_eq!(report.used_pct_after, 80);
    }

    #[test]
    fn apply_removes_via_remover_and_accounts_bytes() {
        let env = Env::new(&["a"]);
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = AllowAllWtProbe;
        let measurer = MapMeasurer::default();
        measurer.set(&env.children[0], 4096);
        // Stays above target so it does not early-stop before processing.
        let disk = ScriptedDisk::new(vec![99]);
        let remover = RecordingRemover::default();
        let guard = ctx(&env, &protected, &live, &wt, &measurer);

        let report = exec_reclaim(
            vec![orphan(&env.children[0])],
            &guard,
            ReclaimMode::Apply,
            85,
            &disk,
            Path::new("/home"),
            &remover,
        );

        assert_eq!(remover.calls.borrow().len(), 1);
        assert_eq!(remover.calls.borrow()[0].0, ReclaimPrimitive::RemoveDir);
        assert_eq!(report.bytes_freed, 4096);
    }

    #[test]
    fn rejected_candidates_go_to_skipped_for_human_review() {
        // A protected candidate is vetted-and-skipped even in apply mode.
        let env = Env::new(&["protected-child", "ok-child"]);
        let protected = ProtectedDenySet::from_paths(vec![env.children[0].clone()]);
        let live = FakeLiveProcessProbe::default();
        let wt = AllowAllWtProbe;
        let measurer = MapMeasurer::default();
        measurer.set(&env.children[0], 500);
        measurer.set(&env.children[1], 100);
        let disk = ScriptedDisk::new(vec![99]);
        let remover = RecordingRemover::default();
        let guard = ctx(&env, &protected, &live, &wt, &measurer);

        let report = exec_reclaim(
            vec![orphan(&env.children[0]), orphan(&env.children[1])],
            &guard,
            ReclaimMode::Apply,
            85,
            &disk,
            Path::new("/home"),
            &remover,
        );

        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].path, env.children[0]);
        assert_eq!(report.skipped[0].reject_reason, RejectReason::ProtectedPath);
        // The protected path was never handed to the remover.
        assert!(
            !remover
                .calls
                .borrow()
                .iter()
                .any(|(_, p)| p == &env.children[0]),
            "a protected path must never reach the remover",
        );
    }

    #[test]
    fn real_remover_refuses_path_outside_allow_roots_toctou() {
        // Directly prove the TOCTOU re-assert in RealPathRemover.
        let allow = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let remover = RealPathRemover {
            parent_repo: allow.path().to_path_buf(),
            allow_roots: vec![allow.path().to_path_buf()],
        };
        let err = remover
            .remove(ReclaimPrimitive::RemoveDir, outside.path())
            .expect_err("outside allow-root must be refused");
        assert!(err.contains("not under any allow-root"), "got: {err}");
        assert!(
            outside.path().exists(),
            "the outside dir must NOT have been removed",
        );
    }

    #[test]
    fn real_remover_refuses_leading_dash_path() {
        let allow = TempDir::new().unwrap();
        let remover = RealPathRemover {
            parent_repo: allow.path().to_path_buf(),
            allow_roots: vec![allow.path().to_path_buf()],
        };
        let err = remover
            .remove(ReclaimPrimitive::RemoveDir, Path::new("-rf"))
            .expect_err("leading-dash path must be refused");
        assert!(err.contains("leading-dash"), "got: {err}");
    }

    #[test]
    fn summary_is_a_stable_one_liner() {
        let report = ReclaimReport {
            mode: ReclaimMode::Apply,
            used_pct_before: 88,
            used_pct_after: 84,
            target_pct: 85,
            bytes_freed: 12_026_531_840,
            removed: vec![],
            would_remove: vec![],
            skipped: vec![],
            failures: vec![],
        };
        assert_eq!(
            report.summary(),
            "disk reclaim: 88% -> 84% used, freed 12026531840 bytes, 0 paths removed, 0 skipped for review",
        );
    }
}
