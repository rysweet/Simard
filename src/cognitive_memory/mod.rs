//! Cognitive memory: the [`CognitiveMemoryOps`] trait and its sole backend.
//!
//! De-fork Phase 2b (issue #2307): Simard's native LadybugDB fork has been
//! deleted. The [`CognitiveMemoryOps`] trait defines the backend-agnostic API;
//! the only implementation is [`LibraryCognitiveMemory`], which delegates to the
//! upstream `amplihack-memory-lib` `CognitiveMemory` (persistent, lbug-backed).
//! The legacy bridge client
//! ([`CognitiveMemoryClient`](crate::memory_client::CognitiveMemoryClient)) and
//! the IPC client also implement the trait so callers stay backend-agnostic.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

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

/// Which cognitive-memory node kind a [`CognitiveMemoryOps::reinforce_access`]
/// call targets (issue #2395).
///
/// The recall paths surface `node_id`s in slightly different shapes
/// (`CognitiveFact` ids carry the adapter's monotonic sequence prefix, whereas
/// `CognitiveEpisode` / `CognitiveProcedure` ids are the raw library ids), so
/// the backend needs to know the kind to normalize the id before recording the
/// access. Kept backend-neutral (no library type named) so every implementor /
/// mock stays leaf-module-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    /// A semantic fact (`CognitiveFact`).
    Fact,
    /// An episodic memory (`CognitiveEpisode`).
    Episode,
    /// A procedural memory (`CognitiveProcedure`).
    Procedure,
}

/// Outcome of a **fail-closed** emptiness probe over a cognitive store (issue
/// #2561).
///
/// This deliberately has only two variants: a store that was *read
/// successfully* is either [`ConfirmedEmpty`](StoreEmptiness::ConfirmedEmpty)
/// or [`NonEmpty`](StoreEmptiness::NonEmpty). A read *failure* is **never**
/// represented here — it is surfaced as `Err` by
/// [`CognitiveMemoryOps::probe_emptiness`] so the auto-restore gate
/// ([`crate::memory_snapshot::auto_restore_if_empty`]) can fail closed instead
/// of mistaking an unreadable store for an empty one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreEmptiness {
    /// The store was read successfully and holds zero memories across every
    /// cognitive type. Safe to hydrate from a snapshot.
    ConfirmedEmpty,
    /// The store was read successfully and holds at least one memory. The
    /// auto-restore gate must NOT hydrate — re-importing a snapshot would
    /// duplicate memories.
    NonEmpty,
}

/// Forgetting threshold for [`CognitiveMemoryOps::forget_low_value_facts`]
/// (issue #2434): live facts whose value falls below this fade during the
/// controlled-forgetting hygiene pass.
///
/// Conservative but non-zero — a `0.0` threshold (the pre-#2434 `prune_superseded`
/// no-op) never forgets a live fact, so semantic memory grew monotonically; a
/// value this small only catches genuinely low-value facts while leaving the
/// long tail of useful knowledge untouched. The actual deletion is doubly
/// guarded (provenance-bearing and above-threshold facts are never in the delete
/// set, dry-run preview first), so the precise value is safe within a wide band.
pub const FORGET_MIN_IMPORTANCE: f64 = 0.1;

/// Outcome of one [`CognitiveMemoryOps::forget_low_value_facts`] pass (issue
/// #2434), used to gate the controlled-forgetting safety self-metric.
///
/// `archived + deleted` is the number of live facts actually forgotten this
/// pass; `candidates` is how many qualified (it equals `archived + deleted` on a
/// live run and is the previewed count on a `dry_run`). `live_before` /
/// `live_after` snapshot the live `Fact` count so a regression (valuable-fact
/// loss) is visible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ForgetReport {
    /// `true` when this was a preview pass that changed nothing.
    pub dry_run: bool,
    /// Live (non-archived) fact count before the pass.
    pub live_before: usize,
    /// Live (non-archived) fact count after the pass (== `live_before` for a
    /// dry run).
    pub live_after: usize,
    /// Number of low-value, unprotected facts identified as forgettable.
    pub candidates: usize,
    /// Facts archived (soft-forgotten) this pass. `0` on a dry run.
    pub archived: usize,
    /// Facts deleted (hard-forgotten) this pass. `0` on a dry run.
    pub deleted: usize,
}

