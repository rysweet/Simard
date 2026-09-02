//! The non-bypassable rail: `vet_candidate`.
//!
//! Every candidate passes through [`vet_candidate`] **immediately before
//! deletion**. This is the deterministic filter the agentic selection step
//! cannot bypass — the agent proposes, this guard disposes. The guard can only
//! ever *shrink* the candidate set: no agent output can widen it past the rails.
//!
//! The guard **composes existing, already-tested primitives** rather than
//! reimplementing them:
//! - [`crate::cognitive_threads::threads::maintenance::is_safe_to_delete`] for
//!   canonicalize + symlink-refusal + allow-root containment ∧ ¬deny-set,
//! - [`crate::worktree_gc::liveness::LiveProcessProbe`] for the live-PID veto,
//! - a [`TrackedWorktreeProbe`] seam that re-derives the merged/closed-PR +
//!   idle + uncommitted/unpushed vetoes live (production wires it to
//!   [`crate::worktree_gc::evaluate_candidate`]).
//!
//! Every inconclusive signal resolves to `Reject` (fail-closed). There is no
//! override flag and no silent fallback.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::cognitive_threads::threads::maintenance::is_safe_to_delete;
use crate::worktree_gc::liveness::LiveProcessProbe;

use super::candidate::{CandidateKind, ReclaimCandidate};
use super::daemon_dir::resolve_daemon_working_dirs;

/// The concrete filesystem primitive the executor will run for an allowed
/// candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReclaimPrimitive {
    /// Tracked worktree: `git worktree prune` → `git worktree remove --force`.
    GitWorktreeRemoveForce,
    /// Orphan dir / stale cache: `rm -rf`, allow-root reasserted at the syscall.
    RemoveDir,
}

/// Why a rail refused a candidate. A **closed** enum — every reclaim skip maps
/// to exactly one of these, so telemetry never overflows to a generic bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectReason {
    /// `worktrees/main` or a resolved daemon `WorkingDirectory`.
    ProtectedPath,
    /// Referenced by a live PID (`/proc/<pid>/cwd` at/under the path).
    LiveProcess,
    /// Dirty tree or commits not contained in a merged/closed PR.
    UncommittedOrUnpushed,
    /// An active recipe/engineer worktree (tmux/PID) owns it.
    ActiveWorktree,
    /// Not under an allow-root, or canonicalization/symlink check failed.
    OutsideAllowRoot,
    /// The PR could not be positively classified as merged/closed.
    UnknownPrState,
}

/// The outcome of vetting one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Cleared all rails; execute `primitive`, expect ~`bytes` freed.
    Allow {
        primitive: ReclaimPrimitive,
        bytes: u64,
    },
    /// A rail refused; route to the human-review list. Never deleted.
    Reject { reason: RejectReason },
}

/// The protected deny-set: paths that must never be removed. The union of the
/// hardcoded `worktrees/main`, the runtime-resolved daemon working dirs, and any
/// operator-supplied `SIMARD_GIT_PROTECTED_REPOS`.
#[derive(Debug, Clone, Default)]
pub struct ProtectedDenySet {
    paths: Vec<PathBuf>,
    /// Canonicalized form of each entry in `paths` (index-aligned), computed
    /// once at construction so `contains` does not re-`canonicalize` the whole
    /// deny-set for every candidate. `None` where a path does not (yet) resolve
    /// — `contains` then falls back to a literal prefix check, exactly as before.
    canonical: Vec<Option<PathBuf>>,
}

impl ProtectedDenySet {
    /// Environment variable operators use to widen the deny-set (never the
    /// allow-set — widening the delete scope from the environment is a footgun).
    const PROTECTED_REPOS_ENV: &'static str = "SIMARD_GIT_PROTECTED_REPOS";

