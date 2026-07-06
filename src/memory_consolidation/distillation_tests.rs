//! TDD (RED) tests for PR-B: episode distillation.
//!
//! These tests pin the contract documented in
//! `docs/architecture/episode-distillation.md` for PR-B (issue #2281,
//! problem 2). They are written **before** the production code change
//! and are expected to FAIL until PR-B lands.
//!
// Test scaffolding intentionally uses a couple of mildly complex
// shapes (tuple-vec mock + Reverse-by-temporal-index sort) that
// clippy flags but rewriting would obscure the test intent.
#![allow(clippy::type_complexity)]
#![allow(clippy::unnecessary_sort_by)]
//!
//! ## What PR-B introduces
//!
//! * A new module `crate::memory_consolidation::distillation` with:
//!   * `pub struct DistillReport { input_count, fact_count, marked_count }`
//!   * `pub fn distill_recent_episodes(memory, repo_root) -> SimardResult<DistillReport>`
//!   * `pub const DISTILL_BATCH_SIZE: u32` (default 50)
//!   * `pub const DISTILL_MIN_EPISODES: u32` (default 20)
//! * Two new methods on `CognitiveMemoryOps` with default no-ops:
//!   * `fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()>`
//!   * `fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>>`
//! * A pluggable recipe runner for the LLM-side distillation step so
//!   tests can substitute a deterministic stub.
//!
//! ## How these compile against pre-PR-B code
//!
//! Every reference to the new API is gated behind the symbol path
//! `crate::memory_consolidation::distillation::*` and the new trait
//! methods. Because those items do not exist yet, the file will fail
//! to compile.
//!
//! That is the intended TDD red signal — the unresolved-path errors
//! are concrete, deterministic, and unmistakable. PR-B's first commit
//! adds the empty types and stubs; this file becomes the contract that
//! drives the rest of the work.
//!
//! Each test below has a header comment listing the precise symbols
//! that must exist for it to even build, so a future contributor can
//! satisfy them one at a time.

// `distillation` module does not exist yet (PR-B introduces it).
use crate::memory_consolidation::distillation::{
    DISTILL_BATCH_SIZE, DISTILL_FACT_CONFIDENCE, DISTILL_MIN_EPISODES, DISTILL_PARSE_RETRY_MAX,
    DISTILL_RELIABILITY_THRESHOLD, DistillRecipeRunner, DistilledFact, KNOWN_DISTILL_CONCEPTS,
    assess_fact_reliability, distill_recent_episodes_with_runner,
};
// Fixed fact-yield corpus, shared with `distillation_fact_yield_bench` so the
// full-pass test below measures the identical input the benchmark records.
use crate::memory_consolidation::distillation_fact_yield_bench::{
    BASELINE_PROMOTED, CORPUS_EPISODE_COUNT, CORPUS_RECIPE_JSON, IMPROVED_PROMOTED,
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
// Pluggable recipe runner trait
// ───────────────────────────────────────────────────────────────────────────
//
// PR-B exposes `DistillRecipeRunner` so tests can inject a deterministic
// stand-in for `recipe-runner-rs`. Production code will provide a
// concrete impl that shells out; tests use the stubs in this file.
//
// The trait is referenced via the `distill_recent_episodes_with_runner`
// entry point; the public `distill_recent_episodes(memory, repo_root)`
// remains the operator-facing call and is covered by integration
// tests elsewhere.

/// Stub runner that classifies every episode into the supplied facts.
/// Records the call count so tests can assert the LLM was invoked.
struct FixedFactsRunner {
    facts: Vec<(String, String, String)>, // (concept, content, source_episode_id)
    call_count: AtomicU32,
}

impl DistillRecipeRunner for FixedFactsRunner {
    fn run(
        &self,
        episodes: &[CognitiveEpisode],
    ) -> SimardResult<Vec<crate::memory_consolidation::distillation::DistilledFact>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let _ = episodes; // every episode contributes via the fixed-fact set
        Ok(self
            .facts
            .iter()
            .map(
                |(c, content, src)| crate::memory_consolidation::distillation::DistilledFact {
                    concept: c.clone(),
                    content: content.clone(),
                    source_episode_id: src.clone(),
                },
            )
            .collect())
    }
}

/// Stub runner that always returns a recipe error. Records call count
/// so the "no markers set" test can verify that the LLM path WAS
/// invoked but produced no usable output.
struct ErroringRunner {
    call_count: AtomicU32,
}

impl DistillRecipeRunner for ErroringRunner {
    fn run(
        &self,
        _episodes: &[CognitiveEpisode],
    ) -> SimardResult<Vec<crate::memory_consolidation::distillation::DistilledFact>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(SimardError::RpcError(
            "stub: recipe runner deliberately failed".to_string(),
        ))
    }
}

/// Stub runner that PANICS if invoked. Used by the under-threshold
/// test to prove the LLM path was bypassed entirely.
struct PanickingRunner;

impl DistillRecipeRunner for PanickingRunner {
    fn run(
        &self,
        _episodes: &[CognitiveEpisode],
    ) -> SimardResult<Vec<crate::memory_consolidation::distillation::DistilledFact>> {
        panic!("distillation must not invoke the LLM under the min-episode threshold");
    }
}

// ───────────────────────────────────────────────────────────────────────────
// In-memory CognitiveMemoryOps mock with mutable episode store
// ───────────────────────────────────────────────────────────────────────────

