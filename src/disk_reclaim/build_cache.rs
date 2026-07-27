//! The **deterministic** half of routine disk reclamation (issue #4810).
//!
//! Where the `disk-reclaim.yaml` agent *proposes* candidates non-deterministically,
//! this module enumerates the regenerable cargo build-cache leaf directories
//! itself — the same set every run, no LLM in the loop — and contributes them to
//! the candidate list the guarded executor disposes of.
//!
//! It exists because ROUTINE reclaim (`85 % ≤ used < 95 %`) never freed the build
//! target: the routine `allow_roots` admitted `<repo>/worktrees` and the shared
//! cargo target dirs but **not** a managed repo's own `<repo>/target`, so every
//! `repo_root/target/*` candidate was rejected `OutsideAllowRoot` → `freed 0
//! bytes, everything skipped for review`. Only the emergency net (`disk_health.rs`
//! at `≥ 95 %`) ever freed the target, and only by `rm -rf`-ing all of
//! `target/debug` (~718 MB) which the next `cargo build` immediately regrew — a
//! delete-rebuild-delete loop.
//!
//! The producer closes the gap **incrementally**: because the executor is
//! all-or-nothing per candidate path but stops as soon as usage drops under
//! `target_pct`, emitting the regenerable **leaves** (`target/debug/incremental`,
//! `…/deps`, `…/build`, and the `llvm-cov-target` mirror) lets the executor evict
//! the fewest cache classes needed to get back under target — never the wholesale
//! `target/debug`, and never the final `simard` binary.
//!
//! See `docs/reference/disk-reclaim-build-cache-producer.md` for the full
//! contract and the two narrow guard changes (leaf-only allow-scope + the
//! exact-canonical deny-set exemption) that admit these candidates.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::candidate::{CandidateKind, ReclaimCandidate};

/// The regenerable cargo cache leaf dirs, relative to a `target/` root. These are
/// `target/debug/{incremental,deps,build}` and the coverage mirror under
/// `target/llvm-cov-target/debug/*`. `debug/` itself and final binaries
/// (`debug/simard`) are deliberately absent — evicting those is the wholesale
/// nuke this fix removes.
pub const EVICTABLE_CACHE_DIRS: &[&str] = &[
    "debug/incremental",
    "debug/deps",
    "debug/build",
    "llvm-cov-target/debug/incremental",
    "llvm-cov-target/debug/deps",
    "llvm-cov-target/debug/build",
];

/// Fixed, low-cardinality rationale carried on every emitted candidate. The
/// guard re-derives the real primitive and the executor re-measures the size, so
/// this string is purely informational (and never a telemetry attribute).
const BUILD_CACHE_REASON: &str = "regenerable cargo build cache (routine reclaim)";

/// The `target/` roots to enumerate for a set of managed repos: `<repo>/target`
/// for each repo plus `<repo>/worktrees/*/target` for each of its worktrees.
///
/// Worktree subdirectories are read via `read_dir` (which does not follow the
/// final symlink component), and a symlinked `worktrees/*` entry is skipped
/// (`file_type().is_dir()` is false for a symlink) so a swapped link cannot
/// redirect enumeration into a foreign tree. The returned roots are **not**
/// filtered for existence here — [`build_cache_leaf_dirs`] does the existence,
/// symlink, and ownership vetting per leaf.
pub fn target_debug_roots(repos: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for repo in repos {
        roots.push(repo.join("target"));

        let worktrees = repo.join("worktrees");
        if let Ok(entries) = std::fs::read_dir(&worktrees) {
            let mut wt_targets: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path().join("target"))
                .collect();
            // Deterministic ordering regardless of directory iteration order.
            wt_targets.sort();
            roots.extend(wt_targets);
        }
    }
    roots
}

/// The canonicalized, vetted build-cache leaf directories that actually exist
/// for the given managed `repos`. Internally derives the `target/` roots via
/// [`target_debug_roots`], then for each root checks every entry in
/// [`EVICTABLE_CACHE_DIRS`] and admits it **only** when it is a real, non-symlink
/// directory owned by the effective UID; it is then canonicalized. Everything
/// else (missing leaf, symlink, non-directory, foreign owner, canonicalize
/// failure) is silently dropped — **fail-closed**. The result is deduplicated and
/// is the exact allowlist threaded into the guard as `build_cache_leaves`.
pub fn build_cache_leaf_dirs(repos: &[PathBuf]) -> Vec<PathBuf> {
    let mut leaves: Vec<PathBuf> = Vec::new();
    for root in target_debug_roots(repos) {
        for rel in EVICTABLE_CACHE_DIRS {
            if let Some(canon) = vetted_leaf(&root.join(rel))
                && !leaves.contains(&canon)
            {
                leaves.push(canon);
            }
        }
    }
    leaves
}

