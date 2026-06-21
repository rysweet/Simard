//! Session lifecycle phases mapped to cognitive memory operations.
//!
//! Each session phase (intake, preparation, execution, reflection, persistence)
//! triggers specific memory operations that progressively build and refine the
//! agent's cognitive state. This module provides the mapping functions.

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::goals::{GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, GoalRecord};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective,
};
use crate::session::SessionId;

/// Context assembled during the preparation phase for use during execution.
///
/// Contains the relevant facts, triggered prospective memories, recalled
/// procedures, and recent similar episodes that the agent should consider
/// when executing the session objective.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedContext {
    pub relevant_facts: Vec<CognitiveFact>,
    pub triggered_prospectives: Vec<CognitiveProspective>,
    pub recalled_procedures: Vec<CognitiveProcedure>,
    /// PR-C (issue #2281, problem 4): episodes with at least one
    /// keyword overlap with the objective, filtered to drop
    /// self-session noise. Defaults to empty (no recall) when the
    /// objective has no usable tokens or the bridge returns nothing.
    #[serde(default)]
    pub episodic_recall: Vec<CognitiveEpisode>,
}

/// A fact extracted during the reflection phase.
///
/// Reflection inspects the session transcript and extracts factual knowledge
/// that should be stored in semantic memory for future sessions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FactExtraction {
    pub concept: String,
    pub content: String,
    pub confidence: f64,
}

// ============================================================================
// Phase operations
// ============================================================================

/// Intake phase: record the session objective as sensory input and push it
/// into working memory.
///
/// This is the first thing that happens when a new session starts. The
/// objective is recorded as a sensory observation (modality "objective") and
/// pushed into working memory so that subsequent phases can reference it.
#[tracing::instrument(skip_all)]
pub fn intake_memory_operations(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // Record the raw objective as a sensory observation (5 min TTL).
    bridge.record_sensory("objective", objective, 300)?;

    // Push the objective into working memory for this session.
    bridge.push_working("objective", objective, session_id.as_str(), 1.0)?;

    // Store as an episodic event so we have a record of what was asked —
    // routed through the ingestion classifier (issue #2327). The session-start
    // marker is operational noise and is dropped unless it carries a failure
    // summary.
    classifier::store_episode_classified(
        bridge,
        &format!("Session {session_id} started with objective: {objective}"),
        "session-intake",
        &classifier::IntakeContext::default(),
    )?;

    Ok(())
}

