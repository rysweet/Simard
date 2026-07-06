//! Failing TDD tests for the G2 de-fork of the precision@k scoring primitive
//! (issue #2491 / #2494, G1 hybrid measurement, Step 7).
//!
//! **Guideline G2:** any memory-measurement capability (recall scoring)
//! belongs in `amplihack-memory-lib`, not forked into Simard. Today
//! `cognitive_memory::metrics` computes the scoring math inline
//! (`query_tokens` + `fact_is_relevant`) — a standing fork. After the de-fork,
//! `cognitive_memory::metrics::precision_at_k` is a **thin adapter** that maps
//! Simard's `CognitiveFact` onto the upstream primitive's decoupled
//! `(concept, content)` pairs and delegates the scoring to
//! `amplihack_memory::measurement::precision_at_k`.
//!
//! Reference: `docs/reference/recall-precision-hybrid-api.md#simard-adapter`
//!
//! ```rust
//! // src/cognitive_memory/metrics.rs (post de-fork)
//! pub fn precision_at_k(query: &str, facts: &[CognitiveFact], k: usize) -> Option<f64> {
//!     let pairs: Vec<(&str, &str)> = facts
//!         .iter()
//!         .map(|f| (f.concept.as_str(), f.content.as_str()))
//!         .collect();
//!     amplihack_memory::measurement::precision_at_k(query, &pairs, k)
//! }
//! ```
//!
//! Two guarantees:
//!   * **Delegation (G2)** — a source scan pins that the scoring math no longer
//!     lives in Simard: `metrics.rs` delegates to `amplihack_memory::measurement`
//!     and no longer defines the inline relevance helper. This is the failing
//!     (red) check today; the behavioural cases below are the *parity gate* that
//!     the move changes nothing observable.
//!   * **Parity** — the adapter reproduces the exact documented semantics
//!     (the 9 pure-math cases moved verbatim upstream as the parity gate).

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::cognitive_memory::metrics::precision_at_k;
    use crate::memory_cognitive::CognitiveFact;

    fn fact(concept: &str, content: &str) -> CognitiveFact {
        CognitiveFact {
            node_id: format!("n-{concept}"),
            concept: concept.to_string(),
            content: content.to_string(),
            confidence: 1.0,
            source_id: "test".to_string(),
            tags: vec![],
            usage_count: 0,
            last_accessed_at: None,
        }
    }

    fn metrics_source() -> String {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/cognitive_memory/metrics.rs");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
    }

    // ── G2 de-fork: delegation, not a local fork ────────────────────────────

    /// The adapter must delegate to the upstream measurement primitive.
    #[test]
    fn precision_at_k_delegates_to_upstream_primitive() {
        let src = metrics_source();
        assert!(
            src.contains("amplihack_memory::measurement::precision_at_k"),
            "metrics.rs must delegate scoring to amplihack_memory::measurement::precision_at_k (G2)"
        );
    }

    /// The forked scoring math must be gone from Simard — its home is upstream.
    /// The tell-tale of the fork is the inline `fact_is_relevant` relevance
    /// helper; after the de-fork the only Simard-side glue is the
    /// `CognitiveFact -> (concept, content)` mapping, which carries no scoring
    /// logic.
    #[test]
    fn scoring_math_is_not_forked_into_simard() {
        let src = metrics_source();
        assert!(
            !src.contains("fn fact_is_relevant"),
            "the relevance/scoring math must move to amplihack-memory-lib, not stay forked \
             in Simard's metrics.rs (G2)"
        );
    }

    // ── Parity gate: adapter reproduces the documented semantics ─────────────

    #[test]
    fn parity_all_relevant_is_one() {
        let facts = [
            fact("kafka streaming", "backpressure"),
            fact("kafka broker", "partition rebalance"),
        ];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(1.0));
    }

    #[test]
    fn parity_half_relevant_over_window() {
        let facts = [
            fact("kafka streaming", "backpressure"),
            fact("kafka broker", "rebalance"),
            fact("postgres index", "btree bloat"),
            fact("redis cache", "eviction"),
        ];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(1.0));
        assert_eq!(precision_at_k("kafka", &facts, 4), Some(0.5));
    }

    #[test]
    fn parity_matches_on_content_not_only_concept() {
        let facts = [fact("infra note", "the kafka consumer lagged")];
        assert_eq!(precision_at_k("kafka", &facts, 1), Some(1.0));
    }

    #[test]
    fn parity_zero_when_top_k_all_irrelevant() {
        let facts = [fact("postgres", "vacuum"), fact("redis", "ttl")];
        assert_eq!(precision_at_k("kafka", &facts, 2), Some(0.0));
    }

    #[test]
    fn parity_clamps_k_to_result_len() {
        let facts = [fact("kafka", "lag")];
        assert_eq!(precision_at_k("kafka", &facts, 10), Some(1.0));
    }

    #[test]
    fn parity_none_for_empty_results() {
        let facts: [CognitiveFact; 0] = [];
        assert_eq!(precision_at_k("kafka", &facts, 5), None);
    }

    #[test]
    fn parity_none_for_wildcard_or_empty_query() {
        let facts = [fact("kafka", "lag")];
        assert_eq!(precision_at_k("*", &facts, 1), None);
        assert_eq!(precision_at_k("   ", &facts, 1), None);
        assert_eq!(precision_at_k("", &facts, 1), None);
    }

    #[test]
    fn parity_multi_token_query_is_case_insensitive() {
        let facts = [
            fact("Kafka Streaming", "Backpressure"),
            fact("unrelated", "topic"),
        ];
        assert_eq!(precision_at_k("KAFKA streaming", &facts, 2), Some(0.5));
    }
}