/// A minimal `CognitiveMemoryOps` implementation that lets tests load
/// an arbitrary set of episodes, observe `store_fact` / `mark_episode_distilled`
/// calls, and assert post-state.
///
/// Wraps state in `Mutex` so the trait's `&self` signature is honoured
/// while still allowing tests to mutate.
#[derive(Default)]
struct EpisodeMock {
    episodes: Mutex<Vec<EpisodeRow>>,
    facts: Mutex<Vec<(String, String, f64, Vec<String>, String)>>, // (concept, content, conf, tags, source)
    mark_calls: Mutex<Vec<String>>,
    /// Issue #2325: records `store_fact_with_provenance` calls as
    /// `(concept, source_id, source_episode_ids)` so the distillation
    /// provenance test can assert the originating episode id was threaded
    /// through to the provenance write (creating a DERIVES_FROM edge).
    prov_calls: Mutex<Vec<(String, String, Vec<String>)>>,
    /// Issue #2433: pre-existing facts returned by `search_facts`, keyed by
    /// concept, so the reliability gate's "don't clobber a stronger prior"
    /// guard can be exercised. Empty by default → existing tests are
    /// unaffected (the gate finds no prior to protect).
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

    /// Like [`with_episodes`](Self::with_episodes) but pre-seeds the facts that
    /// `search_facts` will return, so the reliability gate's don't-clobber
    /// guard (issue #2433) can be tested.
    fn with_episodes_and_seeded_facts(rows: Vec<EpisodeRow>, seeded: Vec<CognitiveFact>) -> Self {
        Self {
            episodes: Mutex::new(rows),
            seeded_facts: Mutex::new(seeded),
            ..Self::default()
        }
    }

    /// Every stored fact as `(concept, content, confidence)` — unlike
    /// [`facts_stored`](Self::facts_stored) this keeps the confidence so the
    /// reliability-gate tests can assert the *computed* (non-constant) score.
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

    /// `(concept, source_id, source_episode_ids)` for every
    /// `store_fact_with_provenance` call observed (issue #2325).
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
        // Issue #2433: model the production library faithfully — `search_facts`
        // matches the concept label and reads the SAME live graph that
        // `store_fact*` writes to. So return BOTH seeded priors AND facts
        // already stored earlier in this same pass, filtered by concept == query
        // and confidence >= min_conf. Reflecting in-pass writes is exactly what
        // lets the don't-clobber guard's identity check observe an in-batch
        // sibling (the case the earlier concept-only guard silently regressed
        // on). Default (no seed, nothing stored yet) → empty, preserving the
        // pre-#2433 behaviour the other tests rely on.
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

    // === PR-B trait method overrides ===

    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        let eps = self.episodes.lock().unwrap();
        let mut out: Vec<EpisodeRow> = eps.iter().filter(|e| !e.distilled).cloned().collect();
        // Newest first by temporal_index descending (UUID-v7 ids are
        // time-prefixed in production; in tests we just sort by
        // temporal_index).
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

    // === Issue #2325 provenance override ===
    //
    // Records into BOTH `facts` (so existing `facts_stored()` assertions
    // keep working once distillation switches to the provenance write) and
    // `prov_calls` (so the new test can assert the source episode id was
    // threaded through). Note the LIBRARY argument order: `source_id`
    // BEFORE `tags`, with `tags`/`metadata` as `Option`s.
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

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

/// Under-threshold gate: when fewer than `DISTILL_MIN_EPISODES`
/// undistilled episodes are present, the pass MUST be skipped without
/// invoking the recipe runner or calling `mark_episode_distilled`.
///
/// The `PanickingRunner` proves the LLM path is not taken.
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
    assert!(
        mock.marks().is_empty(),
        "no episodes must be marked distilled when the pass is skipped"
    );
    assert!(
        mock.facts_stored().is_empty(),
        "no facts must be stored when the pass is skipped"
    );
}

/// Above-threshold happy path: 25 episodes (≥ MIN) → recipe returns
/// 3 facts → 3 `store_fact` calls AND **all 25** episodes marked
/// distilled (the mark-everything rule prevents replay loops).
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
    assert_eq!(report.fact_count, 3, "3 facts emitted by the recipe stub");
    assert_eq!(
        report.marked_count as usize,
        n.min(DISTILL_BATCH_SIZE as usize),
        "every input episode (including those classified 'skip') must be marked distilled"
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
    assert_eq!(
        marks_set.len() as u32,
        report.marked_count,
        "each marked episode must be marked exactly once"
    );
}

/// Recipe error path: when the recipe runner returns Err, NO facts
/// are stored AND NO episodes are marked distilled. The batch is
/// eligible for retry on the next pass.
#[test]
fn distillation_handles_recipe_error_without_marking() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ErroringRunner {
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner);

    // Two acceptable shapes — Err returned, or Ok with zero work
    // recorded — depending on whether PR-B's author treats recipe
    // failure as fatal or as "skip this pass". Either shape MUST
    // leave the store untouched.
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
        "no facts must be stored when the recipe errors"
    );
    assert!(
        mock.marks().is_empty(),
        "no markers must be set when the recipe errors — retry-safety invariant"
    );
}

