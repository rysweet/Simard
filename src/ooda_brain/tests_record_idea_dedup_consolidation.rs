//! TDD **failing** tests for the typed creative-ideas semantic-dedup +
//! consolidation decision RECORDS, their shared closed-enum / cluster
//! chokepoints, and their fail-CLOSED readers (Group C of epic #4719; issue
//! #2925).
//!
//! These specify the seam that REPLACES the forbidden "recipe prints JSON →
//! Rust scrapes prose (`extract_and_parse_json`) → Rust acts" pattern on the two
//! creative-ideas phases, exactly as Group A (`tests_record_orient_decide.rs`,
//! #4785) did for orient/decide and Group B (`tests_record_admission.rs`, #4906)
//! did for the two admission phases. They reference symbols that DO NOT EXIST
//! YET, so the crate will not compile until the Builder phase adds them:
//!
//!   * `super::IDEA_DEDUP_SCHEMA` / `super::IDEA_CONSOLIDATION_SCHEMA` — the
//!     pinned schema strings (`"simard.creative.idea_dedup.v1"` /
//!     `"simard.creative.idea_consolidation.v1"`).
//!   * `IdeaDedupDecision::from_choice_fields(choice, reason, target_node_id)
//!     -> Option<IdeaDedupDecision>` — the SINGLE shared closed-enum chokepoint,
//!     reused by the `record-idea-dedup` CLI writer AND `read_verified_idea_dedup`,
//!     enforcing per-variant field-ownership (`enhance_existing` REQUIRES a
//!     non-empty `target_node_id`; `create_new`/`skip` REJECT it) and a non-empty
//!     sanitized rationale.
//!   * `IdeaCluster::sanitized(&self) -> Option<IdeaCluster>` — the SINGLE shared
//!     per-cluster sanitizing chokepoint, reused by the `record-idea-consolidation`
//!     CLI writer AND `read_verified_idea_consolidation`: drops an empty
//!     `canonical_id` (⇒ `None`), bounds `merged_rationale`, and
//!     sanitizes+caps `redundant_ids` / `evidence`.
//!   * `super::IdeaDedupDecisionRecord` / `super::IdeaConsolidationRecord` — the
//!     typed on-disk records (`{schema, goal_id, cycle_number, …}`; dedup flattens
//!     the tagged decision, consolidation carries a validated cluster `Vec`).
//!   * `super::read_verified_idea_dedup` / `super::read_verified_idea_consolidation`
//!     — the readers that enforce the fail-CLOSED matrix (R1–R7).
//!
//! The whole point of this seam is that EVERY failure mode is an `Err`. HOW that
//! `Err` is surfaced is UNCHANGED by this rework: the dedup gate maps `Err` to
//! `PlannedAction::FailClosed` (drop the candidate this cycle — fail CLOSED), and
//! the consolidation applier maps `Err` to "write nothing, retry later". The
//! `Some(vec![])` vs `None` distinction of the old parser is preserved EXACTLY:
//! a present-but-empty consolidation record reads back `Ok(vec![])` (a valid
//! "nothing to consolidate"), while an absent/malformed/mismatched record is `Err`.
//!
//! Reference contract: `docs/reference/ooda-record-idea-dedup-consolidation-cli.md`.

use std::path::Path;

use super::{
    IDEA_CONSOLIDATION_SCHEMA, IDEA_DEDUP_SCHEMA, IdeaCluster, IdeaConsolidationRecord,
    IdeaDedupDecision, IdeaDedupDecisionRecord, read_verified_idea_consolidation,
    read_verified_idea_dedup,
};

// ---------------------------------------------------------------------------
// Shared hermetic helpers — a temp dir + direct record writer (no CLI, no
// subprocess). The reader tests DELIBERATELY write the on-disk bytes themselves
// (via serde for valid cases, raw strings for hostile cases) so they exercise
// the readers in ISOLATION from the writer tool: the defense-in-depth guarantee
// is that even a record the tool would never produce must fail CLOSED.
// ---------------------------------------------------------------------------

