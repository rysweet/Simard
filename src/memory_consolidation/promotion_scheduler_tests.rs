//! TDD (RED) tests for the automatic promotion (distillation) scheduler and
//! the procedure-distillation extension (issue #2327).
//!
//! Written **before** the production code; expected to FAIL TO COMPILE until
//! the `scheduler` module and the distillation procedure extension land. The
//! unresolved-path errors are the intended, deterministic red signal.
//! `cargo build --release` stays green (this module is `#[cfg(test)]`).
//!
//! ## What this pins
//!
//! 1. `crate::memory_consolidation::scheduler`:
//!    ```ignore
//!    pub struct DistillSchedule { pub min_episodes: u32, pub interval_cycles: u32 }
//!    impl DistillSchedule {
//!        pub const DEFAULT_MIN_EPISODES: u32 = 25;
//!        pub const DEFAULT_INTERVAL_CYCLES: u32 = 50;
//!    }
//!    pub enum DistillTrigger { Threshold, Interval, None }
//!    pub fn distill_trigger(undistilled_count: u32, cycles_since_last: u32,
//!                           schedule: &DistillSchedule) -> DistillTrigger;
//!    pub fn run_scheduled_distillation_with_runner(
//!        memory: &dyn CognitiveMemoryOps, runner: &dyn DistillRecipeRunner,
//!        schedule: &DistillSchedule, cycles_since_last: u32,
//!    ) -> SimardResult<Option<DistillReport>>;
//!    ```
//!    The scheduler fires distillation automatically (decoupled from the OODA
//!    `ConsolidateMemory` action) when `undistilled_count >= min_episodes` OR
//!    `cycles_since_last >= interval_cycles`.
//!
//! 2. The distillation procedure extension (issue #2327, R5):
//!    ```ignore
//!    pub struct DistilledProcedure {
//!        pub name: String, pub steps: Vec<String>, pub source_episode_ids: Vec<String>,
//!    }
//!    pub struct DistillOutput {
//!        pub facts: Vec<DistilledFact>, pub procedures: Vec<DistilledProcedure>,
//!    }
//!    // additive default on the runner trait, wraps `run` with empty procedures:
//!    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput>;
//!    // DistillReport gains `procedure_count: u32`.
//!    ```
//!    Distillation stores procedures via `store_procedure_with_provenance`
//!    (threading `source_episode_ids`) alongside provenance-linked facts.

// The in-memory mock records provenance writes as explicit named tuples
// (e.g. `(name, steps, source_episode_ids)`) rather than factoring them into
// type aliases, so each assertion reads as the exact shape under test.
#![allow(clippy::type_complexity)]

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};
use crate::memory_consolidation::distillation::{
    DISTILL_MIN_EPISODES, DistillOutput, DistillRecipeRunner, DistilledFact, DistilledProcedure,
    distill_recent_episodes_with_runner,
};
use crate::memory_consolidation::scheduler::{
    DistillSchedule, DistillTrigger, distill_trigger, run_scheduled_distillation_with_runner,
};

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

// ───────────────────────────────────────────────────────────────────────────
// Recipe runners
// ───────────────────────────────────────────────────────────────────────────

/// Returns a fixed fact set; records call count.
struct FixedFactsRunner {
    facts: Vec<(String, String, String)>, // (concept, content, source_episode_id)
    call_count: AtomicU32,
}

impl DistillRecipeRunner for FixedFactsRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .facts
            .iter()
            .map(|(c, content, src)| DistilledFact {
                concept: c.clone(),
                content: content.clone(),
                source_episode_id: src.clone(),
            })
            .collect())
    }
}

/// Returns BOTH facts and procedures via the additive `run_all` method.
struct FactsAndProceduresRunner {
    facts: Vec<(String, String, String)>,
    procedures: Vec<(String, Vec<String>, Vec<String>)>, // (name, steps, source_episode_ids)
}

impl DistillRecipeRunner for FactsAndProceduresRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        Ok(self
            .facts
            .iter()
            .map(|(c, content, src)| DistilledFact {
                concept: c.clone(),
                content: content.clone(),
                source_episode_id: src.clone(),
            })
            .collect())
    }

    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        Ok(DistillOutput {
            facts: self.run(episodes)?,
            procedures: self
                .procedures
                .iter()
                .map(|(name, steps, src)| DistilledProcedure {
                    name: name.clone(),
                    steps: steps.clone(),
                    source_episode_ids: src.clone(),
                })
                .collect(),
        })
    }
}

