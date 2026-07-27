//! Outside-in integration tests for the issue #4825 disk-reclaim churn fix.
//!
//! These drive the **real production entry** the OODA daemon's Tier-3
//! coordination calls — [`simard::disk_reclaim::prune_build_tree_artifacts`] —
//! against a real on-disk build tree, exercising the actual hardened
//! `RealPathRemover` (real `rm`), `du -sb` measurer, and `/proc` live-PID probe.
//! No test doubles: this is the consumer boundary a running daemon hits.
//!
//! Root cause being validated: the regenerable Cargo build artifacts
//! (`target/debug`, `target/llvm-cov-target`) sit under the protected daemon
//! working dir, so routine reclaim used to Reject them as "skipped for review"
//! and only the ≥95% emergency net removed them — after `/home` had oscillated
//! to 99%. This proactive prune reclaims them at the routine threshold instead.

use std::fs;
use std::path::Path;

use simard::disk_reclaim::{ReclaimMode, prune_build_tree_artifacts};

/// Create `<build_tree>/<rel>` with one non-empty file so `du` reports > 0 bytes.
fn make_artifact(build_tree: &Path, rel: &str) {
    let dir = build_tree.join(rel);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("blob.bin"), vec![0u8; 4096]).unwrap();
}

/// Scenario 1 (simple): the most basic user-visible behavior — an apply-mode
/// prune actually removes the regenerable `target/debug` from the build tree and
/// reports the freed bytes. This is what stops `/home` from re-climbing to 95%.
#[test]
fn prune_apply_actually_removes_target_debug() {
    let build_tree = tempfile::tempdir().unwrap();
    make_artifact(build_tree.path(), "target/debug");
    let debug = build_tree.path().join("target/debug");
    assert!(debug.is_dir(), "precondition: target/debug exists");

    let report = prune_build_tree_artifacts(build_tree.path(), ReclaimMode::Apply);

    assert!(
        !debug.exists(),
        "target/debug must be gone after apply prune"
    );
    assert_eq!(report.removed.len(), 1, "exactly the one present artifact");
    assert_eq!(report.removed[0].path, debug);
    assert!(report.bytes_freed > 0, "freed bytes must be measured (> 0)");
    assert!(report.pruned_any());
    assert!(report.failures.is_empty());
}

/// Scenario 2 (complex / edge + integration): a full build tree.
///   - dry-run must delete NOTHING (safe default),
///   - a subsequent apply removes BOTH regenerable artifacts,
///   - a non-artifact sibling (`target/release`) is NEVER touched — the
///     carve-out is a closed allow-list, not "everything under target/".
#[test]
fn prune_dry_run_is_safe_then_apply_removes_only_regenerable_artifacts() {
    let build_tree = tempfile::tempdir().unwrap();
    make_artifact(build_tree.path(), "target/debug");
    make_artifact(build_tree.path(), "target/llvm-cov-target");
    // A sibling that must survive: release artifacts are NOT on the allow-list.
    make_artifact(build_tree.path(), "target/release");

    let debug = build_tree.path().join("target/debug");
    let cov = build_tree.path().join("target/llvm-cov-target");
    let release = build_tree.path().join("target/release");

    // --- dry-run: zero destructive ops -----------------------------------
    let dry = prune_build_tree_artifacts(build_tree.path(), ReclaimMode::DryRun);
    assert!(debug.is_dir() && cov.is_dir(), "dry-run must not delete");
    assert_eq!(dry.would_remove.len(), 2, "dry-run reports both intents");
    assert!(dry.removed.is_empty());
    assert_eq!(dry.bytes_freed, 0);
    assert!(!dry.pruned_any());

    // --- apply: remove exactly the two regenerable artifacts -------------
    let apply = prune_build_tree_artifacts(build_tree.path(), ReclaimMode::Apply);
    assert!(!debug.exists(), "target/debug removed");
    assert!(!cov.exists(), "target/llvm-cov-target removed");
    assert!(
        release.is_dir(),
        "target/release must be preserved (not a regenerable-artifact allow-list entry)",
    );
    assert_eq!(apply.removed.len(), 2);
    assert!(apply.bytes_freed > 0);
    assert!(apply.failures.is_empty());
}