/// Preparation phase: gather relevant context from long-term memory.
///
/// Searches semantic memory for facts related to the objective, checks
/// prospective memories for any triggered actions, and recalls relevant
/// procedures. Also explicitly loads active goal records from semantic
/// memory so goals are always available in the prepared context regardless
/// of whether the objective text happens to match (issue #2207).
///
/// This is the legacy 3-argument entry point retained for backward
/// compatibility. It applies the goal-board:snapshot filter (PR-A
/// filter 1, issue #2281) but does **not** apply the stale-slug
/// filter (PR-A filter 2) because that filter requires the live
/// goal-board state to know which slugs are active. New callers
/// should use [`preparation_memory_operations_with_active_slugs`]
/// instead so stale `goal-store:record` facts are dropped.
#[tracing::instrument(skip_all)]
pub fn preparation_memory_operations(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<PreparedContext> {
    preparation_memory_operations_with_active_slugs(
        objective, session_id, bridge,
        // `None` opts out of the stale-slug filter so existing test
        // fixtures (`tests.rs`) that exercise the goal-fact dedup
        // path keep passing unchanged.
        None,
    )
}

/// Preparation phase, taking the live active+backlog slugs so stale
/// `goal-store:record` facts can be filtered out.
///
/// `active_slugs` should be the union of `state.active_goals.active`
/// and `state.active_goals.backlog` ids built immediately before this
/// call from the **live** goal-board (not from snapshot facts). Pass
/// `Some(&empty)` to drop all `goal-store:record` facts (correct when
/// there are no live goals); pass `None` to skip the stale filter
/// entirely (backwards-compat path used by `preparation_memory_operations`).
///
/// Always applies the `goal-board:snapshot` filter (PR-A filter 1)
/// regardless of `active_slugs`: snapshot revisions are pure
/// redundancy because `advance.rs` already injects the live goal
/// board into the prompt.
///
/// Issue [#2281](https://github.com/rysweet/Simard/issues/2281), PR-A.
#[tracing::instrument(skip_all)]
pub fn preparation_memory_operations_with_active_slugs(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
    active_slugs: Option<&std::collections::HashSet<&str>>,
) -> SimardResult<PreparedContext> {
    // Split compound objectives (joined with "; ") into individual fragments
    // and search each separately. The old code passed the full joined string to
    // search_facts() which uses Cypher CONTAINS — no fact matches a giant
    // concatenated string. Issue #2270.
    let fragments: Vec<&str> = objective
        .split("; ")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let mut relevant_facts: Vec<CognitiveFact> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for fragment in &fragments {
        let per_fragment = bridge.search_facts(fragment, 10, 0.0)?;
        for fact in per_fragment {
            // PR-A filter 1: drop goal-board:snapshot revisions even
            // when they surface from a per-fragment match. The live
            // board is already injected by `advance.rs`.
            if fact.concept == GOAL_BOARD_SNAPSHOT_CONCEPT {
                continue;
            }
            if seen_ids.insert(fact.node_id.clone()) {
                relevant_facts.push(fact);
            }
        }
    }
    // Cap total results at 10 to match the original per-query limit.
    relevant_facts.truncate(10);

    // Always load goal facts so goals are accessible from memory even when
    // the objective text doesn't substring-match "goal-store:record".
    // Uses the same limit as CognitiveMemoryGoalStore::list_via_reader()
    // so status churn doesn't cause current goals to fall off (#2207).
    let goal_facts = bridge.search_facts(GOAL_STORE_FACT_CONCEPT, GOAL_STORE_LIST_LIMIT, 0.0)?;

    // Dedup goal facts by slug, keeping only the latest revision per slug
    // (highest node_id, which is UUID-v7 time-ordered). This mirrors the
    // dedup logic in CognitiveMemoryGoalStore::list_via_reader() and
    // prevents historical revisions from crowding out current goals (#2207).
    let mut latest_by_slug: std::collections::HashMap<String, (String, CognitiveFact)> =
        std::collections::HashMap::new();
    for fact in goal_facts {
        if fact.concept != GOAL_STORE_FACT_CONCEPT {
            continue;
        }
        let record: GoalRecord = match serde_json::from_str(&fact.content) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "[simard] preparation: skipping unparseable goal fact \
                     (node_id={}): {e}",
                    fact.node_id
                );
                continue;
            }
        };
        let slug = record.slug;
        match latest_by_slug.get(&slug) {
            Some((_, existing)) if existing.node_id >= fact.node_id => {}
            _ => {
                latest_by_slug.insert(slug.clone(), (slug, fact));
            }
        }
    }

    let existing_ids: std::collections::HashSet<String> =
        relevant_facts.iter().map(|f| f.node_id.clone()).collect();
    let mut dropped_stale: usize = 0;
    for (slug, fact) in latest_by_slug.into_values() {
        // PR-A filter 2 (issue #2281): drop goal-store:record facts
        // whose slug is not in the live active+backlog goal-board.
        // Skipped when `active_slugs.is_none()` — backward-compat for
        // the 3-arg `preparation_memory_operations` entry point.
        if let Some(slugs) = active_slugs
            && !slugs.contains(slug.as_str())
        {
            dropped_stale += 1;
            continue;
        }
        if !existing_ids.contains(&fact.node_id) {
            relevant_facts.push(fact);
        }
    }
    if dropped_stale > 0 {
        tracing::debug!(
            dropped = dropped_stale,
            "preparation: dropped stale goal-store:record facts (slug not in active goal-board)"
        );
    }

    // Check if any prospective memories are triggered by the objective.
    let triggered_prospectives = bridge.check_triggers(objective)?;

    // PR-C (issue #2281, problem 3 + 4): both procedural and episodic
    // recall benefit from breaking the objective into trigger
    // keywords first. `recall_procedure` is a single-CONTAINS query
    // on Procedure.name, so a multi-token objective like
    // `"merge PR #2281"` never matches `pr-merge:bootstrap | triggers: …`
    // unless we issue one query per token. The bootstrap procedures
    // (problem 3) are useless without this fan-out.
    let tokens = tokenize_objective(objective);

    // Recall procedures via the unified tokenized helper (ws2 #2295).
    // Both bootstrap procedures (`*-bootstrap`) seeded by
    // `seed_bootstrap_procedures` and distilled procedures emitted each
    // cycle by `ooda_loop::cycle::compose_procedure_name` go through the
    // exact same `bridge.recall_procedure(token, …)` Cypher CONTAINS path
    // so neither class can win or lose recall relative to the other.
    let recalled_procedures =
        recall_procedures_for_objective_with_tokens(bridge, objective, &tokens, 5)?;

    // PR-C (issue #2281, problem 4): episodic recall.
    //
    // Tokenize the objective into trigger keywords, ask the bridge for
    // up to 5 recent matching episodes, then filter out self-session
    // noise (`source_label.starts_with("session-")`) which is just the
    // current session loop's own breath echoing back into the prompt.
    //
    // When the tokenizer produces no usable keywords we skip the
    // bridge call entirely — there is no cheap "match anything"
    // fallback and a no-token query would surface arbitrary recent
    // episodes that bear no relation to the objective.
    let (raw_recall_count, session_filtered_count, episodic_recall) = if tokens.is_empty() {
        (0usize, 0usize, Vec::<CognitiveEpisode>::new())
    } else {
        let raw = bridge.search_episodes_by_keywords(&tokens, 5)?;
        let raw_len = raw.len();
        let kept: Vec<CognitiveEpisode> = raw
            .into_iter()
            .filter(|e| !e.source_label.starts_with("session-"))
            .collect();
        let filtered = raw_len - kept.len();
        (raw_len, filtered, kept)
    };

    // Push a summary of what we found into working memory.
    let context_summary = format!(
        "Prepared context: {} facts, {} triggers, {} procedures, {} episodes",
        relevant_facts.len(),
        triggered_prospectives.len(),
        recalled_procedures.len(),
        episodic_recall.len(),
    );
    bridge.push_working(
        "context-summary",
        &context_summary,
        session_id.as_str(),
        0.8,
    )?;

    eprintln!(
        "[simard] preparation: {} procedures, {} episodes recalled ({} raw, {} session-filtered)",
        recalled_procedures.len(),
        episodic_recall.len(),
        raw_recall_count,
        session_filtered_count,
    );

    Ok(PreparedContext {
        relevant_facts,
        triggered_prospectives,
        recalled_procedures,
        episodic_recall,
    })
}

