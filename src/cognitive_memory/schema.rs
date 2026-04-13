//! Cypher DDL for the cognitive memory graph schema.
//!
//! Seven node tables (one per memory type) and five relationship tables
//! matching the schema defined in `docs/architecture/cognitive-memory.md`.

/// DDL statements executed by `NativeCognitiveMemory::ensure_schema`.
pub(super) const DDL_STATEMENTS: &[&str] = &[
    // ── Node tables ──
    "CREATE NODE TABLE IF NOT EXISTS SensoryMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        modality STRING DEFAULT '', \
        raw_data STRING DEFAULT '', \
        observation_order INT64 DEFAULT 0, \
        expires_at INT64 DEFAULT 0\
    )",
    "CREATE NODE TABLE IF NOT EXISTS WorkingMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        slot_type STRING DEFAULT '', \
        content STRING DEFAULT '', \
        relevance DOUBLE DEFAULT 0.0, \
        task_id STRING DEFAULT ''\
    )",
    "CREATE NODE TABLE IF NOT EXISTS EpisodicMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        content STRING DEFAULT '', \
        source_label STRING DEFAULT '', \
        temporal_index INT64 DEFAULT 0, \
        compressed INT64 DEFAULT 0\
    )",
    "CREATE NODE TABLE IF NOT EXISTS SemanticMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        concept STRING DEFAULT '', \
        content STRING DEFAULT '', \
        confidence DOUBLE DEFAULT 0.0, \
        source_id STRING DEFAULT '', \
        tags STRING DEFAULT ''\
    )",
    "CREATE NODE TABLE IF NOT EXISTS ProceduralMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        name STRING DEFAULT '', \
        steps STRING DEFAULT '', \
        prerequisites STRING DEFAULT '', \
        usage_count INT64 DEFAULT 0\
    )",
    "CREATE NODE TABLE IF NOT EXISTS ProspectiveMemory(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        desc_text STRING DEFAULT '', \
        trigger_condition STRING DEFAULT '', \
        action_on_trigger STRING DEFAULT '', \
        status STRING DEFAULT 'pending', \
        priority INT64 DEFAULT 0\
    )",
    "CREATE NODE TABLE IF NOT EXISTS ConsolidatedEpisode(\
        node_id STRING PRIMARY KEY, \
        agent_id STRING DEFAULT '', \
        summary STRING DEFAULT '', \
        original_count INT64 DEFAULT 0\
    )",
    // ── Relationship tables ──
    "CREATE REL TABLE IF NOT EXISTS SIMILAR_TO(FROM SemanticMemory TO SemanticMemory)",
    "CREATE REL TABLE IF NOT EXISTS DERIVES_FROM(FROM SemanticMemory TO EpisodicMemory)",
    "CREATE REL TABLE IF NOT EXISTS PROCEDURE_DERIVES_FROM(FROM ProceduralMemory TO EpisodicMemory)",
    "CREATE REL TABLE IF NOT EXISTS CONSOLIDATES(FROM ConsolidatedEpisode TO EpisodicMemory)",
    "CREATE REL TABLE IF NOT EXISTS ATTENDED_TO(FROM SensoryMemory TO EpisodicMemory)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_has_seven_node_tables() {
        let node_count = DDL_STATEMENTS
            .iter()
            .filter(|s| s.contains("NODE TABLE"))
            .count();
        assert_eq!(node_count, 7);
    }

    #[test]
    fn ddl_has_five_relationship_tables() {
        let rel_count = DDL_STATEMENTS
            .iter()
            .filter(|s| s.contains("REL TABLE"))
            .count();
        assert_eq!(rel_count, 5);
    }

    #[test]
    fn ddl_total_statements() {
        assert_eq!(DDL_STATEMENTS.len(), 12);
    }
}
