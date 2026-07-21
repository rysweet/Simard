use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::RelaunchConfig;
use crate::error::{SimardError, SimardResult};
use crate::self_relaunch_semaphore::{HandoffConfig, HandoffResult, LeaderSemaphore};

/// Shared `cargo build --release` driver for the canary and self-deploy build
/// paths.
///
/// Creates `target_dir`, builds `manifest_path` into it, and returns the
/// resulting `target_dir/release/simard` artifact. `label` names the operation
/// in any surfaced error (e.g. `"canary-build"`). When `neutralize_git_env` is
/// set, the ambient git-redirection vars (`GIT_DIR`, `GIT_WORK_TREE`, …) are
/// stripped so `build.rs` derives `SIMARD_GIT_HASH` from the package's own
/// checkout instead of a hijacked one. Any failure is surfaced loudly.
fn run_release_build(
    label: &str,
    target_dir: &Path,
    manifest_path: &Path,
    neutralize_git_env: bool,
) -> SimardResult<PathBuf> {
    std::fs::create_dir_all(target_dir).map_err(|e| SimardError::PersistentStoreIo {
        store: label.to_string(),
        action: "create target directory".to_string(),
        path: target_dir.to_path_buf(),
        reason: e.to_string(),
    })?;

    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--release")
        .arg("--target-dir")
        .arg(target_dir)
        .arg("--manifest-path")
        .arg(manifest_path)
        .env("CARGO_BUILD_JOBS", crate::cargo_jobs::cargo_jobs());
    if neutralize_git_env {
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("GIT_OBJECT_DIRECTORY");
    }

    let output = cmd.output().map_err(|e| SimardError::RpcSpawnFailed {
        endpoint: label.to_string(),
        reason: format!("cargo build failed to start: {e}"),
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SimardError::RpcCallFailed {
            endpoint: label.to_string(),
            method: "cargo build --release".to_string(),
            reason: format!("build failed (exit {}): {}", output.status, stderr),
        });
    }

    let binary_path = target_dir.join("release").join("simard");
    if !binary_path.exists() {
        return Err(SimardError::ArtifactIo {
            path: binary_path,
            reason: format!("{label} binary not found after successful build"),
        });
    }

    Ok(binary_path)
}

/// Build a canary binary via `cargo build --release` in a separate target directory.
pub fn build_canary(config: &RelaunchConfig) -> SimardResult<PathBuf> {
    run_release_build(
        "canary-build",
        &config.canary_target_dir,
        &config.manifest_dir.join("Cargo.toml"),
        false,
    )
}

/// Build a self-deploy candidate binary from an **already-prepared** source
/// checkout into a **persistent warm** target directory (issue #2467).
///
/// This is the [`build_canary`] sibling used by the autonomous self-deploy
/// path. Where [`build_canary`] builds the on-disk cwd checkout
/// (`manifest_dir = "."`) into a fresh per-PID `temp_dir()` target — forcing a
/// cold from-scratch compile every run — this builds `repo`'s `Cargo.toml`
/// into the caller-provided, reusable `target_dir` (e.g.
/// `crate::self_deploy::self_deploy_target_dir()`), so repeated self-deploys
/// are incremental (~2–3 min) instead of cold (~10+ min).
///
/// `repo` is expected to already be checked out at the target merged commit by
/// a [`crate::self_deploy::SelfDeploySourcePreparer`]; this function does not
/// touch git. [`build_canary`] and [`RelaunchConfig`]'s defaults are left
/// byte-for-byte unchanged.
///
/// Contract: creates `target_dir` if absent, runs `cargo build --release`
/// against `repo/Cargo.toml` into `target_dir`, and returns
/// `target_dir/release/simard` on success. Any build failure is surfaced
/// loudly (never a silent success).
pub fn build_self_deploy_candidate(repo: &Path, target_dir: &Path) -> SimardResult<PathBuf> {
    // Neutralize ambient git-repo redirection (issue #2467). Cargo runs
    // `build.rs` in the package root (`repo`), where it derives
    // `SIMARD_GIT_HASH` from `git rev-parse HEAD`. A stray `GIT_DIR` /
    // `GIT_WORK_TREE` in the daemon's environment would redirect that read to a
    // different repo and embed the wrong commit — defeating the post-deploy
    // `version_advanced` integrity gate (which compares the embedded SHA to
    // `target_commit`). The rest of the env (PATH/HOME/CARGO_*/RUSTUP_*) is
    // required for the build to run, so only the git-redirection vars are
    // stripped.
    run_release_build(
        "self-deploy-build",
        target_dir,
        &repo.join("Cargo.toml"),
        true,
    )
}

