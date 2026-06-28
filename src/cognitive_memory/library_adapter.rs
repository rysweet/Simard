//! Library-backed cognitive memory adapter — de-fork Phase 2b (issue #2307).
//!
//! [`LibraryCognitiveMemory`] implements Simard's
//! [`CognitiveMemoryOps`](super::CognitiveMemoryOps) trait by delegating to the
//! upstream `amplihack-memory-lib` [`CognitiveMemory`], opened with
//! `open_persistent` (the library's lbug-backed durable `GraphStore`). As of
//! Phase 2b it is the **sole** cognitive-memory backend: Simard's native
//! LadybugDB fork has been deleted and every code path that opened a backend
//! directly now opens this adapter.
//!
//! # Design decisions
//!
//! * **Interior mutability (A2).** The trait's methods take `&self` (and the
//!   trait is `Send + Sync`), but every mutating library method takes
//!   `&mut self`. The adapter therefore wraps the library memory in a
//!   [`std::sync::Mutex`] and locks per operation. A poisoned lock maps to
//!   [`SimardError::StoragePoisoned`].
//! * **Error mapping.** `open` failures map to
//!   [`SimardError::PersistentStoreIo`]; per-operation failures map to
//!   [`SimardError::BridgeCallFailed`] with `bridge = "cognitive-memory-library"`,
//!   preserving the upstream `MemoryError` message. No new `SimardError` variant
//!   is introduced — this keeps the change additive.
//! * **Documented divergences (A3/A6/A7).** `check_triggers`,
//!   `consolidate_episodes`, and `get_statistics` legitimately differ from the
//!   former native semantics. The adapter maps onto the library's high-level
//!   behavior; the divergences are documented here and in
//!   `docs/architecture/cognitive-memory-library-adapter.md`.
//! * **Episode distillation (A5).** The library exposes a persistent
//!   distilled-flag API (`mark_episode_distilled` / `list_undistilled_episodes`),
//!   so episode distillation runs natively against this backend — see those
//!   methods below. (Earlier phases degraded distillation to a no-op because the
//!   pinned library commit lacked the flag; that gap is closed.)
//!
//! All persistence is rooted at a caller-supplied `state_root` (a `TempDir` in
//! tests). The adapter opens its store at `state_root/cognitive`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use amplihack_memory::{
    AccessKind, CognitiveMemory, DedupMode, DedupOptions, EpisodicMemory, FactInput, MemoryError,
    ProceduralMemory, ProspectiveMemory, RecallOptions, RecallWeights, RetentionPolicy,
    SemanticFact, StoreFactOptions, WorkingMemorySlot,
};
use chrono::{DateTime, Utc};

use super::{
    CognitiveMemoryOps, FORGET_MIN_IMPORTANCE, ForgetReport, MemoryKind, RecallWeightSet,
    forgetting_score,
};
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot, GraphStats,
};

/// Agent name the persistent library store is scoped to. The library rejects an
/// empty name, and the same name must be used across reopens for data to round
/// trip, so this is a fixed, validated, non-empty constant.
const LIBRARY_AGENT_NAME: &str = "simard";

/// Identifier used in mapped [`SimardError`]s so failures are attributable to
/// the library backend.
const STORE_NAME: &str = "cognitive-memory-library";

/// Metadata key the adapter stamps on every fact with a per-store, process-wide
/// monotonic sequence number.
///
/// **Why.** Several Simard call sites (the goal-board snapshot in
/// `goal_curation::operations`, `goals::CognitiveMemoryGoalStore`, and
/// `memory_consolidation`) select "the most recent fact for concept X" by taking
/// the lexicographically-largest `node_id`. That works only when fact ids are
/// time-ordered. The deleted native backend used UUID-v7 ids (time-prefixed);
/// the library uses **random UUID-v4** ids and only second-granularity
/// `created_at`, so neither the id nor the timestamp reliably orders two facts
/// written within the same second. The adapter therefore stamps a monotonic
/// sequence into fact metadata at store time and folds it into the **front** of
/// the `node_id` it surfaces (`to_fact`), restoring the "max node_id == newest"
/// invariant those consumers depend on — without changing the `search_facts`
/// result ordering (which stays confidence-ranked for general recall).
const FACT_SEQ_META_KEY: &str = "_simard_seq";

/// Zero-padding width for the sequence prefix so lexical comparison of the
/// composite `node_id` matches numeric sequence order. 20 digits covers the full
/// `u64` range.
const FACT_SEQ_WIDTH: usize = 20;

/// Concept under which goal-board snapshots are stored (issue #2331).
///
/// The goal-board snapshot write path (`goal_curation::operations`) stores facts
/// under this concept via `store_fact_with_caller_key`, using the same string as
/// the caller key. [`LibraryCognitiveMemory::graph_stats`] groups facts on this
/// concept to surface the snapshot-dedup signal (many revisions collapsed onto a
/// few caller keys). Kept in sync with the literal in `goal_curation::operations`.
const SNAPSHOT_FACT_CONCEPT: &str = "goal-board:snapshot";

/// Sub-path under `state_root` where the **live** library-backed cognitive store
/// lives post-migration (lbug 0.17.x de-fork, issue #2307). This is the exact
/// path [`LibraryCognitiveMemory::open`] passes to `CognitiveMemory::open_persistent`,
/// and therefore the path the daemon actually reads and writes.
///
/// Pinned here as a named constant (issue #2420) so the verified-backup source
/// resolver ([`live_store_path`]) and the daemon's store open can never silently
/// drift to different paths again — the failure that broke verified backups from
/// Jun 20 onward (backups copied the stale legacy [`LEGACY_STORE_FILE`] while the
/// daemon served this `cognitive` store).
pub const LIVE_STORE_SUBDIR: &str = "cognitive";

/// Legacy single-file store name used by the **native fork** before the
/// de-fork migration (issue #2307). Retained only so [`live_store_path`] can
/// resolve a not-yet-migrated `state_root` (and so the backup never errors on a
/// legacy host). Never written by the current backend.
pub const LEGACY_STORE_FILE: &str = "cognitive_memory.ladybug";

