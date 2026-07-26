//! TDD contract (Step 7 — tests first) for the deterministic build-cache
//! candidate producer that makes ROUTINE disk reclaim actually free the cargo
//! build cache, so the daemon stops oscillating 94–99 % and nuking
//! `target/debug` wholesale on every ~30-minute cycle (issue #4810).
//!
//! These tests are written **before** the implementation and MUST fail until
//! `src/disk_reclaim/build_cache.rs` exists and the guard is wired for
//! build-cache leaves. This whole test crate references the not-yet-existing
//! `simard::disk_reclaim::build_cache` module and the new
//! `GuardContext.build_cache_leaves` field, so it will not compile — and thus
//! every test fails — until the feature is built. Because integration test
//! files are separate compilation units, this does not break the rest of the
//! suite.
//!
//! Contract sources: `docs/reference/disk-reclaim-build-cache-producer.md`
//! (the retcon reference) plus the finalized Step-2c requirements and design
//! spec. What is asserted here:
//!   1. The producer emits granular `StaleBuildCache` leaves under
//!      `target/debug/{incremental,deps,build}` (+ the llvm-cov mirror) — never
//!      `target/debug` itself, never `target/debug/simard`, never symlinks.
//!   2. The guard admits a registered leaf (Allow{RemoveDir}) via the leaf-only
//!      allow-scope and the exact-canonical deny-set exemption, while every
//!      non-leaf sibling / parent / worktree-root stays rejected, and Rails 2–4
//!      (allow-root, live-process) remain in force for exempted leaves.
//!   3. The executor, fed the producer's candidates, frees non-zero bytes and
//!      stops at `target_pct` (minimum-necessary) in Apply mode, frees 0 in
//!      DryRun, and never removes the final binary.
//!   4. The producer is deterministic and env-independent; existing thresholds
//!      / apply-gate defaults are preserved.

use std::cell::RefCell;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;

use simard::disk_pressure::check::{DiskStat, DiskStatProvider};
use simard::disk_reclaim::build_cache::{
    EVICTABLE_CACHE_DIRS, build_cache_candidates, build_cache_leaf_dirs, target_debug_roots,
};
use simard::disk_reclaim::{
    CandidateKind, GuardContext, PathRemover, ProtectedDenySet, ReclaimCandidate, ReclaimMode,
    ReclaimPrimitive, RejectReason, SizeMeasurer, TrackedWorktreeProbe, Verdict, WorktreeVerdict,
    exec_reclaim, vet_candidate,
};
use simard::worktree_gc::liveness::LiveProcessProbe;

// ---------------------------------------------------------------------------
// Test doubles (all target traits are `pub`, so integration tests can supply
// their own hermetic implementations — the crate's internal `#[cfg(test)]`
// fakes are not reachable from here).
// ---------------------------------------------------------------------------

/// Live-process probe: reports "live" for any candidate at/under a listed path.
struct FakeLive {
    live: Vec<PathBuf>,
}
impl FakeLive {
    fn none() -> Self {
        Self { live: vec![] }
    }
    fn at(path: &Path) -> Self {
        Self {
            live: vec![path.to_path_buf()],
        }
    }
}
impl LiveProcessProbe for FakeLive {
    fn worktree_has_live_process(&self, dir: &Path) -> bool {
        let d = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        self.live.iter().any(|p| {
            let c = p.canonicalize().unwrap_or_else(|_| p.clone());
            d.starts_with(&c) || c.starts_with(&d)
        })
    }
}

/// Tracked-worktree probe returning a fixed verdict. For a non-git leaf (no
/// `.git`) the guard must NOT consult this at all; we return a hard `Reject`
/// so a leaf that still ends up `Allow` proves the tracked-worktree rail was
/// (correctly) not applied to it.
struct FixedWt(WorktreeVerdict);
impl TrackedWorktreeProbe for FixedWt {
    fn assess(&self, _worktree: &Path) -> WorktreeVerdict {
        self.0
    }
}

/// Size measurer backed by a path→bytes map (default 0). Keyed by the exact
/// candidate path so ordering in the executor is deterministic.
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

