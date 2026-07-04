//! Tests for the Rust domain-expertise experiment (roadmap #2491 / #2492 / #2493).
//!
//! These are the acceptance checks for the first experiment: a reproducible
//! baseline scorecard, a measurable pack lift, the issue-#1241 calibration gap,
//! provenance-carrying ingestion, and pack shape invariants.

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

use super::ingest::{COMPETENCY_PREFIX, IngestScope, ingest_pack_into_memory, ingest_pack_scoped};
use super::measurement::{
    CALIBRATION_DEGRADED_MAX, CALIBRATION_HEALTHY_MIN, CALIBRATION_MIN_GAP, CompetencyLevel,
    calibration_gap, measure, run_baseline, run_degraded, run_with_pack,
};
use super::pack::{RUST_EXPERT_PACK, SUBSKILLS};
use super::scenarios::rust_scenarios;

fn mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory cognitive store")
}

// --- Pack shape invariants ---------------------------------------------------

#[test]
fn pack_has_bounded_fact_and_procedure_yield() {
    // #2493: ~10-15 durable facts + 3-5 procedures.
    assert!(
        (10..=15).contains(&RUST_EXPERT_PACK.facts.len()),
        "expected 10-15 facts, got {}",
        RUST_EXPERT_PACK.facts.len()
    );
    assert!(
        (3..=5).contains(&RUST_EXPERT_PACK.procedures.len()),
        "expected 3-5 procedures, got {}",
        RUST_EXPERT_PACK.procedures.len()
    );
}

#[test]
fn every_subskill_is_independently_covered() {
    for subskill in SUBSKILLS {
        assert!(
            RUST_EXPERT_PACK.facts_for(subskill) >= 2,
            "sub-skill '{subskill}' needs >=2 facts, has {}",
            RUST_EXPERT_PACK.facts_for(subskill)
        );
        assert!(
            RUST_EXPERT_PACK.procedures_for(subskill) >= 1,
            "sub-skill '{subskill}' needs >=1 procedure, has {}",
            RUST_EXPERT_PACK.procedures_for(subskill)
        );
    }
}

#[test]
fn every_fact_and_procedure_carries_provenance() {
    for fact in RUST_EXPERT_PACK.facts {
        assert!(
            !fact.provenance.url.is_empty(),
            "fact {} missing url",
            fact.concept
        );
        assert!(
            fact.provenance.url.starts_with("https://"),
            "fact {} url must be a canonical https source: {}",
            fact.concept,
            fact.provenance.url
        );
        assert!(
            !fact.provenance.section.is_empty(),
            "fact {} missing section",
            fact.concept
        );
        assert!(
            !fact.provenance.version.is_empty(),
            "fact {} missing version",
            fact.concept
        );
        assert!(
            SUBSKILLS.contains(&fact.subskill),
            "fact {} bad subskill",
            fact.concept
        );
        assert!(
            fact.tags.contains(&fact.subskill),
            "fact {} tags must include its subskill",
            fact.concept
        );
    }
    for proc in RUST_EXPERT_PACK.procedures {
        assert!(
            !proc.provenance.url.is_empty(),
            "procedure {} missing url",
            proc.name
        );
        assert!(
            SUBSKILLS.contains(&proc.subskill),
            "procedure {} bad subskill",
            proc.name
        );
        assert!(
            !proc.steps.is_empty(),
            "procedure {} has no steps",
            proc.name
        );
    }
}

#[test]
fn scenarios_cover_each_subskill_once() {
    let scenarios = rust_scenarios();
    assert!(
        (3..=5).contains(&scenarios.len()),
        "expected 3-5 scenarios, got {}",
        scenarios.len()
    );
    for subskill in SUBSKILLS {
        let count = scenarios.iter().filter(|s| s.subskill == subskill).count();
        assert_eq!(
            count, 1,
            "sub-skill '{subskill}' should map to exactly one scenario"
        );
    }
}

// --- Ingestion ------------------------------------------------------

