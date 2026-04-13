//! Turn context preparation and output parsing for base type adapters.
//!
//! A "turn" is a single request-response exchange with an LLM backend. This
//! module handles three responsibilities:
//!
//! 1. **Prepare** — gather memory facts, knowledge results, and procedures
//!    from the bridges and bundle them into a [`TurnContext`].
//! 2. **Format** — serialize the context into a single string prompt that an
//!    LLM adapter can submit.
//! 3. **Parse** — extract structured [`TurnOutput`] from raw LLM text output.

use std::fmt::Write;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::knowledge_bridge::{KnowledgeBridge, KnowledgeQueryResult};
use crate::knowledge_context::enrich_planning_context;
use crate::memory_cognitive::{CognitiveFact, CognitiveProcedure};

/// Maximum number of memory facts to inject per turn.
const MAX_MEMORY_FACTS: u32 = 10;

/// Minimum confidence for memory facts to be included.
const MIN_FACT_CONFIDENCE: f64 = 0.3;

/// Maximum number of procedures to recall per turn.
const MAX_PROCEDURES: u32 = 5;

/// Collected context that informs a single LLM turn.
#[derive(Clone, Debug)]
pub struct TurnContext {
    pub objective: String,
    pub memory_facts: Vec<CognitiveFact>,
    pub knowledge: Vec<KnowledgeQueryResult>,
    pub procedures: Vec<CognitiveProcedure>,
}

/// An action proposed by the LLM in its response.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposedAction {
    pub kind: String,
    pub description: String,
}

/// Structured output parsed from raw LLM text.
#[derive(Clone, Debug)]
pub struct TurnOutput {
    pub actions: Vec<ProposedAction>,
    pub explanation: String,
    /// None when the LLM did not provide a parseable confidence value.
    pub confidence: Option<f64>,
}

/// Prepare a [`TurnContext`] by querying memory and knowledge bridges.
///
/// Both bridges are optional (None = not configured, which is fine).
/// If a bridge IS provided but its call fails, the error propagates — no
/// silent degradation per PHILOSOPHY.md.
pub fn prepare_turn_context(
    objective: &str,
    memory_bridge: Option<&dyn CognitiveMemoryOps>,
    knowledge_bridge: Option<&KnowledgeBridge>,
) -> SimardResult<TurnContext> {
    let memory_facts = match memory_bridge {
        Some(bridge) => bridge.search_facts(objective, MAX_MEMORY_FACTS, MIN_FACT_CONFIDENCE)?,
        None => Vec::new(),
    };

    let procedures = match memory_bridge {
        Some(bridge) => bridge.recall_procedure(objective, MAX_PROCEDURES)?,
        None => Vec::new(),
    };

    let knowledge = match knowledge_bridge {
        Some(bridge) => enrich_planning_context(objective, bridge)?.relevant_knowledge,
        None => Vec::new(),
    };

    Ok(TurnContext {
        objective: objective.to_string(),
        memory_facts,
        knowledge,
        procedures,
    })
}

/// Format a [`TurnContext`] into a prompt string suitable for an LLM.
///
/// The output is a structured text block with labeled sections. Empty
/// sections are omitted to keep the prompt concise.
pub fn format_turn_input(context: &TurnContext) -> String {
    let mut prompt = String::with_capacity(2048);

    let _ = writeln!(prompt, "## Objective\n");
    let _ = writeln!(prompt, "{}\n", context.objective);

    if !context.memory_facts.is_empty() {
        let _ = writeln!(prompt, "## Relevant Memory Facts\n");
        for (i, fact) in context.memory_facts.iter().enumerate() {
            let _ = writeln!(
                prompt,
                "{}. [{}] {} (confidence: {:.2})",
                i + 1,
                fact.concept,
                fact.content,
                fact.confidence
            );
        }
        let _ = writeln!(prompt);
    }

    if !context.procedures.is_empty() {
        let _ = writeln!(prompt, "## Known Procedures\n");
        for proc in &context.procedures {
            let _ = writeln!(prompt, "### {}\n", proc.name);
            if !proc.prerequisites.is_empty() {
                let _ = writeln!(prompt, "Prerequisites: {}", proc.prerequisites.join(", "));
            }
            let _ = writeln!(prompt, "Steps:");
            for (i, step) in proc.steps.iter().enumerate() {
                let _ = writeln!(prompt, "  {}. {step}", i + 1);
            }
            let _ = writeln!(prompt);
        }
    }

    if !context.knowledge.is_empty() {
        let _ = writeln!(prompt, "## Domain Knowledge\n");
        for result in &context.knowledge {
            let _ = writeln!(
                prompt,
                "- {} (confidence: {:.2})",
                result.answer, result.confidence
            );
            for source in &result.sources {
                let _ = write!(prompt, "  Source: {} > {}", source.title, source.section);
                if let Some(url) = &source.url {
                    let _ = write!(prompt, " ({url})");
                }
                let _ = writeln!(prompt);
            }
        }
        let _ = writeln!(prompt);
    }

    let _ = writeln!(
        prompt,
        "## Instructions\n\n\
         Respond with:\n\
         1. ACTIONS: one per line, formatted as `ACTION: <kind> — <description>`\n\
         2. EXPLANATION: a brief rationale\n\
         3. CONFIDENCE: a decimal between 0.0 and 1.0"
    );

    prompt
}

