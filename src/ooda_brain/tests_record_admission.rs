//! TDD **failing** tests for the typed engineer- + resource-ADMISSION decision
//! RECORDS and their fail-CLOSED readers (Group B of epic #4719; issue #4906).
//!
//! These specify the seam that REPLACES the forbidden "recipe prints JSON →
//! Rust scrapes prose → Rust acts" pattern on the two admission phases, exactly
//! as Group A (`tests_record_orient_decide.rs`, #4785) did for orient/decide and
//! #4734 (`tests_record_decision.rs`) did for the per-goal-cycle phase. They
//! reference symbols that DO NOT EXIST YET, so the crate will not compile until
//! the Builder phase adds them:
//!
//!   * `super::ADMISSION_SCHEMA` / `super::RESOURCE_ADMISSION_SCHEMA` — pinned
//!     schema strings (`"simard.ooda.admission.v1"` /
//!     `"simard.ooda.resource_admission.v1"`).
//!   * `EngineerAdmissionDecision::from_choice_fields(choice, rationale,
//!     blocked_by, after_goal_id, overlap_files, retry_after_secs)
//!     -> Option<EngineerAdmissionDecision>` — the SINGLE shared closed-enum
//!     chokepoint, reused by the `record-admission` CLI writer AND
//!     `read_verified_admission`, enforcing the per-variant field-ownership
//!     matrix and a non-empty sanitized rationale.
//!   * `ResourceAdmissionDecision::from_choice_fields(choice, rationale)
//!     -> Option<ResourceAdmissionDecision>` — the SINGLE shared closed-enum
//!     chokepoint for the resource gate (all variants carry only `rationale`).
//!   * `super::AdmissionDecisionRecord` / `super::ResourceAdmissionDecisionRecord`
//!     — the typed on-disk records (`{schema, goal_id, cycle_number,
//!     #[serde(flatten)] decision}`).
//!   * `super::read_verified_admission` / `super::read_verified_resource_admission`
//!     — the readers that enforce the fail-CLOSED matrix (R1–R8).
//!
//! The whole point of this seam is that EVERY failure mode is an `Err`. HOW that
//! `Err` is surfaced differs by gate and is UNCHANGED by this rework (only the
//! `Err` trigger changes): the engineer-admission act-site turns `Err` into a
//! loud `Admit` (fail-OPEN), the resource-admission act-site turns `Err` into a
//! benign `Defer` (fail-CLOSED). Those act-site polarities are asserted by the
//! existing seam tests; here we pin the reader matrix + the chokepoint.
//!
//! Reference contract: `docs/reference/ooda-record-admission-cli.md`.

use std::path::Path;

use super::{
    ADMISSION_SCHEMA, AdmissionDecisionRecord, EngineerAdmissionDecision,
    RESOURCE_ADMISSION_SCHEMA, ResourceAdmissionDecision, ResourceAdmissionDecisionRecord,
    read_verified_admission, read_verified_resource_admission,
};

// ---------------------------------------------------------------------------
// Shared hermetic helpers — a temp dir + direct record writer (no CLI, no
// subprocess). The reader tests DELIBERATELY write the on-disk bytes themselves
// (via serde for valid cases, raw strings for hostile cases) so they exercise
// the readers in ISOLATION from the writer tool: the defense-in-depth guarantee
// is that even a record the tool would never produce must fail CLOSED.
// ---------------------------------------------------------------------------

const GOAL: &str = "add-int8-embeddings-70ab8541";
const CYCLE: u32 = 4287;

fn write_bytes(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join("record.json");
    std::fs::write(&path, bytes).expect("write record bytes");
    path
}

fn write_json<T: serde::Serialize>(dir: &Path, record: &T) -> std::path::PathBuf {
    let bytes = serde_json::to_vec(record).expect("serialize record");
    write_bytes(dir, &bytes)
}

