//! Baseline measurement + Rust competency scorecard (roadmap #2491 Pillar 3).
//!
//! Runs the [`super::scenarios`] set against a cognitive-memory state and emits
//! a per-domain **Rust scorecard**: overall pass-rate, per-sub-skill breakdown,
//! and a novice → competent → expert placement (#2491 §3b). In this first
//! experiment the score measures **knowledge acquisition + right-moment recall
//! coverage** — the scaffold that evidences whether Simard is getting better at
//! Rust — not autonomous code generation.
//!
//! The calibration discipline of issue #1241 is reused: a deliberately-degraded
//! knowledge state must score below [`CALIBRATION_DEGRADED_MAX`] while a healthy
//! one scores above [`CALIBRATION_HEALTHY_MIN`], with the gap asserted in tests
//! ([`calibration_gap`]).

use serde::Serialize;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::error::SimardResult;

use super::bridge::{COMPETENCY_PREFIX, IngestReport, IngestScope, ingest_pack_scoped};
use super::pack::{RUST_EXPERT_PACK, SUBSKILL_OWNERSHIP, SUBSKILLS};
use super::scenarios::{RustScenario, rust_scenarios};

/// A healthy knowledge state must score strictly above this (issue #1241).
pub const CALIBRATION_HEALTHY_MIN: f64 = 0.9;
/// A deliberately-degraded knowledge state must score strictly below this.
pub const CALIBRATION_DEGRADED_MAX: f64 = 0.5;
/// The healthy − degraded pass-rate gap CI asserts (issue #1241).
pub const CALIBRATION_MIN_GAP: f64 = 0.4;

/// How many facts/procedures to pull when grading (comfortably above pack size).
const RECALL_LIMIT: u32 = 512;

/// Competency ladder placement for a domain (#2491 §3b).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompetencyLevel {
    /// Recall coverage < 0.6: the competency to solve most scenarios is not yet
    /// present/recallable in memory.
    Novice,
    /// Recall coverage in `0.6..0.9`: most bounded tasks have their required
    /// competency present and recallable.
    Competent,
    /// Recall coverage >= 0.9: every bounded task's required competency is
    /// present and recallable.
    Expert,
}

/// Result of grading one scenario.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScenarioResult {
    /// Scenario id.
    pub scenario_id: String,
    /// Sub-skill exercised.
    pub subskill: String,
    /// Whether the required competency was present and recallable.
    pub passed: bool,
    /// Sub-skill-tagged facts recalled from memory.
    pub facts_recalled: usize,
    /// Matching procedures recalled from memory.
    pub procedures_recalled: usize,
    /// Facts the natural "moment of need" query surfaced (right-moment recall).
    pub query_recall_hits: usize,
    /// Whether every scenario-specific expected concept was recallable.
    pub expected_concepts_present: bool,
    /// Whether the scenario-specific expected procedure was recallable.
    pub expected_procedure_present: bool,
    /// Human-readable grader detail.
    pub detail: String,
}

/// Per-sub-skill pass breakdown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubskillScore {
    /// Sub-skill name.
    pub subskill: String,
    /// Scenarios passed in this sub-skill.
    pub passed: usize,
    /// Scenarios total in this sub-skill.
    pub total: usize,
    /// `passed / total`.
    pub pass_rate: f64,
}

/// A per-domain Rust competency scorecard.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RustScorecard {
    /// Domain (always `rust` here).
    pub domain: String,
    /// Knowledge variant measured (`baseline`, `rust-expert-pack`, `degraded`).
    pub variant: String,
    /// Total scenarios graded.
    pub scenarios_total: usize,
    /// Scenarios solved.
    pub scenarios_passed: usize,
    /// Overall pass-rate (`scenarios_passed / scenarios_total`).
    pub pass_rate: f64,
    /// Per-sub-skill breakdown, in [`SUBSKILLS`] order.
    pub per_subskill: Vec<SubskillScore>,
    /// Ladder placement.
    pub level: CompetencyLevel,
    /// Facts present in memory at grading time.
    pub facts_in_memory: usize,
    /// Procedures present in memory at grading time.
    pub procedures_in_memory: usize,
    /// Per-scenario detail.
    pub results: Vec<ScenarioResult>,
    /// Free-form measurement notes (multi-seed / reproducibility, etc.).
    pub notes: Vec<String>,
}