/// The fixed synthetic per-seam `goal_id` sentinels (neither ctx is naturally
/// goal-scoped, so R6 enforces write/read self-consistency against these).
const DEDUP_GOAL: &str = "creative-idea-dedup";
const CONSOLIDATION_GOAL: &str = "creative-idea-consolidation";
/// The `REASONER_RECORD_CYCLE = 0` sentinel bound by both writer and reader.
const CYCLE: u32 = 0;

fn write_bytes(dir: &Path, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join("record.json");
    std::fs::write(&path, bytes).expect("write record bytes");
    path
}

fn write_json<T: serde::Serialize>(dir: &Path, record: &T) -> std::path::PathBuf {
    let bytes = serde_json::to_vec(record).expect("serialize record");
    write_bytes(dir, &bytes)
}

// Convenience constructors for the dedup chokepoint. `create_new`/`skip` carry
// NO target; `enhance_existing` REQUIRES one.
fn create_new(reason: &str) -> Option<IdeaDedupDecision> {
    IdeaDedupDecision::from_choice_fields("create_new", reason, "")
}
fn skip(reason: &str) -> Option<IdeaDedupDecision> {
    IdeaDedupDecision::from_choice_fields("skip", reason, "")
}
fn enhance(reason: &str, target: &str) -> Option<IdeaDedupDecision> {
    IdeaDedupDecision::from_choice_fields("enhance_existing", reason, target)
}

fn dedup_record(decision: IdeaDedupDecision) -> IdeaDedupDecisionRecord {
    IdeaDedupDecisionRecord {
        schema: IDEA_DEDUP_SCHEMA.to_string(),
        goal_id: DEDUP_GOAL.to_string(),
        cycle_number: CYCLE,
        decision,
    }
}

fn cluster(canonical: &str, redundant: &[&str], rationale: &str, evidence: &[&str]) -> IdeaCluster {
    IdeaCluster {
        canonical_id: canonical.to_string(),
        redundant_ids: redundant.iter().map(|s| s.to_string()).collect(),
        merged_rationale: rationale.to_string(),
        evidence: evidence.iter().map(|s| s.to_string()).collect(),
    }
}

fn consolidation_record(clusters: Vec<IdeaCluster>) -> IdeaConsolidationRecord {
    IdeaConsolidationRecord {
        schema: IDEA_CONSOLIDATION_SCHEMA.to_string(),
        goal_id: CONSOLIDATION_GOAL.to_string(),
        cycle_number: CYCLE,
        clusters,
    }
}

// ===========================================================================
// DEDUP side — schema pin
// ===========================================================================

