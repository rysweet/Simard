//! TDD **failing** tests for the typed Orient + Decide decision RECORDS and
//! their fail-CLOSED readers (Group A — OODA orient + decide; issue #4785).
//!
//! These specify the seam that REPLACES the forbidden "recipe prints
//! JSON/decimal → Rust scrapes prose → Rust acts" pattern on the Orient and
//! Decide phases, exactly as #4734 (`tests_record_decision.rs`) did for the
//! per-goal-cycle phase. They reference symbols that DO NOT EXIST YET, so the
//! crate will not compile until the Builder phase adds them:
//!
//!   * `super::DECIDE_SCHEMA` / `super::ORIENT_SCHEMA` — pinned schema strings
//!     (`"simard.ooda.decide.v1"` / `"simard.ooda.orient.v1"`).
//!   * `super::DecideChoice` — closed 10-variant `#[serde(tag = "choice",
//!     rename_all = "snake_case")]` enum, each variant `{ reason: String }`,
//!     with the SINGLE shared chokepoint
//!     `DecideChoice::from_choice_fields(choice, reason) -> Option<DecideChoice>`.
//!   * `super::OrientFields` — `{ adjusted_urgency, confidence, demotion_applied,
//!     reason }`, with the SINGLE shared chokepoint
//!     `OrientFields::from_fields(adjusted, confidence, demotion, reason,
//!     base_urgency) -> Result<OrientFields, String>`.
//!   * `super::DecideDecisionRecord` / `super::OrientDecisionRecord` — the typed
//!     on-disk records (the orient record ALSO persists `base_urgency` so the
//!     reader re-validates the `adjusted ≤ base` invariant self-consistently).
//!   * `super::read_verified_decide` / `super::read_verified_orient` — the
//!     readers that enforce the fail-CLOSED matrix.
//!
//! The whole point of this seam is that EVERY failure mode is an `Err` (a safe
//! no-op: the Decide caller SKIPS the priority, the Orient caller KEEPS the base
//! urgency), never a default action / synthesized demotion (#1711). The
//! `read_verified_*` matrix (R1–R8) is the load-bearing invariant and each row
//! is asserted here for BOTH record types.
//!
//! Reference contract: `docs/reference/ooda-record-orient-decide-cli.md`.

use std::path::Path;

use super::{
    DECIDE_SCHEMA, DecideChoice, DecideDecisionRecord, ORIENT_SCHEMA, OrientDecisionRecord,
    OrientFields, read_verified_decide, read_verified_orient,
};
use crate::ooda_loop::ActionKind;

// ---------------------------------------------------------------------------
// Shared hermetic helpers — a temp dir + direct record writer (no CLI, no
// subprocess). The reader tests DELIBERATELY write the on-disk bytes themselves
// (via serde for valid cases, raw strings for hostile cases) so they exercise
// the readers in ISOLATION from the writer tool: the defense-in-depth guarantee
// is that even a record the tool would never produce must fail CLOSED.
// ---------------------------------------------------------------------------

const GOAL: &str = "continuously-research-and-improve-your-own-cogn-70ab8541";
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

// ===========================================================================
// DECIDE side
// ===========================================================================

/// The canonical 10 decide variants (requirements item 1): enumerated from
/// `DecideJudgment` / `decide_judgment_from_variant`. The closed-enum chokepoint
/// must accept EXACTLY these ten — no more, no fewer.
const DECIDE_VARIANTS: &[&str] = &[
    "poll_developer_activity",
    "consolidate_memory",
    "run_improvement",
    "extract_ideas",
    "safe_update",
    "research_query",
    "run_gym_eval",
    "build_skill",
    "launch_session",
    "advance_goal",
];

fn decide_record(choice: DecideChoice) -> DecideDecisionRecord {
    DecideDecisionRecord {
        schema: DECIDE_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        choice,
    }
}

fn decide_choice(variant: &str) -> DecideChoice {
    DecideChoice::from_choice_fields(variant, "a mandatory decide rationale")
        .unwrap_or_else(|| panic!("`{variant}` must be one of the closed 10 decide variants"))
}

// ----- Schema pin -----------------------------------------------------------