/// PANICS if invoked — proves the scheduler bypassed distillation entirely.
struct PanickingRunner;

impl DistillRecipeRunner for PanickingRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        panic!("scheduler must not run distillation when no trigger fires");
    }
}

// ───────────────────────────────────────────────────────────────────────────
// In-memory CognitiveMemoryOps mock
// ───────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct EpisodeRow {
    node_id: String,
    content: String,
    source_label: String,
    temporal_index: i64,
    compressed: bool,
    distilled: bool,
}

#[derive(Default)]
struct EpisodeMock {
    episodes: Mutex<Vec<EpisodeRow>>,
    mark_calls: Mutex<Vec<String>>,
    /// (concept, source_id, source_episode_ids) for each fact-provenance write.
    fact_prov: Mutex<Vec<(String, String, Vec<String>)>>,
    /// (name, steps, source_episode_ids) for each procedure-provenance write.
    proc_prov: Mutex<Vec<(String, Vec<String>, Vec<String>)>>,
}

impl EpisodeMock {
    fn with_episodes(rows: Vec<EpisodeRow>) -> Self {
        Self {
            episodes: Mutex::new(rows),
            ..Self::default()
        }
    }
    fn marks(&self) -> Vec<String> {
        self.mark_calls.lock().unwrap().clone()
    }
    fn fact_provenance_calls(&self) -> Vec<(String, String, Vec<String>)> {
        self.fact_prov.lock().unwrap().clone()
    }
    fn procedure_provenance_calls(&self) -> Vec<(String, Vec<String>, Vec<String>)> {
        self.proc_prov.lock().unwrap().clone()
    }
}