/// Resolve the **live** cognitive-memory store path under `state_root`,
/// migration-aware (issue #2420).
///
/// Resolution order:
///   1. `state_root/`[`LIVE_STORE_SUBDIR`] — the post-migration library store the
///      daemon opens. Preferred whenever it exists.
///   2. `state_root/`[`LEGACY_STORE_FILE`] — the pre-migration native single-file
///      store. Only chosen when the live path is absent but the legacy file is
///      present (a host that has not migrated).
///   3. `state_root/`[`LIVE_STORE_SUBDIR`] — default for a fresh `state_root`
///      where neither exists yet, matching what [`LibraryCognitiveMemory::open`]
///      will create.
///
/// The verified backup uses this so its source is *always* the path the daemon
/// actually opens — asserted by a unit test so it cannot silently rot again.
pub fn live_store_path(state_root: &Path) -> PathBuf {
    let live = state_root.join(LIVE_STORE_SUBDIR);
    if live.exists() {
        return live;
    }
    // Only fall back to the legacy single-file store on a host that has not
    // migrated (live path absent, legacy file present). Never prefer the legacy
    // path over the live one — that preference is the exact bug being fixed.
    let legacy = state_root.join(LEGACY_STORE_FILE);
    if legacy.exists() {
        return legacy;
    }
    // Fresh `state_root`: default to the live path the daemon will create on
    // first open, never the legacy file.
    live
}

/// Cognitive memory backed by the upstream `amplihack-memory-lib`
/// [`CognitiveMemory`] (persistent, lbug-backed).
///
/// Implements [`CognitiveMemoryOps`] so callers are backend-agnostic. This is
/// the only cognitive-memory backend in Simard as of de-fork Phase 2b.
pub struct LibraryCognitiveMemory {
    /// The library memory, behind a `Mutex` for `&self` -> `&mut` interior
    /// mutability (see module docs, A2).
    inner: Mutex<CognitiveMemory>,
    /// Process-wide monotonic fact sequence (see [`FACT_SEQ_META_KEY`]). Seeded
    /// on open from the maximum sequence already persisted so it keeps advancing
    /// across reopens.
    fact_seq: AtomicU64,
    /// The `state_root` this handle was opened against (`None` for the
    /// in-memory test constructor). Used **only** by the `cfg(test)`
    /// hermetic-state-root guard in [`Self::lock_write`], which preserves the
    /// safety property the deleted native backend enforced in every mutating
    /// op: cargo-test must never write into the operator's live cognitive
    /// memory under `$HOME/.simard` (issues #1923 / #1925).
    #[cfg_attr(not(test), allow(dead_code))]
    state_root: Option<std::path::PathBuf>,
}

impl LibraryCognitiveMemory {
    /// Open (or create) a persistent library-backed cognitive memory under
    /// `state_root`.
    ///
    /// The store lives at a dedicated sub-path (`state_root/cognitive`) so it is
    /// isolated from anything else under `state_root`. In tests `state_root` is a
    /// `TempDir`.
    ///
    /// # Errors
    ///
    /// Returns [`SimardError::PersistentStoreIo`] if the underlying LadybugDB
    /// store cannot be opened.
    pub fn open(state_root: &Path) -> SimardResult<Self> {
        // Use the shared `LIVE_STORE_SUBDIR` constant (not a bare literal) so the
        // path the daemon opens and the verified-backup resolver `live_store_path`
        // are anchored to one source of truth and cannot silently drift (#2420).
        let db_path = state_root.join(LIVE_STORE_SUBDIR);
        let inner =
            CognitiveMemory::open_persistent(&db_path, LIBRARY_AGENT_NAME).map_err(|e| {
                SimardError::PersistentStoreIo {
                    store: STORE_NAME.to_string(),
                    action: "open_persistent".to_string(),
                    path: db_path,
                    reason: e.to_string(),
                }
            })?;
        let fact_seq = AtomicU64::new(recover_fact_seq(&inner));
        Ok(Self {
            inner: Mutex::new(inner),
            fact_seq,
            state_root: Some(state_root.to_path_buf()),
        })
    }

    /// Create a non-persistent, in-memory library-backed cognitive memory for
    /// tests.
    ///
    /// Backed by the library's `InMemoryGraphStore`; nothing is written to disk
    /// and nothing survives the process. This is the replacement for the deleted
    /// native in-memory test constructor — the full
    /// [`CognitiveMemoryOps`] surface (including episode distillation) behaves
    /// identically to the persistent backend, only durability differs.
    ///
    /// # Errors
    ///
    /// Returns [`SimardError::PersistentStoreIo`] if the in-memory store cannot
    /// be constructed (only possible on an invalid agent name, which is a fixed
    /// non-empty constant here).
    pub fn in_memory() -> SimardResult<Self> {
        let inner = CognitiveMemory::new(LIBRARY_AGENT_NAME).map_err(|e| {
            SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "new_in_memory".to_string(),
                path: std::path::PathBuf::from("<in-memory>"),
                reason: e.to_string(),
            }
        })?;
        Ok(Self {
            inner: Mutex::new(inner),
            fact_seq: AtomicU64::new(0),
            state_root: None,
        })
    }

    /// Lock the inner library memory, mapping a poisoned lock to a loud error
    /// rather than panicking.
    fn lock(&self) -> SimardResult<MutexGuard<'_, CognitiveMemory>> {
        self.inner.lock().map_err(|_| SimardError::StoragePoisoned {
            store: STORE_NAME.to_string(),
        })
    }

    /// Lock for a **mutating** op, first running the `cfg(test)`-only
    /// hermetic-state-root guard so a cargo-test write can never land in the
    /// operator's live `$HOME/.simard` store. This is the adapter's
    /// reimplementation of the per-write guard the deleted native backend ran;
    /// it keeps the documented multi-site contract intact (`launch_writer_bridge`
    /// remains the other site). No-op for the in-memory constructor (no
    /// `state_root`) and compiled out of release builds. See
    /// `docs/testing/hermetic-tests.md`.
    fn lock_write(&self, _site: &'static str) -> SimardResult<MutexGuard<'_, CognitiveMemory>> {
        #[cfg(test)]
        if let Some(root) = &self.state_root {
            crate::test_support::hermetic_guard::assert_state_root_isolated(root, _site);
        }
        self.lock()
    }

    /// Store a procedure under Simard's idempotent *upsert-that-reinforces*
    /// contract (`docs/reference/cognitive-memory-procedural-idempotency.md`,
    /// #2298) and return its id.
    ///
    /// The library upserts by exact name — re-storing the same name keeps a
    /// single canonical node — but does NOT bump `usage_count` on update (it
    /// reinforces only on a mutating recall). So detect the duplicate by exact
    /// name (avoiding the keyword matcher's superstring hits) and reinforce
    /// after the store. `store` performs the actual library write — plain or
    /// provenance-recording — so [`store_procedure`](CognitiveMemoryOps::store_procedure)
    /// and
    /// [`store_procedure_with_provenance`](CognitiveMemoryOps::store_procedure_with_provenance)
    /// share this subtle contract instead of duplicating it.
    fn store_procedure_reinforcing(
        &self,
        site: &'static str,
        name: &str,
        store: impl FnOnce(&mut CognitiveMemory) -> Result<String, MemoryError>,
    ) -> SimardResult<String> {
        let mut guard = self.lock_write(site)?;
        let existed = guard
            .search_procedures(name, usize::MAX)
            .iter()
            .any(|p| p.name == name);
        let id = store(&mut guard).map_err(|e| map_op_err(site, e))?;
        if existed {
            // `recall_procedure` (mutating) increments the matched procedure's
            // persisted `usage_count` by one — the reinforcement signal.
            let _ = guard.recall_procedure(name, usize::MAX);
        }
        Ok(id)
    }
}

