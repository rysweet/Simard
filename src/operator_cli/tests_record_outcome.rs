//! Tests for the `simard ooda record-outcome` CLI tool (Group D of epic #4719,
//! issue #4967 — the closed-loop OUTCOME-VERIFICATION seam).
//!
//! These specify `dispatch_record_outcome` (reached through the public
//! `dispatch_operator_cli` entry point, exactly like the `record-decision`
//! tests). Contract:
//!   * Validate the closed 4-variant `--choice` enum (`mark_achieved`,
//!     `reopen`, `replan`, `keep_open_and_report`), require a non-empty
//!     `--reason`, harden `--record-path` (absolute, no `..`), then write
//!     EXACTLY ONE atomic `0o600` `OutcomeDecisionRecord`. Any validation
//!     failure ⇒ non-zero exit AND **no file on disk**.
//!   * `--replan-hint` is OWNED by `replan` (optional even there) and REJECTED
//!     on every other choice.
//!   * Free text (`reason` / `replan-hint`) is bounded + sanitized via
//!     `sanitize_context_var(_, 500)` (ANSI/C0 stripped, ≤ 500 chars).
//!   * `RecipeBrain` reads the record back via `read_verified_outcome`; the
//!     round-trip through that reader is exercised here too.

use crate::ooda_brain::{GoalOutcomeDecision, OUTCOME_SCHEMA, OutcomeDecisionRecord};
use crate::operator_cli::dispatch_operator_cli;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

/// Read the record the tool wrote back into the typed struct (proving the tool
/// and the reader agree on the on-disk shape).
fn read_record(path: &std::path::Path) -> OutcomeDecisionRecord {
    let bytes = std::fs::read(path).expect("record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into OutcomeDecisionRecord")
}

// ---------------------------------------------------------------------------
// Happy path — each of the four variants writes exactly one typed record that
// `read_verified_outcome` accepts.
// ---------------------------------------------------------------------------

#[test]
fn valid_mark_achieved_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "mark_achieved",
        "--reason",
        "self_metrics threshold_crossed (verified) confirms the live effect",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "kgpacks-e2big",
        "--cycle-number",
        "0",
    ])
    .expect("a valid mark_achieved outcome must exit Ok");

    let record = read_record(&record_path);
    assert_eq!(record.schema, OUTCOME_SCHEMA);
    assert_eq!(record.goal_id, "kgpacks-e2big");
    assert_eq!(record.cycle_number, 0);
    assert!(matches!(
        record.decision,
        GoalOutcomeDecision::MarkAchieved { .. }
    ));

    // The reader (RecipeBrain's read path) must accept the exact record.
    let decided = crate::ooda_brain::read_verified_outcome(&record_path, "kgpacks-e2big", 0)
        .expect("read_verified_outcome must accept the tool's own record");
    assert_eq!(decided.variant_label(), "mark_achieved");
}

#[test]
fn valid_reopen_and_keep_open_round_trip() {
    for (choice, label) in [
        ("reopen", "reopen"),
        ("keep_open_and_report", "keep_open_and_report"),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("outcome.json");
        run(&[
            "ooda",
            "record-outcome",
            "--choice",
            choice,
            "--reason",
            "artifact landed but the live effect is absent this cycle",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "g1",
            "--cycle-number",
            "0",
        ])
        .unwrap_or_else(|e| panic!("a valid {choice} outcome must exit Ok: {e}"));
        let decided = crate::ooda_brain::read_verified_outcome(&record_path, "g1", 0)
            .expect("read_verified_outcome must accept the record");
        assert_eq!(decided.variant_label(), label);
    }
}

