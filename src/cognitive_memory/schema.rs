//! LadybugDB schema DDL for cognitive memory node tables.

/// Cypher DDL statements to create cognitive memory node tables.
///
/// Each table maps to one of the six cognitive memory types plus
/// ancillary types (Decision, Goal) used for cross-session knowledge.
pub(crate) const SCHEMA_DDL: &[&str] = &[
    "CREATE NODE TABLE IF NOT EXISTS Fact(id STRING PRIMARY KEY, concept STRING, content STRING, confidence DOUBLE DEFAULT 0.0, source_id STRING DEFAULT '', tags STRING DEFAULT '')",
    "CREATE NODE TABLE IF NOT EXISTS Decision(id STRING PRIMARY KEY, description STRING, rationale STRING DEFAULT '', outcome STRING DEFAULT '', session_id STRING DEFAULT '')",
    "CREATE NODE TABLE IF NOT EXISTS Goal(id STRING PRIMARY KEY, description STRING, priority INT64 DEFAULT 0, status STRING DEFAULT 'pending')",
    "CREATE NODE TABLE IF NOT EXISTS Episode(id STRING PRIMARY KEY, content STRING, source_label STRING DEFAULT '', temporal_index INT64 DEFAULT 0, compressed INT64 DEFAULT 0)",
    "CREATE NODE TABLE IF NOT EXISTS Sensory(id STRING PRIMARY KEY, modality STRING, raw_data STRING, observation_order INT64 DEFAULT 0, expires_at DOUBLE DEFAULT 0.0)",
    "CREATE NODE TABLE IF NOT EXISTS WorkingMemory(id STRING PRIMARY KEY, slot_type STRING, content STRING, task_id STRING DEFAULT '', relevance DOUBLE DEFAULT 0.0)",
    "CREATE NODE TABLE IF NOT EXISTS Procedure(id STRING PRIMARY KEY, name STRING, steps STRING DEFAULT '', prerequisites STRING DEFAULT '', usage_count INT64 DEFAULT 0)",
    "CREATE NODE TABLE IF NOT EXISTS Prospective(id STRING PRIMARY KEY, description STRING, trigger_condition STRING, action_on_trigger STRING, status STRING DEFAULT 'pending', priority INT64 DEFAULT 0)",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_eight_tables() {
        assert_eq!(SCHEMA_DDL.len(), 8);
    }

    #[test]
    fn all_ddl_statements_use_if_not_exists() {
        for stmt in SCHEMA_DDL {
            assert!(
                stmt.contains("IF NOT EXISTS"),
                "DDL should be idempotent: {stmt}"
            );
        }
    }
}
