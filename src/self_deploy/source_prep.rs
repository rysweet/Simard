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
//! All git is run via the `env_clear()` + selective `PATH`/`HOME`/`SSH_AUTH_SOCK`
//! re-injection discipline (mirroring [`crate::engineer_worktree`]) so a hostile
//! ambient env cannot hijack the build source.
//!
//! See `docs/reference/self-deploy-source-prep.md` and
//! `docs/howto/run-self-deploy-from-any-directory.md`.

use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::build_lock::BuildLock;
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

/// Max wait for the host-wide build lock before a self-deploy aborts loudly
/// (issue #2467). Long enough for a concurrent *warm* self-deploy build
/// (~2–3 min) to finish and release the lock; a lock still held past this
/// surfaces a loud [`SafeUpdateError::BuildFailed`] instead of racing the shared
/// source checkout.
const BUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(900);

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
    // A leading '-' would let git parse the URL as an *option* rather than a
    // positional (e.g. `--upload-pack=…`, `-c core.pager=…`) — the same argv
    // option-injection class `validate_full_sha` guards against (SEC-I2). The
    // origin URL comes from the cwd's `git remote get-url origin`, the very
    // untrusted input this control exists for, so reject it before the
    // transport check (and `clone_from_origin` additionally passes `--`).
    if trimmed.starts_with('-') {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: redact_credentials(&format!(
                "refusing origin URL {url:?}: must not begin with '-' \
                 (guards against argv option injection into git clone)"
            )),
        });
    }
    let allowed =
        trimmed.starts_with("https://") || trimmed.starts_with("ssh://") || is_scp_like(trimmed);
    if allowed {
        Ok(())
    } else {
        Err(SafeUpdateError::SourceResolveFailed {
            detail: redact_credentials(&format!(
                "refusing origin URL {url:?}: only https:// and ssh:// transports are permitted \
                 (arbitrary-command transports like ext::/fd:: can run code on clone)"
            )),
        })
    }
}

/// Redact URL userinfo (e.g. an embedded access token) from any text before it
/// is surfaced in an error, log, recipe output, or PR (SEC-D2).
///
/// Git error output and remote URLs can embed credentials as
/// `scheme://x-access-token:<TOKEN>@host/…` or `scheme://user:pass@host/…` (the
/// project uses token-bearing remotes). This replaces the userinfo between
/// `://` and the authority-terminating `@` with `***`, so a surfaced
/// [`SafeUpdateError`] (and the operator terminal / logs that display it) never
/// carries a live token. Tokenless URLs and non-URL text pass through
/// unchanged. Multiple URLs in one string are each redacted.
pub(crate) fn redact_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(scheme_pos) = rest.find("://") {
        let after_scheme = scheme_pos + 3;
        out.push_str(&rest[..after_scheme]);
        let authority = &rest[after_scheme..];
        // The authority component ends at the first path/query/fragment
        // delimiter, quote, or whitespace.
        let auth_end = authority
            .find(|c: char| matches!(c, '/' | '?' | '#' | '\'' | '"') || c.is_whitespace())
            .unwrap_or(authority.len());
        let authority_component = &authority[..auth_end];
        // The last '@' in the authority separates userinfo from host (a host
        // never contains '@'); redact everything before it.
        if let Some(at) = authority_component.rfind('@') {
            out.push_str("***");
            out.push_str(&authority_component[at..]);
        } else {
            out.push_str(authority_component);
        }
        rest = &authority[auth_end..];
    }
    out.push_str(rest);
    out
}

