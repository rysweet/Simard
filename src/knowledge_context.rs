//! Planning context enrichment via knowledge graph packs.
//!
//! Before the engineer loop begins planning, this module inspects the
//! objective text, determines which knowledge packs are relevant, and
//! queries the top packs to produce a [`PlanningContext`] that can be
//! injected into the planning prompt.

use std::collections::HashSet;

use crate::error::SimardResult;
use crate::knowledge_client::{KnowledgeClient, KnowledgePackInfo, KnowledgeQueryResult};

/// Maximum number of packs to query per objective.
const MAX_PACKS_PER_OBJECTIVE: usize = 3;

/// Default result limit per pack query.
const DEFAULT_QUERY_LIMIT: u32 = 5;

/// Knowledge gathered from packs to enrich the planning phase.
#[derive(Clone, Debug)]
pub struct PlanningContext {
    /// Query results from the most relevant packs.
    pub relevant_knowledge: Vec<KnowledgeQueryResult>,
    /// Names of packs that contributed knowledge.
    pub pack_sources: Vec<String>,
}

impl PlanningContext {
    /// True when no knowledge was gathered (all queries failed or no packs matched).
    pub fn is_empty(&self) -> bool {
        self.relevant_knowledge.is_empty()
    }
}

/// Enrich the planning phase by querying relevant knowledge packs.
///
/// The function:
/// 1. Lists available packs from the knowledge.
/// 2. Scores each pack by keyword overlap with the objective.
/// 3. Queries the top [`MAX_PACKS_PER_OBJECTIVE`] packs.
/// 4. Returns the aggregated results as a [`PlanningContext`].
///
/// If the knowledge is unavailable or no packs match, an empty context is returned
/// rather than hiding errors — knowledge enrichment failures propagate per PHILOSOPHY.md.
pub fn enrich_planning_context(
    objective: &str,
    knowledge: &KnowledgeClient,
) -> SimardResult<PlanningContext> {
    let packs = knowledge.list_packs()?;

    if packs.is_empty() {
        return Ok(PlanningContext {
            relevant_knowledge: vec![],
            pack_sources: vec![],
        });
    }

    let mut scored: Vec<(usize, &KnowledgePackInfo)> = packs
        .iter()
        .map(|pack| (relevance_score(objective, pack), pack))
        .filter(|(score, _)| *score > 0)
        .collect();

    // Sort descending by score.
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));
    scored.truncate(MAX_PACKS_PER_OBJECTIVE);

    let mut relevant_knowledge = Vec::new();
    let mut pack_sources = Vec::new();

    for (_score, pack) in &scored {
        match knowledge.query(&pack.name, objective, DEFAULT_QUERY_LIMIT) {
            Ok(result) if result.confidence > 0.0 => {
                pack_sources.push(pack.name.clone());
                relevant_knowledge.push(result);
            }
            Ok(_) => {
                // Low confidence -- skip this pack.
            }
            Err(_) => {
                // Query failed -- skip gracefully.
            }
        }
    }

    Ok(PlanningContext {
        relevant_knowledge,
        pack_sources,
    })
}

/// Minimum length of an objective token considered for relevance scoring.
/// One-character tokens (`a`, `1`, stray operators split off punctuation) carry
/// no topical signal and would match too indiscriminately.
const MIN_TOKEN_LEN: usize = 2;

