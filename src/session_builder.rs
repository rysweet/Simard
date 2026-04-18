//! Unified session creation across all operating modes.
//!
//! Extracts the `BaseTypeSessionRequest` + adapter factory pattern into a
//! shared [`SessionBuilder`] so meeting, engineer, and future modes construct
//! sessions the same way.
//!
//! The LLM provider is selected by `SIMARD_LLM_PROVIDER` (env var or CLI flag):
//!
//! | Value         | Behaviour                                            |
//! |---------------|------------------------------------------------------|
//! | `copilot`     | Copilot SDK via `gh` auth                             |
//! | `rustyclawd`  | RustyClawd / Anthropic (requires `ANTHROPIC_API_KEY`) **(default)** |

use crate::base_type_copilot::CopilotSdkAdapter;
use crate::base_type_rustyclawd::RustyClawdAdapter;
use crate::base_types::{BaseTypeFactory, BaseTypeSession, BaseTypeSessionRequest};
use crate::identity::OperatingMode;
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

/// Which LLM provider to use for agent sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// GitHub Copilot SDK via `gh` auth.
    Copilot,
    /// RustyClawd / Anthropic (requires `ANTHROPIC_API_KEY`). Default.
    RustyClawd,
}

impl LlmProvider {
    /// Read from `SIMARD_LLM_PROVIDER` env var.  Defaults to `RustyClawd`.
    pub fn from_env() -> Self {
        match std::env::var("SIMARD_LLM_PROVIDER").as_deref() {
            Ok("copilot") => Self::Copilot,
            // "rustyclawd" or anything else (including unset) → RustyClawd
            _ => Self::RustyClawd,
        }
    }
}

/// Builds and opens a `BaseTypeSession` for any operating mode.
///
/// The adapter tag is a logical name (e.g. `"meeting"`, `"engineer-planner"`).
/// The provider suffix is appended automatically based on [`LlmProvider`].
///
/// # Example
///
/// ```ignore
/// let session = SessionBuilder::new(OperatingMode::Meeting)
///     .node_id("meeting-repl")
///     .address("meeting-repl://local")
///     .adapter_tag("meeting")
///     .open();
/// ```
pub struct SessionBuilder {
    mode: OperatingMode,
    topology: RuntimeTopology,
    prompt_assets: Vec<PromptAssetRef>,
    node_id: String,
    address: String,
    adapter_tag: String,
    provider: LlmProvider,
}

impl SessionBuilder {
    /// Create a builder for the given operating mode.
    ///
    /// Defaults:
    /// - topology: `SingleProcess`
    /// - prompt_assets: empty
    /// - node_id / address / adapter_tag: must be set before calling `open`.
    pub fn new(mode: OperatingMode) -> Self {
        Self {
            mode,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: Vec::new(),
            node_id: String::new(),
            address: String::new(),
            adapter_tag: String::new(),
            provider: LlmProvider::from_env(),
        }
    }

    /// Override the runtime topology (default: `SingleProcess`).
    pub fn topology(mut self, topology: RuntimeTopology) -> Self {
        self.topology = topology;
        self
    }

    /// Supply prompt assets for the session.
    pub fn prompt_assets(mut self, assets: Vec<PromptAssetRef>) -> Self {
        self.prompt_assets = assets;
        self
    }

    /// Set the runtime node identifier (e.g. `"meeting-repl"`).
    pub fn node_id(mut self, id: &str) -> Self {
        self.node_id = id.to_owned();
        self
    }

    /// Set the mailbox address (e.g. `"meeting-repl://local"`).
    pub fn address(mut self, addr: &str) -> Self {
        self.address = addr.to_owned();
        self
    }

    /// Set the adapter registration tag (e.g. `"meeting"`).
    ///
    /// This is a logical name — the provider suffix is added automatically.
    /// Legacy tags containing `"rustyclawd"` or `"copilot"` are stripped to
    /// the base name for backward compatibility.
    pub fn adapter_tag(mut self, tag: &str) -> Self {
        // Normalise legacy tags: "meeting-rustyclawd" → "meeting"
        let base = tag.replace("-rustyclawd", "").replace("-copilot", "");
        self.adapter_tag = base;
        self
    }

    /// Explicitly select the LLM provider (overrides `SIMARD_LLM_PROVIDER`).
    pub fn provider(mut self, provider: LlmProvider) -> Self {
        self.provider = provider;
        self
    }