// Convenience constructors for the engineer chokepoint's six-arg form. The
// "no extra fields" case is the common one; the ownership tests call
// `EngineerAdmissionDecision::from_choice_fields` directly with the smuggled
// field set.
fn admit(rationale: &str) -> Option<EngineerAdmissionDecision> {
    EngineerAdmissionDecision::from_choice_fields("admit", rationale, &[], "", &[], None)
}

// ===========================================================================
// ENGINEER-ADMISSION side (fail-OPEN at the act-site)
// ===========================================================================

/// The canonical three engineer-admission variants (closed set). The chokepoint
/// must accept EXACTLY these three — no more, no fewer.
const ENGINEER_VARIANTS: &[&str] = &["admit", "defer", "serialize_after"];

fn engineer_record(decision: EngineerAdmissionDecision) -> AdmissionDecisionRecord {
    AdmissionDecisionRecord {
        schema: ADMISSION_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        decision,
    }
}

// ----- Schema pin -----------------------------------------------------------

#[test]
fn admission_schema_is_the_pinned_v1_string() {
    assert_eq!(
        ADMISSION_SCHEMA, "simard.ooda.admission.v1",
        "the engineer-admission reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- Chokepoint: exactly the 3 variants, case-insensitive, sanitized -------

#[test]
fn engineer_chokepoint_accepts_admit() {
    let d = admit("independent files").expect("`admit` must be accepted");
    assert_eq!(d.variant_label(), "admit");
    assert_eq!(d.rationale(), "independent files");
    assert!(
        d.blocking_goals().is_empty(),
        "an admit names no blocking goals"
    );
}

#[test]
fn engineer_chokepoint_accepts_defer_with_owned_fields() {
    let d = EngineerAdmissionDecision::from_choice_fields(
        "defer",
        "live engineer holds goals_status.rs",
        &["render-goals-status".to_string()],
        "",
        &[],
        Some(900),
    )
    .expect("`defer` with blocked_by + retry_after_secs must be accepted");
    match d {
        EngineerAdmissionDecision::Defer {
            blocked_by,
            retry_after_secs,
            ..
        } => {
            assert_eq!(blocked_by, vec!["render-goals-status".to_string()]);
            assert_eq!(retry_after_secs, Some(900));
        }
        other => panic!("expected Defer, got {other:?}"),
    }
}

#[test]
fn engineer_chokepoint_accepts_serialize_after_with_owned_fields() {
    let d = EngineerAdmissionDecision::from_choice_fields(
        "serialize_after",
        "rebase behind the adapter rename",
        &[],
        "rename-adapter-to-clients",
        &["src/ooda_loop/types.rs".to_string()],
        None,
    )
    .expect("`serialize_after` with after_goal_id + overlap_files must be accepted");
    match d {
        EngineerAdmissionDecision::SerializeAfter {
            after_goal_id,
            overlap_files,
            ..
        } => {
            assert_eq!(after_goal_id, "rename-adapter-to-clients");
            assert_eq!(overlap_files, vec!["src/ooda_loop/types.rs".to_string()]);
        }
        other => panic!("expected SerializeAfter, got {other:?}"),
    }
}

#[test]
fn engineer_chokepoint_is_case_insensitive() {
    let d = EngineerAdmissionDecision::from_choice_fields("ADMIT", "loud caps", &[], "", &[], None)
        .expect("choice must be matched case-insensitively");
    assert_eq!(d.variant_label(), "admit");
}

#[test]
fn engineer_chokepoint_rejects_unknown_choice() {
    assert!(
        EngineerAdmissionDecision::from_choice_fields("deploy", "smuggled", &[], "", &[], None)
            .is_none(),
        "an unknown choice tag MUST be rejected — the closed enum is the sole authority"
    );
}

#[test]
fn engineer_chokepoint_rejects_empty_rationale() {
    assert!(
        admit("").is_none(),
        "an empty rationale MUST be rejected (fail CLOSED at the chokepoint)"
    );
    assert!(
        admit("   ").is_none(),
        "a whitespace-only rationale MUST be rejected (empty after trim)"
    );
}

#[test]
fn engineer_chokepoint_rejects_control_only_rationale() {
    // A rationale made up ENTIRELY of ANSI/C0 control bytes sanitizes to empty ⇒
    // rejected. A bare `trim()` would wrongly accept it (ESC is not whitespace).
    assert!(
        admit("\u{1b}\u{07}\u{01}").is_none(),
        "a control-byte-only rationale MUST be rejected (empty after sanitize)"
    );
}

#[test]
fn engineer_chokepoint_strips_ansi_and_bounds_rationale() {
    let d =
        admit("\u{1b}[31mALERT\u{1b}[0m collides").expect("a sanitizable rationale is accepted");
    assert!(
        !d.rationale().contains('\u{1b}') && d.rationale().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped while preserving the text; got {:?}",
        d.rationale()
    );

    let huge = "x".repeat(1000);
    let bounded = admit(&huge).expect("an oversized rationale is bounded, not rejected");
    assert!(
        bounded.rationale().chars().count() <= 501,
        "rationale must be bounded to 500 chars (+ ellipsis); got {}",
        bounded.rationale().chars().count()
    );
}

// ----- Field-ownership matrix (A6): reject a field a variant does not own ----

#[test]
fn engineer_admit_rejects_every_non_owned_field() {
    // `admit` owns NO extra fields — any of the four is a smuggle ⇒ rejected.
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "admit",
            "r",
            &["g".to_string()],
            "",
            &[],
            None
        )
        .is_none(),
        "admit + blocked_by MUST be rejected (blocked_by is owned by defer)"
    );
    assert!(
        EngineerAdmissionDecision::from_choice_fields("admit", "r", &[], "", &[], Some(1))
            .is_none(),
        "admit + retry_after_secs MUST be rejected (owned by defer)"
    );
    assert!(
        EngineerAdmissionDecision::from_choice_fields("admit", "r", &[], "gid", &[], None)
            .is_none(),
        "admit + after_goal_id MUST be rejected (owned by serialize_after)"
    );
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "admit",
            "r",
            &[],
            "",
            &["f".to_string()],
            None
        )
        .is_none(),
        "admit + overlap_files MUST be rejected (owned by serialize_after)"
    );
}