    /// Resolve the deny-set once per run against the injectable `proc_root`.
    pub fn resolve(proc_root: &Path) -> Self {
        let mut set = resolve_daemon_working_dirs(proc_root);
        // resolve_daemon_working_dirs already inserts the hardcoded main, but be
        // explicit so this invariant is local and obvious.
        set.insert(PathBuf::from(super::HARDCODED_PROTECTED_MAIN));
        if let Ok(raw) = std::env::var(Self::PROTECTED_REPOS_ENV) {
            for p in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                set.insert(PathBuf::from(p));
            }
        }
        Self::from_paths(set.into_iter().collect())
    }

    /// Build directly from an explicit path list (test seam).
    pub fn from_paths(paths: Vec<PathBuf>) -> Self {
        let canonical = paths.iter().map(|p| p.canonicalize().ok()).collect();
        Self { paths, canonical }
    }

    /// The raw protected paths, for handing to `is_safe_to_delete`.
    pub fn as_paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// `true` iff `candidate` equals or sits under any protected path. Compares
    /// canonical paths when both resolve, else falls back to a literal prefix
    /// check so a not-yet-created protected location still shields its future.
    pub fn contains(&self, candidate: &Path) -> bool {
        let canon = candidate.canonicalize().ok();
        self.paths
            .iter()
            .zip(&self.canonical)
            .any(
                |(deny, deny_canon)| match (canon.as_ref(), deny_canon.as_ref()) {
                    (Some(c), Some(d)) => c == d || c.starts_with(d),
                    _ => candidate == deny || candidate.starts_with(deny),
                },
            )
    }
}

/// The re-derived, live verdict for a tracked worktree candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeVerdict {
    /// Positively confirmed merged/closed PR, clean tree, idle, no live process.
    Reclaimable,
    /// A veto fired; route to human review with this reason.
    Reject(RejectReason),
}

/// Seam that re-derives the merged/closed-PR + idle + uncommitted/unpushed
/// vetoes for a tracked worktree, live, at vet time. Production wires this to
/// `git` + [`crate::worktree_gc`]; tests substitute a deterministic double.
pub trait TrackedWorktreeProbe {
    fn assess(&self, worktree: &Path) -> WorktreeVerdict;
}

/// Seam for measuring a path's size fresh at vet time. The freed-bytes figure
/// **never** comes from the agent's `est_bytes`.
pub trait SizeMeasurer {
    fn measure(&self, path: &Path) -> u64;
}

/// Production size measurer: `du -sb <path>`.
pub struct DuSizeMeasurer;

impl SizeMeasurer for DuSizeMeasurer {
    fn measure(&self, path: &Path) -> u64 {
        Command::new("du")
            .arg("-sb")
            .arg(path)
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    }
}

/// A run-scoped memoizing wrapper over any [`SizeMeasurer`]. Within a single
/// reclamation run a candidate's on-disk size is effectively constant, yet each
/// path is otherwise measured twice — once to order candidates largest-first and
/// again in the guard's `Allow` arm. Because `du -sb` walks the whole directory
/// tree, caching coalesces those into **one** measurement per unique path. It
/// still performs a real measurement (never the agent's `est_bytes`); it only
/// avoids repeating an identical one in the same run.
pub struct CachingSizeMeasurer<'a> {
    inner: &'a dyn SizeMeasurer,
    cache: std::sync::Mutex<std::collections::HashMap<PathBuf, u64>>,
}

