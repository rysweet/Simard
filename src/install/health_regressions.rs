use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::health::{HealthCheckErrorKind, run};

#[cfg(unix)]
fn checker(root: &Path, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write checker");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod checker");
    path
}

#[cfg(unix)]
#[test]
fn health_check_accepts_only_a_completed_explicitly_healthy_response() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = checker(temp.path(), "healthy", "printf '%s' '{\"healthy\":true}'");
    let response = run(&path, Duration::from_secs(1)).expect("healthy");
    assert!(response.healthy);
}

#[cfg(unix)]
#[test]
fn health_check_classifies_every_failure_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cases = [
        (temp.path().join("missing"), HealthCheckErrorKind::Spawn),
        (
            checker(temp.path(), "nonzero", "exit 7"),
            HealthCheckErrorKind::NonzeroExit,
        ),
        (
            checker(temp.path(), "timeout", "sleep 2"),
            HealthCheckErrorKind::Timeout,
        ),
        (
            checker(temp.path(), "transport", "exit 0"),
            HealthCheckErrorKind::Transport,
        ),
        (
            checker(temp.path(), "malformed", "printf '%s' 'not-json'"),
            HealthCheckErrorKind::MalformedResponse,
        ),
        (
            checker(
                temp.path(),
                "unknown-field",
                "printf '%s' '{\"healthy\":true,\"detail\":\"ignored\"}'",
            ),
            HealthCheckErrorKind::MalformedResponse,
        ),
        (
            checker(
                temp.path(),
                "unhealthy",
                "printf '%s' '{\"healthy\":false}'",
            ),
            HealthCheckErrorKind::Unhealthy,
        ),
    ];

    for (path, expected) in cases {
        let error =
            run(&path, Duration::from_millis(50)).expect_err("health failure must be explicit");
        assert_eq!(error.kind(), expected, "checker {}", path.display());
    }
}
