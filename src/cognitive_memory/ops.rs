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
        self.execute(&format!(
            "CREATE (e:Episode {{id: '{}', content: '{}', source_label: 'consolidation', temporal_index: 0, compressed: 1}})",
            escape_cypher(&summary_id),
            escape_cypher(&summary),
        ))?;
        for row in &rows {
            if let Some(eid) = as_str(&row[0]) {
                self.execute(&format!(
                    "MATCH (e:Episode {{id: '{}'}}) SET e.compressed = 1",
                    escape_cypher(eid)
                ))?;
            }
        }
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