    /// Build the `BaseTypeSessionRequest` from the current builder state.
    pub fn build_request(&self) -> BaseTypeSessionRequest {
        BaseTypeSessionRequest {
            session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
            mode: self.mode,
            topology: self.topology,
            prompt_assets: self.prompt_assets.clone(),
            runtime_node: RuntimeNodeId::new(&self.node_id),
            mailbox_address: RuntimeAddress::new(&self.address),
        }
    }

    /// Open a session using the configured LLM provider.
    ///
    /// Returns `Ok(session)` on success, `Err` with a diagnostic message
    /// describing exactly which step failed.
    #[tracing::instrument(skip(self), fields(provider = ?self.provider, tag = %self.adapter_tag))]
    pub fn open(self) -> Result<Box<dyn BaseTypeSession>, String> {
        let request = self.build_request();
        match self.provider {
            LlmProvider::Copilot => {
                let tag = format!("{}-copilot", self.adapter_tag);
                let factory = CopilotSdkAdapter::registered(&tag)
                    .map_err(|e| format!("CopilotSdkAdapter::registered({}): {}", tag, e))?;
                let mut session = factory
                    .open_session(request)
                    .map_err(|e| format!("CopilotSdkAdapter::open_session({}): {}", tag, e))?;
                session
                    .open()
                    .map_err(|e| format!("CopilotSdkAdapter::session.open({}): {}", tag, e))?;
                Ok(session)
            }
            LlmProvider::RustyClawd => {
                let tag = format!("{}-rustyclawd", self.adapter_tag);
                let factory = RustyClawdAdapter::registered(&tag)
                    .map_err(|e| format!("RustyClawdAdapter::registered({}): {}", tag, e))?;
                let mut session = factory
                    .open_session(request)
                    .map_err(|e| format!("RustyClawdAdapter::open_session({}): {}", tag, e))?;
                session
                    .open()
                    .map_err(|e| format!("RustyClawdAdapter::session.open({}): {}", tag, e))?;
                Ok(session)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_populates_all_fields() {
        let builder = SessionBuilder::new(OperatingMode::Meeting)
            .node_id("test-node")
            .address("test://local")
            .adapter_tag("test-adapter");

        let request = builder.build_request();

        assert_eq!(request.mode, OperatingMode::Meeting);
        assert_eq!(request.topology, RuntimeTopology::SingleProcess);
        assert!(request.prompt_assets.is_empty());
        assert_eq!(request.runtime_node, RuntimeNodeId::new("test-node"));
        assert_eq!(request.mailbox_address, RuntimeAddress::new("test://local"));
    }

    #[test]
    fn adapter_tag_strips_legacy_provider_suffix() {
        let builder = SessionBuilder::new(OperatingMode::Meeting).adapter_tag("meeting-rustyclawd");
        assert_eq!(builder.adapter_tag, "meeting");

        let builder =
            SessionBuilder::new(OperatingMode::Meeting).adapter_tag("review-pipeline-copilot");
        assert_eq!(builder.adapter_tag, "review-pipeline");

        let builder = SessionBuilder::new(OperatingMode::Meeting).adapter_tag("plain-tag");
        assert_eq!(builder.adapter_tag, "plain-tag");
    }

    #[test]
    fn default_provider_is_rustyclawd() {
        // Unless SIMARD_LLM_PROVIDER is set, default is RustyClawd.
        unsafe { std::env::remove_var("SIMARD_LLM_PROVIDER") };
        assert_eq!(LlmProvider::from_env(), LlmProvider::RustyClawd);
    }

    #[test]
    fn provider_override_is_respected() {
        let builder =
            SessionBuilder::new(OperatingMode::Engineer).provider(LlmProvider::RustyClawd);
        assert_eq!(builder.provider, LlmProvider::RustyClawd);
    }

    #[test]
    fn open_does_not_panic() {
        let session = SessionBuilder::new(OperatingMode::Meeting)
            .node_id("test-node")
            .address("test://local")
            .adapter_tag("nonexistent-adapter")
            .open();

        // The adapter may or may not open depending on auth — no panic is the invariant.
        drop(session);
    }

    #[test]
    fn topology_override() {
        let builder = SessionBuilder::new(OperatingMode::Engineer)
            .topology(RuntimeTopology::SingleProcess)
            .node_id("eng")
            .address("eng://local")
            .adapter_tag("eng-adapter");

        let request = builder.build_request();
        assert_eq!(request.topology, RuntimeTopology::SingleProcess);
    }
}