#[test]
fn engineer_defer_rejects_serialize_after_owned_fields() {
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "defer",
            "r",
            &["g".to_string()],
            "gid",
            &[],
            None
        )
        .is_none(),
        "defer + after_goal_id MUST be rejected (owned by serialize_after)"
    );
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "defer",
            "r",
            &["g".to_string()],
            "",
            &["f".to_string()],
            None
        )
        .is_none(),
        "defer + overlap_files MUST be rejected (owned by serialize_after)"
    );
}

#[test]
fn engineer_serialize_after_rejects_defer_owned_fields() {
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "serialize_after",
            "r",
            &["g".to_string()],
            "gid",
            &["f".to_string()],
            None
        )
        .is_none(),
        "serialize_after + blocked_by MUST be rejected (owned by defer)"
    );
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "serialize_after",
            "r",
            &[],
            "gid",
            &["f".to_string()],
            Some(1)
        )
        .is_none(),
        "serialize_after + retry_after_secs MUST be rejected (owned by defer)"
    );
}

#[test]
fn engineer_serialize_after_requires_a_non_empty_after_goal_id() {
    assert!(
        EngineerAdmissionDecision::from_choice_fields(
            "serialize_after",
            "r",
            &[],
            "",
            &["f".to_string()],
            None
        )
        .is_none(),
        "serialize_after MUST name the goal to rebase after — an empty after_goal_id is rejected"
    );
}

// ----- R8: every variant round-trips through the record bit-for-bit ----------

#[test]
fn read_verified_admission_round_trips_admit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = admit("independent files").unwrap();
    let path = write_json(dir.path(), &engineer_record(d.clone()));
    let read = read_verified_admission(&path, GOAL, CYCLE).expect("valid admit record reads Ok");
    assert_eq!(read, d, "reader must return the exact recorded decision");
}

