//! Contract tests for the `simard ooda record-lifecycle-decision` CLI writer
//! tool (Group E — issue #4967, epic #4719).
//!
//! These specify the behavior of `dispatch_record_lifecycle_decision` (reached
//! through the public `dispatch_operator_cli` entry point, exactly like the
//! sibling `record-admission` / `record-decision` / `record-outcome` tests).
//!
//! Contract (mirrors the sibling record verbs; see
//! `src/ooda_brain/engineer_lifecycle_record.rs`):
//!   * Validate the closed 6-variant `--decision` enum through the SINGLE shared
//!     `sanitize_lifecycle_fields` chokepoint (the same one the reader applies,
//!     so writer and reader can never drift), harden `--record-path` (absolute,
//!     no `..`), stamp `written_at_epoch`, then write EXACTLY ONE atomic `0o600`
//!     `EngineerLifecycleRecord`. Any validation failure ⇒ non-zero exit AND
//!     **no file on disk** (validate-all-then-write-once).
//!   * `--rationale` is OPTIONAL (an empty rationale is a VALID record — this is
//!     the one contract that differs from the admission/decision siblings, whose
//!     rationale is required).
//!   * Free text is sanitized (ANSI/C0 stripped) and REJECTED — never silently
//!     truncated — if it still exceeds the 500-char cap after sanitize.
//!   * The closed flag set is exactly `decision`, `rationale`, `rationale-path`,
//!     `record-path`, `goal-id`, `cycle-number` (no per-variant flags): any
//!     other flag is unknown and rejected.
//!   * The tool is zero-privilege: its only side effect is that one write.

use crate::operator_cli::dispatch_operator_cli;

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

fn read_record(path: &std::path::Path) -> crate::ooda_brain::EngineerLifecycleRecord {
    let bytes = std::fs::read(path).expect("lifecycle record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into EngineerLifecycleRecord")
}

const VARIANTS: &[&str] = &[
    "continue_skipping",
    "reclaim_and_redispatch",
    "deprioritize",
    "open_tracking_issue",
    "mark_goal_blocked",
    "consider_self_update",
];

// ===========================================================================
// Positive: every closed variant writes exactly one typed, owner-only record
// ===========================================================================

#[test]
fn record_lifecycle_decision_writes_one_typed_record_per_variant() {
    for variant in VARIANTS {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("lifecycle.json");
        run(&[
            "ooda",
            "record-lifecycle-decision",
            "--decision",
            variant,
            "--rationale",
            "engineer idle 7h; log truncated mid-tool-call",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "add-int8-embeddings",
            "--cycle-number",
            "0",
        ])
        .unwrap_or_else(|e| panic!("a valid `{variant}` decision must exit Ok: {e}"));

        let record = read_record(&record_path);
        assert_eq!(record.schema, crate::ooda_brain::ENGINEER_LIFECYCLE_SCHEMA);
        assert_eq!(record.goal_id, "add-int8-embeddings");
        assert_eq!(record.cycle_number, 0);
        assert_eq!(
            record.decision, *variant,
            "the canonical variant token must round-trip verbatim"
        );
        assert!(record.written_at_epoch > 0, "epoch must be stamped");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&record_path)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "record MUST be written owner-only (0o600); got {:#o}",
                mode & 0o777
            );
        }
    }
}

// ===========================================================================
// LIVE CANARY: the CLI writer and the fail-closed reader agree end-to-end.
// A record written by the tool must be accepted by
// `read_verified_engineer_lifecycle_decision` for the same goal + cycle.
// ===========================================================================

#[test]
fn record_lifecycle_decision_round_trips_through_the_fail_closed_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "reclaim_and_redispatch",
        "--rationale",
        "worktree idle 7h, sentinel wedged",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g-live",
        "--cycle-number",
        "0",
    ])
    .expect("valid decision must be written");

    let decision =
        crate::ooda_brain::read_verified_engineer_lifecycle_decision(&record_path, "g-live", 0)
            .expect("the tool's record MUST pass the fail-closed reader (writer/reader agree)");
    assert_eq!(
        crate::ooda_brain::lifecycle_decision_choice(&decision),
        "reclaim_and_redispatch"
    );

    // R6 (goal identity): the SAME record must be rejected for a different goal.
    assert!(
        crate::ooda_brain::read_verified_engineer_lifecycle_decision(
            &record_path,
            "some-other-goal",
            0,
        )
        .is_err(),
        "a record written for g-live MUST NOT be honored for another goal"
    );
}

// ===========================================================================
// Lifecycle-specific: an OMITTED / empty rationale is a VALID record
// (the extra-field variants derive their text from it, empty is allowed).
// ===========================================================================

