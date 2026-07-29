//! Tests for the typed goal-outcome-verification decision RECORD, its
//! fail-CLOSED reader, the shared closed-enum chokepoint, and the Group-D
//! source-shape contract (Group D of epic #4719, issue #4967).
//!
//! This is the FINAL pair of seams that converts the forbidden "recipe prints
//! JSON → Rust scrapes stdout → Rust acts" antipattern to the typed-record +
//! fail-closed-read pattern:
//!   * the goal-outcome-verification seam (`decide_goal_outcome_verification`)
//!   * the RustyClawd per-goal-cycle seam (`decide_per_goal_cycle`, which now
//!     reuses the existing `record-decision` record + `read_verified`).
//!
//! Every failure mode of [`read_verified_outcome`] is an `Err` (a visible cycle
//! failure that keeps the goal open), never a silent `keep_open_and_report`
//! (#1711). The R1–R7 matrix below is the load-bearing invariant.
//!
//! IMPORTANT: Group D removed ONLY its five owned dead symbols, and Group E
//! (#4967) has since retired the engineer-lifecycle stdout-scrape seam (its ACT
//! effect is now a typed `EngineerLifecycleRecord` read fail-closed). The shared
//! `extract_and_parse_json` / `extract.rs` MUST still survive — but now for the
//! remaining out-of-scope consumers (journal `pr_source`, goal-curation
//! `recipe_progress_checker`), NOT the lifecycle decision path. The source-shape
//! assertions below are scoped to the Group-D-owned symbols only; the lifecycle
//! retirement is asserted by the retention guard at the bottom of this file.

use std::path::{Path, PathBuf};

use super::{GoalOutcomeDecision, OUTCOME_SCHEMA, OutcomeDecisionRecord, read_verified_outcome};

const GOAL: &str = "kgpacks-e2big-live-verify-70ab8541";
const CYCLE: u32 = 0;

// ---------------------------------------------------------------------------
// Helpers — hermetic temp dir + direct record writer (no CLI, no subprocess),
// so the reader is exercised in isolation. Even a record the tool would never
// produce must fail CLOSED (defense-in-depth).
// ---------------------------------------------------------------------------

fn write_bytes(dir: &Path, bytes: &[u8]) -> PathBuf {
    let path = dir.join("outcome.json");
    std::fs::write(&path, bytes).expect("write record bytes");
    path
}

fn write_record(dir: &Path, record: &OutcomeDecisionRecord) -> PathBuf {
    let bytes = serde_json::to_vec(record).expect("serialize record");
    write_bytes(dir, &bytes)
}

fn record_for(decision: GoalOutcomeDecision) -> OutcomeDecisionRecord {
    OutcomeDecisionRecord {
        schema: OUTCOME_SCHEMA.to_string(),
        goal_id: GOAL.to_string(),
        cycle_number: CYCLE,
        decision,
    }
}

// ---------------------------------------------------------------------------
// Schema pin sanity.
// ---------------------------------------------------------------------------

#[test]
fn outcome_schema_is_the_pinned_v1_string() {
    assert_eq!(
        OUTCOME_SCHEMA, "simard.ooda.outcome.v1",
        "the reader pins this exact schema; bumping it is a coordinated change"
    );
}

// ---------------------------------------------------------------------------
// The shared chokepoint — GoalOutcomeDecision::from_choice_fields.
// ---------------------------------------------------------------------------

#[test]
fn chokepoint_builds_each_variant() {
    assert!(matches!(
        GoalOutcomeDecision::from_choice_fields("mark_achieved", "verified live", ""),
        Some(GoalOutcomeDecision::MarkAchieved { .. })
    ));
    assert!(matches!(
        GoalOutcomeDecision::from_choice_fields("reopen", "effect absent", ""),
        Some(GoalOutcomeDecision::Reopen { .. })
    ));
    assert!(matches!(
        GoalOutcomeDecision::from_choice_fields("keep_open_and_report", "ambiguous", ""),
        Some(GoalOutcomeDecision::KeepOpenAndReport { .. })
    ));
    match GoalOutcomeDecision::from_choice_fields("replan", "wrong layer", "aim at argv assembly") {
        Some(GoalOutcomeDecision::Replan {
            rationale,
            replan_hint,
        }) => {
            assert_eq!(rationale, "wrong layer");
            assert_eq!(replan_hint, "aim at argv assembly");
        }
        other => panic!("expected Replan, got {other:?}"),
    }
}

#[test]
fn chokepoint_matches_choice_case_insensitively_and_trims() {
    assert!(matches!(
        GoalOutcomeDecision::from_choice_fields(" REopen ", " effect absent ", ""),
        Some(GoalOutcomeDecision::Reopen { .. })
    ));
}

