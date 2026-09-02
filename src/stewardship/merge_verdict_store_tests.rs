//! TDD tests for the durable, typed merge-verdict record store (issue #4721,
//! WS-2). These tests define the contract for the NOT-YET-IMPLEMENTED module
//! `crate::stewardship::merge_verdict_store` (design component C2).
//!
//! The store is the transport that replaces the forbidden
//! "recipe emits JSON → Rust scrapes stdout" pattern: the agent-facing
//! `simard merge record-verdict` tool WRITES a typed record here, and the thin
//! deterministic rail READS it back (freshness/identity-checked) instead of
//! parsing prose. See `docs/reference/merge-record-verdict-cli.md`.
//!
//! Contract these tests pin (all functions live in
//! `crate::stewardship::merge_verdict_store`):
//!   - `VerdictKind { Merge, Hold }` — the only two typed verdicts.
//!   - `MergeVerdictRecord::new(pr: u32, repo: &str, verdict: VerdictKind,
//!        reason: &str, run_token: &str) -> MergeVerdictRecord`
//!     (stamps `schema_version` = 1 and an RFC3339 `recorded_at` internally).
//!   - `record_path(state_root, repo, pr) -> Result<PathBuf, String>`
//!     deterministic `<state_root>/merge_verdicts/<repo__sanitized>/<pr>.json`,
//!     traversal-safe (rejects `..`, absolute, NUL, malformed slug).
//!   - `write_record(state_root, &MergeVerdictRecord) -> Result<(), String>`
//!     atomic (temp + rename), creates parent dirs, overwrites in place.
//!   - `read_verified(state_root, repo, pr, expected_run_token) -> ReadOutcome`
//!     total function; never panics on malformed JSON; fail-closed on unknown
//!     `schema_version`, (repo, pr) mismatch, or run_token mismatch.
//!   - `delete_record(state_root, repo, pr) -> Result<(), String>` idempotent.
//!   - `ReadOutcome { Found(MergeVerdictRecord), Missing, Mismatch(String) }`.
//!
//! ALL of these tests are expected to FAIL TO COMPILE until C2 lands — that is
//! the intended TDD red state.

use std::path::{Path, PathBuf};

use crate::stewardship::merge_verdict_store::{
    MergeVerdictRecord, ReadOutcome, VerdictKind, delete_record, read_verified, record_path,
    write_record,
};

/// A hermetic temp state-root unique to each test (no `$HOME`, no shared dirs).
fn temp_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-mvstore-{tag}-{}-{}",
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

// ─────────────────────────── path derivation ────────────────────────────────

#[test]
fn record_path_is_deterministic_and_sanitizes_repo_slash() {
    let root = temp_state_root("path-det");
    let p = record_path(&root, "rysweet/Simard", 42).expect("valid repo/pr");
    assert_eq!(
        p,
        root.join("merge_verdicts")
            .join("rysweet__Simard")
            .join("42.json"),
        "path must be <root>/merge_verdicts/<owner__repo>/<pr>.json"
    );
    cleanup(&root);
}

#[test]
fn record_path_is_contained_under_merge_verdicts() {
    let root = temp_state_root("path-contain");
    let p = record_path(&root, "rysweet/amplihack-rs", 7).expect("valid");
    assert!(
        p.starts_with(root.join("merge_verdicts")),
        "derived path {p:?} escaped the merge_verdicts subtree"
    );
    cleanup(&root);
}

#[test]
fn record_path_rejects_traversal_repo() {
    let root = temp_state_root("path-traverse");
    for bad in [
        "rysweet/../../etc",
        "../evil",
        "a/../../b",
        "..",
        "rysweet/Sim..ard/../x",
    ] {
        assert!(
            record_path(&root, bad, 1).is_err(),
            "record_path must reject traversal-bearing repo {bad:?}"
        );
    }
    cleanup(&root);
}

#[test]
fn record_path_rejects_absolute_and_nul_and_malformed_slug() {
    let root = temp_state_root("path-bad-slug");
    for bad in ["/abs/repo", "no-slash", "a/b/c", "owner/", "/", "a\0b/c"] {
        assert!(
            record_path(&root, bad, 1).is_err(),
            "record_path must reject malformed/unsafe repo {bad:?}"
        );
    }
    cleanup(&root);
}

// ─────────────────────────── write → read round-trip ────────────────────────

