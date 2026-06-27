//! Tests for [`super::orchestrator`] plus the self-deploy extensions to
//! `UpdateConfig` and `SafeUpdateError` (Workstream A contract surface).
//!
//! The pure config/error contracts are covered now; the load-bearing `run()`
//! sequence — build → gate → backup → drain → reap → swap → restart → health →
//! rollback — is pinned by an `#[ignore]`d end-to-end test pending the
//! Workstream A implementation.

use std::path::PathBuf;

use crate::safe_update::{SafeUpdateError, UpdateConfig};

use super::orchestrator::{DeploySourceKind, SelfDeployOrchestrator};
use super::restart::FakeRestarter;
// --- DeploySourceKind -------------------------------------------------------

#[test]
fn deploy_source_kind_defaults_to_build_from_source() {
    assert_eq!(
        DeploySourceKind::default(),
        DeploySourceKind::BuildFromSource
    );
    assert_ne!(
        DeploySourceKind::BuildFromSource,
        DeploySourceKind::ReleaseDownload
    );
}

// --- UpdateConfig self-deploy fields (documented defaults) -------------------

#[test]
fn update_config_self_deploy_defaults_match_spec() {
    // docs/reference/self-deploy-api.md#updateconfig-self-deploy-fields.
    let cfg = UpdateConfig::default();
    assert_eq!(cfg.deploy_source, DeploySourceKind::BuildFromSource);
    assert!(cfg.memory_backup_required);
    assert_eq!(cfg.orphan_kill_grace_seconds, 10);
    assert_eq!(cfg.health_probe_cycles, 1);
    assert_eq!(cfg.memory_count_tolerance, 0);
}

// --- New SafeUpdateError variants surface loudly ----------------------------

#[test]
fn new_safe_update_error_variants_display_loudly() {
    let cases: Vec<(SafeUpdateError, &str)> = vec![
        (
            SafeUpdateError::BuildFailed {
                detail: "cargo build --release exited 101".to_string(),
            },
            "build",
        ),
        (
            SafeUpdateError::GateFailed {
                gate: "GymBaseline".to_string(),
                detail: "regressed".to_string(),
            },
            "gate",
        ),
        (
            SafeUpdateError::BackupFailed {
                which: "memory".to_string(),
                detail: "snapshot export failed".to_string(),
            },
            "backup",
        ),
        (SafeUpdateError::OrphanReapTimeout { pid: 4242 }, "4242"),
        (
            SafeUpdateError::HealthCheckFailed {
                report: "memory_intact unhealthy".to_string(),
            },
            "health",
        ),
        (
            SafeUpdateError::RolledBack {
                reason: "health check failed".to_string(),
            },
            "roll",
        ),
        (
            SafeUpdateError::RollbackFailed {
                detail: "restart did not come back".to_string(),
            },
            "rollback",
        ),
    ];
    for (err, needle) in cases {
        let s = err.to_string().to_lowercase();
        assert!(
            s.contains(needle),
            "Display for {err:?} should contain {needle:?}, got: {s}"
        );
    }
}

// --- Orchestrator construction + end-to-end (pending) -----------------------

#[test]
fn orchestrator_constructs_with_injected_fake_restarter() {
    // Construction is pure; running is the effectful part exercised below.
    let _orch = SelfDeployOrchestrator::new(
        UpdateConfig::default(),
        Box::new(FakeRestarter::new()),
        "deadbeefcafe".to_string(),
        PathBuf::from("/home/simard/.simard/bin/simard"),
    );
}

#[test]
#[ignore = "TDD pending: orchestrator.rs run() build->...->health->rollback (Workstream A)"]
fn run_rolls_back_to_backup_binary_when_health_check_fails() {
    // When implemented and the post-deploy health check fails, run() must
    // restore the backup binary, restart, and return SafeUpdateError::RolledBack
    // (never leave a half-deployed daemon). The recipe injects a FakeRestarter
    // so no real daemon is restarted.
    let orch = SelfDeployOrchestrator::new(
        UpdateConfig::default(),
        Box::new(FakeRestarter::new()),
        "deadbeefcafe".to_string(),
        PathBuf::from("/home/simard/.simard/bin/simard"),
    );
    let err = orch.run().unwrap_err();
    assert!(matches!(err, SafeUpdateError::RolledBack { .. }));
}

