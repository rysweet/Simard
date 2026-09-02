//! Pass-behaviour tests for episode distillation (issue #2281, PR-B; gate #2433;
//! semantic handoff #2679).
//!
//! These pin the distillation PASS contract — under-threshold skip, store +
//! mark-everything, retry-safety on recipe error, the reliability quarantine
//! gate, identity dedup, and computed (non-constant) confidence — against a
//! deterministic in-memory `CognitiveMemoryOps` mock and stub runners.
//!
//! Post-#2679 the distiller commits facts through the in-process
//! `DistillFactSink` (the default `run_agentic` memories a run-only stub's facts
//! into it), applying the SAME shared `crate::fact_reliability` gate the IPC
//! server uses. The legacy strict-parse / trailing-comma / field-tolerance and
//! bounded-retry unit tests were RETIRED by #2679 (they asserted a
//! `parse_facts_document` / retry machinery that no longer exists); their
//! replacement is `distillation_semantic_handoff_tests` (exit-status-only result
//! path, gate parity, retry-safety) and `fact_reliability_tests` (the pure
//! scorer).
#![allow(clippy::type_complexity)]
#![allow(clippy::unnecessary_sort_by)]

use crate::memory_consolidation::distillation::{
    DISTILL_BATCH_SIZE, DISTILL_MIN_EPISODES, DistillRecipeRunner, DistilledFact,
    distill_recent_episodes_with_runner,
};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

// ───────────────────────────────────────────────────────────────────────────
// Stub runners
// ───────────────────────────────────────────────────────────────────────────

/// Stub runner that classifies every episode into the supplied facts.
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

/// Stub runner that always returns a recipe error.
struct ErroringRunner {
    call_count: AtomicU32,
}

impl DistillRecipeRunner for ErroringRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(SimardError::RpcError(
            "distill: recipe exited with exit status: 1: stderr= stdout=".to_string(),
        ))
    }
}

/// Stub runner that PANICS if invoked. Proves the LLM path is bypassed under the
/// min-episode threshold.
struct PanickingRunner;

impl DistillRecipeRunner for PanickingRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        panic!("distillation must not invoke the LLM under the min-episode threshold");
    }
}

// ───────────────────────────────────────────────────────────────────────────
// In-memory CognitiveMemoryOps mock with mutable episode store
// ───────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct EpisodeMock {
    episodes: Mutex<Vec<EpisodeRow>>,
    facts: Mutex<Vec<(String, String, f64, Vec<String>, String)>>, // (concept, content, conf, tags, source)
    mark_calls: Mutex<Vec<String>>,
    prov_calls: Mutex<Vec<(String, String, Vec<String>)>>,
    seeded_facts: Mutex<Vec<CognitiveFact>>,
}

#[derive(Clone)]
struct EpisodeRow {
    node_id: String,
    content: String,
    source_label: String,
    temporal_index: i64,
    compressed: bool,
    distilled: bool,
}

impl EpisodeMock {
    fn with_episodes(rows: Vec<EpisodeRow>) -> Self {
        Self {
            episodes: Mutex::new(rows),
            ..Self::default()
        }
    }

    fn with_episodes_and_seeded_facts(rows: Vec<EpisodeRow>, seeded: Vec<CognitiveFact>) -> Self {
        Self {
            episodes: Mutex::new(rows),
            seeded_facts: Mutex::new(seeded),
            ..Self::default()
        }
    }

    fn stored_facts_full(&self) -> Vec<(String, String, f64)> {
        self.facts
            .lock()
            .unwrap()
            .iter()
            .map(|(c, content, conf, _, _)| (c.clone(), content.clone(), *conf))
            .collect()
    }

    fn facts_stored(&self) -> Vec<(String, String)> {
        self.facts
            .lock()
            .unwrap()
            .iter()
            .map(|(c, content, _, _, _)| (c.clone(), content.clone()))
            .collect()
    }