#[test]
fn write_then_read_verified_round_trips_all_fields() {
    let root = temp_state_root("rt-merge");
    let rec = MergeVerdictRecord::new(
        1500,
        "rysweet/Simard",
        VerdictKind::Merge,
        "crusty passed; CI green; diff reviewed",
        "run-token-abc123",
    );
    write_record(&root, &rec).expect("atomic write");

    match read_verified(&root, "rysweet/Simard", 1500, "run-token-abc123") {
        ReadOutcome::Found(got) => {
            assert_eq!(got.pr, 1500);
            assert_eq!(got.repo, "rysweet/Simard");
            assert_eq!(got.verdict, VerdictKind::Merge);
            assert_eq!(got.reason, "crusty passed; CI green; diff reviewed");
            assert_eq!(got.run_token, "run-token-abc123");
        }
        other => panic!("expected Found, got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn write_then_read_verified_round_trips_hold_verdict() {
    let root = temp_state_root("rt-hold");
    let rec = MergeVerdictRecord::new(
        820,
        "rysweet/amplihack-rs",
        VerdictKind::Hold,
        "crusty flagged an untested path",
        "tok-hold-1",
    );
    write_record(&root, &rec).expect("write");
    match read_verified(&root, "rysweet/amplihack-rs", 820, "tok-hold-1") {
        ReadOutcome::Found(got) => assert_eq!(got.verdict, VerdictKind::Hold),
        other => panic!("expected Found(Hold), got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn write_record_is_atomic_overwrite_last_writer_wins() {
    let root = temp_state_root("rt-overwrite");
    let first = MergeVerdictRecord::new(9, "o/r", VerdictKind::Hold, "stale hold", "tok-old");
    write_record(&root, &first).expect("first write");
    let second = MergeVerdictRecord::new(9, "o/r", VerdictKind::Merge, "fresh merge", "tok-new");
    write_record(&root, &second).expect("second write");

    match read_verified(&root, "o/r", 9, "tok-new") {
        ReadOutcome::Found(got) => {
            assert_eq!(got.verdict, VerdictKind::Merge);
            assert_eq!(got.reason, "fresh merge");
        }
        other => panic!("expected the second record to win, got {other:?}"),
    }
    // No leftover temp files beside the final record.
    let dir = record_path(&root, "o/r", 9)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "9.json")
        .collect();
    assert!(
        leftovers.is_empty(),
        "atomic write left temp files: {leftovers:?}"
    );
    cleanup(&root);
}

// ─────────────────────────── freshness / fail-closed reads ───────────────────

#[test]
fn read_verified_missing_file_is_missing() {
    let root = temp_state_root("read-missing");
    assert!(matches!(
        read_verified(&root, "o/r", 1, "any"),
        ReadOutcome::Missing
    ));
    cleanup(&root);
}

#[test]
fn read_verified_wrong_run_token_is_mismatch() {
    let root = temp_state_root("read-token");
    let rec = MergeVerdictRecord::new(3, "o/r", VerdictKind::Merge, "ok", "tok-expected");
    write_record(&root, &rec).unwrap();
    assert!(
        matches!(
            read_verified(&root, "o/r", 3, "tok-DIFFERENT"),
            ReadOutcome::Mismatch(_)
        ),
        "a foreign/previous-run token must fail closed as Mismatch"
    );
    cleanup(&root);
}

#[test]
fn read_verified_mismatched_repo_or_pr_is_mismatch() {
    let root = temp_state_root("read-identity");
    // Physically place a record under (o/r, 3) whose *contents* claim a
    // different (repo, pr) — the reader must not trust the filename alone.
    let path = record_path(&root, "o/r", 3).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"schema_version":1,"pr":999,"repo":"other/repo","verdict":"merge","reason":"x","recorded_at":"2026-01-01T00:00:00Z","run_token":"tok"}"#,
    )
    .unwrap();
    assert!(
        matches!(
            read_verified(&root, "o/r", 3, "tok"),
            ReadOutcome::Mismatch(_)
        ),
        "record whose embedded (repo,pr) disagrees with the key must fail closed"
    );
    cleanup(&root);
}

#[test]
fn read_verified_malformed_json_never_panics_and_is_mismatch() {
    let root = temp_state_root("read-malformed");
    let path = record_path(&root, "o/r", 5).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ this is not json, one trailing comma,,").unwrap();
    // Total function: fail closed, do not panic.
    assert!(matches!(
        read_verified(&root, "o/r", 5, "tok"),
        ReadOutcome::Mismatch(_)
    ));
    cleanup(&root);
}

#[test]
fn read_verified_unknown_schema_version_fails_closed() {
    let root = temp_state_root("read-schema");
    let path = record_path(&root, "o/r", 6).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"schema_version":999,"pr":6,"repo":"o/r","verdict":"merge","reason":"x","recorded_at":"2026-01-01T00:00:00Z","run_token":"tok"}"#,
    )
    .unwrap();
    assert!(
        matches!(
            read_verified(&root, "o/r", 6, "tok"),
            ReadOutcome::Mismatch(_)
        ),
        "an unknown schema_version must fail closed, never be treated as a verdict"
    );
    cleanup(&root);
}

// ─────────────────────────── delete (anti-replay) ───────────────────────────

#[test]
fn delete_record_then_read_is_missing() {
    let root = temp_state_root("delete");
    let rec = MergeVerdictRecord::new(11, "o/r", VerdictKind::Merge, "ok", "tok");
    write_record(&root, &rec).unwrap();
    delete_record(&root, "o/r", 11).expect("delete existing");
    assert!(matches!(
        read_verified(&root, "o/r", 11, "tok"),
        ReadOutcome::Missing
    ));
    cleanup(&root);
}

#[test]
fn delete_record_is_idempotent_on_absent() {
    let root = temp_state_root("delete-idem");
    // Deleting a non-existent record must be a no-op success (delete-before-run
    // safety: the rail deletes any prior record before invoking the recipe).
    delete_record(&root, "o/r", 404).expect("delete absent must succeed");
    cleanup(&root);
}