#[test]
fn valid_replan_carries_its_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "replan",
        "--reason",
        "third artifact landed but E2BIG persists; wrong layer",
        "--replan-hint",
        "target the engineer spawn argv assembly, not the kgpack encoder",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a valid replan outcome with a hint must exit Ok");

    let decided = crate::ooda_brain::read_verified_outcome(&record_path, "g1", 0)
        .expect("read_verified_outcome must accept the replan record");
    match decided {
        GoalOutcomeDecision::Replan { replan_hint, .. } => assert_eq!(
            replan_hint,
            "target the engineer spawn argv assembly, not the kgpack encoder"
        ),
        other => panic!("expected Replan, got {other:?}"),
    }
}

#[test]
fn choice_is_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "MARK_ACHIEVED",
        "--reason",
        "verified live",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("upper-case choice must be accepted (matched case-insensitively)");
    assert!(matches!(
        read_record(&record_path).decision,
        GoalOutcomeDecision::MarkAchieved { .. }
    ));
}

// ---------------------------------------------------------------------------
// Rejections — every one MUST exit non-zero AND leave NO file on disk.
// ---------------------------------------------------------------------------

#[test]
fn out_of_enum_choice_rejected_and_no_file_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "archive_now", // not one of the four closed variants
        "--reason",
        "smuggled decision",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(result.is_err(), "an out-of-enum choice MUST be rejected");
    assert!(
        !record_path.exists(),
        "a rejected decision MUST NOT write any record (validate-all-then-write-once)"
    );
}

#[test]
fn empty_reason_rejected_and_no_file_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(result.is_err(), "an empty --reason MUST be rejected");
    assert!(!record_path.exists(), "no record on an empty reason");
}

#[test]
fn replan_hint_on_non_replan_choice_rejected() {
    // The load-bearing ownership guard: a hint smuggled onto a non-replan
    // choice must be rejected by the shared chokepoint before any write.
    for choice in ["mark_achieved", "reopen", "keep_open_and_report"] {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("outcome.json");
        let result = run(&[
            "ooda",
            "record-outcome",
            "--choice",
            choice,
            "--reason",
            "a valid reason",
            "--replan-hint",
            "smuggled re-scope guidance",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "g1",
            "--cycle-number",
            "0",
        ]);
        assert!(
            result.is_err(),
            "a --replan-hint on `{choice}` MUST be rejected (replan owns the hint)"
        );
        assert!(
            !record_path.exists(),
            "no record must be written when a hint is smuggled onto `{choice}`"
        );
    }
}

#[test]
fn non_absolute_record_path_rejected_and_no_file_written() {
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "relative path attempt",
        "--record-path",
        "relative/outcome.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL-8)"
    );
    assert!(
        !std::path::Path::new("relative/outcome.json").exists(),
        "no record must be written for a rejected relative path"
    );
}

#[test]
fn record_path_with_parent_traversal_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "traversal attempt",
        "--record-path",
        traversal.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a --record-path containing '..' MUST be rejected (SR-VAL-8)"
    );
}

#[test]
fn missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "missing goal id",
        "--record-path",
        record_path.to_str().unwrap(),
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a missing required option MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    let result = run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "ok",
        "--totally-unknown",
        "x",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(result.is_err(), "an unknown flag MUST be rejected");
    assert!(!record_path.exists());
}

// ---------------------------------------------------------------------------
// Sanitization — model-controlled free text is stripped of ANSI/C0 controls so
// a prompt-injected reasoner cannot spoof operator logs / audit records.
// ---------------------------------------------------------------------------

#[test]
fn ansi_and_control_bytes_stripped_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("outcome.json");
    run(&[
        "ooda",
        "record-outcome",
        "--choice",
        "reopen",
        "--reason",
        "live effect \u{1b}[31mabsent\u{1b}[0m still\u{7}",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a reason with control bytes is sanitized, not rejected");
    let rationale = read_record(&record_path).decision.rationale().to_string();
    assert!(
        !rationale.contains('\u{1b}'),
        "ESC must be stripped from the persisted rationale; got {rationale:?}"
    );
    assert!(
        !rationale
            .chars()
            .any(|c| c.is_control() && !c.is_whitespace()),
        "non-whitespace controls must be stripped; got {rationale:?}"
    );
}
