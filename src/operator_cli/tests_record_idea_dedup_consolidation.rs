//! TDD **failing** tests for the `simard ooda record-idea-dedup` and
//! `simard ooda record-idea-consolidation` CLI writer tools (Group C — issue
//! #2925).
//!
//! These specify the behavior of the two new dispatch arms (reached through the
//! public `dispatch_operator_cli` entry point, exactly like the Group A
//! `record-orient` / `record-decide` and Group B `record-admission` /
//! `record-resource-admission` tests). Neither subcommand exists yet, so every
//! test here fails until the Builder phase adds the match arms.
//!
//! Contract (see `docs/reference/ooda-record-idea-dedup-consolidation-cli.md`):
//!   * Each tool validates its typed fields through the SINGLE shared chokepoint
//!     (`IdeaDedupDecision::from_choice_fields` / `IdeaCluster::sanitized`),
//!     hardens `--record-path` (absolute, no `..`), then writes EXACTLY ONE
//!     atomic `0o600` record. Any validation failure ⇒ non-zero exit AND **no
//!     file on disk** (validate-all-then-write-once).
//!   * `record-idea-dedup --choice` is the closed 3-variant enum
//!     (`create_new|skip|enhance_existing`, case-insensitive). `--target-node-id`
//!     is REQUIRED on `enhance_existing` and REJECTED on `create_new` / `skip`.
//!     Free text `--reason` (or `--reason-path`) is bounded + sanitized via
//!     `sanitize_context_var(_, 500)`.
//!   * `record-idea-consolidation` takes the clusters as a JSON-array FILE via
//!     `--clusters-path` (inline argv would hit E2BIG). Each cluster passes
//!     `IdeaCluster::sanitized`; the list is capped at 64; an empty array `[]`
//!     is a VALID record ("nothing to consolidate").
//!   * Both tools are zero-privilege: their only side effect is that one write.

use crate::operator_cli::dispatch_operator_cli;

fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    dispatch_operator_cli(args.iter().map(|s| s.to_string()))
}

fn read_dedup(path: &std::path::Path) -> crate::ooda_brain::IdeaDedupDecisionRecord {
    let bytes = std::fs::read(path).expect("dedup record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into IdeaDedupDecisionRecord")
}

fn read_consolidation(path: &std::path::Path) -> crate::ooda_brain::IdeaConsolidationRecord {
    let bytes = std::fs::read(path).expect("consolidation record file must exist");
    serde_json::from_slice(&bytes).expect("record must deserialize into IdeaConsolidationRecord")
}

// ===========================================================================
// `simard ooda record-idea-dedup`
// ===========================================================================

#[test]
fn record_idea_dedup_create_new_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "no shortlist entry proposes this; genuinely novel",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("a valid create_new decision must exit Ok");

    let record = read_dedup(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::IDEA_DEDUP_SCHEMA);
    assert_eq!(record.goal_id, "creative-idea-dedup");
    assert_eq!(record.cycle_number, 0);
    assert_eq!(record.decision.variant_label(), "create_new");
}

#[test]
fn record_idea_dedup_skip_writes_one_typed_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "skip",
        "--reason",
        "pure restatement of node-7a3f",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("a valid skip decision must exit Ok");
    assert_eq!(read_dedup(&record_path).decision.variant_label(), "skip");
}

#[test]
fn record_idea_dedup_enhance_carries_target_node_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "enhance_existing",
        "--reason",
        "same caching idea as node-7a3f; strengthen it",
        "--target-node-id",
        "node-7a3f",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("a valid enhance_existing decision must exit Ok");

    match read_dedup(&record_path).decision {
        crate::ooda_brain::IdeaDedupDecision::EnhanceExisting { target_node_id, .. } => {
            assert_eq!(target_node_id, "node-7a3f");
        }
        other => panic!("expected EnhanceExisting, got {other:?}"),
    }
}

#[test]
fn record_idea_dedup_choice_matched_case_insensitively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "CREATE_NEW",
        "--reason",
        "loud caps",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("upper-case choice must be accepted (matched case-insensitively)");
    assert_eq!(
        read_dedup(&record_path).decision.variant_label(),
        "create_new"
    );
}

#[test]
fn record_idea_dedup_enhance_without_target_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "enhance_existing",
        "--reason",
        "wants to enhance but names no node",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "enhance_existing WITHOUT --target-node-id MUST be rejected (unactionable)"
    );
    assert!(!record_path.exists(), "no record on a targetless enhance");
}