#[test]
fn dedup_schema_is_the_pinned_v1_string() {
    assert_eq!(
        IDEA_DEDUP_SCHEMA, "simard.creative.idea_dedup.v1",
        "the dedup reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- Chokepoint: exactly the 3 variants, case-insensitive, sanitized -------

/// The canonical three dedup variants (closed set). The chokepoint must accept
/// EXACTLY these three — no more, no fewer.
const DEDUP_VARIANTS: &[&str] = &["create_new", "skip", "enhance_existing"];

#[test]
fn dedup_chokepoint_accepts_create_new() {
    let d = create_new("genuinely novel: no shortlist entry proposes this").expect("create_new ok");
    assert_eq!(d.variant_label(), "create_new");
    assert_eq!(
        d.rationale(),
        "genuinely novel: no shortlist entry proposes this"
    );
    match d {
        IdeaDedupDecision::CreateNew { .. } => {}
        other => panic!("expected CreateNew, got {other:?}"),
    }
}

#[test]
fn dedup_chokepoint_accepts_skip() {
    let d = skip("pure restatement of node-7a3f, adds nothing").expect("skip ok");
    assert_eq!(d.variant_label(), "skip");
    match d {
        IdeaDedupDecision::Skip { .. } => {}
        other => panic!("expected Skip, got {other:?}"),
    }
}

#[test]
fn dedup_chokepoint_accepts_enhance_existing_with_target() {
    let d = enhance(
        "same caching idea as node-7a3f; adds a measured number",
        "node-7a3f",
    )
    .expect("enhance_existing with a target must be accepted");
    match d {
        IdeaDedupDecision::EnhanceExisting {
            target_node_id,
            rationale,
        } => {
            assert_eq!(target_node_id, "node-7a3f");
            assert!(rationale.contains("caching"));
        }
        other => panic!("expected EnhanceExisting, got {other:?}"),
    }
}

#[test]
fn dedup_chokepoint_is_case_insensitive() {
    let d = IdeaDedupDecision::from_choice_fields("ENHANCE_EXISTING", "loud caps", "node-1")
        .expect("choice must be matched case-insensitively");
    assert_eq!(d.variant_label(), "enhance_existing");
}

#[test]
fn dedup_chokepoint_rejects_unknown_choice() {
    assert!(
        IdeaDedupDecision::from_choice_fields("merge", "smuggled variant", "node-1").is_none(),
        "an unknown choice tag MUST be rejected — the closed enum is the sole authority"
    );
    assert!(
        IdeaDedupDecision::from_choice_fields("", "empty choice", "").is_none(),
        "an empty choice tag MUST be rejected"
    );
}

#[test]
fn dedup_chokepoint_rejects_empty_reason() {
    assert!(
        create_new("").is_none(),
        "an empty rationale MUST be rejected (fail CLOSED at the chokepoint)"
    );
    assert!(
        create_new("   ").is_none(),
        "a whitespace-only rationale MUST be rejected (empty after trim)"
    );
}

#[test]
fn dedup_chokepoint_rejects_control_only_reason() {
    // A rationale made up ENTIRELY of ANSI/C0 control bytes sanitizes to empty ⇒
    // rejected. A bare `trim()` would wrongly accept it (ESC is not whitespace).
    assert!(
        create_new("\u{1b}\u{07}\u{01}").is_none(),
        "a control-byte-only rationale MUST be rejected (empty after sanitize)"
    );
}

#[test]
fn dedup_chokepoint_strips_ansi_and_bounds_reason() {
    let d = create_new("\u{1b}[31mALERT\u{1b}[0m novel idea")
        .expect("a sanitizable rationale is accepted");
    assert!(
        !d.rationale().contains('\u{1b}') && d.rationale().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped while preserving the text; got {:?}",
        d.rationale()
    );

    let huge = "x".repeat(1000);
    let bounded = create_new(&huge).expect("an oversized rationale is bounded, not rejected");
    assert!(
        bounded.rationale().chars().count() <= 501,
        "rationale must be bounded to 500 chars (+ ellipsis); got {}",
        bounded.rationale().chars().count()
    );
}

// ----- Field-ownership matrix: target_node_id belongs ONLY to enhance ---------

#[test]
fn dedup_enhance_existing_requires_a_non_empty_target() {
    assert!(
        enhance("wants to enhance but names no node", "").is_none(),
        "enhance_existing WITHOUT a target_node_id is unactionable ⇒ MUST be rejected (fail CLOSED)"
    );
    assert!(
        enhance("whitespace target", "   ").is_none(),
        "a whitespace-only target_node_id MUST be rejected (empty after trim)"
    );
}

#[test]
fn dedup_create_new_rejects_a_smuggled_target() {
    assert!(
        IdeaDedupDecision::from_choice_fields("create_new", "novel", "node-7a3f").is_none(),
        "create_new + target_node_id MUST be rejected (target is owned by enhance_existing)"
    );
}

#[test]
fn dedup_skip_rejects_a_smuggled_target() {
    assert!(
        IdeaDedupDecision::from_choice_fields("skip", "dup", "node-7a3f").is_none(),
        "skip + target_node_id MUST be rejected (target is owned by enhance_existing)"
    );
}

// ----- Round-trip every variant through the record (serde flatten) -----------

#[test]
fn read_verified_idea_dedup_round_trips_create_new() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = create_new("no shortlist entry matches").unwrap();
    let path = write_json(dir.path(), &dedup_record(d.clone()));
    let read =
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).expect("valid create_new reads Ok");
    assert_eq!(read, d, "reader must return the exact recorded decision");
}

#[test]
fn read_verified_idea_dedup_round_trips_skip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = skip("exact restatement of node-42").unwrap();
    let path = write_json(dir.path(), &dedup_record(d.clone()));
    let read = read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).expect("valid skip reads Ok");
    assert_eq!(read, d);
}

