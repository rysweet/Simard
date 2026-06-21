//! Cognitive memory: the [`CognitiveMemoryOps`] trait and its sole backend.
//!
//! De-fork Phase 2b (issue #2307): Simard's native LadybugDB fork has been
//! deleted. The [`CognitiveMemoryOps`] trait defines the backend-agnostic API;
//! the only implementation is [`LibraryCognitiveMemory`], which delegates to the
//! upstream `amplihack-memory-lib` `CognitiveMemory` (persistent, lbug-backed).
//! The legacy bridge client
//! ([`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)) and
//! the IPC client also implement the trait so callers stay backend-agnostic.

use std::collections::HashMap;

use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

/// Per-signal weights for [`CognitiveMemoryOps::recall_facts_ranked`] (issue
/// #2329).
///
/// A backend-agnostic mirror of the library's `RecallWeights` (same six fields,
/// same order). It lives here — not in the adapter — so the trait never names a
/// library type and every implementor/mock stays backend-neutral. The
/// `RecallWeightSet -> amplihack_memory::RecallWeights` conversion is
/// adapter-local. The per-[`OodaPhase`](crate::ooda_loop::OodaPhase) mapping
/// lives in `ooda_loop::phase_weights` because only that layer knows about the
/// OODA phases; `cognitive_memory` must stay a leaf module.
///
/// Each field scales one scoring term; weights are un-normalized (only relative
/// magnitudes matter). [`Default`] is the library-balanced baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecallWeightSet {
    /// Weight on keyword overlap between the query and the fact text.
    pub text_relevance: f64,
    /// Weight on the fact's confidence.
    pub confidence: f64,
    /// Weight on the fact's importance/salience.
    pub importance: f64,
    /// Weight on exponential recency decay of the last access / creation time.
    pub recency: f64,
    /// Weight on the sub-linear usage boost.
    pub usage: f64,
    /// Weight on graph-neighbor proximity (e.g. `DERIVES_FROM` neighbours).
    pub graph: f64,
}

impl Default for RecallWeightSet {
    /// Library-balanced default: `1.0, 0.7, 0.5, 0.4, 0.3, 0.6`.
    fn default() -> Self {
        Self {
            text_relevance: 1.0,
            confidence: 0.7,
            importance: 0.5,
            recency: 0.4,
            usage: 0.3,
            graph: 0.6,
        }
    }
}

/// Trait abstracting cognitive memory operations.
///
/// Both [`LibraryCognitiveMemory`] (amplihack-memory-lib, lbug-backed) and
/// [`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)
/// (Python subprocess) implement this trait so callers are backend-agnostic.
pub trait CognitiveMemoryOps: Send + Sync {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String>;

    fn prune_expired_sensory(&self) -> SimardResult<usize>;

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String>;

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>>;

    fn clear_working(&self, task_id: &str) -> SimardResult<usize>;

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String>;

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>>;

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String>;

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>>;

    /// Ranked (scored) recall over semantic facts (issue #2329).
    ///
    /// Scores every candidate fact across six signals — text relevance,
    /// confidence, importance, recency, usage, and graph proximity — weighted by
    /// `weights`, and returns the facts in **descending score order** (the first
    /// element is the best match). Superseded/archived facts are excluded.
    /// Ordering *is* the ranking; no numeric score is surfaced on
    /// [`CognitiveFact`].
    ///
    /// `limit` and `min_confidence` mirror [`search_facts`](Self::search_facts).
    ///
    /// The default implementation delegates to
    /// [`search_facts`](Self::search_facts) (ignoring `weights`) so non-library
    /// backends (legacy Python bridge, IPC client, test mocks) keep working with
    /// confidence-ranked keyword recall. Only [`LibraryCognitiveMemory`]
    /// overrides this to call the library's ranked recall.
    fn recall_facts_ranked(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        _weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        self.search_facts(query, limit, min_confidence)
    }