#[test]
#[ignore = "TDD pending: orchestrator.rs run() happy path verified-running (Workstream A)"]
fn run_succeeds_and_reports_verified_running() {
    // When implemented and all probes pass, run() returns a SelfDeployOutcome
    // whose health report is healthy and whose restarter_kind is recorded.
    let orch = SelfDeployOrchestrator::new(
        UpdateConfig::default(),
        Box::new(FakeRestarter::new()),
        "deadbeefcafe".to_string(),
        PathBuf::from("/home/simard/.simard/bin/simard"),
    );
    let outcome = orch.run().unwrap();
    assert!(outcome.health.healthy);
    assert_eq!(outcome.restarter_kind, "fake");
}

// --- Hermetic sequence coverage via injected fake effects -------------------
//
// These exercise the load-bearing `run_sequence` end-to-end (build → gate →
// backup → drain → reap → swap → restart → health → rollback) WITHOUT building
// from source or restarting a daemon, so the ordering, the health gate, and the
// rollback tail are genuinely tested in CI.

mod fake_sequence {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use crate::safe_update::SafeUpdateError;
    use crate::self_deploy::backup::ProtectiveBackup;
    use crate::self_deploy::health::{
        BrainsLlmBackedProbe, GoalBoardIntactProbe, MemoryIntactProbe, NoQuarantineProbe,
        SelfHealthProbes, SelfHealthReport, VersionAdvancedProbe,
    };
    use crate::self_deploy::orchestrator::{DeployEffects, run_sequence};

