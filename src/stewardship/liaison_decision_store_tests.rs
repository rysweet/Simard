//! TDD tests for the NEW liaison-decision record store (Deliverable 1, design
//! component C3): `crate::stewardship::liaison_decision_store`.
//!
//! The store is a **sibling** of `merge_verdict_store` — cloned structure, but a
//! distinct identity and lifecycle. A liaison decision is keyed by the operator
//! message it answers `(group_id, message_id)`, NOT by a PR. It carries an
//! optional plain-English `reply` and/or an optional intervention `directive`.
//! It never reuses or mutates `MergeVerdictRecord`.
//!
//! Contract these tests pin (all in
//! `crate::stewardship::liaison_decision_store`):
//!   - `Directive { recipe, task_description, target_repo, context_path }`.
//!   - `LiaisonDecisionRecord { schema_version, group_id, message_id, run_token,
//!        recorded_at, reply: Option<String>, directive: Option<Directive> }`.
//!   - `LiaisonDecisionRecord::new(group_id, message_id, run_token,
//!        reply: Option<String>, directive: Option<Directive>) -> Self`
//!     (stamps `schema_version` = 1 and an RFC3339 `recorded_at`).
//!   - `record_path(state_root, group_id, message_id) -> Result<PathBuf, String>`
//!     traversal-safe `<state_root>/liaison_decisions/<group_id_hash>/<message_id>.json`.
//!     The opaque base64 `group_id` (may hold `/`, `+`, `=`) is HASHED into a
//!     single path-safe segment; `message_id` is validated (rejects `..`, `/`).
//!   - `write_record` — atomic (temp + rename), creates dirs, owner-only `0o600`.
//!   - `read_verified(state_root, group_id, message_id, expected_run_token)
//!     -> ReadOutcome` — total, fail-closed on malformed JSON, unknown schema,
//!     identity mismatch, or run_token mismatch.
//!   - `delete_record` — idempotent.
//!   - `ReadOutcome { Found(LiaisonDecisionRecord), Missing, Mismatch(String) }`.
//!
//! These are expected to FAIL TO COMPILE until C3 lands — the intended TDD red.

use std::path::{Path, PathBuf};

use crate::stewardship::liaison_decision_store::{
    Directive, LiaisonDecisionRecord, ReadOutcome, delete_record, read_verified, record_path,
    write_record,
};

const GROUP: &str = "cGxheS9ncm91cCsx=="; // opaque base64-ish: holds '/', '+', '='
const MSG: &str = "1690000000123";

fn temp_state_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "simard-ldstore-{tag}-{}-{}",
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

fn directive() -> Directive {
    Directive {
        recipe: "default-workflow".to_string(),
        task_description: "Investigate and fix the flaky deploy canary.".to_string(),
        target_repo: "rysweet/Simard".to_string(),
        context_path: "/tmp/liaison-directive-context.txt".to_string(),
    }
}

// ─────────────────────────── path derivation ────────────────────────────────

#[test]
fn record_path_is_contained_and_hashes_opaque_group_id() {
    let root = temp_state_root("path-hash");
    let p = record_path(&root, GROUP, MSG).expect("valid group/message");
    assert!(
        p.starts_with(root.join("liaison_decisions")),
        "path {p:?} escaped the liaison_decisions subtree"
    );
    // The raw group id contains '/', '+', '=' — none of those may appear as a
    // path component. The directory segment must be a single hashed token.
    let rel = p.strip_prefix(root.join("liaison_decisions")).unwrap();
    let comps: Vec<_> = rel.components().collect();
    assert_eq!(
        comps.len(),
        2,
        "expected <group_id_hash>/<message_id>.json, got {rel:?}"
    );
    let dir_seg = comps[0].as_os_str().to_string_lossy();
    assert!(
        !dir_seg.contains('/') && !dir_seg.contains('+') && !dir_seg.contains('='),
        "group-id directory segment must be a path-safe hash, got {dir_seg:?}"
    );
    assert!(
        !dir_seg.contains(GROUP),
        "the raw opaque group id must not appear verbatim in the path"
    );
    assert_eq!(
        p.file_name().unwrap().to_string_lossy(),
        format!("{MSG}.json"),
        "record file must be <message_id>.json"
    );
    cleanup(&root);
}

#[test]
fn record_path_is_deterministic_for_same_inputs() {
    let root = temp_state_root("path-det");
    let a = record_path(&root, GROUP, MSG).unwrap();
    let b = record_path(&root, GROUP, MSG).unwrap();
    assert_eq!(a, b, "record_path must be deterministic");
    cleanup(&root);
}

#[test]
fn record_path_rejects_traversal_or_separator_in_message_id() {
    let root = temp_state_root("path-msg-bad");
    for bad in ["../evil", "a/b", "..", "/abs", "msg\0id"] {
        assert!(
            record_path(&root, GROUP, bad).is_err(),
            "record_path must reject unsafe message_id {bad:?}"
        );
    }
    cleanup(&root);
}

// ─────────────────────────── write → read round-trip ────────────────────────

#[test]
fn write_then_read_round_trips_both_reply_and_directive() {
    let root = temp_state_root("rt-both");
    let rec = LiaisonDecisionRecord::new(
        GROUP,
        MSG,
        "run-token-1",
        Some("On it — kicking off a fix now.".to_string()),
        Some(directive()),
    );
    write_record(&root, &rec).expect("atomic write");

    match read_verified(&root, GROUP, MSG, "run-token-1") {
        ReadOutcome::Found(got) => {
            assert_eq!(got.group_id, GROUP);
            assert_eq!(got.message_id, MSG);
            assert_eq!(got.run_token, "run-token-1");
            assert_eq!(got.reply.as_deref(), Some("On it — kicking off a fix now."));
            let d = got.directive.expect("directive present");
            assert_eq!(d.recipe, "default-workflow");
            assert_eq!(d.target_repo, "rysweet/Simard");
        }
        other => panic!("expected Found(both), got {other:?}"),
    }
    cleanup(&root);
}