#[test]
fn ingest_writes_full_pack_yield_into_memory() {
    let memory = mem();
    let report = ingest_pack_into_memory(&RUST_EXPERT_PACK, &memory).expect("ingest");

    assert_eq!(report.pack_name, "rust-expert");
    assert_eq!(report.facts_ingested, RUST_EXPERT_PACK.facts.len());
    assert_eq!(
        report.procedures_ingested,
        RUST_EXPERT_PACK.procedures.len()
    );
    assert_eq!(report.facts_failed, 0);
    assert_eq!(report.procedures_failed, 0);
    assert_eq!(report.fact_ids.len(), RUST_EXPERT_PACK.facts.len());
    assert_eq!(
        report.total_yield(),
        RUST_EXPERT_PACK.facts.len() + RUST_EXPERT_PACK.procedures.len()
    );

    // The facts are actually recallable from memory afterwards.
    let recalled = memory.search_facts("*", 512, 0.0).expect("recall facts");
    assert_eq!(recalled.len(), RUST_EXPERT_PACK.facts.len());
}

#[test]
fn ingested_facts_retain_source_provenance() {
    let memory = mem();
    ingest_pack_into_memory(&RUST_EXPERT_PACK, &memory).expect("ingest");

    let facts = memory.search_facts("*", 512, 0.0).expect("recall");
    // Every ingested fact should carry a pack tag and a source: tag traceable
    // back to a URL, and a non-empty source_id.
    for fact in &facts {
        assert!(
            fact.tags.iter().any(|t| t == "pack:rust-expert"),
            "fact {} lost its pack tag",
            fact.concept
        );
        assert!(
            fact.tags.iter().any(|t| t.starts_with("source:https://")),
            "fact {} lost its source provenance tag: {:?}",
            fact.concept,
            fact.tags
        );
        assert!(
            fact.source_id.starts_with("kgpack:rust-expert:"),
            "fact {} lost its source_id: {}",
            fact.concept,
            fact.source_id
        );
    }
}

#[test]
fn ingested_procedures_carry_competency_marker() {
    let memory = mem();
    ingest_pack_into_memory(&RUST_EXPERT_PACK, &memory).expect("ingest");
    let procs = memory.recall_procedure("*", 512).expect("recall procs");
    assert_eq!(procs.len(), RUST_EXPERT_PACK.procedures.len());
    for proc in &procs {
        assert!(
            proc.prerequisites
                .iter()
                .any(|p| p.starts_with(COMPETENCY_PREFIX)),
            "procedure {} missing competency marker: {:?}",
            proc.name,
            proc.prerequisites
        );
        // Procedure provenance must persist into memory as a breadcrumb.
        assert!(
            proc.prerequisites
                .iter()
                .any(|p| p.starts_with("source:https://")),
            "procedure {} lost its source provenance breadcrumb: {:?}",
            proc.name,
            proc.prerequisites
        );
        assert!(
            proc.prerequisites.iter().any(|p| p == "pack:rust-expert"),
            "procedure {} lost its pack breadcrumb: {:?}",
            proc.name,
            proc.prerequisites
        );
    }
}

/// Non-circularity control (rebuts the "count-only grader" critique): correctly
/// sub-skill-tagged but semantically-wrong facts, plus the right procedure, must
/// NOT pass — because the grader requires the scenario's *specific* expected
/// concepts, not just any N tagged facts.
#[test]
fn correctly_tagged_but_wrong_concepts_do_not_pass() {
    let memory = mem();
    // Two facts tagged `ownership` but with bogus concepts the scenario never
    // expects, plus the real ownership procedure.
    let tags = vec!["ownership".to_string()];
    memory
        .store_fact_with_provenance(
            "bogus-ownership-fact-1",
            "irrelevant content",
            0.9,
            "kgpack:test",
            Some(&tags),
            None,
            &[],
        )
        .expect("store bogus fact 1");
    memory
        .store_fact_with_provenance(
            "bogus-ownership-fact-2",
            "more irrelevant content",
            0.9,
            "kgpack:test",
            Some(&tags),
            None,
            &[],
        )
        .expect("store bogus fact 2");
    memory
        .store_procedure(
            "rust-expert:fix-use-after-move",
            &["step".to_string()],
            &["competency:ownership".to_string()],
        )
        .expect("store procedure");

    let scorecard = measure(&memory, "wrong-concepts-control").expect("measure");
    // Even though the ownership sub-skill has 2 tagged facts and its procedure,
    // the specific expected concepts are absent, so nothing passes.
    assert_eq!(
        scorecard.scenarios_passed, 0,
        "grader must not pass on correctly-tagged but wrong-concept knowledge"
    );
}

// --- Measurement / scorecard -------------------------------------------------

