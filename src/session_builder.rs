//! Unified session creation across all operating modes.
//!
//! Extracts the `BaseTypeSessionRequest` + adapter factory pattern into a
//! shared [`SessionBuilder`] so meeting, engineer, and future modes construct
//! sessions the same way.
//!
//! The LLM provider is selected by [`RuntimeConfig`] (env var
//! `SIMARD_LLM_PROVIDER` ⟶ `~/.simard/config.toml` ⟶ explicit error):
//!
//! | Value         | Behaviour                                            |
//! |---------------|------------------------------------------------------|
//! | `copilot`     | Copilot SDK via `gh` auth                             |
//! | `rustyclawd`  | RustyClawd / Anthropic (requires `ANTHROPIC_API_KEY`) |
//!
//! There is **no silent default**. Callers must obtain an
//! [`LlmProvider`] via [`LlmProvider::resolve`] (which goes through
//! [`RuntimeConfig`]) or pass one explicitly.
//!
//! [`RuntimeConfig`]: crate::runtime_config::RuntimeConfig

use crate::base_type_copilot::CopilotSdkAdapter;
use crate::base_type_rustyclawd::RustyClawdAdapter;
use crate::base_types::{BaseTypeFactory, BaseTypeSession, BaseTypeSessionRequest};
use crate::error::SimardResult;
use crate::identity::OperatingMode;
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

/// Which LLM provider to use for agent sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// GitHub Copilot SDK via `gh` auth.
    Copilot,
    /// RustyClawd / Anthropic (requires `ANTHROPIC_API_KEY`).
    RustyClawd,
}

impl LlmProvider {
    /// Resolve the configured provider via [`RuntimeConfig`]: env var
    /// `SIMARD_LLM_PROVIDER` first, then `~/.simard/config.toml`, then
    /// an error. **No silent default** — a missing provider is an
    /// operator error and must surface as such.
    pub fn resolve() -> SimardResult<Self> {
        Ok(crate::runtime_config::RuntimeConfig::load()?.llm_provider)
    }