    /// Store a fact under a stable `caller_key` so repeated logical records
    /// deduplicate instead of accumulating (issue #2329).
    ///
    /// For a given `caller_key` the backend keeps **at most one live fact**:
    /// identical content is **reused** (no duplicate node), changed content
    /// **supersedes** the prior live fact (old archived, `superseded_by` set, a
    /// typed `SUPERSEDES` edge new -> old). The remaining arguments mirror
    /// [`store_fact`](Self::store_fact); `caller_key` leads so call sites read
    /// `store_fact_with_caller_key(key, …)`.
    ///
    /// The default implementation ignores `caller_key` and delegates to
    /// [`store_fact`](Self::store_fact), so backends without caller-key dedup
    /// (legacy Python bridge, IPC client, test mocks) keep storing the fact
    /// (without dedup). Only [`LibraryCognitiveMemory`] performs the dedup.
    fn store_fact_with_caller_key(
        &self,
        _caller_key: &str,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        self.store_fact(concept, content, confidence, tags, source_id)
    }

    /// Prune superseded/archived facts, returning the number reclaimed
    /// (issue #2329).
    ///
    /// Reclaims the superseded tail produced by
    /// [`store_fact_with_caller_key`](Self::store_fact_with_caller_key) so the
    /// snapshot/goal-record pile-up does not grow unbounded. Provenance-bearing
    /// facts are protected by the backend.
    ///
    /// The default implementation is a no-op (`Ok(0)`) for backends without a
    /// retention pass; only [`LibraryCognitiveMemory`] reclaims.
    fn prune_superseded(&self) -> SimardResult<usize> {
        Ok(0)
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String>;

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>>;

    /// Returns `true` if a procedure with this **exact** `name` already exists.
    ///
    /// [`recall_procedure`](Self::recall_procedure) matches names with Cypher
    /// `CONTAINS`, so a name-shaped query can surface *other* procedures that
    /// merely share trigger tokens (`merge`, `bootstrap`, …) or are
    /// superstrings. An identity check — "does *this* procedure already
    /// exist?" — must therefore filter recall hits down to exact-name
    /// equality; a bare `is_empty()` would over-report presence. Centralizing
    /// the contract here keeps the bootstrap seeder and the OODA consolidation
    /// log in lockstep (issue #2298).
    ///
    /// The default implementation pays for that filter with an
    /// [`EXACT_NAME_RECALL_LIMIT`]-wide recall plus a linear exact-name scan,
    /// and decodes each hit's `steps`/`prerequisites` JSON only to discard it.
    /// Backends that can answer existence directly should override this with an
    /// exact-name probe that returns no payload and stops at the first match.
    fn procedure_exists(&self, name: &str) -> SimardResult<bool> {
        Ok(self
            .recall_procedure(name, EXACT_NAME_RECALL_LIMIT)?
            .iter()
            .any(|hit| hit.name == name))
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String>;

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>>;

    /// Mark a prospective memory as resolved so it no longer fires from
    /// `check_triggers`. Used when a goal is completed or paused.
    ///
    /// Default implementation is a no-op for backends that do not support
    /// status transitions (legacy Python bridge, test stubs).
    fn resolve_prospective(&self, _node_id: &str) -> SimardResult<()> {
        Ok(())
    }

    /// Mark an episode as distilled so subsequent distillation passes
    /// skip it. Default impl is a no-op for backends that do not
    /// support metadata mutation (legacy Python bridge, test stubs).
    /// Issue #2281, PR-B.
    fn mark_episode_distilled(&self, _node_id: &str) -> SimardResult<()> {
        Ok(())
    }

    /// Return up to `limit` undistilled episodes, newest first.
    ///
    /// Default impl returns empty, which makes the distillation pass a
    /// no-op for backends that do not track the `distilled` flag.
    /// [`LibraryCognitiveMemory`] overrides this to return the agent's
    /// not-yet-distilled episodes (newest-first), which is what the OODA
    /// distillation pass consumes. Issue #2281, PR-B; #2307.
    fn list_undistilled_episodes(&self, _limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![])
    }

    /// Return up to `limit` recent episodes whose `content` contains
    /// at least one of the supplied keywords (case-insensitive
    /// substring). Newest first.
    ///
    /// Default impl returns empty so legacy backends keep compiling.
    /// [`LibraryCognitiveMemory`] overrides this with a case-insensitive
    /// keyword-overlap scan ordered newest-first. Issue #2281, PR-C, problem 4;
    /// #2299.
    fn search_episodes_by_keywords(
        &self,
        _keywords: &[String],
        _limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![])
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics>;

    /// Store a semantic fact and record where it was distilled from.
    ///
    /// Identical to [`store_fact`](Self::store_fact) but additionally links the
    /// new fact to each id in `source_episode_ids` with a `DERIVES_FROM` edge,
    /// turning the flat fact store into a connected provenance graph (issue
    /// #2325). The resulting fact is recallable back to its episodes via
    /// [`episodes_for_fact`](Self::episodes_for_fact).
    ///
    /// Note the **library** argument order — `source_id` BEFORE `tags`, with
    /// `tags`/`metadata` as `Option`s — which differs from the legacy
    /// [`store_fact`](Self::store_fact) (tags before source_id). The returned
    /// id is the one to hand to [`episodes_for_fact`](Self::episodes_for_fact).
    ///
    /// Default impl drops the provenance (and metadata) and delegates to
    /// [`store_fact`](Self::store_fact) so non-graph backends (legacy Python
    /// bridge, IPC client, test stubs) keep compiling and still store the fact;
    /// only [`LibraryCognitiveMemory`] records the edges. Mirrors the
    /// `mark_episode_distilled` / `list_undistilled_episodes` extension pattern.
    #[allow(clippy::too_many_arguments)]
    fn store_fact_with_provenance(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Option<&[String]>,
        _metadata: Option<&HashMap<String, serde_json::Value>>,
        _source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.store_fact(concept, content, confidence, tags.unwrap_or(&[]), source_id)
    }

    /// Store a procedure and record which episodes it was distilled from.
    ///
    /// Identical to [`store_procedure`](Self::store_procedure) — including the
    /// idempotent upsert-by-name that reinforces `usage_count` (#2298) — but
    /// additionally links the procedure to each id in `source_episode_ids` with
    /// a `PROCEDURE_DERIVES_FROM` edge (issue #2325).
    ///
    /// Default impl drops the provenance and delegates to
    /// [`store_procedure`](Self::store_procedure); only
    /// [`LibraryCognitiveMemory`] records the edges.
    fn store_procedure_with_provenance(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
        _source_episode_ids: &[String],
    ) -> SimardResult<String> {
        self.store_procedure(name, steps, prerequisites)
    }

    /// Return the ids of the episodes a fact was distilled from (its
    /// `DERIVES_FROM` provenance edges), the read side of
    /// [`store_fact_with_provenance`](Self::store_fact_with_provenance).
    ///
    /// `fact_id` is the id returned by
    /// [`store_fact_with_provenance`](Self::store_fact_with_provenance) (or the
    /// `node_id` of a [`CognitiveFact`](crate::memory_cognitive::CognitiveFact)
    /// from [`search_facts`](Self::search_facts)). An unknown id, or a fact with
    /// no recorded provenance, yields an empty vector rather than an error —
    /// callers tolerate facts that predate provenance wiring.
    ///
    /// Default impl returns empty so backends without a provenance graph
    /// (legacy Python bridge, IPC client, test stubs) keep compiling;
    /// [`LibraryCognitiveMemory`] overrides it to traverse the graph.
    fn episodes_for_fact(&self, _fact_id: &str) -> SimardResult<Vec<String>> {
        Ok(vec![])
    }

    /// Search recent episodes by content prefix.
    ///
    /// Returns `(content, recorded_at)` pairs for episodes whose
    /// `content` starts with `prefix`, ordered most-recent first, capped
    /// at `limit`. Used by the progress-evidence gate
    /// (`update_goal_progress_with_evidence`) to source the `since`
    /// timestamp for goals that have no
    /// `ActiveGoal.last_progress_update_at` field set yet (legacy
    /// on-disk boards from before #1967).
    ///
    /// Default impl returns `Ok(vec![])` — backends without temporal
    /// metadata simply force callers into the next fallback step (the
    /// daemon's process-start timestamp), which is safe.
    fn search_episodes_starting_with(
        &self,
        _prefix: &str,
        _limit: u32,
    ) -> SimardResult<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        Ok(vec![])
    }

