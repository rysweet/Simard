//! Integration tests that walk the [`super::SafeUpdateOrchestrator`] through
//! all six phases using `/usr/bin/true` and `/usr/bin/false` as stand-in
//! "candidate binaries".

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

use super::*;
use crate::safe_update::snapshot::take_snapshot_of;
use crate::safe_update::state::{UpgradePhase, read_status};
use crate::safe_update::swap::atomic_install;
use crate::safe_update::validate::{
    ValidateMode, default_validate_timeout, enter_validation_if_needed, force_validate_timeout,
    record_cycle, validation_required,
};

fn which(name: &str) -> PathBuf {
    for d in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let p = PathBuf::from(d).join(name);
        if p.exists() {
            return p;
        }
    }
    panic!("{name} not found in standard PATH");
}

fn write_bin(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

#[test]
fn happy_path_drain_snapshot_pretest_swap_validate() {
    // Force handover to be skipped so we observe the swap + validate phases
    // without exec()ing into a stand-in binary.
    unsafe {
        std::env::set_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER", "1");
    }

    let state = tempdir().unwrap();
    let bin_dir = tempdir().unwrap();
    let src = tempdir().unwrap();

    // Pre-snapshot a starting binary so rollback is possible.
    let starting = write_bin(src.path(), "simard", b"simard 1.0.0 initial bytes");
    let snap = take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();

    // Candidate "binary" is /usr/bin/true — its self-test always exits 0.
    let true_bin = which("true");
    let install = src.path().join("install").join("simard");
    fs::create_dir_all(install.parent().unwrap()).unwrap();
    fs::write(&install, b"OLD INSTALL").unwrap();

    // Phase 1: drain (no engineers in flight).
    let isolated_engineers = tempdir().unwrap();
    let drain =
        drain::drain_to_quiescence_with_root(state.path(), 1, isolated_engineers.path()).unwrap();
    assert_eq!(drain.in_flight_at_end, 0);
    assert!(state.path().join("draining.flag").exists());

    // Phase 3: pre-test the candidate.
    let pretest = pretest::run_pretest(&true_bin, state.path(), 5).unwrap();
    assert!(pretest.passed);
    assert!(state.path().join("last-pretest.log").exists());

    // Phase 4: swap. We test the install step directly (without going
    // through `do_swap`'s status writer), then mark exec_handover by hand
    // because the candidate is `/usr/bin/true`, not a real Simard binary.
    fs::copy(&true_bin, src.path().join("candidate")).unwrap();
    let candidate = src.path().join("candidate");
    let outcome = atomic_install(&candidate, &install).unwrap();
    assert!(outcome.atomic_rename_used);

    let status = state::UpgradeStatus::exec_handover(
        Some("9.9.9".into()),
        Some(snap.version.clone()),
        2,
        600,
    );
    state::write_status(state.path(), &status).unwrap();

    // Phase 5: validate. Two clean cycles → Validated, draining flag cleared.
    assert!(validation_required(state.path()).unwrap());
    let t0 = chrono::DateTime::parse_from_rfc3339(&status.started_at)
        .unwrap()
        .timestamp();
    let r1 = record_cycle(state.path(), t0 + 1).unwrap();
    assert_eq!(
        r1,
        ValidateMode::InProgress {
            cycles_remaining: 1
        }
    );
    let r2 = record_cycle(state.path(), t0 + 2).unwrap();
    assert_eq!(r2, ValidateMode::Validated);
    assert!(!state.path().join("draining.flag").exists());
    assert!(state.path().join("upgrade-heartbeat.json").exists());

    let final_status = read_status(state.path()).unwrap().unwrap();
    assert_eq!(final_status.phase, UpgradePhase::Validated);

    unsafe {
        std::env::remove_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER");
    }
}

#[test]
fn negative_pretest_failure_aborts_before_swap() {
    unsafe {
        std::env::set_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER", "1");
    }
    let state = tempdir().unwrap();
    let bin_dir = tempdir().unwrap();
    let src = tempdir().unwrap();
    let starting = write_bin(src.path(), "simard", b"simard 1.0.0 initial");
    let _snap = take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();

    let install = src.path().join("install").join("simard");
    fs::create_dir_all(install.parent().unwrap()).unwrap();
    let original_install_bytes = b"GOOD INSTALL BYTES";
    fs::write(&install, original_install_bytes).unwrap();

    // Candidate that fails its self-test.
    let false_bin = which("false");

    let isolated_engineers = tempdir().unwrap();
    let cfg = UpdateConfig {
        drain_timeout_seconds: 1,
        pretest_timeout_seconds: 5,
        validate_timeout_cycles: 2,
        validate_timeout_seconds: 600,
        state_dir: state.path().to_path_buf(),
        engineer_worktrees_root: Some(isolated_engineers.path().to_path_buf()),
        ..UpdateConfig::default()
    };
    let orch = SafeUpdateOrchestrator::new(cfg, false_bin, install.clone());
    let err = orch.run().unwrap_err();
    assert!(matches!(err, SafeUpdateError::PretestSelfTestFailed { .. }));

    // The install path was NOT modified (atomic-swap discipline).
    let after = fs::read(&install).unwrap();
    assert_eq!(after, original_install_bytes);

    // Phase recorded as pretest_failed.
    let status = read_status(state.path()).unwrap().unwrap();
    assert_eq!(status.phase, UpgradePhase::PretestFailed);

    unsafe {
        std::env::remove_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER");
    }
}

#[test]
fn negative_validate_timeout_then_rollback_restores_backup() {
    unsafe {
        std::env::set_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER", "1");
    }
    let state = tempdir().unwrap();
    let bin_dir = tempdir().unwrap();
    let src = tempdir().unwrap();
    let starting_bytes = b"simard 1.0.0 ORIGINAL VALID BINARY".to_vec();
    let starting = write_bin(src.path(), "simard", &starting_bytes);
    let snap = take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();

    // Stand-in install path.
    let install = src.path().join("install").join("simard");
    fs::create_dir_all(install.parent().unwrap()).unwrap();
    fs::write(&install, b"NEW BAD BYTES").unwrap();

    // Pretend a swap to exec_handover.
    let status = state::UpgradeStatus::exec_handover(
        Some("9.9.9".into()),
        Some(snap.version.clone()),
        5,
        60, // tight budget
    );
    state::write_status(state.path(), &status).unwrap();

    // Drive cycle past the budget → ValidateTimeout.
    let t0 = chrono::DateTime::parse_from_rfc3339(&status.started_at)
        .unwrap()
        .timestamp();
    let r = record_cycle(state.path(), t0 + 120).unwrap();
    assert_eq!(r, ValidateMode::Timeout);

    // Watchdog observes timeout → triggers rollback.
    let outcome = rollback::do_rollback_with_bin_dir(
        state.path(),
        &install,
        bin_dir.path(),
        "validate_timeout: budget exceeded",
        None,
    )
    .unwrap();
    assert_eq!(outcome.backup_used, snap.backup_path);

    let restored = fs::read(&install).unwrap();
    assert_eq!(restored, starting_bytes);
    let final_status = read_status(state.path()).unwrap().unwrap();
    assert_eq!(final_status.phase, UpgradePhase::RolledBack);

    unsafe {
        std::env::remove_var("SIMARD_SAFE_UPDATE_SKIP_HANDOVER");
    }
}

#[test]
fn negative_drain_timeout_keeps_flag_in_place() {
    let state = tempdir().unwrap();
    let engineers = tempdir().unwrap();
    // Fake engineer worktree without a pid file → counted as in-flight.
    fs::create_dir_all(engineers.path().join("pretend-engineer")).unwrap();
    let err = drain::drain_to_quiescence_with_root(state.path(), 1, engineers.path()).unwrap_err();
    assert!(matches!(err, SafeUpdateError::DrainTimeout { .. }));
    // Flag deliberately remains set so new dispatches stay refused.
    assert!(state.path().join("draining.flag").exists());
}

#[test]
fn watchdog_force_timeout_then_rollback() {
    let state = tempdir().unwrap();
    let bin_dir = tempdir().unwrap();
    let src = tempdir().unwrap();
    let starting = write_bin(src.path(), "simard", b"simard 1.0.0 starting");
    let snap = take_snapshot_of(&starting, state.path(), 5, bin_dir.path().to_path_buf()).unwrap();

    // Phase = exec_handover, no cycles yet.
    let status = state::UpgradeStatus::exec_handover(
        Some("9.9.9".into()),
        Some(snap.version.clone()),
        5,
        default_validate_timeout(),
    );
    state::write_status(state.path(), &status).unwrap();

    // Watchdog forces the timeout (e.g. heartbeat stale).
    force_validate_timeout(state.path(), "watchdog: heartbeat stale > 90s").unwrap();
    assert_eq!(
        enter_validation_if_needed(state.path()).unwrap(),
        ValidateMode::Timeout
    );

    let install = src.path().join("install").join("simard");
    fs::create_dir_all(install.parent().unwrap()).unwrap();
    fs::write(&install, b"BAD BYTES").unwrap();

    let outcome = rollback::do_rollback_with_bin_dir(
        state.path(),
        &install,
        bin_dir.path(),
        "validate_timeout",
        None,
    )
    .unwrap();
    assert_eq!(outcome.backup_used, snap.backup_path);
    let final_status = read_status(state.path()).unwrap().unwrap();
    assert_eq!(final_status.phase, UpgradePhase::RolledBack);
}
