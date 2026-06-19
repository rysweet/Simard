//! CognitiveMemoryOps trait impl for NativeCognitiveMemory + Cypher escaping.

use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::{
    CognitiveEpisode, CognitiveFact, CognitiveProcedure, CognitiveProspective, CognitiveStatistics,
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

/// Maximum number of keyword tokens OR-joined into a single
/// [`CognitiveMemoryOps::search_facts`] Cypher query (issue #2302).
/// Caps the WHERE clause so a pathologically long objective cannot
/// explode into hundreds of `CONTAINS` clauses; the first few keywords
/// of an objective carry the recall signal. The cap is applied *after*
/// stopword removal so the budget is spent on discriminating keywords
/// rather than on function words like `the`/`on`.
const MAX_FACT_QUERY_TOKENS: usize = 6;

/// Stopwords dropped during [`CognitiveMemoryOps::search_facts`] query
/// tokenization (issue #2302). A function word such as `the` appears in
/// almost every stored fact's `content`, so a `CONTAINS 'the'` clause
/// would collapse the query into "the newest `LIMIT` facts" and — worse,
/// combined with [`MAX_FACT_QUERY_TOKENS`] — crowd the discriminating
/// keywords out of the token budget. Short non-stopword tokens (`CI`,
/// `PR`, `#2302`) are deliberately kept: the distinction that gates a
/// token is keyword-vs-function-word, not length.
///
/// This list is intentionally **fact-recall-local** rather than the
/// `TOKEN_STOPWORDS` constant used by the episodic/procedural
/// `tokenize_objective` helper, so this fix stays confined to fact
/// search and cannot alter episodic or procedural recall (issue #2302
/// scope). All entries are lowercase; tokens are matched against them
/// case-insensitively via `eq_ignore_ascii_case` (no lowercased copy is
/// allocated).
const FACT_QUERY_STOPWORDS: &[&str] = &[
    "the", "and", "for", "with", "this", "that", "from", "has", "was", "were", "will", "into",
    "when", "where", "what", "why", "how", "on", "of", "to", "in", "a", "an", "at", "is", "are",
    "by", "or", "as", "it",
];

/// Tokenize a [`CognitiveMemoryOps::search_facts`] query into the keywords
/// that get OR-joined into `CONTAINS` clauses (issue #2302). Kept separate
/// from `tokenize_objective` (used by episodic/procedural recall) so the
/// fact-search fix cannot alter those paths; see
/// `docs/reference/cognitive-memory-fact-recall.md`.
///
/// Rules (in order):
/// 1. Split on ASCII whitespace ONLY — never on `-`/`:`/`/`, so a
///    single-token concept literal like `goal-store:record` stays intact
///    and the goal-fact load path in `memory_consolidation` is unchanged.
/// 2. Trim leading/trailing non-alphanumerics per token, preserving
///    interior punctuation; drop tokens that become empty.
/// 3. Drop stopwords (compared case-insensitively against
///    [`FACT_QUERY_STOPWORDS`]) and deduplicate (also case-insensitively).
///    Emitted tokens keep their original case so `CONTAINS` case-behaviour
///    matches the prior whole-string query.
/// 4. Cap at the first [`MAX_FACT_QUERY_TOKENS`] surviving tokens.
fn tokenize_fact_query(query: &str) -> Vec<String> {
    // Bounded at MAX_FACT_QUERY_TOKENS surviving tokens, so a pre-sized Vec
    // with a case-insensitive linear dedup is cheaper than a HashSet: it
    // avoids a per-token `to_ascii_lowercase` allocation, the set allocation,
    // and all hashing on the OODA-prep recall path.
    let mut tokens: Vec<String> = Vec::with_capacity(MAX_FACT_QUERY_TOKENS);
    for raw in query.split_ascii_whitespace() {
        let trimmed = raw.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed.is_empty() {
            continue;
        }
        if FACT_QUERY_STOPWORDS
            .iter()
            .any(|s| trimmed.eq_ignore_ascii_case(s))
        {
            continue;
        }
        if tokens.iter().any(|t| t.eq_ignore_ascii_case(trimmed)) {
            continue;
        }
        tokens.push(trimmed.to_string());
        if tokens.len() == MAX_FACT_QUERY_TOKENS {
            break;
        }
    }
    tokens
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
        tracing::debug!(
            query_len = query.len(),
            is_wildcard = (query == "*"),
            "search_facts: starting query"
        );
        // Treat `"*"` as "match everything" — `CONTAINS '*'` would search for
        // the literal asterisk character, producing zero results when the
        // caller intended a wildcard export (e.g. `export_memory_snapshot`).
        // Issue #1710: this was the root cause of empty snapshot exports that
        // made the corruption-recovery path lose all data.
        let rows = if query == "*" {
            self.query(&format!(
                "MATCH (f:Fact) WHERE f.confidence >= {min_confidence} \
                 RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags \
                 ORDER BY f.id DESC LIMIT {limit}"
            ))?
        } else {
            // Tokenize the query into keywords and OR one CONTAINS clause
            // per keyword (issue #2302). The OODA preparation phase passes
            // an entire multi-word objective fragment as `query`; that whole
            // string is almost never a verbatim substring of any stored
            // fact's concept/content, so the previous single whole-string
            // CONTAINS matched 0 rows — the "facts always zero" defect.
            // See `tokenize_fact_query` for the tokenization rules.
            let tokens = tokenize_fact_query(query);
            let where_clause = if tokens.len() >= 2 {
                // Multi-keyword objective fragment: OR one CONTAINS clause
                // per keyword, each escaped individually.
                tokens
                    .iter()
                    .map(|t| {
                        let esc = escape_cypher(t);
                        format!("(f.concept CONTAINS '{esc}' OR f.content CONTAINS '{esc}')")
                    })
                    .collect::<Vec<_>>()
                    .join(" OR ")
            } else if tokens.len() == 1 && query.split_ascii_whitespace().nth(1).is_some() {
                // A multi-word fragment that collapsed to a single keyword
                // after stopword removal (e.g. "the auth" -> ["auth"]).
                // Searching the whole original string would re-introduce the
                // #2302 zero-recall symptom — no stored fact contains
                // "the auth" verbatim — so search the surviving keyword.
                // Gated on the query being multi-word so single-token
                // exact-concept lookups keep their whole-string semantics in
                // the branch below.
                let esc = escape_cypher(&tokens[0]);
                format!("f.concept CONTAINS '{esc}' OR f.content CONTAINS '{esc}'")
            } else {
                // Single-token exact-concept lookup (`research:`,
                // `goal-store:record`, `goal-board:snapshot`, a one-word
                // tag) or an empty/all-stopword query: preserve the original
                // whole-string match byte-for-byte. Namespace-prefix callers
                // post-filter on the trailing colon, so broadening the
                // CONTAINS needle here could evict genuine matches under the
                // `ORDER BY id DESC LIMIT` window.
                let q = escape_cypher(query);
                format!("f.concept CONTAINS '{q}' OR f.content CONTAINS '{q}'")
            };
            self.query(&format!(
                "MATCH (f:Fact) WHERE ({where_clause}) \
                 AND f.confidence >= {min_confidence} \
                 RETURN f.id, f.concept, f.content, f.confidence, f.source_id, f.tags \
                 ORDER BY f.id DESC LIMIT {limit}"
            ))?
        };
        tracing::debug!(result_count = rows.len(), "search_facts: query complete");
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

        // Issue #2298: procedural memory was "frozen" — every OODA
        // consolidation cycle re-stored the identical procedures
        // (`consolidate:ad-hoc`, `pr-merge:adopt-tdd`, …) because this
        // method ran an unconditional `CREATE` with a fresh `new_id`, and
        // the `Procedure` table keys on `id` (not `name`), so the DB never
        // deduped. Duplicate-named nodes piled up, compression stayed at
        // 0%, and recall only ever surfaced the bootstrap set.
        //
        // Make the store an idempotent upsert keyed on the exact `name`:
        // if a procedure with this exact name already exists, bump its
        // `usage_count` (preserving the reinforcement signal) and return
        // the existing id instead of minting a new node.
        let escaped_name = escape_cypher(name);
        let existing = self.query(&format!(
            "MATCH (p:Procedure) WHERE p.name = '{escaped_name}' \
             RETURN p.id ORDER BY p.id LIMIT 1"
        ))?;
        if let Some(row) = existing.first() {
            let existing_id = as_str(&row[0])
                .ok_or_else(|| SimardError::BridgeCallFailed {
                    bridge: "cognitive-memory-native".into(),
                    method: "store_procedure".into(),
                    reason: format!(
                        "existing procedure '{name}' column 0 (id) was not a string: {:?}",
                        row[0]
                    ),
                })?
                .to_string();
            self.execute(&format!(
                "MATCH (p:Procedure {{id: '{}'}}) SET p.usage_count = p.usage_count + 1",
                escape_cypher(&existing_id),
            ))?;
            self.post_write_barrier("store_procedure")?;
            return Ok(existing_id);
        }

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

    /// Issue #2298: exact-name existence probe used by the bootstrap seeder and
    /// the OODA consolidation log. Overrides the trait default — which fans out
    /// a `CONTAINS` recall and JSON-decodes every hit's `steps`/`prerequisites`
    /// only to compare names — with a direct exact-equality lookup that returns
    /// no payload and stops at the first match (`LIMIT 1`). This is the same
    /// authoritative check `store_procedure` performs before deciding whether to
    /// reinforce or create, and unlike the recall fan-out it cannot be starved
    /// by trigger-token collisions exceeding the recall limit.
    fn procedure_exists(&self, name: &str) -> SimardResult<bool> {
        let escaped_name = escape_cypher(name);
        let rows = self.query(&format!(
            "MATCH (p:Procedure) WHERE p.name = '{escaped_name}' RETURN p.id LIMIT 1"
        ))?;
        Ok(!rows.is_empty())
    }

    fn recall_procedure(&self, query: &str, limit: u32) -> SimardResult<Vec<CognitiveProcedure>> {
        // Same wildcard treatment as `search_facts` — `"*"` means "return all"
        // so `export_memory_snapshot` actually captures every procedure.
        let rows = if query == "*" {
            self.query(&format!(
                "MATCH (p:Procedure) \
                 RETURN p.id, p.name, p.steps, p.prerequisites, p.usage_count \
                 LIMIT {limit}"
            ))?
        } else {
            let q = escape_cypher(query);
            self.query(&format!(
                "MATCH (p:Procedure) WHERE p.name CONTAINS '{q}' OR p.steps CONTAINS '{q}' \
                 RETURN p.id, p.name, p.steps, p.prerequisites, p.usage_count \
                 LIMIT {limit}"
            ))?
        };
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
        // Escape first, then interpolate, then case-fold both sides. Folding
        // makes matching case-insensitive (#2300) so a goal's lowercase
        // slug-phrase `trigger_condition` fires even when the OODA objective
        // probe mentions the phrase in its original (mixed) case. The single
        // escape chokepoint is preserved: `toLower` wraps the already-escaped
        // literal, and `p.trigger_condition` is a schema-controlled column.
        let c = escape_cypher(content);
        let rows = self.query(&format!(
            "MATCH (p:Prospective) WHERE p.status = 'pending' AND toLower('{c}') CONTAINS toLower(p.trigger_condition) RETURN p.id, p.description, p.trigger_condition, p.action_on_trigger, p.status, p.priority"
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

    fn resolve_prospective(&self, node_id: &str) -> SimardResult<()> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::resolve_prospective");

        let id = escape_cypher(node_id);
        self.execute(&format!(
            "MATCH (p:Prospective) WHERE p.id = '{id}' SET p.status = 'resolved'"
        ))?;
        self.post_write_barrier("resolve_prospective")?;
        Ok(())
    }

    /// PR-B (issue #2281): mark an episode as distilled so subsequent
    /// distillation passes skip it. Idempotent — re-marking a row
    /// already at `distilled = 1` is a no-op.
    fn mark_episode_distilled(&self, node_id: &str) -> SimardResult<()> {
        #[cfg(test)]
        self.assert_hermetic_for("NativeCognitiveMemory::mark_episode_distilled");

        let id = escape_cypher(node_id);
        self.execute(&format!(
            "MATCH (e:Episode) WHERE e.id = '{id}' SET e.distilled = 1"
        ))?;
        self.post_write_barrier("mark_episode_distilled")?;
        Ok(())
    }

    /// PR-B (issue #2281): return up to `limit` undistilled episodes,
    /// newest first.
    ///
    /// Ordering is `e.id DESC` because Episode ids are UUID-v7
    /// (time-prefixed) and so lex-descending equals
    /// chronologically-newest-first without consulting
    /// `temporal_index`. The `WHERE e.distilled = 0` clause is the
    /// undistilled gate; legacy rows whose column defaults to `0`
    /// from the lazy schema migration are included automatically.
    fn list_undistilled_episodes(&self, limit: u32) -> SimardResult<Vec<CognitiveEpisode>> {
        let rows = self.query(&format!(
            "MATCH (e:Episode) WHERE e.distilled = 0 \
             RETURN e.id, e.content, e.source_label, e.temporal_index, e.compressed \
             ORDER BY e.id DESC LIMIT {limit}"
        ))?;
        Ok(rows
            .into_iter()
            .map(|row| CognitiveEpisode {
                node_id: as_str(&row[0]).unwrap_or("").to_string(),
                content: as_str(&row[1]).unwrap_or("").to_string(),
                source_label: as_str(&row[2]).unwrap_or("").to_string(),
                temporal_index: as_i64(&row[3]).unwrap_or(0),
                compressed: as_i64(&row[4]).unwrap_or(0) != 0,
            })
            .collect())
    }

    /// Native impl of `search_episodes_by_keywords` for
    /// [`NativeCognitiveMemory`]. Lowercases each keyword and OR-joins one
    /// `lc CONTAINS '<lowercased+escaped>'` clause per keyword, where `lc`
    /// is `toLower(e.content)` projected **once per row** via a `WITH`
    /// stage. Matching is **case-insensitive**: keywords come from
    /// `tokenize_objective` already lowercased, but `store_episode`
    /// persists `content` verbatim, so both sides are lowered at query
    /// time (`toLower` on the column once per row via `WITH`,
    /// `to_lowercase()` on the keyword). Projecting the lowered content a
    /// single time — rather than recomputing `toLower(e.content)` inside
    /// every predicate — avoids N redundant per-row lowercasings when N
    /// keywords are searched, which matters on the 20k+ episode corpus
    /// this scans. This fixes the episodic-recall-returns-zero defect
    /// (issue #2299) without re-migrating already-stored verbatim
    /// episodes. Orders by `e.id DESC` to surface newest matches first —
    /// `id` is a UUID-v7 so descending lex-sort is equivalent to
    /// descending creation order without needing the `temporal_index`
    /// column.
    ///
    /// Keywords are trimmed, lowercased, and blank / whitespace-only
    /// entries dropped before predicates are built. If nothing remains —
    /// an empty slice or all-blank keywords — the query short-circuits to
    /// `Ok(vec![])`, so callers never need to special-case it and a lone
    /// blank keyword can never degrade into a `CONTAINS ''` match-all
    /// (over-disclosure + full-corpus scan). `escape_cypher` is applied
    /// last, after lowercasing, so the case-insensitive path keeps the
    /// same Cypher-injection protection as the original.
    ///
    /// Issue #2281, PR-C, problem 4. Issue #2299 (case-insensitive fix).
    fn search_episodes_by_keywords(
        &self,
        keywords: &[String],
        limit: u32,
    ) -> SimardResult<Vec<CognitiveEpisode>> {
        let predicates: Vec<String> = keywords
            .iter()
            .map(|kw| kw.trim().to_lowercase())
            .filter(|kw| !kw.is_empty())
            .map(|kw| format!("lc CONTAINS '{}'", escape_cypher(&kw)))
            .collect();
        if predicates.is_empty() {
            return Ok(vec![]);
        }
        let where_clause = predicates.join(" OR ");
        let rows = self.query(&format!(
            "MATCH (e:Episode) \
             WITH e, toLower(e.content) AS lc \
             WHERE {where_clause} \
             RETURN e.id, e.content, e.source_label, e.temporal_index, e.compressed \
             ORDER BY e.id DESC LIMIT {limit}"
        ))?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                if row.len() < 5 {
                    return None;
                }
                Some(CognitiveEpisode {
                    node_id: as_str(&row[0])?.to_string(),
                    content: as_str(&row[1])?.to_string(),
                    source_label: as_str(&row[2]).unwrap_or("").to_string(),
                    temporal_index: as_i64(&row[3]).unwrap_or(0),
                    compressed: as_i64(&row[4]).map(|n| n != 0).unwrap_or(false),
                })
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

    #[test]
    fn search_facts_wildcard_returns_all() {
        let mem = test_mem();
        mem.store_fact("alpha", "first", 0.9, &[], "src").unwrap();
        mem.store_fact("bravo", "second", 0.8, &[], "src").unwrap();
        mem.store_fact("charlie", "third", 0.7, &[], "src").unwrap();
        let results = mem.search_facts("*", 100, 0.0).unwrap();
        assert_eq!(
            results.len(),
            3,
            "wildcard '*' must return all facts, got {}",
            results.len()
        );
    }

    // ── tokenized fact recall (issue #2302) ────────────────────────────

    /// **TDD red → green (issue #2302).** A realistic multi-word objective
    /// whose full text is NOT a verbatim substring of any stored fact must
    /// still recall a fact that shares a single keyword.
    ///
    /// Before the fix `search_facts` built one whole-string `CONTAINS`
    /// clause from the entire query, so the 38-char objective matched no
    /// fact substring and returned zero rows — the "facts always zero"
    /// defect. After tokenization the shared `auth`/`module` keywords
    /// match. This test FAILS against the pre-fix body and PASSES after.
    #[test]
    fn search_facts_recalls_on_shared_keyword() {
        let mem = test_mem();
        // The CONTENT shares the keywords "auth" and "module" with the
        // objective, but does NOT contain the full objective verbatim.
        mem.store_fact(
            "ci-pattern",
            "the auth module integration tests are flaky under heavy load",
            0.8,
            &[],
            "episode-1",
        )
        .unwrap();

        let query = "investigate the failing auth module CI";
        let results = mem.search_facts(query, 10, 0.0).unwrap();

        assert!(
            results.iter().any(|f| f.concept == "ci-pattern"),
            "a multi-word objective must recall a fact sharing a keyword \
             via tokenized CONTAINS; got {} row(s): {:?}",
            results.len(),
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
    }

    /// Regression guard for the tokenization contract (issue #2302).
    ///
    /// A single-token query that contains internal punctuation — most
    /// importantly the `goal-store:record` concept literal that the
    /// preparation phase loads goal facts with — must stay on the
    /// preserved whole-string exact-match path. The tokenizer splits on
    /// **whitespace only**, so `goal-store:record` is one token and never
    /// explodes into `goal` / `store` / `record`. If it did, the query
    /// would also pull in the unrelated `audit-log` decoy below (whose
    /// content contains the word "record"), breaking the goal-store load.
    ///
    /// Passes on both the pre-fix body and a correct post-fix body; FAILS
    /// only if tokenization wrongly splits on `:`/`-`.
    #[test]
    fn search_facts_single_token_with_punctuation_is_exact() {
        let mem = test_mem();
        mem.store_fact(
            "goal-store:record",
            "{\"slug\":\"fix-auth\",\"title\":\"Stabilize auth tests\"}",
            1.0,
            &[],
            "goal-store",
        )
        .unwrap();
        // Decoy: shares the sub-word "record" but is NOT a goal-store fact.
        mem.store_fact(
            "audit-log",
            "a record of every config change applied this week",
            0.9,
            &[],
            "src",
        )
        .unwrap();

        let results = mem.search_facts("goal-store:record", 256, 0.0).unwrap();

        assert!(
            results.iter().any(|f| f.concept == "goal-store:record"),
            "exact goal-store:record concept must be recalled"
        );
        assert!(
            !results.iter().any(|f| f.concept == "audit-log"),
            "single-token punctuation query must NOT split into sub-words \
             and match the unrelated 'record' decoy; got: {:?}",
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
    }

    /// **Single-keyword recall for multi-word fragments (issue #2302).**
    ///
    /// A multi-word objective fragment that collapses to exactly one
    /// keyword after stopword removal (here `"the auth"` -> `["auth"]`)
    /// must search that surviving keyword, not the whole `"the auth"`
    /// literal. No stored fact contains the verbatim phrase `"the auth"`,
    /// so the pre-fix whole-string clause returned zero rows — the same
    /// "facts always zero" symptom #2302 fixes for the >=2-keyword path.
    ///
    /// FAILS against a body that searches the whole query for the
    /// single-survivor case; PASSES once the surviving keyword is used.
    #[test]
    fn search_facts_multiword_collapsing_to_one_keyword_recalls() {
        let mem = test_mem();
        // Content contains "auth" but never the verbatim phrase "the auth".
        mem.store_fact(
            "auth-pattern",
            "module auth flow is covered by integration tests",
            0.8,
            &[],
            "episode-1",
        )
        .unwrap();

        // "the" is a stopword, so only "auth" survives tokenization.
        let results = mem.search_facts("the auth", 10, 0.0).unwrap();

        assert!(
            results.iter().any(|f| f.concept == "auth-pattern"),
            "a multi-word fragment collapsing to one keyword must recall on \
             that keyword, not the whole 'the auth' literal; got {} row(s): \
             {:?}",
            results.len(),
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
    }

    /// **Namespace-lookup non-broadening guard (issue #2302).**
    ///
    /// A single whitespace-token query that carries a trailing-colon
    /// namespace prefix (`"research:"`, as passed by
    /// `research_tracker::load_research_topics`) must keep its exact
    /// whole-string `CONTAINS 'research:'` needle. The colon is what
    /// distinguishes a `research:<id>` topic fact from arbitrary prose
    /// that merely mentions the word "research"; broadening the needle to
    /// `CONTAINS 'research'` would let unrelated facts crowd the
    /// `ORDER BY id DESC LIMIT` window and evict genuine topics.
    ///
    /// FAILS if the single-survivor keyword path is applied to a
    /// single-token query (which would strip the colon and surface the
    /// prose decoy); PASSES on the whole-string namespace path.
    #[test]
    fn search_facts_single_token_namespace_not_broadened() {
        let mem = test_mem();
        // A genuine namespaced topic fact.
        mem.store_fact(
            "research:topic-1",
            "title=Vector clocks source=paper priority=1 status=proposed",
            0.9,
            &[],
            "research-tracker",
        )
        .unwrap();
        // Decoy: prose mentioning "research" but NOT under the namespace.
        mem.store_fact(
            "weekly-note",
            "spent the afternoon on research and prototyping",
            0.9,
            &[],
            "src",
        )
        .unwrap();

        let results = mem.search_facts("research:", 50, 0.0).unwrap();

        assert!(
            results.iter().any(|f| f.concept == "research:topic-1"),
            "the namespaced topic fact must be recalled"
        );
        assert!(
            !results.iter().any(|f| f.concept == "weekly-note"),
            "a single-token namespace query must NOT be broadened by \
             dropping the trailing colon and match the prose decoy; got: {:?}",
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
    }

    // ── tokenize_fact_query unit contract (issue #2302) ────────────────

    #[test]
    fn tokenize_fact_query_splits_on_whitespace_only() {
        // Internal `-`/`:`/`/` must be preserved; only whitespace splits.
        assert_eq!(
            tokenize_fact_query("goal-store:record src/foo.rs"),
            vec!["goal-store:record".to_string(), "src/foo.rs".to_string()]
        );
    }

    #[test]
    fn tokenize_fact_query_trims_edge_punctuation_keeps_interior() {
        assert_eq!(
            tokenize_fact_query("(auth) module, ci."),
            vec!["auth".to_string(), "module".to_string(), "ci".to_string()]
        );
    }

    #[test]
    fn tokenize_fact_query_drops_stopwords_case_insensitively() {
        // "The"/"the" and "CI" — function words go, keyword stays, and the
        // surviving token keeps its original case.
        assert_eq!(
            tokenize_fact_query("The auth THE module"),
            vec!["auth".to_string(), "module".to_string()]
        );
    }

    #[test]
    fn tokenize_fact_query_dedups_case_insensitively_first_case_wins() {
        assert_eq!(
            tokenize_fact_query("Auth auth AUTH module"),
            vec!["Auth".to_string(), "module".to_string()]
        );
    }

    #[test]
    fn tokenize_fact_query_caps_at_max_tokens() {
        let tokens = tokenize_fact_query("k1 k2 k3 k4 k5 k6 k7 k8");
        assert_eq!(tokens.len(), MAX_FACT_QUERY_TOKENS);
        assert_eq!(tokens.first().map(String::as_str), Some("k1"));
        assert_eq!(tokens.last().map(String::as_str), Some("k6"));
    }

    #[test]
    fn tokenize_fact_query_keeps_short_non_stopwords() {
        // `CI`, `PR`, `#2302` are short but discriminating — never dropped.
        assert_eq!(
            tokenize_fact_query("CI PR #2302"),
            vec!["CI".to_string(), "PR".to_string(), "2302".to_string()]
        );
    }

    #[test]
    fn tokenize_fact_query_empty_for_whitespace_or_all_stopwords() {
        assert!(tokenize_fact_query("   ").is_empty());
        assert!(tokenize_fact_query("the and of to").is_empty());
        assert!(tokenize_fact_query("").is_empty());
    }

    #[test]
    fn recall_procedure_wildcard_returns_all() {
        let mem = test_mem();
        mem.store_procedure("deploy", &["build".into()], &[])
            .unwrap();
        mem.store_procedure("test", &["lint".into()], &[]).unwrap();
        let results = mem.recall_procedure("*", 100).unwrap();
        assert_eq!(
            results.len(),
            2,
            "wildcard '*' must return all procedures, got {}",
            results.len()
        );
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

    // ── procedure_exists (native exact-name probe, issue #2298) ─────────

    #[test]
    fn procedure_exists_true_for_exact_name() {
        let mem = test_mem();
        mem.store_procedure("deploy-prod", &["build".into()], &[])
            .unwrap();
        assert!(
            mem.procedure_exists("deploy-prod").unwrap(),
            "exact name must report present"
        );
    }

    #[test]
    fn procedure_exists_false_when_absent() {
        let mem = test_mem();
        assert!(
            !mem.procedure_exists("never-stored").unwrap(),
            "absent name must report missing"
        );
    }

    #[test]
    fn procedure_exists_is_exact_not_contains() {
        // The native override must match on exact equality, not the
        // `CONTAINS` semantics of `recall_procedure`. A substring or
        // superstring of an existing name must NOT count as present —
        // otherwise the idempotency probe would conflate distinct
        // trigger-sharing procedures (issue #2298).
        let mem = test_mem();
        mem.store_procedure("deploy-prod", &["build".into()], &[])
            .unwrap();
        assert!(
            !mem.procedure_exists("deploy").unwrap(),
            "substring of an existing name must not report present"
        );
        assert!(
            !mem.procedure_exists("deploy-prod-eu").unwrap(),
            "superstring of an existing name must not report present"
        );
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

    // ── #2300 regression: prospective-triggers-never-fire ───────────────
    //
    // Root cause (b): the read/match path never fires a stored trigger when
    // the OODA objective probe mentions the goal phrase in its original
    // (mixed) case. An Active goal writes a prospective whose
    // `trigger_condition` is the lowercase slug-phrase
    // (`prospective_trigger_for` = `goal_slug(id).replace('-', " ")`), but a
    // realistic objective probe carries the goal's phrase in mixed case.
    // Under a case-SENSITIVE `CONTAINS`, the trigger never fires.

    /// RED for #2300: storing an Active-goal-style prospective and probing
    /// `check_triggers` with a realistic, mixed-case objective that contains
    /// the goal phrase MUST fire the trigger. Fails on `main` (case-sensitive
    /// CONTAINS); passes once matching is case-folded.
    #[test]
    fn check_triggers_fires_for_active_goal_objective() {
        let mem = test_mem();
        // Mirrors the live write path: description prefixed with `goal:`,
        // trigger_condition is the lowercase slug-phrase.
        mem.store_prospective(
            "goal:Fix Authentication Bug",
            "fix authentication bug",
            "Pursue goal: Fix Authentication Bug",
            1,
        )
        .unwrap();

        // Realistic OODA objective text — same words, original (mixed) case.
        let triggered = mem
            .check_triggers("Investigate and Fix Authentication Bug in the login flow")
            .unwrap();

        assert!(
            !triggered.is_empty(),
            "an Active-goal prospective must fire when the objective probe \
             mentions the goal phrase (case-insensitively); got {} triggers",
            triggered.len()
        );
        assert!(
            triggered.iter().any(|p| p.description.starts_with("goal:")),
            "the fired trigger must be the goal: prospective"
        );
    }

    /// RED for #2300: `check_triggers` matching MUST be case-insensitive.
    /// A lowercase `trigger_condition` must fire against an UPPERCASE probe.
    #[test]
    fn check_triggers_is_case_insensitive() {
        let mem = test_mem();
        mem.store_prospective("watch", "deploy ci pipeline", "act", 2)
            .unwrap();
        let triggered = mem.check_triggers("DEPLOY CI PIPELINE now").unwrap();
        assert_eq!(
            triggered.len(),
            1,
            "trigger matching must be case-insensitive"
        );
        assert_eq!(triggered[0].trigger_condition, "deploy ci pipeline");
    }

    /// Security regression for #2300: a probe laden with Cypher-significant
    /// characters (quote, backslash, newline, statement terminators) must not
    /// break the generated query nor spuriously match. The single escape
    /// chokepoint (`escape_cypher`) must remain intact under case-folding.
    #[test]
    fn check_triggers_handles_injection_chars_safely() {
        let mem = test_mem();
        mem.store_prospective("watch", "needle", "act", 1).unwrap();
        let probe = "trouble '; MATCH (n) DETACH DELETE n; -- \\ \n \" end";
        let triggered = mem
            .check_triggers(probe)
            .expect("injection-laden probe must not break the query");
        assert!(
            triggered.is_empty(),
            "no trigger should match the injection probe"
        );
    }

    #[test]
    fn resolve_prospective_sets_status_to_resolved() {
        let mem = test_mem();
        let id = mem
            .store_prospective("goal:fix tests", "fix tests", "pursue", 2)
            .unwrap();
        // Before resolve, should trigger.
        let before = mem.check_triggers("fix tests").unwrap();
        assert_eq!(before.len(), 1);

        mem.resolve_prospective(&id).unwrap();

        // After resolve, pending-only filter excludes it.
        let after = mem.check_triggers("fix tests").unwrap();
        assert!(after.is_empty(), "resolved prospective must not trigger");
    }

    #[test]
    fn resolve_prospective_is_idempotent() {
        let mem = test_mem();
        let id = mem
            .store_prospective("goal:idempotent", "idem", "act", 1)
            .unwrap();
        mem.resolve_prospective(&id).unwrap();
        // Resolving again should not error.
        mem.resolve_prospective(&id).unwrap();
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

    /// Injection regression guard for the **multi-token** query path
    /// (issue #2302, security requirement SR-1).
    ///
    /// The single-token path escapes the whole query string; the new
    /// multi-token path escapes each token individually before
    /// interpolating it into its own `CONTAINS` clause (see
    /// `search_facts`). This test forces the multi-token branch (>= 2
    /// surviving keywords) with a keyword that carries an *interior*
    /// single quote — which survives the tokenizer's edge-punctuation
    /// trim — so the per-token `escape_cypher` call is actually
    /// exercised. Left unescaped, that token would break out of its
    /// string literal as the Cypher fragment `'x' OR '1'`; the escape
    /// keeps it a harmless literal. If the per-token escape were ever
    /// dropped, the query would either raise a Cypher syntax error (the
    /// `unwrap` below panics) or widen the match to the `secret` decoy
    /// (the negative assertion fails).
    #[test]
    fn search_facts_multi_token_escapes_injection() {
        let mem = test_mem();
        mem.store_fact("alpha-fact", "benign keyword content", 0.9, &[], "src")
            .unwrap();
        // Decoy that must never surface via an injected `OR`-style payload.
        mem.store_fact("secret", "classified value", 0.9, &[], "src")
            .unwrap();

        // Two surviving tokens -> multi-token path. The second token keeps an
        // interior single quote, so `escape_cypher` must neutralize it; left
        // raw it would inject the Cypher fragment `'x' OR '1'`.
        let results = mem.search_facts("alpha x'OR'1", 256, 0.0).unwrap();

        assert!(
            results.iter().any(|f| f.concept == "alpha-fact"),
            "the escaped multi-token query must still recall the benign fact \
             via its real keyword; got: {:?}",
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
        assert!(
            !results.iter().any(|f| f.concept == "secret"),
            "an interior-quote token must be escaped to a literal, never \
             interpreted as a Cypher OR that leaks the decoy; got: {:?}",
            results
                .iter()
                .map(|f| f.concept.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn store_episode_with_newlines() {
        let mem = test_mem();
        let content = "line1\nline2\ttab\rreturn";
        mem.store_episode(content, "src", None).unwrap();
        let stats = mem.get_statistics().unwrap();
        assert_eq!(stats.episodic_count, 1);
    }

    // ── search_episodes_by_keywords (issue #2299) ──────────────────────

    /// RED test for issue #2299: "episodic recall returns zero".
    ///
    /// `tokenize_objective` lowercases every keyword it extracts from the
    /// OODA objective, but `store_episode` persists `content` verbatim and
    /// the Cypher `CONTAINS` clause is case-sensitive. A lowercased keyword
    /// therefore never matches mixed-case stored content, so recall logs
    /// "0 raw" every cycle despite tens of thousands of stored episodes.
    ///
    /// The trait contract (`mod.rs`) explicitly promises *case-insensitive*
    /// substring matching, so the case-sensitive implementation is a
    /// contract violation. This test stores mixed-case content under a
    /// NON-`session-` label (so the raw count is not confounded by the
    /// downstream session filter) and searches the lowercased keyword. It
    /// must return at least one episode (raw count > 0).
    ///
    /// Fails on base; passes once `search_episodes_by_keywords` matches
    /// case-insensitively.
    #[test]
    fn search_episodes_by_keywords_matches_case_insensitively() {
        let mem = test_mem();
        mem.store_episode("Deploy the Authentication Service", "ooda-objective", None)
            .unwrap();

        let hits = mem
            .search_episodes_by_keywords(&["authentication".to_string()], 10)
            .unwrap();

        assert!(
            !hits.is_empty(),
            "lowercased keyword must match mixed-case stored content (raw count > 0)"
        );
        assert_eq!(hits[0].content, "Deploy the Authentication Service");
    }

    /// Matching is symmetric: an upper-cased keyword must also find
    /// lower-cased stored content. Locks the contract in both directions
    /// so a one-sided `toLower` on only the content (or only the keyword)
    /// cannot silently pass.
    #[test]
    fn search_episodes_by_keywords_is_case_insensitive_both_directions() {
        let mem = test_mem();
        mem.store_episode("deploy the payment gateway", "ooda-objective", None)
            .unwrap();

        let hits = mem
            .search_episodes_by_keywords(&["PAYMENT".to_string()], 10)
            .unwrap();

        assert_eq!(
            hits.len(),
            1,
            "upper-cased keyword must match lower-cased stored content"
        );
    }

    /// An empty keyword slice short-circuits to an empty result so callers
    /// never need to special-case it (regression lock for the existing
    /// fast path).
    #[test]
    fn search_episodes_by_keywords_empty_slice_returns_empty() {
        let mem = test_mem();
        mem.store_episode("anything at all", "ooda-objective", None)
            .unwrap();
        let hits = mem.search_episodes_by_keywords(&[], 10).unwrap();
        assert!(
            hits.is_empty(),
            "empty keyword slice must return no episodes"
        );
    }

    /// A blank / whitespace-only keyword must NOT degrade into a
    /// `CONTAINS ''` match-all (over-disclosure + full-corpus scan). The
    /// guard lives inside `search_episodes_by_keywords` itself, so a single
    /// blank keyword returns nothing rather than every episode.
    ///
    /// Fails on base, where `CONTAINS ''` matches every stored episode.
    #[test]
    fn search_episodes_by_keywords_blank_keyword_emits_no_match_all() {
        let mem = test_mem();
        mem.store_episode("Some Episode Content", "ooda-objective", None)
            .unwrap();

        for blank in ["", "   ", "\t"] {
            let hits = mem
                .search_episodes_by_keywords(&[blank.to_string()], 10)
                .unwrap();
            assert!(
                hits.is_empty(),
                "blank keyword {blank:?} must not match every episode"
            );
        }
    }

    /// A Cypher-injection attempt is treated as a literal substring on the
    /// case-insensitive path: it matches nothing and raises no error. Locks
    /// in that `escape_cypher` is still applied (after lowercasing) so the
    /// fix does not open an injection hole.
    #[test]
    fn search_episodes_by_keywords_treats_injection_as_literal() {
        let mem = test_mem();
        mem.store_episode("Benign Episode", "ooda-objective", None)
            .unwrap();

        let hits = mem
            .search_episodes_by_keywords(&["' OR 1=1 //".to_string()], 10)
            .unwrap();

        assert!(
            hits.is_empty(),
            "injection payload must be a literal substring, matching nothing"
        );
    }

    /// Multi-keyword recall locks the optimized OR-join path. The perf
    /// change projects `toLower(e.content)` once via `WITH e, … AS lc` and
    /// OR-joins one `lc CONTAINS '<kw>'` predicate per keyword, so the
    /// multi-keyword query is the case the optimization actually
    /// restructured — yet every other test exercises only a single
    /// keyword. This stores three mixed-case episodes and searches two
    /// lowercased keywords that each match a different episode: the union
    /// of matches must be returned (case-insensitively via the projected
    /// `lc`), the non-matching episode excluded, and results ordered
    /// newest-first by `id DESC`.
    #[test]
    fn search_episodes_by_keywords_unions_multiple_keywords() {
        let mem = test_mem();
        mem.store_episode("Deploy the Authentication Service", "ooda-objective", None)
            .unwrap();
        mem.store_episode("Configure the Payment Gateway", "ooda-objective", None)
            .unwrap();
        mem.store_episode("Restart the Logging Daemon", "ooda-objective", None)
            .unwrap();

        let hits = mem
            .search_episodes_by_keywords(&["authentication".to_string(), "payment".to_string()], 10)
            .unwrap();

        let contents: Vec<&str> = hits.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(
            contents,
            vec![
                "Configure the Payment Gateway",
                "Deploy the Authentication Service",
            ],
            "OR-joined keywords must return both matches case-insensitively, \
             newest-first by id DESC, excluding the non-matching episode"
        );
    }

    // ── is_read_only ───────────────────────────────────────────────────

    #[test]
    fn is_read_only_false_for_in_memory() {
        let mem = test_mem();
        assert!(!mem.is_read_only());
    }
}