    /// What outcome a fake step should produce.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Ok,
        Unhealthy,
        Err,
    }

    struct FakeEffects {
        calls: RefCell<Vec<&'static str>>,
        restart: Mode,
        health: Mode,
        rollback: Mode,
    }

    impl FakeEffects {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                restart: Mode::Ok,
                health: Mode::Ok,
                rollback: Mode::Ok,
            }
        }
        fn record(&self, step: &'static str) {
            self.calls.borrow_mut().push(step);
        }
        fn order(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    fn report(healthy: bool) -> SelfHealthReport {
        SelfHealthReport::compute(SelfHealthProbes {
            version_advanced: VersionAdvancedProbe {
                healthy,
                running: "deadbeef".into(),
                target: "deadbeef".into(),
            },
            memory_intact: MemoryIntactProbe {
                healthy,
                live_facts: 1206,
                baseline_facts: Some(1206),
            },
            goal_board_intact: GoalBoardIntactProbe {
                healthy: true,
                active_goals: 5,
            },
            brains_llm_backed: BrainsLlmBackedProbe {
                healthy: true,
                fallback_records: 0,
            },
            no_quarantine: NoQuarantineProbe {
                healthy: true,
                quarantined: false,
            },
        })
    }

    fn a_backup() -> ProtectiveBackup {
        ProtectiveBackup {
            memory_snapshot: PathBuf::from("/tmp/mem.json"),
            binary_backup: PathBuf::from("/tmp/simard.bak"),
        }
    }

    impl DeployEffects for FakeEffects {
        fn build_candidate(&self) -> Result<PathBuf, SafeUpdateError> {
            self.record("build");
            Ok(PathBuf::from("/tmp/candidate"))
        }
        fn gate_candidate(&self, _candidate: &Path) -> Result<(), SafeUpdateError> {
            self.record("gate");
            Ok(())
        }
        fn capture_baseline_facts(&self) -> Option<u64> {
            self.record("baseline");
            Some(1206)
        }
        fn take_backup(&self) -> Result<ProtectiveBackup, SafeUpdateError> {
            self.record("backup");
            Ok(a_backup())
        }
        fn drain(&self) -> Result<(), SafeUpdateError> {
            self.record("drain");
            Ok(())
        }
        fn reap_orphans(&self) -> Result<usize, SafeUpdateError> {
            self.record("reap");
            Ok(2)
        }
        fn swap(&self, _candidate: &Path) -> Result<(), SafeUpdateError> {
            self.record("swap");
            Ok(())
        }
        fn restart(&self) -> Result<(), SafeUpdateError> {
            self.record("restart");
            match self.restart {
                Mode::Err => Err(SafeUpdateError::SwapHandoverFailed {
                    reason: "fake restart failure".into(),
                }),
                _ => Ok(()),
            }
        }
        fn health_check(
            &self,
            _baseline_facts: Option<u64>,
        ) -> Result<SelfHealthReport, SafeUpdateError> {
            self.record("health");
            Ok(report(self.health != Mode::Unhealthy))
        }
        fn rollback(&self, _reason: &str) -> Result<(), SafeUpdateError> {
            self.record("rollback");
            match self.rollback {
                Mode::Err => Err(SafeUpdateError::RollbackRestoreFailed {
                    path: PathBuf::from("/tmp/simard"),
                    reason: "fake rollback failure".into(),
                }),
                _ => Ok(()),
            }
        }
        fn restarter_kind(&self) -> &'static str {
            "fake"
        }
    }

    #[test]
    fn happy_path_runs_steps_in_order_and_reports_verified() {
        let fx = FakeEffects::new();
        let outcome = run_sequence(&fx).unwrap();
        assert!(outcome.health.healthy);
        assert_eq!(outcome.reaped_orphans, 2);
        assert_eq!(outcome.restarter_kind, "fake");
        assert_eq!(
            fx.order(),
            vec![
                "build", "gate", "baseline", "backup", "drain", "reap", "swap", "restart", "health"
            ],
            "load-bearing order must hold; no rollback on success"
        );
    }

    #[test]
    fn unhealthy_health_check_triggers_rollback() {
        let mut fx = FakeEffects::new();
        fx.health = Mode::Unhealthy;
        let err = run_sequence(&fx).unwrap_err();
        assert!(matches!(err, SafeUpdateError::RolledBack { .. }), "{err:?}");
        assert!(
            fx.order().contains(&"rollback"),
            "rollback must run after a failed health check"
        );
        // Backups/swap happened before the rollback; health was the last gate.
        let order = fx.order();
        assert_eq!(order.last(), Some(&"rollback"));
    }

    #[test]
    fn failed_restart_triggers_rollback_and_skips_health() {
        let mut fx = FakeEffects::new();
        fx.restart = Mode::Err;
        let err = run_sequence(&fx).unwrap_err();
        assert!(matches!(err, SafeUpdateError::RolledBack { .. }), "{err:?}");
        let order = fx.order();
        assert!(order.contains(&"rollback"));
        assert!(
            !order.contains(&"health"),
            "health must be skipped when restart fails"
        );
    }

    #[test]
    fn rollback_failure_is_surfaced_as_rollback_failed() {
        let mut fx = FakeEffects::new();
        fx.health = Mode::Unhealthy;
        fx.rollback = Mode::Err;
        let err = run_sequence(&fx).unwrap_err();
        assert!(
            matches!(err, SafeUpdateError::RollbackFailed { .. }),
            "a failed rollback must surface as the critical RollbackFailed: {err:?}"
        );
    }

    #[test]
    fn build_failure_aborts_before_any_daemon_mutation() {
        struct BuildFails;
        impl DeployEffects for BuildFails {
            fn build_candidate(&self) -> Result<PathBuf, SafeUpdateError> {
                Err(SafeUpdateError::BuildFailed {
                    detail: "cargo build --release exited 101".into(),
                })
            }
            fn gate_candidate(&self, _c: &Path) -> Result<(), SafeUpdateError> {
                panic!("gate must not run after a build failure")
            }
            fn capture_baseline_facts(&self) -> Option<u64> {
                panic!("no baseline after build failure")
            }
            fn take_backup(&self) -> Result<ProtectiveBackup, SafeUpdateError> {
                panic!("no backup after build failure")
            }
            fn drain(&self) -> Result<(), SafeUpdateError> {
                panic!("no drain after build failure")
            }
            fn reap_orphans(&self) -> Result<usize, SafeUpdateError> {
                panic!("no reap after build failure")
            }
            fn swap(&self, _c: &Path) -> Result<(), SafeUpdateError> {
                panic!("no swap after build failure")
            }
            fn restart(&self) -> Result<(), SafeUpdateError> {
                panic!("no restart after build failure")
            }
            fn health_check(&self, _b: Option<u64>) -> Result<SelfHealthReport, SafeUpdateError> {
                panic!("no health after build failure")
            }
            fn rollback(&self, _r: &str) -> Result<(), SafeUpdateError> {
                panic!("no rollback after build failure")
            }
            fn restarter_kind(&self) -> &'static str {
                "fake"
            }
        }
        let err = run_sequence(&BuildFails).unwrap_err();
        assert!(
            matches!(err, SafeUpdateError::BuildFailed { .. }),
            "{err:?}"
        );
    }
}
