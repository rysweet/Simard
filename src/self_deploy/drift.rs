//! Deploy-drift detection: is the running daemon stale relative to merged `main`?
//!
//! See `docs/reference/self-deploy-api.md#deploydrift` and
//! `docs/concepts/reconcile-and-self-deploy.md`. The `git`/`Cargo.lock` reads
//! are injected through [`DeploySource`] so drift detection runs hermetically
//! with no network and no live repo.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// Deploy drift between the merged `main` tree and the running binary.
///
/// `needs_deploy` is the authoritative "is the running daemon stale?" signal.
/// It is reused verbatim by the deploy-aware done-gate (Workstream B) as the
/// "deployed-and-running" evidence for self-affecting goals.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeployDrift {
    /// Commits the running binary is behind `origin/main`. `0` when current.
    pub behind_commits: usize,
    /// Names of pinned deps whose merged rev differs from the running rev
    /// (e.g. `["amplihack-memory", "rustyclawd-core"]`). Empty when current.
    pub drifted_pins: Vec<String>,
    /// `behind_commits > 0 || !drifted_pins.is_empty()`.
    pub needs_deploy: bool,
}

impl DeployDrift {
    /// Construct from raw counts, deriving `needs_deploy` as the documented
    /// invariant: `behind_commits > 0 || !drifted_pins.is_empty()`.
    pub fn from_parts(behind_commits: usize, drifted_pins: Vec<String>) -> Self {
        let needs_deploy = behind_commits > 0 || !drifted_pins.is_empty();
        Self {
            behind_commits,
            drifted_pins,
            needs_deploy,
        }
    }

    /// A current (no-drift) deploy state — the running binary is at HEAD with
    /// every pin matching.
    pub fn current() -> Self {
        Self::from_parts(0, Vec::new())
    }
}

/// Source of the merged-vs-running facts the [`ReconcileDetector`] compares.
///
/// The `git`/`Cargo.lock` reads are injected so tests run hermetically with no
/// network and no live repo.
pub trait DeploySource: Send + Sync {
    /// Latest merged commit on the default branch of the owned repo.
    fn merged_head(&self) -> SimardResult<String>;
    /// Build commit embedded in the running binary.
    fn running_commit(&self) -> SimardResult<String>;
    /// Count of commits `running_commit..merged_head`.
    fn behind_count(&self) -> SimardResult<usize>;
    /// Pinned dep revs in the merged tree, keyed by crate name.
    fn merged_pins(&self) -> SimardResult<BTreeMap<String, String>>;
    /// Pinned dep revs compiled into the running binary, keyed by crate name.
    fn running_pins(&self) -> SimardResult<BTreeMap<String, String>>;
}

/// Computes [`DeployDrift`] once per OODA cycle from an injected [`DeploySource`].
pub struct ReconcileDetector<S: DeploySource> {
    source: S,
}

impl<S: DeploySource> ReconcileDetector<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }

    /// Returns [`DeployDrift`], or the underlying source error. Unlike
    /// [`detect`](Self::detect), this does **not** fail safe: a git/source error
    /// surfaces as `Err` so callers that must distinguish "positively no drift"
    /// from "could not determine the deploy state" never mistake an unknown
    /// state for a confirmed one.
    ///
    /// This is the fail-*closed* variant the closed-loop outcome-verification
    /// step (issue #2751) requires: it stakes Rail-3 on a `verified` live signal
    /// meaning *authenticated positive corroboration*, so a git probe error must
    /// become an explicit "unknown" rather than a spurious `needs_deploy: false`
    /// (which the caller would otherwise read as "confirmed running").
    pub fn try_detect(&self) -> SimardResult<DeployDrift> {
        let behind = self.source.behind_count()?;
        let merged = self.source.merged_pins()?;
        let running = self.source.running_pins()?;

        // A pin has drifted when its merged rev differs from the running rev
        // (including a pin present in the merged tree but absent from the
        // running binary). Sorted for deterministic ordering.
        let mut drifted: Vec<String> = merged
            .iter()
            .filter(|(name, merged_rev)| running.get(*name) != Some(*merged_rev))
            .map(|(name, _)| name.clone())
            .collect();
        drifted.sort();

        Ok(DeployDrift::from_parts(behind, drifted))
    }

    /// Returns [`DeployDrift`]. Never panics; on a source error returns a
    /// `needs_deploy: false` drift (fail-safe: a transient git failure must not
    /// spuriously trigger a deploy). Callers that must tell "no drift" apart
    /// from "unknown" (e.g. the outcome-verify Rail-3, #2751) use
    /// [`try_detect`](Self::try_detect) instead.
    pub fn detect(&self) -> DeployDrift {
        self.try_detect().unwrap_or_else(|_| DeployDrift::current())
    }
}

