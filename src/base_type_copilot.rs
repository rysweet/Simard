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
use crate::error::{SimardError, SimardResult};
use crate::knowledge_bridge::KnowledgeBridge;
use crate::memory_bridge::CognitiveMemoryBridge;
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
    memory_bridge: Option<CognitiveMemoryBridge>,
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
    fn build_enriched_objective(&self, input: &BaseTypeTurnInput) -> String {
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
            self.memory_bridge.as_ref(),
            self.knowledge_bridge.as_ref(),
        );
        let formatted = format_turn_input(&context);
        build_copilot_terminal_objective(&self.config, &formatted)
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

        let enriched_objective = self.build_enriched_objective(&input);
        let enriched_input = BaseTypeTurnInput::objective_only(enriched_objective);

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
fn build_copilot_terminal_objective(
    config: &CopilotAdapterConfig,
    formatted_prompt: &str,
) -> String {
    let mut objective = String::new();

    if let Some(ref cwd) = config.working_directory {
        objective.push_str(&format!("working-directory: {cwd}\n"));
    }

    // Write the prompt to a temp file to avoid shell quoting issues with
    // large multi-line prompts containing arbitrary characters.
    // Chain the copilot command with `exit` so the shell exits naturally
    // after the copilot finishes — no separate input step needed.
    let escaped = formatted_prompt
        .replace('\\', "\\\\")
        .replace('\'', "'\\''");
    objective.push_str(&format!(
        "command: SIMARD_PROMPT_FILE=$(mktemp /tmp/simard-copilot-prompt.XXXXXX) && \
         printf '%s' '{}' > \"$SIMARD_PROMPT_FILE\" && \
         {} -p \"$(cat \"$SIMARD_PROMPT_FILE\")\" ; \
         rm -f \"$SIMARD_PROMPT_FILE\" ; exit\n",
        escaped, config.command,
    ));

    objective
}

/// Extract the actual copilot LLM response from the terminal transcript
/// embedded in the evidence vector.
///
/// The transcript (available as `terminal-transcript-preview`) contains the
/// command input, the copilot's response, and shell bookkeeping.  We strip
/// the command echo and shell noise to isolate the copilot output.
fn extract_copilot_response_from_evidence(evidence: &[String]) -> String {
    let transcript = evidence
        .iter()
        .find_map(|e| e.strip_prefix("terminal-transcript-preview="))
        .unwrap_or("");

    extract_response_from_transcript(transcript)
}

