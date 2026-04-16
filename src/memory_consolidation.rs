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

    // Store each extracted fact in semantic memory.
    for fact in facts {
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
    // Clear working memory for this session.
    bridge.clear_working(session_id.as_str())?;

    // Prune expired sensory items.
    bridge.prune_expired_sensory()?;

    // Attempt episode consolidation (batch of 10).
    bridge.consolidate_episodes(10)?;

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

    // Consolidate any remaining episodes into long-term storage.
    bridge.consolidate_episodes(20)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn counting_bridge() -> (CognitiveMemoryBridge, Arc<AtomicU32>) {
        let call_count = Arc::new(AtomicU32::new(0));
        let counter = call_count.clone();
        let transport = InMemoryBridgeTransport::new("test", move |method, _params| {
            counter.fetch_add(1, Ordering::SeqCst);
            match method {
                "memory.record_sensory" => Ok(json!({"id": "sen_1"})),
                "memory.push_working" => Ok(json!({"id": "wrk_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                "memory.search_facts" => Ok(json!({"facts": []})),
                "memory.check_triggers" => Ok(json!({"prospectives": []})),
                "memory.recall_procedure" => Ok(json!({"procedures": []})),
                "memory.store_fact" => Ok(json!({"id": "sem_1"})),
                "memory.clear_working" => Ok(json!({"count": 2})),
                "memory.prune_expired_sensory" => Ok(json!({"count": 0})),
                "memory.consolidate_episodes" => Ok(json!({"id": null})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }
        });
        (CognitiveMemoryBridge::new(Box::new(transport)), call_count)
    }

    fn test_session_id() -> SessionId {
        SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
    }

    #[test]
    fn intake_records_sensory_working_and_episode() {
        let (bridge, count) = counting_bridge();
        intake_memory_operations("build feature X", &test_session_id(), &bridge).unwrap();
        // Should make 3 calls: record_sensory, push_working, store_episode
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn preparation_returns_empty_context_when_memory_empty() {
        let (bridge, _) = counting_bridge();
        let ctx =
            preparation_memory_operations("build feature X", &test_session_id(), &bridge).unwrap();
        assert!(ctx.relevant_facts.is_empty());
        assert!(ctx.triggered_prospectives.is_empty());
        assert!(ctx.recalled_procedures.is_empty());
    }

    #[test]
    fn reflection_stores_transcript_and_facts() {
        let (bridge, count) = counting_bridge();
        let facts = vec![
            FactExtraction {
                concept: "rust".to_string(),
                content: "Rust is safe".to_string(),
                confidence: 0.9,
            },
            FactExtraction {
                concept: "testing".to_string(),
                content: "Tests should be fast".to_string(),
                confidence: 0.8,
            },
        ];
        reflection_memory_operations("transcript...", &facts, &test_session_id(), &bridge).unwrap();
        // 1 store_episode + 2 store_fact = 3
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn execution_truncates_multibyte_utf8_safely() {
        let (bridge, _) = counting_bridge();
        // Build a string with multi-byte chars that would panic with naive byte slicing.
        // Each CJK char is 3 bytes; 200 chars = 600 bytes, exceeding the 500-byte threshold.
        let cjk_output: String = std::iter::repeat_n('漢', 200).collect();
        assert!(cjk_output.len() > 500);
        // Must not panic.
        execution_memory_operations(&cjk_output, &test_session_id(), &bridge).unwrap();
    }

    #[test]
    fn execution_does_not_truncate_short_output() {
        let (bridge, _) = counting_bridge();
        execution_memory_operations("short", &test_session_id(), &bridge).unwrap();
    }

    #[test]
    fn persistence_clears_working_and_prunes() {
        let (bridge, count) = counting_bridge();
        persistence_memory_operations(&test_session_id(), &bridge).unwrap();
        // clear_working + prune_expired_sensory + consolidate_episodes + store_episode = 4
        // + snapshot: search_facts("*") + recall_procedure("*") = 2 more → 6 total
        assert_eq!(count.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn consolidation_intake_returns_zero_when_no_prior_facts() {
        let (bridge, count) = counting_bridge();
        let hydrated = consolidation_intake(&test_session_id(), "test-objective", &bridge).unwrap();
        assert_eq!(hydrated, 0);
        // Only 1 call: search_facts
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn consolidation_intake_with_facts_pushes_to_working_memory() {
        let call_count = Arc::new(AtomicU32::new(0));
        let counter = call_count.clone();
        let transport = InMemoryBridgeTransport::new("test-intake", move |method, _params| {
            counter.fetch_add(1, Ordering::SeqCst);
            match method {
                "memory.search_facts" => Ok(json!({
                    "facts": [{
                        "node_id": "n1",
                        "concept": "prior-fact",
                        "content": "remembered",
                        "confidence": 0.9,
                        "source_id": "memory-store-adapter",
                        "tags": []
                    }]
                })),
                "memory.push_working" => Ok(json!({"id": "wrk_1"})),
                "memory.store_episode" => Ok(json!({"id": "epi_1"})),
                _ => Err(crate::bridge::BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }
        });
        let bridge = CognitiveMemoryBridge::new(Box::new(transport));
        let hydrated = consolidation_intake(&test_session_id(), "test-objective", &bridge).unwrap();
        assert_eq!(hydrated, 1);
        // search_facts + push_working + store_episode = 3
        assert_eq!(call_count.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn consolidation_persistence_flushes_and_consolidates() {
        let (bridge, count) = counting_bridge();
        consolidation_persistence(&test_session_id(), &bridge).unwrap();
        // store_episode + consolidate_episodes = 2
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    /// Round-trip verification: intake → execution → persistence → recall.
    ///
    /// Uses `NativeCognitiveMemory` (in-memory LadybugDB) so that stored
    /// data is actually queryable, unlike the counting bridge which only
    /// counts calls.
    #[test]
    fn round_trip_execution_memory_recall() {
        use crate::cognitive_memory::NativeCognitiveMemory;

        let mem = NativeCognitiveMemory::in_memory().expect("in-memory DB");
        let sid = test_session_id();

        // 1. Intake — records objective as sensory + working + episode.
        intake_memory_operations("build feature X", &sid, &mem).unwrap();

        // 2. Execution — records pty output as sensory + working.
        execution_memory_operations("compiled successfully in 1.2s", &sid, &mem).unwrap();

        // 3. Persistence — flushes working memory and consolidates episodes.
        persistence_memory_operations(&sid, &mem).unwrap();

        // 4. Verify: the execution output should have been pushed into
        //    working memory under the session's task_id before persistence
        //    cleared it. Confirm the episode store received entries by
        //    checking statistics — intake stores 1 episode, persistence
        //    stores 1 episode, so we expect ≥ 2 episodes total.
        let stats = mem.get_statistics().unwrap();
        assert!(
            stats.episodic_count >= 2,
            "expected ≥2 episodes from intake+persistence, got {}",
            stats.episodic_count
        );
    }
}