/// Validate that a git ref/SHA handed to the self-deploy **convergence**
/// re-trigger (the stuck red-canary fix) is a bare hex commit-ish (4–64 hex
/// chars) BEFORE it is ever passed as a `Command` argument.
///
/// A convergence re-trigger builds and promotes the target commit, so an
/// unvalidated ref could smuggle a flag (`--upload-pack=…`), shell, or path
/// metacharacters into the build/checkout invocation. Argument-vector calls
/// (`Command::arg`) already prevent shell interpretation; this is the
/// defense-in-depth *shape* check the design mandates ("validate ref format;
/// never shell strings"). Mirrors `overseer::deploy::is_hex_commitish`.
///
/// A valid build ref is a bare git commit-ish: 4–64 ASCII hex characters
/// (short SHA-1 through full SHA-256). Anything else — symbolic refs (`HEAD`,
/// `main`), remote refs (`origin/main`), flags (`--upload-pack=…`), whitespace,
/// shell metacharacters, path traversal, or command substitution — is refused
/// with a `VerificationFailed` error before it can reach a `Command` argument.
#[allow(dead_code)]
pub(crate) fn validate_build_ref(reff: &str) -> SimardResult<()> {
    let len = reff.len();
    if (4..=64).contains(&len) && reff.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(SimardError::VerificationFailed {
            reason: format!(
                "self-deploy convergence build ref must be a bare hex commit-ish \
                 (4-64 hex chars); refused: {reff:?}"
            ),
        })
    }
}

/// Validate preconditions and hand over execution to the canary binary.
///
/// On Unix, this uses `CommandExt::exec()` to replace the current process
/// image with the canary binary. This function does not return on success.
/// Returns error if pid is 0 or binary does not exist.
pub fn handover(current_pid: u32, canary_binary: &Path) -> SimardResult<()> {
    if current_pid == 0 {
        return Err(SimardError::RpcCallFailed {
            endpoint: "self-relaunch".to_string(),
            method: "handover".to_string(),
            reason: "current_pid cannot be 0".to_string(),
        });
    }

    if !canary_binary.exists() {
        return Err(SimardError::ArtifactIo {
            path: canary_binary.to_path_buf(),
            reason: "canary binary does not exist at handover time".to_string(),
        });
    }

    let metadata = std::fs::metadata(canary_binary).map_err(|e| SimardError::ArtifactIo {
        path: canary_binary.to_path_buf(),
        reason: format!("cannot read canary binary metadata: {e}"),
    })?;

    if !metadata.is_file() {
        return Err(SimardError::ArtifactIo {
            path: canary_binary.to_path_buf(),
            reason: "canary path is not a regular file".to_string(),
        });
    }

    // Replace the current process with the canary binary.
    // On Unix, exec() replaces the process image — this does not return on success.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = Command::new(canary_binary).exec();
        // exec() only returns on error.
        Err(SimardError::RpcCallFailed {
            endpoint: "self-relaunch".to_string(),
            method: "handover".to_string(),
            reason: format!("exec failed for '{}': {err}", canary_binary.display()),
        })
    }

    // On non-Unix platforms, spawn the canary and exit the current process.
    #[cfg(not(unix))]
    {
        Command::new(canary_binary)
            .spawn()
            .map_err(|e| SimardError::RpcCallFailed {
                rpc: "self-relaunch".to_string(),
                method: "handover".to_string(),
                reason: format!("failed to spawn canary '{}': {e}", canary_binary.display()),
            })?;
        std::process::exit(0);
    }
}

