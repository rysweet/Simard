//! Cwd-independent self-deploy source preparation (issue #2467).
//!
//! `simard self-deploy` must build and deploy the **merged** `origin/main`
//! commit — the SHA `--check` reports — from *any* working directory, not
//! whatever the cwd checkout happens to be on. This module owns:
//!
//! 1. **Canonical repo resolution** (cwd-independent), with precedence
//!    `SIMARD_SELF_DEPLOY_REPO` → a persistent checkout at
//!    [`self_deploy_src_dir`] → clone-from-origin. The build source is *never*
//!    the cwd.
//! 2. **Fetch + checkout** of the target merged commit before building
//!    ([`SelfDeploySourcePreparer::prepare`]): validate the target is a full
//!    40-hex SHA, `git fetch origin` **only when the commit is not already
//!    present locally** (the `self-deploy` CLI fetches the same repo to read the
//!    merged head, so the object is usually already in the object store — the
//!    second network round-trip is skipped), then `git checkout --detach <sha>`
//!    so the embedded `SIMARD_GIT_HASH` equals the deployed commit.
//! 3. **Warm target dir** ([`self_deploy_target_dir`]): a persistent cargo
//!    target under the state root so self-deploys are incremental, not cold.
//!
//! Failure is **loud**: if the merged commit cannot be made available, the
//! preparer returns an error ([`SafeUpdateError::SourceResolveFailed`] /
//! [`FetchFailed`](SafeUpdateError::FetchFailed) /
//! [`CheckoutFailed`](SafeUpdateError::CheckoutFailed)) and the caller aborts
//! *before* the load-bearing safety sequence — it never silently falls back to
//! building the cwd HEAD.
//!
//! All git is run via the `env_clear()` + selective `PATH`/`HOME` re-injection
//! discipline (mirroring [`crate::engineer_worktree`]) so a hostile ambient env
//! cannot hijack the build source.
//!
//! See `docs/reference/self-deploy-source-prep.md` and
//! `docs/howto/run-self-deploy-from-any-directory.md`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::safe_update::SafeUpdateError;
use crate::state_root::simard_state_root;

/// Environment variable that pins the canonical self-deploy source repo,
/// overriding the persistent-checkout / clone-from-origin resolution.
const SELF_DEPLOY_REPO_ENV: &str = "SIMARD_SELF_DEPLOY_REPO";

/// Subdirectory name (under the resolved state root) for the persistent,
/// cwd-independent self-deploy source checkout.
pub const SELF_DEPLOY_SRC_DIRNAME: &str = "self-deploy-src";

/// Subdirectory name (under the resolved state root) for the persistent **warm**
/// cargo target dir reused across self-deploys.
pub const SELF_DEPLOY_TARGET_DIRNAME: &str = "self-deploy-target";

/// Persistent self-deploy **source checkout** directory.
///
/// Resolves to `<simard_state_root()>/self-deploy-src` (honoring
/// `SIMARD_STATE_ROOT`). Stable across runs and across cwds — never under
/// `temp_dir()` and never keyed by PID — so the canonical checkout survives and
/// the warm target stays warm.
pub fn self_deploy_src_dir() -> PathBuf {
    simard_state_root().join(SELF_DEPLOY_SRC_DIRNAME)
}

/// Persistent **warm** cargo target directory for self-deploy builds.
///
/// Resolves to `<simard_state_root()>/self-deploy-target` (honoring
/// `SIMARD_STATE_ROOT`). Persistent and reused across runs so `cargo build` is
/// incremental (~2–3 min) instead of a cold from-scratch compile (~10+ min).
/// Deliberately outside `temp_dir()` and not PID-keyed, and named so it does
/// **not** match any `cmd_cleanup` disk reaper pattern (which scan
/// `/tmp/simard-*` and `/tmp/simard-*-target`).
pub fn self_deploy_target_dir() -> PathBuf {
    simard_state_root().join(SELF_DEPLOY_TARGET_DIRNAME)
}

