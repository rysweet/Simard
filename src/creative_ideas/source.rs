//! The production, agent-backed idea source (guideline G3).
//!
//! [`AgenticIdeaSource`] renders the generation prompt asset, runs one agentic
//! turn through the shared [`AgentInvoker`] seam (idle-liveness only — no
//! wall-clock cap), and extracts the structured JSON envelope into
//! [`RawIdea`]s. It is **fail-closed**: a response with no JSON envelope is a
//! hard error, never a silent empty batch.
#![allow(dead_code)]

use crate::cognitive_memory::creative_idea::{MemoryLink, MemoryLinkKind};
use crate::cognitive_threads::threads::creative_ideas::{GenerationInputs, IdeaSource, RawIdea};
use crate::creative_ideas::prompt::{extract_json_value, render_generation_prompt};
use crate::creative_ideas::reviewers::AgentInvoker;
use crate::error::{SimardError, SimardResult};

/// The idea-generation prompt asset (produces a JSON array of ten ideas).
const GENERATE_PROMPT: &str = include_str!("../../prompt_assets/simard/creative_ideas_generate.md");

/// Production [`IdeaSource`] backed by an [`AgentInvoker`].
pub struct AgenticIdeaSource<I: AgentInvoker + Send> {
    invoker: I,
}

impl<I: AgentInvoker + Send> AgenticIdeaSource<I> {
    /// Wrap an agent invoker (test seam).
    pub fn new(invoker: I) -> Self {
        Self { invoker }
    }
}

impl AgenticIdeaSource<crate::creative_ideas::reviewers::SessionAgentInvoker> {
    /// Build the production source (the LLM provider is resolved lazily per turn).
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(crate::creative_ideas::reviewers::SessionAgentInvoker::new())
    }
}

impl<I: AgentInvoker + Send> IdeaSource for AgenticIdeaSource<I> {
    fn generate(&self, inputs: &GenerationInputs, n: usize) -> SimardResult<Vec<RawIdea>> {
        let prompt = render_generation_prompt(GENERATE_PROMPT, inputs, n);
        let raw = self.invoker.invoke(&prompt)?;
        let value = extract_json_value(&raw).ok_or_else(|| SimardError::ReviewUnavailable {
            reason: "creative-ideas generation: response contained no JSON envelope".to_string(),
        })?;
        parse_ideas(&value, n)
    }
}

/// Parse the generation envelope into raw ideas (fail-closed on shape).
fn parse_ideas(value: &serde_json::Value, n: usize) -> SimardResult<Vec<RawIdea>> {
    let arr = value
        .get("ideas")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| SimardError::ReviewUnavailable {
            reason: "creative-ideas generation: envelope had no `ideas` array".to_string(),
        })?;

    let mut ideas = Vec::with_capacity(arr.len().min(n));
    for entry in arr.iter().take(n) {
        let Some(text) = entry
            .get("idea")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            // An entry with no idea text is not a valid idea; drop it rather
            // than fabricate one. (Not a fallback — malformed input handling.)
            continue;
        };
        let rationale = entry
            .get("rationale")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let links = parse_links(entry.get("links"));
        ideas.push(RawIdea {
            idea: text.to_string(),
            links,
            rationale,
        });
    }
    Ok(ideas)
}

/// Parse the optional `links` array. Malformed link kinds are skipped (links are
/// optional supporting references); a link with an empty `node_id` is skipped.
fn parse_links(links: Option<&serde_json::Value>) -> Vec<MemoryLink> {
    let Some(arr) = links.and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|l| {
            let kind_str = l.get("kind").and_then(serde_json::Value::as_str)?;
            let kind: MemoryLinkKind = kind_str.parse().ok()?;
            let node_id = l
                .get("node_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())?;
            Some(MemoryLink::new(kind, node_id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct CannedInvoker {
        response: String,
        prompts: RefCell<Vec<String>>,
    }

    impl AgentInvoker for CannedInvoker {
        fn invoke(&self, prompt: &str) -> SimardResult<String> {
            self.prompts.borrow_mut().push(prompt.to_string());
            Ok(self.response.clone())
        }
    }

    #[test]
    fn parses_ten_ideas_with_links() {
        let mut ideas_json = Vec::new();
        for i in 0..10 {
            ideas_json.push(format!(
                "{{\"idea\": \"idea {i}\", \"rationale\": \"because {i}\", \"links\": [{{\"kind\": \"Goal\", \"node_id\": \"g{i}\"}}]}}"
            ));
        }
        let response = format!("```json\n{{\"ideas\": [{}]}}\n```", ideas_json.join(","));
        let source = AgenticIdeaSource::new(CannedInvoker {
            response,
            prompts: RefCell::new(Vec::new()),
        });
        let out = source
            .generate(&GenerationInputs::default(), 10)
            .expect("gen");
        assert_eq!(out.len(), 10);
        assert_eq!(out[0].idea, "idea 0");
        assert_eq!(out[0].links.len(), 1);
        assert_eq!(out[0].links[0].kind, MemoryLinkKind::Goal);
    }

    #[test]
    fn no_json_envelope_fails_closed() {
        let source = AgenticIdeaSource::new(CannedInvoker {
            response: "I could not generate ideas.".to_string(),
            prompts: RefCell::new(Vec::new()),
        });
        assert!(matches!(
            source.generate(&GenerationInputs::default(), 10),
            Err(SimardError::ReviewUnavailable { .. })
        ));
    }
}
