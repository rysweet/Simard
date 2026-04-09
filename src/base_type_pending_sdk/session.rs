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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_type_pending_sdk::PendingSdkAdapter;
    use crate::base_types::BaseTypeFactory;
    use crate::identity::OperatingMode;
    use uuid::Uuid;

    fn make_session() -> PendingSdkSession {
        let adapter = PendingSdkAdapter::registered(
            "test-sdk",
            "backend",
            "registered:test-sdk",
            "SDK not ready",
        )
        .unwrap();
        let request = BaseTypeSessionRequest {
            session_id: crate::session::SessionId::from_uuid(Uuid::nil()),
            mode: OperatingMode::Engineer,
            topology: crate::runtime::RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: crate::runtime::RuntimeNodeId::new("n"),
            mailbox_address: crate::runtime::RuntimeAddress::new("a"),
        };
        // We know open_session returns PendingSdkSession boxed — downcast
        let boxed = adapter.open_session(request).unwrap();
        // Instead of downcasting, create directly
        drop(boxed);
        PendingSdkSession {
            descriptor: adapter.descriptor.clone(),
            request: BaseTypeSessionRequest {
                session_id: crate::session::SessionId::from_uuid(Uuid::nil()),
                mode: OperatingMode::Engineer,
                topology: crate::runtime::RuntimeTopology::SingleProcess,
                prompt_assets: vec![],
                runtime_node: crate::runtime::RuntimeNodeId::new("n"),
                mailbox_address: crate::runtime::RuntimeAddress::new("a"),
            },
            not_implemented_reason: adapter.not_implemented_reason.clone(),
            is_open: false,
            is_closed: false,
        }
    }

    #[test]
    fn session_open_succeeds() {
        let mut session = make_session();
        assert!(session.open().is_ok());
        assert!(session.is_open);
    }

    #[test]
    fn session_double_open_fails() {
        let mut session = make_session();
        session.open().unwrap();
        assert!(session.open().is_err());
    }

    #[test]
    fn session_run_turn_before_open_fails() {
        let mut session = make_session();
        let input = BaseTypeTurnInput::objective_only("test");
        assert!(session.run_turn(input).is_err());
    }

    #[test]
    fn session_run_turn_returns_adapter_error() {
        let mut session = make_session();
        session.open().unwrap();
        let input = BaseTypeTurnInput::objective_only("do something");
        let result = session.run_turn(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("SDK not ready"));
    }

    #[test]
    fn session_close_after_open_succeeds() {
        let mut session = make_session();
        session.open().unwrap();
        assert!(session.close().is_ok());
        assert!(session.is_closed);
    }

    #[test]
    fn session_close_before_open_fails() {
        let mut session = make_session();
        assert!(session.close().is_err());
    }

    #[test]
    fn session_double_close_fails() {
        let mut session = make_session();
        session.open().unwrap();
        session.close().unwrap();
        assert!(session.close().is_err());
    }

    #[test]
    fn session_debug_format() {
        let session = make_session();
        let debug = format!("{session:?}");
        assert!(debug.contains("PendingSdkSession"));
    }

    #[test]
    fn session_descriptor_matches() {
        let session = make_session();
        assert_eq!(session.descriptor().id.as_str(), "test-sdk");
    }
}