#[test]
fn chokepoint_rejects_unknown_choice() {
    assert!(
        GoalOutcomeDecision::from_choice_fields("archive_now", "x", "").is_none(),
        "an unknown choice tag must be rejected — the closed enum is the sole authority"
    );
}

#[test]
fn chokepoint_rejects_empty_or_control_only_rationale() {
    assert!(
        GoalOutcomeDecision::from_choice_fields("reopen", "", "").is_none(),
        "an empty rationale must be rejected"
    );
    assert!(
        GoalOutcomeDecision::from_choice_fields("reopen", "   ", "").is_none(),
        "a whitespace-only rationale must be rejected"
    );
    assert!(
        GoalOutcomeDecision::from_choice_fields("reopen", "\u{1b}\u{7}\u{1}", "").is_none(),
        "a control-byte-only rationale collapses to empty after sanitize ⇒ rejected (fail CLOSED)"
    );
}

#[test]
fn chokepoint_owns_replan_hint_to_replan_only() {
    for choice in ["mark_achieved", "reopen", "keep_open_and_report"] {
        assert!(
            GoalOutcomeDecision::from_choice_fields(choice, "valid reason", "smuggled hint")
                .is_none(),
            "a non-empty replan_hint on `{choice}` MUST be rejected — replan owns the hint"
        );
    }
    // An empty hint is allowed on the non-replan variants.
    assert!(
        GoalOutcomeDecision::from_choice_fields("reopen", "valid reason", "").is_some(),
        "an empty hint on a non-replan choice is fine"
    );
    // replan tolerates an empty hint.
    assert!(
        GoalOutcomeDecision::from_choice_fields("replan", "valid reason", "").is_some(),
        "replan with an empty hint is allowed"
    );
}

#[test]
fn chokepoint_bounds_and_sanitizes_rationale() {
    let huge = "z".repeat(5_000);
    let decided = GoalOutcomeDecision::from_choice_fields("reopen", &huge, "").expect("must build");
    assert!(
        decided.rationale().chars().count() <= 501,
        "rationale must be bounded, got {} chars",
        decided.rationale().chars().count()
    );
    let decided = GoalOutcomeDecision::from_choice_fields(
        "reopen",
        "effect \u{1b}[31mabsent\u{1b}[0m still",
        "",
    )
    .expect("must build");
    assert!(
        !decided.rationale().contains('\u{1b}'),
        "ANSI must be stripped from the rationale; got {:?}",
        decided.rationale()
    );
}

// ---------------------------------------------------------------------------
// R8 — all checks pass ⇒ Ok. Every variant round-trips bit-for-bit.
// ---------------------------------------------------------------------------

#[test]
fn read_verified_outcome_round_trips_every_variant() {
    let variants = [
        GoalOutcomeDecision::MarkAchieved {
            rationale: "verified live signal corroborates the criteria".into(),
        },
        GoalOutcomeDecision::Reopen {
            rationale: "artifact landed, live effect absent".into(),
        },
        GoalOutcomeDecision::Replan {
            rationale: "wrong layer; E2BIG persists".into(),
            replan_hint: "target the spawn argv assembly".into(),
        },
        GoalOutcomeDecision::KeepOpenAndReport {
            rationale: "ambiguous this cycle".into(),
        },
    ];
    for decision in variants {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_record(dir.path(), &record_for(decision.clone()));
        let read = read_verified_outcome(&path, GOAL, CYCLE)
            .expect("a well-formed record must verify (R8)");
        assert_eq!(
            read, decision,
            "read_verified_outcome must return the exact recorded decision"
        );
    }
}

// ---------------------------------------------------------------------------
// R1 — file absent (tool never ran / binary unresolvable / tool exited nonzero).
// ---------------------------------------------------------------------------

#[test]
fn r1_absent_record_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("outcome.json"); // never created
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R1: an absent record MUST fail CLOSED (Err → keep goal open), never a default decision"
    );
}

// ---------------------------------------------------------------------------
// R2 — present but not valid JSON / truncated.
// ---------------------------------------------------------------------------

#[test]
fn r2_malformed_json_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"{ not valid json ");
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R2: malformed JSON MUST fail CLOSED"
    );
}

#[test]
fn r2_empty_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_bytes(dir.path(), b"");
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R2: an empty (truncated) file MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R3 — schema != OUTCOME_SCHEMA (e.g. a future …v2). Version pin.
// ---------------------------------------------------------------------------

#[test]
fn r3_wrong_schema_version_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"simard.ooda.outcome.v2","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"reopen","rationale":"future record"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R3: a mismatched schema version MUST fail CLOSED"
    );
}

#[test]
fn r3_missing_schema_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"reopen","rationale":"no schema"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R3: a record with no schema field MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R4 — choice not one of the four closed variants.
// ---------------------------------------------------------------------------