/// Score a pack's relevance to an objective by **whole-word** keyword overlap.
///
/// The objective and the pack text (`name` + `description`) are each tokenized
/// into lowercase alphanumeric words. The score is the number of **distinct,
/// non-stopword** objective tokens (length >= [`MIN_TOKEN_LEN`]) that appear as
/// a whole word in the pack's word set.
///
/// Three properties matter for recall precision, each replacing a weakness of an
/// earlier scan (mirroring the word-boundary and stopword policy already adopted
/// by [`crate::memory_consolidation`], [`crate::memory_consolidation::classifier`]
/// and [`crate::fact_reliability`]):
///
///   * **Whole-word, not substring.** A short token no longer matches when it is
///     merely *embedded* in an unrelated pack word — `go` must not match
///     `category`/`algorithm`, `test` must not match `latest`, `own` must not
///     match `download`. Substring hits inflated the relevance of unrelated packs
///     and could crowd a genuinely relevant pack out of the
///     [`MAX_PACKS_PER_OBJECTIVE`] cut, injecting off-topic knowledge into the
///     planning prompt.
///   * **Distinct tokens, not repetitions.** A word repeated in the objective
///     counts once, so a verbose objective that restates one term cannot inflate
///     a pack's score and distort the ranking.
///   * **Stopwords carry no signal.** Common English stopwords (`the`, `with`,
///     `how`, `from`, …) are dropped via the shared
///     [`crate::memory_consolidation::is_recall_stopword`] predicate — the SAME
///     source of truth the episodic-recall tokenizer
///     ([`crate::memory_consolidation::tokenize_objective`]) uses. Without this,
///     a stopword the objective happens to share with an off-topic pack's
///     name/description counted as a match, inflating that pack's score and
///     re-opening the top-N crowd-out failure mode above by a different route.
fn relevance_score(objective: &str, pack: &KnowledgePackInfo) -> usize {
    let pack_words = word_set(&format!("{} {}", pack.name, pack.description));

    let objective_tokens: HashSet<String> = objective
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= MIN_TOKEN_LEN)
        .map(str::to_lowercase)
        .filter(|t| !crate::memory_consolidation::is_recall_stopword(t))
        .collect();

    objective_tokens
        .iter()
        .filter(|token| pack_words.contains(*token))
        .count()
}

