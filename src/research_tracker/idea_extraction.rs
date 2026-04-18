//! Automated idea extraction from tracked researcher activity.
//!
//! Queries cognitive memory for recent developer-activity facts, identifies
//! patterns and promising ideas worth investigating, and generates
//! `research:` issue proposals that can be filed on the roadmap.

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardResult;
use crate::memory_cognitive::CognitiveFact;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Keywords that signal a fact is worth surfacing as a research idea.
const SIGNAL_KEYWORDS: &[&str] = &[
    "agent",
    "agentic",
    "autonomous",
    "benchmark",
    "chain-of-thought",
    "code-generation",
    "cognitive",
    "context-window",
    "embedding",
    "fine-tuning",
    "framework",
    "function-calling",
    "grounding",
    "inference",
    "llm",
    "memory",
    "multi-agent",
    "orchestration",
    "planning",
    "prompt",
    "rag",
    "reasoning",
    "retrieval",
    "rl",
    "safety",
    "scaffolding",
    "self-improvement",
    "tool-use",
    "vector",
];

/// Minimum confidence threshold for facts to be considered.
const MIN_FACT_CONFIDENCE: f64 = 0.4;

/// Maximum number of activity facts to retrieve per extraction run.
const FACT_QUERY_LIMIT: u32 = 100;

/// Minimum keyword matches for a fact to become a proposal.
const MIN_KEYWORD_HITS: usize = 1;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A proposed `research:` issue extracted from developer activity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdeaProposal {
    /// Suggested issue title (prefixed with `research:`).
    pub title: String,
    /// Issue body describing what was observed and why it matters.
    pub body: String,
    /// Short rationale explaining why this idea was surfaced.
    pub rationale: String,
    /// GitHub ID of the developer whose activity triggered this idea.
    pub source_developer: String,
    /// The keyword signals that matched.
    pub matched_keywords: Vec<String>,
}

/// Summary of an extraction run.
#[derive(Clone, Debug)]
pub struct ExtractionResult {
    /// Number of activity facts examined.
    pub facts_examined: usize,
    /// Proposals generated from those facts.
    pub proposals: Vec<IdeaProposal>,
}

// ---------------------------------------------------------------------------
// Core extraction logic
// ---------------------------------------------------------------------------

/// Extract promising research ideas from recent developer-activity facts
/// stored in cognitive memory.
///
/// Queries for facts tagged with `developer-activity`, scores them by
/// keyword relevance, deduplicates by title similarity, and returns
/// [`IdeaProposal`]s ready to be filed as `research:` issues.
pub fn extract_ideas(memory: &dyn CognitiveMemoryOps) -> SimardResult<ExtractionResult> {
    let facts = memory.search_facts("dev-activity:", FACT_QUERY_LIMIT, MIN_FACT_CONFIDENCE)?;

    let mut proposals: Vec<IdeaProposal> = Vec::new();
    let mut seen_titles: Vec<String> = Vec::new();

    for fact in &facts {
        if !is_activity_fact(fact) {
            continue;
        }

        let matched = matched_keywords(&fact.content);
        if matched.len() < MIN_KEYWORD_HITS {
            continue;
        }

        let developer = extract_developer_id(&fact.concept);
        let event_title = extract_event_title(&fact.content);
        let proposal_title = format!("research: {event_title}");

        // Deduplicate by normalised title.
        let norm = proposal_title.to_lowercase();
        if seen_titles.iter().any(|t| t == &norm) {
            continue;
        }
        seen_titles.push(norm);

        let rationale = format!(
            "Developer {} activity matched signals: {}",
            developer,
            matched.join(", ")
        );

        let body = format!(
            "## Context\n\n\
             Observed activity from tracked developer **{}**:\n\n\
             > {}\n\n\
             ## Why this matters\n\n\
             Matched research-relevant signals: {}.\n\n\
             ## Suggested next steps\n\n\
             - Review the referenced work for applicability to Simard\n\
             - Assess alignment with current roadmap priorities\n\
             - If promising, promote to an active research topic",
            developer,
            fact.content,
            matched.join(", "),
        );

        proposals.push(IdeaProposal {
            title: proposal_title,
            body,
            rationale,
            source_developer: developer,
            matched_keywords: matched,
        });
    }

    Ok(ExtractionResult {
        facts_examined: facts.len(),
        proposals,
    })
}