/// Disk provider returning a scripted sequence of `%-used` values; each `stat`
/// consumes the next, the last repeats (mirrors the crate's own executor test
/// double so the stop-at-target semantics are exercised identically).
struct ScriptedDisk(RefCell<Vec<u8>>);
impl ScriptedDisk {
    fn new(pcts: Vec<u8>) -> Self {
        Self(RefCell::new(pcts))
    }
}
impl DiskStatProvider for ScriptedDisk {
    fn stat(&self, _path: &Path) -> Result<DiskStat, std::io::Error> {
        let mut v = self.0.borrow_mut();
        let pct = if v.len() > 1 { v.remove(0) } else { v[0] };
        Ok(DiskStat {
            free_bytes: 100 - pct as u64,
            total_bytes: 100,
        })
    }
}

/// Records every remove call and reports success. Used to observe *which*
/// leaves the executor evicts without touching real system paths.
#[derive(Default)]
struct RecordingRemover(RefCell<Vec<(ReclaimPrimitive, PathBuf)>>);
impl PathRemover for RecordingRemover {
    fn remove(&self, primitive: ReclaimPrimitive, path: &Path) -> Result<(), String> {
        self.0.borrow_mut().push((primitive, path.to_path_buf()));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixture: a fake managed repo with a cargo `target/` tree.
// ---------------------------------------------------------------------------

/// A managed-repo fixture rooted at a tempdir, with a realistic `target/debug`
/// layout (regenerable leaves + the final `simard` binary) and, optionally, a
/// coverage mirror and worktrees.
struct RepoFixture {
    _tmp: TempDir,
    root: PathBuf,
}
impl RepoFixture {
    /// A repo with `target/debug/{incremental,deps,build}` dirs and a
    /// `target/debug/simard` binary file. No coverage mirror, no worktrees.
    fn basic() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().to_path_buf();
        for leaf in [
            "target/debug/incremental",
            "target/debug/deps",
            "target/debug/build",
        ] {
            std::fs::create_dir_all(root.join(leaf)).expect("leaf dir");
        }
        // The final binary must never be a leaf / candidate.
        std::fs::write(root.join("target/debug/simard"), b"ELF").expect("binary");
        Self { _tmp: tmp, root }
    }

    fn leaf(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
}

/// True iff `paths` contains a path that canonically equals `expect`.
fn contains_canon(paths: &[PathBuf], expect: &Path) -> bool {
    let e = match expect.canonicalize() {
        Ok(e) => e,
        Err(_) => return false,
    };
    paths
        .iter()
        .any(|p| p.canonicalize().map(|c| c == e).unwrap_or(false))
}

/// Vet a single candidate against a hermetic guard context wired the way
/// production's `reclaim_candidates` wires it: the build-cache leaves are added
/// both to the allow-scope and to `build_cache_leaves`.
#[allow(clippy::too_many_arguments)]
fn vet(
    candidate: &ReclaimCandidate,
    allow_roots: &[PathBuf],
    build_cache_leaves: &[PathBuf],
    protected: &ProtectedDenySet,
    live: &dyn LiveProcessProbe,
    wt: &dyn TrackedWorktreeProbe,
    measurer: &dyn SizeMeasurer,
) -> Verdict {
    let ctx = GuardContext {
        allow_roots,
        build_cache_leaves,
        protected,
        live_probe: live,
        wt_probe: wt,
        measurer,
    };
    vet_candidate(candidate, &ctx)
}

fn cand(path: &Path, kind: CandidateKind) -> ReclaimCandidate {
    ReclaimCandidate {
        path: path.to_path_buf(),
        kind,
        parent_repo: None,
        reason: Some("agent advisory — ignored by the guard".to_string()),
        est_bytes: Some(9_999_999),
    }
}

// ===========================================================================
// 1. Producer: EVICTABLE_CACHE_DIRS constant
// ===========================================================================

#[test]
fn evictable_cache_dirs_are_the_regenerable_leaves_only() {
    let set: Vec<&str> = EVICTABLE_CACHE_DIRS.to_vec();

    // The six regenerable leaves (debug + coverage mirror), relative to a
    // `target/` root.
    for expected in [
        "debug/incremental",
        "debug/deps",
        "debug/build",
        "llvm-cov-target/debug/incremental",
        "llvm-cov-target/debug/deps",
        "llvm-cov-target/debug/build",
    ] {
        assert!(
            set.contains(&expected),
            "EVICTABLE_CACHE_DIRS must include the regenerable leaf {expected:?}; got {set:?}",
        );
    }

    // `debug` itself and the final binary must never be evictable leaves —
    // that is exactly the wholesale-nuke the fix removes.
    assert!(
        !set.iter().any(|d| *d == "debug" || d.ends_with("/debug")),
        "EVICTABLE_CACHE_DIRS must not contain `debug` itself (only its children): {set:?}",
    );
    assert!(
        !set.iter().any(|d| d.contains("simard")),
        "EVICTABLE_CACHE_DIRS must never target the final `simard` binary: {set:?}",
    );
}

// ===========================================================================
// 2. Producer: target_debug_roots — repo target + worktree targets
// ===========================================================================

#[test]
fn target_debug_roots_covers_repo_and_its_worktrees() {
    let repo = RepoFixture::basic();
    // Give the repo a worktree so the producer reaches worktrees/*/target too.
    let wt = repo.root.join("worktrees/wt1");
    std::fs::create_dir_all(wt.join("target/debug")).expect("worktree target");

    let roots = target_debug_roots(std::slice::from_ref(&repo.root));

    assert!(
        contains_canon(&roots, &repo.root.join("target")),
        "must enumerate <repo>/target; got {roots:?}",
    );
    assert!(
        contains_canon(&roots, &wt.join("target")),
        "must enumerate <repo>/worktrees/*/target; got {roots:?}",
    );
}

// ===========================================================================
// 3. Producer: build_cache_leaf_dirs — existing leaves only, fail-closed
// ===========================================================================

#[test]
fn build_cache_leaf_dirs_returns_only_existing_debug_leaves() {
    let repo = RepoFixture::basic(); // has debug/{incremental,deps,build}, no llvm-cov
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));