/// Concept label for goal-board snapshot facts, filtered by PR-A
/// (issue #2281). Snapshot revisions duplicate the live board that
/// `advance.rs` already injects into the prompt.
pub(crate) const GOAL_BOARD_SNAPSHOT_CONCEPT: &str = "goal-board:snapshot";

/// Common English stopwords dropped during objective tokenization
/// for episodic recall (PR-C, issue #2281, problem 4). These tokens
/// add zero signal to a `CONTAINS` search and would only inflate
/// the OR-clause without changing the recall set.
const TOKEN_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "has", "was", "were", "will", "into",
    "when", "where", "what", "why", "how",
];

/// Tokenize the objective text into keywords for
/// [`CognitiveMemoryOps::search_episodes_by_keywords`] **and** for
/// procedural recall via [`recall_procedures_for_objective`].
///
/// Steps (in order):
/// 1. Split on non-alphanumeric runs (so `#2281` becomes `2281`,
///    `src/foo.rs` becomes `src foo rs`, etc.).
/// 2. Lowercase each token.
/// 3. Drop tokens shorter than 3 characters.
/// 4. Drop common English stopwords (`the, and, for, …`).
/// 5. Deduplicate while preserving first-seen order.
///
/// Returns an empty vec when the objective produces no usable
/// tokens; callers should skip the bridge call in that case
/// (per the episodic-recall spec — no "match anything" fallback).
///
/// Pub-crate so [`crate::base_type_turn`] can share the same tokenizer
/// and procedural recall as the OODA cycle preparation phase. The
/// 3-character minimum is the **read-side floor** that the write-side
/// `derive_triggers_from_objective` aligns against — see
/// `docs/reference/cognitive-memory-bootstrap-procedures.md` for the
/// full contract.
pub(crate) fn tokenize_objective(objective: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let lowered = objective.to_ascii_lowercase();
    for raw in lowered.split(|c: char| !c.is_ascii_alphanumeric()) {
        if raw.len() < 3 {
            continue;
        }
        if TOKEN_STOPWORDS.contains(&raw) {
            continue;
        }
        if seen.insert(raw.to_string()) {
            out.push(raw.to_string());
        }
    }
    out
}

