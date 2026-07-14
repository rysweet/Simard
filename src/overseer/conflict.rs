//! M3 — conflict resolution that **never** bypasses hooks (operator hard-gate
//! #8). Conflict-resolution pushes run pre-commit/pre-push hooks; `--no-verify`
//! is refused at the git-runner boundary, so no code path can smuggle it in.
//!
//! Reuse (design doc §capability table): `git_guardrails::check_git_safety`
//! (`src/git_guardrails.rs:41`) guards every git command; the resolver performs
//! a conservative rebase-onto-base + push, all through the guarded runner.

use std::path::{Path, PathBuf};

use crate::overseer::capabilities::OverseerError;
use crate::overseer::merge_ops::ConflictResolver;

/// Runs guarded git commands in a repo. Injectable so the resolver is unit-
/// tested without a real repository or remote. The real runner refuses
/// `--no-verify` and applies `git_guardrails::check_git_safety`.
pub trait GitRunner {
    fn run(&self, repo_dir: &Path, args: &[&str]) -> Result<(), OverseerError>;
}

/// Real git runner. Two always-on floors before any command executes:
/// 1. **Refuse `--no-verify`** — pre-commit/pre-push hooks MUST run (hard-gate #8).
/// 2. **`check_git_safety`** — the shipped destructive-command guardrail.
#[derive(Clone, Debug, Default)]
pub struct RealGitRunner;

impl GitRunner for RealGitRunner {
    fn run(&self, repo_dir: &Path, args: &[&str]) -> Result<(), OverseerError> {
        if args.contains(&"--no-verify") {
            return Err(OverseerError::Capability {
                what: "git",
                detail: "refusing --no-verify — conflict-resolution pushes must run hooks"
                    .to_string(),
            });
        }
        if args.first() == Some(&"push") {
            return Err(OverseerError::Capability {
                what: "git",
                detail:
                    "raw conflict-resolution push is disabled; a durable mutation guard is required"
                        .to_string(),
            });
        }
        crate::git_guardrails::check_git_safety(repo_dir, args).map_err(|e| {
            OverseerError::Capability {
                what: "git_guardrails",
                detail: e,
            }
        })?;
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .args(args)
            .status()
            .map_err(|e| OverseerError::Capability {
                what: "git",
                detail: e.to_string(),
            })?;
        if status.success() {
            Ok(())
        } else {
            Err(OverseerError::Capability {
                what: "git",
                detail: format!("git {} exited {status}", args.join(" ")),
            })
        }
    }
}

/// The [`ConflictResolver`] over a [`GitRunner`] seam. Resolves by rebasing the
/// PR branch onto the base and pushing — through hooks, never `--no-verify`.
pub struct GitConflictResolver {
    git: Box<dyn GitRunner>,
    repo_dir: PathBuf,
    base_ref: String,
}

impl GitConflictResolver {
    pub fn new(git: Box<dyn GitRunner>, repo_dir: PathBuf, base_ref: impl Into<String>) -> Self {
        Self {
            git,
            repo_dir,
            base_ref: base_ref.into(),
        }
    }

    /// Production resolver: real git runner, `main` base.
    pub fn from_env(repo_dir: PathBuf) -> Self {
        Self::new(Box::new(RealGitRunner), repo_dir, "main")
    }
}

impl ConflictResolver for GitConflictResolver {
    fn resolve(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
        let _ = (&self.git, &self.repo_dir, &self.base_ref);
        Err(OverseerError::Capability {
            what: "resolve_conflict",
            detail: "autonomous conflict resolution is disabled until its push uses the durable mutation guard"
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingGit {
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl GitRunner for RecordingGit {
        fn run(&self, _dir: &Path, args: &[&str]) -> Result<(), OverseerError> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|s| s.to_string()).collect());
            Ok(())
        }
    }

    #[test]
    fn resolver_refuses_before_any_local_mutation() {
        let git = std::sync::Arc::new(RecordingGit::default());
        let resolver = GitConflictResolver::new(
            Box::new(GitRef(git.clone())),
            PathBuf::from("/repo"),
            "main",
        );
        resolver.resolve("rysweet/Simard", 7).unwrap_err();

        let calls = git.calls.lock().unwrap();
        assert!(calls.is_empty(), "refusal must precede fetch/merge/push");
    }

    struct GitRef(std::sync::Arc<RecordingGit>);
    impl GitRunner for GitRef {
        fn run(&self, dir: &Path, args: &[&str]) -> Result<(), OverseerError> {
            self.0.run(dir, args)
        }
    }

    #[test]
    fn real_runner_refuses_no_verify_before_running_git() {
        // The real runner refuses --no-verify without shelling out to git, so
        // the hard gate holds even if a caller tries to smuggle it in.
        let err = RealGitRunner
            .run(Path::new("/nonexistent-repo"), &["push", "--no-verify"])
            .unwrap_err();
        assert!(format!("{err}").contains("--no-verify"));
    }
}
