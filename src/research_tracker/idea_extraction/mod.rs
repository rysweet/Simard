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
mod tests;
