//! TDD **failing** tests for the `simard ooda record-admission` and
//! `simard ooda record-resource-admission` CLI writer tools (Group B — issue
//! #4906).
//!
//! These specify the behavior of the two new dispatch arms (reached through the
//! public `dispatch_operator_cli` entry point, exactly like the Group A
//! `record-orient` / `record-decide` tests from #4785). Neither subcommand
//! exists yet, so every test here fails until the Builder phase adds the match
//! arms.
//!
//! Contract (see `docs/reference/ooda-record-admission-cli.md`):
//!   * Each tool validates its typed fields through the SINGLE shared chokepoint
//!     (`EngineerAdmissionDecision::from_choice_fields` /
//!     `ResourceAdmissionDecision::from_choice_fields`), hardens `--record-path`
//!     (absolute, no `..`), then writes EXACTLY ONE atomic `0o600` record. Any
//!     validation failure ⇒ non-zero exit AND **no file on disk**
//!     (validate-all-then-write-once).
//!   * Free text (`rationale`) is bounded + sanitized via
//!     `sanitize_context_var(_, 500)` (ANSI/C0 stripped, ≤ 500 chars).
//!   * `record-admission --choice` is the closed 3-variant engineer enum
//!     (`admit|defer|serialize_after`, case-insensitive) with per-variant field
//!     ownership: `--blocked-by` / `--retry-after-secs` are owned by `defer`,
//!     `--after-goal-id` / `--overlap-files` by `serialize_after`. List flags are
//!     single-value CSV (`parse_named_args` rejects duplicates).
//!   * `record-resource-admission --choice` is the closed 3-variant resource enum
//!     (`admit|defer|reclaim_first`); all variants carry only `rationale`, so any
//!     admission-owned flag is unknown and rejected.
//!   * Both tools are zero-privilege: their only side effect is that one write.

use crate::operator_cli::dispatch_operator_cli;

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

fn read_engineer(path: &std::path::Path) -> crate::ooda_brain::AdmissionDecisionRecord {
    let bytes = std::fs::read(path).expect("engineer-admission record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into AdmissionDecisionRecord")
}

fn read_resource(path: &std::path::Path) -> crate::ooda_brain::ResourceAdmissionDecisionRecord {
    let bytes = std::fs::read(path).expect("resource-admission record file must exist");
    serde_json::from_slice(&bytes)
        .expect("record must deserialize into ResourceAdmissionDecisionRecord")
}

// ===========================================================================
// `simard ooda record-admission` (engineer-admission)
// ===========================================================================

#[test]
fn record_admission_admit_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "candidate touches src/meeting/*, no live engineer changes those files",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "add-int8-embeddings",
        "--cycle-number",
        "0",
    ])
    .expect("a valid admit decision must exit Ok");

    let record = read_engineer(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::ADMISSION_SCHEMA);
    assert_eq!(record.goal_id, "add-int8-embeddings");
    assert_eq!(record.cycle_number, 0);
    assert_eq!(record.decision.variant_label(), "admit");
    assert!(record.decision.blocking_goals().is_empty());
}

#[test]
fn record_admission_defer_carries_blocked_by_and_retry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "defer",
        "--rationale",
        "live engineer rewriting goals_status.rs",
        "--blocked-by",
        "render-goals-status,rename-adapter",
        "--retry-after-secs",
        "900",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a valid defer decision must exit Ok");

    let record = read_engineer(&record_path);
    match record.decision {
        crate::ooda_brain::EngineerAdmissionDecision::Defer {
            blocked_by,
            retry_after_secs,
            ..
        } => {
            assert_eq!(
                blocked_by,
                vec![
                    "render-goals-status".to_string(),
                    "rename-adapter".to_string()
                ],
                "--blocked-by must be split on commas into the owned Vec (single-value CSV)"
            );
            assert_eq!(retry_after_secs, Some(900));
        }
        other => panic!("expected Defer, got {other:?}"),
    }
}

