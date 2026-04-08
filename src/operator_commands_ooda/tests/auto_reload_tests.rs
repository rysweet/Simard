use std::time::{Duration, SystemTime};

use crate::operator_commands_ooda::daemon::binary_changed;

#[test]
fn binary_not_changed_when_start_time_is_recent() {
    // A start_time captured "now" should be >= the binary mtime, so no reload.
    let now = SystemTime::now();
    assert!(!binary_changed(now));
}

#[test]
fn binary_changed_when_start_time_is_old() {
    // A start_time far in the past should always be older than the on-disk
    // binary, so `binary_changed` should return true.
    let epoch = SystemTime::UNIX_EPOCH;
    assert!(binary_changed(epoch));
}

#[test]
fn binary_not_changed_when_start_time_is_in_future() {
    // A start_time in the future can never be exceeded by any real mtime.
    let future = SystemTime::now() + Duration::from_secs(86_400);
    assert!(!binary_changed(future));
}