#[test]
fn reply_only_and_directive_only_and_neither_all_round_trip() {
    // reply-only
    let root = temp_state_root("rt-reply");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", Some("hello".to_string()), None);
    write_record(&root, &rec).unwrap();
    match read_verified(&root, GROUP, MSG, "t") {
        ReadOutcome::Found(g) => {
            assert_eq!(g.reply.as_deref(), Some("hello"));
            assert!(g.directive.is_none());
        }
        o => panic!("reply-only expected Found, got {o:?}"),
    }
    cleanup(&root);

    // directive-only
    let root = temp_state_root("rt-dir");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", None, Some(directive()));
    write_record(&root, &rec).unwrap();
    match read_verified(&root, GROUP, MSG, "t") {
        ReadOutcome::Found(g) => {
            assert!(g.reply.is_none());
            assert!(g.directive.is_some());
        }
        o => panic!("directive-only expected Found, got {o:?}"),
    }
    cleanup(&root);

    // neither (a valid no-op record)
    let root = temp_state_root("rt-neither");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", None, None);
    write_record(&root, &rec).unwrap();
    match read_verified(&root, GROUP, MSG, "t") {
        ReadOutcome::Found(g) => {
            assert!(g.reply.is_none() && g.directive.is_none());
        }
        o => panic!("neither expected Found, got {o:?}"),
    }
    cleanup(&root);
}

#[cfg(unix)]
#[test]
fn write_record_is_owner_only_0o600() {
    use std::os::unix::fs::PermissionsExt;
    let root = temp_state_root("mode");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", Some("hi".to_string()), None);
    write_record(&root, &rec).unwrap();
    let path = record_path(&root, GROUP, MSG).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "liaison decision record must be written owner-only 0o600, got {:o}",
        mode & 0o777
    );
    cleanup(&root);
}

#[test]
fn write_record_leaves_no_temp_files() {
    let root = temp_state_root("no-temp");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", Some("hi".to_string()), None);
    write_record(&root, &rec).unwrap();
    let dir = record_path(&root, GROUP, MSG)
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != &format!("{MSG}.json"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "atomic write left temp files: {leftovers:?}"
    );
    cleanup(&root);
}

// ─────────────────────────── fail-closed reads ──────────────────────────────

#[test]
fn read_missing_is_missing() {
    let root = temp_state_root("miss");
    assert!(matches!(
        read_verified(&root, GROUP, MSG, "any"),
        ReadOutcome::Missing
    ));
    cleanup(&root);
}

#[test]
fn read_wrong_run_token_is_mismatch() {
    let root = temp_state_root("tok");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "expected", Some("x".to_string()), None);
    write_record(&root, &rec).unwrap();
    assert!(
        matches!(
            read_verified(&root, GROUP, MSG, "DIFFERENT"),
            ReadOutcome::Mismatch(_)
        ),
        "a foreign/previous-run token must fail closed"
    );
    cleanup(&root);
}

#[test]
fn read_identity_mismatch_is_mismatch() {
    // Physically place a record whose embedded identity disagrees with the key.
    let root = temp_state_root("ident");
    let path = record_path(&root, GROUP, MSG).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"schema_version":1,"group_id":"OTHER","message_id":"999","run_token":"t","recorded_at":"2026-01-01T00:00:00Z","reply":"x","directive":null}"#,
    )
    .unwrap();
    assert!(matches!(
        read_verified(&root, GROUP, MSG, "t"),
        ReadOutcome::Mismatch(_)
    ));
    cleanup(&root);
}

#[test]
fn read_malformed_json_never_panics_and_is_mismatch() {
    let root = temp_state_root("malformed");
    let path = record_path(&root, GROUP, MSG).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{ not json ,,,,").unwrap();
    assert!(matches!(
        read_verified(&root, GROUP, MSG, "t"),
        ReadOutcome::Mismatch(_)
    ));
    cleanup(&root);
}

#[test]
fn read_unknown_schema_version_fails_closed() {
    let root = temp_state_root("schema");
    let path = record_path(&root, GROUP, MSG).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        format!(
            r#"{{"schema_version":999,"group_id":{GROUP:?},"message_id":{MSG:?},"run_token":"t","recorded_at":"2026-01-01T00:00:00Z","reply":"x","directive":null}}"#
        ),
    )
    .unwrap();
    assert!(matches!(
        read_verified(&root, GROUP, MSG, "t"),
        ReadOutcome::Mismatch(_)
    ));
    cleanup(&root);
}

// ─────────────────────────── delete (idempotent) ────────────────────────────

#[test]
fn delete_then_read_is_missing() {
    let root = temp_state_root("del");
    let rec = LiaisonDecisionRecord::new(GROUP, MSG, "t", Some("x".to_string()), None);
    write_record(&root, &rec).unwrap();
    delete_record(&root, GROUP, MSG).expect("delete existing");
    assert!(matches!(
        read_verified(&root, GROUP, MSG, "t"),
        ReadOutcome::Missing
    ));
    cleanup(&root);
}

#[test]
fn delete_absent_is_ok() {
    let root = temp_state_root("del-absent");
    delete_record(&root, GROUP, "404").expect("delete absent is a no-op success");
    cleanup(&root);
}