    /// The string value for `AMPLIHACK_AGENT_BINARY` env var on subprocesses.
    ///
    /// Each recipe-runner shim sets this on its `Command` so nested agents
    /// use the correct binary (issue #2132).
    pub fn agent_binary_value(&self) -> &'static str {
        match self {
            Self::Copilot => "copilot",
            Self::RustyClawd => "rustyclawd",
        }
    }

    /// Load config and return the agent binary name, or `None` if config is unavailable.
    ///
    /// Convenience wrapper used by recipe-runner shim `new()` constructors
    /// that return `Option<Self>` and want config failure → `None`.
    pub fn resolve_agent_binary() -> Option<&'static str> {
        Some(
            Self::resolve()
                .map_err(|e| tracing::warn!("resolve_agent_binary: config error: {e}"))
                .ok()?
                .agent_binary_value(),
        )
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
/// let session = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::Copilot)
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
    /// Create a builder for the given operating mode and explicit
    /// LLM provider.
    ///
    /// The provider is **required** — there is no default. Production
    /// callers should supply [`LlmProvider::resolve()?`] so the choice
    /// flows from `RuntimeConfig`. Tests pass a literal variant.
    ///
    /// Defaults for other fields:
    /// - topology: `SingleProcess`
    /// - prompt_assets: empty
    /// - node_id / address / adapter_tag: must be set before calling `open`.
    pub fn new(mode: OperatingMode, provider: LlmProvider) -> Self {
        Self {
            mode,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: Vec::new(),
            node_id: String::new(),
            address: String::new(),
            adapter_tag: String::new(),
            provider,
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
        // Check before replacing to avoid redundant allocations in the common case.
        self.adapter_tag = if tag.contains("-rustyclawd") {
            tag.replace("-rustyclawd", "")
        } else if tag.contains("-copilot") {
            tag.replace("-copilot", "")
        } else {
            tag.to_owned()
        };
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
    /// For [`OperatingMode::Meeting`], uses [`PersistentAgentProxy`] which
    /// spawns a persistent interactive agent session (issue #2179). For all
    /// other modes, uses the per-turn adapter (CopilotSdkAdapter or
    /// RustyClawdAdapter).
    ///
    /// Returns `Ok(session)` on success, `Err` with a diagnostic message
    /// describing exactly which step failed.
    #[tracing::instrument(skip(self), fields(provider = ?self.provider, tag = %self.adapter_tag))]
    pub fn open(self) -> Result<Box<dyn BaseTypeSession>, String> {
        // Meeting mode: use persistent interactive proxy (issue #2179)
        if self.mode == OperatingMode::Meeting {
            let mut proxy = crate::meeting_backend::agent_proxy::PersistentAgentProxy::new()
                .map_err(|e| format!("PersistentAgentProxy::new: {e}"))?;
            proxy
                .open()
                .map_err(|e| format!("PersistentAgentProxy::open: {e}"))?;
            return Ok(Box::new(proxy));
        }

        // Non-meeting: build the per-turn adapter session — with production
        // memory + knowledge enrichment wired in — then open it. The build and
        // open steps are separated so the enrichment wiring (which needs no
        // auth) is testable without a live backend; see `build_enriched_session`.
        let (mut session, adapter) = self.build_enriched_session()?;
        session
            .open()
            .map_err(|e| format!("{adapter}::session.open: {e}"))?;
        Ok(session)
    }

    /// Build the per-turn adapter session and populate its memory + knowledge
    /// enrichment bridges, returning the **unopened** session plus the adapter
    /// name (for diagnostics).
    ///
    /// `session.open()` is intentionally left to the caller because it requires
    /// a live backend / credentials. The enrichment bridges, by contrast, are
    /// wired here in `open_session` and need no auth — so this is the exact
    /// production seam that regressed in #2383 (the `RustyClawd` arm built
    /// sessions with empty bridges). The `*_provider_wires_enrichment_*` tests
    /// assert against this method, so dropping a `with_enrichment` call is
    /// caught without standing up a live backend.
    ///
    /// Not called for [`OperatingMode::Meeting`] — [`SessionBuilder::open`]
    /// handles meeting mode via [`PersistentAgentProxy`] before reaching here.
    fn build_enriched_session(self) -> Result<(Box<dyn BaseTypeSession>, &'static str), String> {
        // Inline request construction to move prompt_assets instead of cloning.
        let request = BaseTypeSessionRequest {
            session_id: SessionId::from_uuid(uuid::Uuid::now_v7()),
            mode: self.mode,
            topology: self.topology,
            prompt_assets: self.prompt_assets,
            runtime_node: RuntimeNodeId::new(&self.node_id),
            mailbox_address: RuntimeAddress::new(&self.address),
        };
        match self.provider {
            LlmProvider::Copilot => {
                let tag = format!("{}-copilot", self.adapter_tag);
                // Issue #1664: enable memory + knowledge enrichment on the live
                // production adapter so each turn is enriched with relevant
                // memory facts, procedures, and domain knowledge. This applies
                // to every NON-meeting Copilot session (e.g. the OODA daemon's
                // Orchestrator session and the review pipeline's Engineer
                // session); meeting-mode sessions returned early above via
                // PersistentAgentProxy and are out of scope here. Reads from the
                // default state root (shared with the OODA daemon when running);
                // a bridge launch failure degrades gracefully to no enrichment.
                let factory = CopilotSdkAdapter::registered(&tag)
                    .map_err(|e| format!("CopilotSdkAdapter::registered({}): {}", tag, e))?
                    .with_enrichment(crate::memory_ipc::default_state_root());
                let session = factory
                    .open_session(request)
                    .map_err(|e| format!("CopilotSdkAdapter::open_session({}): {}", tag, e))?;
                Ok((session, "CopilotSdkAdapter"))
            }
            LlmProvider::RustyClawd => {
                let tag = format!("{}-rustyclawd", self.adapter_tag);
                // Issue #2383: enable memory + knowledge enrichment on the
                // RustyClawd production adapter, mirroring the Copilot path
                // above (#1664). #1665 routed `RustyClawd::run_turn` through the
                // shared `enrich_input` entry point, but production sessions
                // were built with empty bridges, so enrichment was a permanent
                // no-op. Wiring `with_enrichment` here populates the bridges so
                // each turn recalls relevant memory facts, procedures, and
                // domain knowledge. Reads from the default state root (shared
                // with the OODA daemon when running); a bridge launch failure
                // degrades gracefully to no enrichment.
                let factory = RustyClawdAdapter::registered(&tag)
                    .map_err(|e| format!("RustyClawdAdapter::registered({}): {}", tag, e))?
                    .with_enrichment(crate::memory_ipc::default_state_root());
                let session = factory
                    .open_session(request)
                    .map_err(|e| format!("RustyClawdAdapter::open_session({}): {}", tag, e))?;
                Ok((session, "RustyClawdAdapter"))
            }
        }
    }
}

/// [`OrchestratorSessionFactory`] backed by [`SessionBuilder`].
///
/// Mints a fresh `Orchestrator`-mode session per call using the resolved
/// [`LlmProvider`], so concurrent `AdvanceGoal` dispatches each get their own
/// independent LLM session instead of serializing on a single shared session.
///
/// Each `open_session` opens a brand-new per-turn adapter session (closed by
/// the caller after the turn), so calls are safe to run on separate threads.
pub struct ProviderSessionFactory {
    provider: LlmProvider,
    adapter_tag: String,
}

impl ProviderSessionFactory {
    /// Create a factory that opens `Orchestrator`-mode sessions for the given
    /// provider, tagged with `adapter_tag` (e.g. `"ooda"`).
    pub fn new(provider: LlmProvider, adapter_tag: impl Into<String>) -> Self {
        Self {
            provider,
            adapter_tag: adapter_tag.into(),
        }
    }
}

impl crate::ooda_loop::OrchestratorSessionFactory for ProviderSessionFactory {
    fn open_session(&self) -> SimardResult<Box<dyn BaseTypeSession>> {
        SessionBuilder::new(OperatingMode::Orchestrator, self.provider)
            .node_id("ooda-daemon-engineer")
            .address("ooda-daemon-engineer://local")
            .adapter_tag(&self.adapter_tag)
            .open()
            .map_err(|e| crate::error::SimardError::RpcTransportError {
                endpoint: "ooda-session-factory".to_string(),
                reason: e,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_populates_all_fields() {
        let builder = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::RustyClawd)
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
        let builder = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::RustyClawd)
            .adapter_tag("meeting-rustyclawd");
        assert_eq!(builder.adapter_tag, "meeting");

        let builder = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::RustyClawd)
            .adapter_tag("review-pipeline-copilot");
        assert_eq!(builder.adapter_tag, "review-pipeline");

        let builder = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::RustyClawd)
            .adapter_tag("plain-tag");
        assert_eq!(builder.adapter_tag, "plain-tag");
    }

    #[test]
    fn provider_override_is_respected() {
        let builder = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::Copilot)
            .provider(LlmProvider::RustyClawd);
        assert_eq!(builder.provider, LlmProvider::RustyClawd);
    }

    #[test]
    fn open_does_not_panic() {
        let session = SessionBuilder::new(OperatingMode::Meeting, LlmProvider::RustyClawd)
            .node_id("test-node")
            .address("test://local")
            .adapter_tag("nonexistent-adapter")
            .open();

        // The adapter may or may not open depending on auth — no panic is the invariant.
        drop(session);
    }

    #[test]
    fn topology_override() {
        let builder = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
            .topology(RuntimeTopology::SingleProcess)
            .node_id("eng")
            .address("eng://local")
            .adapter_tag("eng-adapter");

        let request = builder.build_request();
        assert_eq!(request.topology, RuntimeTopology::SingleProcess);
    }

    // ------------------------------------------------------------------
    // agent_binary_value (issue #2132)
    // ------------------------------------------------------------------

    #[test]
    fn copilot_agent_binary_value_returns_copilot() {
        assert_eq!(LlmProvider::Copilot.agent_binary_value(), "copilot");
    }

    #[test]
    fn rustyclawd_agent_binary_value_returns_rustyclawd() {
        assert_eq!(LlmProvider::RustyClawd.agent_binary_value(), "rustyclawd");
    }

    // ------------------------------------------------------------------
    // Production enrichment wiring through SessionBuilder (issue #2383)
    //
    // These assert against the exact production seam that regressed: the
    // `with_enrichment(...)` call inside `SessionBuilder`'s provider arms.
    // Deleting that call (the #2383 defect) drops both bridges to None, so
    // `is_configured()` flips to false and these tests fail — unlike the
    // adapter-level tests, which exercise the builder method directly and
    // would stay green. `build_enriched_session` stops before the
    // auth-requiring `session.open()`, so no live backend is needed.
    // ------------------------------------------------------------------

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn rustyclawd_provider_wires_enrichment_through_session_builder() {
        // Hermetic: pin SIMARD_STATE_ROOT to a TempDir so `default_state_root()`
        // (hardcoded inside `build_enriched_session`) resolves under temp_dir
        // rather than $HOME/.simard — keeping cognitive-memory writes off the
        // operator's live store and clear of the hermetic-test guard.
        let _hermetic = crate::test_support::HermeticState::new();

        let (session, adapter) =
            SessionBuilder::new(OperatingMode::Engineer, LlmProvider::RustyClawd)
                .node_id("test-node")
                .address("test://local")
                .adapter_tag("test-adapter")
                .build_enriched_session()
                .expect("build_enriched_session must succeed for RustyClawd");

        assert_eq!(adapter, "RustyClawdAdapter");
        let bridges = session
            .enrichment()
            .expect("RustyClawd session must expose enrichment bridges");
        assert!(
            bridges.is_configured(),
            "SessionBuilder must wire memory + knowledge enrichment for the \
             RustyClawd provider (issue #2383); empty bridges means the \
             with_enrichment(...) call was dropped"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn copilot_provider_wires_enrichment_through_session_builder() {
        // Hermetic: see the RustyClawd variant above.
        let _hermetic = crate::test_support::HermeticState::new();

        let (session, adapter) = SessionBuilder::new(OperatingMode::Engineer, LlmProvider::Copilot)
            .node_id("test-node")
            .address("test://local")
            .adapter_tag("test-adapter")
            .build_enriched_session()
            .expect("build_enriched_session must succeed for Copilot");

        assert_eq!(adapter, "CopilotSdkAdapter");
        let bridges = session
            .enrichment()
            .expect("Copilot session must expose enrichment bridges");
        assert!(
            bridges.is_configured(),
            "SessionBuilder must wire memory + knowledge enrichment for the \
             Copilot provider (issue #1664)"
        );
    }
}