#[test]
fn decide_schema_is_the_pinned_v1_string() {
    assert_eq!(
        DECIDE_SCHEMA, "simard.ooda.decide.v1",
        "the decide reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- Chokepoint: exactly the 10 variants, case-insensitive, sanitized ------

#[test]
fn decide_chokepoint_accepts_exactly_the_ten_variants() {
    for v in DECIDE_VARIANTS {
        let choice = DecideChoice::from_choice_fields(v, "routing rationale")
            .unwrap_or_else(|| panic!("variant `{v}` MUST be accepted by the closed chokepoint"));
        assert_eq!(
            choice.variant_label(),
            *v,
            "variant_label must round-trip the snake_case tag for `{v}`"
        );
    }
}

#[test]
fn decide_chokepoint_is_case_insensitive() {
    let choice = DecideChoice::from_choice_fields("ADVANCE_GOAL", "loud caps")
        .expect("choice must be matched case-insensitively");
    assert_eq!(choice.variant_label(), "advance_goal");
}

#[test]
fn decide_chokepoint_rejects_unknown_choice() {
    assert!(
        DecideChoice::from_choice_fields("deploy", "smuggled action").is_none(),
        "an unknown choice tag (`deploy`) MUST be rejected — the closed enum is the sole authority"
    );
}

#[test]
fn decide_chokepoint_rejects_empty_reason() {
    assert!(
        DecideChoice::from_choice_fields("advance_goal", "").is_none(),
        "an empty reason MUST be rejected (fail CLOSED)"
    );
    assert!(
        DecideChoice::from_choice_fields("advance_goal", "   ").is_none(),
        "a whitespace-only reason MUST be rejected (empty after trim)"
    );
}

#[test]
fn decide_chokepoint_rejects_control_only_reason() {
    // A reason made up ENTIRELY of ANSI/C0 control bytes sanitizes to empty ⇒
    // rejected. A bare `trim()` would wrongly accept it (ESC is not whitespace).
    assert!(
        DecideChoice::from_choice_fields("run_improvement", "\u{1b}\u{07}\u{01}").is_none(),
        "a control-byte-only reason MUST be rejected (empty after sanitize)"
    );
}

#[test]
fn decide_chokepoint_strips_ansi_and_bounds_reason() {
    let choice =
        DecideChoice::from_choice_fields("advance_goal", "\u{1b}[31mALERT\u{1b}[0m route now")
            .expect("a sanitizable reason must be accepted");
    assert!(
        !choice.reason().contains('\u{1b}') && choice.reason().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped while preserving the text; got {:?}",
        choice.reason()
    );

    let huge = "x".repeat(1000);
    let bounded = DecideChoice::from_choice_fields("advance_goal", &huge)
        .expect("an oversized reason is bounded, not rejected");
    assert!(
        bounded.reason().chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {}",
        bounded.reason().chars().count()
    );
}

#[test]
fn decide_choice_projects_onto_action_kind() {
    // Each variant projects back onto the existing ActionKind enum so the Decide
    // phase keeps emitting PlannedAction values unchanged.
    let cases = [
        ("advance_goal", ActionKind::AdvanceGoal),
        ("run_improvement", ActionKind::RunImprovement),
        ("consolidate_memory", ActionKind::ConsolidateMemory),
        ("research_query", ActionKind::ResearchQuery),
        ("run_gym_eval", ActionKind::RunGymEval),
        ("build_skill", ActionKind::BuildSkill),
        ("launch_session", ActionKind::LaunchSession),
        ("poll_developer_activity", ActionKind::PollDeveloperActivity),
        ("extract_ideas", ActionKind::ExtractIdeas),
        ("safe_update", ActionKind::SafeUpdate),
    ];
    for (variant, kind) in cases {
        assert_eq!(
            decide_choice(variant).action_kind(),
            kind,
            "`{variant}` must project onto {kind:?}"
        );
    }
}

// ----- R8: all 10 variants round-trip through the record bit-for-bit ---------

#[test]
fn read_verified_decide_round_trips_every_variant() {
    for v in DECIDE_VARIANTS {
        let dir = tempfile::tempdir().expect("tempdir");
        let choice = decide_choice(v);
        let path = write_json(dir.path(), &decide_record(choice.clone()));
        let read = read_verified_decide(&path, GOAL, CYCLE)
            .unwrap_or_else(|e| panic!("valid decide record `{v}` must read back Ok, got {e:?}"));
        assert_eq!(
            read, choice,
            "read_verified_decide must return the exact recorded choice for `{v}`"
        );
    }
}

// ----- R1: file absent ------------------------------------------------------

#[test]
fn decide_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R1: an absent decide record MUST fail CLOSED (Err → skip the priority), never a default action"
    );
}