/// Production [`DeploySource`] backed by `git` in a source checkout and the
/// running binary's embedded build commit (`SIMARD_GIT_HASH`).
///
/// **First-increment scope:** the commit-drift dimension is fully wired (this is
/// the headline "running binary is hours behind merged `main`" signal). Pinned
/// dependency-rev drift is reported as **empty** for both merged and running
/// (so it never produces a *false* drift); wiring running pins from build
/// metadata is tracked as a follow-up. The detector's fail-safe contract means a
/// missing/!git checkout simply reports "no drift".
#[derive(Clone)]
pub struct GitDeploySource {
    /// Source checkout to run `git` in.
    repo_dir: PathBuf,
    /// Default-branch ref to compare against (e.g. `origin/main`).
    default_branch_ref: String,
    /// When `true` (operator/CLI path), [`merged_head`](Self::merged_head) falls
    /// back to local `HEAD` if the default-branch ref cannot be resolved (e.g. a
    /// shallow/detached checkout). On the AUTONOMOUS self-deploy path this MUST
    /// be `false` (issue #2590 SR-1): deploying an unverified local `HEAD` would
    /// bypass the branch-protection / signed-merge root of trust the docs promise
    /// (`docs/concepts/reconcile-and-self-deploy.md §Security prerequisites`), so
    /// an unresolved `origin/<default-branch>` must yield an error → no drift →
    /// no signal, never a `HEAD` deploy.
    head_fallback: bool,
}

impl Default for GitDeploySource {
    fn default() -> Self {
        Self {
            repo_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            default_branch_ref: "origin/main".to_string(),
            head_fallback: true,
        }
    }
}

impl GitDeploySource {
    /// A source rooted at the current working directory comparing to `origin/main`.
    pub fn new() -> Self {
        Self::default()
    }

    /// A source rooted at an explicit checkout (for tests / non-cwd installs).
    pub fn at(repo_dir: impl Into<PathBuf>) -> Self {
        Self {
            repo_dir: repo_dir.into(),
            ..Self::default()
        }
    }

    /// Disable the local-`HEAD` fallback in [`merged_head`](Self::merged_head).
    /// REQUIRED on the autonomous self-deploy path (#2590 SR-1) so an unresolved
    /// `origin/<default-branch>` degrades to "no drift" rather than deploying an
    /// unverified local `HEAD`. Keep the fallback (the default) only on the
    /// operator/CLI path, where a human is choosing to relaunch a local checkout.
    #[must_use]
    pub fn origin_strict(mut self) -> Self {
        self.head_fallback = false;
        self
    }

