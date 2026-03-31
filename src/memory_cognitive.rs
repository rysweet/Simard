//! Rust types matching the Python `amplihack_memory.memory_types` dataclasses.
//!
//! Each struct maps one-to-one to the corresponding Python type in
//! `amplihack-memory-lib`. Fields use the same names and semantics so that
//! JSON round-trips between the Rust bridge client and the Python bridge
//! server are lossless.

use serde::{Deserialize, Serialize};

/// Short-lived raw observation from sensory memory.
///
/// Maps to Python `SensoryItem`. The `expires_at` field is a Unix timestamp
/// (seconds) after which the item may be pruned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CognitiveEpisode {
    pub node_id: String,
    pub content: String,
    pub source_label: String,
    pub temporal_index: i64,
    pub compressed: bool,
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