/// Map an upstream [`MemoryError`] from a delegated call onto a Simard error,
/// preserving the upstream message.
fn map_op_err(method: &str, err: MemoryError) -> SimardError {
    SimardError::BridgeCallFailed {
        bridge: STORE_NAME.to_string(),
        method: method.to_string(),
        reason: err.to_string(),
    }
}

/// Record the controlled-forgetting (issue #2434) before/after self-metric.
///
/// Emits `controlled_forgetting` to `metrics.jsonl` with the live `Fact` count
/// before/after the pass plus candidate / archived / deleted counts, so a
/// regression (valuable-fact loss) is visible. `value` is the *net* number of
/// live facts removed (`live_before - live_after`). Best-effort: a metrics-write
/// failure is logged, never propagated. No-op under `cfg!(test)` so unit tests
/// never append to the operator's real `~/.simard/metrics/metrics.jsonl`.
fn record_forget_metric(
    live_before: usize,
    live_after: usize,
    candidates: usize,
    archived: usize,
    deleted: usize,
) {
    if cfg!(test) {
        return;
    }
    let value = live_before.saturating_sub(live_after) as f64;
    let context = serde_json::json!({
        "live_before": live_before,
        "live_after": live_after,
        "candidates": candidates,
        "archived": archived,
        "deleted": deleted,
    })
    .to_string();
    if let Err(e) = crate::self_metrics::record_metric("controlled_forgetting", value, &context) {
        tracing::warn!(
            target: "simard::memory",
            error = %e,
            "failed to record controlled_forgetting metric (forgetting unaffected)",
        );
    }
}

/// Live low-value facts selected for controlled forgetting (issue #2434), with
/// the per-concept targeting [`forget_low_value_facts`] drives the library
/// retention pass with.
struct ForgetCandidates {
    /// Live (non-archived) fact count when the candidate set was computed.
    live_before: usize,
    /// Node ids of the live facts that qualify for forgetting.
    candidate_ids: HashSet<String>,
    /// Concepts every live member of which is a candidate (the only concepts
    /// safe to target with a per-concept TTL — see [`forget_low_value_facts`]).
    forgettable_concepts: HashSet<String>,
}

/// Identify the live facts safe to forget (issue #2434), keyed off the shared
/// [`forgetting_score`] signal so there is a single source of truth for "low
/// value" across ranked recall and the hygiene pass (design A2).
///
/// A live fact is *forgettable* when it carries NO provenance edge AND its
/// `forgetting_score` exceeds the floor a never-used fact sitting exactly at the
/// importance threshold ([`FORGET_MIN_IMPORTANCE`]) would score. Because the
/// score blends confidence, recency, and usage, a low-confidence fact that has
/// been recently recalled or is frequently used (reinforced via issue #2440)
/// scores *below* the floor and is protected — completing the recall→forgetting
/// signal loop a bare confidence threshold would miss.
///
/// Only *purely forgettable* concepts (every live member is a candidate) are
/// targeted: the library's retention pass is concept-granular, so targeting a
/// mixed concept would archive — then, lacking provenance, delete — a high-value
/// fact that merely shares the concept. Requiring purity keeps such a fact off
/// the delete path entirely.
fn collect_forget_candidates(mem: &CognitiveMemory, now: DateTime<Utc>) -> ForgetCandidates {
    // The floor a never-accessed fact at the importance threshold scores. A
    // strict `>` comparison preserves the `confidence < FORGET_MIN_IMPORTANCE`
    // boundary for fresh facts while letting recency/usage protect reinforced
    // ones.
    let floor_score = forgetting_score(FORGET_MIN_IMPORTANCE, 0, None, now);
    let is_forgettable = |f: &SemanticFact| {
        forgetting_score(f.confidence, f.usage_count, f.last_accessed_at, now) > floor_score
            && mem.fact_provenance(&f.node_id).is_empty()
    };

    // `get_all_facts` includes archived facts; only live ones are forgettable.
    let all = mem.get_all_facts(usize::MAX);
    let live_before = all.iter().filter(|f| !f.archived).count();

    let mut by_concept: HashMap<&str, Vec<&SemanticFact>> = HashMap::new();
    for f in all.iter().filter(|f| !f.archived) {
        by_concept.entry(f.concept.as_str()).or_default().push(f);
    }

    let mut forgettable_concepts = HashSet::new();
    let mut candidate_ids = HashSet::new();
    for (concept, facts) in &by_concept {
        if facts.iter().all(|f| is_forgettable(f)) {
            forgettable_concepts.insert((*concept).to_string());
            candidate_ids.extend(facts.iter().map(|f| f.node_id.clone()));
        }
    }

    ForgetCandidates {
        live_before,
        candidate_ids,
        forgettable_concepts,
    }
}

/// Net effect of a forgetting pass, measured against the live store rather than
/// the library's coarse policy counts (issue #2434), so the self-metric reflects
/// ground truth.
struct ForgetOutcome {
    archived: usize,
    deleted: usize,
    live_after: usize,
}