#[test]
fn read_verified_idea_dedup_round_trips_enhance_existing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let d = enhance("same idea as node-7a3f, strengthen it", "node-7a3f").unwrap();
    let path = write_json(dir.path(), &dedup_record(d.clone()));
    let read = read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE)
        .expect("valid enhance_existing reads Ok");
    assert_eq!(read, d);
}

#[test]
fn dedup_record_carries_the_flattened_choice_tag_on_the_wire() {
    // Defense against a serde-shape regression: the decision must flatten so the
    // CLI tool and the enum can never disagree on the wire.
    for v in DEDUP_VARIANTS {
        let decision = match *v {
            "create_new" => create_new("r").unwrap(),
            "skip" => skip("r").unwrap(),
            _ => enhance("r", "node-1").unwrap(),
        };
        let json = serde_json::to_value(dedup_record(decision)).expect("serialize");
        assert_eq!(
            json.get("choice").and_then(|c| c.as_str()),
            Some(*v),
            "the record must carry a flat `choice` discriminator == the snake_case variant"
        );
        assert_eq!(
            json.get("schema").and_then(|s| s.as_str()),
            Some(IDEA_DEDUP_SCHEMA),
            "the record must carry the pinned schema string"
        );
    }
}

// ----- R1: file absent ------------------------------------------------------

#[test]
fn dedup_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R1: an absent dedup record MUST be an Err (the gate then fails CLOSED — drops the candidate)"
    );
}

// ----- R2: malformed / truncated JSON ---------------------------------------

#[test]
fn dedup_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ not valid json ");
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST be an Err"
    );
}

#[test]
fn dedup_r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) file MUST be an Err"
    );
}

// ----- R3: schema version pin -----------------------------------------------

#[test]
fn dedup_r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.creative.idea_dedup.v2","goal_id":"{DEDUP_GOAL}","cycle_number":{CYCLE},"choice":"create_new","rationale":"future record"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST be an Err — a v2 writer is never honored by a v1 reader"
    );
}

#[test]
fn dedup_r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{DEDUP_GOAL}","cycle_number":{CYCLE},"choice":"create_new","rationale":"no schema"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R3: a record with no schema field MUST be an Err"
    );
}

// ----- R4: choice not one of the closed variants ----------------------------

#[test]
fn dedup_r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{IDEA_DEDUP_SCHEMA}","goal_id":"{DEDUP_GOAL}","cycle_number":{CYCLE},"choice":"merge","rationale":"smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag MUST be an Err — a compromised prompt cannot smuggle a novel variant"
    );
}

#[test]
fn dedup_r4_enhance_without_target_fails_closed() {
    // A serde-parseable record whose `enhance_existing` omits `target_node_id`
    // must still fail on re-validation through the chokepoint.
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{IDEA_DEDUP_SCHEMA}","goal_id":"{DEDUP_GOAL}","cycle_number":{CYCLE},"choice":"enhance_existing","target_node_id":"","rationale":"wants enhance, names nobody"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R4: enhance_existing with an empty target_node_id MUST be an Err (unactionable ⇒ fail CLOSED)"
    );
}

// ----- R5: rationale missing / empty / control-only -------------------------

#[test]
fn dedup_r5_empty_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{IDEA_DEDUP_SCHEMA}","goal_id":"{DEDUP_GOAL}","cycle_number":{CYCLE},"choice":"create_new","rationale":""}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R5: an empty rationale MUST be an Err"
    );
}

#[test]
fn dedup_r5_control_byte_only_reason_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{IDEA_DEDUP_SCHEMA}\",\"goal_id\":\"{DEDUP_GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"create_new\",\"rationale\":\"\\u001b\\u0007\\u0001\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R5: a control-byte-only rationale MUST be an Err (empty after sanitize)"
    );
}

#[test]
fn dedup_reader_sanitizes_ansi_control_from_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{IDEA_DEDUP_SCHEMA}\",\"goal_id\":\"{DEDUP_GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"create_new\",\"rationale\":\"\\u001b[31mALERT\\u001b[0m novel\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let d = read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE)
        .expect("sanitizable rationale must verify");
    assert!(
        !d.rationale().contains('\u{1b}') && d.rationale().contains("ALERT"),
        "ANSI/C0 bytes MUST be stripped on read while preserving the text; got {:?}",
        d.rationale()
    );
}