/// Sentinel that marks the start of the actions block.
const ACTION_PREFIX: &str = "ACTION:";

/// Sentinel for the explanation line.
const EXPLANATION_PREFIX: &str = "EXPLANATION:";

/// Sentinel for the confidence line.
const CONFIDENCE_PREFIX: &str = "CONFIDENCE:";

/// Parse raw LLM output text into a structured [`TurnOutput`].
///
/// The parser is lenient: it extracts what it can and falls back to defaults
/// for missing sections. An empty or purely whitespace input is rejected.
pub fn parse_turn_output(raw: &str) -> SimardResult<TurnOutput> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: "turn-parser".to_string(),
            reason: "LLM output is empty".to_string(),
        });
    }

    let mut actions = Vec::new();
    let mut explanation = String::new();
    let mut confidence: Option<f64> = None;

    for line in trimmed.lines() {
        let line = line.trim();

        if let Some(rest) = strip_prefix_case_insensitive(line, ACTION_PREFIX) {
            let rest = rest.trim();
            if let Some((kind, desc)) = rest.split_once('—').or_else(|| rest.split_once(" - ")) {
                let kind = kind.trim().to_string();
                let desc = desc.trim().to_string();
                if !kind.is_empty() && !desc.is_empty() {
                    actions.push(ProposedAction {
                        kind,
                        description: desc,
                    });
                    continue;
                }
            }
            // Fallback: treat the whole line as description with kind "unknown".
            if !rest.is_empty() {
                actions.push(ProposedAction {
                    kind: "unknown".to_string(),
                    description: rest.to_string(),
                });
            }
            continue;
        }

        if let Some(rest) = strip_prefix_case_insensitive(line, EXPLANATION_PREFIX) {
            let rest = rest.trim();
            if !rest.is_empty() {
                explanation = rest.to_string();
            }
            continue;
        }

        if let Some(rest) = strip_prefix_case_insensitive(line, CONFIDENCE_PREFIX) {
            let rest = rest.trim();
            if let Ok(value) = rest.parse::<f64>() {
                confidence = Some(value.clamp(0.0, 1.0));
            }
            continue;
        }

        // Accumulate unrecognized lines into explanation if we have no actions yet.
        if actions.is_empty() && !line.is_empty() && explanation.is_empty() {
            explanation = line.to_string();
        }
    }

    Ok(TurnOutput {
        actions,
        explanation,
        confidence,
    })
}

/// Case-insensitive prefix strip.
fn strip_prefix_case_insensitive<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    if lower.starts_with(&prefix_lower) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_turn_input_includes_objective() {
        let ctx = TurnContext {
            objective: "implement the widget".to_string(),
            memory_facts: vec![],
            knowledge: vec![],
            procedures: vec![],
        };
        let prompt = format_turn_input(&ctx);
        assert!(prompt.contains("implement the widget"));
        assert!(prompt.contains("## Objective"));
        assert!(prompt.contains("## Instructions"));
    }

    #[test]
    fn format_turn_input_includes_facts_when_present() {
        let ctx = TurnContext {
            objective: "test".to_string(),
            memory_facts: vec![CognitiveFact {
                node_id: "n1".to_string(),
                concept: "rust".to_string(),
                content: "systems language".to_string(),
                confidence: 0.9,
                source_id: "s1".to_string(),
                tags: vec![],
            }],
            knowledge: vec![],
            procedures: vec![],
        };
        let prompt = format_turn_input(&ctx);
        assert!(prompt.contains("## Relevant Memory Facts"));
        assert!(prompt.contains("[rust]"));
        assert!(prompt.contains("systems language"));
    }

    #[test]
    fn parse_turn_output_extracts_structured_response() {
        let raw = "\
ACTION: create — Create the new module file
ACTION: test — Write unit tests
EXPLANATION: The module needs creation and verification.
CONFIDENCE: 0.85";

        let output = parse_turn_output(raw).unwrap();
        assert_eq!(output.actions.len(), 2);
        assert_eq!(output.actions[0].kind, "create");
        assert_eq!(output.actions[1].kind, "test");
        assert!(output.explanation.contains("module"));
        assert!((output.confidence.unwrap() - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_turn_output_rejects_empty_input() {
        let result = parse_turn_output("   ");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("empty"));
    }

    #[test]
    fn parse_turn_output_handles_missing_sections() {
        let raw = "Just some raw explanation text.";
        let output = parse_turn_output(raw).unwrap();
        assert!(output.actions.is_empty());
        assert!(output.explanation.contains("explanation"));
        assert!(output.confidence.is_none());
    }

    #[test]
    fn parse_turn_output_clamps_confidence() {
        let raw = "CONFIDENCE: 1.5";
        let output = parse_turn_output(raw).unwrap();
        assert!((output.confidence.unwrap() - 1.0).abs() < f64::EPSILON);
    }
}
