//! Tests for [`super::health`]: the `SelfHealthReport` AND-of-probes invariant
//! and the durable `simard self-health --json` shape.

use super::health::{
    BrainsLlmBackedProbe, EntrypointParityProbe, GoalBoardIntactProbe, MemoryIntactProbe,
    NoQuarantineProbe, SelfHealthProbes, SelfHealthReport, VersionAdvancedProbe,
};

fn all_healthy_probes() -> SelfHealthProbes {
    SelfHealthProbes {
        version_advanced: VersionAdvancedProbe {
            healthy: true,
            running: "deadbeef".to_string(),
            target: "deadbeef".to_string(),
        },
        memory_intact: MemoryIntactProbe {
            healthy: true,
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
            fresh_quarantines: 0,
            retained: 0,
        },
        entrypoint_parity: EntrypointParityProbe {
            healthy: true,
            installed_version: "simard 0.35.0".to_string(),
            path_version: "simard 0.35.0".to_string(),
            resolved_path: "/home/you/.local/bin/simard".to_string(),
            canonical_path: "/home/you/.simard/bin/simard".to_string(),
            path_mismatch: false,
            foreign_shadow: false,
        },
    }
}

#[test]
fn report_is_healthy_only_when_every_probe_is_healthy() {
    let report = SelfHealthReport::compute(all_healthy_probes());
    assert!(report.healthy);
    assert!(report.is_healthy());
    assert!(report.probes.all_healthy());
}

#[test]
fn any_single_unhealthy_probe_fails_the_report() {
    // Each probe in turn flips the whole report unhealthy (AND semantics).
    let mut p = all_healthy_probes();
    p.memory_intact.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.version_advanced.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.goal_board_intact.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.brains_llm_backed.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.no_quarantine.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);

    let mut p = all_healthy_probes();
    p.entrypoint_parity.healthy = false;
    assert!(!SelfHealthReport::compute(p).healthy);
}

#[test]
fn report_serializes_with_documented_top_level_keys() {
    let report = SelfHealthReport::compute(all_healthy_probes());
    let v: serde_json::Value = serde_json::to_value(&report).unwrap();
    assert!(v.get("healthy").is_some());
    let probes = v.get("probes").expect("probes object");
    for key in [
        "version_advanced",
        "memory_intact",
        "goal_board_intact",
        "brains_llm_backed",
        "no_quarantine",
        "entrypoint_parity",
    ] {
        assert!(probes.get(key).is_some(), "probes.{key} must be present");
    }
}

#[test]
fn report_deserializes_from_documented_json_and_is_unhealthy() {
    // Pin the durable schema from docs/reference/self-deploy-api.md#self-health-output.
    let doc_json = r#"{
        "healthy": false,
        "probes": {
            "version_advanced": { "healthy": true,  "running": "abc123", "target": "abc123" },
            "memory_intact":    { "healthy": false, "live_facts": 1180, "baseline_facts": 1206 },
            "goal_board_intact":{ "healthy": true,  "active_goals": 5 },
            "brains_llm_backed":{ "healthy": true,  "fallback_records": 0 },
            "no_quarantine":    { "healthy": true,  "quarantined": false, "fresh_quarantines": 0, "retained": 3 }
        }
    }"#;
    let report: SelfHealthReport = serde_json::from_str(doc_json).unwrap();
    assert!(!report.healthy);
    assert_eq!(report.probes.memory_intact.live_facts, 1180);
    assert_eq!(report.probes.memory_intact.baseline_facts, Some(1206));
    assert!(!report.probes.memory_intact.healthy);
    assert_eq!(report.probes.goal_board_intact.active_goals, 5);
    assert_eq!(report.probes.brains_llm_backed.fallback_records, 0);
    assert!(!report.probes.no_quarantine.quarantined);
    assert_eq!(report.probes.no_quarantine.fresh_quarantines, 0);
    assert_eq!(report.probes.no_quarantine.retained, 3);
    // The AND invariant agrees with the recorded top-level flag.
    assert_eq!(report.healthy, report.probes.all_healthy());
}

#[test]
fn memory_intact_baseline_is_optional() {
    // When `--pre-deploy-facts` is omitted the probe reports the live count only.
    let json = r#"{ "healthy": true, "live_facts": 900, "baseline_facts": null }"#;
    let probe: MemoryIntactProbe = serde_json::from_str(json).unwrap();
    assert_eq!(probe.baseline_facts, None);
    assert_eq!(probe.live_facts, 900);
}