/// Mark-everything rule: even when the recipe classifies every input
/// as "skip" and returns zero facts, all input episodes MUST be
/// marked distilled. Otherwise the same low-value episodes would be
/// resubmitted to the LLM forever.
#[test]
fn distillation_marks_episodes_classified_as_skip() {
    let n = DISTILL_MIN_EPISODES as usize;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = FixedFactsRunner {
        facts: vec![], // recipe found nothing useful
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    assert!(
        !report.was_skipped(),
        "above-threshold pass must not be skipped"
    );
    assert_eq!(report.fact_count, 0);
    assert_eq!(
        report.marked_count as usize, n,
        "every input episode must be marked distilled even when the recipe yielded no facts"
    );
    assert!(mock.facts_stored().is_empty());
    assert_eq!(mock.marks().len(), n);
}

/// Independence invariant: distillation MUST NOT touch the
/// `compressed` flag. That flag is owned by the textual
/// `consolidate_episodes` pass; the two passes are independent so
/// their outputs can be attributed in observability.
#[test]
fn distillation_does_not_touch_compressed_flag() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mut rows = n_episodes(n);
    // Pre-mark two episodes as compressed; distillation must leave
    // those flags exactly as it found them.
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

/// Provenance threading (issue #2325, RED): distillation already knows
/// each fact's `source_episode_id`. After wiring, it MUST pass that id to
/// `store_fact_with_provenance` as `source_episode_ids` so a DERIVES_FROM
/// edge is created — while still encoding the textual `distill:{id}`
/// `source_id` for back-compat.
///
/// Pre-wiring this FAILS: distillation calls the legacy `store_fact`,
/// which the mock records under `facts` but NOT under `prov_calls`, so
/// `provenance_calls()` is empty.
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
    assert_eq!(report.fact_count, 2, "two facts emitted by the stub");

    let prov = mock.provenance_calls();
    assert_eq!(
        prov.len(),
        2,
        "every distilled fact must be stored via store_fact_with_provenance so a \
         DERIVES_FROM edge is created; got {prov:?}"
    );

    let by_concept: std::collections::HashMap<&str, &(String, String, Vec<String>)> =
        prov.iter().map(|c| (c.0.as_str(), c)).collect();

    let pr = by_concept
        .get("pr-pattern")
        .expect("pr-pattern must be stored with provenance");
    assert_eq!(
        pr.1, "distill:epi_00003",
        "textual source_id must retain the distill: prefix for back-compat"
    );
    assert_eq!(
        pr.2,
        vec!["epi_00003".to_string()],
        "source_episode_ids must carry the originating episode id"
    );

    let bug = by_concept
        .get("bug-pattern")
        .expect("bug-pattern must be stored with provenance");
    assert_eq!(bug.1, "distill:epi_00009");
    assert_eq!(bug.2, vec!["epi_00009".to_string()]);
}

// ═══════════════════════════════════════════════════════════════════════════
// Issues #2622/#2619: distill facts-document parse (dedicated file channel)
// ═══════════════════════════════════════════════════════════════════════════
//
// Post-fix the distill agent WRITES its `{ "facts": [...], "procedures": [...] }`
// envelope to a dedicated facts file and Simard reads THAT file — never the
// recipe-runner stdout. `parse_facts_document` / `parse_facts` therefore parse
// the agent's facts document (the file contents). The launcher-banner
// contamination that motivated the retired stdout-scraping envelope path can no
// longer reach the parser (see the hermetic `issue_2622_file_channel_tests` in
// `distillation.rs` for the "banner on stdout, valid file ⇒ success" proof).
//
// These tests pin the document parse contract: the facts schema, the concept
// allow-list, provenance survival, prose/fence tolerance in the file, explicit
// errors on empty/answerless documents, and bounded/robust error output.

use crate::memory_consolidation::distillation::{parse_facts, parse_facts_document};

/// A representative facts document carrying BOTH facts and procedures.
const FACTS_AND_PROCEDURES_DOCUMENT: &str = r#"{"facts":[{"concept":"pr-pattern","content":"CI flakes on lbug pin bumps until cache is warmed","source_episode_id":"e7"},{"concept":"lesson-learned","content":"Stale worktrees mask deploy drift","source_episode_id":"e12"}],"procedures":[{"name":"ci-fix:auto","steps":["re-run failed job","warm shared target cache","re-push"],"source_episode_ids":["e7","e9"]}]}"#;

/// A clean facts document must parse into a `DistillOutput`.
#[test]
fn document_parses_bare_facts_object() {
    let out = parse_facts_document(
        r#"{"facts":[{"concept":"pr-pattern","content":"bare","source_episode_id":"epi_1"}]}"#,
    )
    .expect("a clean facts document must parse");
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.facts[0].concept, "pr-pattern");
    assert_eq!(out.facts[0].source_episode_id, "epi_1");
    assert!(out.procedures.is_empty());
}

/// The headline acceptance: a document with facts AND procedures yields both,
/// with provenance intact — distillation produces semantic + procedural memory.
#[test]
fn document_yields_facts_and_procedures() {
    let out = parse_facts_document(FACTS_AND_PROCEDURES_DOCUMENT)
        .expect("a facts+procedures document must parse");
    assert_eq!(out.facts.len(), 2);
    let concepts: std::collections::HashSet<&str> =
        out.facts.iter().map(|f| f.concept.as_str()).collect();
    assert!(concepts.contains("pr-pattern") && concepts.contains("lesson-learned"));
    let sources: std::collections::HashSet<&str> = out
        .facts
        .iter()
        .map(|f| f.source_episode_id.as_str())
        .collect();
    assert!(
        sources.contains("e7") && sources.contains("e12"),
        "fact provenance must survive extraction; got {sources:?}"
    );
    assert_eq!(out.procedures.len(), 1);
    let proc = &out.procedures[0];
    assert_eq!(proc.name, "ci-fix:auto");
    assert_eq!(proc.steps.len(), 3);
    assert_eq!(
        proc.source_episode_ids,
        vec!["e7".to_string(), "e9".to_string()],
        "procedure provenance must carry every source episode id"
    );
}

/// Concept allow-list: labels outside the closed set are dropped.
#[test]
fn document_drops_unknown_concepts() {
    let out = parse_facts_document(
        r#"{"facts":[{"concept":"made-up-label","content":"a","source_episode_id":"epi_1"},{"concept":"lesson-learned","content":"b","source_episode_id":"epi_2"}]}"#,
    )
    .expect("document must parse");
    assert_eq!(out.facts.len(), 1, "the made-up label must be filtered out");
    assert_eq!(out.facts[0].concept, "lesson-learned");
}

