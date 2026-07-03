use std::time::{Duration, SystemTime};

use crate::operator_commands_ooda::daemon::binary_changed;

#[test]
fn binary_not_changed_when_start_time_is_recent() {
    // A start_time captured "now" should be >= the binary mtime, so no reload.
    let now = SystemTime::now();
    assert!(!binary_changed(now));
}

#[test]
fn binary_not_changed_on_identical_content_even_with_old_start_time() {
    // Content-identity gate (2026-07-02 operator-review #2): a start_time far in
    // the past makes the mtime pre-filter fire, but the on-disk binary IS the
    // running test binary (identical content — nothing rebuilds it mid-test), so
    // the gate must NOT relaunch. The old mtime-only check returned `true` here
    // and was the ~40–45 min self-restart churn bug; the new check returns
    // `false` on a byte-identical image.
    let epoch = SystemTime::UNIX_EPOCH;
    assert!(
        !binary_changed(epoch),
        "identical on-disk content must not relaunch, even with an epoch start time"
    );
}

#[test]
fn binary_not_changed_when_start_time_is_in_future() {
    // A start_time in the future can never be exceeded by any real mtime.
    let future = SystemTime::now() + Duration::from_secs(86_400);
    assert!(!binary_changed(future));
}
