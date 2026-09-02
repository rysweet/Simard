//! TDD **failing** tests for the `simard ooda record-decide` and
//! `simard ooda record-orient` CLI writer tools (Group A — issue #4785).
//!
//! These specify the behavior of the two new dispatch arms (reached through the
//! public `dispatch_operator_cli` entry point, exactly like the existing
//! `ooda record-decision` tests from #4734). Neither subcommand exists yet, so
//! every test here fails until the Builder phase adds the match arms.
//!
//! Contract (see `docs/reference/ooda-record-orient-decide-cli.md`):
//!   * Each tool validates its typed fields through the SINGLE shared chokepoint
//!     (`DecideChoice::from_choice_fields` / `OrientFields::from_fields`),
//!     hardens `--record-path` (absolute, no `..`), then writes EXACTLY ONE
//!     atomic `0o600` record. Any validation failure ⇒ non-zero exit AND
//!     **no file on disk** (validate-all-then-write-once).
//!   * Free text (`reason`) is bounded + sanitized via
//!     `sanitize_context_var(_, 500)` (ANSI/C0 stripped, ≤ 500 chars).
//!   * `record-decide --choice` is the closed 10-variant enum (case-insensitive).
//!   * `record-orient` REQUIRES `--adjusted-urgency`, `--confidence`,
//!     `--demotion-applied`, and `--base-urgency` (the typed CLI deliberately
//!     tightens `confidence`/`demotion_applied` to required, diverging from the
//!     legacy wire `OrientJudgment`'s `#[serde(default)]` behaviour), rejects
//!     escalation (adjusted > base) and non-finite / out-of-range values.
//!   * Both tools are zero-privilege: their only side effect is that one write.

use crate::operator_cli::dispatch_operator_cli;

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

fn read_decide(path: &std::path::Path) -> crate::ooda_brain::DecideDecisionRecord {
    let bytes = std::fs::read(path).expect("decide record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into DecideDecisionRecord")
}

fn read_orient(path: &std::path::Path) -> crate::ooda_brain::OrientDecisionRecord {
    let bytes = std::fs::read(path).expect("orient record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into OrientDecisionRecord")
}

// ===========================================================================
// `simard ooda record-decide`
// ===========================================================================

/// All ten closed decide variants the tool must accept (requirements item 1).
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

#[test]
fn record_decide_valid_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "consolidate_memory",
        "--reason",
        "12 unconsolidated session memories",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "__memory__",
        "--cycle-number",
        "4287",
    ])
    .expect("a valid decide decision must exit Ok");

    let record = read_decide(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::DECIDE_SCHEMA);
    assert_eq!(record.goal_id, "__memory__");
    assert_eq!(record.cycle_number, 4287);
    assert_eq!(record.choice.variant_label(), "consolidate_memory");
    assert_eq!(record.choice.reason(), "12 unconsolidated session memories");
}

#[test]
fn record_decide_accepts_all_ten_variants() {
    for v in DECIDE_VARIANTS {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("decide.json");
        run(&[
            "ooda",
            "record-decide",
            "--choice",
            v,
            "--reason",
            "routing rationale",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "g1",
            "--cycle-number",
            "1",
        ])
        .unwrap_or_else(|e| panic!("variant `{v}` MUST be accepted, got {e}"));
        assert_eq!(read_decide(&record_path).choice.variant_label(), *v);
    }
}

#[test]
fn record_decide_choice_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "ADVANCE_GOAL",
        "--reason",
        "ordinary slug",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "7",
    ])
    .expect("upper-case choice must be accepted (matched case-insensitively)");
    assert_eq!(
        read_decide(&record_path).choice.variant_label(),
        "advance_goal"
    );
}

#[test]
fn record_decide_out_of_enum_choice_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "self_destruct",
        "--reason",
        "smuggled action",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(result.is_err(), "an out-of-enum choice MUST be rejected");
    assert!(
        !record_path.exists(),
        "a rejected decision MUST NOT write any record (validate-all-then-write-once)"
    );
}

#[test]
fn record_decide_empty_reason_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --reason MUST be rejected"
    );
    assert!(!record_path.exists(), "no record on an empty reason");
}

#[test]
fn record_decide_reason_read_from_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(&reason_file, "run the gym-driven self-improvement loop").unwrap();
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "run_improvement",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "__improvement__",
        "--cycle-number",
        "1",
    ])
    .expect("a file-sourced reason must be accepted");
    assert_eq!(
        read_decide(&record_path).choice.reason(),
        "run the gym-driven self-improvement loop"
    );
}

#[test]
fn record_decide_reason_and_reason_path_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(&reason_file, "from file").unwrap();
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "inline",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "both --reason and --reason-path MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_decide_oversized_reason_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let huge = "x".repeat(1000);
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        &huge,
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("an oversized reason is bounded, not rejected");
    let reason = read_decide(&record_path).choice.reason().to_string();
    assert!(
        reason.chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {}",
        reason.chars().count()
    );
}

#[test]
fn record_decide_ansi_control_stripped_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "\u{1b}[31mALERT\u{1b}[0m worker\u{07} went quiet",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("a sanitizable reason must be accepted");
    let reason = read_decide(&record_path).choice.reason().to_string();
    assert!(
        !reason.contains('\u{1b}') && !reason.contains('\u{07}') && reason.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped before the record is written; got {reason:?}"
    );
}

#[test]
fn record_decide_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "relative path attempt",
        "--record-path",
        "relative/decide.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL-8)"
    );
    assert!(!std::path::Path::new("relative/decide.json").exists());
}

