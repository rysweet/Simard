//! Real LocalHarnessAdapter — a simpler base type adapter that runs a
//! configurable local command via the PTY infrastructure and captures output.
//!
//! Unlike the copilot adapter, the harness adapter does not inject memory or
//! knowledge context. It is intended for local testing and for running
//! arbitrary commands through the same session lifecycle as real adapters.

use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;
use crate::terminal_session::execute_terminal_turn;

/// Configuration for the local harness adapter.
#[derive(Clone, Debug, Default)]
pub struct HarnessConfig {
    /// The command to run for each turn. If `None`, the objective text is
    /// passed directly to the terminal session (the existing behavior).
    pub command: Option<String>,
    /// Shell to use (overrides the default /usr/bin/bash).
    pub shell: Option<String>,
    /// Working directory for command execution.
    pub working_directory: Option<String>,
}

/// A base type factory that creates sessions running a local command through
/// the PTY infrastructure. Suitable for testing and local development.
#[derive(Debug)]
pub struct RealLocalHarnessAdapter {
    descriptor: BaseTypeDescriptor,
    config: HarnessConfig,
}

impl RealLocalHarnessAdapter {
    /// Create an adapter with default configuration that passes objectives
    /// directly to the terminal session infrastructure.
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        Self::with_config(id, HarnessConfig::default())
    }

    /// Create an adapter with explicit configuration.
    pub fn with_config(id: impl Into<String>, config: HarnessConfig) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "local-harness::pty-session",
                    "registered-base-type:local-harness",
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

    /// Access the harness configuration.
    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }
}

impl BaseTypeFactory for RealLocalHarnessAdapter {
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

        Ok(Box::new(RealLocalHarnessSession {
            descriptor: self.descriptor.clone(),
            config: self.config.clone(),
            request,
            is_open: false,
            is_closed: false,
            turn_count: 0,
        }))
    }
}

#[derive(Debug)]
struct RealLocalHarnessSession {
    descriptor: BaseTypeDescriptor,
    config: HarnessConfig,
    request: BaseTypeSessionRequest,
    is_open: bool,
    is_closed: bool,
    turn_count: u32,
}

impl RealLocalHarnessSession {
    /// Build the terminal objective from the turn input and harness config.
    fn build_terminal_objective(&self, input: &BaseTypeTurnInput) -> String {
        let mut objective = String::new();

        if let Some(ref shell) = self.config.shell {
            objective.push_str(&format!("shell: {shell}\n"));
        }
        if let Some(ref cwd) = self.config.working_directory {
            objective.push_str(&format!("working-directory: {cwd}\n"));
        }

        match &self.config.command {
            Some(command) => {
                // Wrap the configured command, passing the objective as an
                // argument via echo/pipe.
                let escaped = input.objective.replace('\\', "\\\\").replace('\'', "'\\''");
                objective.push_str(&format!("command: printf '%s' '{escaped}' | {command}\n"));
                objective.push_str("wait-for: $\n");
            }
            None => {
                // Pass the raw objective directly as terminal steps.
                objective.push_str(&input.objective);
                if !input.objective.ends_with('\n') {
                    objective.push('\n');
                }
            }
        }

        objective
    }
}

impl BaseTypeSession for RealLocalHarnessSession {
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

        let terminal_objective = self.build_terminal_objective(&input);
        let terminal_input = BaseTypeTurnInput::objective_only(terminal_objective);

        let terminal_outcome =
            execute_terminal_turn(&self.descriptor, &self.request, &terminal_input).map_err(
                |err| SimardError::AdapterInvocationFailed {
                    base_type: self.descriptor.id.to_string(),
                    reason: format!("harness terminal turn failed: {err}"),
                },
            )?;

        let objective_summary = objective_metadata(&input.objective);
        let mut evidence = terminal_outcome.evidence;
        evidence.push(format!("harness-adapter-turn={}", self.turn_count));
        if let Some(ref cmd) = self.config.command {
            evidence.push(format!("harness-adapter-command={cmd}"));
        }

        Ok(BaseTypeOutcome {
            plan: format!(
                "Local harness adapter dispatched {} via terminal on '{}' (turn {}).",
                objective_summary, self.request.topology, self.turn_count,
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

#[cfg(test)]
mod tests {
    use super::*;
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
    fn harness_adapter_creates_session() {
        let adapter = RealLocalHarnessAdapter::registered("harness-test").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "harness-test");
        assert!(
            adapter
                .descriptor()
                .capabilities
                .contains(&BaseTypeCapability::TerminalSession)
        );
    }

    #[test]
    fn harness_session_lifecycle() {
        let adapter = RealLocalHarnessAdapter::registered("harness-lc").unwrap();
        let request = test_request();
        let mut session = adapter.open_session(request).unwrap();

        session.open().unwrap();
        assert!(session.open().is_err());
        session.close().unwrap();
        assert!(session.close().is_err());
    }

    #[test]
    fn harness_session_rejects_turn_before_open() {
        let adapter = RealLocalHarnessAdapter::registered("harness-pre").unwrap();
        let request = test_request();
        let mut session = adapter.open_session(request).unwrap();

        let result = session.run_turn(BaseTypeTurnInput::objective_only("echo hello"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("must be opened"));
    }

    #[test]
    fn harness_adapter_rejects_unsupported_topology() {
        let adapter = RealLocalHarnessAdapter::registered("harness-topo").unwrap();
        let mut request = test_request();
        request.topology = RuntimeTopology::MultiProcess;
        let result = adapter.open_session(request);
        assert!(result.is_err());
    }

    #[test]
    fn build_terminal_objective_without_command() {
        let session = RealLocalHarnessSession {
            descriptor: RealLocalHarnessAdapter::registered("t")
                .unwrap()
                .descriptor
                .clone(),
            config: HarnessConfig::default(),
            request: test_request(),
            is_open: true,
            is_closed: false,
            turn_count: 0,
        };
        let input = BaseTypeTurnInput::objective_only("command: echo hello\nwait-for: hello");
        let objective = session.build_terminal_objective(&input);
        assert!(objective.contains("echo hello"));
        assert!(objective.contains("wait-for: hello"));
    }

    #[test]
    fn build_terminal_objective_with_command() {
        let session = RealLocalHarnessSession {
            descriptor: RealLocalHarnessAdapter::registered("t2")
                .unwrap()
                .descriptor
                .clone(),
            config: HarnessConfig {
                command: Some("cat".to_string()),
                shell: Some("/bin/sh".to_string()),
                working_directory: Some("/tmp".to_string()),
            },
            request: test_request(),
            is_open: true,
            is_closed: false,
            turn_count: 0,
        };
        let input = BaseTypeTurnInput::objective_only("hello world");
        let objective = session.build_terminal_objective(&input);
        assert!(objective.contains("shell: /bin/sh"));
        assert!(objective.contains("working-directory: /tmp"));
        assert!(objective.contains("cat"));
        assert!(objective.contains("hello world"));
    }
}