impl<'a> CachingSizeMeasurer<'a> {
    /// Wrap `inner`, caching each measured path for the lifetime of this value.
    pub fn new(inner: &'a dyn SizeMeasurer) -> Self {
        Self {
            inner,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl SizeMeasurer for CachingSizeMeasurer<'_> {
    fn measure(&self, path: &Path) -> u64 {
        if let Some(&bytes) = self.cache.lock().unwrap().get(path) {
            return bytes;
        }
        let bytes = self.inner.measure(path);
        self.cache.lock().unwrap().insert(path.to_path_buf(), bytes);
        bytes
    }
}

/// Everything the guard needs, all injectable so tests are hermetic.
pub struct GuardContext<'a> {
    /// The positive containment allow-list (reclamation scope).
    pub allow_roots: &'a [PathBuf],
    /// The protected deny-set.
    pub protected: &'a ProtectedDenySet,
    /// Live-PID probe (fail-closed).
    pub live_probe: &'a dyn LiveProcessProbe,
    /// Re-derives the tracked-worktree vetoes live.
    pub wt_probe: &'a dyn TrackedWorktreeProbe,
    /// Fresh size measurement.
    pub measurer: &'a dyn SizeMeasurer,
}

/// Vet one candidate immediately before deletion. Returns [`Verdict::Allow`]
/// only when **every** rail passes; any inconclusive or failing rail yields
/// [`Verdict::Reject`] (fail-closed).
///
/// Order matters: the protected deny-set is checked **first** so it wins even
/// for a path that also happens to sit under an allow-root (e.g.
/// `worktrees/main`).
pub fn vet_candidate(candidate: &ReclaimCandidate, ctx: &GuardContext<'_>) -> Verdict {
    let path = candidate.path.as_path();

    // Rail 1 — protected deny-set (absolute; checked first so it always wins).
    if ctx.protected.contains(path) {
        return Verdict::Reject {
            reason: RejectReason::ProtectedPath,
        };
    }

    // Rail 2 — allow-root containment + symlink/canonicalize refusal. Reuses the
    // audited component-wise primitive (no string-prefix confusion).
    if !is_safe_to_delete(path, ctx.allow_roots, ctx.protected.as_paths()) {
        return Verdict::Reject {
            reason: RejectReason::OutsideAllowRoot,
        };
    }

    // Rail 3 — any live process at/under the path (applied to ALL kinds).
    if ctx.live_probe.worktree_has_live_process(path) {
        return Verdict::Reject {
            reason: RejectReason::LiveProcess,
        };
    }

    // Rail 4 — per-kind vetting. The agent-supplied `kind` is advisory and may
    // only ever *deepen* the vetting, never shorten it. A path that is actually
    // a git worktree/repo (a `.git` entry at its root) is ALWAYS routed through
    // the tracked-worktree vetoes — uncommitted/unpushed + merged/closed-PR —
    // even when the agent labelled it `orphan_dir`/`stale_build_cache`. This
    // closes the bypass where a mislabelled `kind` would skip the dirty-tree
    // veto and `rm -rf` committed-but-unpushed work. The check is fail-closed:
    // `symlink_metadata` treats any `.git` entry (file, dir, or even a broken
    // symlink) as a worktree, so it cannot be evaded. Rail 2 has already proven
    // `path` itself is a real (non-symlink) directory under an allow-root.
    let is_git_worktree = matches!(candidate.kind, CandidateKind::TrackedWorktree)
        || path.join(".git").symlink_metadata().is_ok();

    if is_git_worktree {
        match ctx.wt_probe.assess(path) {
            WorktreeVerdict::Reclaimable => Verdict::Allow {
                primitive: ReclaimPrimitive::GitWorktreeRemoveForce,
                bytes: ctx.measurer.measure(path),
            },
            WorktreeVerdict::Reject(reason) => Verdict::Reject { reason },
        }
    } else {
        Verdict::Allow {
            primitive: ReclaimPrimitive::RemoveDir,
            bytes: ctx.measurer.measure(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree_gc::liveness::FakeLiveProcessProbe;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // ---- test doubles -------------------------------------------------

    /// A tracked-worktree probe that returns a fixed verdict.
    struct FixedWtProbe(WorktreeVerdict);
    impl TrackedWorktreeProbe for FixedWtProbe {
        fn assess(&self, _worktree: &Path) -> WorktreeVerdict {
            self.0
        }
    }

    /// A size measurer backed by a path→bytes map (default 0).
    #[derive(Default)]
    struct MapMeasurer(Mutex<HashMap<PathBuf, u64>>);
    impl MapMeasurer {
        fn set(&self, path: &Path, bytes: u64) {
            self.0.lock().unwrap().insert(path.to_path_buf(), bytes);
        }
    }
    impl SizeMeasurer for MapMeasurer {
        fn measure(&self, path: &Path) -> u64 {
            *self.0.lock().unwrap().get(path).unwrap_or(&0)
        }
    }

    /// Build a candidate of `kind` at `path`.
    fn cand(path: &Path, kind: CandidateKind) -> ReclaimCandidate {
        ReclaimCandidate {
            path: path.to_path_buf(),
            kind,
            parent_repo: None,
            reason: Some("delete this!".to_string()),
            est_bytes: Some(9_999_999),
        }
    }

    /// A vetting harness: an allow-root tempdir with a child dir inside it.
    struct Harness {
        _root: TempDir,
        allow_roots: Vec<PathBuf>,
        child: PathBuf,
    }
    impl Harness {
        fn new(child_name: &str) -> Self {
            let root = TempDir::new().expect("allow root");
            let child = root.path().join(child_name);
            std::fs::create_dir_all(&child).expect("child dir");
            let allow_roots = vec![root.path().to_path_buf()];
            Self {
                _root: root,
                allow_roots,
                child,
            }
        }
    }

    fn vet(
        candidate: &ReclaimCandidate,
        allow_roots: &[PathBuf],
        protected: &ProtectedDenySet,
        live: &dyn LiveProcessProbe,
        wt: &dyn TrackedWorktreeProbe,
        measurer: &dyn SizeMeasurer,
    ) -> Verdict {
        let ctx = GuardContext {
            allow_roots,
            protected,
            live_probe: live,
            wt_probe: wt,
            measurer,
        };
        vet_candidate(candidate, &ctx)
    }

    // ---- the reclaimable happy path -----------------------------------

    #[test]
    fn orphan_dir_inside_allow_root_is_allowed_with_fresh_size() {
        let h = Harness::new("orphan-123");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
        let measurer = MapMeasurer::default();
        measurer.set(&h.child, 4096);

        let c = cand(&h.child, CandidateKind::OrphanDir);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Allow {
                primitive: ReclaimPrimitive::RemoveDir,
                bytes: 4096,
            },
            "the agent's est_bytes (9999999) must be ignored; fresh measure wins",
        );
    }

    #[test]
    fn tracked_worktree_is_allowed_only_when_probe_says_reclaimable() {
        let h = Harness::new("wt-merged");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();
        measurer.set(&h.child, 12_000_000_000);

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Allow {
                primitive: ReclaimPrimitive::GitWorktreeRemoveForce,
                bytes: 12_000_000_000,
            },
        );
    }

    /// Regression (issue #4722, brief acceptance): a **fresh worktree at
    /// origin/main** — a real git worktree whose branch has no positively
    /// confirmed merged/closed PR — must NOT be reclaimed, even though its tip is
    /// an ancestor of origin/main and even if the agent mislabels its `kind` as a
    /// disposable build cache. The old `git merge-base --is-ancestor <branch>
    /// origin/main` staleness test matched exactly this case and wrongly deleted
    /// the worktree's live build cache. The production probe fail-closes such a
    /// worktree to `UnknownPrState`; here we drive that verdict through the seam
    /// and assert the guard rejects (never Allow). The `.git` marker forces the
    /// tracked-worktree rails regardless of the advisory `kind`.
    #[test]
    fn fresh_origin_main_worktree_is_not_reclaimed() {
        let h = Harness::new("fresh-origin-main-wt");
        // Make it a real git worktree so the tracked-worktree rails engage even
        // though we deliberately mislabel the kind as StaleBuildCache below.
        std::fs::create_dir_all(h.child.join(".git")).expect(".git marker");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        // A fresh worktree with no merged/closed PR fail-closes to UnknownPrState.
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
        let measurer = MapMeasurer::default();
        measurer.set(&h.child, 8_000_000_000);

        let c = cand(&h.child, CandidateKind::StaleBuildCache);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::UnknownPrState
            },
            "a fresh origin/main worktree without a merged/closed PR must be kept, \
             not deleted — the mislabelled kind must not bypass the PR rails",
        );
        assert!(
            !matches!(v, Verdict::Allow { .. }),
            "must never Allow a worktree whose PR is not positively merged/closed",
        );
    }