/// Perform a coordinated self-relaunch using the leader semaphore.
///
/// This is the recommended relaunch path for production use. It:
/// 1. Acquires the leader semaphore (or confirms we already hold it)
/// 2. Delegates to [`coordinated_handoff`] which builds, gates, spawns, and transfers
/// 3. Returns the handoff result so the caller can shut down gracefully
///
/// Unlike [`handover`] which replaces the process image immediately,
/// this function keeps the old process alive until the new one is verified healthy.
pub fn coordinated_relaunch(
    semaphore_dir: &Path,
    config: &RelaunchConfig,
) -> SimardResult<HandoffResult> {
    let my_pid = std::process::id();
    let lock_path = semaphore_dir.join("simard-leader.lock");
    let semaphore = LeaderSemaphore::new(lock_path);

    // Ensure we are the leader before attempting handoff.
    semaphore.try_acquire(my_pid)?;

    let handoff_config = HandoffConfig::new(semaphore, config.clone());
    crate::self_relaunch_semaphore::coordinated_handoff(my_pid, &handoff_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handover_rejects_zero_pid() {
        let err = handover(0, Path::new("/usr/bin/true")).unwrap_err();
        assert!(err.to_string().contains("current_pid"));
    }

    #[test]
    fn handover_rejects_missing_binary() {
        let err = handover(12345, Path::new("/tmp/no-such-canary-82719")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn handover_rejects_directory_as_binary() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        if dir.exists() {
            let err = handover(12345, &dir).unwrap_err();
            assert!(err.to_string().contains("not a regular file"), "{}", err);
        }
    }

    #[test]
    fn build_canary_creates_target_dir_and_propagates_failure() {
        // Use a unique tempdir so we can verify create_dir_all works
        // and that a build failure (bogus manifest) returns Err, not Ok.
        let tmp =
            std::env::temp_dir().join(format!("simard-canary-test-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(!tmp.exists(), "precondition: temp dir must not exist yet");

        let config = RelaunchConfig {
            canary_target_dir: tmp.clone(),
            manifest_dir: PathBuf::from("/tmp/no-such-manifest-dir-for-canary-test"),
            ..Default::default()
        };
        let result = build_canary(&config);
        // The target dir should have been created even though the build fails.
        assert!(tmp.exists(), "build_canary must create canary_target_dir");
        // The build must fail because the manifest dir doesn't exist.
        assert!(
            result.is_err(),
            "bogus manifest must cause build_canary to return Err"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("build failed") || err_msg.contains("cargo"),
            "error must mention the build failure, got: {err_msg}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn coordinated_relaunch_acquires_semaphore() {
        let dir = std::env::temp_dir().join(format!("simard-relaunch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = RelaunchConfig {
            canary_target_dir: PathBuf::from("/tmp/no-such-canary-dir"),
            manifest_dir: PathBuf::from("/tmp/no-such-manifest"),
            ..Default::default()
        };
        // coordinated_relaunch will acquire the semaphore, then fail at build_canary
        // because manifest_dir doesn't exist — that's fine, we're testing the wiring.
        let err = coordinated_relaunch(&dir, &config).unwrap_err();
        // The error should come from build_canary (not from semaphore acquisition).
        let msg = err.to_string();
        assert!(
            msg.contains("canary") || msg.contains("cargo") || msg.contains("build"),
            "expected build error, got: {msg}"
        );
        // Semaphore should have been acquired — verify the lock file exists.
        assert!(dir.join("simard-leader.lock").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ────────────────────────────────────────────────────────────────────────
    // TDD (Step 7) — P1 convergence & deploy-gate immutability
    //
    // File under test: src/self_relaunch/canary.rs (convergence primitives) plus
    // the FROZEN deploy gate in src/overseer/deploy.rs (imported here per the
    // design's test-placement: canary.rs owns the gate-preservation guard).
    // ────────────────────────────────────────────────────────────────────────

    use crate::overseer::deploy::{
        CRASH_LOOP_CHURN_THRESHOLD, DeployContext, DeployRefusal, evaluate_deploy_gate,
    };

    fn forward_ctx() -> DeployContext {
        DeployContext {
            running_commit: "aaaaaaaaaaaa".to_string(),
            target_commit: "bbbbbbbbbbbb".to_string(),
            target_is_ancestor_of_running: false,
            canary_passed: true,
            recent_restart_churn: 0,
        }
    }

    /// RED (stub): the convergence re-trigger MUST reject any target ref that is
    /// not a bare hex commit-ish before it reaches a `Command` argument, closing
    /// the argument/command-injection class. The Step-7 stub accepts everything,
    /// so every rejection assertion below fails until Step 8 implements it.
    #[test]
    fn validate_build_ref_rejects_injection_and_non_hex() {
        for bad in [
            "",                // empty
            "abc",             // too short (<4)
            &"a".repeat(65),   // too long (>64)
            "HEAD",            // symbolic ref
            "main",            // branch name
            "origin/main",     // remote ref with slash
            "--upload-pack=x", // flag smuggling
            "dead beef",       // whitespace
            "aaaa;rm -rf /",   // shell metacharacters
            "../etc/passwd",   // path traversal
            "cafe$(whoami)",   // command substitution
        ] {
            assert!(
                validate_build_ref(bad).is_err(),
                "convergence build ref must be refused: {bad:?}"
            );
        }
    }

    /// The complement of the RED test: a genuine bare hex commit-ish (short or
    /// full SHA-1/SHA-256) is accepted so a valid convergence re-trigger is not
    /// blocked. Passes against the stub too (it accepts all), and must keep
    /// passing after Step 8 tightens the validation.
    #[test]
    fn validate_build_ref_accepts_bare_hex_commitish() {
        for good in [
            "dead",
            "deadBEEF",
            &"a".repeat(40),
            &"f".repeat(64),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709",
        ] {
            assert!(
                validate_build_ref(good).is_ok(),
                "valid hex commit-ish must be accepted: {good:?}"
            );
        }
    }

    /// GATE-PRESERVATION GUARD (design: gate is FROZEN). A convergence change
    /// must never "unstick" a deploy by weakening the red-canary rule. For every
    /// genuine forward deploy (non-no-op, non-rollback), a failed canary MUST
    /// yield exactly `RedCanary` and NEVER `Ok`, at any churn level. Exhaustive
    /// over the remaining gate inputs.
    #[test]
    fn deploy_gate_red_canary_is_immutable_for_forward_deploys() {
        for churn in [
            0,
            1,
            CRASH_LOOP_CHURN_THRESHOLD - 1,
            CRASH_LOOP_CHURN_THRESHOLD,
            99,
        ] {
            let mut c = forward_ctx();
            c.canary_passed = false;
            c.recent_restart_churn = churn;
            assert_eq!(
                evaluate_deploy_gate(&c),
                Err(DeployRefusal::RedCanary),
                "a red canary must block a forward deploy regardless of churn={churn}"
            );
        }
    }

    /// GATE-PRESERVATION GUARD: a passing canary on a clean forward deploy is
    /// still allowed — the immutability guard above must not be satisfied by
    /// simply refusing everything.
    #[test]
    fn deploy_gate_still_allows_clean_forward_deploy() {
        assert!(
            evaluate_deploy_gate(&forward_ctx()).is_ok(),
            "convergence must not over-tighten the gate into refusing valid deploys"
        );
    }

    /// GATE-PRESERVATION GUARD: the crash-loop threshold is a frozen safety
    /// constant; a convergence change must not raise it to force a stuck deploy
    /// through.
    #[test]
    fn crash_loop_threshold_is_frozen() {
        assert_eq!(
            CRASH_LOOP_CHURN_THRESHOLD, 3,
            "crash-loop churn threshold is a frozen safety constant"
        );
    }

    /// CONVERGENCE BUILD GUARD: a convergence re-trigger reuses the shared
    /// release-build driver, which MUST surface a build failure as `Err` — never
    /// a silent `Ok`. A silently-successful "convergence" would promote an
    /// unbuilt/stale binary. Exercised through `build_self_deploy_candidate`
    /// (the self-deploy sibling the convergence path builds with) against a repo
    /// dir with no `Cargo.toml`.
    #[test]
    fn convergence_build_failure_is_loud_never_silent_ok() {
        let repo = std::env::temp_dir().join(format!(
            "simard-converge-build-{}-{}",
            std::process::id(),
            "no-manifest"
        ));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let target = repo.join("target");

        let result = build_self_deploy_candidate(&repo, &target);
        assert!(
            result.is_err(),
            "a convergence build with no Cargo.toml must fail loudly, not return Ok"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("build failed") || msg.contains("cargo") || msg.contains("self-deploy"),
            "error must name the build failure, got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
