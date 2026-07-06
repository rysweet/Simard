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
use std::path::{Path, PathBuf};

use crate::base_types::BaseTypeTurnInput;
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::knowledge_client::{KnowledgeClient, KnowledgeQueryResult};
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
    memory_client: Option<&dyn CognitiveMemoryOps>,
    knowledge_client: Option<&KnowledgeClient>,
) -> SimardResult<TurnContext> {
    let memory_facts = match memory_client {
        Some(bridge) => bridge.search_facts(objective, MAX_MEMORY_FACTS, MIN_FACT_CONFIDENCE)?,
        None => Vec::new(),
    };

    let procedures = match memory_client {
        // ws2 #2295: route base-type adapter recall through the same
        // tokenized helper the OODA preparation phase uses. The
        // previous direct `recall_procedure(objective, MAX_PROCEDURES)`
        // call passed the entire natural-language objective to a
        // single Cypher CONTAINS, which never matched any stored
        // procedure name and starved the prompt of distilled
        // procedures regardless of how many cycles had run. See
        // `crate::memory_consolidation::recall_procedures_for_objective`
        // for the unification contract and case-folding invariant.
        Some(bridge) => crate::memory_consolidation::recall_procedures_for_objective(
            bridge,
            objective,
            MAX_PROCEDURES,
        )?,
        None => Vec::new(),
    };

    let knowledge = match knowledge_client {
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

    prompt.push_str(&render_enrichment_block(context));

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

/// Render only the memory/knowledge enrichment sections of a [`TurnContext`]
/// — the relevant memory facts, known procedures, and domain knowledge —
/// without the surrounding `## Objective` / `## Instructions` scaffold.
///
/// Returns an empty string when no enrichment is present. This is the shared
/// rendering used both by [`format_turn_input`] (which wraps it with the
/// objective + instructions) and by [`enrich_turn_input`] (which injects it
/// into a turn's prompt preamble / system prompt). Centralizing the rendering
/// keeps every adapter's enrichment output identical (issue #1665).
pub fn render_enrichment_block(context: &TurnContext) -> String {
    let mut prompt = String::with_capacity(1024);

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

    prompt
}

/// A bundle of optional memory + knowledge bridges used to enrich a turn.
///
/// This is the single, normalized home for the enrichment bridges. Every
/// base-type adapter routes its turn through the same call site
/// ([`EnrichmentClients::enrich`] / [`enrich_turn_input`]), eliminating the
/// divergence flagged in issue #1665 where only the Copilot adapter queried
/// memory and knowledge.
///
/// Both bridges are optional: `None` means "not configured", which is fine and
/// yields an unenriched (objective-only) prompt. When a bridge IS configured
/// but its call fails, the error propagates — no silent degradation, per
/// PHILOSOPHY.md.
#[derive(Default)]
pub struct EnrichmentClients {
    pub memory: Option<Box<dyn CognitiveMemoryOps>>,
    pub knowledge: Option<KnowledgeClient>,
}

impl EnrichmentClients {
    /// Create an empty bundle (no bridges configured).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any enrichment bridge is configured.
    pub fn is_configured(&self) -> bool {
        self.memory.is_some() || self.knowledge.is_some()
    }

    /// Enrich `input` with recalled memory + knowledge using the configured
    /// bridges, returning a new [`BaseTypeTurnInput`].
    pub fn enrich(&self, input: &BaseTypeTurnInput) -> SimardResult<BaseTypeTurnInput> {
        enrich_turn_input(input, self.memory.as_deref(), self.knowledge.as_ref())
    }
}

impl std::fmt::Debug for EnrichmentClients {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bridges are not `Debug`; surface only whether each is configured.
        f.debug_struct("EnrichmentClients")
            .field("memory", &self.memory.is_some())
            .field("knowledge", &self.knowledge.is_some())
            .finish()
    }
}

/// Where a base-type session sources its memory + knowledge enrichment bridges.
///
/// The default is [`EnrichmentSource::Disabled`] so that lightweight callers and
/// unit tests incur no filesystem side effects (opening a cognitive-memory
/// store, launching native bridges). The live production path
/// ([`crate::session_builder::SessionBuilder`]) opts in via each adapter's
/// `with_enrichment` builder, wiring the same cognitive-memory store and native
/// knowledge bridge the rest of the runtime uses so each turn is enriched with
/// relevant memory facts, procedures, and domain knowledge.
///
/// This is the single, shared home for the enrichment-source policy used by
/// every adapter that supports production enrichment (Copilot — issue #1664;
/// RustyClawd — issue #2383). Centralizing it here keeps the launch + degrade
/// behavior identical across adapters instead of being duplicated per adapter.
#[derive(Clone, Debug, Default)]
pub enum EnrichmentSource {
    /// No enrichment bridges. [`EnrichmentSource::resolve`] yields an empty
    /// [`EnrichmentClients`], so `enrich_input` returns the input unchanged and
    /// only the `## Objective` section is emitted.
    #[default]
    Disabled,
    /// Launch the native cognitive-memory + knowledge bridges lazily (on
    /// `open_session`), reading memory from `state_root`. A launch failure logs
    /// and degrades that bridge to `None` (never panics).
    Native { state_root: PathBuf },
}

impl EnrichmentSource {
    /// Resolve this source into concrete [`EnrichmentClients`].
    ///
    /// [`EnrichmentSource::Disabled`] yields an empty bundle (no side effects).
    /// [`EnrichmentSource::Native`] launches the native cognitive-memory +
    /// knowledge bridges via [`launch_enrichment_bridges`], degrading any
    /// unavailable bridge to `None` without panicking.
    pub fn resolve(&self) -> EnrichmentClients {
        match self {
            EnrichmentSource::Disabled => EnrichmentClients::new(),
            EnrichmentSource::Native { state_root } => {
                let (memory, knowledge) = launch_enrichment_bridges(state_root);
                EnrichmentClients { memory, knowledge }
            }
        }
    }
}

/// Launch the cognitive-memory and knowledge bridges that enrich each turn,
/// degrading gracefully when either is unavailable.
///
/// Memory is obtained via [`crate::ooda_loop::connect_memory`] (the same
/// IPC-aware connector recipe steps use, sharing the daemon's live store when
/// one is running and otherwise opening the library-backed store directly).
/// Knowledge uses the in-process native transport from
/// [`crate::rpc_subprocess_launcher::launch_knowledge_client_native`].
///
/// Mirrors the honest-degradation contract of
/// [`crate::rpc_subprocess_launcher::launch_all_bridges`]: a launch failure is logged
/// and yields `None` for that bridge so turn dispatch proceeds without that
/// enrichment rather than aborting. Neither failure path panics.
///
/// Shared by every adapter that supports production enrichment (Copilot —
/// issue #1664; RustyClawd — issue #2383) so the launcher exists exactly once.
pub fn launch_enrichment_bridges(
    state_root: &Path,
) -> (Option<Box<dyn CognitiveMemoryOps>>, Option<KnowledgeClient>) {
    let memory = match crate::ooda_loop::connect_memory(state_root) {
        Ok(memory) => Some(memory),
        Err(error) => {
            eprintln!(
                "[simard] base-type adapter: cognitive-memory bridge unavailable — memory \
                 enrichment disabled for this session: {error}"
            );
            None
        }
    };

    let knowledge = match crate::rpc_subprocess_launcher::launch_knowledge_client_native() {
        Ok(knowledge) => Some(knowledge),
        Err(error) => {
            eprintln!(
                "[simard] base-type adapter: knowledge bridge unavailable — knowledge \
                 enrichment disabled for this session: {error}"
            );
            None
        }
    };

    (memory, knowledge)
}

/// Enrich a turn input with recalled memory facts/procedures and domain
/// knowledge, returning a new [`BaseTypeTurnInput`] with the rendered
/// enrichment block appended to its `prompt_preamble`.
///
/// This is the single normalized enrichment entry point shared by every
/// base-type adapter (issue #1665). The memory + knowledge are recalled for the
/// turn's `objective`, rendered via [`render_enrichment_block`], and injected
/// into `prompt_preamble` — the field adapters surface to the model as
/// per-turn system/preamble context. The `objective` and `identity_context`
/// are preserved unchanged, so:
///
/// * stateful adapters (e.g. RustyClawd) keep a clean conversation history
///   (the user message stays the bare objective) while the enrichment rides
///   along in the system prompt, and
/// * prompt-folding adapters (e.g. Copilot) pick the enrichment up
///   automatically because they already fold `prompt_preamble` into the
///   submitted prompt.
///
/// When no bridges are configured (or they return nothing) the input is
/// returned unchanged — identical to the previous unenriched behavior, just
/// reachable from every adapter.
pub fn enrich_turn_input(
    input: &BaseTypeTurnInput,
    memory_client: Option<&dyn CognitiveMemoryOps>,
    knowledge_client: Option<&KnowledgeClient>,
) -> SimardResult<BaseTypeTurnInput> {
    let context = prepare_turn_context(&input.objective, memory_client, knowledge_client)?;
    let block = render_enrichment_block(&context);

    let prompt_preamble = if block.is_empty() {
        input.prompt_preamble.clone()
    } else if input.prompt_preamble.is_empty() {
        block.trim_end().to_string()
    } else {
        format!("{}\n\n{}", input.prompt_preamble, block.trim_end())
    };

    Ok(BaseTypeTurnInput {
        objective: input.objective.clone(),
        identity_context: input.identity_context.clone(),
        prompt_preamble,
    })
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
            // Unrecognized format: treat the whole line as description with kind "unknown".
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
                usage_count: 0,
                last_accessed_at: None,
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

    // ── enrich_turn_input / EnrichmentClients ───────────────────────

    #[test]
    fn enrich_turn_input_without_bridges_returns_input_unchanged() {
        let input = BaseTypeTurnInput::objective_only("implement the widget");
        let enriched = enrich_turn_input(&input, None, None).unwrap();
        // No bridges => objective + preamble unchanged, no memory block.
        assert_eq!(enriched.objective, "implement the widget");
        assert!(enriched.prompt_preamble.is_empty());
        assert!(
            !enriched
                .prompt_preamble
                .contains("## Relevant Memory Facts")
        );
    }

    #[test]
    fn enrich_turn_input_preserves_objective_and_identity() {
        let input = BaseTypeTurnInput {
            objective: "do the task".to_string(),
            identity_context: "you are an engineer".to_string(),
            prompt_preamble: "conversation so far".to_string(),
        };
        let enriched = enrich_turn_input(&input, None, None).unwrap();
        // Without bridges the input is returned verbatim.
        assert_eq!(enriched.objective, "do the task");
        assert_eq!(enriched.identity_context, "you are an engineer");
        assert_eq!(enriched.prompt_preamble, "conversation so far");
    }

    #[test]
    fn render_enrichment_block_is_empty_without_context() {
        let ctx = TurnContext {
            objective: "x".to_string(),
            memory_facts: vec![],
            knowledge: vec![],
            procedures: vec![],
        };
        assert!(render_enrichment_block(&ctx).is_empty());
    }

    #[test]
    fn render_enrichment_block_renders_memory_facts() {
        let ctx = TurnContext {
            objective: "x".to_string(),
            memory_facts: vec![CognitiveFact {
                node_id: "n1".to_string(),
                concept: "rust".to_string(),
                content: "systems language".to_string(),
                confidence: 0.9,
                source_id: "s1".to_string(),
                tags: vec![],
                usage_count: 0,
                last_accessed_at: None,
            }],
            knowledge: vec![],
            procedures: vec![],
        };
        let block = render_enrichment_block(&ctx);
        assert!(block.contains("## Relevant Memory Facts"));
        assert!(block.contains("[rust]"));
        assert!(block.contains("systems language"));
        // The block is only the enrichment sections — no objective/instructions.
        assert!(!block.contains("## Objective"));
        assert!(!block.contains("## Instructions"));
    }

    #[test]
    fn enrichment_bridges_default_is_unconfigured() {
        let bridges = EnrichmentClients::new();
        assert!(!bridges.is_configured());
        // enrich() with no bridges returns the input unchanged.
        let input = BaseTypeTurnInput::objective_only("hello");
        let enriched = bridges.enrich(&input).unwrap();
        assert_eq!(enriched.objective, "hello");
        assert!(
            !enriched
                .prompt_preamble
                .contains("## Relevant Memory Facts")
        );
    }

    #[test]
    fn enrichment_bridges_debug_hides_bridge_internals() {
        let bridges = EnrichmentClients::new();
        let debug = format!("{bridges:?}");
        assert!(debug.contains("EnrichmentClients"));
        assert!(debug.contains("memory: false"));
        assert!(debug.contains("knowledge: false"));
    }

    // ── EnrichmentSource / launch_enrichment_bridges ────────────────

    #[test]
    fn enrichment_source_default_is_disabled() {
        let source = EnrichmentSource::default();
        assert!(matches!(source, EnrichmentSource::Disabled));
        // Disabled resolves to an empty, unconfigured bundle (no side effects).
        let bridges = source.resolve();
        assert!(!bridges.is_configured());
    }

    /// The production launch helper wires both real bridges when the state root
    /// can back a cognitive-memory store. Shared by Copilot (#1664) and
    /// RustyClawd (#2383); lives with the launcher it exercises.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn launch_enrichment_bridges_wires_real_bridges_for_valid_state_root() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        std::fs::create_dir_all(&state_root).unwrap();

        let (memory, knowledge) = launch_enrichment_bridges(&state_root);
        assert!(
            memory.is_some(),
            "cognitive-memory bridge must launch for a writable state_root"
        );
        assert!(
            knowledge.is_some(),
            "native knowledge bridge must launch in-process"
        );
    }

    /// `EnrichmentSource::Native` resolves into a fully-configured bundle for a
    /// writable state root — the policy seam shared by every production adapter.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn enrichment_source_native_resolves_configured_bridges() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let state_root = tmp.path().join("state");
        std::fs::create_dir_all(&state_root).unwrap();

        let bridges = EnrichmentSource::Native { state_root }.resolve();
        assert!(
            bridges.memory.is_some(),
            "Native source must wire the memory bridge for a writable state_root"
        );
        assert!(
            bridges.knowledge.is_some(),
            "Native source must wire the knowledge bridge in-process"
        );
    }

    /// A state root that cannot back a store (a regular file) makes the memory
    /// launch fail; it must degrade to `None` without panicking, while the
    /// in-process knowledge bridge still launches.
    #[test]
    fn launch_enrichment_bridges_degrades_when_memory_unavailable() {
        use tempfile::NamedTempFile;
        // A regular file as `state_root` makes `<state_root>/cognitive` uncreatable.
        let file = NamedTempFile::new().unwrap();

        let (memory, knowledge) = launch_enrichment_bridges(file.path());
        assert!(
            memory.is_none(),
            "memory bridge must degrade to None when the state_root cannot back a store"
        );
        assert!(
            knowledge.is_some(),
            "knowledge bridge must still launch when only memory is unavailable"
        );
    }
}