    /// Reports whether this backend was opened in read-only mode.
    ///
    /// Defaulted to `false` because the overwhelming majority of
    /// implementations are writers (the IPC client, the daemon's
    /// in-process Arc, the live [`LibraryCognitiveMemory`]).
    ///
    /// `WriterBridge` constructors assert that this is `false` so a
    /// read-only handle cannot be silently wrapped as a writer — the
    /// "hollow success" failure mode that issue #1590's follow-up
    /// targets.
    fn is_read_only(&self) -> bool {
        false
    }

    /// Force a WAL checkpoint, collapsing the WAL into the main DB file.
    ///
    /// Defaults to a no-op for backends where this is not meaningful
    /// (IPC client, bridge clients). Overridden by [`LibraryCognitiveMemory`]
    /// to flush the library's lbug-backed store via `close`.
    ///
    /// Call this **before** taking a backup or shutting down the host
    /// process so committed-but-WAL-resident writes are captured (issue #1631).
    fn checkpoint(&self) -> SimardResult<()> {
        Ok(())
    }
}

/// Recall fan-out for the default [`CognitiveMemoryOps::procedure_exists`].
/// [`CognitiveMemoryOps::recall_procedure`] ranks by a `CONTAINS` match on the
/// procedure name, so an exact-name lookup may have to look past several
/// superstring / trigger-sharing hits before it finds (or rules out) the exact
/// one. 16 clears the bootstrap set plus a realistic cycle's worth of
/// trigger-token collisions. Backends that override `procedure_exists` with a
/// direct exact-name probe do not pay this fan-out.
const EXACT_NAME_RECALL_LIMIT: u32 = 16;
pub mod metrics;