// ----- R2: malformed / truncated JSON ---------------------------------------

#[test]
fn decide_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ not valid json ");
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST fail CLOSED"
    );
}

#[test]
fn decide_r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) file MUST fail CLOSED"
    );
}

// ----- R3: schema version pin -----------------------------------------------

#[test]
fn decide_r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.ooda.decide.v2","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"advance_goal","reason":"future record"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST fail CLOSED — a v2 writer is never honored by a v1 reader"
    );
}

#[test]
fn decide_r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"advance_goal","reason":"no schema"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R3: a record with no schema field MUST fail CLOSED"
    );
}

// ----- R4: choice not one of the 10 closed variants -------------------------

#[test]
fn decide_r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{DECIDE_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"self_destruct","reason":"smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag MUST fail CLOSED — a compromised prompt cannot smuggle a novel action"
    );
}

// ----- R5: reason missing / empty / control-only ----------------------------

#[test]
fn decide_r5_empty_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{DECIDE_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"advance_goal","reason":""}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R5: an empty reason MUST fail CLOSED"
    );
}

#[test]
fn decide_r5_missing_reason_field_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{DECIDE_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"wait"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R5: a record with no reason field MUST fail CLOSED"
    );
}

#[test]
fn decide_r5_control_byte_only_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{DECIDE_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"advance_goal\",\"reason\":\"\\u001b\\u0007\\u0001\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R5: a control-byte-only reason MUST fail CLOSED (empty after sanitize)"
    );
}

#[test]
fn decide_reader_sanitizes_ansi_control_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{DECIDE_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"consolidate_memory\",\"reason\":\"\\u001b[31mALERT\\u001b[0m consolidate now\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let choice = read_verified_decide(&path, GOAL, CYCLE).expect("sanitizable reason must verify");
    assert!(
        !choice.reason().contains('\u{1b}') && choice.reason().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped on read while preserving the text; got {:?}",
        choice.reason()
    );
}

// ----- R6/R7: identity binding (no stale / prior-cycle / other-goal replay) --

#[test]
fn decide_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = decide_record(decide_choice("advance_goal"));
    record.goal_id = "some-other-goal".to_string();
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the live ctx MUST fail CLOSED"
    );
}

#[test]
fn decide_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = decide_record(decide_choice("advance_goal"));
    record.cycle_number = CYCLE - 1; // last cycle's record lingering on disk
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_decide(&path, GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST fail CLOSED (no replay of a prior verdict)"
    );
}

// ===========================================================================
// ORIENT side
// ===========================================================================

const BASE_URGENCY: f64 = 0.80;

fn orient_fields() -> OrientFields {
    OrientFields::from_fields(0.40, 0.90, 0.40, "two transient failures", BASE_URGENCY)
        .expect("a valid, non-escalating orient judgment must construct")
}

fn orient_record(fields: OrientFields) -> OrientDecisionRecord {
    OrientDecisionRecord {
        schema: ORIENT_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        base_urgency: BASE_URGENCY,
        fields,
    }
}

// ----- Schema pin -----------------------------------------------------------