/// Validate that `sha` is a full 40-character lowercase hex Git object name.
///
/// This is a security control (SEC-I2): the SHA is interpolated into `git`
/// argument vectors, so a value with a leading `-` could be parsed as an option
/// (e.g. `--upload-pack=…`). Rejects anything that is not exactly
/// `^[0-9a-f]{40}$` — wrong length, uppercase, non-hex, a leading `-`, empty,
/// or whitespace-bearing.
pub fn validate_full_sha(sha: &str) -> Result<(), SafeUpdateError> {
    let is_full_hex =
        sha.len() == 40 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if is_full_hex {
        Ok(())
    } else {
        Err(SafeUpdateError::CheckoutFailed {
            detail: format!(
                "refusing target commit {sha:?}: not a full 40-char lowercase hex SHA \
                 (guards against argv option injection into git)"
            ),
        })
    }
}

/// Validate that an origin remote URL uses an allowed transport before it is
/// ever used to clone (SEC-I3).
///
/// Allows only `https://`, `ssh://`, and the scp-like `user@host:path` SSH
/// form. Rejects arbitrary-command transports (`ext::…`, `fd::…`) and any
/// remote-helper transport that can execute a command, which would let a
/// poisoned remote URL run code during a clone/fetch.
pub fn validate_origin_transport(url: &str) -> Result<(), SafeUpdateError> {
    let trimmed = url.trim();
    let allowed =
        trimmed.starts_with("https://") || trimmed.starts_with("ssh://") || is_scp_like(trimmed);
    if allowed {
        Ok(())
    } else {
        Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "refusing origin URL {url:?}: only https:// and ssh:// transports are permitted \
                 (arbitrary-command transports like ext::/fd:: can run code on clone)"
            ),
        })
    }
}

/// Whether `url` is an scp-like SSH remote (`user@host:path`) and *not* a
/// remote-helper transport. Rejects any `scheme::address` form (e.g. `ext::`,
/// `fd::`) — a `::` is the giveaway of a remote helper that can run a command.
fn is_scp_like(url: &str) -> bool {
    if url.contains("://") || url.contains("::") {
        return false;
    }
    match (url.find('@'), url.find(':')) {
        (Some(at), Some(colon)) => at > 0 && at < colon && colon + 1 < url.len(),
        _ => false,
    }
}

/// Seam that makes the Simard source available at a target merged commit,
/// independent of the current working directory.
///
/// Production wires this to [`GitSourcePreparer`] (real `git`); tests inject a
/// fake to drive the orchestrator and the [`prepare_and_build`] composition
/// hermetically.
pub trait SelfDeploySourcePreparer: Send + Sync {
    /// Resolve the canonical source repo, ensure `target_commit` is present
    /// locally (fetching from `origin` **only when the object is missing**), and
    /// `git checkout --detach <target_commit>`, returning the prepared repo
    /// directory (whose HEAD is now `target_commit`).
    ///
    /// **Loud-fail contract:** on any failure this returns an `Err` and must
    /// **never** return a path to the cwd checkout as a fallback — building the
    /// cwd HEAD instead of the merged commit is exactly the bug #2467 fixes.
    fn prepare(&self, target_commit: &str) -> Result<PathBuf, SafeUpdateError>;
}

/// Production [`SelfDeploySourcePreparer`] backed by real `git`.
///
/// Resolves the canonical repo with precedence `SIMARD_SELF_DEPLOY_REPO` →
/// persistent [`self_deploy_src_dir`] → clone-from-origin, then fetches and
/// checks out the target commit detached.
#[derive(Debug, Default)]
pub struct GitSourcePreparer {
    /// Explicit repo override (tests / non-standard installs). When set it wins
    /// over the env/persistent/clone precedence, mirroring
    /// [`crate::self_deploy::GitDeploySource::at`].
    repo_override: Option<PathBuf>,
}

impl GitSourcePreparer {
    /// A preparer using the documented `SIMARD_SELF_DEPLOY_REPO` → persistent →
    /// clone precedence.
    pub fn new() -> Self {
        Self::default()
    }

    /// A preparer rooted at an explicit checkout, bypassing env/clone
    /// precedence. Mirrors [`crate::self_deploy::GitDeploySource::at`].
    pub fn at(repo_dir: impl Into<PathBuf>) -> Self {
        Self {
            repo_override: Some(repo_dir.into()),
        }
    }