/// Whether `url` is an scp-like SSH remote (`user@host:path`) and *not* a
/// remote-helper transport. Rejects any `scheme::address` form (e.g. `ext::`,
/// `fd::`) — a `::` is the giveaway of a remote helper that can run a command.
fn is_scp_like(url: &str) -> bool {
    // A leading '-' is never a valid host and would be parsed as a `git clone`
    // option (argv option injection); reject it here too so the helper is safe
    // independent of its callers.
    if url.starts_with('-') || url.contains("://") || url.contains("::") {
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

    /// Resolve the canonical source repo **without cloning** — the read-only
    /// resolution used by `self-deploy --check`.
    ///
    /// Mirrors the cwd-independent precedence of [`resolve_repo`](Self::resolve_repo)
    /// (`repo_override` → `SIMARD_SELF_DEPLOY_REPO` → the persistent
    /// [`self_deploy_src_dir`] checkout) but deliberately stops short of the
    /// clone-from-origin step: `--check` documents that it "makes no changes",
    /// so it must never create the persistent checkout as a side effect.
    ///
    /// Returns `None` when no canonical source exists yet (e.g. before the first
    /// deploy), and the caller falls back to a best-effort report against the
    /// current working directory.
    ///
    /// An override that is *present but invalid* (path traversal / symlink /
    /// non-work-tree) also yields `None` — a read-only check tolerates it by
    /// degrading to the cwd report rather than hard-erroring like the effectful
    /// [`resolve_repo`](Self::resolve_repo) — but the degradation is **never
    /// silent**: it is logged loudly to stderr (mirroring the best-effort-fetch
    /// warning in `report_drift`) so an operator who never runs the deploy path
    /// still sees their misconfiguration.
    pub fn resolve_existing_repo(&self) -> Option<PathBuf> {
        // 1) Explicit override (tests / non-standard installs) wins outright.
        if let Some(repo) = &self.repo_override {
            return validated_existing_override(repo, "repo_override");
        }
        // 2) `SIMARD_SELF_DEPLOY_REPO` env override.
        if let Some(env_repo) = env_repo_override() {
            return validated_existing_override(&env_repo, SELF_DEPLOY_REPO_ENV);
        }
        // 3) Persistent canonical checkout under the state root, if present.
        //    No clone fallback: `--check` must not mutate anything.
        let persistent = self_deploy_src_dir();
        if is_git_work_tree(&persistent) {
            return Some(persistent);
        }
        None
    }
}

/// Validate a `--check` source override, warning **loudly** (never silently
/// degrading) when it is rejected.
///
/// A present-but-invalid override is an operator misconfiguration the read-only
/// check must still surface — so it logs to stderr before returning `None`, the
/// same "make the degraded path visible" remedy applied to the best-effort fetch
/// in `report_drift`. It still tolerates the failure (falls back to a cwd report)
/// rather than erroring, which the effectful deploy path
/// ([`GitSourcePreparer::resolve_repo`]) does instead.
fn validated_existing_override(repo: &Path, source: &str) -> Option<PathBuf> {
    match validate_repo_path(repo) {
        Ok(path) => Some(path),
        Err(err) => {
            eprintln!(
                "self-deploy --check: warning: ignoring invalid self-deploy source \
                 override from {source} ({err}); falling back to a current-directory \
                 report, which may not match what an actual deploy would build"
            );
            None
        }
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

    // Self-heal a wedged persistent checkout (issue #2467 review). A clone is
    // only reached when `dest` is NOT a valid git work tree (`resolve_repo`
    // branch 3 returns early when it is), so any path here is stale: a clone
    // killed mid-way, a leftover non-git directory, or a dangling symlink. Left
    // in place it would make `git clone <dest>` fail forever ("destination path
    // already exists and is not an empty directory"), permanently wedging every
    // future self-deploy. Remove it so the clone can recreate a clean checkout.
    remove_stale_checkout(dest)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| SafeUpdateError::SourceResolveFailed {
            detail: format!("cannot create state dir {}: {e}", parent.display()),
        })?;
    }

    let mut cmd = Command::new("git");
    // `--` terminates options so a (validated, but defense-in-depth) origin URL
    // can never be parsed as a `git clone` flag (SEC-I3).
    cmd.arg("clone")
        .arg("--quiet")
        .arg("--")
        .arg(&origin_url)
        .arg(dest);
    scrub_git_env(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| SafeUpdateError::SourceResolveFailed {
            detail: format!("cannot spawn git clone: {e}"),
        })?;
    if !out.status.success() {
        return Err(SafeUpdateError::SourceResolveFailed {
            detail: redact_credentials(&format!(
                "git clone of {origin_url:?} into {} failed: {}",
                dest.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        });
    }
    Ok(dest.to_path_buf())
}

/// Remove a stale/partial path at `dest` (dangling symlink, leftover file, or
/// non-git directory) so a fresh `git clone` can recreate it. Only called from
/// [`clone_from_origin`], which is reached only when `dest` is not a valid git
/// work tree, so deleting it can never discard a usable checkout. A symlink is
/// removed itself (never followed into its target). A non-existent path is a
/// no-op.
pub(crate) fn remove_stale_checkout(dest: &Path) -> Result<(), SafeUpdateError> {
    let meta = match dest.symlink_metadata() {
        Ok(m) => m,
        // Nothing present (or unreadable) — let the clone proceed and report.
        Err(_) => return Ok(()),
    };
    let ft = meta.file_type();
    let result = if ft.is_symlink() || ft.is_file() {
        std::fs::remove_file(dest)
    } else {
        std::fs::remove_dir_all(dest)
    };
    result.map_err(|e| SafeUpdateError::SourceResolveFailed {
        detail: format!(
            "cannot remove stale self-deploy source checkout {}: {e}",
            dest.display()
        ),
    })
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

/// `env_clear()` + selective `PATH`/`HOME`/`SSH_AUTH_SOCK` re-injection for every
/// `git` subprocess, mirroring [`crate::engineer_worktree`]: a hostile ambient
/// env (`GIT_DIR`, `GIT_WORK_TREE`, `LD_PRELOAD`, …) cannot hijack the build
/// source.
///
/// `SSH_AUTH_SOCK` is forwarded so the `ssh://`/scp-like transports permitted by
/// [`validate_origin_transport`] can authenticate via a running ssh-agent
/// (encrypted or agent-only keys) rather than failing. `GIT_SSH_COMMAND` is
/// **deliberately not** forwarded: it executes an arbitrary command, which is
/// exactly the hijack class this scrub exists to strip.
fn scrub_git_env(cmd: &mut Command) {
    cmd.env_clear();
    for var in ["PATH", "HOME", "SSH_AUTH_SOCK"] {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
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
        // Redact any credentials git prints (e.g. a token-bearing remote URL in
        // a "fatal: unable to access 'https://<token>@…'" message) before the
        // detail is surfaced to the operator terminal or logs (SEC-D2).
        return Err(redact_credentials(&format!(
            "git {:?} failed in {}: {}",
            args,
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
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
///
/// The whole resolve→checkout→build critical section is serialized by the
/// host-wide [`BuildLock`] (`<state_root>/cargo_build.lock` — the same lock the
/// operator dashboard surfaces and can force-release). Two concurrent
/// self-deploys share the persistent source checkout ([`self_deploy_src_dir`])
/// **and** the warm `target_dir`; without this lock, run B's
/// `git checkout --detach <shaB>` could rewrite the working tree while run A's
/// `cargo build` is still reading it, so A would compile B's tree yet embed A's
/// `SIMARD_GIT_HASH` — silently shipping the wrong source past the post-deploy
/// `version_advanced` gate, which only compares the *embedded* SHA to the
/// target. The lock is held for the whole prepare+build and released on drop;
/// failing to acquire it within [`BUILD_LOCK_TIMEOUT`] aborts loudly (never a
/// silent race). (Its 10-min stale-reap only weakens the rare concurrent *cold*
/// first build; warm builds finish well inside it.)
pub(crate) fn prepare_and_build(
    source: &dyn SelfDeploySourcePreparer,
    target_commit: &str,
    target_dir: &Path,
) -> Result<PathBuf, SafeUpdateError> {
    let _build_guard = BuildLock::new(&simard_state_root())
        .acquire(BUILD_LOCK_TIMEOUT)
        .map_err(|e| SafeUpdateError::BuildFailed {
            detail: format!(
                "could not acquire the self-deploy build lock \
                 (another self-deploy build may be running): {e}"
            ),
        })?;

    let repo = source.prepare(target_commit)?;
    crate::self_relaunch::build_self_deploy_candidate(&repo, target_dir).map_err(|e| {
        SafeUpdateError::BuildFailed {
            detail: e.to_string(),
        }
    })
}