#[test]
fn read_verified_admission_round_trips_defer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = EngineerAdmissionDecision::from_choice_fields(
        "defer",
        "collision on goals_status.rs",
        &["render-goals-status".to_string()],
        "",
        &[],
        Some(600),
    )
    .unwrap();
    let path = write_json(dir.path(), &engineer_record(d.clone()));
    let read = read_verified_admission(&path, GOAL, CYCLE).expect("valid defer record reads Ok");
    assert_eq!(read, d);
}

#[test]
fn read_verified_admission_round_trips_serialize_after() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = EngineerAdmissionDecision::from_choice_fields(
        "serialize_after",
        "rebase behind the rename",
        &[],
        "rename-adapter-to-clients",
        &["src/ooda_actions/mod.rs".to_string()],
        None,
    )
    .unwrap();
    let path = write_json(dir.path(), &engineer_record(d.clone()));
    let read =
        read_verified_admission(&path, GOAL, CYCLE).expect("valid serialize_after record reads Ok");
    assert_eq!(read, d);
}

#[test]
fn admission_record_carries_the_flattened_choice_tag_on_the_wire() {
    // Defense against a serde-shape regression: the decision must flatten so the
    // CLI tool and the enum can never disagree on the wire.
    for v in ENGINEER_VARIANTS {
        let decision = match *v {
            "admit" => admit("r").unwrap(),
            "defer" => EngineerAdmissionDecision::from_choice_fields(
                "defer",
                "r",
                &["g".to_string()],
                "",
                &[],
                None,
            )
            .unwrap(),
            _ => EngineerAdmissionDecision::from_choice_fields(
                "serialize_after",
                "r",
                &[],
                "g",
                &["f".to_string()],
                None,
            )
            .unwrap(),
        };
        let json = serde_json::to_value(engineer_record(decision)).expect("serialize");
        assert_eq!(
            json.get("choice").and_then(|c| c.as_str()),
            Some(*v),
            "the record must carry a flat `choice` discriminator == the snake_case variant"
        );
        assert_eq!(
            json.get("schema").and_then(|s| s.as_str()),
            Some(ADMISSION_SCHEMA),
            "the record must carry the pinned schema string"
        );
    }
}

// ----- R1: file absent ------------------------------------------------------

#[test]
fn engineer_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R1: an absent engineer-admission record MUST be an Err (the act-site then fails OPEN)"
    );
}

// ----- R2: malformed / truncated JSON ---------------------------------------

#[test]
fn engineer_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ not valid json ");
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST be an Err"
    );
}

#[test]
fn engineer_r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) file MUST be an Err"
    );
}

// ----- R3: schema version pin -----------------------------------------------

#[test]
fn engineer_r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.ooda.admission.v2","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit","rationale":"future record"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST be an Err — a v2 writer is never honored by a v1 reader"
    );
}

#[test]
fn engineer_r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit","rationale":"no schema"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R3: a record with no schema field MUST be an Err"
    );
}

// ----- R4: choice not one of the closed variants ----------------------------

#[test]
fn engineer_r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"self_destruct","rationale":"smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag MUST be an Err — a compromised prompt cannot smuggle a novel variant"
    );
}

// ----- R5: rationale missing / empty / control-only -------------------------

#[test]
fn engineer_r5_empty_rationale_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit","rationale":""}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R5: an empty rationale MUST be an Err"
    );
}

#[test]
fn engineer_r5_missing_rationale_field_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R5: a record with no rationale field MUST be an Err"
    );
}

#[test]
fn engineer_r5_control_byte_only_rationale_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{ADMISSION_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"admit\",\"rationale\":\"\\u001b\\u0007\\u0001\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R5: a control-byte-only rationale MUST be an Err (empty after sanitize)"
    );
}

