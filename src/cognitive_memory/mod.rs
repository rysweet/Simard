//! Cognitive memory: the [`CognitiveMemoryOps`] trait and its sole backend.
//!
//! De-fork Phase 2b (issue #2307): Simard's native LadybugDB fork has been
//! deleted. The [`CognitiveMemoryOps`] trait defines the backend-agnostic API;
//! the only implementation is [`LibraryCognitiveMemory`], which delegates to the
//! upstream `amplihack-memory-lib` `CognitiveMemory` (persistent, lbug-backed).
//! The legacy bridge client
//! ([`CognitiveMemoryBridge`](crate::memory_bridge::CognitiveMemoryBridge)) and
//! the IPC client also implement the trait so callers stay backend-agnostic.

use crate::error::SimardResult;
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

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
