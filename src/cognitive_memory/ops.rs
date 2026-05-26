//! CognitiveMemoryOps trait impl for NativeCognitiveMemory + Cypher escaping.

use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
    CognitiveWorkingSlot,
};

use super::{CognitiveMemoryOps, NativeCognitiveMemory, as_f64, as_i64, as_str};

#[cfg(test)]
impl NativeCognitiveMemory {
    /// `cfg(test)`-only guard that panics when `self.path` is under
    /// `$HOME/.simard` — i.e. when a test is about to mutate the
    /// operator's live cognitive memory. Every mutating
    /// `CognitiveMemoryOps` method calls this at its entry point.
    ///
    /// New mutating methods on this impl **must** call
    /// `self.assert_hermetic_for("<method>")` as their first statement.
    /// See `docs/testing/hermetic-tests.md` for the full contract.
    fn assert_hermetic_for(&self, site: &'static str) {
        crate::test_support::hermetic_guard::assert_state_root_isolated(&self.path, site);
    }
}

/// null bytes — the full set of characters that can break or inject into
/// Cypher string literals.
pub(crate) fn escape_cypher(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

impl CognitiveMemoryOps for NativeCognitiveMemory {
    fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Trait-level `checkpoint` delegates to the inherent
    /// [`NativeCognitiveMemory::checkpoint`] (issue #1631).
    fn checkpoint(&self) -> SimardResult<()> {
        NativeCognitiveMemory::checkpoint(self)
    }

    fn record_sensory(
        &self,
        modality: &str,
        raw_data: &str,
        ttl_seconds: u64,
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::record_sensory");

        let id = Self::new_id("sen");
        let expires_at = Self::now_secs()? + ttl_seconds as f64;
        self.execute(&format!(
            "CREATE (s:Sensory {{id: '{}', modality: '{}', raw_data: '{}', observation_order: 0, expires_at: {expires_at}}})",
            escape_cypher(&id),
            escape_cypher(modality),
            escape_cypher(raw_data),
        ))?;
        self.post_write_barrier("record_sensory")?;
        Ok(id)
    }

    fn prune_expired_sensory(&self) -> SimardResult<usize> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::prune_expired_sensory");

        let now = Self::now_secs()?;
        let rows = self.query(&format!(
            "MATCH (s:Sensory) WHERE s.expires_at < {now} RETURN count(s)"
        ))?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(as_i64)
            .unwrap_or(0) as usize;
        if count > 0 {
            self.execute(&format!(
                "MATCH (s:Sensory) WHERE s.expires_at < {now} DELETE s"
            ))?;
            self.post_write_barrier("prune_expired_sensory")?;
        }
        Ok(count)
    }

    fn push_working(
        &self,
        slot_type: &str,
        content: &str,
        task_id: &str,
        relevance: f64,
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::push_working");

        let id = Self::new_id("wrk");
        self.execute(&format!(
            "CREATE (w:WorkingMemory {{id: '{}', slot_type: '{}', content: '{}', task_id: '{}', relevance: {relevance}}})",
            escape_cypher(&id),
            escape_cypher(slot_type),
            escape_cypher(content),
            escape_cypher(task_id),
        ))?;
        self.post_write_barrier("push_working")?;
        Ok(id)
    }

    fn get_working(&self, task_id: &str) -> SimardResult<Vec<CognitiveWorkingSlot>> {
        let rows = self.query(&format!(
            "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' RETURN w.id, w.slot_type, w.content, w.relevance, w.task_id",
            escape_cypher(task_id)
        ))?;
        Ok(rows
            .iter()
            .map(|row| CognitiveWorkingSlot {
                node_id: as_str(&row[0]).unwrap_or("").to_string(),
                slot_type: as_str(&row[1]).unwrap_or("").to_string(),
                content: as_str(&row[2]).unwrap_or("").to_string(),
                relevance: as_f64(&row[3]).unwrap_or(0.0),
                task_id: as_str(&row[4]).unwrap_or("").to_string(),
            })
            .collect())
    }

    fn clear_working(&self, task_id: &str) -> SimardResult<usize> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::clear_working");

        let rows = self.query(&format!(
            "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' RETURN count(w)",
            escape_cypher(task_id)
        ))?;
        let count = rows
            .first()
            .and_then(|r| r.first())
            .and_then(as_i64)
            .unwrap_or(0) as usize;
        if count > 0 {
            self.execute(&format!(
                "MATCH (w:WorkingMemory) WHERE w.task_id = '{}' DELETE w",
                escape_cypher(task_id)
            ))?;
            self.post_write_barrier("clear_working")?;
        }
        Ok(count)
    }

    fn store_episode(
        &self,
        content: &str,
        source_label: &str,
        _metadata: Option<&serde_json::Value>,
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::store_episode");

        let id = Self::new_id("epi");
        self.execute(&format!(
            "CREATE (e:Episode {{id: '{}', content: '{}', source_label: '{}', temporal_index: 0, compressed: 0}})",
            escape_cypher(&id),
            escape_cypher(content),
            escape_cypher(source_label),
        ))?;
        self.post_write_barrier("store_episode")?;
        Ok(id)
    }

    fn consolidate_episodes(&self, batch_size: u32) -> SimardResult<Option<String>> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::consolidate_episodes");

        let rows = self.query(&format!(
            "MATCH (e:Episode) WHERE e.compressed = 0 RETURN e.id, e.content ORDER BY e.temporal_index LIMIT {batch_size}"
        ))?;
        if rows.len() < 2 {
            return Ok(None);
        }
        let contents: Vec<&str> = rows.iter().filter_map(|r| as_str(&r[1])).collect();
        let original_count = contents.len();
        let mut seen = std::collections::HashSet::new();
        let unique_contents: Vec<&str> = contents
            .iter()
            .filter(|c| seen.insert(c.trim()))
            .copied()
            .collect();
        let unique_count = unique_contents.len();
        eprintln!(
            "[simard] episode consolidation: {original_count} → {unique_count} (compression ratio {:.1}%)",
            if original_count > 0 {
                (1.0 - unique_count as f64 / original_count as f64) * 100.0
            } else {
                0.0
            }
        );
        let summary = format!(
            "[consolidated {}→{} episodes]: {}",
            original_count,
            unique_count,
            unique_contents.join(" | ")
        );
        let summary_id = Self::new_id("epi");

        // Issue #2044 (G4): wrap the summary-insert + per-source
        // SET compressed=1 loop in a single BEGIN/COMMIT transaction
        // so a mid-loop crash cannot produce duplicate summaries.
        // Replaces the previous compensating-action pattern (#1975)
        // which was vulnerable to a crash between the summary CREATE
        // and the compensating DELETE.
        let mut txn_stmts = Vec::with_capacity(rows.len() + 1);
        txn_stmts.push(format!(
            "CREATE (e:Episode {{id: '{}', content: '{}', source_label: 'consolidation', temporal_index: 0, compressed: 1}})",
            escape_cypher(&summary_id),
            escape_cypher(&summary),
        ));
        for row in &rows {
            if let Some(eid) = as_str(&row[0]) {
                txn_stmts.push(format!(
                    "MATCH (e:Episode {{id: '{}'}}) SET e.compressed = 1",
                    escape_cypher(eid)
                ));
            }
        }
        self.execute_in_transaction(&txn_stmts)?;

        // Per-write barrier — one barrier for the whole consolidation op
        // (summary insert + N compress flips), not per Cypher statement.
        // Issue #1973 spec rationale (decision D5): consolidation is a
        // single semantic op; per-statement fsync would be O(N) syscalls.
        self.post_write_barrier("consolidate_episodes")?;
        Ok(Some(summary_id))
    }

    fn store_fact(
        &self,
        concept: &str,
        content: &str,
        confidence: f64,
        tags: &[String],
        source_id: &str,
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::store_fact");

        let id = Self::new_id("sem");
        let tags_str = tags.join(",");
        self.execute(&format!(
            "CREATE (f:Fact {{id: '{}', concept: '{}', content: '{}', confidence: {confidence}, tags: '{}', source_id: '{}'}})",
            escape_cypher(&id),
            escape_cypher(concept),
            escape_cypher(content),
            escape_cypher(&tags_str),
            escape_cypher(source_id),
        ))?;
        self.post_write_barrier("store_fact")?;
        Ok(id)
    }

    fn search_facts(
        &self,
        query: &str,
        limit: u32,
        min_confidence: f64,
    ) -> SimardResult<Vec<CognitiveFact>> {
        let q = escape_cypher(query);
        let rows = self.query(&format!(
            "MATCH (f:Fact) WHERE (f.concept CONTAINS '{q}' OR f.content CONTAINS '{q}') AND f.confidence >= {min_confidence} RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags ORDER BY f.id DESC LIMIT {limit}"
        ))?;
        Ok(rows
            .iter()
            .map(|row| {
                let tags_str = as_str(&row[5]).unwrap_or("");
                CognitiveFact {
                    node_id: as_str(&row[0]).unwrap_or("").to_string(),
                    concept: as_str(&row[1]).unwrap_or("").to_string(),
                    content: as_str(&row[2]).unwrap_or("").to_string(),
                    confidence: as_f64(&row[3]).unwrap_or(0.0),
                    source_id: as_str(&row[4]).unwrap_or("").to_string(),
                    tags: if tags_str.is_empty() {
                        vec![]
                    } else {
                        tags_str.split(',').map(|s| s.to_string()).collect()
                    },
                }
            })
            .collect())
    }

    fn store_procedure(
        &self,
        name: &str,
        steps: &[String],
        prerequisites: &[String],
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::store_procedure");

        let id = Self::new_id("proc");
        // Errors propagated (no silent `unwrap_or_default()`) so a
        // serialize failure cannot land a row whose `steps` column is the
        // empty string — that would round-trip as `[]` and look like a
        // legitimate zero-step procedure on recall.  See issue #1604 gap
        // G17 / the #1711/#1748/#1754 "no silent fallback" pattern.
        let steps_json =
            serde_json::to_string(steps).map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "store_procedure".into(),
                reason: format!(
                    "failed to serialize {} step(s) for procedure '{name}': {e}",
                    steps.len()
                ),
            })?;
        let prereqs_json =
            serde_json::to_string(prerequisites).map_err(|e| SimardError::BridgeCallFailed {
                bridge: "cognitive-memory-native".into(),
                method: "store_procedure".into(),
                reason: format!(
                    "failed to serialize {} prerequisite(s) for procedure '{name}': {e}",
                    prerequisites.len()
                ),
            })?;
        self.execute(&format!(
            "CREATE (p:Procedure {{id: '{}', name: '{}', steps: '{}', prerequisites: '{}', usage_count: 0}})",
            escape_cypher(&id),
            escape_cypher(name),
            escape_cypher(&steps_json),
            escape_cypher(&prereqs_json),
        ))?;
        self.post_write_barrier("store_procedure")?;
        Ok(id)
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        let q = escape_cypher(query);
        let rows = self.query(&format!(
            "MATCH (p:Procedure) WHERE p.name CONTAINS '{q}' OR p.steps CONTAINS '{q}' RETURN p.id, p.name, p.steps, p.prerequisites, p.usage_count LIMIT {limit}"
        ))?;
        // Each row is decoded with **loud** failure on schema drift or
        // corrupt JSON in `steps` / `prerequisites`.  The previous
        // implementation called `unwrap_or_default()` on the JSON parse
        // and `unwrap_or("")` on every column, which turned a corrupt
        // procedure into a "valid procedure with zero steps" — the exact
        // silent-empty-recall failure mode called out in issue #1604
        // (gap G17) and the recent #1711/#1748/#1754 work to remove
        // silent fallbacks from the cognitive substrate.
        rows.into_iter()
            .map(|row| -> SimardResult<CognitiveProcedure> {
                if row.len() < 5 {
                    return Err(SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "expected 5 columns from MATCH (p:Procedure), got {}",
                            row.len()
                        ),
                    });
                }
                let node_id = as_str(&row[0])
                    .ok_or_else(|| SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "procedure row column 0 (id) was not a string: {:?}",
                            row[0]
                        ),
                    })?
                    .to_string();
                let name = as_str(&row[1])
                    .ok_or_else(|| SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "procedure '{node_id}' column 1 (name) was not a string: {:?}",
                            row[1]
                        ),
                    })?
                    .to_string();
                let steps_str =
                    as_str(&row[2]).ok_or_else(|| SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "procedure '{node_id}' column 2 (steps) was not a string: {:?}",
                            row[2]
                        ),
                    })?;
                let prereqs_str =
                    as_str(&row[3]).ok_or_else(|| SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "procedure '{node_id}' column 3 (prerequisites) was not a string: {:?}",
                            row[3]
                        ),
                    })?;
                let steps: Vec<String> = serde_json::from_str(steps_str).map_err(|e| {
                    tracing::warn!(
                        node_id = %node_id,
                        column = "steps",
                        payload = %steps_str,
                        error = %e,
                        "cognitive_memory::recall_procedure: corrupt steps JSON",
                    );
                    SimardError::BridgeCallFailed {
                        bridge: "cognitive-memory-native".into(),
                        method: "recall_procedure".into(),
                        reason: format!(
                            "procedure '{node_id}' has corrupt steps JSON ({e}); payload={steps_str:?}"
                        ),
                    }
                })?;
                let prerequisites: Vec<String> =
                    serde_json::from_str(prereqs_str).map_err(|e| {
                        tracing::warn!(
                            node_id = %node_id,
                            column = "prerequisites",
                            payload = %prereqs_str,
                            error = %e,
                            "cognitive_memory::recall_procedure: corrupt prerequisites JSON",
                        );
                        SimardError::BridgeCallFailed {
                            bridge: "cognitive-memory-native".into(),
                            method: "recall_procedure".into(),
                            reason: format!(
                                "procedure '{node_id}' has corrupt prerequisites JSON ({e}); payload={prereqs_str:?}"
                            ),
                        }
                    })?;
                let usage_count = as_i64(&row[4]).unwrap_or(0);
                Ok(CognitiveProcedure {
                    node_id,
                    name,
                    steps,
                    prerequisites,
                    usage_count,
                })
            })
            .collect()
    }

    fn store_prospective(
        &self,
        description: &str,
        trigger_condition: &str,
        action_on_trigger: &str,
        priority: i64,
    ) -> SimardResult<String> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::store_prospective");

        let id = Self::new_id("pro");
        self.execute(&format!(
            "CREATE (p:Prospective {{id: '{}', description: '{}', trigger_condition: '{}', action_on_trigger: '{}', status: 'pending', priority: {priority}}})",
            escape_cypher(&id),
            escape_cypher(description),
            escape_cypher(trigger_condition),
            escape_cypher(action_on_trigger),
        ))?;
        self.post_write_barrier("store_prospective")?;
        Ok(id)
    }

    fn check_triggers(&self, content: &str) -> SimardResult<Vec<CognitiveProspective>> {
        let c = escape_cypher(content);
        let rows = self.query(&format!(
            "MATCH (p:Prospective) WHERE p.status = 'pending' AND '{c}' CONTAINS p.trigger_condition RETURN p.id, p.description, p.trigger_condition, p.action_on_trigger, p.status, p.priority"
        ))?;
        Ok(rows
            .iter()
            .map(|row| CognitiveProspective {
                node_id: as_str(&row[0]).unwrap_or("").to_string(),
                description: as_str(&row[1]).unwrap_or("").to_string(),
                trigger_condition: as_str(&row[2]).unwrap_or("").to_string(),
                action_on_trigger: as_str(&row[3]).unwrap_or("").to_string(),
                status: as_str(&row[4]).unwrap_or("pending").to_string(),
                priority: as_i64(&row[5]).unwrap_or(0),
            })
            .collect())
    }

    fn get_statistics(&self) -> SimardResult<CognitiveStatistics> {
        let count_query = |table: &str| -> SimardResult<u64> {
            let rows = self.query(&format!("MATCH (n:{table}) RETURN count(n)"))?;
            Ok(rows
                .first()
                .and_then(|r| r.first())
                .and_then(as_i64)
                .unwrap_or(0) as u64)
        };
        Ok(CognitiveStatistics {
            sensory_count: count_query("Sensory")?,
            working_count: count_query("WorkingMemory")?,
            episodic_count: count_query("Episode")?,
            semantic_count: count_query("Fact")?,
            procedural_count: count_query("Procedure")?,
            prospective_count: count_query("Prospective")?,
        })
    }
}