/// Parse the raw transcript text to isolate the copilot's response.
///
/// The transcript (pipe-delimited from `transcript_preview`) typically looks
/// like:
///   <shell prompt> <command>
///   <copilot output lines...>
///   <shell prompt> exit
///
/// We find the command line, skip it, and take everything up to the `exit`
/// line, stripping shell prompts and empty lines.
fn extract_response_from_transcript(transcript: &str) -> String {
    let lines: Vec<&str> = transcript.split(" | ").collect();

    // Find the command line (contains "amplihack copilot" or SIMARD_PROMPT_FILE)
    let command_pos = lines
        .iter()
        .position(|line| line.contains("amplihack copilot") || line.contains("SIMARD_PROMPT_FILE"));

    let start = command_pos.map_or(0, |p| p + 1);

    lines[start..]
        .iter()
        .filter(|l| {
            let trimmed = l.trim();
            !(trimmed.is_empty()
                || trimmed == "exit"
                || trimmed.ends_with("$ exit")
                || trimmed.ends_with("# exit")
                || trimmed.starts_with("bash-") && trimmed.ends_with('$'))
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse copilot response text and extract a turn context summary for
/// evidence. This is a lightweight wrapper around the turn parser.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeSessionRequest;
    use crate::identity::OperatingMode;
    use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
    use crate::session::SessionId;

    fn test_request() -> BaseTypeSessionRequest {
        BaseTypeSessionRequest {
            session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::new("node-1"),
            mailbox_address: RuntimeAddress::new("addr-1"),
        }
    }

    #[test]
    fn copilot_adapter_creates_session() {
        let adapter = CopilotSdkAdapter::registered("copilot-test").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "copilot-test");
        assert!(
            adapter
                .descriptor()
                .capabilities
                .contains(&BaseTypeCapability::TerminalSession)
        );
    }

    #[test]
    fn copilot_session_lifecycle() {
        let adapter = CopilotSdkAdapter::registered("copilot-lifecycle").unwrap();
        let request = test_request();
        let mut session = adapter.open_session(request).unwrap();

        session.open().unwrap();
        assert!(session.open().is_err()); // Double open
        session.close().unwrap();
        assert!(session.close().is_err()); // Double close
    }

    #[test]
    fn copilot_session_rejects_turn_before_open() {
        let adapter = CopilotSdkAdapter::registered("copilot-pre-open").unwrap();
        let request = test_request();
        let mut session = adapter.open_session(request).unwrap();

        let result = session.run_turn(BaseTypeTurnInput::objective_only("test"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("must be opened"));
    }

    #[test]
    fn copilot_adapter_rejects_unsupported_topology() {
        let adapter = CopilotSdkAdapter::registered("copilot-topo").unwrap();
        let mut request = test_request();
        request.topology = RuntimeTopology::MultiProcess;
        let result = adapter.open_session(request);
        assert!(result.is_err());
    }

    #[test]
    fn copilot_adapter_rejects_command_with_shell_metacharacters() {
        let config = CopilotAdapterConfig {
            command: "echo; rm -rf /".to_string(),
            working_directory: None,
        };
        let result = CopilotSdkAdapter::with_config("copilot-inject", config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("forbidden"));
    }

    #[test]
    fn copilot_adapter_rejects_empty_command() {
        let config = CopilotAdapterConfig {
            command: "  ".to_string(),
            working_directory: None,
        };
        let result = CopilotSdkAdapter::with_config("copilot-empty", config);
        assert!(result.is_err());
    }

    #[test]
    fn build_copilot_objective_includes_command_and_exit() {
        let config = CopilotAdapterConfig {
            command: "my-copilot run".to_string(),
            working_directory: Some("/tmp/work".to_string()),
        };
        let prompt = "Do the thing.";
        let objective = build_copilot_terminal_objective(&config, prompt);
        assert!(objective.contains("my-copilot run"));
        assert!(objective.contains("working-directory: /tmp/work"));
        assert!(objective.contains("Do the thing."));
        assert!(
            objective.contains("; exit"),
            "objective must chain exit to end the shell naturally"
        );
        assert!(
            !objective.contains("wait-for:"),
            "no wait-for — process exits naturally"
        );
        assert!(
            !objective.contains("wait-timeout"),
            "no timeout — process runs to completion"
        );
    }

    #[test]
    fn build_copilot_objective_without_working_directory() {
        let config = CopilotAdapterConfig::default();
        let prompt = "Hello world";
        let objective = build_copilot_terminal_objective(&config, prompt);
        assert!(!objective.contains("working-directory:"));
        assert!(objective.contains("; exit"));
        assert!(objective.contains("amplihack copilot"));
    }

    #[test]
    fn build_copilot_objective_escapes_single_quotes() {
        let config = CopilotAdapterConfig::default();
        let prompt = "it's a test with 'quotes'";
        let objective = build_copilot_terminal_objective(&config, prompt);
        assert!(!objective.contains("it's"), "single quotes must be escaped");
    }

    #[test]
    fn extract_response_from_transcript_isolates_copilot_output() {
        let transcript = "bash-5.2$ SIMARD_PROMPT_FILE=$(mktemp) && printf '%s' 'Hello' > \"$SIMARD_PROMPT_FILE\" && amplihack copilot -p \"$(cat \"$SIMARD_PROMPT_FILE\")\" ; rm -f \"$SIMARD_PROMPT_FILE\" | I'm Simard, your engineering agent. How can I help? | bash-5.2$ exit";
        let response = extract_response_from_transcript(transcript);
        assert!(
            response.contains("I'm Simard"),
            "should extract copilot response: got '{response}'"
        );
        assert!(
            !response.contains("exit"),
            "exit command should be stripped"
        );
    }

    #[test]
    fn extract_response_from_transcript_handles_no_command_marker() {
        let transcript = "some output | actual response text | more text";
        let response = extract_response_from_transcript(transcript);
        assert!(!response.is_empty(), "should return all content lines");
    }

    #[test]
    fn extract_response_from_transcript_handles_empty() {
        let response = extract_response_from_transcript("");
        assert!(response.is_empty() || response.trim().is_empty());
    }

    #[test]
    fn extract_copilot_response_from_evidence_finds_transcript() {
        let evidence = vec![
            "selected-base-type=copilot-test".to_string(),
            "terminal-transcript-preview=SIMARD_PROMPT_FILE=x && amplihack copilot -p test | Hello from Simard | bash-5.2$ exit".to_string(),
        ];
        let response = extract_copilot_response_from_evidence(&evidence);
        assert!(
            response.contains("Hello from Simard"),
            "should extract response: got '{response}'"
        );
    }

    #[test]
    fn extract_copilot_response_from_evidence_handles_missing_preview() {
        let evidence = vec!["selected-base-type=copilot-test".to_string()];
        let response = extract_copilot_response_from_evidence(&evidence);
        assert!(response.is_empty());
    }
}
