//! Real CopilotSdkAdapter — a base type that drives `amplihack copilot` via
//! the existing PTY terminal infrastructure and integrates with the memory
//! and knowledge bridges to enrich turn input with relevant context.
//!
//! The adapter launches a copilot subprocess through the PTY `script` wrapper
//! (reusing `terminal_session::execute_terminal_turn`), formats each turn's
//! objective with memory facts and domain knowledge, and parses the copilot's
//! structured output into [`BaseTypeOutcome`].

use crate::base_type_turn::{format_turn_input, parse_turn_output, prepare_turn_context};
use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
};
use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::knowledge_bridge::KnowledgeBridge;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;
use crate::terminal_session::execute_terminal_turn;

/// Default command used to launch the copilot subprocess.
const DEFAULT_COPILOT_COMMAND: &str = "amplihack copilot";

/// Configuration for the copilot adapter.
#[derive(Clone, Debug)]
pub struct CopilotAdapterConfig {
    /// The shell command that launches the copilot.
    pub command: String,
    /// Working directory for the copilot session.
    pub working_directory: Option<String>,
}

impl Default for CopilotAdapterConfig {
    fn default() -> Self {
        Self {
            command: DEFAULT_COPILOT_COMMAND.to_string(),
            working_directory: None,
        }
    }
}

/// A base type factory that creates sessions driving `amplihack copilot`
/// through the PTY infrastructure with memory and knowledge enrichment.
#[derive(Debug)]
pub struct CopilotSdkAdapter {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
}

impl CopilotSdkAdapter {
    /// Create a new CopilotSdkAdapter with default configuration.
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        Self::with_config(id, CopilotAdapterConfig::default())
    }

    /// Create a new CopilotSdkAdapter with explicit configuration.
    ///
    /// The `config.command` is validated to reject shell metacharacters
    /// (`;`, `|`, `&`, `` ` ``, `$`) for defense-in-depth.
    pub fn with_config(id: impl Into<String>, config: CopilotAdapterConfig) -> SimardResult<Self> {
        validate_command(&config.command)?;
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "copilot-sdk::pty-session",
                    "registered-base-type:copilot-sdk",
                    Freshness::now()?,
                ),
                capabilities: capability_set([
                    BaseTypeCapability::PromptAssets,
                    BaseTypeCapability::SessionLifecycle,
                    BaseTypeCapability::Memory,
                    BaseTypeCapability::Evidence,
                    BaseTypeCapability::Reflection,
                    BaseTypeCapability::TerminalSession,
                ]),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            config,
        })
    }

    /// Access the adapter configuration.
    pub fn config(&self) -> &CopilotAdapterConfig {
        &self.config
    }
}

impl BaseTypeFactory for CopilotSdkAdapter {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>> {
        if !self.descriptor.supports_topology(request.topology) {
            return Err(SimardError::UnsupportedTopology {
                base_type: self.descriptor.id.to_string(),
                topology: request.topology,
            });
        }

        Ok(Box::new(CopilotSdkSession {
            descriptor: self.descriptor.clone(),
            config: self.config.clone(),
            request,
            memory_bridge: None,
            knowledge_bridge: None,
            is_open: false,
            is_closed: false,
            turn_count: 0,
        }))
    }
}

/// A live copilot session that enriches objectives with memory and knowledge
/// before dispatching them through the terminal PTY.
struct CopilotSdkSession {
    descriptor: BaseTypeDescriptor,
    config: CopilotAdapterConfig,
    request: BaseTypeSessionRequest,
    memory_bridge: Option<Box<dyn CognitiveMemoryOps>>,
    knowledge_bridge: Option<KnowledgeBridge>,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
}

impl std::fmt::Debug for CopilotSdkSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopilotSdkSession")
            .field("descriptor", &self.descriptor)
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .field("turn_count", &self.turn_count)
            .finish()
    }
}