// ── Root Cause A end-to-end: the `no_quarantine` probe scans the live-store
//    directory (`<state_root>/state/`) and applies the window filter ──────────
//
// These exercise `run_self_health_probe` against a real on-disk state root so
// they pin BOTH halves of Root Cause A:
//   * the probe scans `resolve_subdir("state")` (where the live `cognitive`
//     store and its quarantines actually live), NOT top-level `~/.simard`; and
//   * it only FAILS on a quarantine whose mtime is at/after the window start,
//     so retained historical snapshots let the probe clear.

use crate::journal::test_support::FakeMemory;
use crate::self_deploy::health::run_self_health_probe;
use crate::state_root::STATE_ROOT_ENV;

/// Scoped `SIMARD_STATE_ROOT` override that restores on drop. Env access is
/// process-global, so the tests below run under `#[serial]`.
struct StateRootGuard {
    prev: Option<std::ffi::OsString>,
}

impl StateRootGuard {
    fn set(value: &std::path::Path) -> Self {
        let prev = std::env::var_os(STATE_ROOT_ENV);
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            std::env::set_var(STATE_ROOT_ENV, value);
        }
        Self { prev }
    }
}

impl Drop for StateRootGuard {
    fn drop(&mut self) {
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(STATE_ROOT_ENV, v),
                None => std::env::remove_var(STATE_ROOT_ENV),
            }
        }
    }
}

fn set_mtime(path: &std::path::Path, when: std::time::SystemTime) {
    let times = std::fs::FileTimes::new().set_modified(when);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

/// Acceptance #2 + directory-targeting: a FRESH quarantine created under
/// `<state_root>/state/` (mtime ≥ window start) fails the `no_quarantine` probe.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn no_quarantine_fails_on_fresh_quarantine_in_state_dir() {
    let root = tempfile::tempdir().unwrap();
    let _g = StateRootGuard::set(root.path());
    let state = root.path().join("state");
    std::fs::create_dir_all(&state).unwrap();

    // A fresh quarantine (mtime ≈ now) inside the window.
    std::fs::write(state.join("cognitive.corrupt-20260722"), b"corrupt").unwrap();

    let window = chrono::Utc::now() - chrono::Duration::seconds(120);
    let mem = FakeMemory::new();
    let report = run_self_health_probe(&mem, "deadbeef", None, 0, window).unwrap();

    assert!(
        report.probes.no_quarantine.quarantined,
        "a fresh quarantine under <state_root>/state/ must be detected"
    );
    assert!(
        !report.probes.no_quarantine.healthy,
        "no_quarantine must FAIL on fresh corruption"
    );
    assert_eq!(
        report.probes.no_quarantine.fresh_quarantines, 1,
        "the single fresh quarantine must be counted"
    );
}

/// Acceptance #1: retained historical snapshots (mtime before the window) under
/// `<state_root>/state/` do NOT fail the probe — it reports healthy and can
/// finally clear.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn no_quarantine_passes_with_only_historical_quarantines_in_state_dir() {
    let root = tempfile::tempdir().unwrap();
    let _g = StateRootGuard::set(root.path());
    let state = root.path().join("state");
    std::fs::create_dir_all(&state).unwrap();

    // The window opens now; the retained snapshots below predate it.
    let window = chrono::Utc::now();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
    for i in 0..5 {
        let p = state.join(format!("cognitive.corrupt-{i:04}"));
        std::fs::write(&p, b"forensic").unwrap();
        set_mtime(&p, old);
    }
    // The live store files coexist and must not be mistaken for corruption.
    std::fs::write(state.join("cognitive"), b"live").unwrap();
    std::fs::write(state.join("cognitive.wal"), b"wal").unwrap();

    let mem = FakeMemory::new();
    let report = run_self_health_probe(&mem, "deadbeef", None, 0, window).unwrap();

    assert!(
        !report.probes.no_quarantine.quarantined,
        "historical retained snapshots must not be flagged as current corruption"
    );
    assert!(
        report.probes.no_quarantine.healthy,
        "no_quarantine must be HEALTHY when only historical snapshots exist"
    );
    assert_eq!(
        report.probes.no_quarantine.fresh_quarantines, 0,
        "no fresh quarantines exist"
    );
    assert_eq!(
        report.probes.no_quarantine.retained, 5,
        "all 5 historical snapshots must be surfaced as retained diagnostics"
    );
}