// De-fork Phase 2b (issue #2307): the library-backed `CognitiveMemoryOps`
// adapter is now the sole cognitive-memory backend. Re-exported at the
// module root so callers reference `cognitive_memory::LibraryCognitiveMemory`.
mod library_adapter;
pub use library_adapter::LibraryCognitiveMemory;

// PR-C (issue #2281): bootstrap procedural-memory seeding. Three
// baseline procedures (`pr-merge:bootstrap`, `ci-fix:bootstrap`,
// `run-tests:bootstrap`) are seeded into procedural memory on
// daemon boot so `recall_procedure` returns ≥1 hit for common
// engineer-loop objectives from the very first cycle.
pub mod bootstrap_procedures;

// De-fork Phase 2b (issue #2307): conformance tests that drive the
// cognitive-memory scenarios against the library-backed
// `LibraryCognitiveMemory` adapter (the sole backend).
#[cfg(test)]
mod tests_library_parity;

// PR-C (issue #2281): tests for `bootstrap_procedures::seed_bootstrap_procedures`
// — idempotency, error propagation, and the three required procedure
// names with their `| triggers:` suffixes.
#[cfg(test)]
mod bootstrap_procedures_tests;

// PR-B (issue #2281) + de-fork Phase 2b (#2307): episode-distillation
// trait-method tests against the library backend. Verify that
// `mark_episode_distilled` and `list_undistilled_episodes` round-trip
// through `LibraryCognitiveMemory`.
#[cfg(test)]
mod tests_pr_b_distill;

// Issue #2298: procedural-memory non-idempotency regression. `store_procedure`
// must be an idempotent upsert keyed on exact `name` so repeated OODA
// consolidation cycles stop re-storing identical procedures.
#[cfg(test)]
mod tests_pr_2298_idempotency;

// Issues #2299 / #2300: re-validate episodic recall (`search_episodes_by_keywords`)
// and prospective triggers (`check_triggers`) against the library backend after
// the de-fork (#2308) deleted the native fork where the original fixes lived.
// Guards "0 raw episodes" (#2299) and "0 triggers" (#2300) regressions.
#[cfg(test)]
mod tests_pr_2299_2300_recall_triggers;

// Issue #2325: fact/procedure provenance. Pins the round-trip contract
// for `store_fact_with_provenance` / `store_procedure_with_provenance` /
// `episodes_for_fact` against `LibraryCognitiveMemory` — a fact stored
// with a source episode must be recallable back to that episode
// (DERIVES_FROM edge), while base `store_fact`/`store_procedure`
// behaviour (searchability, `FACT_SEQ_META_KEY` stamping, idempotent
// procedure upsert) is preserved.
#[cfg(test)]
mod tests_provenance;

// Issue #2329: ranked fact recall (phase-weighted) + snapshot retention/dedup.
// Pins the new `recall_facts_ranked` (descending score order, default delegates
// to `search_facts`), `store_fact_with_caller_key` (CallerKey reuse/supersede,
// single live record), and `prune_superseded` (reclaims the superseded tail)
// contracts against `LibraryCognitiveMemory`.
#[cfg(test)]
mod tests_ranked_recall;