    /// Resolve (and if necessary validate) the canonical source repo directory.
    ///
    /// Returns [`SafeUpdateError::SourceResolveFailed`] on a bad
    /// `SIMARD_SELF_DEPLOY_REPO` (path traversal, missing, or not a git work
    /// tree) or when no repo can be made available — never the cwd.
    pub fn resolve_repo(&self) -> Result<PathBuf, SafeUpdateError> {
        // 1) Explicit override (tests / non-standard installs) wins outright.
        if let Some(repo) = &self.repo_override {
            return validate_repo_path(repo);
        }
        // 2) `SIMARD_SELF_DEPLOY_REPO` env override.
        if let Some(env_repo) = env_repo_override() {
            return validate_repo_path(&env_repo);
        }
        // 3) Persistent canonical checkout under the state root, if present.
        let persistent = self_deploy_src_dir();
        if is_git_work_tree(&persistent) {
            return Ok(persistent);
        }
        // 4) Clone from the origin discovered via the current checkout. This is
        //    the only branch that consults the cwd, and only to read its
        //    `origin` URL — the build still happens in the cloned `persistent`
        //    checkout, never the cwd.
        clone_from_origin(&persistent)
    }

    /// `git fetch origin` in the resolved/owned source repo so its
    /// remote-tracking refs (and thus the merged head) are current.
    ///
    /// The first dedicated git-fetch helper in `src/` (issue #2467). Called by
    /// the `self-deploy` CLI (which fetches before reading the merged head to
    /// deploy) and by [`prepare`](SelfDeploySourcePreparer::prepare) — the latter
    /// **only when the target commit is not already present locally**, so the
    /// CLI's fetch is not redundantly repeated. Loud on failure: returns
    /// [`SafeUpdateError::FetchFailed`], never a silent fallback.
    pub fn fetch_origin(&self, repo: &Path) -> Result<(), SafeUpdateError> {
        git_capture(repo, &["fetch", "origin"])
            .map(|_| ())
            .map_err(|detail| SafeUpdateError::FetchFailed { detail })
    }
}

impl SelfDeploySourcePreparer for GitSourcePreparer {
    fn prepare(&self, target_commit: &str) -> Result<PathBuf, SafeUpdateError> {
        // Validate the SHA BEFORE any git call so an option-injection attempt
        // (leading `-`) can never reach a git argv (SEC-I2).
        validate_full_sha(target_commit)?;

        // Resolve the canonical repo (never the cwd), make sure the exact merged
        // commit is in the object store, then check it out detached so the
        // embedded `SIMARD_GIT_HASH` equals the deployed commit.
        let repo = self.resolve_repo()?;
        // Perf (issue #2467): the merged commit is almost always already present
        // — the `self-deploy` CLI `git fetch`es this same canonical repo to read
        // the merged head before handing it to the orchestrator. Re-fetching it
        // here is a redundant network round-trip, so skip it when the object is
        // already in the store (a fast, offline `cat-file` check) and only hit
        // the network when the commit is genuinely missing. `checkout --detach`
        // is pinned to the validated full SHA, so a skipped fetch can never check
        // out a different/stale tree — and the deploy survives a transient
        // network blip after the head was read.
        if !commit_present(&repo, target_commit) {
            self.fetch_origin(&repo)?;
        }
        git_capture(&repo, &["checkout", "--detach", target_commit])
            .map(|_| ())
            .map_err(|detail| SafeUpdateError::CheckoutFailed { detail })?;
        Ok(repo)
    }
}

/// Validate a candidate source-repo path: reject `..` traversal, a relative
/// path, a symlink, and any path that is not a git work tree (SEC). Never
/// resolves to the cwd.
fn validate_repo_path(repo: &Path) -> Result<PathBuf, SafeUpdateError> {
    if repo.components().any(|c| c == Component::ParentDir) {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "self-deploy source repo path must not contain '..': {}",
                repo.display()
            ),
        });
    }
    if !repo.is_absolute() {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "self-deploy source repo path must be absolute: {}",
                repo.display()
            ),
        });
    }
    if repo
        .symlink_metadata()
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "self-deploy source repo path must not be a symlink: {}",
                repo.display()
            ),
        });
    }
    if !is_git_work_tree(repo) {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "self-deploy source repo is not a git work tree: {}",
                repo.display()
            ),
        });
    }
    Ok(repo.to_path_buf())
}

