//! Hermetic tests for the cwd-independent self-deploy source preparer
//! (issue #2467 — `src/self_deploy/source_prep.rs`).
//!
//! These pin the two fixes the issue requires:
//!   1. **Build the merged commit, not cwd HEAD** — the preparer `git fetch`es
//!      the canonical repo and `checkout --detach`es the *target merged SHA*
//!      (`prepare`), resolved independently of the current working directory.
//!   2. **Warm target dir** — `self_deploy_target_dir()` is a persistent,
//!      non-PID-keyed dir under the state root so builds are incremental.
//!
//! Everything runs offline: the git tests use a local `file`-path "origin"
//! repo (a `git fetch` from a local path needs no network), and the wiring
//! tests inject a fake [`SelfDeploySourcePreparer`].
//!
//! Written against the public contract in
//! `docs/reference/self-deploy-source-prep.md`. They MUST fail in the red
//! phase (the `source_prep.rs` bodies are `unimplemented!()` stubs) and MUST
//! pass once the implementation lands — without any test edits.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::safe_update::SafeUpdateError;
use crate::self_relaunch::build_self_deploy_candidate;
use crate::state_root::{STATE_ROOT_ENV, simard_state_root};

use super::source_prep::{
    GitSourcePreparer, SELF_DEPLOY_SRC_DIRNAME, SELF_DEPLOY_TARGET_DIRNAME,
    SelfDeploySourcePreparer, prepare_and_build, self_deploy_src_dir, self_deploy_target_dir,
    validate_full_sha, validate_origin_transport,
};

const SELF_DEPLOY_REPO_ENV: &str = "SIMARD_SELF_DEPLOY_REPO";

// ---------------------------------------------------------------------------
// Env + git fixtures (mirror engineer_worktree/state_root test discipline:
// env_clear() then re-inject only PATH/HOME so ambient GIT_* / state-root env
// cannot poison these fixtures).
// ---------------------------------------------------------------------------

/// Scoped env override that restores the previous value on drop. Tests that use
/// it MUST be `#[serial]` (env is process-global).
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &Path) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: callers are serialized via #[serial]; edition-2024 set_var is unsafe.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: callers are serialized via #[serial].
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

fn git_cmd(repo: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo).env_clear();
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    if let Ok(h) = std::env::var("HOME") {
        cmd.env("HOME", h);
    }
    cmd
}

