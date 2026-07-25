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
//!   [`SimardError::RpcCallFailed`] with `memory = "cognitive-memory-library"`,
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
    /// Cross-process open-serialization guard (issue: lbug lock-contention
    /// mistaken for corruption). Held for the lifetime of this handle so no
    /// other process can open the same store concurrently and trip lbug's
    /// lock-conflict-as-corruption rebuild (which wipes memory). Declared
    /// **last** so it drops **after** `inner` — the advisory `flock` is
    /// released only once the underlying lbug store has finished closing and
    /// dropped its own PID lock. `None` for the in-memory test constructor.
    #[allow(dead_code)]
    open_guard: Option<super::open_guard::CognitiveOpenGuard>,
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
        // Serialize cross-process opens BEFORE touching the library. lbug takes
        // a POSIX/PID lock on the store and mis-classifies a lock conflict from
        // a *second* concurrent process as catalog corruption — quarantining the
        // DB and rebuilding it EMPTY. Acquiring this guard first means the
        // library never sees a concurrent open on this path: a transient race
        // waits (bounded backoff) and then proceeds, and a genuinely
        // still-held store makes us FAIL LOUD here rather than let the library
        // wipe memory. Same-process re-opens share the guard (no self-deadlock).
        let open_guard = super::open_guard::CognitiveOpenGuard::acquire(state_root)?;

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
            open_guard: Some(open_guard),
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
            open_guard: None,
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
    /// it keeps the documented multi-site contract intact (`launch_writer_client`
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
    SimardError::RpcCallFailed {
        endpoint: STORE_NAME.to_string(),
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
// Word-boundary episodic-recall relevance gate
// ---------------------------------------------------------------------------

/// Tokenize `text` into its set of distinct lowercase alphanumeric words.
///
/// Splitting on any run of non-alphanumeric characters (rather than only
/// whitespace) means punctuation attached to a token is folded away — `"kafka,"`
/// and `"kafka"` both yield the bare word `kafka`, and `"durable-recall"` yields
/// `{durable, recall}`. Empty tokens produced by leading/trailing/repeated
/// separators are dropped. This is the same tokenization
/// [`crate::knowledge_context`] and [`crate::memory_consolidation::tokenize_objective`]
/// use, so the recall gate scores relevance on the same word basis as the rest
/// of the cognition stack.
fn tokenize_words(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Minimum length a de-pluralized needle stem must clear before it is allowed to
/// fold onto a content word (mirrors `knowledge_context`'s `MIN_TOKEN_LEN`), so a
/// short token never collapses onto a one/zero-character fragment — e.g. `"is"`
/// does not fold to `"i"`, and the fold can never re-introduce broad matching.
const MIN_FOLDED_STEM_LEN: usize = 2;

/// Minimum length a CLEAN (all-alphanumeric) query token must clear before it is
/// admitted to a recall needle set. Mirrors `knowledge_context`'s `MIN_TOKEN_LEN`
/// (which drops sub-threshold objective tokens before pack matching), so the
/// whole cognition stack applies the same cut.
///
/// A single-character clean token (`"s"` from a possessive like "Rust's", an
/// initial, or a stray list separator) is recall NOISE on the word-boundary
/// gate: `needle_matches_word` accepts it via `word.starts_with(needle)`, so it
/// matches EVERY content word beginning with that character — floating unrelated
/// episodes/facts into the capped turn/OODA working-context recall and dragging
/// recall precision (and effective distillation fact-yield) down. Dropping such
/// tokens closes that leak without touching the inflectional/plural recall a
/// two-or-more-character token still drives. RAW tokens (markers, hyphenated
/// concepts) are never length-cut — they keep the library's exact substring
/// semantics their callers store and re-filter on.
const MIN_CLEAN_NEEDLE_LEN: usize = 2;

/// `true` iff query token `needle` is relevant to a single content `word` — the
/// per-(needle, word) predicate [`shares_word_prefix`] applies across the query
/// word set and the tokenized content.
///
/// Two match shapes, in precedence order:
///  1. **Word-boundary prefix** (`word.starts_with(needle)`): the primary gate,
///     which preserves the inflectional recall the live path depends on — a
///     query stem still recalls its inflected forms (`deploy` → "deployed" /
///     "deploys" / "deployment", `sync` → "syncing"). This also covers the
///     additive regular plural (`test` → "tests", `box` → "boxes").
///  2. **Conservative singular/plural fold by EQUALITY**: closes the direction
///     the prefix rule alone cannot — a PLURAL query token recalling the
///     SINGULAR content word (`tests` → "test", `caches` → "cache",
///     `categories` → "category"), plus the shape-changing `-y`↔`-ies` pair in
///     both directions (`category` → "categories"). Folding matches only on
///     whole-word EQUALITY of a generated variant (never a prefix) and only when
///     the stem clears [`MIN_FOLDED_STEM_LEN`], so — unlike a prefix on a
///     stripped stem — it cannot re-introduce the interior over-matching the
///     word-boundary rule removed. This is the same guarded folding
///     `knowledge_context::token_matches_pack` applies to pack selection
///     (PR #4241 lineage), now extended to episodic/keyword/fact recall so the
///     whole cognition stack folds regular plurals uniformly.
///
/// Both arguments are already lowercase (the caller lowercases content words and
/// [`tokenize_words`] lowercases needles); the variants operate on ASCII
/// inflectional endings only.
fn needle_matches_word(needle: &str, word: &str) -> bool {
    if word.starts_with(needle) {
        return true;
    }
    // Subtractive regular plural: plural needle → singular content word. `-es`
    // is tried before `-s` so a base is not over-generated (`caches` also folds
    // via `-s` → "cache", which the OR below still reaches).
    if needle
        .strip_suffix("es")
        .filter(|s| s.len() >= MIN_FOLDED_STEM_LEN)
        .is_some_and(|s| s == word)
        || needle
            .strip_suffix('s')
            .filter(|s| s.len() >= MIN_FOLDED_STEM_LEN)
            .is_some_and(|s| s == word)
    {
        return true;
    }
    // `-y` ↔ `-ies`, both directions (the stem changes, so the prefix branch
    // misses these): `categories` → "category" and `category` → "categories".
    if needle
        .strip_suffix("ies")
        .filter(|s| s.len() >= MIN_FOLDED_STEM_LEN)
        .is_some_and(|s| word.strip_suffix('y') == Some(s))
    {
        return true;
    }
    if needle.len() > MIN_FOLDED_STEM_LEN
        && needle
            .strip_suffix('y')
            .is_some_and(|s| word.strip_suffix("ies") == Some(s))
    {
        return true;
    }
    false
}

/// `true` iff `content` shares a keyword with the query at a WORD BOUNDARY: some
/// `needle` query token matches a whole word in `content` per
/// [`needle_matches_word`] — a word-boundary prefix, or a conservative
/// singular/plural fold.
///
/// This is the episodic-recall relevance gate. It replaces a raw-substring gate
/// (`content.to_lowercase().contains(kw)`) that matched a query token wherever
/// it was embedded — including the INTERIOR or SUFFIX of an unrelated content
/// word (`act` in "reactor" / "contract", `test` in "latest", `own` in
/// "download"), floating off-topic episodes into the ranked set that feeds the
/// OODA cycle's working context and degrading recall precision.
///
/// Anchoring the match to a word boundary removes those interior/suffix false
/// positives while PRESERVING the inflectional recall the live path depends on
/// (`deploy` → "deployed"), and — via the fold in [`needle_matches_word`] — also
/// recalls the singular form of a plural query token (`tests` → "test"), the
/// asymmetry a prefix-only gate leaves open. A pure whole-word (equality) gate
/// would drop the inflectional recalls; a raw substring gate admits interior
/// noise. This aligns episodic recall with the word-boundary + plural-folding
/// policy adopted elsewhere in the cognition stack
/// (`knowledge_context::relevance_score`, `memory_consolidation::classifier`,
/// `fact_reliability`; PR #4241 lineage).
///
/// `needles` is the pre-tokenized query word set (see [`tokenize_words`]);
/// `content` is tokenized on the same word basis. Content with no alphanumeric
/// words shares nothing and is gated out.
fn shares_word_prefix(content: &str, needles: &HashSet<String>) -> bool {
    content
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .any(|w| {
            let word = w.to_lowercase();
            needles
                .iter()
                .any(|needle| needle_matches_word(needle, &word))
        })
}

/// A `search_facts` query partitioned into the two token shapes the fact-recall
/// relevance gate ([`fact_shares_query_relevance`]) treats differently — exactly
/// mirroring the clean/raw partition [`LibraryCognitiveMemory::search_episodes_by_keywords`]
/// already applies to keyword episode recall.
struct FactQueryNeedles {
    /// Whitespace tokens that are entirely alphanumeric — the shape a
    /// natural-language recall query emits (a turn objective, `"rust"`,
    /// `"project"`). These match at a WORD BOUNDARY (prefix of a whole word).
    clean: HashSet<String>,
    /// Whitespace tokens carrying any non-alphanumeric char — a concept label
    /// (`"bug-pattern"`), a marker (`"journal:2026-07-18"`, `"goal-edge:blocks"`,
    /// `"sub:abc"`), or a punctuated phrase. These keep the library's exact
    /// case-insensitive SUBSTRING semantics their callers store and re-filter on.
    raw: Vec<String>,
}

/// Partition a `search_facts` query into [`FactQueryNeedles`], lowercasing every
/// token. Empty tokens (from repeated separators) are dropped. A token is CLEAN
/// iff every character is alphanumeric AND it clears [`MIN_CLEAN_NEEDLE_LEN`]
/// (a sub-threshold clean token is dropped as recall noise); otherwise it is RAW.
fn partition_fact_query(query: &str) -> FactQueryNeedles {
    let mut clean: HashSet<String> = HashSet::new();
    let mut raw: Vec<String> = Vec::new();
    for token in query.split_whitespace() {
        let lowered = token.to_lowercase();
        if lowered.is_empty() {
            continue;
        }
        if lowered.chars().all(char::is_alphanumeric) {
            // Drop a sub-threshold clean token: as a word-boundary PREFIX a lone
            // character matches nearly every word, so it is recall noise — the
            // same MIN_TOKEN_LEN cut `knowledge_context` applies to objective
            // tokens. RAW tokens keep exact substring semantics and are never cut.
            if lowered.len() >= MIN_CLEAN_NEEDLE_LEN {
                clean.insert(lowered);
            }
        } else {
            raw.push(lowered);
        }
    }
    FactQueryNeedles { clean, raw }
}

/// `true` iff a fact is genuinely relevant to `needles` — the fact-recall
/// relevance gate that closes the interior/suffix substring-recall gap on the
/// FACT path, mirroring the word-boundary gate `recall_episodes_ranked` /
/// `search_episodes_by_keywords` already apply to episode recall (PR #4241
/// lineage).
///
/// The upstream library's `search_facts` matches a query token as a raw
/// case-insensitive SUBSTRING of a fact's concept OR content, so a clean
/// natural-language token floated facts in on the INTERIOR/SUFFIX of an
/// unrelated word — `"act"` recalled "re**act**or" and "artif**act**", `"own"`
/// recalled "d**own**load", `"test"` recalled "la**test**" — polluting the
/// capped working-context recall the turn/OODA path (`base_type_turn::
/// prepare_turn_context`) feeds to reasoning and dragging fact recall precision
/// down. A fact is relevant iff:
///
///   * some CLEAN token is a prefix of a whole word in the concept OR content
///     (word-boundary match) — this drops the interior/suffix noise while
///     PRESERVING the inflectional recall the live path depends on (`deploy`
///     still recalls "deployed"/"deploys"), OR
///   * some RAW token is a case-insensitive substring of the concept OR content
///     — preserving verbatim the exact concept/marker lookups (`"bug-pattern"`,
///     `"journal:2026-07-18"`, `"goal-edge:blocks"`) their callers store and
///     re-filter on.
///
/// Both fields are checked because the library matches a query against concept
/// AND content, so gating on content alone would drop a legitimate concept hit.
fn fact_shares_query_relevance(concept: &str, content: &str, needles: &FactQueryNeedles) -> bool {
    if !needles.clean.is_empty()
        && (shares_word_prefix(content, &needles.clean)
            || shares_word_prefix(concept, &needles.clean))
    {
        return true;
    }
    if !needles.raw.is_empty() {
        let content_lc = content.to_lowercase();
        let concept_lc = concept.to_lowercase();
        if needles
            .raw
            .iter()
            .any(|kw| content_lc.contains(kw.as_str()) || concept_lc.contains(kw.as_str()))
        {
            return true;
        }
    }
    false
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
        // Carry the real wall-clock instant through so the dashboard
        // "Recent Memories" panel can render "time ago" (issue #4383).
        created_at: Some(e.created_at),
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
            let needles = partition_fact_query(query);
            if needles.clean.is_empty() && needles.raw.is_empty() {
                // The query held ONLY sub-threshold clean tokens (e.g. a lone
                // "s"/"a" from a possessive or initial); after the
                // MIN_CLEAN_NEEDLE_LEN cut nothing survives to match on. Recall
                // nothing rather than fall through to the library's raw-substring
                // `search_facts`, which would match such a character as a
                // substring of nearly every stored fact — a worse over-match than
                // the word-boundary leak this cut removes.
                Vec::new()
            } else if needles.clean.is_empty() {
                // Pure concept/marker query (every token carries a non-alphanumeric
                // char — a hyphenated concept like `bug-pattern`, or a `journal:` /
                // `goal-edge:` / `sub:` marker). Preserve the library's exact
                // substring semantics AND its `limit` verbatim: these callers store
                // and re-filter on that precise surface form, so the word-boundary
                // gate does not apply and must add no behavior.
                guard.search_facts(query, limit as usize, min_confidence)
            } else {
                // Natural-language (or mixed) query: apply the word-boundary
                // relevance gate so a clean token no longer floats a fact in on the
                // INTERIOR/SUFFIX of an unrelated word (`act` in "reactor", `own` in
                // "download"), aligning FACT recall precision with the episodic
                // recall gate (PR #4241 lineage). Truncation is deferred until AFTER
                // the gate — the library is queried unbounded (`usize::MAX`) so a
                // genuinely relevant fact ranked behind an interior-substring false
                // positive is not dropped before the gate runs (mirroring
                // `recall_episodes_ranked`), then the surviving set is capped to
                // `limit`.
                guard
                    .search_facts(query, usize::MAX, min_confidence)
                    .into_iter()
                    .filter(|f| fact_shares_query_relevance(&f.concept, &f.content, &needles))
                    .take(limit as usize)
                    .collect()
            }
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
        let facts: Vec<CognitiveFact> = scored.into_iter().map(|s| to_fact(s.item)).collect();
        // Release the whole-memory write lock BEFORE the metric step: `facts` is
        // fully owned here, and precision folding allocates lowercased copies and
        // takes a second (metrics) lock — none of which should run inside the
        // serialized recall critical section on this hot path.
        drop(guard);
        // Fold this recall's precision@k into the in-process recall-quality
        // aggregate so the per-cycle metric sweep emits ONE durable
        // `recall_precision_at_k` sample (cycle mean) — no `metrics.jsonl` write
        // on this hot path. Undefined-precision recalls (wildcard/empty query or
        // an empty result) contribute no sample. Pure observation: it never
        // changes the returned set.
        if let Some(p) = super::metrics::precision_at_k(query, &facts, facts.len()) {
            super::metrics::observe_recall_precision(super::metrics::RECALL_PRECISION_SITE, p);
        }
        Ok(facts)
    }

    fn recall_facts_ranked_reinforced(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
        weights: RecallWeightSet,
    ) -> SimardResult<Vec<CognitiveFact>> {
        // Issue #2440 (perf): library-backend override of the default trait
        // impl. The default scores via `recall_facts_ranked` (one write-lock
        // acquisition) and then reinforces each returned fact through
        // `reinforce_access`, which re-acquires the write lock ONCE PER fact —
        // `1 + N` lock acquisitions and `N` separate `record_access`
        // transactions for a `limit`-sized recall on a direct recall-intent
        // read path. (Staged #2440 API: no production caller is wired yet — see
        // the trait doc — so this override's win is realized once one is.) Here
        // we score AND reinforce the top-k under a SINGLE write-lock
        // acquisition: identical returned set and order, identical best-effort
        // per-fact reinforcement, but one lock and one critical section. The
        // scored items carry the raw library `node_id`, so we reinforce on that
        // directly (no seq-prefix round-trip via `strip_seq_prefix` the default
        // pays). Holding the lock across the batch also closes the
        // scoring→reinforcement window the default's contract calls out, so a
        // concurrent forgetting pass can't delete a just-recalled fact
        // mid-batch.
        let options = RecallOptions {
            limit: limit as usize,
            min_confidence,
            include_archived: false,
            include_superseded: false,
            record_access: false,
            weights: to_library_weights(weights),
            ..RecallOptions::default()
        };
        let mut guard = self.lock_write("recall_facts_ranked_reinforced")?;
        let scored = guard
            .recall_facts_ranked(query, options)
            .map_err(|e| map_op_err("recall_facts_ranked_reinforced", e))?;
        let facts: Vec<CognitiveFact> = scored
            .into_iter()
            .map(|s| {
                let item = s.item;
                // Best-effort per the trait contract: a failed usage/recency
                // bump must never drop the returned set or turn a successful
                // recall into an error.
                if let Err(e) = guard.record_access(&item.node_id, AccessKind::Recall) {
                    tracing::debug!(
                        target: "simard::memory",
                        node_id = %item.node_id,
                        error = %e,
                        "recall_facts_ranked_reinforced: record_access failed (non-fatal, recall unaffected)"
                    );
                }
                to_fact(item)
            })
            .collect();
        // Release the whole-memory write lock before folding the metric (same
        // rationale as `recall_facts_ranked`). Fold precision here too so the
        // `recall_precision_at_k` coverage does NOT silently drop to zero if a
        // future production caller is wired to this reinforced path instead of
        // the pure `recall_facts_ranked` (invariant #8 — every ranked fact
        // recall is measured).
        drop(guard);
        if let Some(p) = super::metrics::precision_at_k(query, &facts, facts.len()) {
            super::metrics::observe_recall_precision(super::metrics::RECALL_PRECISION_SITE, p);
        }
        Ok(facts)
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

        // Release the cognitive-memory write lock before the metric's synchronous
        // `metrics.jsonl` append: `measure_forget_outcome` was the last reader of
        // `guard`, and `record_forget_metric` needs only the copied counts, so
        // holding the lock across blocking file I/O would needlessly serialize
        // other memory ops behind a disk write.
        drop(guard);

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

    fn list_all_prospective(&self, limit: u32) -> SimardResult<Vec<CognitiveProspective>> {
        // Issue #2550: read-only enumeration of every prospective memory (all
        // statuses), priority-ordered, for the verified backup. Routes through
        // the library's `get_all_prospective` (a pure `&self` read over
        // `query_nodes(NT_PROSPECTIVE, agent_filter)`), so it neither mutates
        // status nor filters by content the way `check_triggers` does.
        Ok(self
            .lock()?
            .get_all_prospective(limit as usize)
            .into_iter()
            .map(to_prospective)
            .collect())
    }

    fn list_prospective_by_trigger(
        &self,
        trigger: &str,
        limit: u32,
    ) -> SimardResult<Vec<CognitiveProspective>> {
        // Issue #122: trigger-scoped enumeration of prospective memories, all
        // statuses, priority-ordered. Routes through the library's
        // `get_prospective_by_trigger`, which pushes the `trigger_condition`
        // equality filter into the node query so the `limit` bounds only
        // matching nodes — unlike `get_all_prospective`, whose window is
        // applied across every trigger. This is what keeps the creative-ideas
        // dashboard complete in a large store. **Fail-closed**: the library
        // returns a `Result`, so a genuine backend read error is propagated
        // (mapped onto `RpcCallFailed`), never masked as an empty `Ok`.
        Ok(self
            .lock()?
            .get_prospective_by_trigger(trigger, limit as usize)
            .map_err(|e| map_op_err("list_prospective_by_trigger", e))?
            .into_iter()
            .map(to_prospective)
            .collect())
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

    fn episode_exists(&self, node_id: &str) -> SimardResult<bool> {
        // Issue #2679: grounding primitive for the distillation write-boundary
        // gate. The library has no direct "does this episode id exist" lookup,
        // so we scan the same unfiltered enumeration `list_all_episodes` uses
        // (`get_episodes(_, true)`, newest-first, INCLUDING compressed episodes)
        // and short-circuit on the first id match. Compressed episodes are
        // included so a fact citing a consolidated source still grounds. A
        // grounding check is a read; it never mutates the store.
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Ok(false);
        }
        Ok(self
            .lock()?
            .get_episodes(usize::MAX, true)
            .iter()
            .any(|e| e.node_id == node_id))
    }

    fn any_episode_exists(&self, node_ids: &[String]) -> SimardResult<bool> {
        // Batch grounding for the write-boundary gate (issue #2679): materialize
        // the episode enumeration ONCE (the same unfiltered, compressed-inclusive
        // `get_episodes(_, true)` scan `episode_exists` uses) and test every
        // candidate id against it, instead of re-materializing the full set once
        // per cited id. Trims the per-fact grounding cost from O(cited·episodes)
        // to O(episodes). A grounding check is a read; it never mutates the store.
        let wanted: Vec<&str> = node_ids
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if wanted.is_empty() {
            return Ok(false);
        }
        Ok(self
            .lock()?
            .get_episodes(usize::MAX, true)
            .iter()
            .any(|e| wanted.iter().any(|w| e.node_id == *w)))
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

    fn list_all_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        // Issue #2550: unfiltered enumeration of every episode (including
        // compressed/consolidated ones), newest-first, for the verified backup.
        // `get_episodes(_, true)` is the same "return all" read
        // `search_episodes_by_keywords` builds on, minus the keyword gate.
        Ok(self
            .lock()?
            .get_episodes(limit as usize, true)
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
        // Partition the query keywords by shape — a recall-quality gate that
        // mirrors, for this flat keyword scan, the word-boundary relevance gate
        // `recall_episodes_ranked` already applies:
        //   * a CLEAN token (non-empty, every char alphanumeric — the shape the
        //     natural-language callers emit, e.g. `creative_ideas` ->
        //     "meeting"/"conversation"/"decision", and `tokenize_objective`) is
        //     matched at a WORD BOUNDARY via `shares_word_prefix`, so a token
        //     merely embedded in the interior/suffix of an unrelated content word
        //     ("test" in "latest", "own" in "download", "decision" in
        //     "indecision") no longer floats an off-topic episode into recall;
        //   * a keyword carrying ANY non-alphanumeric char — a phrase or,
        //     crucially, a bracketed provenance MARKER (`[reflect-occ=…]`,
        //     `[reflect-key=…|…]`) that `memory_consolidation::reflection_lessons`
        //     dedup relies on — keeps the exact case-insensitive SUBSTRING
        //     semantics its callers re-filter on (`content.contains(marker)`).
        // This closes the substring-recall gap on the clean-token callers while
        // preserving, by construction, the exact-marker substring path the flat
        // scan was deliberately kept on.
        let mut clean_needles: HashSet<String> = HashSet::new();
        let mut raw_needles: Vec<String> = Vec::new();
        for keyword in keywords {
            let lowered = keyword.to_lowercase();
            if lowered.is_empty() {
                continue;
            }
            if lowered.chars().all(char::is_alphanumeric) {
                // Drop a sub-threshold clean keyword (a lone character): as a
                // word-boundary PREFIX it matches nearly every content word, so it
                // is recall noise. RAW marker keywords are never length-cut.
                if lowered.len() >= MIN_CLEAN_NEEDLE_LEN {
                    clean_needles.insert(lowered);
                }
            } else {
                raw_needles.push(lowered);
            }
        }
        if clean_needles.is_empty() && raw_needles.is_empty() {
            return Ok(vec![]);
        }
        let has_clean = !clean_needles.is_empty();
        // Include compressed episodes so consolidation sources remain recallable
        // by keyword (matching native, whose query has no compressed filter).
        // `get_episodes` already returns newest-first by `temporal_index`.
        // `take(limit)` short-circuits the per-episode scan (and the DTO
        // conversion) once `limit` matches are found, instead of converting every
        // match and truncating afterwards. The `has_clean` guard skips the
        // word-boundary tokenization entirely on the marker-only path
        // (`reflection_lessons::count_recurring_failures` scans with
        // `limit = u32::MAX`), so that path does no extra work.
        let episodes: Vec<CognitiveEpisode> = self
            .lock()?
            .get_episodes(usize::MAX, true)
            .into_iter()
            .filter(|e| {
                let content = e.content.to_lowercase();
                (has_clean && shares_word_prefix(&content, &clean_needles))
                    || raw_needles.iter().any(|kw| content.contains(kw))
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
        //
        // The relevance gate is WORD-BOUNDARY, not raw substring: a query token
        // gates an episode in only when it is a prefix of a whole word in the
        // episode's content, not when it is merely *embedded* in the interior or
        // suffix of an unrelated word (`act` in "reactor" / "contract", `test`
        // in "latest", `own` in "download"). Substring matching floated such
        // off-topic episodes into the ranked set that feeds the OODA cycle's
        // working context, degrading recall precision. Anchoring to a word
        // boundary preserves the inflectional recall the path depends on
        // (`deploy` still recalls "deployed" / "deploys") while dropping the
        // interior/suffix noise — aligning episodic recall with the
        // word-boundary policy adopted by `knowledge_context::relevance_score`,
        // `memory_consolidation::classifier`, and `fact_reliability` (PR #4241
        // lineage). Tokenizing the query on non-alphanumeric runs (not only
        // whitespace) also folds any punctuation attached to a query token onto
        // its bare word so it can still match. Sub-threshold (single-char) clean
        // tokens are then dropped (MIN_CLEAN_NEEDLE_LEN) so a lone char like "s"
        // from "Rust's" cannot prefix-match every s-word — the same cut
        // `knowledge_context` applies to objective tokens.
        let needles: HashSet<String> = tokenize_words(query)
            .into_iter()
            .filter(|t| t.len() >= MIN_CLEAN_NEEDLE_LEN)
            .collect();
        if needles.is_empty() {
            return Ok(vec![]);
        }
        let matches_kw = |content: &str| shares_word_prefix(content, &needles);

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

#[cfg(test)]
mod word_boundary_gate_tests {
    use super::{shares_word_prefix, tokenize_words};

    #[test]
    fn tokenize_words_folds_case_and_punctuation() {
        let words = tokenize_words("Deploy, the durable-recall PATH!");
        // Lowercased, split on every non-alphanumeric run (so the comma, the
        // hyphen and the trailing `!` are separators), empties dropped.
        for expected in ["deploy", "the", "durable", "recall", "path"] {
            assert!(
                words.contains(expected),
                "missing {expected:?} in {words:?}"
            );
        }
        assert_eq!(words.len(), 5, "distinct words only: {words:?}");
    }

    #[test]
    fn tokenize_words_empty_for_no_alphanumerics() {
        assert!(tokenize_words("   ,.;-—  ").is_empty());
        assert!(tokenize_words("").is_empty());
    }

    #[test]
    fn shares_word_prefix_rejects_interior_and_suffix_embedding() {
        let needles = tokenize_words("act test own");
        // Every match would be interior/suffix, not a word-boundary prefix.
        assert!(!shares_word_prefix(
            "the reactor signed a contract",
            &needles
        ));
        assert!(!shares_word_prefix("the latest greatest contest", &needles));
        assert!(!shares_word_prefix("download the file", &needles));
    }

    #[test]
    fn shares_word_prefix_accepts_word_boundary_match() {
        let needles = tokenize_words("act log own");
        assert!(shares_word_prefix("we act on the alert", &needles));
        assert!(shares_word_prefix("check the log file", &needles));
        assert!(shares_word_prefix("this is my own note", &needles));
    }

    #[test]
    fn shares_word_prefix_preserves_inflectional_recall() {
        // A query stem must still recall its inflected forms — the recall the
        // live OODA path depends on (a pure whole-word/equality gate would drop
        // these).
        let needles = tokenize_words("deploy");
        assert!(shares_word_prefix("deployed the payment service", &needles));
        assert!(shares_word_prefix("two deploys ran today", &needles));
        assert!(shares_word_prefix("the deployment succeeded", &needles));
        // But an unrelated interior embedding of the stem is still rejected.
        assert!(!shares_word_prefix("we redeployed nothing new", &needles));
    }

    #[test]
    fn shares_word_prefix_is_case_insensitive_and_punctuation_tolerant() {
        let needles = tokenize_words("authentication");
        // ALL-CAPS content, punctuation-attached word — still a boundary match.
        assert!(shares_word_prefix(
            "DEPLOYED THE AUTHENTICATION, OK",
            &needles
        ));
    }

    #[test]
    fn shares_word_prefix_false_on_empty_content() {
        let needles = tokenize_words("deploy");
        assert!(!shares_word_prefix("", &needles));
        assert!(!shares_word_prefix("   ...   ", &needles));
    }

    #[test]
    fn shares_word_prefix_folds_plural_query_onto_singular_content() {
        // The asymmetry a prefix-only gate leaves open: a PLURAL query token must
        // recall the SINGULAR-form content word. Prefix alone misses these
        // (`"test".starts_with("tests")` is false); the conservative singular
        // fold closes it.
        assert!(shares_word_prefix(
            "wrote a test for the parser",
            &tokenize_words("tests")
        ));
        assert!(shares_word_prefix(
            "warmed the cache on startup",
            &tokenize_words("caches")
        ));
        assert!(shares_word_prefix(
            "the box was shipped",
            &tokenize_words("boxes")
        ));
    }

    #[test]
    fn shares_word_prefix_folds_y_ies_both_directions() {
        // The `-y` ↔ `-ies` pair changes the stem, so the prefix branch misses it
        // in BOTH directions; the fold handles each.
        assert!(shares_word_prefix(
            "the category was wrong",
            &tokenize_words("categories")
        ));
        assert!(shares_word_prefix(
            "ran several categories of checks",
            &tokenize_words("category")
        ));
    }

    #[test]
    fn shares_word_prefix_fold_does_not_reintroduce_interior_matching() {
        // Folding matches only on whole-word EQUALITY of a generated variant, so a
        // stripped stem must NOT prefix-match an unrelated longer word: the stem
        // "bus" (from the plural "buses") must not surface "business".
        assert!(!shares_word_prefix(
            "the business plan shipped",
            &tokenize_words("buses")
        ));
        // A plural query must not fold onto a same-prefix but distinct word:
        // "tests" (stem "test") must not gate in "testing".
        assert!(!shares_word_prefix(
            "the testing harness is flaky",
            &tokenize_words("tests")
        ));
    }

    #[test]
    fn shares_word_prefix_short_token_never_folds_to_fragment() {
        // A short token cannot collapse onto a one/zero-character stem: `"is"`
        // must not fold to `"i"` and gate in an episode that merely contains "i".
        assert!(!shares_word_prefix("i wrote it", &tokenize_words("is")));
    }
}

#[cfg(test)]
mod fact_query_gate_tests {
    use super::{fact_shares_query_relevance, partition_fact_query};

    #[test]
    fn partition_splits_clean_and_raw_tokens() {
        // Clean natural-language words fold to lowercase in the clean set; a
        // hyphenated concept and a colon marker land in the raw set verbatim
        // (lowercased); empty tokens from repeated separators are dropped.
        let n = partition_fact_query("Reactor  bug-pattern journal:2026-07-18 OWNS");
        assert!(n.clean.contains("reactor"));
        assert!(n.clean.contains("owns"));
        assert_eq!(n.clean.len(), 2, "clean: {:?}", n.clean);
        assert!(n.raw.contains(&"bug-pattern".to_string()));
        assert!(n.raw.contains(&"journal:2026-07-18".to_string()));
        assert_eq!(n.raw.len(), 2, "raw: {:?}", n.raw);
    }

    #[test]
    fn clean_token_gate_rejects_interior_and_suffix_but_keeps_word_boundary() {
        // `act` must not float a fact in on the interior of "reactor"/"artifact"
        // or the suffix of a word, but must keep a genuine word-boundary hit.
        let n = partition_fact_query("act");
        assert!(!fact_shares_query_relevance(
            "bug-pattern",
            "the reactor overheated badly",
            &n
        ));
        assert!(!fact_shares_query_relevance(
            "bug-pattern",
            "download the latest artifact",
            &n
        ));
        assert!(fact_shares_query_relevance(
            "bug-pattern",
            "act quickly on failures",
            &n
        ));
    }

    #[test]
    fn clean_token_gate_preserves_inflectional_recall() {
        // A stem still recalls its inflected forms (the live-path recall a pure
        // whole-word gate would drop).
        let n = partition_fact_query("deploy");
        assert!(fact_shares_query_relevance(
            "lesson-learned",
            "deployed the payment service",
            &n
        ));
        assert!(fact_shares_query_relevance(
            "lesson-learned",
            "two deploys ran today",
            &n
        ));
    }

    #[test]
    fn clean_token_gate_matches_on_concept_field_too() {
        // The library matches a query against concept AND content, so a clean
        // token that is a word-boundary prefix of the CONCEPT keeps the fact even
        // when the content shares nothing.
        let n = partition_fact_query("pr");
        assert!(fact_shares_query_relevance(
            "pr-pattern",
            "unrelated body text",
            &n
        ));
    }

    #[test]
    fn raw_token_gate_preserves_marker_substring_semantics() {
        // A colon marker keeps the library's exact substring lookup — matched
        // against either field — so marker/concept callers are unaffected.
        let n = partition_fact_query("journal:2026-07-18");
        assert!(n.clean.is_empty());
        assert!(fact_shares_query_relevance(
            "journal:2026-07-18",
            "{\"body\":\"...\"}",
            &n
        ));
        assert!(!fact_shares_query_relevance(
            "journal:2026-07-19",
            "unrelated",
            &n
        ));
    }

    #[test]
    fn mixed_query_keeps_fact_via_either_clean_or_raw_token() {
        // A mixed query gates the clean token at a word boundary while still
        // honoring the raw marker substring.
        let n = partition_fact_query("reactor goal-edge:blocks");
        // Kept via the clean word-boundary token.
        assert!(fact_shares_query_relevance(
            "bug-pattern",
            "the reactor tripped",
            &n
        ));
        // Kept via the raw marker substring even though the clean token misses.
        assert!(fact_shares_query_relevance(
            "goal-edge:blocks",
            "edge payload",
            &n
        ));
        // Dropped: clean token only interior-embeds and no raw token matches.
        assert!(!fact_shares_query_relevance(
            "bug-pattern",
            "subreactor note",
            &n
        ));
    }

    #[test]
    fn partition_drops_sub_threshold_clean_tokens() {
        // Single-char clean tokens (a possessive fragment, an initial, a stray
        // separator) are recall noise on the word-boundary prefix gate, so they
        // are dropped; a two-or-more-char clean token is kept.
        let n = partition_fact_query("s rust a session");
        assert!(n.clean.contains("rust"));
        assert!(n.clean.contains("session"));
        assert_eq!(
            n.clean.len(),
            2,
            "single-char clean tokens must be dropped, clean: {:?}",
            n.clean
        );
        assert!(n.raw.is_empty(), "raw: {:?}", n.raw);
    }

    #[test]
    fn partition_never_length_cuts_raw_tokens() {
        // The cut applies only to CLEAN tokens — RAW tokens keep exact substring
        // semantics regardless of length (a short colon/hyphen marker survives).
        let n = partition_fact_query("x: y-z");
        assert!(n.clean.is_empty(), "clean: {:?}", n.clean);
        assert!(n.raw.contains(&"x:".to_string()), "raw: {:?}", n.raw);
        assert!(n.raw.contains(&"y-z".to_string()), "raw: {:?}", n.raw);
    }

    #[test]
    fn sub_threshold_only_query_matches_nothing() {
        // A query of only sub-threshold clean tokens leaves both needle sets
        // empty, so the gate matches nothing — the `search_facts` caller turns
        // this into an empty recall rather than a raw-substring flood where the
        // lone char matches every fact containing an s-word.
        let n = partition_fact_query("s");
        assert!(n.clean.is_empty() && n.raw.is_empty());
        assert!(!fact_shares_query_relevance(
            "session",
            "the s3 storage layer synced",
            &n
        ));
    }
}