/// The agent may add a little prose or a Markdown fence around its answer in the
/// file; the facts are still recovered (clean-channel format leniency, NOT the
/// launcher-banner stdout scraping this fix removed).
#[test]
fn document_tolerates_prose_and_fence() {
    let prose = "Here is the distilled output:\n\
        {\"facts\":[{\"concept\":\"bug-pattern\",\"content\":\"y\",\"source_episode_id\":\"epi_2\"}]}\n\
        Done.";
    let out = parse_facts_document(prose).expect("prose-wrapped document must parse");
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.facts[0].concept, "bug-pattern");

    let fenced = "```json\n\
        {\"facts\":[{\"concept\":\"pr-pattern\",\"content\":\"z\",\"source_episode_id\":\"epi_3\"}],\"procedures\":[]}\n\
        ```";
    let out = parse_facts_document(fenced).expect("fenced document must parse");
    assert_eq!(out.facts.len(), 1);
    assert_eq!(out.facts[0].source_episode_id, "epi_3");
}

/// The facts-only wrapper reads the document too, returning just the facts.
#[test]
fn facts_only_wrapper_reads_document() {
    let facts = parse_facts(FACTS_AND_PROCEDURES_DOCUMENT).expect("facts-only wrapper must parse");
    assert_eq!(facts.len(), 2);
}

// ── Failure semantics — explicit Err, never silent degradation ──────────────

/// An empty document (the agent created the file but wrote nothing) is an
/// explicit `Err` — never a hollow `Ok`.
#[test]
fn empty_document_is_err() {
    assert!(parse_facts_document("").is_err());
    assert!(parse_facts_document("   \n\t ").is_err());
}

/// A document with no facts object (e.g. a launcher banner accidentally written
/// to the file) errors explicitly — the parser never scrapes a banner for a
/// `{ "facts": [...] }` object.
#[test]
fn answerless_document_is_err() {
    let banner = "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference)\n\
        launching copilot binary=/x version=\"GitHub Copilot CLI 1.0.69-1.\"\n";
    assert!(parse_facts_document(banner).is_err());
}

