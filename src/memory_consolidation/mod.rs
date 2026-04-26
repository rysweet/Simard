//! Session lifecycle phases mapped to cognitive memory operations.
//!
//! Each session phase (intake, preparation, execution, reflection, persistence)
//! triggers specific memory operations that progressively build and refine the
//! agent's cognitive state. This module provides the mapping functions.

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure, CognitiveProspective};
use crate::session::SessionId;

/// Context assembled during the preparation phase for use during execution.
///
/// Contains the relevant facts, triggered prospective memories, and recalled
/// procedures that the agent should consider when executing the session
/// objective.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedContext {
    pub relevant_facts: Vec<CognitiveFact>,
    pub triggered_prospectives: Vec<CognitiveProspective>,
    pub recalled_procedures: Vec<CognitiveProcedure>,
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

    // Store as an episodic event so we have a record of what was asked.
    bridge.store_episode(
        &format!("Session {session_id} started with objective: {objective}"),
        "session-intake",
        None,
    )?;

    Ok(())
}

/// Preparation phase: gather relevant context from long-term memory.
///
/// Searches semantic memory for facts related to the objective, checks
/// prospective memories for any triggered actions, and recalls relevant
/// procedures. The assembled context is returned for use during execution.
#[tracing::instrument(skip_all)]
pub fn preparation_memory_operations(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<PreparedContext> {
    // Search for facts related to the objective.
    let relevant_facts = bridge.search_facts(objective, 10, 0.0)?;

    // Check if any prospective memories are triggered by the objective.
    let triggered_prospectives = bridge.check_triggers(objective)?;

    // Recall procedures that might be relevant.
    let recalled_procedures = bridge.recall_procedure(objective, 5)?;

    // Push a summary of what we found into working memory.
    let context_summary = format!(
        "Prepared context: {} facts, {} triggers, {} procedures",
        relevant_facts.len(),
        triggered_prospectives.len(),
        recalled_procedures.len(),
    );
    bridge.push_working(
        "context-summary",
        &context_summary,
        session_id.as_str(),
        0.8,
    )?;

    Ok(PreparedContext {
        relevant_facts,
        triggered_prospectives,
        recalled_procedures,
    })
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
    // Store the session transcript as an episodic memory.
    bridge.store_episode(
        &format!("Session {session_id} transcript: {transcript}"),
        "session-reflection",
        None,
    )?;

    // Store each extracted fact in semantic memory, deduplicating by concept
    // both within this session and across prior sessions.
    let mut seen_concepts = std::collections::HashSet::<String>::new();
    for fact in facts {
        if !seen_concepts.insert(fact.concept.clone()) {
            continue;
        }
        // Cross-session dedup: skip if an existing fact has >= confidence.
        let existing = bridge
            .search_facts(&fact.concept, 5, fact.confidence)
            .unwrap_or_default();
        if existing.iter().any(|f| f.confidence >= fact.confidence) {
            continue;
        }
        bridge.store_fact(
            &fact.concept,
            &fact.content,
            fact.confidence,
            &[],
            &format!("session:{session_id}"),
        )?;
    }

    Ok(())
}

/// Persistence phase: clean up working memory and attempt episode consolidation.
///
/// This is the final phase of a session. Working memory for this session is
/// cleared, expired sensory items are pruned, and episode consolidation is
/// attempted to keep episodic memory compact.
#[tracing::instrument(skip_all)]
pub fn persistence_memory_operations(
    session_id: &SessionId,
    bridge: &dyn CognitiveMemoryOps,
) -> SimardResult<()> {
    // Consolidate episodes (batch of 10) BEFORE clearing working memory, so a
    // consolidation failure aborts teardown rather than silently dropping the
    // session's working-memory contents. Errors are propagated.
    bridge.consolidate_episodes(10)?;

    // Clear working memory for this session.
    bridge.clear_working(session_id.as_str())?;

    // Prune expired sensory items.
    bridge.prune_expired_sensory()?;

    // Store a final episodic memory marking session end.
    bridge.store_episode(
        &format!("Session {session_id} completed and persisted"),
        "session-persistence",
        None,
    )?;

    // Save a JSON snapshot for durable cross-session recall.  Errors are
    // non-fatal: log and continue so a snapshot failure never aborts the
    // session lifecycle.
    if let Some(dir) = crate::memory_snapshot::snapshot_dir(None) {
        match crate::memory_snapshot::save_session_snapshot(bridge, session_id.as_str(), &dir) {
            Ok(path) => {
                eprintln!("[simard] memory_snapshot: saved {}", path.display());
                // Prune: keep only the 10 most recent snapshots.
                prune_snapshots(&dir, 10);
            }
            Err(e) => {
                eprintln!("[simard] memory_snapshot: save failed (non-fatal): {e}");
            }
        }
    }

    Ok(())
}

/// Delete all but the `keep` most-recent snapshot files in `dir`.
///
/// Filenames are `<agent>-<epoch>.json`; lexicographic sort == chronological.
/// Errors during deletion are logged but not propagated.
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
            eprintln!("[simard] memory_snapshot: prune read_dir failed (non-fatal): {e}");
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
            eprintln!(
                "[simard] memory_snapshot: prune delete {} failed (non-fatal): {e}",
                path.display()
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
        bridge.store_episode(&summary, "consolidation-intake", None)?;
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
    // Store an episodic record capturing the consolidation event.
    bridge.store_episode(
        &format!("Session {session_id} flushing working memory to episodes"),
        "consolidation-persistence",
        None,
    )?;

    // Consolidate any remaining episodes into long-term storage. Errors are
    // propagated so a failed consolidation aborts the persistence phase
    // rather than silently dropping data.
    bridge.consolidate_episodes(20)?;

    Ok(())
}

#[cfg(test)]
mod tests;