impl RustScorecard {
    /// One-line scorecard summary, e.g. `rust: 1.00 (5/5) [expert]`.
    pub fn headline(&self) -> String {
        format!(
            "{}: {:.2} ({}/{}) [{}]",
            self.domain,
            self.pass_rate,
            self.scenarios_passed,
            self.scenarios_total,
            match self.level {
                CompetencyLevel::Novice => "novice",
                CompetencyLevel::Competent => "competent",
                CompetencyLevel::Expert => "expert",
            }
        )
    }
}

fn place_on_ladder(pass_rate: f64) -> CompetencyLevel {
    // First-experiment gym shape (one scenario per sub-skill) makes every
    // per-sub-skill rate binary (0 or 1), so the ladder gates on overall recall
    // coverage. Per-sub-skill breadth floors (roadmap #2491 §3b) become
    // meaningful — and will be re-introduced — once each sub-skill has multiple
    // scenarios.
    if pass_rate >= 0.9 {
        CompetencyLevel::Expert
    } else if pass_rate >= 0.6 {
        CompetencyLevel::Competent
    } else {
        CompetencyLevel::Novice
    }
}

/// Grade one scenario against the recalled memory contents.
///
/// A scenario passes only when memory yields **all** of the scenario's
/// specific expected concepts and its expected procedure, *and* the scenario's
/// natural-language recall query actually surfaces a sub-skill fact (right-moment
/// recall). Requiring named, scenario-specific evidence — not just a count of
/// sub-skill-tagged items — means a pack of correctly-tagged but irrelevant
/// knowledge cannot pass.
fn grade_scenario(
    scenario: &RustScenario,
    all_facts: &[crate::memory_cognitive::CognitiveFact],
    all_procedures: &[crate::memory_cognitive::CognitiveProcedure],
    query_hits: usize,
) -> ScenarioResult {
    let facts_recalled = all_facts
        .iter()
        .filter(|f| f.tags.iter().any(|t| t == scenario.subskill))
        .count();
    let competency_marker = format!("{COMPETENCY_PREFIX}{}", scenario.subskill);
    let procedures_recalled = all_procedures
        .iter()
        .filter(|p| p.prerequisites.iter().any(|pr| pr == &competency_marker))
        .count();

    let expected_concepts_present = scenario
        .expected_concepts
        .iter()
        .all(|concept| all_facts.iter().any(|f| f.concept == *concept));
    let expected_procedure_present = all_procedures
        .iter()
        .any(|p| p.name == scenario.expected_procedure);

    // Right-moment recall must actually land: the scenario's query has to
    // surface at least one sub-skill fact, otherwise the knowledge exists but is
    // not reachable at the moment of need (roadmap Pillar 2c).
    let recall_lands = query_hits >= 1;

    let passed = expected_concepts_present && expected_procedure_present && recall_lands;
    let detail = format!(
        "sub-skill '{}': expected_concepts_present={expected_concepts_present} (need {:?}), \
         expected_procedure_present={expected_procedure_present} ('{}'), \
         query surfaced {query_hits} sub-skill fact(s); grader: {}",
        scenario.subskill, scenario.expected_concepts, scenario.expected_procedure, scenario.grader
    );

    ScenarioResult {
        scenario_id: scenario.id.to_string(),
        subskill: scenario.subskill.to_string(),
        passed,
        facts_recalled,
        procedures_recalled,
        query_recall_hits: query_hits,
        expected_concepts_present,
        expected_procedure_present,
        detail,
    }
}