/// A legitimate "nothing worth distilling" answer is a valid SUCCESS (zero
/// facts), never a parse failure.
#[test]
fn empty_facts_envelope_is_success_not_failure() {
    let out = parse_facts_document(r#"{"facts":[],"procedures":[]}"#)
        .expect("an empty envelope is a valid success");
    assert!(out.facts.is_empty() && out.procedures.is_empty());
}

// ── Robustness — bounded error output, no panic/OOM on hostile input ────────

/// Error output must be bounded and must NOT echo the full document (truncated
/// excerpt only): a sentinel far past the truncation window must be absent.
#[test]
fn document_error_does_not_leak_full_payload() {
    let sentinel = "SECRET_SENTINEL_AT_END";
    let raw = format!("{}{sentinel}", "x".repeat(50_000));
    let err =
        parse_facts_document(&raw).expect_err("a 50 KiB blob with no facts object must error");
    let msg = err.to_string();
    assert!(
        !msg.contains(sentinel),
        "error message must not leak content far past the truncation window"
    );
    assert!(
        msg.len() < 600,
        "error message must stay bounded (truncated excerpt), got {} chars",
        msg.len()
    );
}

/// Pathologically deep brace nesting must terminate with an `Err` — no stack
/// overflow, no hang.
#[test]
fn document_tolerates_deeply_nested_input_without_panic() {
    let depth = 50_000;
    let raw = format!("{}{}", "{".repeat(depth), "}".repeat(depth));
    assert!(
        parse_facts_document(&raw).is_err(),
        "deeply nested braces with no facts object must error without panicking"
    );
}

/// A large but VALID document (many facts) must extract every fact.
#[test]
fn document_handles_large_valid_input() {
    let facts: Vec<serde_json::Value> = (0..1_000)
        .map(|i| {
            serde_json::json!({
                "concept": "lesson-learned",
                "content": format!("fact number {i}"),
                "source_episode_id": format!("epi_{i:05}")
            })
        })
        .collect();
    let raw =
        serde_json::to_string(&serde_json::json!({ "facts": facts, "procedures": [] })).unwrap();
    let out = parse_facts_document(&raw).expect("a large valid document must parse all facts");
    assert_eq!(
        out.facts.len(),
        1_000,
        "every fact in a large batch must survive"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2433: ISAO-style reliability gate on consolidation
// ═══════════════════════════════════════════════════════════════════════════
//
// Distillation now SELF-ASSESSES each candidate fact's reliability and gates on
// `Fact.confidence` instead of writing a blind constant (BGML's ISAO, §IV):
//   * low-reliability candidates are quarantined (not promoted), and
//   * a weaker new fact never clobbers a stronger existing fact on the concept.

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

/// A nominal fact — valid in-batch provenance, ≥3 words, known concept —
/// scores at or above the legacy baseline (0.9) so good facts keep their
/// downstream behaviour.
#[test]
fn reliability_score_nominal_fact_is_above_baseline() {
    let episodes = vec![CognitiveEpisode {
        node_id: "epi_00001".to_string(),
        content: "event 1".to_string(),
        source_label: "goal-curator".to_string(),
        temporal_index: 1,
        compressed: false,
    }];
    let fact = DistilledFact {
        concept: "pr-pattern".to_string(),
        content: "enable auto-merge before final review".to_string(),
        source_episode_id: "epi_00001".to_string(),
    };
    let score = assess_fact_reliability(&fact, &episodes, std::slice::from_ref(&fact));
    assert!(
        (score - 0.9).abs() < 1e-9,
        "grounded + ≥3 words + known concept must score 0.9; got {score}"
    );
    assert!(
        score >= DISTILL_FACT_CONFIDENCE,
        "a nominal fact must clear the legacy baseline for continuity"
    );
    assert!(score >= DISTILL_RELIABILITY_THRESHOLD);
}

/// Hallucinated provenance (source episode NOT in the batch) loses the 0.5
/// grounding weight and drops below the gate even with good content.
#[test]
fn reliability_score_ungrounded_fact_is_below_threshold() {
    let episodes = vec![CognitiveEpisode {
        node_id: "epi_00001".to_string(),
        content: "event 1".to_string(),
        source_label: "goal-curator".to_string(),
        temporal_index: 1,
        compressed: false,
    }];
    let fact = DistilledFact {
        concept: "bug-pattern".to_string(),
        content: "this content looks plausible enough".to_string(),
        source_episode_id: "epi_99999".to_string(), // not in batch
    };
    let score = assess_fact_reliability(&fact, &episodes, std::slice::from_ref(&fact));
    assert!(
        score < DISTILL_RELIABILITY_THRESHOLD,
        "ungrounded provenance must fall below the gate; got {score}"
    );
    assert!(
        (score - 0.4).abs() < 1e-9,
        "0.3 content + 0.1 concept = 0.4; got {score}"
    );
}

/// Corroboration: ≥2 facts agreeing on the same concept this pass adds a 0.1
/// independent-agreement bonus, taking a nominal fact to a perfect 1.0.
#[test]
fn reliability_score_corroboration_bonus_applies() {
    let episodes = vec![
        CognitiveEpisode {
            node_id: "epi_00001".to_string(),
            content: "event 1".to_string(),
            source_label: "goal-curator".to_string(),
            temporal_index: 1,
            compressed: false,
        },
        CognitiveEpisode {
            node_id: "epi_00002".to_string(),
            content: "event 2".to_string(),
            source_label: "goal-curator".to_string(),
            temporal_index: 2,
            compressed: false,
        },
    ];
    let batch = vec![
        DistilledFact {
            concept: "pr-pattern".to_string(),
            content: "small PRs merge faster".to_string(),
            source_episode_id: "epi_00001".to_string(),
        },
        DistilledFact {
            concept: "pr-pattern".to_string(),
            content: "auto-merge avoids stale reviews".to_string(),
            source_episode_id: "epi_00002".to_string(),
        },
    ];
    let score = assess_fact_reliability(&batch[0], &episodes, &batch);
    assert!(
        (score - 1.0).abs() < 1e-9,
        "corroborated nominal fact = 1.0; got {score}"
    );
    assert!(KNOWN_DISTILL_CONCEPTS.contains(&"pr-pattern"));
}

/// An off-spec concept loses the concept-validity component.
#[test]
fn reliability_score_unknown_concept_loses_weight() {
    let episodes = vec![CognitiveEpisode {
        node_id: "epi_00001".to_string(),
        content: "event 1".to_string(),
        source_label: "goal-curator".to_string(),
        temporal_index: 1,
        compressed: false,
    }];
    let fact = DistilledFact {
        concept: "made-up-concept".to_string(),
        content: "grounded but off spec concept".to_string(),
        source_episode_id: "epi_00001".to_string(),
    };
    let score = assess_fact_reliability(&fact, &episodes, std::slice::from_ref(&fact));
    assert!(
        (score - 0.8).abs() < 1e-9,
        "0.5 grounded + 0.3 content = 0.8; got {score}"
    );
}

/// Hallucinated provenance must not ride on a same-concept sibling's
/// corroboration to clear the gate: an ungrounded fact stays at 0.4 (content +
/// concept only) even when corroboration is present, because the corroboration
/// bonus is awarded only to grounded facts (issue #2433).
#[test]
fn reliability_score_ungrounded_corroborated_still_below_threshold() {
    let episodes = vec![CognitiveEpisode {
        node_id: "epi_00001".to_string(),
        content: "event 1".to_string(),
        source_label: "goal-curator".to_string(),
        temporal_index: 1,
        compressed: false,
    }];
    // Two same-concept facts → corroboration is present in the batch, but the
    // fact under test cites an episode NOT in the batch (hallucinated provenance).
    let batch = vec![
        DistilledFact {
            concept: "bug-pattern".to_string(),
            content: "looks plausible but provenance is fake".to_string(),
            source_episode_id: "epi_99999".to_string(), // ungrounded
        },
        DistilledFact {
            concept: "bug-pattern".to_string(),
            content: "a grounded sibling on the same concept".to_string(),
            source_episode_id: "epi_00001".to_string(),
        },
    ];
    let score = assess_fact_reliability(&batch[0], &episodes, &batch);
    assert!(
        (score - 0.4).abs() < 1e-9,
        "ungrounded + corroborated must stay at 0.3 content + 0.1 concept = 0.4; got {score}"
    );
    assert!(
        score < DISTILL_RELIABILITY_THRESHOLD,
        "hallucinated provenance must be quarantined regardless of corroboration"
    );
}

/// Empty / whitespace-only content is quarantined unconditionally — even with
/// valid provenance and a known concept it scores 0.0 (issue #2433 hard gate).
#[test]
fn reliability_score_empty_content_is_zero() {
    let episodes = vec![CognitiveEpisode {
        node_id: "epi_00001".to_string(),
        content: "event 1".to_string(),
        source_label: "goal-curator".to_string(),
        temporal_index: 1,
        compressed: false,
    }];
    let fact = DistilledFact {
        concept: "pr-pattern".to_string(),
        content: "   ".to_string(),                 // whitespace only
        source_episode_id: "epi_00001".to_string(), // grounded, but empty
    };
    let score = assess_fact_reliability(&fact, &episodes, std::slice::from_ref(&fact));
    assert_eq!(
        score, 0.0,
        "empty content must score 0.0 regardless of provenance"
    );
    assert!(score < DISTILL_RELIABILITY_THRESHOLD);
}

/// End-to-end gate: a hallucinated-provenance fact is QUARANTINED — not stored
/// — while the grounded fact in the same batch is promoted. Every episode is
/// still marked distilled (the quarantine does not break the replay guard).
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

/// Integrity guard: a weaker new fact must not downgrade a stronger existing
/// copy of the SAME fact (identity = concept + content). The seeded 0.95 prior
/// has identical content to the candidate the recipe re-emits at a computed
/// 0.9, so it is blocked; a distinct fact on another concept is promoted.
#[test]
fn distillation_does_not_clobber_stronger_prior() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes_and_seeded_facts(
        n_episodes(n),
        // Same concept AND same content as the candidate below, but stronger.
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
        "the weaker identical pr-pattern candidate is blocked"
    );

    let stored: Vec<(String, String, f64)> = mock.stored_facts_full();
    assert!(
        !stored.iter().any(|(c, content, _)| c == "pr-pattern"
            && content == "enable auto-merge before final review"),
        "the stronger 0.95 prior must be preserved, not downgraded by the 0.9 candidate"
    );
    assert!(stored.iter().any(|(c, _, _)| c == "bug-pattern"));
}

/// Regression (issue #2433): two DISTINCT facts that share a concept label must
/// BOTH be promoted. The recipe emits only three concept labels and every
/// grounded, well-formed fact scores identically, so an earlier concept-level
/// don't-clobber guard quarantined every same-concept fact after the first —
/// silently neutering distillation. The identity-level guard, plus a mock whose
/// `search_facts` reflects in-pass writes (as production does), proves distinct
/// same-concept facts accumulate. This test FAILS under the concept-only guard.
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
        "two distinct pr-pattern facts must both be promoted, not deduped by label"
    );
    assert_eq!(
        report.quarantined_count, 0,
        "distinct same-concept facts must not be quarantined as clobbering priors"
    );

    let contents: Vec<String> = mock
        .stored_facts_full()
        .into_iter()
        .map(|(_, content, _)| content)
        .collect();
    assert!(contents.contains(&"enable auto-merge before final review".to_string()));
    assert!(contents.contains(&"rebase onto main before requesting review".to_string()));
}