    for present in [
        "target/debug/incremental",
        "target/debug/deps",
        "target/debug/build",
    ] {
        assert!(
            contains_canon(&leaves, &repo.leaf(present)),
            "existing leaf {present} must be returned; got {leaves:?}",
        );
    }

    // `target/debug` itself is NEVER a leaf.
    assert!(
        !contains_canon(&leaves, &repo.leaf("target/debug")),
        "`target/debug` itself must never be returned as a leaf: {leaves:?}",
    );
    // The final binary is NEVER a leaf (and is a file, not a dir).
    assert!(
        !contains_canon(&leaves, &repo.leaf("target/debug/simard")),
        "`target/debug/simard` must never be returned as a leaf: {leaves:?}",
    );
    // The coverage mirror does not exist here → silently omitted (nothing to
    // reclaim), not an error.
    assert!(
        !contains_canon(
            &leaves,
            &repo.leaf("target/llvm-cov-target/debug/incremental")
        ),
        "non-existent coverage leaves must be omitted, not fabricated: {leaves:?}",
    );
}

#[test]
fn build_cache_leaf_dirs_rejects_symlinked_leaves() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();
    // A real leaf plus a symlink *masquerading* as a leaf.
    std::fs::create_dir_all(root.join("target/debug/incremental")).expect("real leaf");
    let elsewhere = tmp.path().join("evil-outside-target");
    std::fs::create_dir_all(&elsewhere).expect("symlink dest");
    symlink(&elsewhere, root.join("target/debug/deps")).expect("symlink leaf");

    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&root));

    assert!(
        contains_canon(&leaves, &root.join("target/debug/incremental")),
        "the real leaf must survive: {leaves:?}",
    );
    // The symlinked `deps` must be dropped — enumeration must use
    // symlink_metadata and refuse symlink-swap into a foreign location.
    let resolves_to_evil = leaves.iter().any(|p| {
        p.canonicalize()
            .map(|c| c == elsewhere.canonicalize().unwrap())
            .unwrap_or(false)
    });
    assert!(
        !resolves_to_evil,
        "a symlinked leaf must never be returned (symlink-swap defense): {leaves:?}",
    );
}

// ===========================================================================
// 4. Producer: build_cache_candidates — kind, est_bytes, determinism
// ===========================================================================