/// Measure Rust competency of a memory state and produce a scorecard.
///
/// Uses the real cognitive-memory recall path: it pulls every stored fact and
/// procedure once, then grades each scenario against that snapshot, additionally
/// issuing each scenario's natural-language `recall_query` so that right-moment
/// recall must land for the scenario to pass.
pub fn measure(memory: &dyn CognitiveMemoryOps, variant: &str) -> SimardResult<RustScorecard> {
    let all_facts = memory.search_facts("*", RECALL_LIMIT, 0.0)?;
    let all_procedures = memory.recall_procedure("*", RECALL_LIMIT)?;

    let scenarios = rust_scenarios();
    let mut results = Vec::with_capacity(scenarios.len());
    for scenario in scenarios {
        let query_hits = memory
            .search_facts(scenario.recall_query, RECALL_LIMIT, 0.0)?
            .iter()
            .filter(|f| f.tags.iter().any(|t| t == scenario.subskill))
            .count();
        results.push(grade_scenario(
            scenario,
            &all_facts,
            &all_procedures,
            query_hits,
        ));
    }

    let scenarios_total = results.len();
    let scenarios_passed = results.iter().filter(|r| r.passed).count();
    let pass_rate = if scenarios_total == 0 {
        0.0
    } else {
        scenarios_passed as f64 / scenarios_total as f64
    };

    let per_subskill: Vec<SubskillScore> = SUBSKILLS
        .iter()
        .map(|subskill| {
            let matching: Vec<&ScenarioResult> =
                results.iter().filter(|r| r.subskill == *subskill).collect();
            let total = matching.len();
            let passed = matching.iter().filter(|r| r.passed).count();
            let pass_rate = if total == 0 {
                0.0
            } else {
                passed as f64 / total as f64
            };
            SubskillScore {
                subskill: (*subskill).to_string(),
                passed,
                total,
                pass_rate,
            }
        })
        .collect();

    let level = place_on_ladder(pass_rate);

    Ok(RustScorecard {
        domain: "rust".to_string(),
        variant: variant.to_string(),
        scenarios_total,
        scenarios_passed,
        pass_rate,
        per_subskill,
        level,
        facts_in_memory: all_facts.len(),
        procedures_in_memory: all_procedures.len(),
        results,
        notes: vec![
            "Deterministic single-seed run; grading is a pure function of memory \
             contents, so repeated runs reproduce this scorecard exactly."
                .to_string(),
            "Scope (first experiment): the level reflects whether the competency \
             required to solve each scenario is present and recallable from memory \
             (knowledge acquisition + right-moment recall), NOT autonomous code \
             generation. No cargo build/test is run against a candidate solution in \
             this cycle; the scenario graders describe the verification the next \
             cycle will drive an LLM engineer against."
                .to_string(),
        ],
    })
}

/// Run the **baseline** measurement: an empty-memory control (no Rust pack).
///
/// This is the "before" number the roadmap asks for. It is a clean-slate
/// control: current Simard ships **no** `rust-expert` pack, so her semantic
/// memory holds no competency facts/procedures for these Rust sub-skills — an
/// empty store is representative of that "no acquisition yet" starting point.
pub fn run_baseline() -> SimardResult<RustScorecard> {
    let memory = LibraryCognitiveMemory::in_memory()?;
    let mut scorecard = measure(&memory, "baseline")?;
    scorecard.notes.push(
        "Baseline is an empty-memory control (no rust-expert pack ingested), \
         representing Simard's pre-acquisition starting point for these sub-skills."
            .to_string(),
    );
    Ok(scorecard)
}

/// Run the measurement with the healthy `rust-expert` pack ingested.
///
/// Returns the ingest yield ([`IngestReport`]) and the resulting scorecard.
pub fn run_with_pack() -> SimardResult<(IngestReport, RustScorecard)> {
    let memory = LibraryCognitiveMemory::in_memory()?;
    let report = ingest_pack_scoped(&RUST_EXPERT_PACK, &memory, &IngestScope::All)?;
    let mut scorecard = measure(&memory, "rust-expert-pack")?;
    scorecard.notes.push(format!(
        "Ingested {} facts + {} procedures from pack '{}'.",
        report.facts_ingested, report.procedures_ingested, report.pack_name
    ));
    Ok((report, scorecard))
}

/// Run the measurement with a deliberately-degraded knowledge state.
///
/// Only the `ownership` sub-skill is ingested (issue #1241 calibration): a
/// grader that measures real competence must score this state below
/// [`CALIBRATION_DEGRADED_MAX`], well under the healthy run.
pub fn run_degraded() -> SimardResult<(IngestReport, RustScorecard)> {
    let memory = LibraryCognitiveMemory::in_memory()?;
    let scope = IngestScope::OnlySubskills(vec![SUBSKILL_OWNERSHIP.to_string()]);
    let report = ingest_pack_scoped(&RUST_EXPERT_PACK, &memory, &scope)?;
    let mut scorecard = measure(&memory, "degraded")?;
    scorecard.notes.push(
        "Deliberately-degraded state: only the 'ownership' sub-skill ingested \
         (issue #1241 calibration guard)."
            .to_string(),
    );
    Ok((report, scorecard))
}

/// The healthy − degraded pass-rate gap, used by the calibration guard.
pub fn calibration_gap(healthy: &RustScorecard, degraded: &RustScorecard) -> f64 {
    healthy.pass_rate - degraded.pass_rate
}