#[test]
fn record_idea_dedup_create_new_with_target_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "novel",
        "--target-node-id",
        "node-7a3f",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "create_new MUST reject --target-node-id (owned by enhance_existing)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_idea_dedup_out_of_enum_choice_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "merge",
        "--reason",
        "smuggled variant",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(result.is_err(), "an out-of-enum choice MUST be rejected");
    assert!(!record_path.exists());
}

#[test]
fn record_idea_dedup_empty_reason_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "   ",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a whitespace-only --reason MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_idea_dedup_reason_read_from_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(
        &reason_file,
        "candidate is a novel angle no shortlist entry covers",
    )
    .unwrap();
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("a file-sourced reason must be accepted");
    assert_eq!(
        read_dedup(&record_path).decision.rationale(),
        "candidate is a novel angle no shortlist entry covers"
    );
}

#[test]
fn record_idea_dedup_reason_and_reason_path_mutually_exclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let reason_file = dir.path().join("reason.txt");
    std::fs::write(&reason_file, "from file").unwrap();
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "inline",
        "--reason-path",
        reason_file.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "both --reason and --reason-path MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_idea_dedup_oversized_reason_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let huge = "x".repeat(1000);
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        &huge,
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("an oversized reason is bounded, not rejected");
    let rationale = read_dedup(&record_path).decision.rationale().to_string();
    assert!(
        rationale.chars().count() <= 501,
        "reason must be bounded to 500 chars (+ ellipsis); got {}",
        rationale.chars().count()
    );
}

#[test]
fn record_idea_dedup_ansi_control_stripped_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "\u{1b}[31mALERT\u{1b}[0m novel\u{07} idea",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("a sanitizable reason must be accepted");
    let rationale = read_dedup(&record_path).decision.rationale().to_string();
    assert!(
        !rationale.contains('\u{1b}')
            && !rationale.contains('\u{07}')
            && rationale.contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped before the record is written; got {rationale:?}"
    );
}

#[test]
fn record_idea_dedup_non_absolute_path_rejected() {
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "relative path attempt",
        "--record-path",
        "relative/dedup.json",
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL path hardening)"
    );
    assert!(!std::path::Path::new("relative/dedup.json").exists());
}

#[test]
fn record_idea_dedup_parent_traversal_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let traversal = dir.path().join("..").join("escape.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "traversal attempt",
        "--record-path",
        traversal.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a --record-path containing '..' MUST be rejected (SR-VAL path hardening)"
    );
}

#[test]
fn record_idea_dedup_missing_required_option_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    // --goal-id omitted.
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
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
fn record_idea_dedup_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "has an unknown flag",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
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
fn record_idea_dedup_duplicate_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    let result = run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "create_new",
        "--reason",
        "first",
        "--reason",
        "second",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a duplicated flag MUST be rejected (parse_named_args)"
    );
    assert!(!record_path.exists());
}

// The record the tool writes reads back through the fail-CLOSED reader — the
// writer and reader share the chokepoint, so a tool-written record MUST verify.
#[test]
fn record_idea_dedup_written_record_reads_back_through_the_verified_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("dedup.json");
    run(&[
        "ooda",
        "record-idea-dedup",
        "--choice",
        "enhance_existing",
        "--reason",
        "same idea as node-7a3f",
        "--target-node-id",
        "node-7a3f",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-dedup",
        "--cycle-number",
        "0",
    ])
    .expect("valid enhance must write");
    let decision =
        crate::ooda_brain::read_verified_idea_dedup(&record_path, "creative-idea-dedup", 0)
            .expect("a tool-written record MUST verify (writer & reader share the chokepoint)");
    assert_eq!(decision.variant_label(), "enhance_existing");
}

// ===========================================================================
// `simard ooda record-idea-consolidation`
// ===========================================================================

fn write_clusters(dir: &std::path::Path, json: &str) -> std::path::PathBuf {
    let path = dir.join("clusters.json");
    std::fs::write(&path, json).expect("write clusters input");
    path
}

#[test]
fn record_idea_consolidation_writes_the_validated_cluster_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(
        dir.path(),
        r#"[{"canonical_id":"node-7a3f","redundant_ids":["node-91cc","node-4d20"],"merged_rationale":"caching cluster","evidence":["12% fewer reads"]}]"#,
    );
    run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ])
    .expect("a valid cluster list must exit Ok");

    let record = read_consolidation(&record_path);
    assert_eq!(record.schema, crate::ooda_brain::IDEA_CONSOLIDATION_SCHEMA);
    assert_eq!(record.goal_id, "creative-idea-consolidation");
    assert_eq!(record.clusters.len(), 1);
    assert_eq!(record.clusters[0].canonical_id, "node-7a3f");
    assert_eq!(
        record.clusters[0].redundant_ids,
        vec!["node-91cc", "node-4d20"]
    );
}

