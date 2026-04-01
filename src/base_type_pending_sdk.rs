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