    fn marks(&self) -> Vec<String> {
        self.mark_calls.lock().unwrap().clone()
    }

    fn provenance_calls(&self) -> Vec<(String, String, Vec<String>)> {
        self.prov_calls.lock().unwrap().clone()
    }

    fn compressed_flags(&self) -> Vec<(String, bool)> {
        self.episodes
            .lock()
            .unwrap()
            .iter()
            .map(|e| (e.node_id.clone(), e.compressed))
            .collect()
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
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        self.facts.lock().unwrap().push((
            concept.to_string(),
            content.to_string(),
            confidence,
            tags.to_vec(),
            source_id.to_string(),
        ));
        Ok(format!("sem_{}", self.facts.lock().unwrap().len()))
    }
    fn search_facts(&self, q: &str, _l: u32, min_conf: f64) -> SimardResult<Vec<CognitiveFact>> {
        // Model the production library faithfully — `search_facts` matches the
        // concept label and reads the SAME live graph `store_fact*` writes to, so
        // return BOTH seeded priors AND facts already stored earlier in this pass,
        // filtered by concept == query and confidence >= min_conf. This lets the
        // don't-clobber guard's identity check observe an in-batch sibling.
        let mut out: Vec<CognitiveFact> = self
            .seeded_facts
            .lock()
            .unwrap()
            .iter()
            .filter(|f| f.concept == q && f.confidence >= min_conf)
            .cloned()
            .collect();
        for (concept, content, conf, _tags, source) in self.facts.lock().unwrap().iter() {
            if concept == q && *conf >= min_conf {
                out.push(CognitiveFact {
                    node_id: format!("sem_stored_{concept}"),
                    concept: concept.clone(),
                    content: content.clone(),
                    confidence: *conf,
                    source_id: source.clone(),
                    tags: vec![concept.clone()],
                    usage_count: 0,
                    last_accessed_at: None,
                });
            }
        }
        Ok(out)
    }
    fn store_procedure(&self, _n: &str, _s: &[String], _p: &[String]) -> SimardResult<String> {
        Ok("prc_x".to_string())
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
        out.sort_by(|a, b| b.temporal_index.cmp(&a.temporal_index));
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
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Option<&[String]>,
        _metadata: Option<&HashMap<String, serde_json::Value>>,
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.facts.lock().unwrap().push((
            concept.to_string(),
            content.to_string(),
            confidence,
            tags.map(<[String]>::to_vec).unwrap_or_default(),
            source_id.to_string(),
        ));
        self.prov_calls.lock().unwrap().push((
            concept.to_string(),
            source_id.to_string(),
            source_episode_ids.to_vec(),
        ));
        Ok(format!("sem_{}", self.facts.lock().unwrap().len()))
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

fn episode(idx: i64, label: &str, content: &str) -> EpisodeRow {
    EpisodeRow {
        node_id: format!("epi_{idx:05}"),
        content: content.to_string(),
        source_label: label.to_string(),
        temporal_index: idx,
        compressed: false,
        distilled: false,
    }
}

fn n_episodes(n: usize) -> Vec<EpisodeRow> {
    (0..n as i64)
        .map(|i| episode(i, "goal-curator", &format!("event {i}")))
        .collect()
}

/// Build a pre-existing `CognitiveFact` for seeding the mock's `search_facts`.
fn seeded_fact(concept: &str, content: &str, confidence: f64) -> CognitiveFact {
    CognitiveFact {
        node_id: format!("sem_seed_{concept}"),
        concept: concept.to_string(),
        content: content.to_string(),
        confidence,
        source_id: "prior".to_string(),
        tags: vec![concept.to_string()],
        usage_count: 0,
        last_accessed_at: None,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

/// Under-threshold gate: fewer than `DISTILL_MIN_EPISODES` undistilled episodes
/// → the pass is skipped without invoking the runner or marking anything.
#[test]
fn distillation_skipped_under_min_threshold() {
    let n = (DISTILL_MIN_EPISODES as usize).saturating_sub(1);
    assert!(n >= 1, "DISTILL_MIN_EPISODES must be > 1 for this test");
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = PanickingRunner;

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert!(
        report.was_skipped(),
        "report must signal skipped when input < DISTILL_MIN_EPISODES; got {report:?}"
    );
    assert_eq!(report.input_count, 0);
    assert_eq!(report.fact_count, 0);
    assert_eq!(report.marked_count, 0);
    assert!(mock.marks().is_empty());
    assert!(mock.facts_stored().is_empty());
}

/// Above-threshold happy path: 3 grounded facts → 3 committed AND every input
/// episode marked distilled (the mark-everything rule prevents replay loops).
#[test]
fn distillation_stores_facts_and_marks_originals() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
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
            (
                "lesson-learned".to_string(),
                "prefer keyword overlap over embeddings for episodic recall".to_string(),
                "epi_00012".to_string(),
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert!(
        !report.was_skipped(),
        "above-threshold pass must not be skipped"
    );
    assert_eq!(
        report.input_count as usize,
        n.min(DISTILL_BATCH_SIZE as usize)
    );
    assert_eq!(report.fact_count, 3, "3 grounded facts committed");
    assert_eq!(
        report.marked_count as usize,
        n.min(DISTILL_BATCH_SIZE as usize),
        "every input episode must be marked distilled"
    );
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        1,
        "the recipe runner must be invoked exactly once per pass"
    );

    let stored = mock.facts_stored();
    let concepts: std::collections::HashSet<&str> =
        stored.iter().map(|(c, _)| c.as_str()).collect();
    assert!(concepts.contains("pr-pattern"));
    assert!(concepts.contains("bug-pattern"));
    assert!(concepts.contains("lesson-learned"));

    let marks_set: std::collections::HashSet<String> = mock.marks().into_iter().collect();
    assert_eq!(marks_set.len() as u32, report.marked_count);
}

/// Recipe error path: when the runner returns Err, NO facts are stored AND NO
/// episodes are marked distilled — the batch is retry-eligible next pass.
#[test]
fn distillation_handles_recipe_error_without_marking() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ErroringRunner {
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner);

    match report {
        Err(_) => {}
        Ok(r) => {
            assert_eq!(r.fact_count, 0, "no facts on recipe error");
            assert_eq!(r.marked_count, 0, "no marks on recipe error");
        }
    }
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        1,
        "the recipe runner must be invoked once (the failure happens inside)"
    );
    assert!(
        mock.facts_stored().is_empty(),
        "no facts stored on recipe error"
    );
    assert!(
        mock.marks().is_empty(),
        "no markers set on recipe error — retry-safety invariant"
    );
}

/// Mark-everything rule: even when the recipe returns zero facts, all input
/// episodes MUST be marked distilled so they are not resubmitted forever.
#[test]
fn distillation_marks_episodes_classified_as_skip() {
    let n = DISTILL_MIN_EPISODES as usize;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert!(!report.was_skipped());
    assert_eq!(report.fact_count, 0);
    assert_eq!(
        report.marked_count as usize, n,
        "every input episode must be marked distilled even when the recipe yielded no facts"
    );
    assert!(mock.facts_stored().is_empty());
    assert_eq!(mock.marks().len(), n);
}

/// Independence invariant: distillation MUST NOT touch the `compressed` flag —
/// that flag is owned by the textual `consolidate_episodes` pass.
#[test]
fn distillation_does_not_touch_compressed_flag() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mut rows = n_episodes(n);
    rows[0].compressed = true;
    rows[3].compressed = true;
    let pre_compressed: Vec<(String, bool)> = rows
        .iter()
        .map(|r| (r.node_id.clone(), r.compressed))
        .collect();
    let mock = EpisodeMock::with_episodes(rows);
    let runner = FixedFactsRunner {
        facts: vec![(
            "lesson-learned".to_string(),
            "passes are independent".to_string(),
            "epi_00000".to_string(),
        )],
        call_count: AtomicU32::new(0),
    };

    distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    let post = mock.compressed_flags();
    assert_eq!(
        post, pre_compressed,
        "compressed flags must be identical before and after distillation"
    );
}

/// Provenance threading (issue #2325): each fact is committed via
/// `store_fact_with_provenance` with the originating episode id (a DERIVES_FROM
/// edge) and the textual `distill:{id}` `source_id` for back-compat.
#[test]
fn distillation_passes_source_episode_id_as_provenance() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                "epi_00003".to_string(),
            ),
            (
                "bug-pattern".to_string(),
                "empty outcome list panics cycle.rs".to_string(),
                "epi_00009".to_string(),
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();
    assert_eq!(report.fact_count, 2, "two facts committed");

    let prov = mock.provenance_calls();
    assert_eq!(
        prov.len(),
        2,
        "every fact stored via store_fact_with_provenance; got {prov:?}"
    );

    let by_concept: std::collections::HashMap<&str, &(String, String, Vec<String>)> =
        prov.iter().map(|c| (c.0.as_str(), c)).collect();

    let pr = by_concept.get("pr-pattern").expect("pr-pattern provenance");
    assert_eq!(
        pr.1, "distill:epi_00003",
        "source_id retains distill: prefix"
    );
    assert_eq!(pr.2, vec!["epi_00003".to_string()], "episode id threaded");

    let bug = by_concept
        .get("bug-pattern")
        .expect("bug-pattern provenance");
    assert_eq!(bug.1, "distill:epi_00009");
    assert_eq!(bug.2, vec!["epi_00009".to_string()]);
}