#[test]
fn record_decide_parent_traversal_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "traversal attempt",
        "--record-path",
        traversal.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a --record-path containing '..' MUST be rejected (SR-VAL-8)"
    );
}

#[test]
fn record_decide_missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "missing goal id",
        "--record-path",
        record_path.to_str().unwrap(),
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a missing required option MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_decide_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    let result = run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "has an unknown flag",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
        "--bogus",
        "value",
    ]);
    assert!(
        result.is_err(),
        "an unknown flag MUST be rejected — never silently ignored"
    );
}

// ===========================================================================
// `simard ooda record-orient`
// ===========================================================================

#[test]
fn record_orient_valid_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "0.90",
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        "two transient failures",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "cognition-research",
        "--cycle-number",
        "4287",
    ])
    .expect("a valid orient judgment must exit Ok");

    let record = read_orient(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::ORIENT_SCHEMA);
    assert_eq!(record.goal_id, "cognition-research");
    assert_eq!(record.cycle_number, 4287);
    assert!(
        (record.base_urgency - 0.80).abs() < 1e-9,
        "the base_urgency MUST be persisted so the reader can re-check the no-escalation invariant"
    );
    assert!((record.fields.adjusted_urgency - 0.40).abs() < 1e-9);
    assert!((record.fields.confidence - 0.90).abs() < 1e-9);
    assert!((record.fields.demotion_applied - 0.40).abs() < 1e-9);
    assert_eq!(record.fields.reason, "two transient failures");
}

#[test]
fn record_orient_escalation_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.95", // > base_urgency 0.80 — escalation
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.0",
        "--base-urgency",
        "0.80",
        "--reason",
        "escalation attempt",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "adjusted_urgency > base_urgency MUST be rejected (escalation forbidden)"
    );
    assert!(!record_path.exists(), "no record on a rejected escalation");
}

#[test]
fn record_orient_non_finite_urgency_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "inf",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.0",
        "--base-urgency",
        "0.80",
        "--reason",
        "infinite urgency",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a non-finite adjusted_urgency MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_out_of_range_urgency_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "1.5",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.0",
        "--base-urgency",
        "2.0",
        "--reason",
        "out of range",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "an adjusted_urgency outside [0,1] MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_empty_reason_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --reason MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_missing_confidence_rejected() {
    // The typed CLI deliberately TIGHTENS confidence/demotion_applied to
    // required (diverging from the legacy wire `#[serde(default)]`), so a
    // record missing them fails rather than synthesizing a default.
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        // --confidence omitted
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        "missing confidence",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "--confidence MUST be required (no default synthesized) — writer/reader consistency"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_missing_demotion_applied_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        // --demotion-applied omitted
        "--base-urgency",
        "0.80",
        "--reason",
        "missing demotion",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "--demotion-applied MUST be required (deliberate tightening)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_missing_base_urgency_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.40",
        // --base-urgency omitted — without it the reader cannot re-check escalation
        "--reason",
        "missing base",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "--base-urgency MUST be required (persisted for the reader's re-check)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_orient_oversized_reason_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let huge = "y".repeat(1000);
    run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        &huge,
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("an oversized reason is bounded, not rejected");
    let reason = read_orient(&record_path).fields.reason;
    assert!(
        reason.chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {}",
        reason.chars().count()
    );
}

#[test]
fn record_orient_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        "relative path attempt",
        "--record-path",
        "relative/orient.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL-8)"
    );
    assert!(!std::path::Path::new("relative/orient.json").exists());
}

#[test]
fn record_orient_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    let result = run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.40",
        "--confidence",
        "1.0",
        "--demotion-applied",
        "0.40",
        "--base-urgency",
        "0.80",
        "--reason",
        "has an unknown flag",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
        "--bogus",
        "value",
    ]);
    assert!(
        result.is_err(),
        "an unknown flag MUST be rejected — never silently ignored"
    );
}

// ---------------------------------------------------------------------------
// Round-trip through the CLI writer AND the readers: what one writes, the other
// must accept (the anti-drift guarantee — a single shared chokepoint per record
// type invoked by BOTH writer and reader).
// ---------------------------------------------------------------------------

#[test]
fn record_decide_written_by_cli_reads_back_via_read_verified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decide.json");
    run(&[
        "ooda",
        "record-decide",
        "--choice",
        "advance_goal",
        "--reason",
        "standard development goal",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "ship-v1",
        "--cycle-number",
        "9",
    ])
    .expect("valid decide write");
    let choice = crate::ooda_brain::read_verified_decide(&record_path, "ship-v1", 9)
        .expect("the reader MUST accept what the writer produced (anti-drift)");
    assert_eq!(choice.variant_label(), "advance_goal");
}

#[test]
fn record_orient_written_by_cli_reads_back_via_read_verified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("orient.json");
    run(&[
        "ooda",
        "record-orient",
        "--adjusted-urgency",
        "0.30",
        "--confidence",
        "0.95",
        "--demotion-applied",
        "0.50",
        "--base-urgency",
        "0.80",
        "--reason",
        "chronic failures",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "ship-v1",
        "--cycle-number",
        "9",
    ])
    .expect("valid orient write");
    let fields = crate::ooda_brain::read_verified_orient(&record_path, "ship-v1", 9)
        .expect("the reader MUST accept what the writer produced (anti-drift)");
    assert!((fields.adjusted_urgency - 0.30).abs() < 1e-9);
}