/// Measure how many of `candidate_ids` were archived vs. deleted, plus the live
/// fact count, by re-reading the store after the retention pass.
fn measure_forget_outcome(mem: &CognitiveMemory, candidate_ids: &HashSet<String>) -> ForgetOutcome {
    let after = mem.get_all_facts(usize::MAX);
    let present: HashSet<&str> = after.iter().map(|f| f.node_id.as_str()).collect();
    let archived_present: HashSet<&str> = after
        .iter()
        .filter(|f| f.archived)
        .map(|f| f.node_id.as_str())
        .collect();
    ForgetOutcome {
        deleted: candidate_ids
            .iter()
            .filter(|id| !present.contains(id.as_str()))
            .count(),
        archived: candidate_ids
            .iter()
            .filter(|id| archived_present.contains(id.as_str()))
            .count(),
        live_after: after.iter().filter(|f| !f.archived).count(),
    }
}

/// Convert Simard's backend-agnostic [`RecallWeightSet`] into the library's
/// [`RecallWeights`] (same six fields, same order). Issue #2329 — the trait
/// stays backend-neutral, so this conversion is adapter-local.
fn to_library_weights(w: RecallWeightSet) -> RecallWeights {
    RecallWeights {
        text_relevance: w.text_relevance,
        confidence: w.confidence,
        importance: w.importance,
        recency: w.recency,
        usage: w.usage,
        graph: w.graph,
    }
}

// ---------------------------------------------------------------------------
// Library record -> Simard DTO converters
// ---------------------------------------------------------------------------

fn to_fact(f: SemanticFact) -> CognitiveFact {
    // Fold the per-store monotonic sequence (see `FACT_SEQ_META_KEY`) into the
    // FRONT of the exposed `node_id` so the "max node_id == most recent fact"
    // selection used by the goal-board snapshot / goal store / consolidation
    // call sites is correct on the library backend (whose raw ids are random
    // UUID-v4). Facts written before this stamping existed (or by other tooling)
    // have no sequence and sort oldest via a zero prefix. The original library id
    // is preserved as the suffix so the value stays unique and traceable.
    let seq = seq_from_metadata(&f.metadata).unwrap_or(0);
    let node_id = format!("{seq:0width$}_{}", f.node_id, width = FACT_SEQ_WIDTH);
    CognitiveFact {
        node_id,
        concept: f.concept,
        content: f.content,
        confidence: f.confidence,
        source_id: f.source_id,
        tags: f.tags,
        // Issue #2395: surface the library's reinforcement counters so callers
        // (and ranked recall) can see usage/recency, and so the reinforce-on-use
        // seam is observable after `reinforce_access`.
        usage_count: f.usage_count,
        last_accessed_at: f.last_accessed_at,
    }
}