#[test]
fn baseline_is_novice_with_no_pack() {
    let scorecard = run_baseline().expect("baseline");
    assert_eq!(scorecard.variant, "baseline");
    assert_eq!(scorecard.scenarios_passed, 0);
    assert_eq!(scorecard.pass_rate, 0.0);
    assert_eq!(scorecard.level, CompetencyLevel::Novice);
    assert_eq!(scorecard.facts_in_memory, 0);
    assert_eq!(scorecard.procedures_in_memory, 0);
}

#[test]
fn healthy_pack_lifts_to_expert() {
    let (report, scorecard) = run_with_pack().expect("with pack");
    assert_eq!(scorecard.variant, "rust-expert-pack");
    assert!(report.facts_ingested >= 10);
    assert!(report.procedures_ingested >= 3);
    assert!(
        scorecard.pass_rate > CALIBRATION_HEALTHY_MIN,
        "healthy pass-rate {} must exceed {CALIBRATION_HEALTHY_MIN}",
        scorecard.pass_rate
    );
    assert_eq!(scorecard.level, CompetencyLevel::Expert);
    // Every sub-skill must be solved.
    for s in &scorecard.per_subskill {
        assert_eq!(
            s.pass_rate, 1.0,
            "sub-skill '{}' not fully solved",
            s.subskill
        );
    }
}

#[test]
fn measurement_is_reproducible_across_runs() {
    let a = run_baseline().expect("a");
    let b = run_baseline().expect("b");
    assert_eq!(a.pass_rate, b.pass_rate);
    let (_, p1) = run_with_pack().expect("p1");
    let (_, p2) = run_with_pack().expect("p2");
    assert_eq!(p1.pass_rate, p2.pass_rate);
    assert_eq!(p1.results, p2.results);
}

#[test]
fn right_moment_recall_surfaces_subskill_facts_for_healthy_pack() {
    let (_, scorecard) = run_with_pack().expect("with pack");
    // Each scenario's natural-language query should surface at least one
    // sub-skill fact (Pillar 2c right-moment recall actually works).
    for result in &scorecard.results {
        assert!(
            result.query_recall_hits >= 1,
            "scenario {} query surfaced no sub-skill facts",
            result.scenario_id
        );
    }
}

// --- Calibration guard (issue #1241 discipline) ------------------------------

#[test]
fn calibration_gap_is_enforced() {
    let (_, healthy) = run_with_pack().expect("healthy");
    let (_, degraded) = run_degraded().expect("degraded");

    assert!(
        healthy.pass_rate > CALIBRATION_HEALTHY_MIN,
        "healthy {} must exceed {CALIBRATION_HEALTHY_MIN}",
        healthy.pass_rate
    );
    assert!(
        degraded.pass_rate < CALIBRATION_DEGRADED_MAX,
        "degraded {} must be below {CALIBRATION_DEGRADED_MAX}",
        degraded.pass_rate
    );
    let gap = calibration_gap(&healthy, &degraded);
    assert!(
        gap >= CALIBRATION_MIN_GAP,
        "calibration gap {gap} must be >= {CALIBRATION_MIN_GAP}"
    );
}

#[test]
fn degraded_only_solves_the_ingested_subskill() {
    let (_, degraded) = run_degraded().expect("degraded");
    // Only ownership was ingested, so only that sub-skill can pass.
    let ownership = degraded
        .per_subskill
        .iter()
        .find(|s| s.subskill == "ownership")
        .expect("ownership subskill present");
    assert_eq!(ownership.pass_rate, 1.0);
    let others_solved = degraded
        .per_subskill
        .iter()
        .filter(|s| s.subskill != "ownership")
        .any(|s| s.passed > 0);
    assert!(
        !others_solved,
        "degraded state solved a non-ingested sub-skill"
    );
}

#[test]
fn scoped_ingest_only_writes_requested_subskills() {
    let memory = mem();
    let scope = IngestScope::OnlySubskills(vec!["error-handling".to_string()]);
    let report = ingest_pack_scoped(&RUST_EXPERT_PACK, &memory, &scope).expect("scoped ingest");
    assert_eq!(
        report.facts_ingested,
        RUST_EXPERT_PACK.facts_for("error-handling")
    );
    assert_eq!(
        report.procedures_ingested,
        RUST_EXPERT_PACK.procedures_for("error-handling")
    );

    let scorecard = measure(&memory, "scoped").expect("measure");
    let solved: Vec<&str> = scorecard
        .results
        .iter()
        .filter(|r| r.passed)
        .map(|r| r.subskill.as_str())
        .collect();
    assert_eq!(solved, vec!["error-handling"]);
}
