//! End-to-end convergence contract for the stuck-quarantine deadlock (#4469).
//!
//! A genuinely-stuck `cognitive*.corrupt-*` quarantine freezes self-deploy: the
//! `no_quarantine` health probe never clears, yet the #2550 retention rule
//! protects the recovery asset from deletion. The durable acknowledgement path
//! breaks the deadlock **without destroying data** — acknowledging a quarantine
//! writes a `.ack` sidecar that lets the probe (and the cleanup sweep) treat the
//! artifact as "seen" while the quarantined store itself is retained on disk.
//!
//! These tests exercise the public `simard::self_deploy` acknowledgement API end
//! to end against a hermetic state root. They are the outside-in specification
//! for the convergence path; they FAIL until #4469 is implemented and PASS once
//! the durable ack sidecar is in place.

use simard::self_deploy::{ack_marker_path, acknowledge, is_ack_marker_name, is_acknowledged};

const QUARANTINE: &str = "cognitive.corrupt-20260101120000";

/// Seed a hermetic state root with a substantial, stuck quarantine artifact and
/// return the guard plus the artifact path. The `HermeticState` pins
/// `SIMARD_STATE_ROOT` to a tempdir for the duration of the test.
fn seed_stuck_quarantine() -> (simard::test_support::HermeticState, std::path::PathBuf) {
    let hermetic = simard::test_support::HermeticState::new();
    let artifact = hermetic.state_root().join(QUARANTINE);
    // A multi-MB recovery asset: exactly the kind #2550 protects and refuses to
    // delete, so the ONLY way to converge is a non-destructive acknowledgement.
    std::fs::write(&artifact, vec![0u8; 2 * 1024 * 1024]).unwrap();
    (hermetic, artifact)
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn acknowledge_is_durable_and_retains_the_recovery_asset() {
    let (hermetic, artifact) = seed_stuck_quarantine();
    let root = hermetic.state_root();

    // Before: the quarantine is unacknowledged (the probe would fail here).
    assert!(!is_acknowledged(root, QUARANTINE));

    // Acknowledge: writes a durable sidecar under the SAME state root the probe
    // scans, and returns that path.
    let marker = acknowledge(root, QUARANTINE).expect("acknowledge succeeds");
    let expected = ack_marker_path(root, QUARANTINE).expect("valid quarantine name");
    assert_eq!(marker, expected, "marker path must match ack_marker_path");
    assert_eq!(
        marker.parent(),
        Some(root),
        "marker lives under the state root"
    );
    assert!(is_ack_marker_name(
        &marker.file_name().unwrap().to_string_lossy()
    ));

    // After: acknowledged, and the recovery asset is RETAINED (not deleted).
    assert!(is_acknowledged(root, QUARANTINE));
    assert!(marker.is_file(), "sidecar is a durable regular file");
    assert!(
        artifact.is_file(),
        "the quarantined recovery asset must be retained for recovery"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn acknowledge_is_idempotent_across_repeated_convergence_attempts() {
    let (hermetic, _artifact) = seed_stuck_quarantine();
    let root = hermetic.state_root();

    let first = acknowledge(root, QUARANTINE).unwrap();
    let second = acknowledge(root, QUARANTINE).unwrap();
    assert_eq!(first, second, "repeated ack is idempotent");
    assert!(is_acknowledged(root, QUARANTINE));

    // Exactly one durable marker exists — no accumulation across OODA cycles.
    let markers = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| is_ack_marker_name(&e.file_name().to_string_lossy()))
        .count();
    assert_eq!(markers, 1, "acknowledgement must not accumulate markers");
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn fresh_corruption_after_ack_is_not_silenced() {
    let (hermetic, _artifact) = seed_stuck_quarantine();
    let root = hermetic.state_root();

    acknowledge(root, QUARANTINE).unwrap();
    assert!(is_acknowledged(root, QUARANTINE));

    // A NEW corruption event lands under the same root. Filename-keyed markers
    // must not mark the fresh artifact as acknowledged.
    let fresh = "cognitive.corrupt-20260202235959";
    std::fs::write(root.join(fresh), vec![0u8; 1024]).unwrap();
    assert!(
        !is_acknowledged(root, fresh),
        "a prior ack must never silence a new corruption event"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn acknowledge_rejects_unsafe_names_end_to_end() {
    let hermetic = simard::test_support::HermeticState::new();
    let root = hermetic.state_root();

    // Path traversal / separators / absolute paths / non-quarantine names are
    // all refused, so an operator (or a compromised caller) cannot use the ack
    // path to write outside the state root or silence the live store.
    for bad in [
        "../escape",
        "sub/cognitive.corrupt-1",
        "/etc/passwd",
        "cognitive",     // the live store, not a quarantine
        "cognitive.wal", // live WAL, not a quarantine
    ] {
        assert!(
            acknowledge(root, bad).is_err(),
            "acknowledge must reject unsafe/non-quarantine name: {bad:?}"
        );
        assert!(ack_marker_path(root, bad).is_none());
    }
}