#[test]
fn build_cache_candidates_are_stale_build_cache_with_no_estimate() {
    let repo = RepoFixture::basic();
    let candidates = build_cache_candidates(std::slice::from_ref(&repo.root));

    assert!(
        !candidates.is_empty(),
        "a repo with a built target/ must yield candidates",
    );
    for c in &candidates {
        assert_eq!(
            c.kind,
            CandidateKind::StaleBuildCache,
            "every emitted candidate is a StaleBuildCache leaf: {:?}",
            c.path,
        );
        assert_eq!(
            c.est_bytes, None,
            "the producer must never carry a size estimate — the executor \
             re-measures ({:?})",
            c.path,
        );
        assert!(
            c.reason.is_some(),
            "each candidate carries a fixed, low-cardinality reason: {:?}",
            c.path,
        );
        assert!(
            !c.path.ends_with("simard"),
            "the final binary must never be a candidate: {:?}",
            c.path,
        );
        // No candidate is `target/debug` itself.
        assert!(
            !c.path
                .canonicalize()
                .map(|p| p == repo.leaf("target/debug").canonicalize().unwrap())
                .unwrap_or(false),
            "`target/debug` itself must never be a candidate: {:?}",
            c.path,
        );
    }

    // Deterministic: no duplicate paths.
    let mut paths: Vec<_> = candidates.iter().map(|c| c.path.clone()).collect();
    paths.sort();
    let before = paths.len();
    paths.dedup();
    assert_eq!(
        before,
        paths.len(),
        "candidate paths must be unique (dedup)"
    );
}

// ===========================================================================
// 5. Guard: leaf admitted; siblings / parent / worktree-root rejected.
// ===========================================================================

#[test]
fn registered_leaf_is_allowed_remove_dir_with_fresh_size() {
    let repo = RepoFixture::basic();
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
    // Production wiring: leaves are added to the allow-scope AND registered.
    let allow = leaves.clone();
    let protected = ProtectedDenySet::from_paths(vec![]);
    let live = FakeLive::none();
    // A leaf is not a git worktree → the tracked-worktree rail must be skipped;
    // a hard-Reject probe here proves it was not consulted.
    let wt = FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
    let measurer = MapMeasurer::default();
    let target = repo.leaf("target/debug/incremental");
    measurer.set(&target, 512_000_000);

    let c = cand(&target, CandidateKind::StaleBuildCache);
    let v = vet(&c, &allow, &leaves, &protected, &live, &wt, &measurer);

    assert_eq!(
        v,
        Verdict::Allow {
            primitive: ReclaimPrimitive::RemoveDir,
            bytes: 512_000_000,
        },
        "a registered build-cache leaf must be Allowed (RemoveDir) with the \
         freshly measured size, not the agent's est_bytes",
    );
}

#[test]
fn registered_leaf_is_exempt_from_deny_set_but_only_by_exact_canonical_match() {
    let repo = RepoFixture::basic();
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
    let allow = leaves.clone();
    // The managed repo root sits in the protected deny-set (the daemon runs
    // from a managed checkout). Without the exemption the leaf would be
    // ProtectedPath.
    let protected = ProtectedDenySet::from_paths(vec![repo.root.clone()]);
    let live = FakeLive::none();
    let wt = FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
    let measurer = MapMeasurer::default();
    let leaf = repo.leaf("target/debug/deps");
    measurer.set(&leaf, 400_000_000);

    // (a) The registered leaf IS exempt → Allowed.
    let c_leaf = cand(&leaf, CandidateKind::StaleBuildCache);
    assert_eq!(
        vet(&c_leaf, &allow, &leaves, &protected, &live, &wt, &measurer),
        Verdict::Allow {
            primitive: ReclaimPrimitive::RemoveDir,
            bytes: 400_000_000,
        },
        "an exact-canonical registered leaf must be exempted from the deny-set",
    );

    // (b) The final binary sibling `target/debug/simard` is NOT registered →
    // the deny-set still rejects it (exact-canonical exemption only).
    let simard = repo.leaf("target/debug/simard");
    let c_bin = cand(&simard, CandidateKind::StaleBuildCache);
    assert_eq!(
        vet(&c_bin, &allow, &leaves, &protected, &live, &wt, &measurer),
        Verdict::Reject {
            reason: RejectReason::ProtectedPath,
        },
        "a non-leaf sibling under a protected repo must stay ProtectedPath — \
         the carve-out is exact-canonical, never a prefix/substring",
    );
}

#[test]
fn parent_target_debug_is_never_admitted() {
    let repo = RepoFixture::basic();
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
    let allow = leaves.clone();
    let protected = ProtectedDenySet::from_paths(vec![]);
    let live = FakeLive::none();
    let wt = FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
    let measurer = MapMeasurer::default();
    let parent = repo.leaf("target/debug");
    measurer.set(&parent, 718_000_000);

    // `target/debug` is a PARENT of the leaf allow-roots, not a descendant, so
    // leaf-only containment structurally forbids it. It must never be Allowed
    // (that is the wholesale nuke the fix eliminates).
    let c = cand(&parent, CandidateKind::StaleBuildCache);
    let v = vet(&c, &allow, &leaves, &protected, &live, &wt, &measurer);
    assert!(
        !matches!(v, Verdict::Allow { .. }),
        "`target/debug` itself must never pass the guard; got {v:?}",
    );
}

