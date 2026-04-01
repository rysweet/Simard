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
    fn build_enriched_objective(&self, input: &BaseTypeTurnInput) -> String {
        let context = prepare_turn_context(
            &input.objective,
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

        // Build enriched objective and dispatch via terminal infrastructure.
        let enriched_objective = self.build_enriched_objective(&input);
        let enriched_input = BaseTypeTurnInput::objective_only(enriched_objective);

        // Delegate to the terminal session infrastructure. If the terminal
        // turn fails, wrap the error with copilot-specific context.
        let terminal_outcome =
            execute_terminal_turn(&self.descriptor, &self.request, &enriched_input).map_err(
                |err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("copilot terminal turn failed: {err}"),
                },
            )?;

        // Augment evidence with copilot-specific metadata.
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
            execution_summary: terminal_outcome.execution_summary,
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
fn build_copilot_terminal_objective(
    config: &CopilotAdapterConfig,
    formatted_prompt: &str,
) -> String {
    let mut objective = String::new();

    if let Some(ref cwd) = config.working_directory {
        objective.push_str(&format!("working-directory: {cwd}\n"));
    }

    // The command sends the formatted prompt to the copilot via stdin echo.
    // We use printf to avoid issues with special characters in the prompt.
    let escaped = formatted_prompt
        .replace('\\', "\\\\")
        .replace('\'', "'\\''");
    objective.push_str(&format!(
        "command: printf '%s' '{}' | {}\n",
        escaped, config.command
    ));
    objective.push_str("wait-for: $\n");

    objective
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
    fn build_copilot_objective_includes_command() {
        let config = CopilotAdapterConfig {
            command: "my-copilot run".to_string(),
            working_directory: Some("/tmp/work".to_string()),
        };
        let prompt = "Do the thing.";
        let objective = build_copilot_terminal_objective(&config, prompt);
        assert!(objective.contains("my-copilot run"));
        assert!(objective.contains("working-directory: /tmp/work"));
        assert!(objective.contains("Do the thing."));
    }
}
