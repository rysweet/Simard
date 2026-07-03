//! Tests for [`super::restart`]: the [`DaemonRestarter`] seam, the recording
//! [`FakeRestarter`] (used by tests and the recipe so no real daemon is
//! restarted), and the production restarter's stub.

use super::restart::{
    DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS, DaemonRestarter, FakeRestarter, SelfRelaunchInterval,
    SystemdOrExecRestarter, self_relaunch_min_interval_from_env, should_request_self_relaunch,
};

#[test]
fn fake_restarter_records_calls_and_succeeds() {
    let r = FakeRestarter::new();
    assert_eq!(r.restart_count(), 0);
    r.restart().unwrap();
    r.restart().unwrap();
    assert_eq!(r.restart_count(), 2);
    assert_eq!(r.kind(), "fake");
}

#[test]
fn failing_fake_restarter_errors_but_still_records() {
    let r = FakeRestarter::failing();
    let err = r.restart().unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("restart"),
        "error should mention restart, got: {err}"
    );
    // The call is still recorded so the orchestrator can observe the attempt.
    assert_eq!(r.restart_count(), 1);
}

#[test]
fn fake_restarter_is_usable_as_boxed_trait_object() {
    // The orchestrator takes `Box<dyn DaemonRestarter>` by injection.
    let r: Box<dyn DaemonRestarter> = Box::new(FakeRestarter::new());
    r.restart().unwrap();
    assert_eq!(r.kind(), "fake");
}

#[test]
fn systemd_or_exec_restarter_reports_its_kind() {
    let r = SystemdOrExecRestarter::new();
    assert_eq!(r.kind(), "systemd-or-exec");
}

#[test]
fn self_relaunch_interval_env_uses_day_default_for_empty_or_garbage() {
    assert_eq!(
        self_relaunch_min_interval_from_env(None),
        SelfRelaunchInterval::Seconds(DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS)
    );
    assert_eq!(
        self_relaunch_min_interval_from_env(Some("")),
        SelfRelaunchInterval::Seconds(DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS)
    );
    assert_eq!(
        self_relaunch_min_interval_from_env(Some("garbage")),
        SelfRelaunchInterval::Seconds(DEFAULT_SELF_RELAUNCH_MIN_INTERVAL_SECS)
    );
}

#[test]
fn self_relaunch_interval_env_honors_override_and_off() {
    assert_eq!(
        self_relaunch_min_interval_from_env(Some("7200")),
        SelfRelaunchInterval::Seconds(7200)
    );
    assert_eq!(
        self_relaunch_min_interval_from_env(Some("0")),
        SelfRelaunchInterval::Off
    );
    assert_eq!(
        self_relaunch_min_interval_from_env(Some("off")),
        SelfRelaunchInterval::Off
    );
}

#[test]
fn should_request_self_relaunch_allows_real_binary_change_immediately() {
    assert!(should_request_self_relaunch(
        1_000,
        Some(999),
        SelfRelaunchInterval::Seconds(86_400),
        true,
    ));
    assert!(should_request_self_relaunch(
        1_000,
        Some(999),
        SelfRelaunchInterval::Off,
        true,
    ));
}

#[test]
fn should_request_self_relaunch_throttles_interval_only_restarts() {
    assert!(!should_request_self_relaunch(
        1_000,
        Some(900),
        SelfRelaunchInterval::Seconds(300),
        false,
    ));
    assert!(should_request_self_relaunch(
        1_200,
        Some(900),
        SelfRelaunchInterval::Seconds(300),
        false,
    ));
    assert!(!should_request_self_relaunch(
        1_200,
        Some(900),
        SelfRelaunchInterval::Off,
        false,
    ));
}

#[test]
#[ignore = "TDD pending: restart.rs SystemdOrExecRestarter::restart (Workstream A)"]
fn systemd_or_exec_restarter_restart_requests_a_restart() {
    // When implemented: prefer `systemctl --user restart simard-ooda`, else the
    // coordinated exec() handover. Never run against a real daemon in tests.
    SystemdOrExecRestarter::new().restart().unwrap();
}