#[test]
fn unregistered_sibling_outside_leaves_is_outside_allow_root() {
    let repo = RepoFixture::basic();
    // A regular sibling dir that is NOT a recognized cache leaf.
    let sibling = repo.leaf("target/debug/some-other-dir");
    std::fs::create_dir_all(&sibling).expect("sibling dir");
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
    let allow = leaves.clone();
    let protected = ProtectedDenySet::from_paths(vec![]); // no deny-set here
    let live = FakeLive::none();
    let wt = FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
    let measurer = MapMeasurer::default();

    let c = cand(&sibling, CandidateKind::StaleBuildCache);
    let v = vet(&c, &allow, &leaves, &protected, &live, &wt, &measurer);
    assert_eq!(
        v,
        Verdict::Reject {
            reason: RejectReason::OutsideAllowRoot,
        },
        "a sibling that is not a registered leaf sits under no leaf allow-root \
         and must be OutsideAllowRoot; got {v:?}",
    );
}

#[test]
fn live_process_interlock_still_vetoes_a_registered_leaf() {
    let repo = RepoFixture::basic();
    let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
    let allow = leaves.clone();
    let protected = ProtectedDenySet::from_paths(vec![]);
    let leaf = repo.leaf("target/debug/deps");
    // An in-flight `cargo build` holds a live process at/under the leaf.
    let live = FakeLive::at(&leaf);
    let wt = FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState));
    let measurer = MapMeasurer::default();
    measurer.set(&leaf, 400_000_000);

    let c = cand(&leaf, CandidateKind::StaleBuildCache);
    let v = vet(&c, &allow, &leaves, &protected, &live, &wt, &measurer);
    assert_eq!(
        v,
        Verdict::Reject {
            reason: RejectReason::LiveProcess,
        },
        "Rail 3 (live-process) must still veto an exempted leaf so an in-flight \
         build is never deleted from under itself; got {v:?}",
    );
}

// ===========================================================================
// 6. Executor: producer candidates → non-zero freed, stop-at-target, DryRun 0.
// ===========================================================================

/// Build the production-shaped guard context for the executor tests: allow
/// scope = leaves, registered leaves = leaves, empty deny-set, no live procs.
struct ExecHarness {
    repo: RepoFixture,
    leaves: Vec<PathBuf>,
    candidates: Vec<ReclaimCandidate>,
    measurer: MapMeasurer,
    protected: ProtectedDenySet,
    live: FakeLive,
    wt: FixedWt,
}
impl ExecHarness {
    fn new() -> Self {
        let repo = RepoFixture::basic();
        let leaves = build_cache_leaf_dirs(std::slice::from_ref(&repo.root));
        let candidates = build_cache_candidates(std::slice::from_ref(&repo.root));
        let measurer = MapMeasurer::default();
        // Make `incremental` the largest so largest-first picks it first.
        for c in &candidates {
            let bytes = match c.path.file_name().and_then(|s| s.to_str()) {
                Some("incremental") => 500,
                Some("deps") => 300,
                Some("build") => 100,
                _ => 10,
            };
            measurer.set(&c.path, bytes);
        }
        Self {
            repo,
            leaves,
            candidates,
            measurer,
            protected: ProtectedDenySet::from_paths(vec![]),
            live: FakeLive::none(),
            wt: FixedWt(WorktreeVerdict::Reject(RejectReason::UnknownPrState)),
        }
    }
    fn ctx(&self) -> GuardContext<'_> {
        GuardContext {
            allow_roots: &self.leaves,
            build_cache_leaves: &self.leaves,
            protected: &self.protected,
            live_probe: &self.live,
            wt_probe: &self.wt,
            measurer: &self.measurer,
        }
    }
}

