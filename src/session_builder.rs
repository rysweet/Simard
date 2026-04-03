//! Unified session creation across all operating modes.
//!
//! Extracts the `BaseTypeSessionRequest` + `RustyClawdAdapter` factory pattern
//! into a shared [`SessionBuilder`] so meeting, engineer, and future modes
//! construct sessions the same way.

use crate::base_type_rustyclawd::RustyClawdAdapter;
use crate::base_types::{BaseTypeFactory, BaseTypeSession, BaseTypeSessionRequest};
use crate::identity::OperatingMode;
use crate::prompt_assets::PromptAssetRef;
use crate::runtime::{RuntimeAddress, RuntimeNodeId, RuntimeTopology};
use crate::session::SessionId;

/// Builds and opens a `BaseTypeSession` for any operating mode.
///
/// Encapsulates the common pattern of:
/// 1. Constructing a `BaseTypeSessionRequest` from mode-specific parameters.
/// 2. Trying the RustyClawd adapter (requires `ANTHROPIC_API_KEY`).
/// 3. Returning the opened session or `None`.
///
/// # Example
///
/// ```ignore
/// let session = SessionBuilder::new(OperatingMode::Meeting)
///     .node_id("meeting-repl")
///     .address("meeting-repl://local")
///     .adapter_tag("meeting-rustyclawd")
///     .open();
/// ```
pub struct SessionBuilder {
    mode: OperatingMode,
    topology: RuntimeTopology,
    prompt_assets: Vec<PromptAssetRef>,
    node_id: String,
    address: String,
    adapter_tag: String,
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

    /// Set the adapter registration tag (e.g. `"meeting-rustyclawd"`).
    pub fn adapter_tag(mut self, tag: &str) -> Self {
        self.adapter_tag = tag.to_owned();
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

    /// Try to open a session via RustyClawd.
    ///
    /// Returns `Some(session)` if the `ANTHROPIC_API_KEY` is set and the
    /// adapter successfully opens; `None` otherwise.
    pub fn open(self) -> Option<Box<dyn BaseTypeSession>> {
        if std::env::var("ANTHROPIC_API_KEY").is_err() {
            return None;
        }

        let request = self.build_request();
        let factory = RustyClawdAdapter::registered(&self.adapter_tag).ok()?;
        let mut session = factory.open_session(request).ok()?;
        session.open().ok()?;
        Some(session)
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
    fn open_returns_none_without_api_key() {
        // Ensure the key is unset for this test.
        // SAFETY: test-only; single-threaded test runner for this module.
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

        let session = SessionBuilder::new(OperatingMode::Meeting)
            .node_id("test-node")
            .address("test://local")
            .adapter_tag("nonexistent-adapter")
            .open();

        assert!(session.is_none());
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