#[test]
fn orient_schema_is_the_pinned_v1_string() {
    assert_eq!(
        ORIENT_SCHEMA, "simard.ooda.orient.v1",
        "the orient reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- Chokepoint: finite, [0,1], no-escalation, sanitized+bounded reason ----

#[test]
fn orient_chokepoint_accepts_valid_non_escalating_judgment() {
    let f = OrientFields::from_fields(0.30, 0.85, 0.50, "chronic failures", 0.80)
        .expect("a valid judgment must construct");
    assert!((f.adjusted_urgency - 0.30).abs() < 1e-9);
    assert!((f.confidence - 0.85).abs() < 1e-9);
    assert!((f.demotion_applied - 0.50).abs() < 1e-9);
    assert_eq!(f.reason, "chronic failures");
}

#[test]
fn orient_chokepoint_allows_echoing_base_urgency_within_fp_slack() {
    // A brain echoing base_urgency EXACTLY (zero demotion) must not trip on
    // rounding — mirrors `OrientJudgment::validate`'s 1e-9 slack.
    assert!(
        OrientFields::from_fields(0.80, 1.0, 0.0, "no demotion warranted", 0.80).is_ok(),
        "adjusted == base_urgency MUST be accepted (no escalation)"
    );
}

#[test]
fn orient_chokepoint_rejects_escalation() {
    assert!(
        OrientFields::from_fields(0.90, 1.0, 0.0, "tries to escalate", 0.80).is_err(),
        "adjusted_urgency > base_urgency MUST be rejected (escalation forbidden)"
    );
}

#[test]
fn orient_chokepoint_rejects_out_of_range_urgency() {
    assert!(
        OrientFields::from_fields(1.5, 1.0, 0.0, "too big", 2.0).is_err(),
        "adjusted_urgency outside [0,1] MUST be rejected even when ≤ base_urgency"
    );
    assert!(
        OrientFields::from_fields(-0.1, 1.0, 0.0, "negative", 0.8).is_err(),
        "a negative adjusted_urgency MUST be rejected"
    );
}

#[test]
fn orient_chokepoint_rejects_non_finite() {
    assert!(
        OrientFields::from_fields(f64::NAN, 1.0, 0.0, "nan", 0.8).is_err(),
        "a non-finite adjusted_urgency MUST be rejected"
    );
    assert!(
        OrientFields::from_fields(f64::INFINITY, 1.0, 0.0, "inf", 0.8).is_err(),
        "an infinite adjusted_urgency MUST be rejected"
    );
}

#[test]
fn orient_chokepoint_rejects_out_of_range_confidence() {
    assert!(
        OrientFields::from_fields(0.4, 1.5, 0.4, "conf too high", 0.8).is_err(),
        "confidence outside [0,1] MUST be rejected (chokepoint superset of OrientJudgment::validate)"
    );
    assert!(
        OrientFields::from_fields(0.4, f64::NAN, 0.4, "conf nan", 0.8).is_err(),
        "a non-finite confidence MUST be rejected"
    );
}

#[test]
fn orient_chokepoint_rejects_empty_or_control_only_reason() {
    assert!(
        OrientFields::from_fields(0.4, 1.0, 0.4, "", 0.8).is_err(),
        "an empty reason MUST be rejected"
    );
    assert!(
        OrientFields::from_fields(0.4, 1.0, 0.4, "   ", 0.8).is_err(),
        "a whitespace-only reason MUST be rejected"
    );
    assert!(
        OrientFields::from_fields(0.4, 1.0, 0.4, "\u{1b}\u{07}", 0.8).is_err(),
        "a control-byte-only reason MUST be rejected (empty after sanitize)"
    );
}

#[test]
fn orient_chokepoint_strips_ansi_and_bounds_reason() {
    let f = OrientFields::from_fields(0.4, 1.0, 0.4, "\u{1b}[31mALERT\u{1b}[0m demote", 0.8)
        .expect("a sanitizable reason must be accepted");
    assert!(
        !f.reason.contains('\u{1b}') && f.reason.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped while preserving the text; got {:?}",
        f.reason
    );
    let huge = "y".repeat(1000);
    let bounded = OrientFields::from_fields(0.4, 1.0, 0.4, &huge, 0.8)
        .expect("an oversized reason is bounded, not rejected");
    assert!(
        bounded.reason.chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {}",
        bounded.reason.chars().count()
    );
}

// ----- R8: a valid orient record round-trips its fields bit-for-bit ----------

#[test]
fn read_verified_orient_round_trips_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fields = orient_fields();
    let path = write_json(dir.path(), &orient_record(fields.clone()));
    let read = read_verified_orient(&path, GOAL, CYCLE).expect("valid orient record must read Ok");
    assert!((read.adjusted_urgency - fields.adjusted_urgency).abs() < 1e-9);
    assert!((read.confidence - fields.confidence).abs() < 1e-9);
    assert!((read.demotion_applied - fields.demotion_applied).abs() < 1e-9);
    assert_eq!(read.reason, fields.reason);
}

// ----- R1: file absent ------------------------------------------------------

#[test]
fn orient_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R1: an absent orient record MUST fail CLOSED (Err → keep base urgency), never a synthesized demotion"
    );
}