/// Unified procedural-recall helper used by both the OODA cycle's
/// preparation phase ([`preparation_memory_operations_with_active_slugs`])
/// and the base-type-adapter turn preparation
/// ([`crate::base_type_turn::prepare_turn_context`]).
///
/// **Why a shared helper.** Before ws2 #2295, the two call sites used
/// different recall strategies:
///
/// * Preparation phase tokenized the objective and fanned out one
///   `recall_procedure` call per token (PR-C, #2281), which surfaces
///   both bootstrap procedures (`pr-merge:bootstrap`, …) and distilled
///   procedures written by the OODA cycle's
///   [`crate::ooda_loop::cycle::compose_procedure_name`].
/// * Base-type adapters passed the **entire raw objective** to a single
///   `bridge.recall_procedure(objective, 5)` call. The Cypher
///   `name CONTAINS '<full objective>'` clause never matched any
///   stored procedure because no procedure name embeds a natural
///   sentence. Effect: zero distilled procedures ever reached the
///   prompt unless the operator's prompt happened to literally
///   contain a procedure name.
///
/// Unifying both sites here means the same set of procedures surfaces
/// regardless of which adapter is driving the turn, and prevents a
/// future divergence by giving callers one obvious entry point.
///
/// **Case-folding contract.** [`tokenize_objective`] lowercases every
/// token; both [`crate::cognitive_memory::bootstrap_procedures::BOOTSTRAP_PROCEDURES`]
/// and [`crate::ooda_loop::cycle::compose_procedure_name`] emit names
/// whose trigger portion is already lowercase. The Cypher `CONTAINS`
/// operator is case-sensitive, so the all-lowercase invariant on both
/// sides is what makes recall actually fire.
///
/// **Empty-token fallback.** When the objective produces no tokens
/// of three or more chars (very short or punctuation-only input), we
/// issue a single `recall_procedure(objective, max)` call so callers
/// that pass a pre-tokenized or exact-name query keep working.
pub fn recall_procedures_for_objective(
    bridge: &dyn CognitiveMemoryOps,
    objective: &str,
    max: u32,
) -> SimardResult<Vec<CognitiveProcedure>> {
    let tokens = tokenize_objective(objective);
    recall_procedures_for_objective_with_tokens(bridge, objective, &tokens, max)
}