#[test]
fn engineer_reader_sanitizes_ansi_control_from_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{ADMISSION_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"admit\",\"rationale\":\"\\u001b[31mALERT\\u001b[0m independent\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let d = read_verified_admission(&path, GOAL, CYCLE).expect("sanitizable rationale must verify");
    assert!(
        !d.rationale().contains('\u{1b}') && d.rationale().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped on read while preserving the text; got {:?}",
        d.rationale()
    );
}

// ----- R6/R7: identity binding (no stale / prior-cycle / other-goal replay) --

#[test]
fn engineer_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = engineer_record(admit("independent").unwrap());
    record.goal_id = "some-other-goal".to_string();
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the live ctx MUST be an Err"
    );
}

#[test]
fn engineer_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = engineer_record(admit("independent").unwrap());
    record.cycle_number = CYCLE - 1; // last cycle's record lingering on disk
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_admission(&path, GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST be an Err (no replay of a prior verdict)"
    );
}

// ===========================================================================
// RESOURCE-ADMISSION side (fail-CLOSED at the act-site)
// ===========================================================================

/// The canonical three resource-admission variants (closed set).
const RESOURCE_VARIANTS: &[&str] = &["admit", "defer", "reclaim_first"];

fn resource_record(decision: ResourceAdmissionDecision) -> ResourceAdmissionDecisionRecord {
    ResourceAdmissionDecisionRecord {
        schema: RESOURCE_ADMISSION_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        decision,
    }
}

fn resource_choice(variant: &str) -> ResourceAdmissionDecision {
    ResourceAdmissionDecision::from_choice_fields(variant, "a mandatory rationale")
        .unwrap_or_else(|| panic!("`{variant}` must be one of the closed 3 resource variants"))
}

// ----- Schema pin -----------------------------------------------------------