#[test]
fn record_idea_consolidation_empty_array_is_a_valid_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(dir.path(), "[]");
    run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ])
    .expect("an empty cluster array is a VALID 'nothing to consolidate' record");
    assert!(
        read_consolidation(&record_path).clusters.is_empty(),
        "the written record must carry an empty cluster list"
    );
}

#[test]
fn record_idea_consolidation_drops_headless_clusters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(
        dir.path(),
        r#"[{"canonical_id":"","redundant_ids":["node-1"],"merged_rationale":"anonymous","evidence":[]},{"canonical_id":"node-7a3f","redundant_ids":["node-2"],"merged_rationale":"real","evidence":[]}]"#,
    );
    run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ])
    .expect("a list with one headless cluster still writes the survivors");
    let record = read_consolidation(&record_path);
    assert_eq!(record.clusters.len(), 1, "the headless cluster is dropped");
    assert_eq!(record.clusters[0].canonical_id, "node-7a3f");
}

#[test]
fn record_idea_consolidation_caps_the_list_at_64() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters: Vec<String> = (0..200)
        .map(|i| format!(r#"{{"canonical_id":"node-{i}","redundant_ids":[],"merged_rationale":"r","evidence":[]}}"#))
        .collect();
    let clusters_path = write_clusters(dir.path(), &format!("[{}]", clusters.join(",")));
    run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ])
    .expect("an over-long list is capped, not rejected");
    assert!(
        read_consolidation(&record_path).clusters.len() <= 64,
        "the cluster list MUST be capped at 64 (prompt-cost DoS guard)"
    );
}

#[test]
fn record_idea_consolidation_malformed_clusters_file_rejected_and_no_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(dir.path(), "{ not a json array ");
    let result = run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a malformed clusters file MUST be rejected"
    );
    assert!(
        !record_path.exists(),
        "no record on a malformed clusters input (validate-all-then-write-once)"
    );
}

#[test]
fn record_idea_consolidation_non_absolute_clusters_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let result = run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        "relative/clusters.json",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --clusters-path MUST be rejected (SR-VAL path hardening)"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_idea_consolidation_non_absolute_record_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clusters_path = write_clusters(dir.path(), "[]");
    let result = run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        "relative/consolidation.json",
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a non-absolute --record-path MUST be rejected (SR-VAL path hardening)"
    );
    assert!(!std::path::Path::new("relative/consolidation.json").exists());
}

#[test]
fn record_idea_consolidation_missing_clusters_path_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let result = run(&[
        "ooda",
        "record-idea-consolidation",
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ]);
    assert!(
        result.is_err(),
        "a missing required --clusters-path MUST be rejected"
    );
    assert!(!record_path.exists());
}

#[test]
fn record_idea_consolidation_unknown_flag_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(dir.path(), "[]");
    let result = run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
        "--choice",
        "create_new",
    ]);
    assert!(
        result.is_err(),
        "a dedup-only flag (--choice) is unknown on the consolidation verb and MUST be rejected"
    );
    assert!(!record_path.exists());
}

// A tool-written consolidation record reads back through the fail-CLOSED reader.
#[test]
fn record_idea_consolidation_written_record_reads_back_through_the_verified_reader() {
    let dir = tempfile::tempdir().expect("tempdir");
    let record_path = dir.path().join("consolidation.json");
    let clusters_path = write_clusters(
        dir.path(),
        r#"[{"canonical_id":"node-7a3f","redundant_ids":["node-2"],"merged_rationale":"r","evidence":[]}]"#,
    );
    run(&[
        "ooda",
        "record-idea-consolidation",
        "--clusters-path",
        clusters_path.to_str().unwrap(),
        "--record-path",
        record_path.to_str().unwrap(),
        "--goal-id",
        "creative-idea-consolidation",
        "--cycle-number",
        "0",
    ])
    .expect("valid clusters must write");
    let clusters = crate::ooda_brain::read_verified_idea_consolidation(
        &record_path,
        "creative-idea-consolidation",
        0,
    )
    .expect("a tool-written record MUST verify (writer & reader share the chokepoint)");
    assert_eq!(clusters.len(), 1);
    assert_eq!(clusters[0].canonical_id, "node-7a3f");
}