#[test]
fn r4_out_of_enum_choice_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{OUTCOME_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"archive_now","rationale":"smuggled"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R4: an unknown choice tag MUST fail CLOSED — the closed enum is the sole authority"
    );
}

// ---------------------------------------------------------------------------
// R5 — rationale missing / empty / control-only.
// ---------------------------------------------------------------------------

#[test]
fn r5_empty_rationale_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{OUTCOME_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"reopen","rationale":""}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R5: an empty rationale MUST fail CLOSED"
    );
}

#[test]
fn r5_missing_rationale_field_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        r#"{{"schema":"{OUTCOME_SCHEMA}","goal_id":"{GOAL}","cycle_number":{CYCLE},"choice":"reopen"}}"#
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R5: a record with no rationale field MUST fail CLOSED"
    );
}

#[test]
fn r5_control_byte_only_rationale_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{OUTCOME_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"reopen\",\"rationale\":\"\\u001b\\u0007\\u0001\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R5: a control-byte-only rationale sanitizes to empty ⇒ fail CLOSED"
    );
}

#[test]
fn read_verified_outcome_sanitizes_ansi_control_from_rationale() {
    let dir = tempfile::tempdir().expect("tempdir");
    let json = format!(
        "{{\"schema\":\"{OUTCOME_SCHEMA}\",\"goal_id\":\"{GOAL}\",\"cycle_number\":{CYCLE},\"choice\":\"reopen\",\"rationale\":\"\\u001b[31mALERT\\u001b[0m effect absent\"}}"
    );
    let path = write_bytes(dir.path(), json.as_bytes());
    let decision =
        read_verified_outcome(&path, GOAL, CYCLE).expect("a sanitizable rationale must verify");
    let r = decision.rationale();
    assert!(
        !r.contains('\u{1b}') && r.contains("ALERT") && r.contains("effect absent"),
        "ANSI/C0 bytes MUST be stripped on read while preserving text; got {r:?}"
    );
}

// ---------------------------------------------------------------------------
// R6 — goal_id mismatch (stale / other-goal record).
// ---------------------------------------------------------------------------

#[test]
fn r6_goal_id_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = record_for(GoalOutcomeDecision::MarkAchieved {
        rationale: "verified".into(),
    });
    record.goal_id = "some-other-goal".into();
    let path = write_record(dir.path(), &record);
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R6: a record whose embedded goal_id differs from the live ctx MUST fail CLOSED"
    );
}

// ---------------------------------------------------------------------------
// R7 — cycle_number mismatch (a prior cycle's verdict lingering on disk).
// ---------------------------------------------------------------------------

#[test]
fn r7_cycle_number_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut record = record_for(GoalOutcomeDecision::Reopen {
        rationale: "prior-cycle verdict".into(),
    });
    record.cycle_number = CYCLE + 1;
    let path = write_record(dir.path(), &record);
    assert!(
        read_verified_outcome(&path, GOAL, CYCLE).is_err(),
        "R7: a record from a different cycle MUST fail CLOSED (no replay of a prior verdict)"
    );
}

