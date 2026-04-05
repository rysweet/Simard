//! [`PendingSdkSession`] – session that always returns an explicit
//! "not-yet-implemented" error on `run_turn`.

use crate::base_types::{
    BaseTypeDescriptor, BaseTypeOutcome, BaseTypeSession, BaseTypeSessionRequest,
    BaseTypeTurnInput, ensure_session_not_already_open, ensure_session_not_closed,
    ensure_session_open, joined_prompt_ids,
};
use crate::error::{SimardError, SimardResult};
use crate::sanitization::objective_metadata;

pub(crate) struct PendingSdkSession {
    pub(crate) descriptor: BaseTypeDescriptor,
    pub(crate) request: BaseTypeSessionRequest,
    pub(crate) not_implemented_reason: String,
    pub(crate) is_open: bool,
    pub(crate) is_closed: bool,
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
