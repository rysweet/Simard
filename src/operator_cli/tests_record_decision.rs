//! TDD failing tests for the `simard ooda record-decision` CLI tool (WS-4).
//!
//! These specify the behavior of `dispatch_record_decision` (reached through
//! the public `dispatch_operator_cli` entry point, exactly like the existing
//! `ooda terminal` tests). The subcommand does NOT exist yet, so every test
//! here fails until the Builder phase adds the `record-decision` match arm.
//!
//! Contract (see `docs/reference/ooda-record-decision-cli.md`):
//!   * Validate the closed `--choice` enum, require a non-empty `--reason`,
//!     harden `--record-path` (absolute, no `..`), then write EXACTLY ONE
//!     atomic `0o600` record. Any validation failure ⇒ non-zero exit AND
//!     **no file on disk** (validate-all-then-write-once).
//!   * Free text (`reason` / `task_hint`) is bounded + sanitized via
//!     `sanitize_context_var(_, 500)` (ANSI/C0 stripped, ≤ 500 chars).
//!   * The tool is zero-privilege: its only side effect is that one write.

use crate::operator_cli::dispatch_operator_cli;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

/// Read the record the tool wrote back into the typed struct (proving the tool
/// and the reader agree on the on-disk shape).
fn read_record(path: &std::path::Path) -> crate::ooda_brain::PerGoalDecisionRecord {
    let bytes = std::fs::read(path).expect("record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into PerGoalDecisionRecord")
}

// ---------------------------------------------------------------------------
// Happy path — a valid decision writes exactly one typed record.
// ---------------------------------------------------------------------------

#[test]
fn valid_spawn_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let record_path_str = record_path.to_str().unwrap();

    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason",
        "no live work; standing research goal must seek the next source",
        "--task-hint",
        "survey arXiv 2026 for new distillation results",
        "--record-path",
        record_path_str,
        "--goal-id",
        "cognition-research",
        "--cycle-number",
        "4287",
    ])
    .expect("a valid spawn decision must exit Ok");

    let record = read_record(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::EXPECTED_SCHEMA);
    assert_eq!(record.goal_id, "cognition-research");
    assert_eq!(record.cycle_number, 4287);
    match record.action {
        crate::ooda_brain::PerGoalAction::Spawn { reason, task_hint } => {
            assert_eq!(
                reason,
                "no live work; standing research goal must seek the next source"
            );
            assert_eq!(task_hint, "survey arXiv 2026 for new distillation results");
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn choice_is_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "WAIT",
        "--reason",
        "PR awaiting CI",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "7",
    ])
    .expect("upper-case choice must be accepted (matched case-insensitively)");
    assert!(matches!(
        read_record(&record_path).action,
        crate::ooda_brain::PerGoalAction::Wait { .. }
    ));
}

// ---------------------------------------------------------------------------
// Rejections — every one MUST exit non-zero AND leave NO file on disk.
// ---------------------------------------------------------------------------

#[test]
fn out_of_enum_choice_rejected_and_no_file_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "deploy", // not one of the six closed variants
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
fn empty_reason_rejected_and_no_file_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason",
        "",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(result.is_err(), "an empty --reason MUST be rejected");
    assert!(!record_path.exists(), "no record on an empty reason");
}

#[test]
fn whitespace_only_reason_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "wait",
        "--reason",
        "    ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --reason MUST be rejected (empty after trim)"
    );
    assert!(
        !record_path.exists(),
        "no record on a whitespace-only reason"
    );
}

#[test]
fn non_absolute_record_path_rejected_and_no_file_written() {
    // A relative record path (SR-VAL-8) must be rejected before any write.
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason",
        "relative path attempt",
        "--record-path",
        "relative/decision.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL-8)"
    );
    assert!(
        !std::path::Path::new("relative/decision.json").exists(),
        "no record must be written for a rejected relative path"
    );
}

#[test]
fn record_path_with_parent_traversal_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Absolute but contains `..` — the traversal guard must reject it.
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
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
fn missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
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
fn unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
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
        "an unknown flag MUST be rejected (reject_extra_args) — never silently ignored"
    );
}

#[test]
fn reason_and_reason_path_are_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(&reason_file, "from file").unwrap();
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason",
        "inline reason",
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
        "supplying both --reason and --reason-path MUST be rejected (mutually exclusive per field)"
    );
    assert!(!record_path.exists());
}

// ---------------------------------------------------------------------------
// Large payload via file (not argv).
// ---------------------------------------------------------------------------

#[test]
fn reason_read_from_file_when_reason_path_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(
        &reason_file,
        "angle exhausted; pivot to a fresh distillation experiment",
    )
    .unwrap();

    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "reorient",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("a file-sourced reason must be accepted");

    let record = read_record(&record_path);
    assert_eq!(
        record.action.reason(),
        "angle exhausted; pivot to a fresh distillation experiment",
        "reason must be read from the --reason-path file"
    );
}

