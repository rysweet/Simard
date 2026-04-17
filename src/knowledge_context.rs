//! Planning context enrichment via knowledge graph packs.
//!
//! Before the engineer loop begins planning, this module inspects the
//! objective text, determines which knowledge packs are relevant, and
//! queries the top packs to produce a [`PlanningContext`] that can be
//! injected into the planning prompt.

use crate::error::SimardResult;
use crate::knowledge_bridge::{KnowledgeBridge, KnowledgePackInfo, KnowledgeQueryResult};

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
/// 1. Lists available packs from the bridge.
/// 2. Scores each pack by keyword overlap with the objective.
/// 3. Queries the top [`MAX_PACKS_PER_OBJECTIVE`] packs.
/// 4. Returns the aggregated results as a [`PlanningContext`].
///
/// If the bridge is unavailable or no packs match, an empty context is returned
/// rather than hiding errors — knowledge enrichment failures propagate per PHILOSOPHY.md.
pub fn enrich_planning_context(
    objective: &str,
    bridge: &KnowledgeBridge,
) -> SimardResult<PlanningContext> {
    let packs = bridge.list_packs()?;

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
    scored.sort_by_key(|b| std::cmp::Reverse(b.0));
    scored.truncate(MAX_PACKS_PER_OBJECTIVE);

    let mut relevant_knowledge = Vec::new();
    let mut pack_sources = Vec::new();

    for (_score, pack) in &scored {
        match bridge.query(&pack.name, objective, DEFAULT_QUERY_LIMIT) {
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

/// Score a pack's relevance to an objective by keyword overlap.
///
/// The objective is tokenized into lowercase words, and each word is
/// checked against the pack name and description. The score is the
/// number of matching tokens.
fn relevance_score(objective: &str, pack: &KnowledgePackInfo) -> usize {
    let objective_tokens: Vec<&str> = objective
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .collect();

    let pack_text = format!("{} {}", pack.name, pack.description).to_lowercase();

    objective_tokens
        .iter()
        .filter(|token| pack_text.contains(&token.to_lowercase()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{BRIDGE_ERROR_METHOD_NOT_FOUND, BridgeErrorPayload};
    use crate::bridge_subprocess::InMemoryBridgeTransport;

    fn mock_transport() -> InMemoryBridgeTransport {
        InMemoryBridgeTransport::new("knowledge-ctx-test", |method, params| match method {
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
            _ => Err(BridgeErrorPayload {
                code: BRIDGE_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            }),
        })
    }

    fn failing_transport() -> InMemoryBridgeTransport {
        InMemoryBridgeTransport::new("knowledge-fail", |method, _params| {
            Err(BridgeErrorPayload {
                code: -32603,
                message: format!("bridge down: {method}"),
            })
        })
    }

    #[test]
    fn enrich_picks_relevant_packs() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("Fix Rust ownership bug", &bridge).unwrap();
        assert!(ctx.pack_sources.contains(&"rust-expert".to_string()));
        assert!(!ctx.relevant_knowledge.is_empty());
    }

    #[test]
    fn enrich_returns_error_when_bridge_unavailable() {
        let bridge = KnowledgeBridge::new(Box::new(failing_transport()));
        let result = enrich_planning_context("anything", &bridge);
        assert!(
            result.is_err(),
            "should propagate bridge error, not silently degrade"
        );
    }

    #[test]
    fn enrich_returns_empty_for_unrelated_objective() {
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context("xyzzy plugh", &bridge).unwrap();
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
        let bridge = KnowledgeBridge::new(Box::new(mock_transport()));
        let ctx = enrich_planning_context(
            "Rust Python Docker containers programming language",
            &bridge,
        )
        .unwrap();
        assert!(ctx.pack_sources.len() <= MAX_PACKS_PER_OBJECTIVE);
    }
}