impl CognitiveMemoryOps for EpisodeMock {
    fn record_sensory(&self, _m: &str, _r: &str, _t: u64) -> SimardResult<String> {
        Ok("sen_x".to_string())
    }
    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(0)
    }
    fn push_working(&self, _s: &str, _c: &str, _t: &str, _r: f64) -> SimardResult<String> {
        Ok("wrk_x".to_string())
    }
    fn get_working(&self, _t: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(vec![])
    }
    fn clear_working(&self, _t: &str) -> SimardResult<usize> {
        Ok(0)
    }
    fn store_episode(
        &self,
        _c: &str,
        _s: &str,
        _m: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        Ok("epi_new".to_string())
    }
    fn consolidate_episodes(&self, _b: u32) -> SimardResult<Option<String>> {
        Ok(None)
    }
    fn store_fact(
        &self,
        _concept: &str,
        _content: &str,
        _confidence: f64,
        _tags: &[String],
        _source_id: &str,
    ) -> SimardResult<String> {
        Ok("sem_legacy".to_string())
    }
    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc_legacy".to_string())
    }
    fn recall_procedure(&self, _q: &str, _l: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        Ok(vec![])
    }
    fn store_prospective(&self, _d: &str, _t: &str, _a: &str, _p: i64) -> SimardResult<String> {
        Ok("pro_x".to_string())
    }
    fn check_triggers(&self, _c: &str) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }
    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        Ok(CognitiveStatistics::default())
    }

    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        let eps = self.episodes.lock().unwrap();
        let mut out: Vec<EpisodeRow> = eps.iter().filter(|e| !e.distilled).cloned().collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.temporal_index));
        out.truncate(limit as usize);
        Ok(out
            .into_iter()
            .map(|r| CognitiveEpisode {
                node_id: r.node_id,
                content: r.content,
                source_label: r.source_label,
                temporal_index: r.temporal_index,
                compressed: r.compressed,
                created_at: None,
            })
            .collect())
    }

    fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
        self.mark_calls.lock().unwrap().push(node_id.to_string());
        let mut eps = self.episodes.lock().unwrap();
        if let Some(e) = eps.iter_mut().find(|e| e.node_id == node_id) {
            e.distilled = true;
        }
        Ok(())
    }

    fn store_fact_with_provenance(
        &self,
        concept: &str,
        _content: &str,
        _confidence: f64,
        source_id: &str,
        _tags: Option<&[String]>,
        _metadata: Option<&HashMap<String, serde_json::Value>>,
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.fact_prov.lock().unwrap().push((
            concept.to_string(),
            source_id.to_string(),
            source_episode_ids.to_vec(),
        ));
        Ok("sem_prov".to_string())
    }

    fn store_procedure_with_provenance(
        &self,
        name: &str,
        steps: &[String],
        _prerequisites: &[String],
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.proc_prov.lock().unwrap().push((
            name.to_string(),
            steps.to_vec(),
            source_episode_ids.to_vec(),
        ));
        Ok("prc_prov".to_string())
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

fn episode(idx: i64) -> EpisodeRow {
    EpisodeRow {
        node_id: format!("epi_{idx:05}"),
        content: format!("event {idx}"),
        source_label: "goal-curator".to_string(),
        temporal_index: idx,
        compressed: false,
        distilled: false,
    }
}

fn n_episodes(n: usize) -> Vec<EpisodeRow> {
    (0..n as i64).map(episode).collect()
}

// ───────────────────────────────────────────────────────────────────────────
// DistillSchedule defaults
// ───────────────────────────────────────────────────────────────────────────

/// Config default threshold is 25 episodes (issue #2327, A3) and the
/// cycle-count interval default is 50.
#[test]
fn distill_schedule_defaults() {
    assert_eq!(DistillSchedule::DEFAULT_MIN_EPISODES, 25);
    assert_eq!(DistillSchedule::DEFAULT_INTERVAL_CYCLES, 50);
    let s = DistillSchedule::default();
    assert_eq!(s.min_episodes, 25);
    assert_eq!(s.interval_cycles, 50);
}

// ───────────────────────────────────────────────────────────────────────────
// distill_trigger: pure decision logic
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn trigger_fires_threshold_at_or_above_min_episodes() {
    let s = DistillSchedule::default();
    assert_eq!(distill_trigger(25, 0, &s), DistillTrigger::Threshold);
    assert_eq!(distill_trigger(40, 0, &s), DistillTrigger::Threshold);
}

#[test]
fn trigger_fires_interval_when_cycle_count_reached() {
    let s = DistillSchedule::default();
    // Below the episode threshold, but the cycle interval has elapsed.
    assert_eq!(distill_trigger(5, 50, &s), DistillTrigger::Interval);
    assert_eq!(distill_trigger(0, 51, &s), DistillTrigger::Interval);
}

#[test]
fn trigger_none_below_both_thresholds() {
    let s = DistillSchedule::default();
    assert_eq!(distill_trigger(24, 49, &s), DistillTrigger::None);
    assert_eq!(distill_trigger(0, 0, &s), DistillTrigger::None);
}

// ───────────────────────────────────────────────────────────────────────────
// run_scheduled_distillation_with_runner
// ───────────────────────────────────────────────────────────────────────────

/// Reaching the episode threshold MUST trigger a distillation pass that emits
/// provenance-linked facts — independent of any OODA `ConsolidateMemory`
/// action. Each fact is written via `store_fact_with_provenance` with the
/// originating episode id threaded through as `source_episode_ids`.
#[test]
fn scheduler_at_threshold_runs_distillation_with_provenance_facts() {
    // Exactly the configured threshold of undistilled episodes (>= the hard
    // floor `DISTILL_MIN_EPISODES`, so the underlying pass actually fires).
    let n = DistillSchedule::DEFAULT_MIN_EPISODES as usize;
    assert!(n >= DISTILL_MIN_EPISODES as usize, "threshold ≥ hard floor");
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                "epi_00001".to_string(),
            ),
            (
                "bug-pattern".to_string(),
                "empty outcome list panics cycle.rs".to_string(),
                "epi_00007".to_string(),
            ),
        ],
        call_count: AtomicU32::new(0),
    };
    let schedule = DistillSchedule::default();

    let result = run_scheduled_distillation_with_runner(&mock, &runner, &schedule, 0).expect("ok");

    let report = result.expect("threshold reached → distillation must run (Some report)");
    assert_eq!(report.fact_count, 2, "two facts distilled");
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        1,
        "the recipe runner must be invoked exactly once"
    );

    // Provenance: each fact links back to its source episode.
    let prov = mock.fact_provenance_calls();
    assert_eq!(
        prov.len(),
        2,
        "both facts stored with provenance; got {prov:?}"
    );
    let by_concept: HashMap<&str, &(String, String, Vec<String>)> =
        prov.iter().map(|c| (c.0.as_str(), c)).collect();
    let pr = by_concept.get("pr-pattern").expect("pr-pattern provenance");
    assert_eq!(
        pr.1, "distill:epi_00001",
        "source_id retains distill: prefix"
    );
    assert_eq!(pr.2, vec!["epi_00001".to_string()], "episode id threaded");

    // All input episodes marked distilled (replay-loop guard).
    assert_eq!(
        mock.marks().len(),
        n,
        "every input episode must be marked distilled"
    );
}

