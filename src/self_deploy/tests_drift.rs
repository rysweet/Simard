//! Tests for [`super::drift`]: the `DeployDrift` invariant and the
//! `ReconcileDetector` over an injected, hermetic `DeploySource`.

use std::collections::BTreeMap;

use crate::error::{SimardError, SimardResult};

use super::drift::{DeployDrift, DeploySource, ReconcileDetector};

/// Hermetic, fully-controllable `DeploySource` for tests.
#[derive(Default)]
struct FakeDeploySource {
    merged_head: String,
    running_commit: String,
    behind: usize,
    merged_pins: BTreeMap<String, String>,
    running_pins: BTreeMap<String, String>,
    /// When set, every accessor errors (models a transient git failure).
    fail: bool,
}

impl DeploySource for FakeDeploySource {
    fn merged_head(&self) -> SimardResult<String> {
        self.guard()?;
        Ok(self.merged_head.clone())
    }
    fn running_commit(&self) -> SimardResult<String> {
        self.guard()?;
        Ok(self.running_commit.clone())
    }
    fn behind_count(&self) -> SimardResult<usize> {
        self.guard()?;
        Ok(self.behind)
    }
    fn merged_pins(&self) -> SimardResult<BTreeMap<String, String>> {
        self.guard()?;
        Ok(self.merged_pins.clone())
    }
    fn running_pins(&self) -> SimardResult<BTreeMap<String, String>> {
        self.guard()?;
        Ok(self.running_pins.clone())
    }
}

impl FakeDeploySource {
    fn guard(&self) -> SimardResult<()> {
        if self.fail {
            return Err(SimardError::GitCommandFailed {
                command: "git rev-list".to_string(),
                reason: "fake transient failure".to_string(),
            });
        }
        Ok(())
    }
}

fn pins(items: &[(&str, &str)]) -> BTreeMap<String, String> {
    items
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// --- DeployDrift invariant --------------------------------------------------

#[test]
fn deploy_drift_needs_deploy_is_false_only_when_current() {
    let current = DeployDrift::from_parts(0, vec![]);
    assert!(!current.needs_deploy);
    assert_eq!(current, DeployDrift::current());
}

#[test]
fn deploy_drift_needs_deploy_true_on_commit_drift() {
    let d = DeployDrift::from_parts(3, vec![]);
    assert!(d.needs_deploy);
    assert_eq!(d.behind_commits, 3);
}

#[test]
fn deploy_drift_needs_deploy_true_on_pin_drift_only() {
    let d = DeployDrift::from_parts(0, vec!["amplihack-memory".to_string()]);
    assert!(d.needs_deploy, "pin drift alone must require a deploy");
}

#[test]
fn deploy_drift_serde_roundtrips() {
    let d = DeployDrift::from_parts(2, vec!["rustyclawd-core".to_string()]);
    let json = serde_json::to_string(&d).unwrap();
    let back: DeployDrift = serde_json::from_str(&json).unwrap();
    assert_eq!(d, back);
}

// --- ReconcileDetector ------------------------------------------------------

#[test]
fn detect_reports_no_drift_when_current() {
    let src = FakeDeploySource {
        behind: 0,
        merged_pins: pins(&[("amplihack-memory", "abc"), ("rustyclawd-core", "def")]),
        running_pins: pins(&[("amplihack-memory", "abc"), ("rustyclawd-core", "def")]),
        ..Default::default()
    };
    let drift = ReconcileDetector::new(src).detect();
    assert_eq!(drift, DeployDrift::current());
    assert!(!drift.needs_deploy);
}

#[test]
fn detect_reports_commit_drift() {
    let src = FakeDeploySource {
        behind: 5,
        merged_pins: pins(&[("amplihack-memory", "abc")]),
        running_pins: pins(&[("amplihack-memory", "abc")]),
        ..Default::default()
    };
    let drift = ReconcileDetector::new(src).detect();
    assert_eq!(drift.behind_commits, 5);
    assert!(drift.drifted_pins.is_empty());
    assert!(drift.needs_deploy);
}

#[test]
fn detect_lists_drifted_pins_sorted() {
    let src = FakeDeploySource {
        behind: 0,
        merged_pins: pins(&[
            ("rustyclawd-core", "new1"),
            ("amplihack-memory", "new2"),
            ("rustyclawd-tools", "same"),
        ]),
        running_pins: pins(&[
            ("rustyclawd-core", "old1"),
            ("amplihack-memory", "old2"),
            ("rustyclawd-tools", "same"),
        ]),
        ..Default::default()
    };
    let drift = ReconcileDetector::new(src).detect();
    assert_eq!(
        drift.drifted_pins,
        vec![
            "amplihack-memory".to_string(),
            "rustyclawd-core".to_string()
        ],
        "only changed pins, sorted; unchanged pin excluded"
    );
    assert!(drift.needs_deploy);
}

#[test]
fn detect_treats_pin_missing_from_running_as_drift() {
    let src = FakeDeploySource {
        behind: 0,
        merged_pins: pins(&[("amplihack-memory", "abc")]),
        running_pins: pins(&[]),
        ..Default::default()
    };
    let drift = ReconcileDetector::new(src).detect();
    assert_eq!(drift.drifted_pins, vec!["amplihack-memory".to_string()]);
    assert!(drift.needs_deploy);
}

#[test]
fn detect_is_failsafe_on_source_error() {
    // A transient git failure must NOT spuriously trigger a deploy.
    let src = FakeDeploySource {
        fail: true,
        ..Default::default()
    };
    let drift = ReconcileDetector::new(src).detect();
    assert!(
        !drift.needs_deploy,
        "fail-safe: unverifiable drift never triggers a deploy"
    );
    assert_eq!(drift, DeployDrift::current());
}

#[test]
fn try_detect_surfaces_source_error_while_detect_fails_safe() {
    // Regression (#2751): `try_detect` must SURFACE a source error as `Err` so
    // the outcome-verify Rail-3 can tell "could not determine" apart from
    // "positively no drift" — while `detect` stays fail-safe for the
    // deploy-trigger path. If `try_detect` folded the error into `current()`
    // like `detect`, an unknown deploy state would forge a `verified` live
    // signal for a self-affecting goal.
    let src = FakeDeploySource {
        fail: true,
        ..Default::default()
    };
    let detector = ReconcileDetector::new(src);
    assert!(
        detector.try_detect().is_err(),
        "try_detect must not swallow a source error"
    );
    assert!(
        !detector.detect().needs_deploy,
        "detect must still fail safe (no spurious deploy)"
    );
}

#[test]
fn try_detect_agrees_with_detect_on_success() {
    // When the source is healthy, `try_detect` and `detect` return the same
    // drift — the fallible variant only diverges on error.
    let src = FakeDeploySource {
        behind: 4,
        ..Default::default()
    };
    let detector = ReconcileDetector::new(src);
    let via_try = detector.try_detect().unwrap();
    assert_eq!(via_try, detector.detect());
    assert!(via_try.needs_deploy);
    assert_eq!(via_try.behind_commits, 4);
}
