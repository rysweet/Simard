//! Rust types matching the Python `amplihack_memory.memory_types` dataclasses.
//!
//! Each struct maps one-to-one to the corresponding Python type in
//! `amplihack-memory-lib`. Fields use the same names and semantics so that
//! JSON round-trips between the Rust memory client and the Python memory
//! server are lossless.

use serde::{Deserialize, Serialize};

/// Short-lived raw observation from sensory memory.
///
/// Maps to Python `SensoryItem`. The `expires_at` field is a Unix timestamp
/// (seconds) after which the item may be pruned.
///
/// Used by future `get_recent_sensory` memory method (Phase 3+).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct CognitiveSensoryItem {
    pub node_id: String,
    pub modality: String,
    pub raw_data: String,
    pub observation_order: i64,
    pub expires_at: f64,
}

/// Active task-context slot from working memory.
///
/// Maps to Python `WorkingMemorySlot`. Bounded capacity is enforced by the
/// Python `CognitiveMemory` layer (default 20 slots per task).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveWorkingSlot {
    pub node_id: String,
    pub slot_type: String,
    pub content: String,
    pub relevance: f64,
    pub task_id: String,
}

/// Autobiographical event from episodic memory.
///
/// Maps to Python `EpisodicMemory`. Episodes can be consolidated into
/// summaries via `consolidate_episodes`.
///
/// Used by future `get_episodes` memory method (Phase 3+).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct CognitiveEpisode {
    pub node_id: String,
    pub content: String,
    pub source_label: String,
    pub temporal_index: i64,
    pub compressed: bool,
    /// Wall-clock instant the episode was recorded, carried through from the
    /// library `EpisodicMemory::created_at` (issue #4383). `None` for backends
    /// or callers that genuinely lack a timestamp — never a fabricated epoch,
    /// so the dashboard "Recent Memories" panel degrades honestly to a blank
    /// "time ago" label rather than showing a nonsensical 1970s date.
    /// `#[serde(default)]` so episodes serialized before this field existed
    /// deserialize to `None`.
    #[serde(default)]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Distilled knowledge fact from semantic memory.
///
/// Maps to Python `SemanticFact`. The `confidence` field ranges from 0.0 to
/// 1.0 and is used for search filtering and hive quality gating.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveFact {
    pub node_id: String,
    pub concept: String,
    pub content: String,
    pub confidence: f64,
    pub source_id: String,
    pub tags: Vec<String>,
    /// Number of times this fact has been reinforced on recall/use (issue
    /// #2395). Feeds the ranked-recall `usage` signal. `#[serde(default)]` so
    /// facts serialized before this field existed deserialize to `0`.
    #[serde(default)]
    pub usage_count: i64,
    /// When this fact was last reinforced via
    /// [`CognitiveMemoryOps::reinforce_access`](crate::cognitive_memory::CognitiveMemoryOps::reinforce_access)
    /// (issue #2395). `None` until first reinforced; feeds the ranked-recall
    /// `recency` signal. `#[serde(default)]` for back-compat.
    #[serde(default)]
    pub last_accessed_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Reusable step-by-step procedure from procedural memory.
///
/// Maps to Python `ProceduralMemory`. The `usage_count` is incremented
/// each time the procedure is recalled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveProcedure {
    pub node_id: String,
    pub name: String,
    pub steps: Vec<String>,
    pub prerequisites: Vec<String>,
    pub usage_count: i64,
}

/// Future-oriented trigger-action pair from prospective memory.
///
/// Maps to Python `ProspectiveMemory`. Status transitions from "pending"
/// to "triggered" when `check_triggers` matches, then to "resolved" on
/// explicit resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveProspective {
    pub node_id: String,
    pub description: String,
    pub trigger_condition: String,
    pub action_on_trigger: String,
    pub status: String,
    pub priority: i64,
}

/// Aggregate counts across all six cognitive memory types.
///
/// Returned by `get_statistics` to give a quick snapshot of memory
/// utilisation without fetching individual records.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CognitiveStatistics {
    pub sensory_count: u64,
    pub working_count: u64,
    pub episodic_count: u64,
    pub semantic_count: u64,
    pub procedural_count: u64,
    pub prospective_count: u64,
}