/// Confidence written to semantic memory is the *computed* reliability score,
/// not the legacy constant — turning `confidence` into a live signal.
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

// ───────────────────────────────────────────────────────────────────────────
// PR-1 (issue #2468): bounded in-cycle distill retry with format reinforcement
// ───────────────────────────────────────────────────────────────────────────
//
// TDD (RED) tests written BEFORE the production change; they FAIL until #2468
// lands. They pin the contract:
//
//   * A *transient* failure class (`ParseFailure`, `CopilotTerminalFailure`) is
//     retried in-cycle up to `DISTILL_PARSE_RETRY_MAX` times; a recovered batch
//     stores facts and marks ALL episodes within ONE pass.
//   * A *structural* class (`SpawnFailure`, `SerializeFailure`) escalates
//     immediately — NO retry.
//   * The retry-safety invariant holds across all in-cycle attempts: on final
//     failure NO facts are stored and NO episodes are marked.
//
// Symbol the implementation must add: `pub const DISTILL_PARSE_RETRY_MAX: u32`
// in `distillation.rs`, plus the retry loop in
// `distill_recent_episodes_with_runner`.

/// One scripted outcome per `run_all` attempt. Error variants use the exact
/// stable prefixes `classify_distill_error` anchors on, so each maps to the
/// intended `DistillFailureClass`.
enum Attempt {
    /// Success: emit these `(concept, content, source_episode_id)` facts.
    Ok(Vec<(&'static str, &'static str, &'static str)>),
    /// Transient: exited 0 but output unparseable → `ParseFailure`.
    Parse,
    /// Transient: recipe process exited non-zero → `CopilotTerminalFailure`.
    Terminal,
    /// Structural: runner could not be spawned → `SpawnFailure`.
    Spawn,
}

/// Runner that returns a different scripted outcome on each successive call,
/// recording the call count so the retry bound can be asserted. Once the script
/// is exhausted it repeats the last outcome.
struct ScriptedRunner {
    attempts: Vec<Attempt>,
    call_count: AtomicU32,
}

impl ScriptedRunner {
    fn new(attempts: Vec<Attempt>) -> Self {
        Self {
            attempts,
            call_count: AtomicU32::new(0),
        }
    }
}

impl DistillRecipeRunner for ScriptedRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst) as usize;
        let attempt = self
            .attempts
            .get(idx)
            .or_else(|| self.attempts.last())
            .expect("ScriptedRunner needs at least one attempt");
        match attempt {
            Attempt::Ok(facts) => Ok(facts
                .iter()
                .map(|(c, content, src)| DistilledFact {
                    concept: (*c).to_string(),
                    content: (*content).to_string(),
                    source_episode_id: (*src).to_string(),
                })
                .collect()),
            Attempt::Parse => Err(SimardError::RpcError(
                "distill: facts document did not contain a parseable { \"facts\": [...] } object: hi"
                    .to_string(),
            )),
            Attempt::Terminal => Err(SimardError::RpcError(
                "distill: recipe exited with exit status: 1: stderr= stdout=".to_string(),
            )),
            Attempt::Spawn => Err(SimardError::RpcError(
                "distill: recipe-runner-rs spawn failed: no such file".to_string(),
            )),
        }
    }
}

/// One real, grounded, known-concept fact (passes the ISAO reliability gate) on
/// the success attempt, tied to a real episode id from `n_episodes`.
fn recovered_fact() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![(
        "pr-pattern",
        "enable auto-merge before final review",
        "epi_00001",
    )]
}