#[test]
fn record_admission_serialize_after_carries_after_goal_and_overlap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "serialize_after",
        "--rationale",
        "rebase onto the adapter rename before editing shared files",
        "--after-goal-id",
        "rename-adapter-to-clients",
        "--overlap-files",
        "src/ooda_loop/types.rs,src/ooda_actions/mod.rs",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a valid serialize_after decision must exit Ok");

    let record = read_engineer(&record_path);
    match record.decision {
        crate::ooda_brain::EngineerAdmissionDecision::SerializeAfter {
            after_goal_id,
            overlap_files,
            ..
        } => {
            assert_eq!(after_goal_id, "rename-adapter-to-clients");
            assert_eq!(
                overlap_files,
                vec![
                    "src/ooda_loop/types.rs".to_string(),
                    "src/ooda_actions/mod.rs".to_string()
                ]
            );
        }
        other => panic!("expected SerializeAfter, got {other:?}"),
    }
}

#[test]
fn record_admission_choice_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "ADMIT",
        "--rationale",
        "loud caps",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("upper-case choice must be accepted (matched case-insensitively)");
    assert_eq!(
        read_engineer(&record_path).decision.variant_label(),
        "admit"
    );
}

#[test]
fn record_admission_out_of_enum_choice_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "self_destruct",
        "--rationale",
        "smuggled action",
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
fn record_admission_empty_rationale_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --rationale MUST be rejected"
    );
    assert!(!record_path.exists(), "no record on an empty rationale");
}

// ----- Per-variant field ownership (A6): reject a non-owned flag, no file ----

#[test]
fn record_admission_admit_with_blocked_by_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "no overlap",
        "--blocked-by",
        "some-goal",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "admit MUST reject --blocked-by (owned by defer) — a defer field cannot leak onto an admit"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_admission_defer_with_after_goal_id_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "defer",
        "--rationale",
        "collision",
        "--blocked-by",
        "g2",
        "--after-goal-id",
        "g3",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "defer MUST reject --after-goal-id (owned by serialize_after)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_admission_serialize_after_with_blocked_by_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "serialize_after",
        "--rationale",
        "rebase behind",
        "--after-goal-id",
        "g2",
        "--overlap-files",
        "a.rs",
        "--blocked-by",
        "g3",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "serialize_after MUST reject --blocked-by (owned by defer)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_admission_rationale_read_from_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let rationale_file = dir.path().join("rationale.txt");
    std::fs::write(
        &rationale_file,
        "candidate scope is independent of every live engineer",
    )
    .unwrap();
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
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
        read_engineer(&record_path).decision.rationale(),
        "candidate scope is independent of every live engineer"
    );
}

#[test]
fn record_admission_rationale_and_rationale_path_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let rationale_file = dir.path().join("rationale.txt");
    std::fs::write(&rationale_file, "from file").unwrap();
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
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

#[test]
fn record_admission_oversized_rationale_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let huge = "x".repeat(1000);
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        &huge,
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("an oversized rationale is bounded, not rejected");
    let rationale = read_engineer(&record_path).decision.rationale().to_string();
    assert!(
        rationale.chars().count() <= 501,
        "rationale must be bounded to 500 chars (+ ellipsis); got {}",
        rationale.chars().count()
    );
}

#[test]
fn record_admission_ansi_control_stripped_from_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "\u{1b}[31mALERT\u{1b}[0m worker\u{07} independent",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ])
    .expect("a sanitizable rationale must be accepted");
    let rationale = read_engineer(&record_path).decision.rationale().to_string();
    assert!(
        !rationale.contains('\u{1b}')
            && !rationale.contains('\u{07}')
            && rationale.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped before the record is written; got {rationale:?}"
    );
}

#[test]
fn record_admission_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "relative path attempt",
        "--record-path",
        "relative/admission.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL path hardening)"
    );
    assert!(!std::path::Path::new("relative/admission.json").exists());
}

#[test]
fn record_admission_parent_traversal_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
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
        "a --record-path containing '..' MUST be rejected (SR-VAL path hardening)"
    );
}