// ---------------------------------------------------------------------------
// Group-D SOURCE-SHAPE CONTRACT. These read the on-disk sources/recipes so they
// pin the *shape* of the conversion regardless of runtime wiring.
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_rel(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

const RECIPE_BRAIN: &str = "src/ooda_brain/recipe_brain.rs";
const RUSTYCLAWD: &str = "src/ooda_brain/rustyclawd.rs";
const MOD_RS: &str = "src/ooda_brain/mod.rs";
const OODA_CLI: &str = "src/operator_cli/ooda.rs";
const OUTCOME_RECIPE: &str = "prompt_assets/simard/recipes/ooda-goal-outcome-verification.yaml";

#[test]
fn mod_exposes_the_outcome_record_surface() {
    let src = read_rel(MOD_RS);
    for symbol in [
        "OUTCOME_SCHEMA",
        "OutcomeDecisionRecord",
        "read_verified_outcome",
        "RecordDecisionContext",
    ] {
        assert!(
            src.contains(symbol),
            "{MOD_RS} must expose `{symbol}` for the Group-D outcome seam"
        );
    }
}

#[test]
fn group_d_owned_dead_symbols_are_deleted() {
    // The five Group-D-owned scrape symbols must be GONE from the tree. This is
    // scoped to Group-D's own symbols — NOT the shared `extract_and_parse_json`,
    // which the engineer-lifecycle path still uses (see the survival test below).
    let recipe_brain = read_rel(RECIPE_BRAIN);
    for gone in [
        "OutcomeEnvelope",
        "parse_outcome_decision",
        "outcome_decision_from_variant",
    ] {
        assert!(
            !recipe_brain.contains(gone),
            "{RECIPE_BRAIN} must no longer contain the deleted Group-D symbol `{gone}`"
        );
    }
    let rustyclawd = read_rel(RUSTYCLAWD);
    assert!(
        !rustyclawd.contains("parse_per_goal_action_from_response"),
        "{RUSTYCLAWD} must no longer scrape per-goal actions from prose"
    );
    let mod_rs = read_rel(MOD_RS);
    for gone in ["from_recipe_envelope", "PerGoalEnvelope"] {
        assert!(
            !mod_rs.contains(gone),
            "{MOD_RS} must no longer contain the deleted Group-D symbol `{gone}`"
        );
    }
}

#[test]
fn outcome_seam_reads_the_typed_record_not_stdout() {
    let src = read_rel(RECIPE_BRAIN);
    assert!(
        src.contains("read_verified_outcome"),
        "the outcome seam must READ the typed record via read_verified_outcome"
    );
    assert!(
        src.contains("run_outcome_verify_recipe"),
        "the outcome seam must run the recipe via run_outcome_verify_recipe (stdout ignored)"
    );
    assert!(
        !src.contains("invoke_outcome_verify_raw"),
        "the old stdout-scraping invoke_outcome_verify_raw must be gone"
    );
}

#[test]
fn rustyclawd_seam_records_then_reads_verified() {
    let src = read_rel(RUSTYCLAWD);
    assert!(
        src.contains("submit_for_record") && src.contains("super::read_verified("),
        "the RustyClawd per-goal seam must record via submit_for_record then read_verified"
    );
    assert!(
        src.contains("record-decision"),
        "the RustyClawd prompt must instruct the agent to call `simard ooda record-decision`"
    );
}

#[test]
fn outcome_recipe_calls_the_tool_and_threads_record_vars() {
    let recipe = read_rel(OUTCOME_RECIPE);
    assert!(
        recipe.contains("ooda record-outcome"),
        "the outcome recipe must instruct the agent to CALL `simard ooda record-outcome`"
    );
    for var in [
        "{{record_path}}",
        "{{simard_bin}}",
        "{{cycle_number}}",
        "{{goal_id}}",
    ] {
        assert!(
            recipe.contains(var),
            "the outcome recipe must thread the `{var}` context var"
        );
    }
    assert!(
        !recipe.contains(r#"{"decision""#),
        "the outcome recipe must no longer instruct the agent to PRINT a JSON decision envelope"
    );
}

#[test]
fn operator_cli_dispatches_the_record_outcome_arm() {
    let src = read_rel(OODA_CLI);
    assert!(
        src.contains("\"record-outcome\"") && src.contains("dispatch_record_outcome"),
        "operator_cli must dispatch the `record-outcome` verb"
    );
}

// ---------------------------------------------------------------------------
// RETENTION GUARD — the shared `extract_and_parse_json` / `extract.rs` MUST
// survive because out-of-scope consumers (journal `pr_source`, goal-curation
// `recipe_progress_checker`) still use `strip_recipe_noise` / the shared
// extractor. Group E (#4967) retired ONLY the engineer-lifecycle stdout-scrape
// seam: the ACT effect is now a typed `EngineerLifecycleRecord` read
// fail-closed, so `extract_decision_envelope` / `DecisionEnvelope` MUST be gone
// from recipe_brain and the typed reader MUST be wired in their place.
// ---------------------------------------------------------------------------

#[test]
fn shared_extract_survives_for_out_of_scope_consumers() {
    // The shared extractor module and function still exist in the tree.
    let extract = read_rel("src/recipe_output/extract.rs");
    assert!(
        extract.contains("extract_and_parse_json"),
        "extract_and_parse_json MUST survive — out-of-scope consumers still use the shared extractor"
    );
}

#[test]
fn lifecycle_scrape_path_is_retired_for_the_typed_record() {
    // Group E (#4967): recipe_brain no longer scrapes the lifecycle decision —
    // the envelope parsers are gone and the typed fail-closed reader is wired.
    let recipe_brain = read_rel(RECIPE_BRAIN);
    assert!(
        !recipe_brain.contains("extract_decision_envelope")
            && !recipe_brain.contains("DecisionEnvelope"),
        "the engineer-lifecycle stdout-scrape envelope path MUST be retired (Group E #4967)"
    );
    assert!(
        recipe_brain.contains("read_verified_engineer_lifecycle_decision"),
        "the lifecycle rail MUST read the typed record fail-closed"
    );
    let cli = read_rel(OODA_CLI);
    assert!(
        cli.contains("dispatch_record_lifecycle_decision")
            && cli.contains("\"record-lifecycle-decision\""),
        "operator_cli MUST dispatch the gated `record-lifecycle-decision` writer verb"
    );
}