/// Transient `ParseFailure` on attempt 1, success on attempt 2: the batch
/// recovers WITHIN ONE PASS — facts stored, every episode marked, runner called
/// exactly twice.
#[test]
fn transient_parse_failure_then_ok_recovers_in_one_pass() {
    let n = DISTILL_MIN_EPISODES as usize + 5;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ScriptedRunner::new(vec![Attempt::Parse, Attempt::Ok(recovered_fact())]);

    let report = distill_recent_episodes_with_runner(&mock, &runner)
        .expect("transient parse miss must be recovered by an in-cycle retry");

    assert!(report.fact_count >= 1, "facts stored after the retry");
    assert_eq!(
        report.marked_count as usize,
        n.min(DISTILL_BATCH_SIZE as usize),
        "every input episode marked distilled within the same pass"
    );
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        2,
        "exactly one retry after the transient parse miss"
    );
    assert!(!mock.facts_stored().is_empty(), "a fact was persisted");
    assert_eq!(
        mock.marks().len() as u32,
        report.marked_count,
        "marks match the report"
    );
}

/// Transient `CopilotTerminalFailure` (non-zero exit) is ALSO retried, then
/// recovers.
#[test]
fn transient_terminal_failure_then_ok_recovers_in_one_pass() {
    let n = DISTILL_MIN_EPISODES as usize + 2;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ScriptedRunner::new(vec![Attempt::Terminal, Attempt::Ok(recovered_fact())]);

    let report = distill_recent_episodes_with_runner(&mock, &runner)
        .expect("a transient terminal failure must be retried and recovered");

    assert!(report.fact_count >= 1, "facts stored after the retry");
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        2,
        "the transient terminal failure is retried once"
    );
}

/// Both attempts fail transiently: the pass returns `Err`, NO facts are stored,
/// NO episodes are marked (retry-safety invariant), and the runner is called a
/// BOUNDED number of times (`DISTILL_PARSE_RETRY_MAX + 1`).
///
/// `#[serial]` on the raw-capture env group: this is the one OTHER test that
/// drives a distill pass to a SURVIVING `ParseFailure`, so it would hit the
/// Wave 1 capture path and — if it ran concurrently with
/// `surviving_parse_failure_writes_a_raw_capture_sample_when_enabled` (which
/// sets the process-global `SIMARD_DISTILL_RAW_CAPTURE*` env) — write a stray
/// sample into that test's capture dir. Serializing it on the same key keeps the
/// enabled-capture assertion (`exactly one sample`) deterministic.
#[test]
#[serial_test::serial(simard_distill_raw_capture_env, cognitive_memory)]
fn transient_failure_exhausts_bounded_retries_then_errs_unmarked() {
    let n = DISTILL_MIN_EPISODES as usize + 3;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    // Always-parse-fail; ScriptedRunner repeats the last outcome past the script.
    let runner = ScriptedRunner::new(vec![Attempt::Parse]);

    let report = distill_recent_episodes_with_runner(&mock, &runner);

    assert!(report.is_err(), "exhausted retries must surface as Err");
    assert!(
        mock.facts_stored().is_empty(),
        "no facts stored when every attempt fails"
    );
    assert!(
        mock.marks().is_empty(),
        "retry-safety: no episodes marked when every attempt fails"
    );
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        DISTILL_PARSE_RETRY_MAX + 1,
        "retries are bounded: one initial attempt + DISTILL_PARSE_RETRY_MAX retries"
    );
}

/// A STRUCTURAL failure (`SpawnFailure`) escalates immediately and is NOT
/// retried, even though a later attempt would have succeeded.
#[test]
fn structural_spawn_failure_is_not_retried() {
    let n = DISTILL_MIN_EPISODES as usize + 1;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ScriptedRunner::new(vec![Attempt::Spawn, Attempt::Ok(recovered_fact())]);

    let report = distill_recent_episodes_with_runner(&mock, &runner);

    assert!(report.is_err(), "a structural failure must escalate");
    assert_eq!(
        runner.call_count.load(Ordering::SeqCst),
        1,
        "structural classes must NOT be retried"
    );
    assert!(
        mock.marks().is_empty(),
        "no episodes marked on a structural failure"
    );
    assert!(mock.facts_stored().is_empty());
}

// ───────────────────────────────────────────────────────────────────────────
// Wave 1 (RED): env-gated raw-capture fires on a SURVIVING parse failure
// ───────────────────────────────────────────────────────────────────────────
//
// Contract (`docs/reference/distill-raw-capture-on-parse-failure.md`): when
// `SIMARD_DISTILL_RAW_CAPTURE` is truthy and a `ParseFailure` survives the
// bounded in-cycle retry, the distill failure path in
// `distill_recent_episodes_with_runner` writes exactly one
// `distill-parsefail-*.txt` sample under the configured capture dir, carrying a
// `# failure_class: parse-failure` header.
//
// This is a RUNNABLE RED: it references no not-yet-existing symbol (only env +
// filesystem + the existing runner path), so it COMPILES against today's code
// and FAILS at runtime because no capture is wired yet. It also pins the
// requirement that capture is gated by the ENV toggle ONLY — never by
// `cfg!(test)` — so it is observable under `cargo test`. Wave 1 turns it green.

/// Scoped env override that restores the previous value on drop.
struct RawCaptureEnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}
impl RawCaptureEnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: this test is `#[serial]` on the raw-capture env group.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
}
impl Drop for RawCaptureEnvGuard {
    fn drop(&mut self) {
        // SAFETY: this test is `#[serial]` on the raw-capture env group.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[test]
#[serial_test::serial(simard_distill_raw_capture_env, cognitive_memory)]
fn surviving_parse_failure_writes_a_raw_capture_sample_when_enabled() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_dir = tmp.path().join("distill-captures");
    let _enable = RawCaptureEnvGuard::set("SIMARD_DISTILL_RAW_CAPTURE", "1");
    let _dir = RawCaptureEnvGuard::set(
        "SIMARD_DISTILL_RAW_CAPTURE_DIR",
        capture_dir.to_str().unwrap(),
    );

    let n = DISTILL_MIN_EPISODES as usize + 3;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    // Every attempt is a ParseFailure ⇒ the retry is exhausted and the pass
    // escalates with `Err(ParseFailure)` — the exact "surviving parse failure"
    // the harvester exists to capture.
    let runner = ScriptedRunner::new(vec![Attempt::Parse]);

    let report = distill_recent_episodes_with_runner(&mock, &runner);
    assert!(report.is_err(), "a surviving parse failure must return Err");

    let samples: Vec<std::path::PathBuf> = std::fs::read_dir(&capture_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n.starts_with("distill-parsefail-") && n.ends_with(".txt"))
                })
                .collect()
        })
        .unwrap_or_default();

    assert_eq!(
        samples.len(),
        1,
        "an enabled, surviving parse failure must write exactly one capture sample; found {samples:?}"
    );
    let body = std::fs::read_to_string(&samples[0]).unwrap();
    assert!(
        body.contains("# failure_class: parse-failure"),
        "captured sample must carry the parse-failure header; body:\n{body}"
    );
}

