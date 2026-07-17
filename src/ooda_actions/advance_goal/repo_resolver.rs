//! Goal target-repo resolver (issue #2359, BUG 1).
//!
//! Maps a goal's `repo` **slug** to a validated, absolute path to a local git
//! repository. This is the single authority for goal→path mapping; nothing
//! else may synthesise a repo path.
//!
//! Contract reference: `docs/reference/goal-target-repo-routing.md`.
//!
//! # TDD red-phase placeholder (issue #2359)
//!
//! The function bodies below are intentional stubs. The inline `#[cfg(test)]`
//! tests are written against the public contract and **must fail** in the red
//! phase (the stubs panic via `unimplemented!()`). They **must pass** once the
//! real resolver lands in the implementation step — without further test edits.

use std::path::{Path, PathBuf};

use crate::error::{SimardError, SimardResult};

/// Maximum length of a repo slug, mirroring
/// [`crate::engineer_worktree::validate_goal_id`]'s 64-byte cap.
const MAX_REPO_SLUG_LEN: usize = 64;

/// Validates an operator-supplied repo slug before it is used to build a
/// filesystem path. Modeled on
/// [`crate::engineer_worktree::validate_goal_id`].
///
/// Accepts `^[A-Za-z0-9._-]{1,64}$`; rejects empty input, path traversal
/// (`..`), a leading `-` (argv injection), a leading `.` (hidden / `.`/`..`),
/// and anything longer than 64 bytes.
pub fn validate_repo_slug(slug: &str) -> SimardResult<()> {
    let reject = |reason: String| -> SimardError {
        SimardError::InvalidConfigValue {
            key: "goal.repo".to_string(),
            value: slug.to_string(),
            help: reason,
        }
    };

    if slug.is_empty() {
        return Err(reject("repo slug must not be empty".to_string()));
    }
    if slug.len() > MAX_REPO_SLUG_LEN {
        return Err(reject(format!(
            "repo slug length {} exceeds max {MAX_REPO_SLUG_LEN}",
            slug.len()
        )));
    }
    let first = slug.as_bytes()[0];
    if first == b'-' || first == b'.' {
        return Err(reject(format!(
            "repo slug must not start with {:?}",
            first as char
        )));
    }
    for (i, b) in slug.bytes().enumerate() {
        let ok = b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-';
        if !ok {
            return Err(reject(format!(
                "repo slug contains disallowed byte {:?} at index {i}",
                b as char
            )));
        }
    }
    Ok(())
}

/// Resolves a goal's target repo (a [`crate::goal_curation::ActiveGoal::repo`]
/// slug) to a local git-repository path.
///
/// - `None`, or `Some(slug)` where `slug` case-insensitively equals `"simard"`,
///   resolves to the daemon's own checkout (`std::env::current_dir()`).
/// - Any other slug resolves to `$HOME/src/<slug>`, validated as a real git
///   repository contained under `$HOME/src/`.
///
/// Returns `Err` — never a silent Simard fallback — when a targeted repo
/// cannot be resolved (missing, not a git repo, containment violation, bad
/// slug, or `HOME`/`current_dir` unavailable).
pub fn resolve_goal_repo(repo: Option<&str>) -> SimardResult<PathBuf> {
    // None or the daemon's own slug → the daemon's checkout. Never validated
    // as an ecosystem repo: this is the trusted working directory.
    let slug = match repo {
        None => return daemon_repo(),
        Some(s) if s.eq_ignore_ascii_case("simard") => return daemon_repo(),
        Some(s) => s,
    };

    // Reject traversal / injection / out-of-charset slugs before they ever
    // touch the filesystem.
    validate_repo_slug(slug)?;

    let home = std::env::var_os("HOME").ok_or_else(|| SimardError::NotARepo {
        path: PathBuf::from(format!("$HOME/src/{slug}")),
        reason: "HOME environment variable is not set; cannot locate ~/src".to_string(),
    })?;
    let src_root = PathBuf::from(&home).join("src");
    let candidate = src_root.join(slug);

    // Canonicalize to resolve symlinks for both existence and containment.
    let canonical = candidate
        .canonicalize()
        .map_err(|e| SimardError::NotARepo {
            path: candidate.clone(),
            reason: format!(
                "target repo for slug {slug:?} does not exist or is unreadable at {}: {e}",
                candidate.display()
            ),
        })?;

    // Containment: the resolved path must live under the canonical $HOME/src,
    // so a symlink inside ~/src cannot escape the ecosystem root.
    let canonical_src_root = src_root.canonicalize().map_err(|e| SimardError::NotARepo {
        path: src_root.clone(),
        reason: format!("$HOME/src is not accessible: {e}"),
    })?;
    if !canonical.starts_with(&canonical_src_root) {
        return Err(SimardError::NotARepo {
            path: canonical,
            reason: format!(
                "resolved repo path for slug {slug:?} escapes the containment root {}",
                canonical_src_root.display()
            ),
        });
    }

    // Validate it is actually a git repository — never fall back to Simard.
    if !is_git_repo(&canonical) {
        return Err(SimardError::NotARepo {
            path: canonical.clone(),
            reason: format!(
                "{} exists but is not a git repository (slug {slug:?})",
                canonical.display()
            ),
        });
    }

    Ok(canonical)
}

