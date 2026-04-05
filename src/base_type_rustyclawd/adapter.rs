use crate::base_types::{
    BaseTypeDescriptor, BaseTypeFactory, BaseTypeId, BaseTypeSession, BaseTypeSessionRequest,
    standard_session_capabilities,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};
use crate::runtime::RuntimeTopology;

use super::session::RustyClawdSession;

#[derive(Debug)]
pub struct RustyClawdAdapter {
    pub(super) descriptor: BaseTypeDescriptor,
}

impl RustyClawdAdapter {
    pub fn registered(id: impl Into<String>) -> SimardResult<Self> {
        let id = BaseTypeId::new(id);
        Ok(Self {
            descriptor: BaseTypeDescriptor {
                id,
                backend: BackendDescriptor::for_runtime_type::<Self>(
                    "rusty-clawd::session-backend",
                    "registered-base-type:rusty-clawd",
                    Freshness::now()?,
                ),
                capabilities: standard_session_capabilities(),
                supported_topologies: [
                    RuntimeTopology::SingleProcess,
                    RuntimeTopology::MultiProcess,
                ]
                .into_iter()
                .collect(),
            },
        })
    }
}

impl BaseTypeFactory for RustyClawdAdapter {
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

        Ok(Box::new(RustyClawdSession {
            descriptor: self.descriptor.clone(),
            request,
            is_open: false,
            is_closed: false,
            client: None,
            rt: None,
            conversation_history: Vec::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::BaseTypeFactory;
    use crate::runtime::RuntimeTopology;

    // ── RustyClawdAdapter construction ──

    #[test]
    fn registered_adapter_has_correct_backend_identity() {
        let adapter = RustyClawdAdapter::registered("rusty-clawd").unwrap();
        assert_eq!(
            adapter.descriptor().backend.identity,
            "rusty-clawd::session-backend"
        );
    }

    #[test]
    fn registered_adapter_has_expected_id() {
        let adapter = RustyClawdAdapter::registered("my-id").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-id");
    }

    #[test]
    fn registered_adapter_supports_single_and_multi_process() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let desc = adapter.descriptor();
        assert!(desc.supports_topology(RuntimeTopology::SingleProcess));
        assert!(desc.supports_topology(RuntimeTopology::MultiProcess));
        assert!(!desc.supports_topology(RuntimeTopology::Distributed));
    }

    #[test]
    fn registered_adapter_has_standard_capabilities() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let caps = &adapter.descriptor().capabilities;
        assert!(
            !caps.is_empty(),
            "should have standard session capabilities"
        );
    }

    #[test]
    fn descriptor_returns_reference_to_stored_descriptor() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        let d1 = adapter.descriptor();
        let d2 = adapter.descriptor();
        assert_eq!(d1.id, d2.id);
    }

    #[test]
    fn adapter_debug_format_contains_type_name() {
        let adapter = RustyClawdAdapter::registered("debug-test").unwrap();
        let debug = format!("{adapter:?}");
        assert!(debug.contains("RustyClawdAdapter"));
    }

    #[test]
    fn registered_adapter_with_empty_id() {
        let adapter = RustyClawdAdapter::registered("");
        assert!(adapter.is_ok(), "empty id should still construct");
        assert_eq!(adapter.unwrap().descriptor().id.as_str(), "");
    }

    #[test]
    fn registered_adapter_with_hyphenated_id() {
        let adapter = RustyClawdAdapter::registered("my-custom-agent-type").unwrap();
        assert_eq!(adapter.descriptor().id.as_str(), "my-custom-agent-type");
    }

    #[test]
    fn registered_adapter_backend_identity_is_stable() {
        let a1 = RustyClawdAdapter::registered("test1").unwrap();
        let a2 = RustyClawdAdapter::registered("test2").unwrap();
        assert_eq!(
            a1.descriptor().backend.identity,
            a2.descriptor().backend.identity,
            "backend identity should be the same regardless of adapter id"
        );
    }

    #[test]
    fn registered_adapter_does_not_support_distributed() {
        let adapter = RustyClawdAdapter::registered("rc").unwrap();
        assert!(
            !adapter
                .descriptor()
                .supports_topology(RuntimeTopology::Distributed),
        );
    }

    // ── open_session ──

    #[test]
    fn open_session_rejects_unsupported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000001")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::Distributed, // not supported
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_err());
        match result {
            Err(SimardError::UnsupportedTopology {
                base_type,
                topology,
            }) => {
                assert_eq!(base_type, "rc-test");
                assert_eq!(topology, RuntimeTopology::Distributed);
            }
            Err(other) => panic!("expected UnsupportedTopology, got {other:?}"),
            Ok(_) => panic!("expected error"),
        }
    }

    #[test]
    fn open_session_succeeds_for_supported_topology() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000002")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_ok());
    }

    #[test]
    fn open_session_succeeds_for_multi_process() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000010")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::MultiProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let result = adapter.open_session(request);
        assert!(result.is_ok());
    }

    // ── Session lifecycle guards ──

    #[test]
    fn session_run_turn_before_open_fails() {
        use crate::base_types::{BaseTypeSessionRequest, BaseTypeTurnInput};
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000003")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();

        let input = BaseTypeTurnInput {
            objective: "test".to_string(),
            identity_context: "".to_string(),
            prompt_preamble: "".to_string(),
        };
        let result = session.run_turn(input);
        assert!(result.is_err(), "run_turn before open should fail");
    }

    #[test]
    fn session_close_before_open_fails() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-test").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000004")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let mut session = adapter.open_session(request).unwrap();
        let result = session.close();
        assert!(result.is_err(), "close before open should fail");
    }

    #[test]
    fn session_descriptor_matches_adapter_descriptor() {
        use crate::base_types::BaseTypeSessionRequest;
        use crate::identity::OperatingMode;
        use crate::runtime::{RuntimeAddress, RuntimeNodeId};
        use crate::session::SessionId;

        let adapter = RustyClawdAdapter::registered("rc-desc").unwrap();
        let request = BaseTypeSessionRequest {
            session_id: SessionId::try_from("session-00000000-0000-0000-0000-000000000011")
                .unwrap(),
            mode: OperatingMode::Engineer,
            topology: RuntimeTopology::SingleProcess,
            prompt_assets: vec![],
            runtime_node: RuntimeNodeId::local(),
            mailbox_address: RuntimeAddress::new("test-addr"),
        };
        let session = adapter.open_session(request).unwrap();
        assert_eq!(
            session.descriptor().id.as_str(),
            adapter.descriptor().id.as_str()
        );
    }
}
