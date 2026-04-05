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