// ============================================================================
// Inline unit tests for ops.rs (issue #2036)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_mem() -> NativeCognitiveMemory {
        NativeCognitiveMemory::in_memory().expect("in-memory DB should create")
    }

    // ── escape_cypher ──────────────────────────────────────────────────

    #[test]
    fn escape_cypher_passthrough_for_ascii() {
        assert_eq!(escape_cypher("hello world"), "hello world");
    }

    #[test]
    fn escape_cypher_escapes_backslash() {
        assert_eq!(escape_cypher("a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_cypher_escapes_single_quote() {
        assert_eq!(escape_cypher("it's"), "it\\'s");
    }

    #[test]
    fn escape_cypher_escapes_control_chars() {
        assert_eq!(escape_cypher("\n"), "\\n");
        assert_eq!(escape_cypher("\r"), "\\r");
        assert_eq!(escape_cypher("\t"), "\\t");
        assert_eq!(escape_cypher("\0"), "\\0");
    }

    #[test]
    fn escape_cypher_handles_empty_string() {
        assert_eq!(escape_cypher(""), "");
    }

    #[test]
    fn escape_cypher_handles_mixed_special_chars() {
        assert_eq!(
            escape_cypher("it's a\nnew\\world\0"),
            "it\\'s a\\nnew\\\\world\\0"
        );
    }

    #[test]
    fn escape_cypher_preserves_unicode() {
        assert_eq!(escape_cypher("日本語🦀"), "日本語🦀");
    }

    // ── record_sensory / prune_expired_sensory ─────────────────────────

    #[test]
    fn record_sensory_returns_id_with_prefix() {
        let mem = test_mem();
        let id = mem.record_sensory("audio", "raw-bytes", 300).unwrap();
        assert!(
            id.starts_with("sen_"),
            "sensory id must start with sen_: {id}"
        );
    }

    #[test]
    fn record_sensory_is_queryable() {
        let mem = test_mem();
        mem.record_sensory("visual", "frame-1", 3600).unwrap();
        let rows = mem
            .query("MATCH (s:Sensory) WHERE s.modality = 'visual' RETURN s.raw_data")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(as_str(&rows[0][0]), Some("frame-1"));
    }

    #[test]
    fn prune_expired_sensory_removes_expired() {
        let mem = test_mem();
        mem.record_sensory("test", "will-expire", 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let pruned = mem.prune_expired_sensory().unwrap();
        assert!(pruned >= 1, "should prune at least 1 expired entry");
    }

    #[test]
    fn prune_expired_sensory_keeps_valid() {
        let mem = test_mem();
        mem.record_sensory("test", "long-lived", 99999).unwrap();
        let pruned = mem.prune_expired_sensory().unwrap();
        assert_eq!(pruned, 0, "non-expired sensory must not be pruned");
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.sensory_count, 1);
    }

    // ── push_working / get_working / clear_working ─────────────────────

    #[test]
    fn push_working_returns_prefixed_id() {
        let mem = test_mem();
        let id = mem.push_working("goal", "content", "task-1", 0.8).unwrap();
        assert!(id.starts_with("wrk_"), "working id must start with wrk_");
    }

    #[test]
    fn get_working_returns_matching_slots() {
        let mem = test_mem();
        mem.push_working("goal", "g1", "task-A", 1.0).unwrap();
        mem.push_working("ctx", "c1", "task-A", 0.5).unwrap();
        mem.push_working("goal", "g2", "task-B", 0.9).unwrap();

        let slots = mem.get_working("task-A").unwrap();
        assert_eq!(slots.len(), 2, "only task-A slots returned");
        assert!(
            slots.iter().all(|s| s.task_id == "task-A"),
            "all slots must belong to task-A"
        );
    }

    #[test]
    fn get_working_returns_empty_for_unknown_task() {
        let mem = test_mem();
        let slots = mem.get_working("nonexistent").unwrap();
        assert!(slots.is_empty());
    }

    #[test]
    fn clear_working_returns_count_and_removes() {
        let mem = test_mem();
        mem.push_working("a", "x", "task-C", 1.0).unwrap();
        mem.push_working("b", "y", "task-C", 0.5).unwrap();

        let cleared = mem.clear_working("task-C").unwrap();
        assert_eq!(cleared, 2);
        assert!(mem.get_working("task-C").unwrap().is_empty());
    }

    #[test]
    fn clear_working_returns_zero_for_empty() {
        let mem = test_mem();
        let cleared = mem.clear_working("no-such-task").unwrap();
        assert_eq!(cleared, 0);
    }

    // ── store_episode / consolidate_episodes ───────────────────────────

    #[test]
    fn store_episode_returns_prefixed_id() {
        let mem = test_mem();
        let id = mem.store_episode("event happened", "source", None).unwrap();
        assert!(id.starts_with("epi_"), "episode id must start with epi_");
    }

    #[test]
    fn store_episode_persists_content() {
        let mem = test_mem();
        mem.store_episode("test event", "my-source", None).unwrap();
        let rows = mem
            .query("MATCH (e:Episode) WHERE e.source_label = 'my-source' RETURN e.content")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(as_str(&rows[0][0]), Some("test event"));
    }

    #[test]
    fn consolidate_episodes_needs_at_least_two() {
        let mem = test_mem();
        mem.store_episode("solo", "src", None).unwrap();
        assert!(mem.consolidate_episodes(10).unwrap().is_none());
    }

    #[test]
    fn consolidate_episodes_creates_summary() {
        let mem = test_mem();
        mem.store_episode("alpha", "src", None).unwrap();
        mem.store_episode("beta", "src", None).unwrap();
        let summary_id = mem.consolidate_episodes(10).unwrap();
        assert!(summary_id.is_some());
        let sid = summary_id.unwrap();
        assert!(sid.starts_with("epi_"));
    }

    #[test]
    fn consolidate_episodes_marks_originals_compressed() {
        let mem = test_mem();
        for i in 0..3 {
            mem.store_episode(&format!("e{i}"), "src", None).unwrap();
        }
        mem.consolidate_episodes(10).unwrap();
        let rows = mem
            .query("MATCH (e:Episode) WHERE e.compressed = 0 RETURN count(e)")
            .unwrap();
        let uncompressed = as_i64(&rows[0][0]).unwrap();
        assert_eq!(
            uncompressed, 0,
            "all originals should be marked compressed=1"
        );
    }

    // ── store_fact / search_facts ──────────────────────────────────────

    #[test]
    fn store_fact_returns_prefixed_id() {
        let mem = test_mem();
        let id = mem
            .store_fact("concept", "content", 0.9, &[], "src")
            .unwrap();
        assert!(id.starts_with("sem_"), "fact id must start with sem_");
    }

    #[test]
    fn store_fact_with_tags() {
        let mem = test_mem();
        let tags = vec!["rust".to_string(), "perf".to_string()];
        mem.store_fact("concept", "content", 0.8, &tags, "src")
            .unwrap();
        let facts = mem.search_facts("concept", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].tags, tags);
    }

    #[test]
    fn search_facts_by_content() {
        let mem = test_mem();
        mem.store_fact("k", "needle in haystack", 0.9, &[], "src")
            .unwrap();
        let results = mem.search_facts("needle", 10, 0.0).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_facts_respects_limit() {
        let mem = test_mem();
        for i in 0..5 {
            mem.store_fact(&format!("topic{i}"), "common-content", 0.9, &[], "src")
                .unwrap();
        }
        let results = mem.search_facts("common-content", 3, 0.0).unwrap();
        assert!(results.len() <= 3, "limit must be respected");
    }

    #[test]
    fn search_facts_empty_result_for_no_match() {
        let mem = test_mem();
        mem.store_fact("rust", "fast", 0.9, &[], "src").unwrap();
        let results = mem.search_facts("python", 10, 0.0).unwrap();
        assert!(results.is_empty());
    }

    // ── store_procedure / recall_procedure ─────────────────────────────

    #[test]
    fn store_procedure_returns_prefixed_id() {
        let mem = test_mem();
        let id = mem
            .store_procedure("deploy", &["build".into(), "push".into()], &[])
            .unwrap();
        assert!(id.starts_with("proc_"), "procedure id prefix");
    }

    #[test]
    fn recall_procedure_returns_steps_and_prerequisites() {
        let mem = test_mem();
        let steps = vec!["compile".to_string(), "test".to_string()];
        let prereqs = vec!["install-deps".to_string()];
        mem.store_procedure("build-flow", &steps, &prereqs).unwrap();

        let procs = mem.recall_procedure("build-flow", 10).unwrap();
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].steps, steps);
        assert_eq!(procs[0].prerequisites, prereqs);
        assert_eq!(procs[0].usage_count, 0);
    }

    #[test]
    fn recall_procedure_empty_for_no_match() {
        let mem = test_mem();
        let procs = mem.recall_procedure("nonexistent", 10).unwrap();
        assert!(procs.is_empty());
    }

    // ── store_prospective / check_triggers ─────────────────────────────

    #[test]
    fn store_prospective_returns_prefixed_id() {
        let mem = test_mem();
        let id = mem
            .store_prospective("desc", "trigger", "action", 5)
            .unwrap();
        assert!(id.starts_with("pro_"), "prospective id prefix");
    }

    #[test]
    fn check_triggers_matches_substring() {
        let mem = test_mem();
        mem.store_prospective("watch for failure", "FAIL", "alert", 1)
            .unwrap();
        let triggered = mem.check_triggers("build FAILED with errors").unwrap();
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].description, "watch for failure");
        assert_eq!(triggered[0].priority, 1);
    }

    #[test]
    fn check_triggers_returns_empty_on_no_match() {
        let mem = test_mem();
        mem.store_prospective("watch for failure", "FAIL", "alert", 1)
            .unwrap();
        let triggered = mem.check_triggers("everything is fine").unwrap();
        assert!(triggered.is_empty());
    }

    #[test]
    fn check_triggers_only_returns_pending() {
        let mem = test_mem();
        mem.store_prospective("p1", "match-me", "act", 1).unwrap();
        // Manually flip status to 'done' to confirm pending-only filter.
        mem.execute(
            "MATCH (p:Prospective) WHERE p.trigger_condition = 'match-me' SET p.status = 'done'",
        )
        .unwrap();
        let triggered = mem.check_triggers("match-me").unwrap();
        assert!(
            triggered.is_empty(),
            "non-pending prospectives must not trigger"
        );
    }

    // ── get_statistics ─────────────────────────────────────────────────

    #[test]
    fn get_statistics_empty_db() {
        let mem = test_mem();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.sensory_count, 0);
        assert_eq!(stats.working_count, 0);
        assert_eq!(stats.episodic_count, 0);
        assert_eq!(stats.semantic_count, 0);
        assert_eq!(stats.procedural_count, 0);
        assert_eq!(stats.prospective_count, 0);
    }

    #[test]
    fn get_statistics_reflects_all_types() {
        let mem = test_mem();
        mem.record_sensory("m", "d", 300).unwrap();
        mem.push_working("s", "c", "t", 1.0).unwrap();
        mem.store_episode("e", "l", None).unwrap();
        mem.store_fact("f", "c", 0.5, &[], "s").unwrap();
        mem.store_procedure("p", &[], &[]).unwrap();
        mem.store_prospective("d", "t", "a", 1).unwrap();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.sensory_count, 1);
        assert_eq!(stats.working_count, 1);
        assert_eq!(stats.episodic_count, 1);
        assert_eq!(stats.semantic_count, 1);
        assert_eq!(stats.procedural_count, 1);
        assert_eq!(stats.prospective_count, 1);
    }

    // ── Cypher injection safety ────────────────────────────────────────

    #[test]
    fn store_fact_with_quotes_in_all_fields() {
        let mem = test_mem();
        let id = mem
            .store_fact(
                "con'cept",
                "con'tent",
                0.5,
                &["tag'1".to_string()],
                "src'id",
            )
            .unwrap();
        assert!(id.starts_with("sem_"));
        let facts = mem.search_facts("con", 10, 0.0).unwrap();
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn store_episode_with_newlines() {
        let mem = test_mem();
        let content = "line1\nline2\ttab\rreturn";
        mem.store_episode(content, "src", None).unwrap();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.episodic_count, 1);
    }

    // ── is_read_only ───────────────────────────────────────────────────

    #[test]
    fn is_read_only_false_for_in_memory() {
        let mem = test_mem();
        assert!(!mem.is_read_only());
    }
}