impl CopilotSdkSession {
    /// Build an enriched terminal objective from the turn input.
    ///
    /// The enriched objective includes a shell/command preamble that the
    /// terminal session infrastructure parses, followed by the formatted
    /// turn context as the command payload.
    ///
    /// When `identity_context` or `prompt_preamble` are provided (e.g. in
    /// meeting mode), they are prepended to the objective so the agent
    /// receives the full conversational context.
    fn build_enriched_objective(&self, input: &BaseTypeTurnInput) -> SimardResult<String> {
        let mut parts = Vec::new();
        if !input.prompt_preamble.is_empty() {
            parts.push(input.prompt_preamble.as_str());
        }
        if !input.identity_context.is_empty() {
            parts.push(input.identity_context.as_str());
        }
        parts.push(&input.objective);

        let combined_objective = parts.join("\n\n");
        let context = prepare_turn_context(
            &combined_objective,
            self.memory_bridge.as_deref(),
            self.knowledge_bridge.as_ref(),
        )?;
        let formatted = format_turn_input(&context);
        Ok(build_copilot_terminal_objective(&self.config, &formatted))
    }
}

impl BaseTypeSession for CopilotSdkSession {
    fn descriptor(&self) -> &BaseTypeDescriptor {
        &self.descriptor
    }

    fn open(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "open")?;
        ensure_session_not_already_open(&self.descriptor, self.is_open)?;
        self.is_open = true;
        Ok(())
    }

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "run_turn")?;
        ensure_session_open(&self.descriptor, self.is_open, "run_turn")?;

        self.turn_count += 1;

        let enriched_objective = self.build_enriched_objective(&input)?;
        let enriched_input = BaseTypeTurnInput::objective_only(enriched_objective);

        tracing::info!(mode = %self.request.mode, turn = self.turn_count, "Copilot adapter: sending turn to Copilot SDK (this may take 30-90s)…");
        let terminal_outcome =
            execute_terminal_turn(&self.descriptor, &self.request, &enriched_input).map_err(
                |err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("copilot terminal turn failed: {err}"),
                },
            )?;

        // Extract the actual LLM response from the transcript.  The transcript
        // contains the command we sent, the copilot's response text, and then
        // our sentinel marker.  We extract everything between the command echo
        // and the sentinel as the meaningful response.
        let response_text = extract_copilot_response_from_evidence(&terminal_outcome.evidence);
        tracing::info!(
            response_len = response_text.len(),
            turn = self.turn_count,
            "Copilot adapter: received response"
        );
        // No fallback: an empty response after stripping noise/footer lines
        // means the Copilot CLI exited without producing a reply (auth
        // failure, rate limit, transcript truncated, etc.). Surface that as
        // an error rather than handing the caller an empty string or — worse
        // — leaking the billing-summary footer as if it were the assistant's
        // reply (issue #1062).
        if response_text.trim().is_empty() {
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.descriptor.id.to_string(),
                reason: "copilot returned no conversational content (auth failure, rate limit, or \
                     truncated transcript). Run `gh auth status` and inspect the most recent \
                     transcript in ~/.simard/agent_logs/."
                    .to_string(),
            });
        }

        // Record cost estimate based on prompt/response character sizes.
        let prompt_chars = enriched_input.objective.len();
        let completion_chars = response_text.len();
        if let Err(e) = crate::cost_tracking::record_cost(
            self.request.session_id.as_str(),
            "copilot",
            prompt_chars,
            completion_chars,
            &format!(
                "copilot turn {} on {}",
                self.turn_count, self.request.topology
            ),
        ) {
            eprintln!("[simard] cost tracking write failed: {e}");
        }

        let objective_summary = objective_metadata(&input.objective);
        let mut evidence = terminal_outcome.evidence;
        evidence.push(format!("copilot-adapter-command={}", self.config.command));
        evidence.push(format!("copilot-adapter-turn={}", self.turn_count));
        evidence.push(format!(
            "copilot-enriched-objective-length={}",
            enriched_input.objective.len()
        ));

        Ok(BaseTypeOutcome {
            plan: format!(
                "Copilot SDK adapter dispatched {} via '{}' on '{}' (turn {}).",
                objective_summary, self.config.command, self.request.topology, self.turn_count,
            ),
            execution_summary: response_text,
            evidence,
        })
    }

    fn close(&mut self) -> SimardResult<()> {
        ensure_session_not_closed(&self.descriptor, self.is_closed, "close")?;
        ensure_session_open(&self.descriptor, self.is_open, "close")?;
        self.is_closed = true;
        Ok(())
    }
}