/// Token-aware inner form of [`recall_procedures_for_objective`].
///
/// Lets the OODA preparation phase reuse the same tokenization it
/// already computed for episodic recall instead of paying for a second
/// pass over the objective string.
pub(crate) fn recall_procedures_for_objective_with_tokens(
    bridge: &dyn CognitiveMemoryOps,
    objective: &str,
    tokens: &[String],
    max: u32,
) -> SimardResult<Vec<CognitiveProcedure>> {
    if tokens.is_empty() {
        // Empty-token fallback (see doc on
        // `recall_procedures_for_objective`).
        let mut hits = bridge.recall_procedure(objective, max)?;
        hits.truncate(max as usize);
        return Ok(hits);
    }

    let per_token_cap = max;
    let mut by_id: std::collections::HashMap<String, CognitiveProcedure> =
        std::collections::HashMap::new();
    for tok in tokens {
        let hits = bridge.recall_procedure(tok, per_token_cap)?;
        for p in hits {
            by_id.entry(p.node_id.clone()).or_insert(p);
        }
    }
    let mut out: Vec<CognitiveProcedure> = by_id.into_values().collect();
    // Highest usage_count first, then name, then node_id for a fully
    // deterministic order. Procedure names are NOT unique (only the
    // graph `id` is) — repeated OODA cycles can store the same composed
    // name under different `node_id`s. Without the `node_id` tiebreaker a
    // `usage_count`+`name` tie would fall through to unordered
    // `HashMap::into_values()` iteration, so `truncate` could keep a
    // different procedure across runs and silently vary prompt contents.
    out.sort_by(|a, b| {
        b.usage_count
            .cmp(&a.usage_count)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    out.truncate(max as usize);

    // Structured tracing event so the journal records exactly which
    // procedure names surfaced. The previous summary-count-only log
    // line ("prepared context (… N procedures …)") hid which
    // procedures actually fired, which made it impossible for
    // operators to tell whether distilled procedures were being
    // recalled or only the bootstrap set. Names are emitted as a
    // single comma-separated structured field so a downstream JSON
    // log layer captures each name in full (no per-field truncation).
    if !out.is_empty() {
        let names: Vec<&str> = out.iter().map(|p| p.name.as_str()).collect();
        tracing::info!(
            procedure_count = out.len(),
            tokens = ?tokens,
            procedure_names = %names.join(" | "),
            "recalled procedures for objective",
        );
    } else {
        tracing::debug!(
            tokens = ?tokens,
            "no procedures recalled for objective",
        );
    }

    Ok(out)
}

/// Execution phase: record PTY output as sensory observations.
///
/// During execution, the agent interacts with the terminal. Each chunk of
/// output is recorded as a sensory observation so that it can be attended
/// to if noteworthy.
#[tracing::instrument(skip_all)]
pub fn execution_memory_operations(
    pty_output: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // Record the output as a sensory observation (short TTL since it is
    // transient terminal output).
    bridge.record_sensory("pty-output", pty_output, 120)?;

    // Push a truncated version into working memory for immediate context.
    // Use char-boundary-safe truncation to avoid panic on multi-byte UTF-8.
    let truncated = if pty_output.len() > 500 {
        let boundary = pty_output
            .char_indices()
            .take_while(|(i, _)| *i < 500)
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
        format!("{}...[truncated]", &pty_output[..boundary])
    } else {
        pty_output.to_string()
    };
    bridge.push_working("execution-output", &truncated, session_id.as_str(), 0.6)?;

    Ok(())
}

/// Reflection phase: extract facts and store the session transcript.
///
/// After execution completes, the agent reflects on what happened. The
/// transcript is stored as an episodic memory, and any extracted facts
/// are stored in semantic memory.
#[tracing::instrument(skip_all)]
pub fn reflection_memory_operations(
    transcript: &str,
    facts: &[FactExtraction],
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // Store the session transcript as an episodic memory, capturing its id so
    // each fact derived from this transcript can link back to it (issue #2325).
    //
    // Issue #2327 (A1/A2): the transcript is sanitized of `continue_skipping` /
    // no-decision-keyword noise before storage. A pure-noise transcript with no
    // derived facts is dropped entirely; otherwise the (cleaned) transcript is
    // stored. When facts ARE derived we keep the episode unconditionally — even
    // pure noise — because its id is the provenance anchor for those facts.
    let ctx = classifier::IntakeContext::default();
    let has_facts = !facts.is_empty();
    let sanitized = classifier::sanitize_transcript(transcript);
    let episode_id = match (&sanitized, has_facts) {
        // Pure noise and nothing derived from it → drop the reflection episode.
        (None, false) => {
            classifier::global_intake_counters().record(&classifier::IntakeDecision::Drop);
            return Ok(());
        }
        _ => {
            let body = sanitized.as_deref().unwrap_or(transcript);
            let content = format!("Session {session_id} transcript: {body}");
            if has_facts {
                // Provenance anchor required — store even if down-scoped.
                Some(classifier::store_episode_for_provenance(
                    bridge,
                    &content,
                    "session-reflection",
                    &ctx,
                )?)
            } else {
                classifier::store_episode_classified(bridge, &content, "session-reflection", &ctx)?
            }
        }
    };
    // No episode (classified Drop) and no facts to link — nothing more to do.
    let Some(episode_id) = episode_id else {
        return Ok(());
    };

    // Store each extracted fact in semantic memory, deduplicating by concept
    // both within this session and across prior sessions.
    let mut seen_concepts = std::collections::HashSet::<&str>::new();
    for fact in facts {
        if !seen_concepts.insert(fact.concept.as_str()) {
            continue;
        }
        // Cross-session dedup: skip if an existing fact has >= confidence.
        let existing = bridge
            .search_facts(&fact.concept, 5, fact.confidence)
            .unwrap_or_default();
        if existing.iter().any(|f| f.confidence >= fact.confidence) {
            continue;
        }
        // Provenance write (#2325): thread the transcript episode id so a
        // `DERIVES_FROM` edge links this fact back to the transcript it was
        // reflected from, instead of the legacy no-provenance `store_fact`.
        bridge.store_fact_with_provenance(
            &fact.concept,
            &fact.content,
            fact.confidence,
            &format!("session:{session_id}"),
            None,
            None,
            std::slice::from_ref(&episode_id),
        )?;
    }

    Ok(())
}

/// Persistence phase: clean up working memory and attempt episode consolidation.
///
/// This is the final phase of a session. Working memory for this session is
/// cleared, expired sensory items are pruned, and episode consolidation is
/// attempted to keep episodic memory compact.
///
/// A JSON snapshot is also saved to the default snapshot directory
/// (`~/.simard/snapshots/`) so cross-session recall survives process exit.
/// Snapshot save failures are **propagated** (issue #1604, gap G10) so that
/// disk-full / permission errors fail loudly rather than silently degrading
/// durability.
#[tracing::instrument(skip_all)]
pub fn persistence_memory_operations(
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    persistence_memory_operations_with_snapshot_dir(session_id, bridge, None)
}

/// Same as [`persistence_memory_operations`] but allows callers (typically
/// tests) to override the snapshot directory.  When `snapshot_dir_override`
/// is `None`, the default location (`~/.simard/snapshots/`) is used.
///
/// Snapshot save errors are propagated via `?` (issue #1604, gap G10).
#[tracing::instrument(skip_all)]
pub fn persistence_memory_operations_with_snapshot_dir(
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
    snapshot_dir_override: Option<&std::path::Path>,
) -> SimardResult<()> {
    // Consolidate episodes (batch of 10) BEFORE clearing working memory, so a
    // consolidation failure aborts teardown rather than silently dropping the
    // session's working-memory contents. Errors are propagated.
    bridge.consolidate_episodes(10)?;

    // Clear working memory for this session.
    bridge.clear_working(session_id.as_str())?;

    // Prune expired sensory items.
    bridge.prune_expired_sensory()?;

    // Store a final episodic memory marking session end — routed through the
    // ingestion classifier (issue #2327). The "completed and persisted" marker
    // is operational noise and is dropped unless it carries a failure summary.
    classifier::store_episode_classified(
        bridge,
        &format!("Session {session_id} completed and persisted"),
        "session-persistence",
        &classifier::IntakeContext::default(),
    )?;

    // Save a JSON snapshot for durable cross-session recall.  Errors are
    // PROPAGATED so the operator can fix the underlying disk/permission
    // issue (issue #1604, gap G10).  Previously these errors were swallowed
    // via `eprintln!`, which is exactly the silent-degradation bug class
    // that #1427 was filed against.
    if let Some(dir) = crate::memory_snapshot::snapshot_dir(snapshot_dir_override) {
        let path =
            crate::memory_snapshot::save_session_snapshot(bridge, session_id.as_str(), &dir)?;
        tracing::info!(path = %path.display(), "memory_snapshot: saved");
        // Prune: keep only the 10 most recent snapshots.
        prune_snapshots(&dir, 10);
    } else {
        // `snapshot_dir` returned `None`.  When the caller supplied an
        // explicit override and we still got `None`, that is a hard error
        // (the override was unusable).  Otherwise it just means the home
        // directory could not be resolved — log and continue so headless
        // environments without `$HOME` are not broken.
        if let Some(override_path) = snapshot_dir_override {
            return Err(crate::error::SimardError::PersistentStoreIo {
                store: "memory_snapshot".to_string(),
                action: "snapshot_dir".to_string(),
                path: override_path.to_path_buf(),
                reason: "snapshot_dir() returned None for the supplied override".to_string(),
            });
        }
        tracing::info!("memory_snapshot: home directory not resolved — skipping save");
    }

    Ok(())
}

/// Delete all but the `keep` most-recent snapshot files in `dir`.
///
/// Filenames are `<agent>-<epoch>.json`; lexicographic sort == chronological.
/// Errors during deletion are logged via `tracing::warn!` (issue #1604,
/// gap G11) so a stuck pruner is detectable through the existing
/// `tracing-subscriber` / `tracing-opentelemetry` pipeline.  Errors are
/// intentionally not propagated: leaving stale snapshots is preferable to
/// failing the session teardown.
fn prune_snapshots(dir: &std::path::Path, keep: usize) {
    let mut entries: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| {
                let e = e.ok()?;
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("json") {
                    Some(p)
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "memory_snapshot: prune read_dir failed (non-fatal)",
            );
            return;
        }
    };
    if entries.len() <= keep {
        return;
    }
    entries.sort();
    let to_delete = entries.len() - keep;
    for path in entries.iter().take(to_delete) {
        if let Err(e) = std::fs::remove_file(path) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "memory_snapshot: prune delete failed (non-fatal)",
            );
        }
    }
}