    #[test]
    fn mislabelled_orphan_that_is_a_git_worktree_is_still_veto_checked() {
        // THE BYPASS THIS FIX CLOSES: an agent labels a real engineer worktree
        // holding committed-but-unpushed work as `orphan_dir`. The `.git` entry
        // forces the full tracked-worktree vetoes to run anyway, so the
        // uncommitted/unpushed veto still fires and the path is NOT `rm -rf`ed.
        let h = Harness::new("wt-mislabelled-as-orphan");
        std::fs::write(h.child.join(".git"), b"gitdir: /repo/.git/worktrees/x\n")
            .expect("write .git file");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::UncommittedOrUnpushed));
        let measurer = MapMeasurer::default();
        measurer.set(&h.child, 4096);

        let c = cand(&h.child, CandidateKind::OrphanDir);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::UncommittedOrUnpushed,
            },
            "a real worktree mislabelled `orphan_dir` must still hit the \
             uncommitted/unpushed veto — the agent's `kind` cannot shorten vetting",
        );
    }

    #[test]
    fn git_worktree_labelled_orphan_rederives_the_worktree_primitive() {
        // When the path is actually a worktree the guard re-derives the correct
        // primitive (`git worktree remove --force`), never a bare `rm -rf`,
        // regardless of the advisory `kind`.
        let h = Harness::new("wt-labelled-orphan-clean");
        std::fs::write(h.child.join(".git"), b"gitdir: /repo/.git/worktrees/y\n")
            .expect("write .git file");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();
        measurer.set(&h.child, 8192);

        let c = cand(&h.child, CandidateKind::StaleBuildCache);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Allow {
                primitive: ReclaimPrimitive::GitWorktreeRemoveForce,
                bytes: 8192,
            },
            "a worktree mislabelled as a cache must be removed via the \
             worktree primitive, not a raw rm -rf",
        );
    }

    // ---- HARD SAFETY RAILS: refuse even when instructed to delete ------

    #[test]
    fn rail_refuses_protected_main_even_inside_allow_root() {
        // `main` lives *inside* the allow-root (like `Simard/worktrees/main`
        // under `Simard/worktrees`) yet must still be refused.
        let h = Harness::new("main");
        let protected = ProtectedDenySet::from_paths(vec![h.child.clone()]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::ProtectedPath
            },
            "an LLM instructing deletion of a protected path must be refused",
        );
    }

    #[test]
    fn rail_refuses_daemon_working_directory() {
        let h = Harness::new("daemon-cwd");
        // Simulate resolve() having found this as a live daemon cwd.
        let protected = ProtectedDenySet::from_paths(vec![h.child.clone()]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::OrphanDir);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::ProtectedPath
            }
        );
    }

    #[test]
    fn rail_refuses_path_referenced_by_a_live_process() {
        let h = Harness::new("busy-wt");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        live.mark_live(h.child.clone());
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::LiveProcess
            },
            "a path in use by a live PID must never be removed",
        );
    }

    #[test]
    fn rail_refuses_uncommitted_or_unpushed_worktree() {
        let h = Harness::new("dirty-wt");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::UncommittedOrUnpushed));
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::UncommittedOrUnpushed
            },
            "a worktree carrying unsaved/unpushed work must never be removed",
        );
    }

    #[test]
    fn rail_refuses_active_worktree() {
        let h = Harness::new("active-wt");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::ActiveWorktree));
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::ActiveWorktree
            }
        );
    }

    #[test]
    fn rail_refuses_path_outside_every_allow_root() {
        // Candidate is a real dir but NOT under the allow-root.
        let outside = TempDir::new().expect("outside");
        let allow = TempDir::new().expect("allow root");
        let allow_roots = vec![allow.path().to_path_buf()];
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(outside.path(), CandidateKind::OrphanDir);
        let v = vet(&c, &allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::OutsideAllowRoot
            },
            "a path outside the reclamation scope must be refused first",
        );
    }

    #[test]
    fn rail_refuses_hallucinated_nonexistent_path() {
        let allow = TempDir::new().expect("allow root");
        let allow_roots = vec![allow.path().to_path_buf()];
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let bogus = allow.path().join("does-not-exist-zzz");
        let c = cand(&bogus, CandidateKind::OrphanDir);
        let v = vet(&c, &allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::OutsideAllowRoot
            }
        );
    }

    #[test]
    fn rail_refuses_symlink_candidate() {
        // A symlink pointing into the allow-root must be refused (SR-5 / TOCTOU).
        let allow = TempDir::new().expect("allow root");
        let real = allow.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = allow.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let allow_roots = vec![allow.path().to_path_buf()];
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(&link, CandidateKind::OrphanDir);
        let v = vet(&c, &allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::OutsideAllowRoot
            },
            "a symlink candidate must be refused by the canonical guard",
        );
    }

    #[test]
    fn rail_refuses_unknown_pr_state_tracked_worktree() {
        // The open-PR / merge-base-is-ancestor REGRESSION: a fresh worktree
        // whose PR is not positively merged/closed is NOT reclaimed. The prod
        // probe maps `evaluate_candidate == None` (no confirmed merged PR) to
        // UnknownPrState; here we assert the guard refuses it.
        let h = Harness::new("fresh-open-pr-wt");
        let protected = ProtectedDenySet::from_paths(vec![]);
        let live = FakeLiveProcessProbe::default();
        let wt = FixedWtProbe(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::UnknownPrState
            },
            "a fresh open-PR worktree must never be reclaimed (misfire regression)",
        );
    }

    #[test]
    fn protected_wins_over_liveness_and_kind() {
        // Even if the path is ALSO live and the probe says reclaimable, the
        // protected-path rail (checked first) is the reported reason.
        let h = Harness::new("main");
        let protected = ProtectedDenySet::from_paths(vec![h.child.clone()]);
        let live = FakeLiveProcessProbe::default();
        live.mark_live(h.child.clone());
        let wt = FixedWtProbe(WorktreeVerdict::Reclaimable);
        let measurer = MapMeasurer::default();

        let c = cand(&h.child, CandidateKind::TrackedWorktree);
        let v = vet(&c, &h.allow_roots, &protected, &live, &wt, &measurer);
        assert_eq!(
            v,
            Verdict::Reject {
                reason: RejectReason::ProtectedPath
            }
        );
    }

    #[test]
    fn caching_measurer_measures_each_path_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingMeasurer(AtomicUsize);
        impl SizeMeasurer for CountingMeasurer {
            fn measure(&self, _path: &Path) -> u64 {
                self.0.fetch_add(1, Ordering::SeqCst);
                42
            }
        }

        let inner = CountingMeasurer(AtomicUsize::new(0));
        let caching = CachingSizeMeasurer::new(&inner);
        let p = Path::new("/some/path");
        assert_eq!(caching.measure(p), 42);
        assert_eq!(caching.measure(p), 42, "second call returns cached value");
        assert_eq!(caching.measure(Path::new("/other")), 42);
        assert_eq!(
            inner.0.load(Ordering::SeqCst),
            2,
            "a repeated path must hit the cache; only distinct paths measure",
        );
    }

    #[test]
    fn deny_set_contains_matches_subpaths() {
        let root = TempDir::new().unwrap();
        let protected_dir = root.path().join("protected");
        let sub = protected_dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let set = ProtectedDenySet::from_paths(vec![protected_dir.clone()]);
        assert!(set.contains(&protected_dir), "equal path is protected");
        assert!(set.contains(&sub), "subpath is protected");
        assert!(!set.contains(root.path()), "parent is not protected");
    }
}
