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
    DISTILL_BATCH_SIZE, DISTILL_MIN_EPISODES, DistillRecipeRunner,
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
        Err(SimardError::BridgeError(
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
    fn search_facts(&self, _q: &str, _l: u32, _c: f64) -> SimardResult<Vec<CognitiveFact>> {
        Ok(vec![])
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