    fn git(&self, args: &[&str]) -> SimardResult<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo_dir)
            .args(args)
            .output()
            .map_err(|e| SimardError::GitCommandFailed {
                command: format!("git {}", args.join(" ")),
                reason: format!("spawn failed: {e}"),
            })?;
        if !out.status.success() {
            return Err(SimardError::GitCommandFailed {
                command: format!("git {}", args.join(" ")),
                reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

impl DeploySource for GitDeploySource {
    fn merged_head(&self) -> SimardResult<String> {
        // Prefer the tracked default branch. On the operator/CLI path, fall back
        // to local `HEAD` when the remote ref is absent (e.g. a shallow or
        // detached checkout). On the AUTONOMOUS path (`origin_strict`) that
        // fallback is DISABLED (#2590 SR-1): an unresolved `origin/<default>` is
        // returned as an error so OBSERVE yields no drift rather than deploying
        // an unverified local `HEAD` that never passed branch protection.
        let origin = self.git(&["rev-parse", &self.default_branch_ref]);
        if self.head_fallback {
            origin.or_else(|_| self.git(&["rev-parse", "HEAD"]))
        } else {
            origin
        }
    }

    fn running_commit(&self) -> SimardResult<String> {
        let commit = env!("SIMARD_GIT_HASH");
        if commit.is_empty() || commit == "unknown" {
            return Err(SimardError::GitCommandFailed {
                command: "SIMARD_GIT_HASH".to_string(),
                reason: "running binary has no embedded build commit".to_string(),
            });
        }
        Ok(commit.to_string())
    }

    fn behind_count(&self) -> SimardResult<usize> {
        let running = self.running_commit()?;
        let merged = self.merged_head()?;
        let range = format!("{running}..{merged}");
        let count = self.git(&["rev-list", "--count", &range])?;
        count
            .trim()
            .parse::<usize>()
            .map_err(|e| SimardError::GitCommandFailed {
                command: format!("git rev-list --count {range}"),
                reason: format!("unparseable count {count:?}: {e}"),
            })
    }

    fn merged_pins(&self) -> SimardResult<BTreeMap<String, String>> {
        // First-increment: commit-drift only (see type docs). Empty == no pin
        // drift, never a false positive.
        Ok(BTreeMap::new())
    }

    fn running_pins(&self) -> SimardResult<BTreeMap<String, String>> {
        Ok(BTreeMap::new())
    }
}

/// Convenience: the production reconcile detector over [`GitDeploySource`].
pub fn production_reconcile_detector() -> ReconcileDetector<GitDeploySource> {
    ReconcileDetector::new(GitDeploySource::new())
}

#[cfg(test)]
mod prod_source_tests {
    use super::*;

    #[test]
    fn git_source_constructs_with_defaults() {
        let s = GitDeploySource::new();
        assert_eq!(s.default_branch_ref, "origin/main");
    }

    #[test]
    fn running_commit_is_embedded_build_hash() {
        // In CI the build embeds a real SIMARD_GIT_HASH; assert it round-trips
        // (or that the documented error fires when unknown).
        let s = GitDeploySource::new();
        match s.running_commit() {
            Ok(c) => assert!(!c.is_empty()),
            Err(SimardError::GitCommandFailed { .. }) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn detect_on_nonexistent_repo_is_failsafe_no_deploy() {
        let detector = ReconcileDetector::new(GitDeploySource::at("/no-such-repo-xyz-123"));
        let drift = detector.detect();
        assert!(
            !drift.needs_deploy,
            "a missing checkout must fail safe (no spurious deploy)"
        );
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .status()
            .expect("git spawn")
            .success();
        assert!(ok, "git {args:?} failed");
    }

    /// SR-1 (#2590): with no `origin/main` ref, the operator/CLI source falls
    /// back to local `HEAD`, but an `origin_strict` (autonomous) source refuses
    /// — so OBSERVE yields no drift rather than deploying an unverified `HEAD`.
    #[test]
    fn origin_strict_merged_head_refuses_head_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q"]);
        std::fs::write(dir.join("f"), "x").unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "c0"]);

        // No remote → `origin/main` is unresolvable.
        let lenient = GitDeploySource::at(dir);
        let strict = GitDeploySource::at(dir).origin_strict();

        assert!(
            lenient.merged_head().is_ok(),
            "operator/CLI path falls back to local HEAD"
        );
        assert!(
            strict.merged_head().is_err(),
            "autonomous path must NOT fall back to local HEAD"
        );

        // And the strict source's detector therefore fails safe (no drift).
        let drift = ReconcileDetector::new(strict).detect();
        assert!(
            !drift.needs_deploy,
            "unresolved origin ref on the strict path must yield no drift"
        );
    }
}