/// Directory-targeting regression: a fresh quarantine at TOP-LEVEL
/// `<state_root>/` (the pre-fix scan location) must NOT fail the probe, because
/// the live store and its quarantines live under `<state_root>/state/`. This
/// pins that probe and cleanup agree on the same live-store directory.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn no_quarantine_scans_state_dir_not_top_level() {
    let root = tempfile::tempdir().unwrap();
    let _g = StateRootGuard::set(root.path());
    // Empty live-store dir; the only quarantine is at the wrong (top) level.
    std::fs::create_dir_all(root.path().join("state")).unwrap();
    std::fs::write(root.path().join("cognitive.corrupt-toplevel"), b"corrupt").unwrap();

    let window = chrono::Utc::now() - chrono::Duration::seconds(120);
    let mem = FakeMemory::new();
    let report = run_self_health_probe(&mem, "deadbeef", None, 0, window).unwrap();

    assert!(
        !report.probes.no_quarantine.quarantined,
        "the probe must scan <state_root>/state/, not top-level <state_root>"
    );
    assert!(report.probes.no_quarantine.healthy);
}

/// Scoped `HOME` override that restores on drop. Env access is process-global,
/// so this runs under the same `#[serial]` key as the state-root tests.
struct HomeGuard {
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn set(value: &std::path::Path) -> Self {
        let prev = std::env::var_os("HOME");
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            std::env::set_var("HOME", value);
        }
        Self { prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: serialized via #[serial(simard_state_root_env, cognitive_memory)].
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// Deploy-gate exit-101 regression (rysweet/Simard#4506): the self-deploy health
/// probe must be **independent of the ambient `$HOME`**. The deploy-gate canary
/// runs `cargo test` under a `$HOME`/`SIMARD_STATE_ROOT` that differs from a
/// developer laptop; if the probe (or a test-body assertion) resolved paths from
/// `$HOME`, the on-disk layout would diverge and a `.unwrap()`/`assert!` would
/// panic — surfacing as `process didn't exit successfully: ... (exit status:
/// 101)`. Because the state root is pinned to a per-test `TempDir` via
/// `StateRootGuard`, a deliberately hostile `HOME=/nonexistent` must NOT change
/// the outcome: the probe sees exactly the files the test created and clears.
///
/// See docs/reference/deploy-gate-drop-test-state-root-robustness.md. This locks
/// the env-independence contract so a future change that reintroduces a `$HOME`
/// dependency fails here instead of red-canarying the deploy-gate.
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn health_probe_is_independent_of_ambient_home() {
    let root = tempfile::tempdir().unwrap();
    // A divergent, non-existent HOME mirrors the deploy-gate canary environment.
    let _home = HomeGuard::set(std::path::Path::new("/nonexistent"));
    let _g = StateRootGuard::set(root.path());
    let state = root.path().join("state");
    std::fs::create_dir_all(&state).unwrap();

    // Only historical (pre-window) snapshots exist: the probe must clear.
    let window = chrono::Utc::now();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
    let p = state.join("cognitive.corrupt-old");
    std::fs::write(&p, b"forensic").unwrap();
    set_mtime(&p, old);

    let mem = FakeMemory::new();
    let report = run_self_health_probe(&mem, "deadbeef", None, 0, window).unwrap();

    assert!(
        report.probes.no_quarantine.healthy,
        "under a hostile HOME the probe must still resolve <SIMARD_STATE_ROOT>/state \
         from the TempDir and clear (rysweet/Simard#4506 exit-101 regression)"
    );
    assert!(
        !report.probes.no_quarantine.quarantined,
        "no fresh quarantine exists; a $HOME-derived path divergence must not \
         invent one"
    );
}

/// A fresh quarantine must STILL be detected under a hostile `$HOME` — proving
/// the env-independence fix did not silently disable detection (the probe reads
/// the TempDir state root, not `$HOME`, in both the pass and fail directions).
#[test]
#[serial_test::serial(simard_state_root_env, cognitive_memory)]
fn health_probe_detects_fresh_quarantine_regardless_of_home() {
    let root = tempfile::tempdir().unwrap();
    let _home = HomeGuard::set(std::path::Path::new("/nonexistent"));
    let _g = StateRootGuard::set(root.path());
    let state = root.path().join("state");
    std::fs::create_dir_all(&state).unwrap();

    std::fs::write(state.join("cognitive.corrupt-20260722"), b"corrupt").unwrap();

    let window = chrono::Utc::now() - chrono::Duration::seconds(120);
    let mem = FakeMemory::new();
    let report = run_self_health_probe(&mem, "deadbeef", None, 0, window).unwrap();

    assert!(
        report.probes.no_quarantine.quarantined,
        "detection must read the TempDir state root, not $HOME, in both directions"
    );
    assert!(!report.probes.no_quarantine.healthy);
}
