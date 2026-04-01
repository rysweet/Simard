//! Test support utilities — provides a lightweight adapter for integration tests
//! that need a BaseTypeFactory without requiring external processes or API keys.

use crate::base_types::{
    BaseTypeCapability, BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeOutcome,
    BaseTypeSession, BaseTypeSessionRequest, BaseTypeTurnInput, capability_set,
    ensure_session_not_already_open, ensure_session_not_closed, ensure_session_open,
    joined_prompt_ids, standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

/// A lightweight test adapter that returns canned results without spawning
/// any external processes or requiring API keys.
#[derive(Debug)]
pub struct TestAdapter {
    descriptor: BaseTypeDescriptor,
}

impl TestAdapter {
    pub fn new(
        id: impl Into<String>,
        implementation_identity: impl Into<String>,
        capabilities: impl IntoIterator<Item = BaseTypeCapability>,
        supported_topologies: impl IntoIterator<Item = RuntimeTopology>,
    ) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        let implementation_identity = implementation_identity.into();
        let backend = BackendDescriptor::for_runtime_type::<Self>(
            implementation_identity.clone(),
            format!("registered-base-type:{id}::implementation:{implementation_identity}"),
            Freshness::now()?,
        );
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend,
                capabilities: capability_set(capabilities),
                supported_topologies: supported_topologies.into_iter().collect(),
            },
        })
    }

    pub fn single_process(id: impl Into<String>) -> SimardResult<Self> {
        let id = id.into();
        Self::single_process_alias(id.clone(), id)
    }

    pub fn single_process_alias(
        id: impl Into<String>,
        implementation_identity: impl Into<String>,
    ) -> SimardResult<Self> {
        Self::new(
            id,
            implementation_identity,
            standard_session_capabilities(),
            [RuntimeTopology::SingleProcess],
        )
    }
}

impl BaseTypeFactory for TestAdapter {
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

        Ok(Box::new(TestSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
        }))
    }
}

#[derive(Debug)]
struct TestSession {
    descriptor: BaseTypeDescriptor,
    request: BaseTypeSessionRequest,
    is_open: bool,
    is_closed: bool,
}

impl BaseTypeSession for TestSession {
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

        let plan = format!(
            "Open '{}' session on '{}' via {} and run prompt assets [{}].",
            self.request.mode,
            self.request.topology,
            crate::sanitization::objective_metadata(&input.objective),
            prompt_ids,
        );

        let implementation_identity = &self.descriptor.backend.identity;
        let execution_summary = format!(
            "Local single-process harness session executed {} via selected base type '{}' on implementation '{}' from node '{}' at '{}'.",
            crate::sanitization::objective_metadata(&input.objective),
            self.descriptor.id,
            implementation_identity,
            self.request.runtime_node,
            self.request.mailbox_address,
        );

        let evidence = vec![
            format!("selected-base-type={}", self.descriptor.id),
            format!("prompt-assets=[{}]", prompt_ids),
            format!("runtime-node={}", self.request.runtime_node),
            format!("mailbox-address={}", self.request.mailbox_address),
        ];

        Ok(BaseTypeOutcome {
            plan,
            execution_summary,
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