#[test]
fn record_admission_missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
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
fn record_admission_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "has an unknown flag",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
        "--bogus",
        "value",
    ]);
    assert!(
        result.is_err(),
        "an unknown flag MUST be rejected — never silently ignored"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_admission_duplicate_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("admission.json");
    let result = run(&[
        "ooda",
        "record-admission",
        "--choice",
        "admit",
        "--rationale",
        "first",
        "--rationale",
        "second",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a duplicated flag MUST be rejected (parse_named_args); list flags are single-value CSV"
    );
    assert!(!record_path.exists());
}

// ===========================================================================
// `simard ooda record-resource-admission`
// ===========================================================================

const RESOURCE_VARIANTS: &[&str] = &["admit", "defer", "reclaim_first"];

#[test]
fn record_resource_admission_accepts_all_three_variants() {
    for v in RESOURCE_VARIANTS {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("resource.json");
        run(&[
            "ooda",
            "record-resource-admission",
            "--choice",
            v,
            "--rationale",
            "disk 62% well below the ceiling",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "g1",
            "--cycle-number",
            "0",
        ])
        .unwrap_or_else(|e| panic!("variant `{v}` MUST be accepted, got {e}"));
        let record = read_resource(&record_path);
        assert_eq!(record.schema, crate::ooda_brain::RESOURCE_ADMISSION_SCHEMA);
        assert_eq!(record.decision.variant_label(), *v);
    }
}

#[test]
fn record_resource_admission_out_of_enum_choice_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("resource.json");
    // `serialize_after` is an ENGINEER-gate variant — invalid on the resource gate.
    let result = run(&[
        "ooda",
        "record-resource-admission",
        "--choice",
        "serialize_after",
        "--rationale",
        "wrong gate",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "an engineer-gate variant MUST be rejected on the resource gate"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_resource_admission_rejects_engineer_owned_flags() {
    // All resource variants carry only `rationale`, so admission-owned flags are
    // unknown flags here and rejected against the KNOWN_FLAGS allowlist.
    for bad_flag in [
        "--blocked-by",
        "--after-goal-id",
        "--overlap-files",
        "--retry-after-secs",
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let record_path = dir.path().join("resource.json");
        let result = run(&[
            "ooda",
            "record-resource-admission",
            "--choice",
            "defer",
            "--rationale",
            "resources tight",
            bad_flag,
            "x",
            "--record-path",
            record_path.to_str().unwrap(),
            "--goal-id",
            "g1",
            "--cycle-number",
            "0",
        ]);
        assert!(
            result.is_err(),
            "the resource gate MUST reject the engineer-owned flag {bad_flag}"
        );
        assert!(!record_path.exists());
    }
}

#[test]
fn record_resource_admission_empty_rationale_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("resource.json");
    let result = run(&[
        "ooda",
        "record-resource-admission",
        "--choice",
        "defer",
        "--rationale",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --rationale MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_resource_admission_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-resource-admission",
        "--choice",
        "admit",
        "--rationale",
        "relative path attempt",
        "--record-path",
        "relative/resource.json",
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected"
    );
    assert!(!std::path::Path::new("relative/resource.json").exists());
}

#[test]
fn record_resource_admission_rationale_read_from_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("resource.json");
    let rationale_file = dir.path().join("rationale.txt");
    std::fs::write(
        &rationale_file,
        "disk 88% with 41 stale worktrees; reclaim first",
    )
    .unwrap();
    run(&[
        "ooda",
        "record-resource-admission",
        "--choice",
        "reclaim_first",
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
        read_resource(&record_path).decision.rationale(),
        "disk 88% with 41 stale worktrees; reclaim first"
    );
}

#[test]
fn record_resource_admission_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("resource.json");
    let result = run(&[
        "ooda",
        "record-resource-admission",
        "--choice",
        "admit",
        "--rationale",
        "has an unknown flag",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "g1",
        "--cycle-number",
        "0",
        "--bogus",
        "value",
    ]);
    assert!(
        result.is_err(),
        "an unknown flag MUST be rejected — never silently ignored"
    );
    assert!(!record_path.exists());
}
