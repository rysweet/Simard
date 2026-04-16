//! Base type trait contracts and shared types.
//!
//! A "base type" is an agent execution substrate — the runtime that Simard
//! delegates work to. This module defines the trait pair (`BaseTypeFactory` +
//! `BaseTypeSession`), shared data structures, and helper functions. Concrete
//! adapter implementations live in their own modules:
//!
//! - `base_type_rustyclawd` — production adapter using rustyclawd-core SDK
//! - `base_type_copilot` — GitHub Copilot SDK adapter via PTY
//! - `base_type_claude_agent_sdk` — Claude Agent SDK (structural)
//! - `base_type_ms_agent` — Microsoft Agent Framework (structural)
//! - `test_support` — lightweight test adapter returning canned results

use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter, Write};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::metadata::BackendDescriptor;
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct BaseTypeId(String);

impl BaseTypeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for BaseTypeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Display for BaseTypeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BaseTypeCapability {
    PromptAssets,
    SessionLifecycle,
    Memory,
    Evidence,
    Reflection,
    TerminalSession,
}

impl Display for BaseTypeCapability {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::PromptAssets => "prompt-assets",
            Self::SessionLifecycle => "session-lifecycle",
            Self::Memory => "memory",
            Self::Evidence => "evidence",
            Self::Reflection => "reflection",
            Self::TerminalSession => "terminal-session",
        };
        f.write_str(label)
    }
}

pub fn capability_set(
    capabilities: impl IntoIterator<Item = BaseTypeCapability>,
) -> BTreeSet<BaseTypeCapability> {
    capabilities.into_iter().collect()
}

pub fn standard_session_capabilities() -> BTreeSet<BaseTypeCapability> {
    capability_set([
        BaseTypeCapability::PromptAssets,
        BaseTypeCapability::SessionLifecycle,
        BaseTypeCapability::Memory,
        BaseTypeCapability::Evidence,
        BaseTypeCapability::Reflection,
    ])
}

pub fn joined_prompt_ids(prompt_assets: &[PromptAssetRef]) -> String {
    let mut joined = String::with_capacity(prompt_assets.len() * 24);
    for (index, asset) in prompt_assets.iter().enumerate() {
        if index > 0 {
            joined.push_str(", ");
        }
        let _ = write!(&mut joined, "{}", asset.id);
    }
    joined
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeDescriptor {
    pub id: BaseTypeId,
    pub backend: BackendDescriptor,
    pub capabilities: BTreeSet<BaseTypeCapability>,
    pub supported_topologies: BTreeSet<RuntimeTopology>,
}

impl BaseTypeDescriptor {
    pub fn supports_topology(&self, topology: RuntimeTopology) -> bool {
        self.supported_topologies.contains(&topology)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeSessionRequest {
    pub session_id: SessionId,
    pub mode: OperatingMode,
    pub topology: RuntimeTopology,
    pub prompt_assets: Vec<PromptAssetRef>,
    pub runtime_node: RuntimeNodeId,
    pub mailbox_address: RuntimeAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeTurnInput {
    pub objective: String,
    /// System-level identity context loaded from the manifest's prompt assets.
    /// Used by LLM-calling adapters to construct system prompts.
    pub identity_context: String,
    /// Additional prompt preamble for the turn (e.g., from enrichment bridges).
    pub prompt_preamble: String,
}

impl BaseTypeTurnInput {
    /// Create a turn input with just an objective and empty context fields.
    /// Useful in tests and for adapters that don't need LLM system prompts.
    pub fn objective_only(objective: impl Into<String>) -> Self {
        Self {
            objective: objective.into(),
            identity_context: String::new(),
            prompt_preamble: String::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseTypeOutcome {
    pub plan: String,
    pub execution_summary: String,
    pub evidence: Vec<String>,
}

pub trait BaseTypeSession: Send {
    fn descriptor(&self) -> &BaseTypeDescriptor;

    fn open(&mut self) -> SimardResult<()>;

    fn run_turn(&mut self, input: BaseTypeTurnInput) -> SimardResult<BaseTypeOutcome>;

    fn close(&mut self) -> SimardResult<()>;
}

pub trait BaseTypeFactory: Send + Sync {
    fn descriptor(&self) -> &BaseTypeDescriptor;

    fn open_session(
        &self,
        request: BaseTypeSessionRequest,
    ) -> SimardResult<Box<dyn BaseTypeSession>>;
}

pub fn ensure_session_not_closed(
    descriptor: &BaseTypeDescriptor,
    is_closed: bool,
    action: &str,
) -> SimardResult<()> {
    if is_closed {
        return Err(SimardError::InvalidBaseTypeSessionState {
            base_type: descriptor.id.to_string(),
            action: action.to_string(),
            reason: "session is already closed".to_string(),
        });
    }

    Ok(())
}

pub fn ensure_session_open(
    descriptor: &BaseTypeDescriptor,
    is_open: bool,
    action: &str,
) -> SimardResult<()> {
    if !is_open {
        return Err(SimardError::InvalidBaseTypeSessionState {
            base_type: descriptor.id.to_string(),
            action: action.to_string(),
            reason: "session must be opened before turns can run".to_string(),
        });
    }

    Ok(())
}

pub fn ensure_session_not_already_open(
    descriptor: &BaseTypeDescriptor,
    is_open: bool,
) -> SimardResult<()> {
    if is_open {
        return Err(SimardError::InvalidBaseTypeSessionState {
            base_type: descriptor.id.to_string(),
            action: "open".to_string(),
            reason: "session is already open".to_string(),
        });
    }

    Ok(())
}

/// Collect evidence from a completed child process output. Shared by adapters
/// that defer to process-based execution.
pub fn process_output_evidence(prefix: &str, output: &std::process::Output) -> Vec<String> {
    let exit_code = output.status.code().unwrap_or(-1);

    let mut evidence = Vec::with_capacity(5);
    evidence.push(format!("{prefix}-exit-code={exit_code}"));
    evidence.push(format!("{prefix}-stdout-bytes={}", output.stdout.len()));
    evidence.push(format!("{prefix}-stderr-bytes={}", output.stderr.len()));
    if !output.stdout.is_empty() {
        let stdout = String::from_utf8_lossy(&output.stdout[..output.stdout.len().min(1024)]);
        evidence.push(format!("{prefix}-stdout-head={stdout}"));
    }
    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr[..output.stderr.len().min(512)]);
        evidence.push(format!("{prefix}-stderr-head={stderr}"));
    }
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_set_collects_unique_capabilities() {
        let caps = capability_set([
            BaseTypeCapability::Memory,
            BaseTypeCapability::Evidence,
            BaseTypeCapability::Memory,
        ]);
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&BaseTypeCapability::Memory));
        assert!(caps.contains(&BaseTypeCapability::Evidence));
    }

    #[test]
    fn base_type_id_display_and_from() {
        let id = BaseTypeId::new("test-adapter");
        assert_eq!(id.to_string(), "test-adapter");
        assert_eq!(id.as_str(), "test-adapter");
        let from: BaseTypeId = "test-adapter".into();
        assert_eq!(from, id);
    }

    #[test]
    fn turn_input_objective_only_sets_empty_context() {
        let input = BaseTypeTurnInput::objective_only("test objective");
        assert_eq!(input.objective, "test objective");
        assert!(input.identity_context.is_empty());
        assert!(input.prompt_preamble.is_empty());
    }
}