/// Extract the adapter's monotonic fact sequence from a library fact's metadata,
/// tolerating either a JSON number or a stringified number.
fn seq_from_metadata(metadata: &HashMap<String, serde_json::Value>) -> Option<u64> {
    let v = metadata.get(FACT_SEQ_META_KEY)?;
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Seed the monotonic fact sequence on open from the maximum sequence already
/// persisted, so it keeps advancing across reopens. Returns the next value to
/// hand out (max existing + 1, or 0 for an empty / fresh store).
fn recover_fact_seq(inner: &CognitiveMemory) -> u64 {
    inner
        .get_all_facts(usize::MAX)
        .iter()
        .filter_map(|f| seq_from_metadata(&f.metadata))
        .max()
        .map_or(0, |m| m.saturating_add(1))
}

/// Strip the adapter's `{seq}_` ordering prefix (see [`FACT_SEQ_META_KEY`] /
/// [`to_fact`]) from a fact id, yielding the raw library `node_id` the
/// provenance graph is keyed on.
///
/// [`store_fact`](CognitiveMemoryOps::store_fact) /
/// [`store_fact_with_provenance`](CognitiveMemoryOps::store_fact_with_provenance)
/// return the raw library id, but [`search_facts`](CognitiveMemoryOps::search_facts)
/// surfaces the composite `{20-digit seq}_{raw}` id. Accepting either here lets
/// a caller pass the id from a recalled [`CognitiveFact`] straight into
/// [`episodes_for_fact`](CognitiveMemoryOps::episodes_for_fact) and still hit the
/// `DERIVES_FROM` edges, rather than silently getting an empty result.
fn strip_seq_prefix(fact_id: &str) -> &str {
    let bytes = fact_id.as_bytes();
    if bytes.len() > FACT_SEQ_WIDTH
        && bytes[FACT_SEQ_WIDTH] == b'_'
        && bytes[..FACT_SEQ_WIDTH].iter().all(u8::is_ascii_digit)
    {
        &fact_id[FACT_SEQ_WIDTH + 1..]
    } else {
        fact_id
    }
}

fn to_procedure(p: ProceduralMemory) -> CognitiveProcedure {
    CognitiveProcedure {
        node_id: p.node_id,
        name: p.name,
        steps: p.steps,
        prerequisites: p.prerequisites,
        usage_count: p.usage_count,
    }
}

fn to_prospective(p: ProspectiveMemory) -> CognitiveProspective {
    CognitiveProspective {
        node_id: p.node_id,
        description: p.description,
        trigger_condition: p.trigger_condition,
        action_on_trigger: p.action_on_trigger,
        status: p.status,
        priority: i64::from(p.priority),
    }
}

fn to_working(w: WorkingMemorySlot) -> CognitiveWorkingSlot {
    CognitiveWorkingSlot {
        node_id: w.node_id,
        slot_type: w.slot_type,
        content: w.content,
        relevance: w.relevance,
        task_id: w.task_id,
    }
}

fn to_episode(e: EpisodicMemory) -> CognitiveEpisode {
    CognitiveEpisode {
        node_id: e.node_id,
        content: e.content,
        source_label: e.source_label,
        temporal_index: e.temporal_index,
        compressed: e.compressed,
    }
}

impl CognitiveMemoryOps for LibraryCognitiveMemory {
    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        let ttl = i64::try_from(ttl_seconds).unwrap_or(i64::MAX);
        self.lock_write("record_sensory")?
            .store_sensory(modality, raw_data, ttl)
            .map_err(|e| map_op_err("record_sensory", e))
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(self
            .lock_write("prune_expired_sensory")?
            .prune_expired_sensory())
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        self.lock_write("push_working")?
            .store_working(slot_type, content, task_id, relevance)
            .map_err(|e| map_op_err("push_working", e))
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        Ok(self
            .lock()?
            .get_working(task_id)
            .into_iter()
            .map(to_working)
            .collect())
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        Ok(self.lock_write("clear_working")?.clear_working(task_id))
    }

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        // Fold a JSON object into the library's `HashMap<String, Value>` episode
        // payload. Non-object metadata is dropped (the native backend ignores
        // metadata entirely), so observable parity is preserved either way.
        let meta_map: Option<HashMap<String, serde_json::Value>> = metadata.and_then(|v| {
            v.as_object().map(|obj| {
                obj.iter()
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect()
            })
        });
        self.lock_write("store_episode")?
            .store_episode(content, source_label, None, meta_map.as_ref())
            .map_err(|e| map_op_err("store_episode", e))
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        // Divergence (A6): the library consolidates EXACTLY `batch_size`
        // episodes (returning None if fewer exist) and emits a separate
        // `ConsolidatedEpisode` (`con`-id), whereas the native backend
        // consolidates up to `batch_size` as long as >= 2 exist and marks the
        // sources `compressed = 1` in place (`epi`-id). To preserve the native
        // OBSERVABLE behavior ("consolidate if there are >= 2 un-compressed
        // episodes, up to batch_size"), clamp the effective batch to the number
        // of available un-compressed episodes and require >= 2.
        let mut guard = self.lock_write("consolidate_episodes")?;
        let available = guard.get_episodes(usize::MAX, false).len();
        let effective = (batch_size as usize).min(available);
        if effective < 2 {
            return Ok(None);
        }
        let summarizer = |contents: &[String]| -> String {
            format!(
                "[consolidated {} episodes]: {}",
                contents.len(),
                contents.join(" | ")
            )
        };
        guard
            .consolidate_episodes(effective, Some(summarizer))
            .map_err(|e| map_op_err("consolidate_episodes", e))
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        // Stamp a process-wide monotonic sequence into metadata so `to_fact` can
        // expose a time-ordered `node_id` (see `FACT_SEQ_META_KEY`). The fetch is
        // done while holding the write lock so the sequence order matches the
        // store order.
        let mut guard = self.lock_write("store_fact")?;
        let seq = self.fact_seq.fetch_add(1, Ordering::Relaxed);
        let mut metadata: HashMap<String, serde_json::Value> = HashMap::with_capacity(1);
        metadata.insert(FACT_SEQ_META_KEY.to_string(), serde_json::Value::from(seq));
        guard
            .store_fact(
                concept,
                content,
                confidence,
                source_id,
                Some(tags),
                Some(&metadata),
            )
            .map_err(|e| map_op_err("store_fact", e))
    }

    #[allow(clippy::too_many_arguments)]
    fn store_fact_with_provenance(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        source_id: &str,
        tags: Option<&[String]>,
        metadata: Option<&HashMap<String, serde_json::Value>>,
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        // Stamp the same process-wide monotonic sequence base `store_fact`
        // injects (see `FACT_SEQ_META_KEY`) so the provenance write path keeps
        // the "max node_id == newest fact" invariant the goal board / goal store
        // / consolidation depend on. Fold it into the caller's metadata rather
        // than replacing it, so caller-supplied keys survive. The fetch is done
        // under the write lock so sequence order matches store order.
        //
        // Deliberately the non-strict library variant (over the available
        // `store_fact_with_provenance_strict`): storing the fact is the primary
        // operation and must never fail just because a `DERIVES_FROM` edge can't
        // be drawn — provenance is additive. A `source_episode_id` that doesn't
        // resolve skips only that edge (the library logs a `warn!`), so we keep
        // the fact rather than losing it. Both call sites supply an episode that
        // is expected to exist (reflection: just stored; distillation: the
        // source episode the fact was distilled from).
        let mut guard = self.lock_write("store_fact_with_provenance")?;
        let seq = self.fact_seq.fetch_add(1, Ordering::Relaxed);
        let mut merged: HashMap<String, serde_json::Value> = metadata.cloned().unwrap_or_default();
        merged.insert(FACT_SEQ_META_KEY.to_string(), serde_json::Value::from(seq));
        guard
            .store_fact_with_provenance(
                concept,
                content,
                confidence,
                source_id,
                tags,
                Some(&merged),
                source_episode_ids,
            )
            .map_err(|e| map_op_err("store_fact_with_provenance", e))
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let guard = self.lock()?;
        // Wildcard / empty query (A4): map to the library's "return all" path
        // rather than tokenizing a literal `*`. Apply `min_confidence` and the
        // limit, matching the native wildcard semantics (filter then cap).
        // `get_all_facts` returns facts sorted by confidence descending, so the
        // facts passing `min_confidence` are always a prefix; requesting only
        // `limit` rows up front yields the same top-`limit` qualifying facts as
        // fetching everything and truncating, while materializing far fewer rows
        // when the store is large.
        let facts: Vec<SemanticFact> = if query == "*" || query.trim().is_empty() {
            let mut top = guard.get_all_facts(limit as usize);
            top.retain(|f| f.confidence >= min_confidence);
            top
        } else {
            guard.search_facts(query, limit as usize, min_confidence)
        };
        Ok(facts.into_iter().map(to_fact).collect())
    }

    fn recall_facts_ranked(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        // Issue #2329: ranked recall. `record_access = false` keeps this a pure
        // read — gathering `relevant_facts` to prepare a cycle must not bump a
        // fact's usage/recency and skew later recalls. `include_archived` /
        // `include_superseded = false` means superseded snapshot revisions
        // (collapsed by `store_fact_with_caller_key`) never re-enter recall. The
        // remaining knobs are the library defaults (limit/min_confidence here,
        // 1-hop graph, 7-day recency half-life). The library takes `&mut self`
        // (it *can* record access), so we lock for write even though this call
        // mutates nothing.
        let options = RecallOptions {
            limit: limit as usize,
            min_confidence,
            include_archived: false,
            include_superseded: false,
            record_access: false,
            weights: to_library_weights(weights),
            ..RecallOptions::default()
        };
        let mut guard = self.lock_write("recall_facts_ranked")?;
        let scored = guard
            .recall_facts_ranked(query, options)
            .map_err(|e| map_op_err("recall_facts_ranked", e))?;
        // The library already sorted by descending score; preserve that order
        // (ordering *is* the ranking — no score is surfaced on `CognitiveFact`).
        Ok(scored.into_iter().map(|s| to_fact(s.item)).collect())
    }

    fn store_fact_with_caller_key(
        &self,
        caller_key: &str,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        // Issue #2329: CallerKey dedup. Stamp the same process-wide monotonic
        // sequence `store_fact` injects (see `FACT_SEQ_META_KEY`) so the
        // "max node_id == newest fact" invariant the goal board / goal store /
        // consolidation depend on still holds for caller-key writes. The fetch is
        // done under the write lock so sequence order matches store order.
        //
        // `DedupMode::CallerKey(k)`: an identical-content write for `k` is reused
        // (no new node); a changed-content write supersedes the prior live fact
        // (archive old + `superseded_by` + `SUPERSEDES` edge new -> old). Either
        // way exactly one live fact survives per key. The returned id is the live
        // fact after the call.
        let mut guard = self.lock_write("store_fact_with_caller_key")?;
        let seq = self.fact_seq.fetch_add(1, Ordering::Relaxed);
        let mut metadata: HashMap<String, serde_json::Value> = HashMap::with_capacity(1);
        metadata.insert(FACT_SEQ_META_KEY.to_string(), serde_json::Value::from(seq));
        let input = FactInput {
            concept: concept.to_string(),
            content: content.to_string(),
            confidence,
            source_id: source_id.to_string(),
            tags: tags.to_vec(),
            metadata,
            dedup_key: Some(caller_key.to_string()),
            ..FactInput::default()
        };
        let options = StoreFactOptions {
            dedup: DedupOptions {
                mode: DedupMode::CallerKey(caller_key.to_string()),
                ..DedupOptions::default()
            },
            ..StoreFactOptions::default()
        };
        let outcome = guard
            .upsert_fact(input, &options)
            .map_err(|e| map_op_err("store_fact_with_caller_key", e))?;
        Ok(outcome.node_id)
    }

    fn prune_superseded(&self) -> SimardResult<usize> {
        // Issue #2329: reclaim the superseded tail produced by caller-key dedup.
        // `include_superseded = true` is what makes the archived revisions
        // prunable; `max_facts_per_concept = None` and `min_importance_to_keep =
        // 0.0` ensure no *live* fact is evicted (all goal records share one
        // concept, so a per-concept cap would evict live records). The library
        // protects provenance-bearing facts from deletion.
        let policy = RetentionPolicy {
            max_facts_per_concept: None,
            min_importance_to_keep: 0.0,
            include_superseded: true,
            dry_run: false,
            ..RetentionPolicy::default()
        };
        let report = self
            .lock_write("prune_superseded")?
            .prune_semantic_memory(&policy)
            .map_err(|e| map_op_err("prune_superseded", e))?;
        Ok(report.archived + report.deleted)
    }

    fn forget_low_value_facts(&self, dry_run: bool) -> SimardResult<ForgetReport> {
        // Issue #2434: controlled forgetting of *live* low-value facts. Reuses
        // the library's `prune_semantic_memory` retention machinery (the same
        // `prune_superseded` calls), but driven so it is BOTH bounded and safe.
        //
        // Candidacy ([`collect_forget_candidates`]) flows through the shared
        // [`forgetting_score`] signal and a hard provenance gate, then targets
        // only *purely forgettable* concepts. Why not just set
        // `min_importance_to_keep = FORGET_MIN_IMPORTANCE`? Because the library's
        // delete-protection is `importance >= min_importance_to_keep &&
        // has_provenance`: a low-value candidate (importance < threshold) can
        // never satisfy it, so a blanket importance threshold would delete
        // provenance-bearing low-value facts too. Instead we drive deletion via a
        // per-concept TTL (ttl = 0) over exactly the forgettable concepts with
        // `min_importance_to_keep = 0.0`. The 0.0 keep-threshold disables the
        // importance trigger (only our targeted concepts are candidates) AND
        // turns delete-protection into "ANY provenance-bearing fact is protected"
        // — belt-and-suspenders over the concept-level exclusion.
        //
        // Mandatory safety (issue #2434): a `dry_run` returns the candidate
        // preview without mutating; a live run only deletes when candidates
        // exist and records a before/after self-metric so valuable-fact loss is
        // visible.
        let mut guard = self.lock_write("forget_low_value_facts")?;

        let ForgetCandidates {
            live_before,
            candidate_ids,
            forgettable_concepts,
        } = collect_forget_candidates(&guard, Utc::now());
        let candidates = candidate_ids.len();

        // Dry-run preview: change nothing (the mandatory preview before any live
        // deletion).
        if dry_run {
            return Ok(ForgetReport {
                dry_run: true,
                live_before,
                live_after: live_before,
                candidates,
                archived: 0,
                deleted: 0,
            });
        }

        // Safe no-op: nothing qualifies, so no live run (the `archived + deleted
        // > 0` precondition from the safety contract).
        if candidates == 0 {
            return Ok(ForgetReport {
                dry_run: false,
                live_before,
                live_after: live_before,
                candidates: 0,
                archived: 0,
                deleted: 0,
            });
        }

        // Live run. Two passes because the library archives-before-deletes: pass
        // one archives the fresh candidates, pass two deletes the now-archived
        // ones. Both use the same policy.
        let ttl_seconds_by_concept: HashMap<String, i64> = forgettable_concepts
            .into_iter()
            .map(|c| (c, 0_i64))
            .collect();
        let policy = RetentionPolicy {
            max_facts_per_concept: None,
            ttl_seconds_by_concept,
            min_importance_to_keep: 0.0,
            include_superseded: false,
            dry_run: false,
        };
        guard
            .prune_semantic_memory(&policy)
            .map_err(|e| map_op_err("forget_low_value_facts", e))?;
        guard
            .prune_semantic_memory(&policy)
            .map_err(|e| map_op_err("forget_low_value_facts", e))?;

        // Measure the net effect against our candidate set (never trust the
        // coarse policy counts to attribute the change).
        let ForgetOutcome {
            archived,
            deleted,
            live_after,
        } = measure_forget_outcome(&guard, &candidate_ids);

        // Gate on a self-metric so a regression (valuable-fact loss) is visible
        // in `metrics.jsonl`. Best-effort, no-op under `cfg!(test)`.
        record_forget_metric(live_before, live_after, candidates, archived, deleted);

        Ok(ForgetReport {
            dry_run: false,
            live_before,
            live_after,
            candidates,
            archived,
            deleted,
        })
    }

    fn episodes_for_fact(&self, fact_id: &str) -> SimardResult<Vec<String>> {
        // Read side of `store_fact_with_provenance`: traverse the fact's
        // outgoing `DERIVES_FROM` edges. `fact_provenance` returns an empty
        // vector (not an error) for an unknown id or a fact with no provenance,
        // which matches the trait contract. `strip_seq_prefix` lets a caller
        // pass either the raw id from the store call or the composite id from
        // `search_facts`.
        Ok(self.lock()?.fact_provenance(strip_seq_prefix(fact_id)))
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        self.store_procedure_reinforcing("store_procedure", name, |m| {
            m.store_procedure(name, steps, Some(prerequisites))
        })
    }

    fn store_procedure_with_provenance(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
        source_episode_ids: &[String],
    ) -> SimardResult<String> {
        // Identical idempotent upsert-that-reinforces contract as
        // `store_procedure` (#2298, enforced by `store_procedure_reinforcing`),
        // plus `PROCEDURE_DERIVES_FROM` edges to `source_episode_ids` (#2325) —
        // which the library attaches to the single canonical node, so
        // re-storing the same name does not fork it. Non-strict variant for the
        // same reason as `store_fact_with_provenance`: a missing source episode
        // skips only that edge (logged), it never fails the procedure write.
        self.store_procedure_reinforcing("store_procedure_with_provenance", name, |m| {
            m.store_procedure_with_provenance(name, steps, Some(prerequisites), source_episode_ids)
        })
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        // Wildcard (A4): `"*"` means "return all"; the library's empty-query
        // path returns every procedure (truncated to `limit`).
        let effective_query = if query == "*" { "" } else { query };
        let mut procedures: Vec<CognitiveProcedure> = self
            .lock()?
            .search_procedures(effective_query, limit as usize)
            .into_iter()
            .map(to_procedure)
            .collect();
        // Issue #2440: order by `usage_count` DESC so a frequently-used procedure
        // ranks ahead of a cold one matching the same query — `recall_procedure`
        // is a recall path and ordering IS the ranking. `search_procedures`
        // returns library order (CONTAINS match), which carries no usage signal;
        // a stable sort keeps that order as the tiebreaker among equal usage.
        procedures.sort_by_key(|p| std::cmp::Reverse(p.usage_count));
        Ok(procedures)
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        let priority_i32 = priority.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.lock_write("store_prospective")?
            .store_prospective(
                description,
                trigger_condition,
                action_on_trigger,
                priority_i32,
            )
            .map_err(|e| map_op_err("store_prospective", e))
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        // Divergence (A3): the library uses tokenized/lowercased keyword-overlap
        // matching, mutates the matched prospective's status to "triggered", and
        // therefore fires each prospective at most once; the native backend uses
        // a case-sensitive whole-substring `content CONTAINS trigger`, is
        // read-only, and re-fires on every call. Both agree on FIRST-fire for a
        // matching trigger, which is what callers rely on. Documented for #85.
        Ok(self
            .lock_write("check_triggers")?
            .check_triggers(content)
            .into_iter()
            .map(to_prospective)
            .collect())
    }

    fn resolve_prospective(&self, node_id: &str) -> SimardResult<()> {
        self.lock_write("resolve_prospective")?
            .resolve_prospective(node_id);
        Ok(())
    }

    fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
        // De-fork Phase 2b (issue #2307): the library now exposes a persistent,
        // one-way distilled latch. Delegate to it. The library returns `false`
        // when the id is unknown or owned by a different agent; that is not an
        // error for this caller (the native backend likewise no-op'd a
        // non-matching id), so we map any outcome to `Ok(())`.
        self.lock_write("mark_episode_distilled")?
            .mark_episode_distilled(node_id);
        Ok(())
    }

    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        // De-fork Phase 2b (issue #2307): episode distillation now runs against
        // this backend. The library returns this agent's not-yet-distilled
        // episodes, newest-first, capped at `limit` — the same contract the
        // deleted native backend implemented with `WHERE e.distilled = 0
        // ORDER BY e.id DESC`.
        Ok(self
            .lock()?
            .list_undistilled_episodes(limit as usize)
            .into_iter()
            .map(to_episode)
            .collect())
    }

    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        if keywords.is_empty() {
            return Ok(vec![]);
        }
        let needles: Vec<String> = keywords.iter().map(|k| k.to_lowercase()).collect();
        // Include compressed episodes so consolidation sources remain recallable
        // by keyword (matching native, whose query has no compressed filter).
        // `get_episodes` already returns newest-first by `temporal_index`.
        // `take(limit)` short-circuits the per-episode lowercase/contains scan
        // (and the DTO conversion) once `limit` matches are found, instead of
        // converting every match and truncating afterwards.
        let episodes: Vec<CognitiveEpisode> = self
            .lock()?
            .get_episodes(usize::MAX, true)
            .into_iter()
            .filter(|e| {
                let content = e.content.to_lowercase();
                needles.iter().any(|kw| content.contains(kw))
            })
            .take(limit as usize)
            .map(to_episode)
            .collect();
        Ok(episodes)
    }

    fn recall_episodes_ranked(
        &self,
        query: &str,
        limit: u32,
        weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        // Issue #2395: ranked episodic recall over the *keyword-relevant*
        // episodes. The library scores recency/usage/graph for EVERY
        // non-compressed episode, but Simard's episodic recall is
        // relevance-gated (an unrelated-but-recent episode must not surface), so
        // gate the ranked output to episodes that share a query keyword — this
        // preserves the existing recall semantics while upgrading the *ordering*
        // from newest-first to the multi-signal rank.
        //
        // `record_access = false` keeps this a pure read: the OODA cycle issues
        // several recalls and reinforcement is the separate `reinforce_access`
        // seam, so a recall must not skew later recalls. The library takes
        // `&mut self`, so we still lock for write. `limit = usize::MAX` defers
        // truncation until *after* the keyword gate, so a relevant episode
        // ranked behind recent noise is not dropped before the gate runs.
        let needles: Vec<String> = query
            .split_whitespace()
            .map(str::to_lowercase)
            .filter(|s| !s.is_empty())
            .collect();
        if needles.is_empty() {
            return Ok(vec![]);
        }
        let matches_kw = |content: &str| {
            let c = content.to_lowercase();
            needles.iter().any(|kw| c.contains(kw))
        };

        let options = RecallOptions {
            limit: usize::MAX,
            record_access: false,
            weights: to_library_weights(weights),
            ..RecallOptions::default()
        };
        let mut guard = self.lock_write("recall_episodes_ranked")?;
        let scored = guard
            .recall_episodes_ranked(query, options)
            .map_err(|e| map_op_err("recall_episodes_ranked", e))?;

        let mut out: Vec<CognitiveEpisode> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for s in scored {
            let ep = to_episode(s.item);
            if matches_kw(&ep.content) && seen.insert(ep.node_id.clone()) {
                out.push(ep);
            }
        }
        // UNION backfill: the library ranked path skips compressed episodes, but
        // consolidation sources stay relevant — a distilled fact/procedure must
        // remain traceable to the episodes it came from. Append the compressed
        // keyword matches the ranked pass dropped (newest-first), deduped.
        for e in guard.get_episodes(usize::MAX, true) {
            if e.compressed && matches_kw(&e.content) {
                let ep = to_episode(e);
                if seen.insert(ep.node_id.clone()) {
                    out.push(ep);
                }
            }
        }
        out.truncate(limit as usize);
        Ok(out)
    }

    fn reinforce_access(&self, node_id: &str, kind: MemoryKind) -> SimardResult<()> {
        // Issue #2395: reinforce-on-use. The library's `record_access` bumps
        // `usage_count` (saturating) and stamps `last_accessed_at`, persisted
        // across reopen. Fact ids surfaced by recall carry the adapter's
        // monotonic sequence prefix (see `FACT_SEQ_META_KEY` / `to_fact`), so
        // strip it to match the raw library node; episode / procedure ids are
        // already raw.
        let raw = match kind {
            MemoryKind::Fact => strip_seq_prefix(node_id),
            MemoryKind::Episode | MemoryKind::Procedure => node_id,
        };
        self.lock_write("reinforce_access")?
            .record_access(raw, AccessKind::Recall)
            .map_err(|e| map_op_err("reinforce_access", e))
    }

    fn search_episodes_starting_with(
        &self,
        prefix: &str,
        limit: u32,
    ) -> SimardResult<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
        // `get_episodes` returns newest-first; `take(limit)` stops once `limit`
        // matches are collected instead of materializing every match and then
        // truncating.
        let out: Vec<(String, chrono::DateTime<chrono::Utc>)> = self
            .lock()?
            .get_episodes(usize::MAX, true)
            .into_iter()
            .filter(|e| e.content.starts_with(prefix))
            .take(limit as usize)
            .map(|e| (e.content, e.created_at))
            .collect();
        Ok(out)
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        // Divergence (A7): the library returns a `HashMap<String, usize>` keyed
        // by `MemoryCategory::as_str()`. Fold it into the typed DTO; any key the
        // library does not emit defaults to 0.
        let stats = self.lock()?.get_statistics();
        let get = |key: &str| stats.get(key).copied().unwrap_or(0) as u64;
        Ok(CognitiveStatistics {
            sensory_count: get("sensory"),
            working_count: get("working"),
            episodic_count: get("episodic"),
            semantic_count: get("semantic"),
            procedural_count: get("procedural"),
            prospective_count: get("prospective"),
        })
    }

    fn is_read_only(&self) -> bool {
        // The library backend is always a writer (no read-only constructor at
        // the pinned commit), so this is a fixed `false` rather than a stored
        // flag — matching the trait's documented default.
        false
    }

    fn graph_stats(&self) -> SimardResult<GraphStats> {
        // Issue #2331. Read-only aggregate over the cognitive-memory graph,
        // computed under a single read lock so the snapshot is internally
        // consistent. The pinned library rev exposes provenance readers
        // (`fact_provenance` / `procedure_provenance`) but NO public per-type
        // edge counter, so `SIMILAR_TO` / `SUPERSEDES` stay 0 (documented in
        // `GraphStats` and `docs/memory.md`); the snapshot-dedup fields below
        // give the operator a computed proxy for the `SUPERSEDES` activity.
        let guard = self.lock()?;

        // `get_all_facts` returns every `Semantic` node for this agent
        // (live + archived/superseded — `get_statistics`'s semantic count is the
        // same node set), so `facts_total` here matches the per-type table.
        let facts = guard.get_all_facts(usize::MAX);

        let mut derives_from_edges: u64 = 0;
        let mut facts_with_provenance: u64 = 0;
        let mut snapshot_facts_total: u64 = 0;
        let mut snapshot_caller_keys: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for fact in &facts {
            // `fact_provenance` is keyed on the raw library `node_id`
            // (`get_all_facts` surfaces the raw id, not the seq-prefixed
            // composite, so no `strip_seq_prefix` is needed here).
            let provenance = guard.fact_provenance(&fact.node_id);
            if !provenance.is_empty() {
                facts_with_provenance += 1;
                derives_from_edges += provenance.len() as u64;
            }
            if fact.concept == SNAPSHOT_FACT_CONCEPT {
                snapshot_facts_total += 1;
                if let Some(key) = fact.dedup_key.as_deref().filter(|k| !k.is_empty()) {
                    snapshot_caller_keys.insert(key.to_string());
                }
            }
        }

        // Procedures: sum `PROCEDURE_DERIVES_FROM` edges. The empty query maps to
        // the library's "return all" path (same as `recall_procedure("*", …)`).
        let mut procedure_derives_from_edges: u64 = 0;
        for proc in guard.search_procedures("", usize::MAX) {
            procedure_derives_from_edges += guard.procedure_provenance(&proc.node_id).len() as u64;
        }

        Ok(GraphStats {
            derives_from_edges,
            procedure_derives_from_edges,
            // No public reader at the pinned rev — surfaced as 0; see doc above.
            similar_to_edges: 0,
            supersedes_edges: 0,
            facts_with_provenance,
            facts_total: facts.len() as u64,
            distinct_snapshot_caller_keys: snapshot_caller_keys.len() as u64,
            snapshot_facts_total,
        })
    }

    fn checkpoint(&self) -> SimardResult<()> {
        // The library exposes durability via `close`, which issues a LadybugDB
        // CHECKPOINT (collapsing the WAL into the main file) while keeping the
        // store usable. Flushing here mirrors the native backend's CHECKPOINT so
        // a subsequent reopen of the same path observes all committed writes.
        self.lock()?.close();
        Ok(())
    }
}