// ----- R6/R7: identity binding (no stale / prior-cycle / other-seam replay) --

#[test]
fn dedup_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = dedup_record(create_new("novel").unwrap());
    record.goal_id = "creative-idea-consolidation".to_string(); // the OTHER seam's sentinel
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the seam sentinel MUST be an Err"
    );
}

#[test]
fn dedup_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = dedup_record(create_new("novel").unwrap());
    record.cycle_number = CYCLE + 1; // a stale prior-cycle record lingering on disk
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_idea_dedup(&path, DEDUP_GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST be an Err (no replay of a prior verdict)"
    );
}

// ===========================================================================
// CONSOLIDATION side — schema pin
// ===========================================================================

#[test]
fn consolidation_schema_is_the_pinned_v1_string() {
    assert_eq!(
        IDEA_CONSOLIDATION_SCHEMA, "simard.creative.idea_consolidation.v1",
        "the consolidation reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ----- IdeaCluster::sanitized chokepoint ------------------------------------

#[test]
fn cluster_sanitized_keeps_a_well_formed_cluster() {
    let c = cluster(
        "node-7a3f",
        &["node-91cc", "node-4d20"],
        "three entries all propose caching goal_board.json across cycles",
        &["node-4d20 measured 12% fewer reads"],
    );
    let out = c.sanitized().expect("a well-formed cluster survives");
    assert_eq!(out.canonical_id, "node-7a3f");
    assert_eq!(out.redundant_ids, vec!["node-91cc", "node-4d20"]);
    assert!(out.merged_rationale.contains("caching"));
    assert_eq!(out.evidence.len(), 1);
}

#[test]
fn cluster_sanitized_drops_empty_canonical_id() {
    assert!(
        cluster("", &["node-1"], "anonymous", &[])
            .sanitized()
            .is_none(),
        "a cluster with an empty canonical_id MUST be dropped (returns None) — nothing to keep"
    );
    assert!(
        cluster("   ", &["node-1"], "whitespace", &[])
            .sanitized()
            .is_none(),
        "a whitespace-only canonical_id sanitizes to empty ⇒ dropped"
    );
}

#[test]
fn cluster_sanitized_strips_ansi_and_bounds_merged_rationale() {
    let huge = "z".repeat(1000);
    let c = cluster("node-1", &[], &format!("\u{1b}[31m{huge}"), &[]);
    let out = c.sanitized().expect("a sanitizable cluster survives");
    assert!(
        !out.merged_rationale.contains('\u{1b}'),
        "ANSI/C0 bytes MUST be stripped from merged_rationale"
    );
    assert!(
        out.merged_rationale.chars().count() <= 501,
        "merged_rationale must be bounded to 500 chars (+ ellipsis); got {}",
        out.merged_rationale.chars().count()
    );
}

#[test]
fn cluster_sanitized_drops_empty_redundant_and_evidence_entries() {
    let c = cluster(
        "node-1",
        &["node-2", "", "   "],
        "r",
        &["ev-1", "", "\u{1b}\u{07}"],
    );
    let out = c.sanitized().expect("survives");
    assert_eq!(
        out.redundant_ids,
        vec!["node-2"],
        "empty / whitespace redundant_ids MUST be dropped"
    );
    assert_eq!(
        out.evidence,
        vec!["ev-1"],
        "empty / control-only evidence entries MUST be dropped"
    );
}

// ----- Round-trip a populated + an empty cluster list ------------------------

#[test]
fn read_verified_idea_consolidation_round_trips_a_populated_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clusters = vec![
        cluster(
            "node-7a3f",
            &["node-91cc"],
            "caching cluster",
            &["12% fewer reads"],
        ),
        cluster("node-33aa", &["node-9001"], "retry-jitter cluster", &[]),
    ];
    let path = write_json(dir.path(), &consolidation_record(clusters.clone()));
    let read = read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE)
        .expect("a valid populated consolidation record reads Ok");
    assert_eq!(
        read, clusters,
        "reader must return the exact sanitized cluster list"
    );
}