impl CognitiveStatistics {
    pub fn total(&self) -> u64 {
        self.sensory_count
            + self.working_count
            + self.episodic_count
            + self.semantic_count
            + self.procedural_count
            + self.prospective_count
    }
}

/// Edge / connection counts across the cognitive-memory graph (issue #2331).
///
/// Where [`CognitiveStatistics`] reports per-type *node* counts, `GraphStats`
/// surfaces the *connections* between those nodes so an operator can SEE the
/// cognitive-memory graph forming: provenance edges (a fact / procedure points
/// back at the episode it was distilled from), similarity edges, and the
/// `SUPERSEDES` chain left behind by caller-key snapshot dedup.
///
/// Returned by [`CognitiveMemoryOps::graph_stats`](crate::cognitive_memory::CognitiveMemoryOps::graph_stats).
/// Backends without a provenance graph (IPC client, memory, stubs) return an
/// all-zero value via the trait default, so callers stay backend-agnostic.
///
/// `similar_to_edges` and `supersedes_edges` are present for completeness but
/// the pinned library rev exposes no public reader for them, so the library
/// adapter reports `0`; the snapshot-dedup fields below give the operator a
/// computed proxy for the `SUPERSEDES` activity instead.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphStats {
    /// `DERIVES_FROM` edges (fact → source episode), summed over all facts.
    pub derives_from_edges: u64,
    /// `PROCEDURE_DERIVES_FROM` edges (procedure → source episode).
    pub procedure_derives_from_edges: u64,
    /// `SIMILAR_TO` edges (fact ↔ fact). `0` on the library backend — no public
    /// reader at the pinned rev.
    pub similar_to_edges: u64,
    /// `SUPERSEDES` edges (new snapshot → archived prior). `0` on the library
    /// backend — no public reader at the pinned rev; see the snapshot-dedup
    /// fields for a computed proxy.
    pub supersedes_edges: u64,
    /// Facts that carry at least one `DERIVES_FROM` edge.
    pub facts_with_provenance: u64,
    /// Total semantic facts considered (matches the `semantic` node count).
    pub facts_total: u64,
    /// Distinct caller keys (`dedup_key`) seen among snapshot facts. After
    /// dedup each key keeps one live fact, so this is the number of logical
    /// snapshot streams.
    pub distinct_snapshot_caller_keys: u64,
    /// Snapshot facts in the store (live + superseded revisions). A value well
    /// above `distinct_snapshot_caller_keys` is the visible dedup signal: many
    /// revisions collapsed onto a few caller keys.
    pub snapshot_facts_total: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cognitive_fact_round_trips_through_json() {
        let fact = CognitiveFact {
            node_id: "sem_abc123".to_string(),
            concept: "rust".to_string(),
            content: "Rust is a systems language".to_string(),
            confidence: 0.95,
            source_id: "epi_xyz".to_string(),
            tags: vec!["language".to_string(), "systems".to_string()],
            usage_count: 0,
            last_accessed_at: None,
        };
        let json = serde_json::to_string(&fact).unwrap();
        let parsed: CognitiveFact = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, fact);
    }

    #[test]
    fn cognitive_statistics_total_sums_all_types() {
        let stats = CognitiveStatistics {
            sensory_count: 10,
            working_count: 5,
            episodic_count: 20,
            semantic_count: 15,
            procedural_count: 3,
            prospective_count: 2,
        };
        assert_eq!(stats.total(), 55);
    }

    #[test]
    fn cognitive_statistics_default_is_all_zeros() {
        let stats = CognitiveStatistics::default();
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn cognitive_prospective_deserializes_status_field() {
        let json = r#"{
            "node_id": "pro_1",
            "description": "watch for errors",
            "trigger_condition": "error",
            "action_on_trigger": "alert",
            "status": "triggered",
            "priority": 5
        }"#;
        let pm: CognitiveProspective = serde_json::from_str(json).unwrap();
        assert_eq!(pm.status, "triggered");
        assert_eq!(pm.priority, 5);
    }

    #[test]
    fn cognitive_working_slot_deserializes_relevance() {
        let json = r#"{
            "node_id": "wrk_1",
            "slot_type": "goal",
            "content": "build feature",
            "relevance": 0.85,
            "task_id": "task-001"
        }"#;
        let slot: CognitiveWorkingSlot = serde_json::from_str(json).unwrap();
        assert!((slot.relevance - 0.85).abs() < f64::EPSILON);
    }
}