fn git_run(repo: &Path, args: &[&str]) {
    let out = git_cmd(repo, args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = git_cmd(repo, args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Initialize a bare-ish "origin" repo on `main` with one seed commit; return
/// (repo_path, seed_sha).
fn init_origin(dir: &Path) -> (PathBuf, String) {
    std::fs::create_dir_all(dir).unwrap();
    git_run(dir, &["init", "--initial-branch=main", "--quiet"]);
    git_run(dir, &["config", "user.email", "t@e.com"]);
    git_run(dir, &["config", "user.name", "t"]);
    git_run(dir, &["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.join("VERSION"), "c1\n").unwrap();
    git_run(dir, &["add", "VERSION"]);
    git_run(dir, &["commit", "-m", "c1", "--quiet"]);
    let sha = git_out(dir, &["rev-parse", "HEAD"]);
    (dir.to_path_buf(), sha)
}

/// Add a second commit on `main` in `origin` and return its sha (the "merged
/// head" a stale clone has not yet fetched).
fn add_merged_commit(origin: &Path) -> String {
    std::fs::write(origin.join("VERSION"), "c2\n").unwrap();
    git_run(origin, &["add", "VERSION"]);
    git_run(origin, &["commit", "-m", "c2 (merged head)", "--quiet"]);
    git_out(origin, &["rev-parse", "HEAD"])
}

/// Clone `origin` into `dest` (local path => offline). The clone's `origin`
/// remote points at the local origin path.
fn clone_local(origin: &Path, dest: &Path) {
    let parent = dest.parent().unwrap();
    std::fs::create_dir_all(parent).unwrap();
    let out = git_cmd(parent, &["clone", "--quiet"])
        .arg(origin)
        .arg(dest)
        .output()
        .expect("spawn git clone");
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------------------
// Warm target dir (fix #2): persistent, under state root, not PID/temp,
// reaper-safe name.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn self_deploy_dirs_resolve_under_state_root_honoring_env() {
    let tmp = tempfile::tempdir().unwrap();
    let _g = EnvGuard::set(STATE_ROOT_ENV, tmp.path());

    assert_eq!(
        self_deploy_target_dir(),
        tmp.path().join(SELF_DEPLOY_TARGET_DIRNAME),
        "warm target dir must be <SIMARD_STATE_ROOT>/{SELF_DEPLOY_TARGET_DIRNAME}"
    );
    assert_eq!(
        self_deploy_src_dir(),
        tmp.path().join(SELF_DEPLOY_SRC_DIRNAME),
        "source checkout dir must be <SIMARD_STATE_ROOT>/{SELF_DEPLOY_SRC_DIRNAME}"
    );
}

#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn warm_target_dir_is_persistent_not_pid_keyed_and_under_state_root() {
    let tmp = tempfile::tempdir().unwrap();
    let _g = EnvGuard::set(STATE_ROOT_ENV, tmp.path());

    let a = self_deploy_target_dir();
    let b = self_deploy_target_dir();
    assert_eq!(
        a, b,
        "warm target dir must be stable across calls (persistent)"
    );

    assert!(
        a.starts_with(simard_state_root()),
        "warm target dir {} must live under the durable state root",
        a.display()
    );

    // The whole point of fix #2: NOT the old per-PID cold temp dir.
    let pid = std::process::id().to_string();
    assert!(
        !a.to_string_lossy().contains(&pid),
        "warm target dir must NOT be PID-keyed, got: {}",
        a.display()
    );
    let legacy = crate::self_relaunch::RelaunchConfig::default().canary_target_dir;
    assert_ne!(
        a, legacy,
        "warm target dir must differ from the legacy per-PID temp canary dir"
    );

    // The whole point of fix #2 is a *durable* warm dir, not the ephemeral
    // system temp base. The hermetic setup above pins the state root to a
    // tempdir purely for test isolation (which — per the repo's own
    // `hermetic_state_root_lives_under_temp_dir` — necessarily lives *under*
    // env::temp_dir()), so asserting `!a.starts_with(temp_dir())` on `a`
    // directly would contradict the equality the production resolution
    // guarantees. Instead assert the property that actually matters: under a
    // durable (production-shaped, $HOME-based) state root, the warm dir escapes
    // temp_dir() entirely. (`self_deploy_target_dir()` only computes the path —
    // nothing is created under $HOME.)
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME is set in the test environment");
    let _g2 = EnvGuard::set(STATE_ROOT_ENV, &home.join(".simard"));
    let durable_warm = self_deploy_target_dir();
    assert_eq!(
        durable_warm,
        home.join(".simard").join(SELF_DEPLOY_TARGET_DIRNAME),
        "under a durable state root the warm dir is <state_root>/{SELF_DEPLOY_TARGET_DIRNAME}"
    );
    assert!(
        !durable_warm.starts_with(std::env::temp_dir()),
        "durable warm target dir must not live under temp_dir(), got: {}",
        durable_warm.display()
    );
}

#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn warm_target_dir_name_does_not_match_any_cleanup_reaper() {
    // cmd_cleanup/disk.rs reapers match by *name* within their scan roots
    // (/tmp). The warm dir must not be eligible for deletion. We replicate the
    // documented predicates and assert the warm dir name matches none.
    fn matches_tmp_canary_reaper(name: &str) -> bool {
        name.starts_with("simard-canary")
            || name.starts_with("simard-e2e")
            || name.starts_with("simard-")
            || name.starts_with("amplihack-")
            || name.starts_with("amplihack_eval")
            || name.starts_with("ia2-")
    }
    fn matches_tmp_target_cap(name: &str) -> bool {
        name.starts_with("simard-") && name.ends_with("-target")
    }

    let tmp = tempfile::tempdir().unwrap();
    let _g = EnvGuard::set(STATE_ROOT_ENV, tmp.path());

    let warm = self_deploy_target_dir();
    let name = warm
        .file_name()
        .expect("warm dir has a final component")
        .to_string_lossy()
        .into_owned();

    assert_eq!(name, SELF_DEPLOY_TARGET_DIRNAME);
    assert!(
        !matches_tmp_canary_reaper(&name),
        "warm dir name {name:?} must not match the /tmp canary reaper"
    );
    assert!(
        !matches_tmp_target_cap(&name),
        "warm dir name {name:?} must not match the /tmp '-target' LRU cap reaper"
    );
}

// ---------------------------------------------------------------------------
// SHA validation (SEC-I2: block leading-'-' option injection into git argv).
// ---------------------------------------------------------------------------

#[test]
fn validate_full_sha_accepts_exactly_40_lowercase_hex() {
    let ok = "abcdef0123456789abcdef0123456789abcdef01";
    assert_eq!(ok.len(), 40);
    assert!(
        validate_full_sha(ok).is_ok(),
        "a full 40-hex sha must be accepted"
    );
}

#[test]
fn validate_full_sha_rejects_malformed_and_injection() {
    let bad = [
        "",                                          // empty
        "abcdef",                                    // too short
        "abcdef0123456789abcdef0123456789abcdef0",   // 39
        "abcdef0123456789abcdef0123456789abcdef012", // 41
        "ABCDEF0123456789ABCDEF0123456789ABCDEF01",  // uppercase
        "gbcdef0123456789abcdef0123456789abcdef01",  // non-hex 'g'
        "-bcdef0123456789abcdef0123456789abcdef01",  // leading '-' (argv option injection)
        " bcdef0123456789abcdef0123456789abcdef01",  // leading space
        "deadbeef",                                  // short symbolic-ish
    ];
    for s in bad {
        assert!(
            validate_full_sha(s).is_err(),
            "validate_full_sha must reject {s:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Origin transport allow-list (SEC-I3: refuse arbitrary-command transports).
// ---------------------------------------------------------------------------

#[test]
fn validate_origin_transport_allows_https_and_ssh() {
    for url in [
        "https://github.com/rysweet/Simard.git",
        "ssh://git@github.com/rysweet/Simard.git",
        "git@github.com:rysweet/Simard.git",
    ] {
        assert!(
            validate_origin_transport(url).is_ok(),
            "transport must be allowed: {url}"
        );
    }
}

#[test]
fn validate_origin_transport_rejects_command_transports() {
    for url in [
        "ext::sh -c \"touch /tmp/pwned\"",
        "ext::git-upload-pack",
        "fd::3",
    ] {
        assert!(
            validate_origin_transport(url).is_err(),
            "arbitrary-command transport must be rejected: {url}"
        );
    }
}

// ---------------------------------------------------------------------------
// prepare(): fetch + checkout the MERGED head, independent of cwd (fix #1).
// ---------------------------------------------------------------------------

#[test]
fn prepare_fetches_and_checks_out_merged_head_not_cwd_head() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let canonical = root.path().join("canonical");

    // origin starts at c1; a stale clone is made; THEN origin advances to c2.
    let (_origin, _c1) = init_origin(&origin);
    clone_local(&origin, &canonical);
    let c2 = add_merged_commit(&origin);

    // The stale clone is still on c1 and has never seen c2.
    assert_ne!(git_out(&canonical, &["rev-parse", "HEAD"]), c2);

    // Resolution is via the explicit repo (cwd-independent); the test's actual
    // cwd is the worktree, which is NOT this canonical clone.
    let preparer = GitSourcePreparer::at(&canonical);
    let prepared = preparer
        .prepare(&c2)
        .expect("prepare must fetch + checkout the merged head");

    assert_eq!(
        std::fs::canonicalize(&prepared).unwrap(),
        std::fs::canonicalize(&canonical).unwrap(),
        "prepare must return the canonical repo (not the cwd checkout)"
    );
    assert_ne!(
        std::fs::canonicalize(&prepared).unwrap(),
        std::env::current_dir().unwrap(),
        "the build source must not be the current working directory"
    );
    assert_eq!(
        git_out(&canonical, &["rev-parse", "HEAD"]),
        c2,
        "prepared repo HEAD must be the merged head c2"
    );
    // Detached HEAD at the exact merged commit (so SIMARD_GIT_HASH == c2).
    assert!(
        !git_cmd(&canonical, &["symbolic-ref", "-q", "HEAD"])
            .status()
            .unwrap()
            .success(),
        "HEAD must be detached at the merged commit"
    );
}

#[test]
fn prepare_is_loud_when_target_commit_is_absent_no_cwd_fallback() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let canonical = root.path().join("canonical");
    init_origin(&origin);
    clone_local(&origin, &canonical);

    // A syntactically valid 40-hex sha that does not exist in the repo.
    let absent = "0".repeat(40);
    let err = GitSourcePreparer::at(&canonical)
        .prepare(&absent)
        .expect_err("an unavailable merged commit must fail loudly, not fall back");
    assert!(
        matches!(
            err,
            SafeUpdateError::CheckoutFailed { .. }
                | SafeUpdateError::FetchFailed { .. }
                | SafeUpdateError::SourceResolveFailed { .. }
        ),
        "expected a loud source/fetch/checkout error, got: {err:?}"
    );
}

#[test]
fn prepare_rejects_non_full_sha_before_touching_git() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let canonical = root.path().join("canonical");
    init_origin(&origin);
    clone_local(&origin, &canonical);

    // Option-injection attempt; a valid repo so the ONLY reason to fail is the
    // bad SHA being rejected up front.
    let err = GitSourcePreparer::at(&canonical)
        .prepare("--upload-pack=touch /tmp/pwned")
        .expect_err("a non-40-hex target must be rejected before any git call");
    assert!(
        !matches!(err, SafeUpdateError::BuildFailed { .. }),
        "rejection must happen during source prep, never reach a build: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// resolve_repo(): SIMARD_SELF_DEPLOY_REPO override precedence + validation,
// never a cwd fallback.
// ---------------------------------------------------------------------------

#[test]
#[serial_test::serial(simard_self_deploy_repo)]
fn resolve_repo_env_override_wins_when_valid() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("origin");
    let canonical = root.path().join("canonical");
    init_origin(&origin);
    clone_local(&origin, &canonical);

    let _g = EnvGuard::set(SELF_DEPLOY_REPO_ENV, &canonical);
    let resolved = GitSourcePreparer::new()
        .resolve_repo()
        .expect("a valid SIMARD_SELF_DEPLOY_REPO must resolve");
    assert_eq!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::fs::canonicalize(&canonical).unwrap(),
        "SIMARD_SELF_DEPLOY_REPO must win the resolution precedence"
    );
    assert_ne!(
        std::fs::canonicalize(&resolved).unwrap(),
        std::env::current_dir().unwrap(),
        "resolution must never fall back to cwd"
    );
}

#[test]
#[serial_test::serial(simard_self_deploy_repo)]
fn resolve_repo_rejects_invalid_env_override_without_cwd_fallback() {
    // A path-traversal value and a non-repo path must both be rejected loudly,
    // never silently resolving to the current working directory.
    let traversal = PathBuf::from("/tmp/../tmp/not-a-real-self-deploy-repo-2467");
    let _g = EnvGuard::set(SELF_DEPLOY_REPO_ENV, &traversal);
    let err = GitSourcePreparer::new()
        .resolve_repo()
        .expect_err("a '..'-bearing SIMARD_SELF_DEPLOY_REPO must be rejected");
    assert!(
        matches!(err, SafeUpdateError::SourceResolveFailed { .. }),
        "invalid override must surface SourceResolveFailed, got: {err:?}"
    );

    let not_a_repo = tempfile::tempdir().unwrap();
    let _g2 = EnvGuard::set(SELF_DEPLOY_REPO_ENV, not_a_repo.path());
    let err2 = GitSourcePreparer::new()
        .resolve_repo()
        .expect_err("a non-git-work-tree SIMARD_SELF_DEPLOY_REPO must be rejected");
    assert!(
        matches!(err2, SafeUpdateError::SourceResolveFailed { .. }),
        "non-repo override must surface SourceResolveFailed, got: {err2:?}"
    );
}

// ---------------------------------------------------------------------------
// prepare_and_build(): the orchestrator's step-1 composition. A prep failure
// must propagate BEFORE any build (and a fortiori before daemon mutation), and
// prep must be asked for the exact target merged commit.
// ---------------------------------------------------------------------------

/// Records the target commit it was asked to prepare and returns a chosen
/// outcome — never touches git or the filesystem.
struct RecordingPreparer {
    asked: Mutex<Vec<String>>,
    outcome: Result<PathBuf, ()>,
}

impl RecordingPreparer {
    fn failing() -> Self {
        Self {
            asked: Mutex::new(Vec::new()),
            outcome: Err(()),
        }
    }
}

impl SelfDeploySourcePreparer for RecordingPreparer {
    fn prepare(&self, target_commit: &str) -> Result<PathBuf, SafeUpdateError> {
        self.asked.lock().unwrap().push(target_commit.to_string());
        match &self.outcome {
            Ok(p) => Ok(p.clone()),
            Err(()) => Err(SafeUpdateError::FetchFailed {
                detail: "fake: offline".to_string(),
            }),
        }
    }
}

#[test]
fn prepare_and_build_propagates_prepare_failure_before_building() {
    let warm = tempfile::tempdir().unwrap();
    let fake = RecordingPreparer::failing();
    let sha = "abcdef0123456789abcdef0123456789abcdef01";

    let err = prepare_and_build(&fake, sha, warm.path())
        .expect_err("a failed source prep must abort the build");
    assert!(
        matches!(err, SafeUpdateError::FetchFailed { .. }),
        "prep failure must propagate untransformed, got: {err:?}"
    );

    // No build was attempted: the warm target has no release artifact.
    assert!(
        !warm.path().join("release").join("simard").exists(),
        "build must NOT run after a prep failure (no cwd-HEAD fallback)"
    );
}

#[test]
fn prepare_and_build_asks_to_prepare_the_target_merged_commit() {
    let warm = tempfile::tempdir().unwrap();
    let fake = RecordingPreparer::failing();
    let sha = "0123456789abcdef0123456789abcdef01234567";

    let _ = prepare_and_build(&fake, sha, warm.path());

    let asked = fake.asked.lock().unwrap();
    assert_eq!(
        asked.as_slice(),
        &[sha.to_string()],
        "prepare_and_build must request exactly the target merged commit"
    );
}

// ---------------------------------------------------------------------------
// build_self_deploy_candidate(): builds the prepared repo into the WARM target
// dir; loud on failure. (Mirrors self_relaunch::build_canary's failure test —
// no full project compile.)
// ---------------------------------------------------------------------------

#[test]
fn build_self_deploy_candidate_creates_warm_dir_and_fails_loudly_on_bad_repo() {
    let warm =
        std::env::temp_dir().join(format!("simard-sd-warm-build-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&warm);
    let bogus_repo = PathBuf::from("/tmp/no-such-self-deploy-repo-for-2467-test");

    let res = build_self_deploy_candidate(&bogus_repo, &warm);

    assert!(
        warm.exists(),
        "build_self_deploy_candidate must create the warm target dir"
    );
    assert!(
        res.is_err(),
        "a missing source manifest must make the build fail loudly"
    );
    let _ = std::fs::remove_dir_all(&warm);
}