// ----- R2: malformed / truncated JSON ---------------------------------------

#[test]
fn orient_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"not json at all");
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST fail CLOSED"
    );
}

#[test]
fn orient_r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) orient file MUST fail CLOSED (parity with the decide reader)"
    );
}

// ----- R3: schema version pin -----------------------------------------------

#[test]
fn orient_r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.ooda.orient.v2","goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":{BASE_URGENCY},"adjusted_urgency":0.4,"confidence":1.0,"demotion_applied":0.4,"reason":"future"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST fail CLOSED"
    );
}

#[test]
fn orient_r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":{BASE_URGENCY},"adjusted_urgency":0.4,"confidence":1.0,"demotion_applied":0.4,"reason":"no schema"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R3: an orient record with no schema field MUST fail CLOSED (parity with the decide reader)"
    );
}

// ----- R4 (orient analog): invalid numerics / escalation --------------------
// The persisted `base_urgency` lets the reader re-run the SAME chokepoint, so a
// hostile record whose adjusted_urgency escalates above the recorded base is
// rejected even when goal_id + cycle_number match (the anti-drift self-check).

#[test]
fn orient_r4_escalating_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Structurally valid, correct goal/cycle — but adjusted (0.95) > base (0.80).
    let json = format!(
        r#"{{"schema":"{ORIENT_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":{BASE_URGENCY},"adjusted_urgency":0.95,"confidence":1.0,"demotion_applied":0.0,"reason":"escalation smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R4: an escalating record (adjusted > persisted base_urgency) MUST fail CLOSED via the reader's re-check"
    );
}

#[test]
fn orient_r4_out_of_range_urgency_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ORIENT_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":2.0,"adjusted_urgency":1.5,"confidence":1.0,"demotion_applied":0.5,"reason":"out of range"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R4: an adjusted_urgency outside [0,1] MUST fail CLOSED"
    );
}

// ----- R5: reason missing / empty -------------------------------------------

#[test]
fn orient_r5_empty_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ORIENT_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":{BASE_URGENCY},"adjusted_urgency":0.4,"confidence":1.0,"demotion_applied":0.4,"reason":""}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R5: an empty orient reason MUST fail CLOSED"
    );
}

#[test]
fn orient_r5_control_byte_only_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{ORIENT_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"base_urgency\":{BASE_URGENCY},\"adjusted_urgency\":0.4,\"confidence\":1.0,\"demotion_applied\":0.4,\"reason\":\"\\u001b\\u0007\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R5: a control-byte-only orient reason MUST fail CLOSED (empty after sanitize)"
    );
}

#[test]
fn orient_r5_missing_reason_field_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{ORIENT_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"base_urgency":{BASE_URGENCY},"adjusted_urgency":0.4,"confidence":1.0,"demotion_applied":0.4}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R5: an orient record with no reason field MUST fail CLOSED (parity with the decide reader)"
    );
}

#[test]
fn orient_reader_sanitizes_ansi_control_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{ORIENT_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"base_urgency\":{BASE_URGENCY},\"adjusted_urgency\":0.4,\"confidence\":1.0,\"demotion_applied\":0.4,\"reason\":\"\\u001b[31mALERT\\u001b[0m keep base urgency\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let fields = read_verified_orient(&path, GOAL, CYCLE).expect("sanitizable reason must verify");
    assert!(
        !fields.reason.contains('\u{1b}') && fields.reason.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped on read while preserving the text (parity with the decide reader); got {:?}",
        fields.reason
    );
}

// ----- R6/R7: identity binding ----------------------------------------------

#[test]
fn orient_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = orient_record(orient_fields());
    record.goal_id = "some-other-goal".to_string();
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R6: an orient record for a different goal MUST fail CLOSED"
    );
}

#[test]
fn orient_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = orient_record(orient_fields());
    record.cycle_number = CYCLE - 1;
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_orient(&path, GOAL, CYCLE).is_err(),
        "R7: an orient record from a prior cycle MUST fail CLOSED (no replay)"
    );
}
