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
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::base_type_turn::EnrichmentClients;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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

impl FromStr for BaseTypeCapability {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "prompt-assets" => Ok(Self::PromptAssets),
            "session-lifecycle" => Ok(Self::SessionLifecycle),
            "memory" => Ok(Self::Memory),
            "evidence" => Ok(Self::Evidence),
            "reflection" => Ok(Self::Reflection),
            "terminal-session" => Ok(Self::TerminalSession),
            other => Err(format!("unknown base type capability: '{other}'")),
        }
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
    /// Additional prompt preamble for the turn (e.g., from enrichment readers).
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

    /// Run a turn, streaming incremental output chunks to `on_chunk` as the
    /// agent produces them, and returning the final outcome.
    ///
    /// The default implementation runs the turn to completion via
    /// [`BaseTypeSession::run_turn`] and emits the full result as a single
    /// chunk, so adapters that cannot stream incrementally still satisfy the
    /// contract (the caller sees exactly one chunk equal to the final text).
    /// Adapters that support true incremental output (e.g. the
    /// `persistent-agent-proxy`) override this to tee each line as it arrives
    /// (issue #2581).
    fn run_turn_streaming(
        &mut self,
        input: BaseTypeTurnInput,
        on_chunk: &mut dyn FnMut(&str),
    ) -> SimardResult<BaseTypeOutcome> {
        let outcome = self.run_turn(input)?;
        on_chunk(&outcome.execution_summary);
        Ok(outcome)
    }

    fn close(&mut self) -> SimardResult<()>;

    /// Optional memory + knowledge readers used to enrich each turn.
    ///
    /// Defaults to `None` (enrichment not supported / not configured). Adapters
    /// that support memory + knowledge enrichment override this to expose their
    /// stored [`EnrichmentClients`]. See [`BaseTypeSession::enrich_input`].
    fn enrichment(&self) -> Option<&EnrichmentClients> {
        None
    }

    /// Mutable access to this session's [`EnrichmentClients`] so the runtime
    /// (or tests) can inject configured readers after the session is created.
    ///
    /// Defaults to `None` for adapters that do not support enrichment.
    fn enrichment_mut(&mut self) -> Option<&mut EnrichmentClients> {
        None
    }

    /// Normalized memory + knowledge enrichment entry point shared by every
    /// adapter (issue #1665).
    ///
    /// Recalls memory facts/procedures and domain knowledge for the turn's
    /// objective using the session's configured readers, and returns a new
    /// [`BaseTypeTurnInput`] with the rendered enrichment injected into
    /// `prompt_preamble` (the per-turn system/preamble context). The
    /// `objective` and `identity_context` are preserved, so stateful adapters
    /// keep clean conversation history and prompt-folding adapters pick the
    /// enrichment up automatically. When no readers are configured the input is
    /// returned unchanged.
    ///
    /// Before #1665 only `CopilotSdkAdapter` enriched its turns; this provided
    /// method gives every adapter the same enrichment behavior through one
    /// shared call site, so the behavior cannot silently diverge again.
    fn enrich_input(&self, input: &BaseTypeTurnInput) -> SimardResult<BaseTypeTurnInput> {
        match self.enrichment() {
            Some(readers) => readers.enrich(input),
            // No enrichment configured for this session → expected=false so the
            // observability seam logs a benign INFO, never a degrade WARN (#2942).
            None => crate::base_type_turn::enrich_turn_input(input, None, None, false),
        }
    }
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

    // ── BaseTypeCapability serde ────────────────────────────────────

    #[test]
    fn base_type_capability_serializes_to_kebab_case() {
        let json = serde_json::to_string(&BaseTypeCapability::PromptAssets).unwrap();
        assert_eq!(json, "\"prompt-assets\"");
        let json = serde_json::to_string(&BaseTypeCapability::SessionLifecycle).unwrap();
        assert_eq!(json, "\"session-lifecycle\"");
        let json = serde_json::to_string(&BaseTypeCapability::TerminalSession).unwrap();
        assert_eq!(json, "\"terminal-session\"");
    }

    #[test]
    fn base_type_capability_deserializes_from_kebab_case() {
        let cap: BaseTypeCapability = serde_json::from_str("\"prompt-assets\"").unwrap();
        assert_eq!(cap, BaseTypeCapability::PromptAssets);
        let cap: BaseTypeCapability = serde_json::from_str("\"memory\"").unwrap();
        assert_eq!(cap, BaseTypeCapability::Memory);
    }

    #[test]
    fn base_type_capability_roundtrips_through_serde() {
        let caps = [
            BaseTypeCapability::PromptAssets,
            BaseTypeCapability::SessionLifecycle,
            BaseTypeCapability::Memory,
            BaseTypeCapability::Evidence,
            BaseTypeCapability::Reflection,
            BaseTypeCapability::TerminalSession,
        ];
        for cap in caps {
            let json = serde_json::to_string(&cap).unwrap();
            let back: BaseTypeCapability = serde_json::from_str(&json).unwrap();
            assert_eq!(cap, back);
        }
    }

    #[test]
    fn base_type_capability_display_matches_serde_names() {
        let caps = [
            (BaseTypeCapability::PromptAssets, "prompt-assets"),
            (BaseTypeCapability::SessionLifecycle, "session-lifecycle"),
            (BaseTypeCapability::Memory, "memory"),
            (BaseTypeCapability::Evidence, "evidence"),
            (BaseTypeCapability::Reflection, "reflection"),
            (BaseTypeCapability::TerminalSession, "terminal-session"),
        ];
        for (cap, expected) in caps {
            let display_str = cap.to_string();
            let serde_str = serde_json::to_string(&cap).unwrap();
            assert_eq!(display_str, expected, "Display mismatch for {cap:?}");
            assert_eq!(
                serde_str,
                format!("\"{expected}\""),
                "serde mismatch for {cap:?}"
            );
        }
    }

    #[test]
    fn base_type_capability_fromstr_valid() {
        assert_eq!(
            "prompt-assets".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::PromptAssets
        );
        assert_eq!(
            "session-lifecycle".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::SessionLifecycle
        );
        assert_eq!(
            "memory".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::Memory
        );
        assert_eq!(
            "evidence".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::Evidence
        );
        assert_eq!(
            "reflection".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::Reflection
        );
        assert_eq!(
            "terminal-session".parse::<BaseTypeCapability>().unwrap(),
            BaseTypeCapability::TerminalSession
        );
    }

    #[test]
    fn base_type_capability_fromstr_invalid() {
        assert!("unknown".parse::<BaseTypeCapability>().is_err());
        assert!("PromptAssets".parse::<BaseTypeCapability>().is_err());
        assert!("".parse::<BaseTypeCapability>().is_err());
    }
}