/// Negative control: with capture DISABLED (the default), the same surviving
/// parse failure writes NO sample. Guards against the diagnostic ever becoming
/// default-on. Runnable RED until capture exists (the dir is simply never
/// created); stays green thereafter because the toggle is off.
#[test]
#[serial_test::serial(simard_distill_raw_capture_env, cognitive_memory)]
fn surviving_parse_failure_writes_nothing_when_capture_disabled() {
    let tmp = tempfile::tempdir().unwrap();
    let capture_dir = tmp.path().join("distill-captures");
    // Explicitly OFF, and point the dir override at the tempdir so that IF a
    // future regression made capture default-on we would still detect the file.
    let _disable = RawCaptureEnvGuard::set("SIMARD_DISTILL_RAW_CAPTURE", "0");
    let _dir = RawCaptureEnvGuard::set(
        "SIMARD_DISTILL_RAW_CAPTURE_DIR",
        capture_dir.to_str().unwrap(),
    );

    let n = DISTILL_MIN_EPISODES as usize + 3;
    let mock = EpisodeMock::with_episodes(n_episodes(n));
    let runner = ScriptedRunner::new(vec![Attempt::Parse]);

    let report = distill_recent_episodes_with_runner(&mock, &runner);
    assert!(report.is_err(), "a surviving parse failure must return Err");

    let created_any = std::fs::read_dir(&capture_dir)
        .map(|mut rd| rd.next().is_some())
        .unwrap_or(false);
    assert!(
        !created_any,
        "capture is default-off: no sample may be written when the toggle is falsy"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Full-pass companion to the deterministic fact-yield benchmark
// ───────────────────────────────────────────────────────────────────────────

/// Runner that parses a fixed raw facts document through the REAL production
/// parse path (`parse_facts_document`), so the concept canonicalization in
/// `RecipeEnvelope::into_facts` is exercised end-to-end via
/// `distill_recent_episodes_with_runner` (parse → gate → dedup guard → store)
/// rather than bypassed by a facts-returning stub.
struct RawEnvelopeRunner {
    raw: &'static str,
    call_count: AtomicU32,
}

impl DistillRecipeRunner for RawEnvelopeRunner {
    fn run(&self, _episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        parse_facts_document(self.raw).map(|o| o.facts)
    }
}

/// Full-pass companion to `distillation_fact_yield_bench`. Routes the SAME fixed
/// corpus through the real `distill_recent_episodes_with_runner` — parse +
/// concept canonicalization + reliability gate + the identity dedup guard +
/// storage — against a FRESH (empty) memory, and confirms all `IMPROVED_PROMOTED`
/// canonical / surface-variant grounded facts are actually stored while the
/// ungrounded and empty candidates are quarantined. This upgrades the
/// benchmark's "dedup-neutral corpus ⇒ parse+gate survivors == full-pass
/// promoted" claim from *asserted* to *exercised*, closing the gap between the
/// DB-free benchmark and a real production pass.
#[test]
fn full_pass_promotes_canonicalized_surface_variants_through_dedup() {
    let rows: Vec<EpisodeRow> = (0..CORPUS_EPISODE_COUNT)
        .map(|i| EpisodeRow {
            node_id: format!("ep-{i:03}"),
            content: format!("episode {i} body"),
            source_label: "distill-bench".to_string(),
            temporal_index: i as i64,
            compressed: false,
            distilled: false,
        })
        .collect();
    let mock = EpisodeMock::with_episodes(rows);
    let runner = RawEnvelopeRunner {
        raw: CORPUS_RECIPE_JSON,
        call_count: AtomicU32::new(0),
    };

    let report = distill_recent_episodes_with_runner(&mock, &runner).unwrap();

    // The runner (and thus the LLM-side parse) ran exactly once — no transient
    // retry, since the fixed corpus parses cleanly.
    assert_eq!(runner.call_count.load(Ordering::SeqCst), 1);

    // All eight canonical/surface-variant grounded facts are promoted through the
    // FULL pipeline (dedup guard included), matching the benchmark's parse+gate
    // survivor count. The two precision-guard candidates (ungrounded ep-999 and
    // empty content) are quarantined by the reliability gate; the three off-spec
    // concepts are dropped at the concept filter (not counted as quarantined).
    assert_eq!(
        report.fact_count, IMPROVED_PROMOTED as u32,
        "full pass must promote all recovered surface-variant facts"
    );
    assert_eq!(
        report.quarantined_count, 2,
        "ungrounded + empty candidates must be quarantined by the reliability gate"
    );
    assert!(
        report.fact_count > BASELINE_PROMOTED as u32,
        "full-pass yield must exceed the exact-match baseline"
    );

    // Every stored fact carries a canonical lower-hyphen concept label, proving
    // canonicalization normalized the surface variants before storage.
    for (concept, _content) in mock.facts_stored() {
        assert!(
            matches!(
                concept.as_str(),
                "pr-pattern" | "bug-pattern" | "lesson-learned"
            ),
            "stored concept was not canonicalized: {concept:?}"
        );
    }
}