/// End-to-end gate: a hallucinated-provenance fact is QUARANTINED while the
/// grounded fact in the same batch is promoted; every episode is still marked.
#[test]
fn distillation_quarantines_low_reliability_fact() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                "epi_00001".to_string(), // grounded → promoted
            ),
            (
                "bug-pattern".to_string(),
                "looks plausible but provenance is fake".to_string(),
                "epi_99999".to_string(), // NOT in batch → quarantined
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert_eq!(report.fact_count, 1, "only the grounded fact is promoted");
    assert_eq!(
        report.quarantined_count, 1,
        "the hallucinated-provenance fact is gated"
    );
    assert_eq!(
        report.marked_count as usize, n,
        "all episodes still marked distilled"
    );

    let stored: Vec<String> = mock.facts_stored().into_iter().map(|(c, _)| c).collect();
    assert!(stored.contains(&"pr-pattern".to_string()));
    assert!(
        !stored.contains(&"bug-pattern".to_string()),
        "the low-reliability fact must NOT be promoted into semantic memory"
    );
}

/// Seam-parity + fact-yield guard (grounding surface-form robustness): a
/// distilled fact whose `source_episode_id` carries stray SURROUNDING whitespace
/// (an LLM re-emitting a real id with a trailing newline/space) must still ground
/// against the batch and be PROMOTED — exactly as the IPC server seam already
/// does (its `any_episode_exists` grounding trims). Before the fix the in-process
/// sink did an untrimmed `contains`, so this genuinely-grounded fact scored
/// ungrounded (≤0.4) and was silently quarantined — a real fact lost, and the two
/// write-boundary seams disagreeing on disposition. The persisted provenance must
/// also thread the TRIMMED id so its `DERIVES_FROM` edge resolves.
#[test]
fn distillation_grounds_whitespace_padded_source_episode_id() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                " epi_00002 ".to_string(), // padded id of a REAL batch episode
            ),
            (
                "bug-pattern".to_string(),
                "empty outcome list panics cycle".to_string(),
                "epi_00004\n".to_string(), // trailing newline on a REAL id
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert_eq!(
        report.fact_count, 2,
        "both whitespace-padded-but-grounded facts must be promoted (seam parity)"
    );
    assert_eq!(
        report.quarantined_count, 0,
        "a grounded fact must not be quarantined merely for a padded cited id"
    );

    // Provenance threads the TRIMMED id, so the DERIVES_FROM edge resolves.
    let prov = mock.provenance_calls();
    let by_concept: std::collections::HashMap<&str, &(String, String, Vec<String>)> =
        prov.iter().map(|c| (c.0.as_str(), c)).collect();
    let pr = by_concept.get("pr-pattern").expect("pr-pattern provenance");
    assert_eq!(
        pr.1, "distill:epi_00002",
        "source_id must carry the trimmed id, not the padded one"
    );
    assert_eq!(
        pr.2,
        vec!["epi_00002".to_string()],
        "provenance episode id must be trimmed so the DERIVES_FROM edge resolves"
    );
    let bug = by_concept
        .get("bug-pattern")
        .expect("bug-pattern provenance");
    assert_eq!(bug.2, vec!["epi_00004".to_string()]);
}