/// Format a human-readable summary of an extraction result.
pub fn summarize_extraction(result: &ExtractionResult) -> String {
    if result.proposals.is_empty() {
        return format!(
            "examined {} activity fact(s), no new research ideas identified",
            result.facts_examined,
        );
    }
    let titles: Vec<&str> = result.proposals.iter().map(|p| p.title.as_str()).collect();
    format!(
        "examined {} activity fact(s), surfaced {} idea(s): {}",
        result.facts_examined,
        result.proposals.len(),
        titles.join("; "),
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether a fact represents a developer-activity event.
fn is_activity_fact(fact: &CognitiveFact) -> bool {
    fact.concept.starts_with("dev-activity:") || fact.tags.iter().any(|t| t == "developer-activity")
}

/// Return the subset of `SIGNAL_KEYWORDS` that appear in `text`.
fn matched_keywords(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    SIGNAL_KEYWORDS
        .iter()
        .filter(|kw| lower.contains(*kw))
        .map(|kw| (*kw).to_string())
        .collect()
}

/// Extract the developer GitHub ID from a concept like
/// `dev-activity:octocat:1234:0`.
fn extract_developer_id(concept: &str) -> String {
    concept
        .strip_prefix("dev-activity:")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or("unknown")
        .to_string()
}

/// Extract the event title from a fact content string that uses the
/// `key=value; ...` format produced by [`GitHubActivityEvent::summary`].
fn extract_event_title(content: &str) -> String {
    content
        .split("; ")
        .find_map(|segment| segment.strip_prefix("title="))
        .unwrap_or("untitled activity")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BridgeErrorPayload;
    use crate::bridge_subprocess::InMemoryBridgeTransport;
    use crate::memory_bridge::CognitiveMemoryBridge;

    // -- helpers --

    fn mock_memory_with_facts(facts: Vec<CognitiveFact>) -> Box<dyn CognitiveMemoryOps> {
        let facts_json = serde_json::to_value(&facts).unwrap();
        Box::new(CognitiveMemoryBridge::new(Box::new(
            InMemoryBridgeTransport::new("test-ideas", move |method, _params| match method {
                "memory.search_facts" => Ok(serde_json::json!({ "facts": facts_json })),
                "memory.store_fact" => Ok(serde_json::json!({"id": "sem_x"})),
                "memory.store_episode" => Ok(serde_json::json!({"id": "epi_x"})),
                _ => Err(BridgeErrorPayload {
                    code: -32601,
                    message: format!("unknown: {method}"),
                }),
            }),
        )))
    }

    fn activity_fact(developer: &str, title: &str, idx: usize) -> CognitiveFact {
        CognitiveFact {
            node_id: format!("sem_{idx}"),
            concept: format!("dev-activity:{developer}:1700000000:{idx}"),
            content: format!(
                "type=pull_request; repo={developer}/project; title={title}; created_at=2024-01-15T10:00:00Z",
            ),
            confidence: 0.6,
            source_id: "activity-poller".to_string(),
            tags: vec!["developer-activity".to_string(), format!("dev:{developer}")],
        }
    }

    // -- extract_ideas --

    #[test]
    fn extract_ideas_returns_empty_when_no_facts() {
        let memory = mock_memory_with_facts(vec![]);
        let result = extract_ideas(&*memory).unwrap();
        assert_eq!(result.facts_examined, 0);
        assert!(result.proposals.is_empty());
    }

    #[test]
    fn extract_ideas_surfaces_relevant_activity() {
        let facts = vec![
            activity_fact("simonw", "Add multi-agent orchestration layer", 0),
            activity_fact("ramparte", "Fix typo in docs", 1),
        ];
        let memory = mock_memory_with_facts(facts);
        let result = extract_ideas(&*memory).unwrap();
        assert_eq!(result.facts_examined, 2);
        // The multi-agent PR should match; the typo fix should not.
        assert_eq!(result.proposals.len(), 1);
        assert!(result.proposals[0].title.contains("multi-agent"));
        assert_eq!(result.proposals[0].source_developer, "simonw");
    }

    #[test]
    fn extract_ideas_deduplicates_by_title() {
        let facts = vec![
            activity_fact("dev1", "New LLM reasoning benchmark", 0),
            activity_fact("dev2", "New LLM reasoning benchmark", 1),
        ];
        let memory = mock_memory_with_facts(facts);
        let result = extract_ideas(&*memory).unwrap();
        // Same title → only one proposal.
        assert_eq!(result.proposals.len(), 1);
    }

    #[test]
    fn extract_ideas_populates_all_proposal_fields() {
        let facts = vec![activity_fact("octocat", "Agent framework for tool-use", 0)];
        let memory = mock_memory_with_facts(facts);
        let result = extract_ideas(&*memory).unwrap();
        assert_eq!(result.proposals.len(), 1);
        let p = &result.proposals[0];
        assert!(p.title.starts_with("research:"));
        assert!(!p.body.is_empty());
        assert!(!p.rationale.is_empty());
        assert_eq!(p.source_developer, "octocat");
        assert!(!p.matched_keywords.is_empty());
    }

    // -- matched_keywords --

    #[test]
    fn matched_keywords_finds_signals() {
        let text = "Implement multi-agent orchestration for LLM";
        let kw = matched_keywords(text);
        assert!(kw.contains(&"multi-agent".to_string()));
        assert!(kw.contains(&"orchestration".to_string()));
        assert!(kw.contains(&"llm".to_string()));
    }

    #[test]
    fn matched_keywords_case_insensitive() {
        let kw = matched_keywords("New RAG Pipeline with Embeddings");
        assert!(kw.contains(&"rag".to_string()));
        assert!(kw.contains(&"embedding".to_string()));
    }

    #[test]
    fn matched_keywords_empty_for_irrelevant_text() {
        let kw = matched_keywords("Fix typo in README");
        assert!(kw.is_empty());
    }

    // -- extract_developer_id --

    #[test]
    fn extract_developer_id_parses_concept() {
        assert_eq!(
            extract_developer_id("dev-activity:octocat:170000:0"),
            "octocat"
        );
    }

    #[test]
    fn extract_developer_id_handles_missing_prefix() {
        assert_eq!(extract_developer_id("other:key"), "unknown");
    }

    // -- extract_event_title --

    #[test]
    fn extract_event_title_parses_summary() {
        let content =
            "type=pull_request; repo=org/repo; title=Add feature X; created_at=2024-01-15";
        assert_eq!(extract_event_title(content), "Add feature X");
    }

    #[test]
    fn extract_event_title_returns_default_when_missing() {
        assert_eq!(extract_event_title("no title here"), "untitled activity");
    }

    // -- is_activity_fact --

    #[test]
    fn is_activity_fact_matches_by_concept() {
        let fact = CognitiveFact {
            node_id: "n1".into(),
            concept: "dev-activity:user:1:0".into(),
            content: "something".into(),
            confidence: 0.5,
            source_id: "s".into(),
            tags: vec![],
        };
        assert!(is_activity_fact(&fact));
    }

    #[test]
    fn is_activity_fact_matches_by_tag() {
        let fact = CognitiveFact {
            node_id: "n1".into(),
            concept: "other".into(),
            content: "something".into(),
            confidence: 0.5,
            source_id: "s".into(),
            tags: vec!["developer-activity".into()],
        };
        assert!(is_activity_fact(&fact));
    }

    #[test]
    fn is_activity_fact_rejects_unrelated() {
        let fact = CognitiveFact {
            node_id: "n1".into(),
            concept: "research:topic".into(),
            content: "something".into(),
            confidence: 0.5,
            source_id: "s".into(),
            tags: vec!["research".into()],
        };
        assert!(!is_activity_fact(&fact));
    }

    // -- summarize_extraction --

    #[test]
    fn summarize_extraction_no_proposals() {
        let result = ExtractionResult {
            facts_examined: 5,
            proposals: vec![],
        };
        let s = summarize_extraction(&result);
        assert!(s.contains("5 activity fact(s)"));
        assert!(s.contains("no new research ideas"));
    }

    #[test]
    fn summarize_extraction_with_proposals() {
        let result = ExtractionResult {
            facts_examined: 10,
            proposals: vec![IdeaProposal {
                title: "research: LLM agents".into(),
                body: "body".into(),
                rationale: "r".into(),
                source_developer: "dev".into(),
                matched_keywords: vec!["llm".into()],
            }],
        };
        let s = summarize_extraction(&result);
        assert!(s.contains("surfaced 1 idea(s)"));
        assert!(s.contains("LLM agents"));
    }
}
