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
    let content = "type=pull_request; repo=org/repo; title=Add feature X; created_at=2024-01-15";
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