/// `SIMARD_SELF_DEPLOY_REPO`, as a path, when set and non-empty.
fn env_repo_override() -> Option<PathBuf> {
    let raw = std::env::var_os(SELF_DEPLOY_REPO_ENV)?;
    let s = raw.to_string_lossy();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Whether `repo` is the root of a git work tree.
fn is_git_work_tree(repo: &Path) -> bool {
    if !repo.is_dir() {
        return false;
    }
    matches!(
        git_capture(repo, &["rev-parse", "--is-inside-work-tree"]),
        Ok(out) if out.trim() == "true"
    )
}

/// Clone the canonical source from the cwd's `origin` URL into `dest`.
///
/// Only consulted when no override and no persistent checkout exist. The origin
/// URL is transport-validated (SEC-I3) before it is ever handed to `git clone`.
fn clone_from_origin(dest: &Path) -> Result<PathBuf, SafeUpdateError> {
    let origin_url = discover_origin_url()?;
    validate_origin_transport(&origin_url)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SafeUpdateError::SourceResolveFailed {
            detail: format!("cannot create state dir {}: {e}", parent.display()),
        })?;
    }

    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--quiet").arg(&origin_url).arg(dest);
    scrub_git_env(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| SafeUpdateError::SourceResolveFailed {
            detail: format!("cannot spawn git clone: {e}"),
        })?;
    if !out.status.success() {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "git clone of {origin_url:?} into {} failed: {}",
                dest.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    Ok(dest.to_path_buf())
}

/// Discover the `origin` remote URL of the current checkout (read-only).
fn discover_origin_url() -> Result<String, SafeUpdateError> {
    let cwd = std::env::current_dir().map_err(|e| SafeUpdateError::SourceResolveFailed {
        detail: format!("cannot resolve cwd to discover the origin URL: {e}"),
    })?;
    git_capture(&cwd, &["remote", "get-url", "origin"])
        .map(|s| s.trim().to_string())
        .map_err(|detail| SafeUpdateError::SourceResolveFailed {
            detail: format!(
                "cannot discover an origin URL from {}: {detail}",
                cwd.display()
            ),
        })
}

/// `env_clear()` + selective `PATH`/`HOME` re-injection for every `git`
/// subprocess, mirroring [`crate::engineer_worktree`]: a hostile ambient env
/// (`GIT_DIR`, `GIT_WORK_TREE`, `LD_PRELOAD`, …) cannot hijack the build source.
fn scrub_git_env(cmd: &mut Command) {
    cmd.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
}

/// Whether the commit `sha` is already present in `repo`'s object store — a
/// fast, **offline** check (`git cat-file -e <sha>^{commit}`) used to skip a
/// redundant network `git fetch` when the merged commit was already fetched
/// (e.g. by the `self-deploy` CLI before it read the merged head). `sha` must
/// already have passed [`validate_full_sha`]; the `^{commit}` peel ensures the
/// object resolves to a commit so the subsequent `checkout --detach` succeeds
/// without the network. A missing object (or any git error) returns `false`,
/// so the caller falls back to fetching — never a false "present".
fn commit_present(repo: &Path, sha: &str) -> bool {
    git_capture(repo, &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
}

/// Run a `git` subcommand in `repo` (scrubbed env) and return stdout, or an
/// `Err(String)` describing the failure for the caller to wrap in the right
/// loud [`SafeUpdateError`] variant.
fn git_capture(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo);
    scrub_git_env(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("spawn git {args:?} in {}: {e}", repo.display()))?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed in {}: {}",
            args,
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Compose source preparation with the warm-target build: the exact step the
/// self-deploy orchestrator's `build_candidate` performs as step 1 of the
/// load-bearing sequence (issue #2467).
///
/// Prepares the source at `target_commit` (fetch + detached checkout of the
/// merged head) and then builds it into the persistent warm `target_dir`. A
/// preparation failure propagates **before** any build is attempted — and a
/// fortiori before any daemon mutation — so a missing/unreachable merged commit
/// can never deploy the cwd HEAD.
pub(crate) fn prepare_and_build(
    source: &dyn SelfDeploySourcePreparer,
    target_commit: &str,
    target_dir: &Path,
) -> Result<PathBuf, SafeUpdateError> {
    let repo = source.prepare(target_commit)?;
    crate::self_relaunch::build_self_deploy_candidate(&repo, target_dir).map_err(|e| {
        SafeUpdateError::BuildFailed {
            detail: e.to_string(),
        }
    })
}
