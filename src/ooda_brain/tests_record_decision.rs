//! TDD failing tests for the typed per-goal-cycle decision RECORD and its
//! fail-CLOSED reader (WS-4, issues #2573 / #2658 / #1711).
//!
//! These tests specify the contract of the seam that REPLACES the forbidden
//! "recipe prints JSON → Rust scrapes prose → Rust acts" pattern on the core
//! decision path. They reference symbols that DO NOT EXIST YET, so the crate
//! will not compile until the Builder phase adds them:
//!
//!   * `super::PerGoalDecisionRecord` — the typed on-disk record
//!     `{schema, goal_id, cycle_number, #[flatten] action: PerGoalAction}`.
//!   * `super::EXPECTED_SCHEMA` — the pinned schema string
//!     (`"simard.ooda.per_goal_decision.v1"`).
//!   * `super::read_verified(path, goal_id, cycle_number) -> SimardResult<PerGoalAction>`
//!     — the reader that enforces the fail-CLOSED matrix.
//!
//! The whole point of this seam is that EVERY failure mode is an `Err` (a safe
//! no-op cycle failure), never a default action (#1711). The `read_verified`
//! matrix (R1–R8) is the load-bearing invariant and each row is asserted here.
//!
//! Reference contract: `docs/reference/ooda-record-decision-cli.md`.

use std::path::Path;

use super::{EXPECTED_SCHEMA, PerGoalAction, PerGoalDecisionRecord, read_verified};

// ---------------------------------------------------------------------------
// Helpers — a hermetic temp dir + direct record writer (no CLI, no subprocess).
//
// The reader tests DELIBERATELY write the on-disk bytes themselves (via serde
// for the valid cases, raw strings for the hostile cases) so they exercise
// `read_verified` in isolation from the writer tool. This is the defense-in-
// depth guarantee: even a record the tool would never produce must fail CLOSED.
// ---------------------------------------------------------------------------

const GOAL: &str = "continuously-research-and-improve-your-own-cogn-70ab8541";
const CYCLE: u32 = 4287;

fn write_bytes(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join("decision.json");
    std::fs::write(&path, bytes).expect("write record bytes");
    path
}

/// Serialize a well-formed record and drop it at `<dir>/decision.json`.
fn write_record(dir: &Path, record: &PerGoalDecisionRecord) -> std::path::PathBuf {
    let bytes = serde_json::to_vec(record).expect("serialize record");
    write_bytes(dir, &bytes)
}

fn record_for(action: PerGoalAction) -> PerGoalDecisionRecord {
    PerGoalDecisionRecord {
        schema: EXPECTED_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        action,
    }
}

fn spawn_action() -> PerGoalAction {
    PerGoalAction::Spawn {
        reason: "no live work; standing research goal must seek the next source".into(),
        task_hint: "survey arXiv 2026 for new distillation results".into(),
    }
}

// ---------------------------------------------------------------------------
// Schema pin sanity — the constant is the exact versioned string.
// ---------------------------------------------------------------------------

#[test]
fn expected_schema_is_the_pinned_v1_string() {
    assert_eq!(
        EXPECTED_SCHEMA, "simard.ooda.per_goal_decision.v1",
        "the reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ---------------------------------------------------------------------------
// R8 — all checks pass ⇒ Ok(PerGoalAction). Every one of the six variants
// round-trips through the record bit-for-bit.
// ---------------------------------------------------------------------------

#[test]
fn read_verified_round_trips_every_variant() {
    let variants = [
        PerGoalAction::Continue {
            reason: "engineer healthy, PR in review".into(),
        },
        spawn_action(),
        PerGoalAction::Reorient {
            reason: "angle exhausted; deliberately pivot".into(),
        },
        PerGoalAction::Investigate {
            reason: "worker quiet; read logs before any reclaim".into(),
        },
        PerGoalAction::Wait {
            reason: "PR awaiting required CI checks".into(),
        },
        PerGoalAction::Complete {
            reason: "success criteria observed live".into(),
        },
    ];

    for action in variants {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_record(dir.path(), &record_for(action.clone()));
        let read = read_verified(&path, GOAL, CYCLE)
            .unwrap_or_else(|e| panic!("valid record for {action:?} must read back Ok, got {e:?}"));
        assert_eq!(
            read, action,
            "read_verified must return the exact recorded action bit-for-bit"
        );
    }
}

#[test]
fn read_verified_preserves_spawn_task_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_record(dir.path(), &record_for(spawn_action()));
    match read_verified(&path, GOAL, CYCLE).expect("spawn record reads ok") {
        PerGoalAction::Spawn { task_hint, .. } => assert_eq!(
            task_hint, "survey arXiv 2026 for new distillation results",
            "the spawn task_hint must survive the record round-trip"
        ),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// R1 — file absent (tool never ran / binary unresolvable / tool exited nonzero)
// ---------------------------------------------------------------------------

#[test]
fn r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("decision.json"); // never created
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R1: an absent record MUST fail CLOSED (Err → safe no-op), never a default action"
    );
}

// ---------------------------------------------------------------------------
// R2 — present but not valid JSON / truncated.
// ---------------------------------------------------------------------------

#[test]
fn r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ this is not valid json ");
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST fail CLOSED"
    );
}