#[test]
fn read_verified_idea_consolidation_present_empty_list_is_ok_not_err() {
    // The CRITICAL Some(vec![]) vs None preservation: a present-but-empty list is
    // a VALID "nothing to consolidate" result, NOT an error.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_json(dir.path(), &consolidation_record(vec![]));
    let read = read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE)
        .expect("a present-but-empty cluster list MUST read back Ok(vec![]), never Err");
    assert!(
        read.is_empty(),
        "an empty-but-present record is Ok(vec![]) — 'nothing to consolidate', distinct from absent"
    );
}

#[test]
fn read_verified_idea_consolidation_drops_headless_clusters_on_read() {
    // A cluster whose canonical_id is empty after sanitizing is silently dropped
    // by IdeaCluster::sanitized on read — NOT an error for the whole list.
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{IDEA_CONSOLIDATION_SCHEMA}","goal_id":"{CONSOLIDATION_GOAL}","cycle_number":{CYCLE},"clusters":[{{"canonical_id":"","redundant_ids":["node-1"],"merged_rationale":"anonymous","evidence":[]}},{{"canonical_id":"node-7a3f","redundant_ids":["node-2"],"merged_rationale":"real","evidence":[]}}]}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let read = read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE)
        .expect("surviving clusters still read Ok");
    assert_eq!(
        read.len(),
        1,
        "the headless cluster is dropped, the real one survives"
    );
    assert_eq!(read[0].canonical_id, "node-7a3f");
}

#[test]
fn read_verified_idea_consolidation_caps_the_list_at_64() {
    // The cluster list is capped at 64 (mirrors the 64-entry prompt-cost DoS
    // guard). An over-long list is re-capped on read, never trusted whole.
    let dir = tempfile::tempdir().expect("tempdir");
    let clusters: Vec<IdeaCluster> = (0..200)
        .map(|i| cluster(&format!("node-{i}"), &[], "r", &[]))
        .collect();
    let path = write_json(dir.path(), &consolidation_record(clusters));
    let read = read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE)
        .expect("an over-long list is capped, not rejected");
    assert!(
        read.len() <= 64,
        "the cluster list MUST be capped at 64 on read; got {}",
        read.len()
    );
}

// ----- Consolidation fail matrix (R1/R2/R3/R6/R7) ----------------------------

#[test]
fn consolidation_r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("record.json"); // never created
    assert!(
        read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE).is_err(),
        "R1: an ABSENT consolidation record MUST be an Err — distinct from a present-but-empty Ok(vec![])"
    );
}

#[test]
fn consolidation_r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ clusters: oops ");
    assert!(
        read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST be an Err"
    );
}

#[test]
fn consolidation_r3_wrong_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.creative.idea_consolidation.v2","goal_id":"{CONSOLIDATION_GOAL}","cycle_number":{CYCLE},"clusters":[]}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST be an Err even for an otherwise-valid empty list"
    );
}

#[test]
fn consolidation_r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = consolidation_record(vec![]);
    record.goal_id = "creative-idea-dedup".to_string(); // the OTHER seam's sentinel
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the seam sentinel MUST be an Err"
    );
}

#[test]
fn consolidation_r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = consolidation_record(vec![]);
    record.cycle_number = CYCLE + 1;
    let path = write_json(dir.path(), &record);
    assert!(
        read_verified_idea_consolidation(&path, CONSOLIDATION_GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST be an Err (no replay of a prior verdict)"
    );
}

#[test]
fn consolidation_record_carries_the_pinned_schema_on_the_wire() {
    let json = serde_json::to_value(consolidation_record(vec![cluster("node-1", &[], "r", &[])]))
        .expect("serialize");
    assert_eq!(
        json.get("schema").and_then(|s| s.as_str()),
        Some(IDEA_CONSOLIDATION_SCHEMA),
        "the consolidation record must carry the pinned schema string"
    );
    assert!(
        json.get("clusters").and_then(|c| c.as_array()).is_some(),
        "the consolidation record must carry a `clusters` array (NOT a flattened choice enum)"
    );
}