#[test]
fn record_lifecycle_decision_omitted_rationale_is_valid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("an omitted --rationale is VALID for the lifecycle verb");
    let record = read_record(&record_path);
    assert_eq!(record.decision, "continue_skipping");
    assert_eq!(record.rationale, "", "an omitted rationale stores empty");
}

#[test]
fn record_lifecycle_decision_whitespace_only_rationale_is_valid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--rationale",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a whitespace-only rationale sanitizes to empty, which is valid");
    assert_eq!(read_record(&record_path).rationale, "");
}

// ===========================================================================
// Case-insensitive decision matching (the shared chokepoint trims + case-folds)
// ===========================================================================

#[test]
fn record_lifecycle_decision_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "  Reclaim_And_Redispatch  ",
        "--rationale",
        "mixed case with surrounding space",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a case/space-variant of a known variant must be accepted");
    assert_eq!(
        read_record(&record_path).decision,
        "reclaim_and_redispatch",
        "the persisted decision must be the canonical snake_case token"
    );
}

// ===========================================================================
// CANARY: an out-of-set decision is rejected and writes NO file
// ===========================================================================

#[test]
fn record_lifecycle_decision_out_of_set_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "delete_all_the_things",
        "--rationale",
        "smuggled action",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "an out-of-set --decision MUST be rejected (a compromised prompt cannot smuggle a novel action)"
    );
    assert!(
        !record_path.exists(),
        "a rejected decision MUST NOT write any record (validate-all-then-write-once)"
    );
}

// ===========================================================================
// CANARY: an oversized rationale is REJECTED (never silently truncated)
// — this is the lifecycle contract; the admission sibling instead bounds.
// ===========================================================================

#[test]
fn record_lifecycle_decision_oversized_rationale_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    let huge = "x".repeat(1000);
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--rationale",
        &huge,
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "an oversized rationale MUST be rejected (fail-closed), never silently truncated"
    );
    assert!(!record_path.exists(), "no record on an oversized rationale");
}

// ===========================================================================
// ANSI/C0 control bytes are stripped from the rationale before the write
// ===========================================================================

#[test]
fn record_lifecycle_decision_ansi_control_stripped_from_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "deprioritize",
        "--rationale",
        "\u{1b}[31mALERT\u{1b}[0m engineer\u{07} churning",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a sanitizable rationale must be accepted");
    let rationale = read_record(&record_path).rationale;
    assert!(
        !rationale.contains('\u{1b}')
            && !rationale.contains('\u{07}')
            && rationale.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped before the record is written; got {rationale:?}"
    );
}

// ===========================================================================
// Rationale sourced from a file; mutually exclusive with the inline flag
// ===========================================================================

#[test]
fn record_lifecycle_decision_rationale_read_from_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    let rationale_file = dir.path().join("rationale.txt");
    std::fs::write(&rationale_file, "engineer OOM on cycle 12, recurring").unwrap();
    run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "open_tracking_issue",
        "--rationale-path",
        rationale_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a file-sourced rationale must be accepted");
    assert_eq!(
        read_record(&record_path).rationale,
        "engineer OOM on cycle 12, recurring"
    );
}

#[test]
fn record_lifecycle_decision_rationale_and_rationale_path_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    let rationale_file = dir.path().join("rationale.txt");
    std::fs::write(&rationale_file, "from file").unwrap();
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--rationale",
        "inline",
        "--rationale-path",
        rationale_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "both --rationale and --rationale-path MUST be rejected"
    );
    assert!(!record_path.exists());
}

// ===========================================================================
// Path hardening (SR-VAL): non-absolute and parent-traversal paths rejected
// ===========================================================================

#[test]
fn record_lifecycle_decision_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--record-path",
        "relative/lifecycle.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL path hardening)"
    );
    assert!(!std::path::Path::new("relative/lifecycle.json").exists());
}

#[test]
fn record_lifecycle_decision_parent_traversal_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--record-path",
        traversal.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a --record-path containing '..' MUST be rejected (SR-VAL path hardening)"
    );
}

// ===========================================================================
// Argument-surface guards: missing required, unknown flag, non-u32 cycle
// ===========================================================================

#[test]
fn record_lifecycle_decision_missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
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
fn record_lifecycle_decision_unknown_flag_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    // A per-variant flag from the siblings is NOT part of this verb's closed set.
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
        "--blocked-by",
        "some-goal",
    ]);
    assert!(
        result.is_err(),
        "an unknown flag MUST be rejected — never silently ignored"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_lifecycle_decision_non_u32_cycle_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("lifecycle.json");
    let result = run(&[
        "ooda",
        "record-lifecycle-decision",
        "--decision",
        "continue_skipping",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "not-a-number",
    ]);
    assert!(result.is_err(), "a non-u32 --cycle-number MUST be rejected");
    assert!(!record_path.exists());
}