/// The daemon's own checkout: the trusted current working directory.
fn daemon_repo() -> SimardResult<PathBuf> {
    std::env::current_dir().map_err(|e| SimardError::NotARepo {
        path: PathBuf::from("."),
        reason: format!("cannot resolve the daemon's current_dir: {e}"),
    })
}

/// True iff `path` is a git repository: it has a `.git` entry (a directory for
/// a normal clone, a file for a linked worktree / submodule) **or**
/// `git rev-parse --is-inside-work-tree` succeeds inside it.
fn is_git_repo(path: &Path) -> bool {
    if path.join(".git").exists() {
        return true;
    }
    std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::engineer_worktree::EngineerWorktree;

    // ── Fixtures ───────────────────────────────────────────────────────────

    /// RAII guard that sets `HOME` for the duration of a test and restores the
    /// previous value (or unset) on drop. Tests that use it MUST be annotated
    /// `#[serial_test::serial(cognitive_memory)]` — NOT the bare `#[serial]`.
    /// A write to `HOME` can tear a concurrent `SIMARD_STATE_ROOT` read (glibc
    /// `setenv` may `realloc(environ)`), so every lib-test-binary mutator that
    /// touches `HOME` (or the cognitive-memory state-root env surface) shares
    /// the ONE `cognitive_memory` serial key (issues #2360/#2375; see docs/testing/cognitive-memory-serial-isolation.md). The
    /// bare `#[serial]` uses an INDEPENDENT lock, so it would let these HOME
    /// writers run concurrently with `cognitive_memory` tests that read `HOME`
    /// (e.g. the cost-ledger meeting-turn regression), reintroducing the race.
    /// The serial-guard meta-test cannot see the `set_var` hidden inside this
    /// helper method, so this key is an author obligation, not an auto-check.
    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn set(value: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            // SAFETY: env mutation is serialised via
            // `#[serial_test::serial(cognitive_memory)]`, so no other thread
            // reads/writes the environment concurrently.
            unsafe {
                std::env::set_var("HOME", value);
            }
            Self { prev }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo).env_clear();
        if let Ok(p) = std::env::var("PATH") {
            cmd.env("PATH", p);
        }
        // Provide a HOME so git never reads a developer's global config.
        cmd.env("HOME", repo);
        let out = cmd.output().expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Initialise a real git repo with a `main` branch and one commit at `dir`.
    fn init_git_repo(dir: &Path) {
        fs::create_dir_all(dir).expect("create repo dir");
        run_git(dir, &["init", "--initial-branch=main", "--quiet"]);
        run_git(dir, &["config", "user.email", "test@example.com"]);
        run_git(dir, &["config", "user.name", "test"]);
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        fs::write(dir.join("README.md"), "seed\n").expect("seed file");
        run_git(dir, &["add", "README.md"]);
        run_git(dir, &["commit", "-m", "seed", "--quiet"]);
    }

    fn git_output(repo: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(repo).env_clear();
        if let Ok(p) = std::env::var("PATH") {
            cmd.env("PATH", p);
        }
        cmd.env("HOME", repo);
        let out = cmd.output().expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn canon(p: &Path) -> PathBuf {
        p.canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.display()))
    }

    // ── Resolution: daemon repo (None / "Simard") ──────────────────────────

    #[test]
    fn resolve_none_returns_daemon_current_dir() {
        let cwd = std::env::current_dir().expect("current_dir");
        let resolved = resolve_goal_repo(None).expect("None must resolve to the daemon repo");
        assert_eq!(
            canon(&resolved),
            canon(&cwd),
            "repo=None must resolve to the daemon's current_dir"
        );
    }

    #[test]
    fn resolve_simard_slug_is_case_insensitive_daemon_repo() {
        let cwd = canon(&std::env::current_dir().expect("current_dir"));
        for slug in ["Simard", "simard", "SIMARD", "SiMaRd"] {
            let resolved = resolve_goal_repo(Some(slug))
                .unwrap_or_else(|e| panic!("{slug:?} must resolve: {e:?}"));
            assert_eq!(
                canon(&resolved),
                cwd,
                "repo={slug:?} must resolve to the daemon repo (case-insensitive)"
            );
        }
    }

    // ── Resolution: ecosystem repo ($HOME/src/<slug>) ──────────────────────

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_ecosystem_slug_returns_src_path() {
        let home = tempdir().expect("home tempdir");
        let target = home.path().join("src").join("amplihack-rs");
        init_git_repo(&target);

        let _home = HomeGuard::set(home.path());
        let resolved = resolve_goal_repo(Some("amplihack-rs"))
            .expect("present ecosystem git repo must resolve");

        assert_eq!(
            canon(&resolved),
            canon(&target),
            "repo=Some(\"amplihack-rs\") must resolve to $HOME/src/amplihack-rs"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_missing_target_repo_is_err() {
        let home = tempdir().expect("home tempdir");
        // No $HOME/src/ghost-repo created.
        let _home = HomeGuard::set(home.path());
        let result = resolve_goal_repo(Some("ghost-repo"));
        assert!(
            result.is_err(),
            "a slug whose repo is absent locally MUST be an Err (never a silent \
             Simard fallback), got {result:?}"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_non_git_directory_is_err() {
        let home = tempdir().expect("home tempdir");
        let plain = home.path().join("src").join("not-a-repo");
        fs::create_dir_all(&plain).expect("create plain dir");
        fs::write(plain.join("file.txt"), "hi").expect("write file");

        let _home = HomeGuard::set(home.path());
        let result = resolve_goal_repo(Some("not-a-repo"));
        assert!(
            result.is_err(),
            "an existing directory that is not a git repo MUST be an Err, got {result:?}"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_rejects_traversal_slug() {
        let home = tempdir().expect("home tempdir");
        let _home = HomeGuard::set(home.path());
        for bad in ["../Simard", "..", "a/../b", "sub/dir"] {
            let result = resolve_goal_repo(Some(bad));
            assert!(
                result.is_err(),
                "traversal/invalid slug {bad:?} MUST be rejected by resolve_goal_repo, got {result:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolve_rejects_symlink_escape_from_src_root() {
        // A real git repo OUTSIDE $HOME/src, reached via a symlink placed
        // inside $HOME/src, must be rejected by the containment check.
        let home = tempdir().expect("home tempdir");
        let outside = tempdir().expect("outside tempdir");
        let real_repo = outside.path().join("amplihack-rs");
        init_git_repo(&real_repo);

        let src_root = home.path().join("src");
        fs::create_dir_all(&src_root).expect("create src root");
        std::os::unix::fs::symlink(&real_repo, src_root.join("amplihack-rs"))
            .expect("create escaping symlink");

        let _home = HomeGuard::set(home.path());
        let result = resolve_goal_repo(Some("amplihack-rs"));
        assert!(
            result.is_err(),
            "a slug resolving (via symlink) to a path outside $HOME/src/ MUST be \
             rejected as a containment violation, got {result:?}"
        );
    }

    // ── Composition: resolve → allocate the engineer worktree in the target ─

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn resolved_target_repo_hosts_the_engineer_worktree() {
        // The core BUG-1 guarantee: a goal targeting "amplihack-rs" must get
        // its engineer worktree branched off the amplihack-rs repo — NOT the
        // daemon's own checkout. Use a temp git repo as the target.
        let home = tempdir().expect("home tempdir");
        let state = tempdir().expect("state tempdir");
        let target = home.path().join("src").join("amplihack-rs");
        init_git_repo(&target);

        let _home = HomeGuard::set(home.path());
        let parent_repo =
            resolve_goal_repo(Some("amplihack-rs")).expect("target repo must resolve");

        let wt = EngineerWorktree::allocate(&parent_repo, state.path(), "amplihack-coverage")
            .expect("allocate must succeed against the resolved target repo");

        // The worktree must be registered in the TARGET repo, proving the
        // engineer branches off amplihack-rs and opens PRs there.
        let listing = git_output(&target, &["worktree", "list", "--porcelain"]);
        let needle = format!("worktree {}", wt.path().display());
        assert!(
            listing.lines().any(|l| l == needle),
            "engineer worktree {} must be registered in the TARGET repo {}; listing:\n{listing}",
            wt.path().display(),
            target.display(),
        );

        // The engineer branch must sit on the target repo's `main` HEAD.
        let branch_sha = git_output(&target, &["rev-parse", wt.branch()]);
        let main_sha = git_output(&target, &["rev-parse", "main"]);
        assert_eq!(
            branch_sha.trim(),
            main_sha.trim(),
            "engineer branch {} must be cut from the target repo's main HEAD",
            wt.branch(),
        );
    }

    // ── validate_repo_slug ─────────────────────────────────────────────────

    #[test]
    fn validate_repo_slug_accepts_valid_slugs() {
        for slug in [
            "amplihack-rs",
            "RustyClawd",
            "agent-kgpacks",
            "amplihack-memory-lib",
            "a",
            "a.b_c-d",
            "repo123",
        ] {
            assert!(
                validate_repo_slug(slug).is_ok(),
                "valid slug {slug:?} must be accepted"
            );
        }
    }

    #[test]
    fn validate_repo_slug_rejects_traversal() {
        for slug in ["..", "../etc", "a/../b", "a/b"] {
            assert!(
                validate_repo_slug(slug).is_err(),
                "path-traversal slug {slug:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_repo_slug_rejects_leading_dash() {
        for slug in ["-amplihack-rs", "--force", "-"] {
            assert!(
                validate_repo_slug(slug).is_err(),
                "leading-dash slug {slug:?} must be rejected (argv injection)"
            );
        }
    }

    #[test]
    fn validate_repo_slug_rejects_leading_dot() {
        for slug in [".git", ".", ".."] {
            assert!(
                validate_repo_slug(slug).is_err(),
                "leading-dot slug {slug:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_repo_slug_rejects_bad_charset() {
        for slug in ["amplihack rs", "repo/sub", "repo$x", "repo;rm", "ünïcode"] {
            assert!(
                validate_repo_slug(slug).is_err(),
                "out-of-charset slug {slug:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_repo_slug_rejects_empty_and_overlong() {
        assert!(
            validate_repo_slug("").is_err(),
            "empty slug must be rejected"
        );
        let overlong = "a".repeat(65);
        assert!(
            validate_repo_slug(&overlong).is_err(),
            "65-char slug must be rejected (max length 64)"
        );
        let max = "a".repeat(64);
        assert!(
            validate_repo_slug(&max).is_ok(),
            "64-char slug must be accepted (boundary)"
        );
    }
}