/// Build a terminal-session-compatible objective string that launches the
/// copilot command with the enriched prompt.
///
/// Strategy: write the prompt to a temp file, run `<copilot-cmd> -p @file`,
/// then `exit` the shell.  The terminal infrastructure calls `finish()` which
/// waits for the process to exit naturally — no artificial timeouts.  The
/// copilot runs to completion however long it takes, and we read the full
/// transcript afterward.
pub(super) fn build_copilot_terminal_objective(
    config: &CopilotAdapterConfig,
    formatted_prompt: &str,
) -> String {
    let mut objective = String::new();

    if let Some(ref cwd) = config.working_directory {
        objective.push_str(&format!("working-directory: {cwd}\n"));
    }

    // Write the prompt to a temp file and pipe it via stdin to the copilot
    // command. Using stdin avoids `-p` flag incompatibility between the Python
    // and Rust versions of amplihack. The `--subprocess-safe` flag skips
    // interactive staging/env updates. Chain with `exit` so the shell exits
    // after the copilot finishes.
    let escaped = formatted_prompt
        .replace('\\', "\\\\")
        .replace('\'', "'\\''");
    objective.push_str(&format!(
        "command: SIMARD_PROMPT_FILE=$(mktemp /tmp/simard-copilot-prompt.XXXXXX) && \
         printf '%s' '{}' > \"$SIMARD_PROMPT_FILE\" && \
         cat \"$SIMARD_PROMPT_FILE\" | {} --subprocess-safe ; \
         rm -f \"$SIMARD_PROMPT_FILE\" ; exit\n",
        escaped, config.command,
    ));

    objective
}

/// Extract the actual copilot LLM response from the terminal transcript
/// embedded in the evidence vector.
///
/// Prefers the full transcript (`terminal-transcript-full`) over the
/// truncated preview.  The full transcript is newline-delimited; the
/// preview is pipe-delimited.
pub(super) fn extract_copilot_response_from_evidence(evidence: &[String]) -> String {
    // Prefer full transcript; use preview if transcript is empty.
    let transcript = evidence
        .iter()
        .find_map(|e| e.strip_prefix("terminal-transcript-full="))
        .or_else(|| {
            evidence
                .iter()
                .find_map(|e| e.strip_prefix("terminal-transcript-preview="))
        })
        .unwrap_or("");

    extract_response_from_transcript(transcript)
}

mod transcript;
pub use transcript::*;

pub fn parse_copilot_response(raw: &str) -> SimardResult<crate::base_type_turn::TurnOutput> {
    parse_turn_output(raw)
}

/// Validate that a command string does not contain shell metacharacters.
///
/// This is a defense-in-depth check. The command is an operator-configured
/// value, not user input, but we reject obvious injection patterns to
/// prevent accidental misconfiguration.
fn validate_command(command: &str) -> SimardResult<()> {
    const FORBIDDEN: &[char] = &[';', '|', '&', '`', '$'];
    if let Some(ch) = command.chars().find(|c| FORBIDDEN.contains(c)) {
        return Err(SimardError::InvalidConfigValue {
            key: "command".to_string(),
            value: command.to_string(),
            help: format!(
                "command contains forbidden shell metacharacter '{ch}'; \
                 use a simple command without shell operators"
            ),
        });
    }
    if command.trim().is_empty() {
        return Err(SimardError::InvalidConfigValue {
            key: "command".to_string(),
            value: command.to_string(),
            help: "command must not be empty".to_string(),
        });
    }
    Ok(())
}