#[test]
fn r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) file MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R3 — schema != EXPECTED_SCHEMA (e.g. a future …v2). Version pin.
// ---------------------------------------------------------------------------

#[test]
fn r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Structurally valid, correct goal/cycle, valid choice — ONLY the schema
    // string is wrong. A v2 writer must not be read by a v1 reader.
    let json = format!(
        r#"{{"schema":"simard.ooda.per_goal_decision.v2","goal_id":"{GOAL}",\
"cycle_number":{CYCLE},"choice":"spawn","reason":"future record","task_hint":""}}"#
    );
    let path = write_bytes(dir.path(), json.replace('\\', "").as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST fail CLOSED"
    );
}

#[test]
fn r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"wait","reason":"blocked on CI"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R3: a record with no schema field MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R4 — choice not one of the six closed variants. A compromised prompt cannot
// smuggle a novel destructive action (e.g. `deploy`) past the reader.
// ---------------------------------------------------------------------------

#[test]
fn r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{EXPECTED_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},\
"choice":"deploy","reason":"smuggled action"}}"#
    );
    let path = write_bytes(dir.path(), json.replace('\\', "").as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag (`deploy`) MUST fail CLOSED — the closed enum is the sole authority"
    );
}

// ---------------------------------------------------------------------------
// R5 — reason missing or empty. A reason-less decision must never be honored
// (acceptance: "a recorded reason every cycle").
// ---------------------------------------------------------------------------

#[test]
fn r5_empty_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{EXPECTED_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},\
"choice":"spawn","reason":"","task_hint":""}}"#
    );
    let path = write_bytes(dir.path(), json.replace('\\', "").as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R5: an empty reason MUST fail CLOSED"
    );
}

#[test]
fn r5_whitespace_only_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{EXPECTED_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},\
"choice":"reorient","reason":"   "}}"#
    );
    let path = write_bytes(dir.path(), json.replace('\\', "").as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R5: a whitespace-only reason MUST fail CLOSED (empty after trim)"
    );
}

#[test]
fn r5_missing_reason_field_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{EXPECTED_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"wait"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R5: a record with no reason field MUST fail CLOSED"
    );
}

#[test]
fn r5_control_byte_only_reason_fails_closed() {
    // Defense-in-depth: a hostile record the tool would never produce whose
    // `reason` is ONLY ANSI/control bytes (JSON-escaped on disk) sanitizes to
    // empty ⇒ fail CLOSED. A bare trim would wrongly accept it (ESC is not
    // ASCII whitespace).
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{EXPECTED_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\
\"choice\":\"spawn\",\"reason\":\"\\u001b\\u0007\\u0001\",\"task_hint\":\"\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "a control-byte-only reason MUST fail CLOSED (empty after sanitize)"
    );
}

#[test]
fn read_verified_sanitizes_ansi_control_from_reason() {
    // A record carrying ANSI/C0 escape sequences in `reason` (JSON-escaped on
    // disk) must NOT be honored verbatim — the reader re-sanitizes through the
    // same chokepoint the tool uses on write, so control bytes never reach
    // operator logs (#2751).
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{EXPECTED_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\
\"choice\":\"wait\",\"reason\":\"\\u001b[31mALERT\\u001b[0m blocked on CI\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let action = read_verified(&path, GOAL, CYCLE).expect("sanitizable reason must verify");
    let reason = action.reason();
    assert!(
        !reason.contains('\u{1b}') && reason.contains("ALERT") && reason.contains("blocked on CI"),
        "ANSI/C0 bytes MUST be stripped on read while preserving the text; got {reason:?}"
    );
}

// ---------------------------------------------------------------------------
// R6 — goal_id != live ctx goal_id (stale / other-goal record). Defeats the
// fail-OPEN risk of honoring a record written for a DIFFERENT goal.
// ---------------------------------------------------------------------------

#[test]
fn r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = record_for(PerGoalAction::Complete {
        reason: "criteria met".into(),
    });
    record.goal_id = "some-other-goal".to_string();
    let path = write_record(dir.path(), &record);
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the live ctx MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R7 — cycle_number != live ctx cycle_number (a prior cycle's decision). This
// is the subtle fail-OPEN the whole verification exists to prevent.
// ---------------------------------------------------------------------------

#[test]
fn r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = record_for(PerGoalAction::Reorient {
        reason: "prior-cycle verdict".into(),
    });
    record.cycle_number = CYCLE - 1; // last cycle's record lingering on disk
    let path = write_record(dir.path(), &record);
    assert!(
        read_verified(&path, GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST fail CLOSED (no replay of a prior verdict)"
    );
}

// ---------------------------------------------------------------------------
// Cross-check: a fully-correct record with a DESTRUCTIVE action still verifies
// (so the fail-closed checks are not accidentally rejecting valid mutations).
// ---------------------------------------------------------------------------

#[test]
fn valid_reorient_record_verifies_and_reports_mutation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let action = PerGoalAction::Reorient {
        reason: "deliberate redirect".into(),
    };
    let path = write_record(dir.path(), &record_for(action.clone()));
    let read = read_verified(&path, GOAL, CYCLE).expect("valid reorient must verify");
    assert_eq!(read, action);
    assert!(
        read.mutates_refs(),
        "reorient must still be recognized as a ref-mutating action after the round-trip"
    );
}