/// The deterministic `StaleBuildCache` candidates for the given managed repos —
/// one per existing leaf, `est_bytes = None` (the executor re-measures; the
/// producer never trusts an estimate for a size decision). Deterministic and
/// env-independent: candidate selection is a pure function of the on-disk layout.
pub fn build_cache_candidates(repos: &[PathBuf]) -> Vec<ReclaimCandidate> {
    build_cache_candidates_from_leaves(build_cache_leaf_dirs(repos))
}

/// The allocation-only half of [`build_cache_candidates`]: wrap already-vetted
/// leaf dirs as `StaleBuildCache` candidates. Split out so a caller that has
/// already paid for [`build_cache_leaf_dirs`] (production's `run_disk_reclaim`,
/// which also needs the same leaves as the guard allowlist) reuses that single
/// filesystem walk instead of re-`read_dir`/`canonicalize`-ing every leaf twice.
pub fn build_cache_candidates_from_leaves(leaves: Vec<PathBuf>) -> Vec<ReclaimCandidate> {
    leaves
        .into_iter()
        .map(|path| ReclaimCandidate {
            path,
            kind: CandidateKind::StaleBuildCache,
            parent_repo: None,
            reason: Some(BUILD_CACHE_REASON.to_string()),
            est_bytes: None,
        })
        .collect()
}

/// Vet one candidate leaf: it must be a real, non-symlink directory owned by the
/// effective UID. Returns its canonical path on success, `None` otherwise
/// (fail-closed). Uses `symlink_metadata` so the final component is not followed,
/// closing symlink-swap into a foreign or protected location.
fn vetted_leaf(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return None;
    }
    // SAFETY: `geteuid` takes no arguments, reads no memory, and cannot fail.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        return None;
    }
    std::fs::canonicalize(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    /// Create the three regenerable `target/debug/*` leaves plus a `simard`
    /// binary file under `root`.
    fn basic_repo(root: &Path) {
        for leaf in [
            "target/debug/incremental",
            "target/debug/deps",
            "target/debug/build",
        ] {
            std::fs::create_dir_all(root.join(leaf)).expect("leaf dir");
        }
        std::fs::write(root.join("target/debug/simard"), b"ELF").expect("binary");
    }

    #[test]
    fn evictable_dirs_exclude_debug_root_and_binary() {
        assert!(EVICTABLE_CACHE_DIRS.contains(&"debug/incremental"));
        assert!(EVICTABLE_CACHE_DIRS.contains(&"llvm-cov-target/debug/deps"));
        assert!(
            !EVICTABLE_CACHE_DIRS
                .iter()
                .any(|d| *d == "debug" || d.ends_with("/debug")),
            "`debug` itself must never be an evictable leaf",
        );
        assert!(
            !EVICTABLE_CACHE_DIRS.iter().any(|d| d.contains("simard")),
            "the final binary must never be an evictable leaf",
        );
    }

    #[test]
    fn leaf_dirs_are_existing_debug_leaves_only() {
        let tmp = TempDir::new().unwrap();
        basic_repo(tmp.path());
        let leaves = build_cache_leaf_dirs(std::slice::from_ref(&tmp.path().to_path_buf()));

        for present in [
            "target/debug/incremental",
            "target/debug/deps",
            "target/debug/build",
        ] {
            let want = tmp.path().join(present).canonicalize().unwrap();
            assert!(leaves.contains(&want), "missing {present}: {leaves:?}");
        }
        // Neither `target/debug` nor the binary is ever a leaf.
        let debug = tmp.path().join("target/debug").canonicalize().unwrap();
        assert!(!leaves.contains(&debug));
    }

    #[test]
    fn leaf_dirs_reject_symlinked_leaf() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("target/debug/incremental")).unwrap();
        let elsewhere = tmp.path().join("evil");
        std::fs::create_dir_all(&elsewhere).unwrap();
        symlink(&elsewhere, tmp.path().join("target/debug/deps")).unwrap();

        let leaves = build_cache_leaf_dirs(std::slice::from_ref(&tmp.path().to_path_buf()));
        let evil = elsewhere.canonicalize().unwrap();
        assert!(
            !leaves.contains(&evil),
            "a symlinked leaf must never be admitted: {leaves:?}",
        );
    }

    #[test]
    fn candidates_are_stale_build_cache_without_estimate() {
        let tmp = TempDir::new().unwrap();
        basic_repo(tmp.path());
        let candidates = build_cache_candidates(std::slice::from_ref(&tmp.path().to_path_buf()));
        assert!(!candidates.is_empty());
        for c in &candidates {
            assert_eq!(c.kind, CandidateKind::StaleBuildCache);
            assert_eq!(c.est_bytes, None);
            assert!(c.reason.is_some());
            assert!(!c.path.ends_with("simard"));
        }
    }
}