#[test]
fn resource_admission_schema_is_the_pinned_v1_string() {
    assert_eq!(
        RESOURCE_ADMISSION_SCHEMA, "simard.ooda.resource_admission.v1",
        "the resource-admission reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- Chokepoint: exactly the 3 variants, case-insensitive, sanitized -------

#[test]
fn resource_chokepoint_accepts_exactly_the_three_variants() {
    for v in RESOURCE_VARIANTS {
        let d = ResourceAdmissionDecision::from_choice_fields(v, "headroom rationale")
            .unwrap_or_else(|| panic!("variant `{v}` MUST be accepted by the closed chokepoint"));
        assert_eq!(
            d.variant_label(),
            *v,
            "variant_label must round-trip the snake_case tag for `{v}`"
        );
    }
}

#[test]
fn resource_chokepoint_is_case_insensitive() {
    let d = ResourceAdmissionDecision::from_choice_fields("RECLAIM_FIRST", "loud caps")
        .expect("choice must be matched case-insensitively");
    assert_eq!(d.variant_label(), "reclaim_first");
}

#[test]
fn resource_chokepoint_rejects_unknown_choice() {
    assert!(
        ResourceAdmissionDecision::from_choice_fields("serialize_after", "wrong gate").is_none(),
        "an engineer-gate variant MUST be rejected on the resource gate — the closed enum is the authority"
    );
    assert!(
        ResourceAdmissionDecision::from_choice_fields("deploy", "smuggled").is_none(),
        "an unknown choice tag MUST be rejected"
    );
}

#[test]
fn resource_chokepoint_rejects_empty_and_control_only_rationale() {
    assert!(
        ResourceAdmissionDecision::from_choice_fields("defer", "").is_none(),
        "an empty rationale MUST be rejected"
    );
    assert!(
        ResourceAdmissionDecision::from_choice_fields("defer", "   ").is_none(),
        "a whitespace-only rationale MUST be rejected"
    );
    assert!(
        ResourceAdmissionDecision::from_choice_fields("defer", "\u{1b}\u{07}\u{01}").is_none(),
        "a control-byte-only rationale MUST be rejected (empty after sanitize)"
    );
}

#[test]
fn resource_chokepoint_strips_ansi_and_bounds_rationale() {
    let d =
        ResourceAdmissionDecision::from_choice_fields("defer", "\u{1b}[31mALERT\u{1b}[0m tight")
            .expect("a sanitizable rationale is accepted");
    assert!(
        !d.rationale().contains('\u{1b}') && d.rationale().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped while preserving the text; got {:?}",
        d.rationale()
    );

    let huge = "y".repeat(1000);
    let bounded = ResourceAdmissionDecision::from_choice_fields("defer", &huge)
        .expect("an oversized rationale is bounded, not rejected");
    assert!(
        bounded.rationale().chars().count() <= 501,
        "rationale must be bounded to 500 chars (+ ellipsis); got {}",
        bounded.rationale().chars().count()
    );
}

// ----- R8: every variant round-trips through the record ----------------------

#[test]
fn read_verified_resource_admission_round_trips_every_variant() {
    for v in RESOURCE_VARIANTS {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = resource_choice(v);
        let path = write_json(dir.path(), &resource_record(d.clone()));
        let read = read_verified_resource_admission(&path, GOAL, CYCLE)
            .unwrap_or_else(|e| panic!("valid resource record `{v}` must read back Ok, got {e:?}"));
        assert_eq!(
            read, d,
            "read_verified_resource_admission must return the exact recorded decision for `{v}`"
        );
    }
}

#[test]
fn resource_record_carries_the_flattened_choice_tag_on_the_wire() {
    for v in RESOURCE_VARIANTS {
        let json = serde_json::to_value(resource_record(resource_choice(v))).expect("serialize");
        assert_eq!(
            json.get("choice").and_then(|c| c.as_str()),
            Some(*v),
            "the resource record must carry a flat `choice` discriminator == the snake_case variant"
        );
        assert_eq!(
            json.get("schema").and_then(|s| s.as_str()),
            Some(RESOURCE_ADMISSION_SCHEMA)
        );
    }
}

// ----- R1..R7 fail-closed matrix --------------------------------------------

#[test]
fn resource_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R1: an absent resource-admission record MUST be an Err (the act-site then fails CLOSED to Defer)"
    );
}

#[test]
fn resource_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"}{ garbage");
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST be an Err"
    );
}

#[test]
fn resource_r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.ooda.resource_admission.v2","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit","rationale":"future"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST be an Err"
    );
}

#[test]
fn resource_r3_engineer_schema_is_rejected_on_the_resource_reader() {
    // Cross-record confusion guard: an engineer-admission record must not be
    // honored by the resource reader (distinct schema pins).
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"admit","rationale":"wrong record type"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R3: the resource reader MUST reject an engineer-admission-schema record"
    );
}

#[test]
fn resource_r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{RESOURCE_ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"nuke","rationale":"smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag MUST be an Err"
    );
}

#[test]
fn resource_r5_empty_and_missing_rationale_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let empty = format!(
        r#"{{"schema":"{RESOURCE_ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"defer","rationale":""}}"#
    );
    let path = write_bytes(dir.path(), empty.as_bytes());
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R5: an empty rationale MUST be an Err"
    );

    let dir2 = tempfile::tempdir().expect("tempdir");
    let missing = format!(
        r#"{{"schema":"{RESOURCE_ADMISSION_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"defer"}}"#
    );
    let path2 = write_bytes(dir2.path(), missing.as_bytes());
    assert!(
        read_verified_resource_admission(&path2, GOAL, CYCLE).is_err(),
        "R5: a record with no rationale field MUST be an Err"
    );
}

#[test]
fn resource_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = resource_record(resource_choice("admit"));
    record.goal_id = "other-goal".to_string();
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the live ctx MUST be an Err"
    );
}

#[test]
fn resource_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = resource_record(resource_choice("admit"));
    record.cycle_number = CYCLE + 1;
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_resource_admission(&path, GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST be an Err"
    );
}