// ---------------------------------------------------------------------------
// Free-text sanitization + bound (SR-VAL-3 / #2751): reason and task_hint go
// through sanitize_context_var(_, 500) before being written.
// ---------------------------------------------------------------------------

#[test]
fn oversized_reason_is_bounded_to_500_chars() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let huge = "x".repeat(1000);
    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
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

    let reason = read_record(&record_path).action.reason().to_string();
    assert!(
        reason.chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {} chars",
        reason.chars().count()
    );
}

#[test]
fn ansi_and_control_bytes_stripped_from_reason_and_hint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason",
        "\u{1b}[31mALERT\u{1b}[0m worker\u{07} went quiet",
        "--task-hint",
        "look\u{00}at\u{1b}[2J logs",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("sanitizable free text must be accepted");

    let action = read_record(&record_path).action;
    let reason = action.reason();
    assert!(
        !reason.contains('\u{1b}') && !reason.contains('\u{07}'),
        "ANSI/C0 control bytes MUST be stripped from reason; got {reason:?}"
    );
    match action {
        crate::ooda_brain::PerGoalAction::Spawn { task_hint, .. } => assert!(
            !task_hint.contains('\u{1b}') && !task_hint.contains('\u{00}'),
            "ANSI/C0 control bytes MUST be stripped from task_hint; got {task_hint:?}"
        ),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// SR-DATA-1 — the record is written owner-only (0o600).
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn record_file_is_owner_only_0o600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "continue",
        "--reason",
        "healthy; leave it",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("valid continue decision must exit Ok");

    let mode = std::fs::metadata(&record_path)
        .expect("record exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o600,
        "the record MUST be written owner-only (0o600); got {mode:o}"
    );
}

// ---------------------------------------------------------------------------
// Zero-privilege: the tool's ONLY side effect is the one record file. Nothing
// else is created in the daemon-supplied directory.
// ---------------------------------------------------------------------------

#[test]
fn tool_writes_only_the_record_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "investigate",
        "--reason",
        "worker quiet; inspect logs before any reclaim",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("valid investigate decision must exit Ok");

    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("read tempdir")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "the tool must write exactly ONE file (the record); found {entries:?}"
    );
    assert_eq!(entries[0], std::ffi::OsStr::new("decision.json"));
}

// ---------------------------------------------------------------------------
// Defense-in-depth hardening (S1/S2): field-input files are bounded (64 KiB,
// fail-closed) and hardened (absolute, no `..`) — the same SR-VAL-8 gate the
// `--record-path` enforces, applied to `--reason-path` / `--task-hint-path`.
// ---------------------------------------------------------------------------

#[test]
fn oversized_reason_path_file_rejected_and_no_record_written() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let reason_file = dir.path().join("reason.txt");
    // One byte over the 64 KiB cap ⇒ fail-closed before any downstream 500-char
    // bound, preventing a transient OOM from reading the whole file into memory.
    std::fs::write(&reason_file, "A".repeat(64 * 1024 + 1)).unwrap();

    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
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
        "a --reason-path file over the 64 KiB cap MUST be rejected"
    );
    assert!(
        !record_path.exists(),
        "no record must be written when the input file is rejected"
    );
}

#[test]
fn reason_path_at_the_cap_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let reason_file = dir.path().join("reason.txt");
    // Exactly at the cap must still be read (downstream sanitize bounds to 500).
    std::fs::write(&reason_file, "A".repeat(64 * 1024)).unwrap();

    run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ])
    .expect("a --reason-path file exactly at the cap must be accepted");
    assert!(record_path.exists(), "the record must be written");
}

#[test]
fn reason_path_with_parent_traversal_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let traversal = dir.path().join("..").join("reason.txt");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason-path",
        traversal.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a --reason-path containing '..' MUST be rejected (SR-VAL-8)"
    );
    assert!(!record_path.exists(), "no record for a rejected input path");
}

#[test]
fn relative_reason_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason-path",
        "relative/reason.txt",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --reason-path MUST be rejected (SR-VAL-8)"
    );
    assert!(!record_path.exists(), "no record for a rejected input path");
}

#[test]
fn task_hint_path_with_parent_traversal_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("decision.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(&reason_file, "valid reason").unwrap();
    let traversal = dir.path().join("..").join("hint.txt");
    let result = run(&[
        "ooda",
        "record-decision",
        "--choice",
        "spawn",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--task-hint-path",
        traversal.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "1",
    ]);
    assert!(
        result.is_err(),
        "a --task-hint-path containing '..' MUST be rejected (SR-VAL-8)"
    );
    assert!(!record_path.exists(), "no record for a rejected input path");
}
