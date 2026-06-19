//! Library-backed cognitive memory adapter — de-fork Phase 2a (issue #86).
//!
//! [`LibraryCognitiveMemory`] implements Simard's existing
//! [`CognitiveMemoryOps`](super::CognitiveMemoryOps) trait by delegating to the
//! upstream `amplihack-memory-lib` [`CognitiveMemory`], opened with
//! `open_persistent` (the library's lbug-backed durable `GraphStore`). It is the
//! "SAFE INTEGRATION" seam: it proves Simard can drive the library behind its own
//! abstraction WITHOUT deleting [`NativeCognitiveMemory`](super::NativeCognitiveMemory)
//! and WITHOUT migrating live daemon data. The native backend remains the
//! default; this adapter is only compiled behind the `library-memory` cargo
//! feature.
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
//!   [`SimardError::BridgeCallFailed`] with `bridge = "cognitive-memory-library"`
//!   (mirroring how the native backend tags `cognitive-memory-native`),
//!   preserving the upstream `MemoryError` message. No new `SimardError` variant
//!   is introduced — this keeps the change additive.
//! * **Documented divergences (A3/A6/A7).** `check_triggers`,
//!   `consolidate_episodes`, and `get_statistics` legitimately differ from the
//!   native semantics. The adapter maps onto the library's high-level behavior
//!   rather than forcing native semantics; the divergences are documented here
//!   and in `docs/architecture/cognitive-memory-library-adapter.md`, and feed
//!   amplihack-memory-lib#85.
//! * **API gaps (A5).** `mark_episode_distilled` and `list_undistilled_episodes`
//!   have no library equivalent at the pinned commit, so episode distillation
//!   cannot run against this backend. A panic on the OODA distillation hot path
//!   would be strictly worse than degrading, so the adapter degrades to a no-op
//!   — but *loudly*: `list_undistilled_episodes` is overridden to emit a
//!   one-time warning (so a disabled backend is distinguishable from a merely
//!   quiet one) before returning empty, while `mark_episode_distilled` inherits
//!   the trait's no-op default (it is only reached after a non-empty undistilled
//!   list, which never occurs here). Tracked upstream as amplihack-memory-lib#85.
//!
//! All persistence is rooted at a caller-supplied `state_root` (a `TempDir` in
//! tests). The adapter never opens, reads, writes, or migrates the live daemon
//! store at `~/.simard/cognitive_memory.ladybug`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, Once};

use amplihack_memory::{
    CognitiveMemory, EpisodicMemory, MemoryError, ProceduralMemory, ProspectiveMemory,
    SemanticFact, WorkingMemorySlot,
};

use super::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

/// Agent name the persistent library store is scoped to. The library rejects an
/// empty name, and the same name must be used across reopens for data to round
/// trip, so this is a fixed, validated, non-empty constant.
const LIBRARY_AGENT_NAME: &str = "simard";

/// Identifier used in mapped [`SimardError`]s so failures are attributable to
/// the library backend (vs. the native `cognitive-memory-native`).
const STORE_NAME: &str = "cognitive-memory-library";

/// Cognitive memory backed by the upstream `amplihack-memory-lib`
/// [`CognitiveMemory`] (persistent, lbug-backed).
///
/// Implements [`CognitiveMemoryOps`] so callers are backend-agnostic. Only
/// available with `--features library-memory`.
pub struct LibraryCognitiveMemory {
    /// The library memory, behind a `Mutex` for `&self` -> `&mut` interior
    /// mutability (see module docs, A2).
    inner: Mutex<CognitiveMemory>,
    /// Fires the "distillation disabled" warning at most once per instance
    /// (A5). The library has no distilled-flag API at the pinned commit, so
    /// distillation degrades to a no-op; this makes that degradation loud once
    /// instead of invisible on the OODA hot path. See `list_undistilled_episodes`.
    distillation_warning: Once,
}

