//! Shared adapter for agent SDK base types whose runtime bindings are not yet
//! available. Each pending SDK registers properly in the base type catalog and
//! returns an explicit error when a turn is attempted, so the system fails
//! closed rather than silently ignoring the delegation.

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome, BaseTypeSession,
    BaseTypeSessionRequest, BaseTypeTurnInput, ensure_session_not_already_open,
    ensure_session_not_closed, ensure_session_open, joined_prompt_ids,
    standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;
use crate::sanitization::objective_metadata;

/// A base type adapter for SDKs whose Rust bindings are not yet available.
///
/// The adapter registers in the catalog with correct metadata and capabilities
/// so identity manifests can reference it. When `run_turn` is called, it
/// returns an explicit error describing which SDK is missing.
#[derive(Debug)]
pub struct PendingSdkAdapter {
    descriptor: BaseTypeDescriptor,
    not_implemented_reason: String,
}

impl PendingSdkAdapter {
    /// Create a pending SDK adapter.
    ///
    /// - `id`: base type ID (e.g. "claude-agent-sdk")
    /// - `backend_identity`: backend descriptor identity string
    /// - `backend_registration`: backend descriptor registration string
    /// - `not_implemented_reason`: human-readable reason shown when `run_turn` is called
    pub fn registered(
        id: impl Into<String>,
        backend_identity: impl Into<String>,
        backend_registration: impl Into<String>,
        not_implemented_reason: impl Into<String>,
    ) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        let backend_identity = backend_identity.into();
        let backend_registration = backend_registration.into();
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    backend_identity,
                    &backend_registration,
                    Freshness::now()?,
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [RuntimeTopology::SingleProcess].into_iter().collect(),
            },
            not_implemented_reason: not_implemented_reason.into(),
        })
    }
}

impl BaseTypeFactory for PendingSdkAdapter {
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

        Ok(Box::new(PendingSdkSession {
            descriptor: self.descriptor.clone(),
            request,
            not_implemented_reason: self.not_implemented_reason.clone(),
            is_open: false,
            is_closed: false,
        }))
    }
}

struct PendingSdkSession {
    descriptor: BaseTypeDescriptor,
    request: BaseTypeSessionRequest,
    not_implemented_reason: String,
    is_open: bool,
    is_closed: bool,
}

impl std::fmt::Debug for PendingSdkSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingSdkSession")
            .field("descriptor", &self.descriptor)
            .field("is_open", &self.is_open)
            .field("is_closed", &self.is_closed)
            .finish()
    }
}

impl BaseTypeSession for PendingSdkSession {
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

        let prompt_ids = joined_prompt_ids(&self.request.prompt_assets);
        let objective_summary = objective_metadata(&input.objective);