/// Tokenize `text` into a set of distinct lowercase alphanumeric words for
/// whole-word membership tests. Empty tokens (produced by leading/trailing or
/// repeated separators) are dropped.
fn word_set(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{RPC_ERROR_METHOD_NOT_FOUND, RpcErrorPayload};
    use crate::rpc_transport::InMemoryRpcTransport;

    fn mock_transport() -> InMemoryRpcTransport {
        InMemoryRpcTransport::new("knowledge-ctx-test", |method, params| match method {
            "bridge.health" => Ok(serde_json::json!({
                "server_name": "simard-knowledge",
                "healthy": true,
            })),
            "knowledge.list_packs" => Ok(serde_json::json!({
                "packs": [
                    {
                        "name": "rust-expert",
                        "description": "Rust programming language ownership borrowing",
                        "article_count": 120,
                        "section_count": 450,
                    },
                    {
                        "name": "python-expert",
                        "description": "Python programming language stdlib",
                        "article_count": 200,
                        "section_count": 800,
                    },
                    {
                        "name": "docker-expert",
                        "description": "Docker containers images",
                        "article_count": 80,
                        "section_count": 300,
                    },
                ]
            })),
            "knowledge.query" => {
                let pack = params
                    .get("pack_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({
                    "answer": format!("Knowledge from {pack}"),
                    "sources": [{
                        "title": format!("{pack} article"),
                        "section": "Overview",
                    }],
                    "confidence": 0.8,
                }))
            }
            _ => Err(RpcErrorPayload {
                code: RPC_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            }),
        })
    }

    fn failing_transport() -> InMemoryRpcTransport {
        InMemoryRpcTransport::new("knowledge-fail", |method, _params| {
            Err(RpcErrorPayload {
                code: -32603,
                message: format!("knowledge down: {method}"),
            })
        })
    }

    #[test]
    fn enrich_picks_relevant_packs() {
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("Fix Rust ownership bug", &knowledge).unwrap();
        assert!(ctx.pack_sources.contains(&"rust-expert".to_string()));
        assert!(!ctx.relevant_knowledge.is_empty());
    }

    #[test]
    fn enrich_returns_error_when_knowledge_unavailable() {
        let knowledge = KnowledgeClient::new(Box::new(failing_transport()));
        let result = enrich_planning_context("anything", &knowledge);
        assert!(
            result.is_err(),
            "should propagate knowledge error, not silently degrade"
        );
    }

    #[test]
    fn enrich_returns_empty_for_unrelated_objective() {
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("xyzzy plugh", &knowledge).unwrap();
        assert!(ctx.is_empty());
    }

    #[test]
    fn relevance_score_matches_pack_name() {
        let pack = KnowledgePackInfo {
            name: "rust-expert".to_string(),
            description: "Rust programming language".to_string(),
            article_count: 100,
            section_count: 400,
        };
        let score = relevance_score("Fix Rust ownership issue", &pack);
        assert!(score >= 1, "expected match on 'rust', got {score}");
    }

    #[test]
    fn relevance_score_zero_for_no_match() {
        let pack = KnowledgePackInfo {
            name: "docker-expert".to_string(),
            description: "Docker containers".to_string(),
            article_count: 80,
            section_count: 300,
        };
        let score = relevance_score("Fix Rust ownership issue", &pack);
        assert_eq!(score, 0);
    }

    #[test]
    fn relevance_score_requires_whole_word_match() {
        // A short objective token must NOT match when it is merely *embedded* in
        // an unrelated pack word: `go` inside `category`/`algorithm`, `test`
        // inside `latest`. A raw-substring scan spuriously matched these and
        // inflated an unrelated pack's relevance.
        let pack = KnowledgePackInfo {
            name: "algorithms-expert".to_string(),
            description: "Sorting category latest algorithm".to_string(),
            article_count: 10,
            section_count: 20,
        };
        assert_eq!(
            relevance_score("go test", &pack),
            0,
            "'go'/'test' embedded in 'category'/'algorithm'/'latest' must not match"
        );
        // The same tokens DO match when present as whole words.
        let go_pack = KnowledgePackInfo {
            name: "go-expert".to_string(),
            description: "Go test tooling".to_string(),
            article_count: 10,
            section_count: 20,
        };
        assert_eq!(relevance_score("go test", &go_pack), 2);
    }

    #[test]
    fn relevance_score_counts_distinct_tokens_only() {
        // A word repeated in the objective must count once, so a verbose
        // objective that restates one term cannot inflate a pack's score.
        let pack = KnowledgePackInfo {
            name: "rust-expert".to_string(),
            description: "Rust programming language".to_string(),
            article_count: 100,
            section_count: 400,
        };
        assert_eq!(
            relevance_score("rust rust rust programming", &pack),
            2,
            "distinct tokens {{rust, programming}} => 2, not 4"
        );
    }

    #[test]
    fn relevance_score_ignores_stopwords() {
        // Common English stopwords carry no topical signal and must NOT count as
        // matches, even when the objective shares them with a pack's
        // name/description. Otherwise a stopword overlap inflates an off-topic
        // pack's score and can crowd a genuinely relevant pack out of the top-N
        // cut. The shared source of truth is
        // `memory_consolidation::is_recall_stopword`, which the episodic-recall
        // tokenizer also uses.
        //
        // The pack's word set is composed entirely of stopwords, and the
        // objective's ONLY overlap with it is via those stopwords — so a
        // stopword-blind scan would have scored 5+ here. With filtering the
        // topical score is exactly zero.
        let pack = KnowledgePackInfo {
            name: "faq".to_string(),
            description: "the and with this that from how why what when where".to_string(),
            article_count: 10,
            section_count: 20,
        };
        assert_eq!(
            relevance_score("how the rust with this from that", &pack),
            0,
            "stopword-only overlap ('how','the','with','this','from','that') must not score"
        );
    }

    #[test]
    fn relevance_score_stopword_does_not_crowd_out_topical_pack() {
        // Regression guard for the crowd-out failure mode: a topical pack that
        // shares one genuine keyword must out-rank an off-topic pack that only
        // shares stopwords with the objective.
        let objective = "How to fix this Rust ownership with the borrow checker";
        let topical = KnowledgePackInfo {
            name: "rust-expert".to_string(),
            description: "Rust ownership borrow checker".to_string(),
            article_count: 100,
            section_count: 400,
        };
        let off_topic = KnowledgePackInfo {
            name: "faq".to_string(),
            description: "how this with the that".to_string(),
            article_count: 10,
            section_count: 20,
        };
        let topical_score = relevance_score(objective, &topical);
        let off_topic_score = relevance_score(objective, &off_topic);
        assert_eq!(
            off_topic_score, 0,
            "off-topic pack shares only stopwords => 0, got {off_topic_score}"
        );
        assert!(
            topical_score > off_topic_score,
            "topical pack ({topical_score}) must out-rank stopword-only pack ({off_topic_score})"
        );
    }

    #[test]
    fn planning_context_is_empty_when_no_results() {
        let ctx = PlanningContext {
            relevant_knowledge: vec![],
            pack_sources: vec![],
        };
        assert!(ctx.is_empty());
    }

    #[test]
    fn max_packs_capped() {
        // Even with many matching packs, we cap at MAX_PACKS_PER_OBJECTIVE.
        let knowledge = KnowledgeClient::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context(
            "Rust Python Docker containers programming language",
            &knowledge,
        )
        .unwrap();
        assert!(ctx.pack_sources.len() <= MAX_PACKS_PER_OBJECTIVE);
    }
}
