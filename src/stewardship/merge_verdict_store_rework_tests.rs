//! TDD tests for the EXTENDED merge-verdict record (Deliverable 2, design
//! component C4). The rework loop reuses the existing durable merge-verdict
//! store (`crate::stewardship::merge_verdict_store`) and adds two **optional**
//! fields — no new stdout envelope:
//!
//!   - `reworkable: Option<bool>` (`#[serde(default)]`) — `Some(true)` marks a
//!     *fixable* hold the rework rail may dispatch. **Absent or `Some(false)` ⇒
//!     not reworkable** (fail-closed).
//!   - `concern: Option<String>` (`#[serde(default)]`) — a concise plain-English
//!     description of exactly what must change (handed to the rework recipe as a
//!     ContextFile).
//!
//! Compatibility contract these tests pin:
//!   - `SCHEMA_VERSION` stays `1`; the struct is NOT `deny_unknown_fields`, so an
//!     OLD record (no `reworkable`/`concern`) deserializes cleanly and reads as
//!     *not reworkable*.
//!   - Every record written is owner-only `0o600`.
//!   - The whole pre-existing fail-closed read matrix is unchanged.
//!
//! These reference NEW struct fields and are expected to FAIL TO COMPILE until
//! C4 lands — the intended TDD red state.

use std::path::{Path, PathBuf};

use crate::stewardship::merge_verdict_store::{
    MergeVerdictRecord, ReadOutcome, VerdictKind, read_verified, record_path, write_record,
};

fn temp_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-mvrework-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

// ─────────────────────── default constructor is not reworkable ──────────────

#[test]
fn new_record_defaults_to_not_reworkable() {
    // The existing constructor must leave the new fields "off": a plain merge or
    // hold verdict is NOT a rework request.
    let rec = MergeVerdictRecord::new(10, "rysweet/Simard", VerdictKind::Hold, "held", "tok");
    assert_eq!(
        rec.reworkable, None,
        "a freshly-built record must default to reworkable=None (fail-closed)"
    );
    assert_eq!(rec.concern, None, "no concern by default");
}

// ─────────────────────── round-trip with rework fields ──────────────────────

#[test]
fn reworkable_hold_round_trips_reworkable_and_concern() {
    let root = temp_state_root("rt");
    let mut rec = MergeVerdictRecord::new(
        4931,
        "rysweet/Simard",
        VerdictKind::Hold,
        "Fixable: backoff clamp ordering",
        "run-token-rework",
    );
    rec.reworkable = Some(true);
    rec.concern =
        Some("Clamp the retry backoff before multiplying; add a ceiling unit test.".to_string());
    write_record(&root, &rec).expect("write");

    match read_verified(&root, "rysweet/Simard", 4931, "run-token-rework") {
        ReadOutcome::Found(got) => {
            assert_eq!(
                got.verdict,
                VerdictKind::Hold,
                "rework only pairs with hold"
            );
            assert_eq!(got.reworkable, Some(true));
            assert_eq!(
                got.concern.as_deref(),
                Some("Clamp the retry backoff before multiplying; add a ceiling unit test.")
            );
        }
        other => panic!("expected Found(reworkable hold), got {other:?}"),
    }
    cleanup(&root);
}

// ─────────────────────── backward compatibility (old JSON) ──────────────────

#[test]
fn old_record_without_new_fields_deserializes_as_not_reworkable() {
    // A record written by a prior build (no reworkable/concern keys at all) must
    // still deserialize cleanly (serde default) and read as not reworkable.
    let root = temp_state_root("compat");
    let path = record_path(&root, "o/r", 7).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"schema_version":1,"pr":7,"repo":"o/r","verdict":"hold","reason":"x","recorded_at":"2026-01-01T00:00:00Z","run_token":"tok"}"#,
    )
    .unwrap();
    match read_verified(&root, "o/r", 7, "tok") {
        ReadOutcome::Found(got) => {
            assert_eq!(
                got.reworkable, None,
                "an old record with no reworkable key must read as None (not reworkable)"
            );
            assert_eq!(got.concern, None);
        }
        other => panic!("old record must still be Found, got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn schema_version_stays_one_for_reworkable_records() {
    let rec = MergeVerdictRecord::new(1, "o/r", VerdictKind::Hold, "x", "t");
    assert_eq!(
        rec.schema_version, 1,
        "adding optional fields must NOT bump SCHEMA_VERSION (additive, not breaking)"
    );
}

// ─────────────────────── owner-only 0o600 on write ──────────────────────────

#[cfg(unix)]
#[test]
fn write_record_is_owner_only_0o600() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_state_root("mode");
    let mut rec = MergeVerdictRecord::new(3, "o/r", VerdictKind::Hold, "x", "t");
    rec.reworkable = Some(true);
    rec.concern = Some("fix it".to_string());
    write_record(&root, &rec).unwrap();
    let path = record_path(&root, "o/r", 3).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "merge-verdict record must be written owner-only 0o600, got {:o}",
        mode & 0o777
    );
    cleanup(&root);
}

// ─────────────────────── fail-closed reads still hold ───────────────────────

#[test]
fn reworkable_record_wrong_token_still_fails_closed() {
    let root = temp_state_root("tok");
    let mut rec = MergeVerdictRecord::new(9, "o/r", VerdictKind::Hold, "x", "expected");
    rec.reworkable = Some(true);
    rec.concern = Some("fix".to_string());
    write_record(&root, &rec).unwrap();
    assert!(
        matches!(
            read_verified(&root, "o/r", 9, "DIFFERENT"),
            ReadOutcome::Mismatch(_)
        ),
        "a reworkable record with a stale token must still fail closed"
    );
    cleanup(&root);
}