// ============================================================================
// Session-boundary auto-trigger helpers
// ============================================================================

/// Hydrate memories from prior sessions at startup.
///
/// Call this early in the session lifecycle (e.g. after `intake_memory_operations`)
/// to pull any cross-session facts into the current working context.  The
/// bridge is queried for recent facts and any matching records are pushed
/// into working memory so the agent can reason over prior session knowledge.
pub fn consolidation_intake(
    session_id: &SessionId,
    objective: &str,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<usize> {
    let prior_facts = bridge.search_facts(objective, 50, 0.0)?;
    let count = prior_facts.len();
    if count > 0 {
        let summary = format!("Hydrated {count} prior-session facts for cross-session recall");
        bridge.push_working("consolidation-intake", &summary, session_id.as_str(), 0.7)?;
        // Cross-session hydration bookkeeping is operational: the classifier
        // down-scopes it (low importance, is_operational = true) rather than
        // storing it at full importance (issue #2327).
        classifier::store_episode_classified(
            bridge,
            &summary,
            "consolidation-intake",
            &classifier::IntakeContext::default(),
        )?;
    }
    Ok(count)
}

/// Flush working memory to episodes at shutdown.
///
/// Call this during session cleanup (e.g. before `persistence_memory_operations`)
/// to ensure any remaining working-memory items are persisted as episodes
/// before the session terminates.  This closes the intake→persistence
/// round-trip and prevents data loss on unexpected shutdown.
pub fn consolidation_persistence(
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // Drain all working-memory slots into episodic store so they survive
    // session teardown.  Each slot is written as an episode using its
    // slot_type as the source label, preserving the memory category — routed
    // through the ingestion classifier so per-slot noise is dropped/down-scoped
    // by content (issue #2327).
    let ctx = classifier::IntakeContext::default();
    let slots = bridge.get_working(session_id.as_str())?;
    for slot in &slots {
        classifier::store_episode_classified(bridge, &slot.content, &slot.slot_type, &ctx)?;
    }

    // Store an episodic record capturing the consolidation event. The
    // "flushing working memory" marker is operational noise and is dropped
    // unless it carries a failure summary (issue #2327).
    classifier::store_episode_classified(
        bridge,
        &format!("Session {session_id} flushing working memory to episodes"),
        "consolidation-persistence",
        &ctx,
    )?;

    // Consolidate any remaining episodes into long-term storage. Errors are
    // propagated so a failed consolidation aborts the persistence phase
    // rather than silently dropping data.
    bridge.consolidate_episodes(20)?;

    Ok(())
}

#[cfg(test)]
mod tests;

// PR-A (issue #2281): preparation-phase memory filters. Tests assert
// that `goal-board:snapshot` revisions are dropped from the prepared
// context and that stale `goal-store:record` facts (slugs not in the
// live goal-board) are filtered out when the caller supplies the
// active+backlog slug set.
#[cfg(test)]
mod tests_pr_a;

// PR-B (issue #2281): episode distillation — periodic many-to-few
// extraction of semantic facts from recent episodes via an LLM
// recipe. See `docs/architecture/episode-distillation.md`.
pub mod distillation;

// Issue #2327: episode-ingestion classifier — the deterministic policy that
// runs before every `store_episode` intake site, dropping operational noise,
// down-scoping bookkeeping, and storing meaningful episodics with
// {importance, event_kind, goal_id, cycle, is_operational} metadata.
pub mod classifier;

// Issue #2327: automatic promotion (distillation) scheduler — fires episode →
// fact/procedure distillation on an undistilled-count threshold or a
// cycle-count interval, decoupled from the OODA `ConsolidateMemory` action.
pub mod scheduler;

#[cfg(test)]
mod distillation_tests;

// PR-C (issue #2281, problem 4): episodic recall tests for
// `preparation_memory_operations`. Pins the tokenizer rules,
// self-session noise filter, and no-tokens short-circuit
// behaviour.
#[cfg(test)]
mod tests_pr_c;

// Issue #2327: TDD (RED) tests for the episode-ingestion classifier
// (`classifier::classify`) that drops operational-noise episodes,
// down-scopes bookkeeping, and stores meaningful episodics with
// {importance, event_kind, goal_id, cycle, is_operational} metadata.
#[cfg(test)]
mod classifier_tests;

// Issue #2327: TDD (RED) tests for the automatic promotion (distillation)
// scheduler (`scheduler::distill_trigger` / `run_scheduled_distillation_*`)
// and the procedure-distillation extension (DistilledProcedure / DistillOutput
// / DistillReport.procedure_count) that stores procedures with provenance.
#[cfg(test)]
mod promotion_scheduler_tests;
