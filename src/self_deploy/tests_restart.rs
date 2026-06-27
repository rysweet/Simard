//! Tests for [`super::restart`]: the [`DaemonRestarter`] seam, the recording
//! [`FakeRestarter`] (used by tests and the recipe so no real daemon is
//! restarted), and the production restarter's stub.

use super::restart::{DaemonRestarter, FakeRestarter, SystemdOrExecRestarter};

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
#[ignore = "TDD pending: restart.rs SystemdOrExecRestarter::restart (Workstream A)"]
fn systemd_or_exec_restarter_restart_requests_a_restart() {
    // When implemented: prefer `systemctl --user restart simard-ooda`, else the
    // coordinated exec() handover. Never run against a real daemon in tests.
    SystemdOrExecRestarter::new().restart().unwrap();
}