/// When neither the episode threshold nor the cycle interval is reached, the
/// scheduler MUST NOT run distillation: returns `Ok(None)`, never touches the
/// runner, stores no facts, marks no episodes.
#[test]
fn scheduler_skips_when_no_trigger() {
    let mock = EpisodeMock::with_episodes(n_episodes(5));
    let runner = PanickingRunner; // panics if distillation runs
    let schedule = DistillSchedule::default();

    let result = run_scheduled_distillation_with_runner(&mock, &runner, &schedule, 0).expect("ok");

    assert!(
        result.is_none(),
        "no trigger → no distillation pass (Ok(None))"
    );
    assert!(mock.fact_provenance_calls().is_empty(), "no facts stored");
    assert!(mock.marks().is_empty(), "no episodes marked distilled");
}

// ───────────────────────────────────────────────────────────────────────────
// Procedure distillation (issue #2327, R5)
// ───────────────────────────────────────────────────────────────────────────

/// Distillation now ALSO emits procedures. They MUST be stored via
/// `store_procedure_with_provenance`, threading `source_episode_ids` so a
/// `PROCEDURE_DERIVES_FROM` edge links the procedure to its episodes, and the
/// report MUST count them in `procedure_count`. Facts continue to be stored
/// with provenance in the same pass.
#[test]
fn distillation_emits_and_stores_procedures_with_provenance() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FactsAndProceduresRunner {
        facts: vec![(
            "lesson-learned".to_string(),
            "prefer keyword overlap over embeddings for episodic recall".to_string(),
            "epi_00003".to_string(),
        )],
        procedures: vec![(
            "ci-fix:auto".to_string(),
            vec![
                "read failing job log".to_string(),
                "reproduce locally".to_string(),
                "apply minimal fix".to_string(),
            ],
            vec!["epi_00002".to_string(), "epi_00005".to_string()],
        )],
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).expect("ok");

    assert_eq!(report.fact_count, 1, "one fact distilled");
    assert_eq!(report.procedure_count, 1, "one procedure distilled");

    // Fact stored with provenance.
    assert_eq!(
        mock.fact_provenance_calls().len(),
        1,
        "fact must be stored via store_fact_with_provenance"
    );

    // Procedure stored with provenance, steps and source episode ids intact.
    let procs = mock.procedure_provenance_calls();
    assert_eq!(
        procs.len(),
        1,
        "procedure must be stored via store_procedure_with_provenance; got {procs:?}"
    );
    let (name, steps, src) = &procs[0];
    assert_eq!(name, "ci-fix:auto");
    assert_eq!(steps.len(), 3, "all procedure steps preserved");
    assert_eq!(
        src,
        &vec!["epi_00002".to_string(), "epi_00005".to_string()],
        "source_episode_ids must be threaded for the PROCEDURE_DERIVES_FROM edge"
    );

    // All inputs marked distilled.
    assert_eq!(mock.marks().len(), n);
}

/// Back-compat: a runner that only implements `run` (no procedures) yields a
/// pass with zero procedures via the default `run_all`, leaving existing
/// fact-only distillation behaviour intact.
#[test]
fn distillation_with_fact_only_runner_emits_no_procedures() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![(
            "pr-pattern".to_string(),
            "small PRs merge faster".to_string(),
            "epi_00000".to_string(),
        )],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).expect("ok");

    assert_eq!(report.fact_count, 1);
    assert_eq!(
        report.procedure_count, 0,
        "fact-only runner must not synthesize procedures"
    );
    assert!(mock.procedure_provenance_calls().is_empty());
}