/// Integrity guard: a weaker new fact must not downgrade a stronger existing copy
/// of the SAME fact (identity = concept + content).
#[test]
fn distillation_does_not_clobber_stronger_prior() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes_and_seeded_facts(
        n_episodes(n),
        vec![seeded_fact(
            "pr-pattern",
            "enable auto-merge before final review",
            0.95,
        )],
    );
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                "epi_00001".to_string(), // computed 0.9 < identical 0.95 prior → blocked
            ),
            (
                "bug-pattern".to_string(),
                "empty outcome list panics cycle.rs".to_string(),
                "epi_00007".to_string(), // no prior → promoted
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert_eq!(
        report.fact_count, 1,
        "only the un-conflicted fact is promoted"
    );
    assert_eq!(
        report.quarantined_count, 1,
        "the weaker identical candidate is blocked"
    );

    let stored: Vec<(String, String, f64)> = mock.stored_facts_full();
    assert!(
        !stored.iter().any(|(c, content, _)| c == "pr-pattern"
            && content == "enable auto-merge before final review"),
        "the stronger 0.95 prior must be preserved, not downgraded by the 0.9 candidate"
    );
    assert!(stored.iter().any(|(c, _, _)| c == "bug-pattern"));
}

/// Regression (issue #2433): two DISTINCT facts sharing a concept label must BOTH
/// be promoted — identity dedup, not label dedup.
#[test]
fn distillation_promotes_distinct_same_concept_facts() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![
            (
                "pr-pattern".to_string(),
                "enable auto-merge before final review".to_string(),
                "epi_00001".to_string(),
            ),
            (
                "pr-pattern".to_string(),
                "rebase onto main before requesting review".to_string(),
                "epi_00002".to_string(),
            ),
        ],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert_eq!(
        report.fact_count, 2,
        "two distinct pr-pattern facts must both be promoted"
    );
    assert_eq!(
        report.quarantined_count, 0,
        "distinct same-concept facts must not be quarantined"
    );

    let contents: Vec<String> = mock
        .stored_facts_full()
        .into_iter()
        .map(|(_, content, _)| content)
        .collect();
    assert!(contents.contains(&"enable auto-merge before final review".to_string()));
    assert!(contents.contains(&"rebase onto main before requesting review".to_string()));
}

/// Confidence written to semantic memory is the *computed* reliability score, not
/// a legacy constant.
#[test]
fn distilled_fact_confidence_is_computed_not_constant() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![(
            "pr-pattern".to_string(),
            "enable auto-merge before final review".to_string(),
            "epi_00001".to_string(),
        )],
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();
    assert_eq!(report.fact_count, 1);

    let stored = mock.stored_facts_full();
    assert_eq!(stored.len(), 1);
    let (concept, _, confidence) = &stored[0];
    assert_eq!(concept, "pr-pattern");
    assert!(
        (confidence - 0.9).abs() < 1e-9,
        "confidence must be the computed score 0.9, not a constant; got {confidence}"
    );
}