/// Pure forgetting signal for a live fact (issue #2440 / #2434): a bounded
/// `[0.0, 1.0]` score where a **higher** value means **more forgettable**.
///
/// The single source of truth for "low value" in the controlled-forgetting
/// hygiene pass ([`CognitiveMemoryOps::forget_low_value_facts`]): a fact is a
/// forgetting candidate only when its score clears the floor a never-accessed
/// fact at [`FORGET_MIN_IMPORTANCE`] would score. Because it blends recency and
/// usage — not just confidence — a low-confidence fact that was recently
/// recalled or is frequently used (reinforced via issue #2440) scores *below*
/// the floor and is protected, closing the recall→forgetting signal loop. It is
/// the complement of a retention score blended from the three signals the
/// Generative-Agents retrieval model uses, mirrored from local fact metadata (no
/// LLM call):
///
/// - **importance** ≈ `confidence` (already `[0,1]`),
/// - **recency** — exponential decay of the time since `last_accessed_at` with a
///   7-day half-life (matching the ranked-recall recency term); a never-accessed
///   fact (`None`) contributes no recency,
/// - **usage** — a sub-linear boost from `usage_count` (`1 - 1/(1+n)`), bounded
///   in `[0,1)`.
///
/// `forgetting = 1 - (w_imp·confidence + w_rec·recency + w_use·usage)`, with
/// weights summing to 1 so the result stays bounded. A stale, low-confidence,
/// never-used fact scores near `1.0` (very forgettable); a fresh, high-confidence,
/// frequently-used fact scores near `0.0` (protect).
pub fn forgetting_score(
    confidence: f64,
    usage_count: i64,
    last_accessed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> f64 {
    // Importance proxy — clamp defensively in case a backend surfaces an
    // out-of-range confidence.
    let importance = confidence.clamp(0.0, 1.0);

    // Recency: 0.5^(age_days / 7) ∈ (0, 1]. A never-accessed fact has no recency
    // signal, so it contributes 0 (maximally forgettable on this axis).
    const RECENCY_HALF_LIFE_DAYS: f64 = 7.0;
    let recency = match last_accessed_at {
        Some(ts) => {
            let age_days = (now - ts).num_seconds().max(0) as f64 / 86_400.0;
            0.5_f64.powf(age_days / RECENCY_HALF_LIFE_DAYS)
        }
        None => 0.0,
    };

    // Usage: sub-linear, saturating boost in [0, 1). 0 -> 0, 1 -> 0.5, 25 -> ~0.96.
    let usage = {
        let n = usage_count.max(0) as f64;
        1.0 - 1.0 / (1.0 + n)
    };

    // Retention is a convex blend (weights sum to 1) so forgetting stays in
    // [0, 1] without an explicit clamp; importance leads, then recency, then
    // usage — the same precedence ranked recall and the forgetting hygiene pass
    // rely on.
    const W_IMPORTANCE: f64 = 0.5;
    const W_RECENCY: f64 = 0.3;
    const W_USAGE: f64 = 0.2;
    let retention = W_IMPORTANCE * importance + W_RECENCY * recency + W_USAGE * usage;
    (1.0 - retention).clamp(0.0, 1.0)
}

/// Trait abstracting cognitive memory operations.
///
/// Both [`LibraryCognitiveMemory`] (amplihack-memory-lib, lbug-backed) and
/// [`CognitiveMemoryClient`](crate::memory_client::CognitiveMemoryClient)
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

    /// Reinforcing ranked recall (issue #2440, A3 / AC#2): score like
    /// [`recall_facts_ranked`](Self::recall_facts_ranked) and, after scoring,
    /// reinforce (`reinforce_access`) ONLY the returned top-k, so a **direct**
    /// recall-intent caller that consumes the returned set as-is feeds the
    /// `usage` / `recency` signals on later cycles in a single call.
    ///
    /// This is the single-call recall+reinforce convenience for such direct
    /// callers. It is deliberately NOT used by the OODA reasoning / context-prep
    /// path: that path gathers candidates with the pure
    /// [`recall_facts_ranked`](Self::recall_facts_ranked), filters / dedups / caps
    /// them into a [`PreparedContext`](crate::memory_consolidation::PreparedContext),
    /// then reinforces only the *surviving* nodes at the point of use via
    /// [`reinforce_prepared_context`](crate::memory_consolidation::reinforce_prepared_context)
    /// — so usage/recency reflect what actually reached reasoning, not every raw
    /// hit, and the two reinforcement seams never double-count. The pure
    /// [`recall_facts_ranked`](Self::recall_facts_ranked) likewise stays
    /// non-reinforcing for structural / index reads that must not inflate usage.
    ///
    /// No production call site is wired to this method yet; it is staged #2440
    /// (A3 / AC#2) API surface. The [`LibraryCognitiveMemory`] override exists so
    /// that when a direct recall-intent caller IS wired, scoring and the per-fact
    /// reinforcement happen under a single write-lock acquisition instead of the
    /// default's `1 + N`. Reinforcement is best-effort per fact — it never changes
    /// the returned set, only the persisted access signal.
    ///
    /// The default implementation works for every backend: it delegates scoring
    /// to [`recall_facts_ranked`](Self::recall_facts_ranked) and bumps each hit
    /// via [`reinforce_access`](Self::reinforce_access) (a no-op on backends
    /// without access tracking), so only [`LibraryCognitiveMemory`] actually
    /// persists the reinforcement.
    fn recall_facts_ranked_reinforced(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let facts = self.recall_facts_ranked(query, limit, min_confidence, weights)?;
        for fact in &facts {
            // Best-effort, per the contract above: a failed usage/recency bump
            // must NEVER turn a successful recall into an error or drop the
            // returned set. The bump can fail benignly — e.g. a concurrent
            // `forget_low_value_facts` / consolidation pass deletes a
            // just-recalled fact in the window between scoring and reinforcement,
            // and the backend reports the now-missing node as a storage error.
            // Log and continue; the recalled facts are still valid to return.
            if let Err(e) = self.reinforce_access(&fact.node_id, MemoryKind::Fact) {
                tracing::debug!(
                    target: "simard::memory",
                    node_id = %fact.node_id,
                    error = %e,
                    "recall_facts_ranked_reinforced: reinforce_access failed (non-fatal, recall unaffected)"
                );
            }
        }
        Ok(facts)
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

    /// Controlled forgetting of *live* low-value facts (issue #2434): a bounded,
    /// safe hygiene pass that lets genuinely low-value facts fade while
    /// protecting valuable knowledge, complementing
    /// [`prune_superseded`](Self::prune_superseded) (which only reclaims the
    /// superseded tail).
    ///
    /// A fact is a forgetting *candidate* only when it is both **low value** —
    /// its [`forgetting_score`] clears the floor a never-accessed fact at
    /// [`FORGET_MIN_IMPORTANCE`] scores, so confidence, recency, and usage all
    /// count — and **unprotected**, carrying no provenance (`DERIVES_FROM`) edge.
    /// Provenance-bearing facts are NEVER in the delete set, and a low-confidence
    /// fact kept warm by recall (issue #2440 reinforcement) scores below the
    /// floor and survives. Mandatory safety (issue #2434): the candidate set is
    /// computed first (a `dry_run` returns it as a pure preview that changes
    /// nothing), and a live run only deletes when candidates exist, snapshotting
    /// the live `Fact` count before/after via a self-metric so valuable-fact loss
    /// is visible.
    ///
    /// The default implementation is a safe no-op (`Ok(ForgetReport::default())`)
    /// for backends without a retention pass (legacy bridge, IPC client, test
    /// stubs); only [`LibraryCognitiveMemory`] forgets.
    fn forget_low_value_facts(&self, _dry_run: bool) -> SimardResult<ForgetReport> {
        Ok(ForgetReport::default())
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

    /// Report whether an episode with `node_id` exists in this store (issue
    /// #2679).
    ///
    /// This is the **grounding** primitive for the distillation write-boundary
    /// gate: when the distiller agent commits a fact through the memory IPC
    /// socket, the server holds no in-memory batch, so it grounds the fact by an
    /// existence lookup — the fact is grounded iff at least one of its cited
    /// `source_episode_ids` resolves to a real episode node here.
    ///
    /// Default impl returns `false` (fail-closed: an unresolvable id is treated
    /// as ungrounded) so non-graph backends (legacy Python bridge, IPC client,
    /// test stubs) keep compiling; only [`LibraryCognitiveMemory`] overrides it
    /// to look the episode up in the store.
    fn episode_exists(&self, _node_id: &str) -> SimardResult<bool> {
        Ok(false)
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

    /// Ranked (scored) recall over episodic memory (issue #2395).
    ///
    /// The episodic counterpart of [`recall_facts_ranked`](Self::recall_facts_ranked):
    /// scores keyword-relevant episodes across the ranked signals (text
    /// relevance, recency, usage, graph proximity — confidence/importance are
    /// facts-only) weighted by `weights`, returning them in **descending score
    /// order**. This is a **pure read** — it never reinforces usage/recency, so
    /// the several recalls a single OODA cycle issues cannot skew one another;
    /// reinforcement is the explicit, separate [`reinforce_access`](Self::reinforce_access)
    /// seam applied at the point of use.
    ///
    /// The default implementation splits `query` on whitespace into keywords and
    /// delegates to [`search_episodes_by_keywords`](Self::search_episodes_by_keywords)
    /// (ignoring `weights`), so non-library backends (legacy Python bridge, IPC
    /// client, test mocks) keep working with newest-first keyword recall. Only
    /// [`LibraryCognitiveMemory`] overrides this to call the library's ranked
    /// recall (relevance-gated, with a UNION backfill that keeps compressed
    /// consolidation sources recallable).
    fn recall_episodes_ranked(
        &self,
        query: &str,
        limit: u32,
        _weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        let keywords: Vec<String> = query.split_whitespace().map(str::to_string).collect();
        self.search_episodes_by_keywords(&keywords, limit)
    }

    /// Reinforce a recalled node: increment its `usage_count` and stamp
    /// `last_accessed_at` (issue #2395).
    ///
    /// This is the "reinforce-at-use" seam the ranked-recall `usage` / `recency`
    /// signals feed on. Preparation recall stays a pure read; the OODA loop
    /// calls this when a recalled fact / procedure / episode is actually
    /// surfaced into a cycle's working context (see
    /// [`crate::memory_consolidation::reinforce_prepared_context`]). `kind` tells
    /// the backend how to normalize `node_id` (fact ids carry the adapter's
    /// sequence prefix; episode / procedure ids are raw).
    ///
    /// The default implementation is a no-op (`Ok(())`) for backends without
    /// access tracking (legacy Python bridge, IPC client, test mocks); only
    /// [`LibraryCognitiveMemory`] records the access.
    fn reinforce_access(&self, _node_id: &str, _kind: MemoryKind) -> SimardResult<()> {
        Ok(())
    }

    /// Return up to `limit` episodes for this agent, newest-first, **including
    /// compressed/consolidated ones** (issue #2550).
    ///
    /// Unlike [`list_undistilled_episodes`](Self::list_undistilled_episodes)
    /// (distillation-gated) or
    /// [`search_episodes_by_keywords`](Self::search_episodes_by_keywords)
    /// (keyword-gated), this is an unfiltered enumeration used by the verified
    /// backup to capture **every** episode so a restore round-trips episodic
    /// memory. The default returns empty so non-library backends (IPC client,
    /// test stubs) degrade gracefully; only [`LibraryCognitiveMemory`] overrides
    /// it.
    fn list_all_episodes(&self, _limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        Ok(vec![])
    }

    /// Return up to `limit` prospective (trigger → action) memories for this
    /// agent, in **every** status, priority-ordered (issue #2550).
    ///
    /// A pure read: unlike [`check_triggers`](Self::check_triggers) it neither
    /// filters by content nor mutates any status, so it is safe on the backup
    /// path. The default returns empty so non-library backends degrade
    /// gracefully; only [`LibraryCognitiveMemory`] overrides it (via the
    /// library's `get_all_prospective`). This is the enumerator the verified
    /// backup needs so a restore round-trips prospective triggers — the memory
    /// type the incident lost with no way back.
    fn list_all_prospective(&self, _limit: u32) -> SimardResult<Vec<CognitiveProspective>> {
        Ok(vec![])
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics>;

    /// Probe whether the store is *confirmed* empty, **failing closed** on a
    /// read error (issue #2561).
    ///
    /// The auto-restore gate
    /// ([`crate::memory_snapshot::auto_restore_if_empty`]) hydrates a store from
    /// an on-disk snapshot only when it is genuinely empty (e.g. a fresh install
    /// or a corruption-reset self-heal, issue #2550). Deciding "empty" purely
    /// from a count is unsafe when the underlying read can *fail silently*: if a
    /// transient read error is coerced into an all-zeros count, the gate would
    /// re-import a snapshot on top of still-present-but-unreadable durable data
    /// and duplicate every memory once reads recover.
    ///
    /// This method therefore returns a `Result` so a surfaced read error is
    /// **propagated as `Err`**, never mapped to
    /// [`StoreEmptiness::ConfirmedEmpty`]. The default implementation derives the
    /// answer from [`get_statistics`](Self::get_statistics) — which already
    /// returns a `Result`, so any backend that surfaces its read/transport
    /// errors (the bridge and IPC clients, test mocks) fails closed for free, and
    /// this is the single seam a future error-propagating count plugs into.
    ///
    /// # Backend note
    ///
    /// The direct library backend ([`LibraryCognitiveMemory`]) cannot yet
    /// observe a read error that the pinned `amplihack-memory-lib` swallows
    /// internally (`Err(_) => Vec::new()`); closing that last gap needs the
    /// upstream library to propagate the error (issue #2561). Until then this
    /// seam guarantees the *decision path* is fail-closed for every backend that
    /// can surface the error and centralises where the fix lands.
    fn probe_emptiness(&self) -> SimardResult<StoreEmptiness> {
        let stats = self.get_statistics()?;
        Ok(if stats.total() == 0 {
            StoreEmptiness::ConfirmedEmpty
        } else {
            StoreEmptiness::NonEmpty
        })
    }

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

    /// Return edge / connection counts across the cognitive-memory graph
    /// (issue #2331): provenance edges (`DERIVES_FROM` fact→episode,
    /// `PROCEDURE_DERIVES_FROM` procedure→episode), `SIMILAR_TO` and
    /// `SUPERSEDES` edges, fact-provenance coverage, and snapshot-dedup
    /// grouping. This is the aggregate read side of
    /// [`store_fact_with_provenance`](Self::store_fact_with_provenance) /
    /// [`store_fact_with_caller_key`](Self::store_fact_with_caller_key) that
    /// powers the "edges / connections" section of `simard memory stats`.
    ///
    /// Default impl returns an all-zero [`GraphStats`] so backends without a
    /// provenance graph (IPC client, bridge clients, test stubs) keep
    /// compiling; only [`LibraryCognitiveMemory`](crate::cognitive_memory::LibraryCognitiveMemory)
    /// overrides it to traverse the graph. Read-only — never mutates the store.
    fn graph_stats(&self) -> SimardResult<GraphStats> {
        Ok(GraphStats::default())
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
    /// `WriterClient` constructors assert that this is `false` so a
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

// Issue #2491 / measurement issue #2494 (G1 hybrid measurement): the
// fixed-corpus recall-precision BENCHMARK rail. Scores a small, in-repo frozen
// corpus through the same precision@k primitive the live rail uses and persists
// one comparable `ScoreRecord{suite:"cognition", scenario:"recall_precision_at_k"}`
// so a claimed cognition improvement can be validated on a stable benchmark, not
// only observed live.
pub mod recall_precision_bench;

// Issue #2419 (design spike): the `CreativeIdea` prospective-memory type + its
// `IdeaStatus` state machine + `CreativeIdeaStore` round-trip seam. Additive;
// no schema change to prospective memory. Gated OFF at the subsystem level.
pub mod creative_idea;

// De-fork Phase 2b (issue #2307): the library-backed `CognitiveMemoryOps`
// adapter is now the sole cognitive-memory backend. Re-exported at the
// module root so callers reference `cognitive_memory::LibraryCognitiveMemory`.
mod library_adapter;
pub use library_adapter::LibraryCognitiveMemory;

// Issue #2420: migration-aware live-store path resolution. Re-exported at the
// module root so the verified-backup path (`memory_backup`) and the daemon both
// reference `cognitive_memory::live_store_path` — the single source of truth for
// "the path the daemon actually opens", so a verified backup can never again
// silently target a stale store (the Jun-20 backup regression).
pub use library_adapter::{LEGACY_STORE_FILE, LIVE_STORE_SUBDIR, live_store_path};

// Issue #2331: re-export the graph-edge / dedup stats DTO at the module root so
// callers reference `cognitive_memory::GraphStats` alongside the trait that
// returns it.
pub use crate::memory_cognitive::GraphStats;

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

// Issue #2420: migration-aware live-store path resolution. Pins that
// `live_store_path` resolves to the post-migration `cognitive` store the daemon
// actually opens (and falls back to the legacy single-file store only on an
// un-migrated host), so verified backups can never silently target a stale path.
#[cfg(test)]
mod tests_live_store_path_2420;

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

// Issue #2331: graph-edge / dedup stats. Pins the new `graph_stats` aggregate
// against `LibraryCognitiveMemory` — DERIVES_FROM / PROCEDURE_DERIVES_FROM edge
// counts, fact-provenance coverage, and snapshot-dedup caller-key grouping.
#[cfg(test)]
mod tests_graph_stats;

// Issue #2395: ranked episodic recall (UNION-backfilled so compressed
// consolidation sources stay recallable) and the `reinforce_access` usage/recency
// reinforcement seam. Pins descending recall order, compressed-source recovery,
// the default keyword-scan delegation, and fact/procedure reinforcement against
// `LibraryCognitiveMemory`.
#[cfg(test)]
mod tests_ranked_episodic;

// Issue #2440 (PR-2): ranked multi-signal recall + forgetting signal. Pins the
// pure `forgetting_score` helper (bounded, recency/usage/confidence ordering),
// the reinforcing `recall_facts_ranked_reinforced` recall+reinforce API (bumps
// the returned top-k; see its doc — staged API, not yet wired to a production
// caller), and usage-ordered `recall_procedure` against
// `LibraryCognitiveMemory`.
#[cfg(test)]
mod tests_recall_forgetting;

// Issue #2434 (PR-3): controlled forgetting of live facts. Pins the bounded,
// safe `forget_low_value_facts` hygiene pass (`ForgetReport`,
// `FORGET_MIN_IMPORTANCE`) — low-value facts fade, provenance-bearing /
// high-value facts are protected, dry-run is a pure preview — and its wiring
// into the consolidation cadence.
#[cfg(test)]
mod tests_controlled_forgetting;

// Issue #2441: close the episodic->procedural skill-reuse loop. End-to-end guards
// that reuse feeds back into recall ordering through the Simard `CognitiveMemoryOps`
// surface — a recalled+reinforced procedure ranks higher on a later recall ("close
// the loop, don't just store") — while a freshly distilled (`usage_count == 0`)
// procedure stays recallable for its first reuse. The distill/recall/reinforce
// halves are already wired; these pin that they compose (existing tests only cover
// each half in isolation).
#[cfg(test)]
mod tests_procedural_loop;

// Issue #2491 / measurement issue #2494 (G1 hybrid measurement, Step 7):
// the fixed-corpus recall-precision BENCHMARK rail — a deterministic
// `score_recall_precision_corpus()` and `run_recall_precision_bench()` that
// persists one comparable `ScoreRecord{suite:"cognition",
// scenario:"recall_precision_at_k"}` through the existing gym signal machinery.
#[cfg(test)]
mod tests_recall_precision_bench;

// Issue #2491 / #2494 (G2 de-fork, Step 7): pins that
// `metrics::precision_at_k` becomes a thin adapter delegating to the upstream
// `amplihack_memory::measurement` primitive (no scoring math forked into
// Simard), and that the move is behaviour-preserving (parity gate).
#[cfg(test)]
mod tests_recall_precision_delegation;