impl LibraryCognitiveMemory {
    /// Open (or create) a persistent library-backed cognitive memory under
    /// `state_root`.
    ///
    /// The store lives at a dedicated sub-path (`state_root/cognitive`) so it is
    /// isolated from anything else under `state_root`. In tests `state_root` is a
    /// `TempDir`; the adapter never touches the live daemon store.
    ///
    /// # Errors
    ///
    /// Returns [`SimardError::PersistentStoreIo`] if the underlying LadybugDB
    /// store cannot be opened.
    pub fn open(state_root: &Path) -> SimardResult<Self> {
        let db_path = state_root.join("cognitive");
        let inner =
            CognitiveMemory::open_persistent(&db_path, LIBRARY_AGENT_NAME).map_err(|e| {
                SimardError::PersistentStoreIo {
                    store: STORE_NAME.to_string(),
                    action: "open_persistent".to_string(),
                    path: db_path,
                    reason: e.to_string(),
                }
            })?;
        Ok(Self {
            inner: Mutex::new(inner),
            distillation_warning: Once::new(),
        })
    }

    /// Lock the inner library memory, mapping a poisoned lock to a loud error
    /// rather than panicking.
    fn lock(&self) -> SimardResult<MutexGuard<'_, CognitiveMemory>> {
        self.inner.lock().map_err(|_| SimardError::StoragePoisoned {
            store: STORE_NAME.to_string(),
        })
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

// ---------------------------------------------------------------------------
// Library record -> Simard DTO converters
// ---------------------------------------------------------------------------

fn to_fact(f: SemanticFact) -> CognitiveFact {
    CognitiveFact {
        node_id: f.node_id,
        concept: f.concept,
        content: f.content,
        confidence: f.confidence,
        source_id: f.source_id,
        tags: f.tags,
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
        self.lock()?
            .store_sensory(modality, raw_data, ttl)
            .map_err(|e| map_op_err("record_sensory", e))
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        Ok(self.lock()?.prune_expired_sensory())
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        self.lock()?
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
        Ok(self.lock()?.clear_working(task_id))
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
        self.lock()?
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
        let mut guard = self.lock()?;
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
        self.lock()?
            .store_fact(concept, content, confidence, source_id, Some(tags), None)
            .map_err(|e| map_op_err("store_fact", e))
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

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        self.lock()?
            .store_procedure(name, steps, Some(prerequisites))
            .map_err(|e| map_op_err("store_procedure", e))
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        // Wildcard (A4): `"*"` means "return all"; the library's empty-query
        // path returns every procedure (truncated to `limit`).
        let effective_query = if query == "*" { "" } else { query };
        Ok(self
            .lock()?
            .search_procedures(effective_query, limit as usize)
            .into_iter()
            .map(to_procedure)
            .collect())
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        let priority_i32 = priority.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        self.lock()?
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
            .lock()?
            .check_triggers(content)
            .into_iter()
            .map(to_prospective)
            .collect())
    }

    fn resolve_prospective(&self, node_id: &str) -> SimardResult<()> {
        self.lock()?.resolve_prospective(node_id);
        Ok(())
    }

    // `mark_episode_distilled` is intentionally NOT overridden (A5): the library
    // has no distilled-flag API at the pinned commit, and it is only reached
    // after `list_undistilled_episodes` yields episodes — which never happens
    // here — so the adapter inherits the trait's contractually-safe no-op
    // default instead of panicking. Tracked upstream as amplihack-memory-lib#85.

    fn list_undistilled_episodes(&self, _limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        // A5 gap: the library has no distilled-flag API at the pinned commit, so
        // episode distillation cannot run against this backend. The trait's
        // no-op default returns empty — but on the OODA distillation hot path
        // that is indistinguishable from "no episodes to distill", silently
        // disabling the feature. Surface the degradation LOUDLY exactly once per
        // instance so an operator can tell a disabled backend from a quiet one,
        // then return empty so the pass cleanly (and non-fatally) skips. Tracked
        // upstream as amplihack-memory-lib#85.
        self.distillation_warning.call_once(|| {
            eprintln!(
                "[simard] cognitive memory: library backend cannot track the \
                 distilled flag — episode distillation is DISABLED (no-op) for \
                 this backend (de-fork Phase 2a, issue #86; amplihack-memory-lib#85)"
            );
        });
        Ok(vec![])
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

    fn checkpoint(&self) -> SimardResult<()> {
        // The library exposes durability via `close`, which issues a LadybugDB
        // CHECKPOINT (collapsing the WAL into the main file) while keeping the
        // store usable. Flushing here mirrors the native backend's CHECKPOINT so
        // a subsequent reopen of the same path observes all committed writes.
        self.lock()?.close();
        Ok(())
    }
}