        Err(SimardError::AdapterInvocationFailed {
            base_type: self.descriptor.id.to_string(),
            reason: format!(
                "{}. Objective '{}' on topology '{}' with prompt assets [{}] \
                 cannot be executed until the SDK integration is complete.",
                self.not_implemented_reason, objective_summary, self.request.topology, prompt_ids,
            ),
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
    use crate::runtime::{RuntimeAddress, RuntimeNodeId};
    use crate::session::SessionId;

    fn make_adapter() -> PendingSdkAdapter {
        PendingSdkAdapter::registered(
            "test-pending-sdk",
            "test-backend",
            "test-registration",
            "SDK not yet available",
        )
        .unwrap()
    }

    fn make_session_request(topology: RuntimeTopology) -> BaseTypeSessionRequest {
        BaseTypeSessionRequest {
            session_id: SessionId::parse("session-00000000-0000-0000-0000-000000000001")
                .expect("valid session id"),
            mode: OperatingMode::Engineer,
            topology,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::local(&RuntimeNodeId::local()),
        }
    }

    // --- PendingSdkAdapter::registered ---

    #[test]
    fn registered_creates_adapter_with_correct_id() {
        let adapter = make_adapter();
        assert_eq!(adapter.descriptor.id.as_str(), "test-pending-sdk");
    }

    #[test]
    fn registered_stores_not_implemented_reason() {
        let adapter = make_adapter();
        assert_eq!(adapter.not_implemented_reason, "SDK not yet available");
    }

    #[test]
    fn registered_with_custom_reason() {
        let adapter = PendingSdkAdapter::registered(
            "custom-sdk",
            "custom-backend",
            "custom-reg",
            "Custom reason: bindings pending",
        )
        .unwrap();
        assert_eq!(
            adapter.not_implemented_reason,
            "Custom reason: bindings pending"
        );
    }

    #[test]
    fn registered_with_empty_id_succeeds() {
        let result = PendingSdkAdapter::registered("", "b", "r", "reason");
        assert!(result.is_ok());
    }

    // --- descriptor ---

    #[test]
    fn descriptor_has_single_process_topology_only() {
        let adapter = make_adapter();
        assert!(
            adapter
                .descriptor
                .supported_topologies
                .contains(&RuntimeTopology::SingleProcess)
        );
        assert!(
            !adapter
                .descriptor
                .supported_topologies
                .contains(&RuntimeTopology::MultiProcess)
        );
        assert!(
            !adapter
                .descriptor
                .supported_topologies
                .contains(&RuntimeTopology::Distributed)
        );
    }

    #[test]
    fn descriptor_has_standard_capabilities() {
        let adapter = make_adapter();
        assert_eq!(
            adapter.descriptor.capabilities,
            standard_session_capabilities()
        );
    }

    #[test]
    fn factory_descriptor_matches_struct_descriptor() {
        let adapter = make_adapter();
        let factory_desc = BaseTypeFactory::descriptor(&adapter);
        assert_eq!(factory_desc.id, adapter.descriptor.id);
    }

    // --- open_session topology gating ---

    #[test]
    fn open_session_single_process_succeeds() {
        let adapter = make_adapter();
        let request = make_session_request(RuntimeTopology::SingleProcess);
        assert!(adapter.open_session(request).is_ok());
    }

    #[test]
    fn open_session_multi_process_fails_with_unsupported_topology() {
        let adapter = make_adapter();
        let request = make_session_request(RuntimeTopology::MultiProcess);
        let result = adapter.open_session(request);
        match result {
            Err(SimardError::UnsupportedTopology {
                base_type,
                topology,
            }) => {
                assert_eq!(base_type, "test-pending-sdk");
                assert_eq!(topology, RuntimeTopology::MultiProcess);
            }
            Err(other) => panic!("expected UnsupportedTopology, got: {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn open_session_distributed_fails() {
        let adapter = make_adapter();
        let request = make_session_request(RuntimeTopology::Distributed);
        assert!(adapter.open_session(request).is_err());
    }

    // --- session lifecycle ---

    #[test]
    fn session_open_succeeds() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        assert!(session.open().is_ok());
    }

    #[test]
    fn session_double_open_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        assert!(session.open().is_err());
    }

    #[test]
    fn session_open_close_lifecycle() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        assert!(session.close().is_ok());
    }

    #[test]
    fn session_close_before_open_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        assert!(session.close().is_err());
    }

    #[test]
    fn session_double_close_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        session.close().unwrap();
        assert!(session.close().is_err());
    }

    #[test]
    fn session_open_after_close_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        session.close().unwrap();
        assert!(session.open().is_err());
    }

    // --- run_turn ---

    #[test]
    fn run_turn_before_open_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        let input = BaseTypeTurnInput::objective_only("test");
        assert!(session.run_turn(input).is_err());
    }

    #[test]
    fn run_turn_returns_adapter_invocation_failed() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        let input = BaseTypeTurnInput::objective_only("test objective");
        let result = session.run_turn(input);
        assert!(result.is_err());
        match result.unwrap_err() {
            SimardError::AdapterInvocationFailed { base_type, reason } => {
                assert_eq!(base_type, "test-pending-sdk");
                assert!(reason.contains("SDK not yet available"));
            }
            other => panic!("expected AdapterInvocationFailed, got: {other:?}"),
        }
    }

    #[test]
    fn run_turn_error_mentions_topology() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        let input = BaseTypeTurnInput::objective_only("test");
        let err = session.run_turn(input).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("single") || msg.to_lowercase().contains("topology"),
            "error should mention topology: {msg}"
        );
    }

    #[test]
    fn run_turn_after_close_fails() {
        let adapter = make_adapter();
        let mut session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        session.open().unwrap();
        session.close().unwrap();
        let input = BaseTypeTurnInput::objective_only("test");
        assert!(session.run_turn(input).is_err());
    }

    // --- session descriptor ---

    #[test]
    fn session_descriptor_matches_adapter() {
        let adapter = make_adapter();
        let session = adapter
            .open_session(make_session_request(RuntimeTopology::SingleProcess))
            .unwrap();
        assert_eq!(session.descriptor().id, adapter.descriptor.id);
    }

    // --- Debug ---

    #[test]
    fn adapter_debug_contains_type_name() {
        let adapter = make_adapter();
        let debug = format!("{:?}", adapter);
        assert!(debug.contains("PendingSdkAdapter"));
    }

    #[test]
    fn adapter_debug_contains_id() {
        let adapter = make_adapter();
        let debug = format!("{:?}", adapter);
        assert!(debug.contains("test-pending-sdk"));
    }
}