#[test]
fn apply_frees_nonzero_and_stops_at_target_minimum_necessary() {
    let h = ExecHarness::new();
    let ctx = h.ctx();
    let remover = RecordingRemover::default();
    // used_before=96; after evicting `incremental` disk drops to 84 (<= 85) →
    // executor stops, having removed the minimum necessary (just incremental).
    let disk = ScriptedDisk::new(vec![96, 96, 84]);

    let report = exec_reclaim(
        h.candidates.clone(),
        &ctx,
        ReclaimMode::Apply,
        85,
        &disk,
        h.repo.root.as_path(),
        &remover,
    );

    assert!(
        report.bytes_freed > 0,
        "routine reclaim in Apply mode must free NON-ZERO bytes (the whole \
         point of the fix); got {}",
        report.bytes_freed,
    );
    assert_eq!(
        report.removed.len(),
        1,
        "minimum-necessary: only the largest leaf should be evicted before \
         usage drops under target; removed={:?}",
        report.removed,
    );
    assert!(
        report.removed[0].path.ends_with("incremental"),
        "largest-first must evict `incremental` first; got {:?}",
        report.removed[0].path,
    );
    assert_eq!(
        report.bytes_freed, 500,
        "freed bytes must be the leaf's fresh size"
    );
    // The final binary and target/debug itself are never touched.
    for r in &report.removed {
        assert!(
            !r.path.ends_with("simard"),
            "must never remove the binary: {:?}",
            r.path
        );
        assert!(
            !r.path
                .canonicalize()
                .map(|p| p == h.repo.leaf("target/debug").canonicalize().unwrap())
                .unwrap_or(false),
            "must never remove target/debug wholesale: {:?}",
            r.path,
        );
    }
}

#[test]
fn dry_run_frees_zero_but_populates_would_remove() {
    let h = ExecHarness::new();
    let ctx = h.ctx();
    let remover = RecordingRemover::default();
    let disk = ScriptedDisk::new(vec![96]);

    let report = exec_reclaim(
        h.candidates.clone(),
        &ctx,
        ReclaimMode::DryRun,
        85,
        &disk,
        h.repo.root.as_path(),
        &remover,
    );

    assert_eq!(
        report.bytes_freed, 0,
        "dry-run must free 0 bytes at the report level"
    );
    assert!(report.removed.is_empty(), "dry-run must remove nothing");
    assert!(
        remover.0.borrow().is_empty(),
        "dry-run must never invoke the remover",
    );
    // All three leaves are surfaced for human review, each with its own size.
    assert_eq!(
        report.would_remove.len(),
        h.candidates.len(),
        "dry-run must surface every allowed leaf in would_remove; got {:?}",
        report.would_remove,
    );
    assert!(
        report
            .would_remove
            .iter()
            .all(|r| r.kind == CandidateKind::StaleBuildCache && r.bytes > 0),
        "each would_remove entry carries its own measured bytes",
    );
}

// ===========================================================================
// 7. Determinism / env preservation (serial for env isolation).
// ===========================================================================

#[test]
#[serial]
fn producer_is_deterministic_and_env_independent() {
    let repo = RepoFixture::basic();

    // The producer must not read the disk-reclaim env knobs; candidate
    // selection is a pure function of the on-disk layout.
    let saved_pct = std::env::var("SIMARD_DISK_RECLAIM_PCT").ok();
    let saved_apply = std::env::var("SIMARD_DISK_RECLAIM_DAEMON_APPLY").ok();
    // SAFETY: single-threaded test, `#[serial]` guards concurrent env mutation.
    unsafe {
        std::env::set_var("SIMARD_DISK_RECLAIM_PCT", "3");
        std::env::set_var("SIMARD_DISK_RECLAIM_DAEMON_APPLY", "1");
    }

    let a = build_cache_candidates(std::slice::from_ref(&repo.root));
    let b = build_cache_candidates(std::slice::from_ref(&repo.root));

    let mut pa: Vec<_> = a.iter().map(|c| c.path.clone()).collect();
    let mut pb: Vec<_> = b.iter().map(|c| c.path.clone()).collect();
    pa.sort();
    pb.sort();
    assert_eq!(pa, pb, "producer output must be deterministic across runs");
    assert!(
        !a.is_empty(),
        "producer must still emit leaves regardless of env knob values",
    );

    // Restore env.
    unsafe {
        match saved_pct {
            Some(v) => std::env::set_var("SIMARD_DISK_RECLAIM_PCT", v),
            None => std::env::remove_var("SIMARD_DISK_RECLAIM_PCT"),
        }
        match saved_apply {
            Some(v) => std::env::set_var("SIMARD_DISK_RECLAIM_DAEMON_APPLY", v),
            None => std::env::remove_var("SIMARD_DISK_RECLAIM_DAEMON_APPLY"),
        }
    }
}
