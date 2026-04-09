//! [`PendingSdkAdapter`] – catalog entry for SDKs whose Rust bindings are not
//! yet available.

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeSession, BaseTypeSessionRequest,
    standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

use super::session::PendingSdkSession;

/// A base type adapter for SDKs whose Rust bindings are not yet available.
///
/// The adapter registers in the catalog with correct metadata and capabilities
/// so identity manifests can reference it. When `run_turn` is called, it
/// returns an explicit error describing which SDK is missing.
#[derive(Debug)]
pub struct PendingSdkAdapter {
    pub(crate) descriptor: BaseTypeDescriptor,
    pub(crate) not_implemented_reason: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::OperatingMode;
    use uuid::Uuid;

    fn test_request(topology: RuntimeTopology) -> BaseTypeSessionRequest {
        BaseTypeSessionRequest {
            session_id: crate::session::SessionId::from_uuid(Uuid::nil()),
            mode: OperatingMode::Engineer,
            topology,
            prompt_assets: vec![],
            runtime_node: crate::runtime::RuntimeNodeId::new("node"),
            mailbox_address: crate::runtime::RuntimeAddress::new("addr"),
        }
    }

    #[test]
    fn registered_creates_adapter() {
        let adapter = PendingSdkAdapter::registered(
            "test-sdk",
            "test-backend",
            "registered-base-type:test",
            "SDK not yet implemented",
        )
        .unwrap();
        assert_eq!(adapter.descriptor.id.as_str(), "test-sdk");
        assert_eq!(adapter.not_implemented_reason, "SDK not yet implemented");
    }

    #[test]
    fn descriptor_returns_correct_id() {
        let adapter =
            PendingSdkAdapter::registered("my-sdk", "backend-id", "registered:my-sdk", "not ready")
                .unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-sdk");
    }

    #[test]
    fn supports_single_process_topology() {
        let adapter =
            PendingSdkAdapter::registered("sdk-1", "backend", "registered:sdk-1", "pending")
                .unwrap();
        assert!(
            adapter
                .descriptor
                .supported_topologies
                .contains(&RuntimeTopology::SingleProcess)
        );
    }

    #[test]
    fn open_session_returns_pending_session() {
        let adapter =
            PendingSdkAdapter::registered("sdk-2", "backend", "registered:sdk-2", "not yet")
                .unwrap();
        let request = test_request(RuntimeTopology::SingleProcess);
        let session = adapter.open_session(request);
        assert!(session.is_ok());
    }

    #[test]
    fn open_session_unsupported_topology_returns_error() {
        let adapter =
            PendingSdkAdapter::registered("sdk-3", "backend", "registered:sdk-3", "not yet")
                .unwrap();
        let request = test_request(RuntimeTopology::Distributed);
        let result = adapter.open_session(request);
        assert!(result.is_err());
    }
}
