//! Recipe-step confinement helpers + the post-run reconciliation diff.
//!
//! The "agent proposes, Rust disposes" guarantee is only sound if the analysis
//! agent genuinely **cannot** delete anything itself. Two first-class defenses
//! back the prompt-level ban:
//!
//! 1. [`scrub_analysis_path`] strips any `PATH` entry that exposes a mutating
//!    binary (`rm`, `rmdir`, `find`, `truncate`, …) so the agent cannot invoke
//!    one even if the prompt guard is bypassed.
//! 2. [`reconcile`] compares a cheap pre/post inventory of the managed roots and
//!    flags any disappearance the executor did **not** perform as a confinement
//!    breach — which refuses apply mode until investigated.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Binaries that can mutate the filesystem and must not be reachable from the
/// analysis step's `PATH`.
pub const MUTATING_BINARIES: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "truncate", "find", "mv", "dd",
];

/// Return a filtered copy of a `:`-separated `PATH` that excludes every
/// directory exposing any [`MUTATING_BINARIES`] entry, leaving read-only
/// inspection tools intact.
pub fn scrub_analysis_path(original: &str) -> String {
    original
        .split(':')
        .filter(|dir| !dir.is_empty() && !dir_exposes_mutator(Path::new(dir)))
        .collect::<Vec<_>>()
        .join(":")
}

/// `true` iff `dir` contains any mutating binary as an existing entry.
pub fn dir_exposes_mutator(dir: &Path) -> bool {
    MUTATING_BINARIES.iter().any(|bin| dir.join(bin).exists())
}

/// A detected confinement breach: paths that disappeared during the analysis
/// step that the guarded executor did **not** remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinementBreach {
    pub disappeared: Vec<PathBuf>,
}

/// Compare a pre/post inventory of the managed roots. Anything present in `pre`
/// but absent from `post` that is **not** in `executor_removed` is a breach: the
/// analysis step deleted something out of band.
pub fn reconcile(
    pre: &BTreeSet<PathBuf>,
    post: &BTreeSet<PathBuf>,
    executor_removed: &BTreeSet<PathBuf>,
) -> Result<(), ConfinementBreach> {
    let disappeared: Vec<PathBuf> = pre
        .difference(post)
        .filter(|p| !executor_removed.contains(*p))
        .cloned()
        .collect();
    if disappeared.is_empty() {
        Ok(())
    } else {
        Err(ConfinementBreach { disappeared })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn scrub_removes_dirs_exposing_mutating_binaries() {
        let bad = TempDir::new().unwrap();
        std::fs::write(bad.path().join("rm"), b"#!/bin/sh\n").unwrap();
        let good = TempDir::new().unwrap();
        std::fs::write(good.path().join("df"), b"#!/bin/sh\n").unwrap();

        let original = format!("{}:{}", bad.path().display(), good.path().display());
        let scrubbed = scrub_analysis_path(&original);

        assert!(
            !scrubbed.contains(&bad.path().display().to_string()),
            "the dir exposing `rm` must be scrubbed from PATH: {scrubbed}",
        );
        assert!(
            scrubbed.contains(&good.path().display().to_string()),
            "the read-only-tools dir must be retained: {scrubbed}",
        );
    }

    #[test]
    fn dir_exposes_mutator_detects_find_and_truncate() {
        let dir = TempDir::new().unwrap();
        assert!(!dir_exposes_mutator(dir.path()));
        std::fs::write(dir.path().join("truncate"), b"x").unwrap();
        assert!(dir_exposes_mutator(dir.path()));
    }

    #[test]
    fn reconcile_ok_when_nothing_disappeared() {
        let pre = set(&["/r/a", "/r/b"]);
        let post = set(&["/r/a", "/r/b"]);
        assert!(reconcile(&pre, &post, &BTreeSet::new()).is_ok());
    }

    #[test]
    fn reconcile_ok_when_disappearance_was_the_executor() {
        let pre = set(&["/r/a", "/r/b"]);
        let post = set(&["/r/a"]);
        let removed = set(&["/r/b"]);
        assert!(
            reconcile(&pre, &post, &removed).is_ok(),
            "an executor-performed removal is authorized, not a breach",
        );
    }

    #[test]
    fn reconcile_flags_out_of_band_disappearance_as_breach() {
        let pre = set(&["/r/a", "/r/b"]);
        let post = set(&["/r/a"]);
        // /r/b vanished but the executor removed NOTHING → breach.
        let breach = reconcile(&pre, &post, &BTreeSet::new())
            .expect_err("an unexplained disappearance must be a confinement breach");
        assert_eq!(breach.disappeared, vec![PathBuf::from("/r/b")]);
    }
}
