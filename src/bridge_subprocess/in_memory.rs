use crate::bridge::{BridgeErrorPayload, BridgeRequest, BridgeResponse, BridgeTransport};
use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};

/// Handler function type for in-memory bridge transports.
type BridgeHandler =
    dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, BridgeErrorPayload> + Send + Sync;

/// A bridge transport backed by an in-memory handler function, for testing.
///
/// The handler receives a method name and params, and returns a result value
/// or an error payload.
pub struct InMemoryBridgeTransport {
    bridge_name: String,
    handler: Box<BridgeHandler>,
}

impl InMemoryBridgeTransport {
    pub fn new(
        bridge_name: impl Into<String>,
        handler: impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, BridgeErrorPayload>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            bridge_name: bridge_name.into(),
            handler: Box::new(handler),
        }
    }

    /// Create a transport that echoes the params back as the result.
    pub fn echo(bridge_name: impl Into<String>) -> Self {
        Self::new(bridge_name, |_method, params| Ok(params.clone()))
    }
}

impl BridgeTransport for InMemoryBridgeTransport {
    fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse> {
        match (self.handler)(&request.method, &request.params) {
            Ok(result) => Ok(BridgeResponse {
                id: request.id,
                result: Some(result),
                error: None,
            }),
            Err(error) => Ok(BridgeResponse {
                id: request.id,
                result: None,
                error: Some(error),
            }),
        }
    }

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor::for_runtime_type::<Self>(
            format!("bridge:in-memory:{}", self.bridge_name),
            format!("bridge::in-memory::{}", self.bridge_name),
            Freshness::now().unwrap_or(Freshness {
                state: crate::metadata::FreshnessState::Stale,
                observed_at_unix_ms: 0,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        BRIDGE_ERROR_METHOD_NOT_FOUND, BridgeHealth, new_request_id, unpack_bridge_response,
    };

    #[test]
    fn in_memory_echo_transport_roundtrips_params() {
        let transport = InMemoryBridgeTransport::echo("test-echo");
        let request = BridgeRequest {
            id: new_request_id(),
            method: "bridge.health".to_string(),
            params: serde_json::json!({"server_name": "echo", "healthy": true}),
        };
        let response = transport.call(request).unwrap();
        let health: BridgeHealth =
            unpack_bridge_response("test", "bridge.health", response).unwrap();
        assert_eq!(health.server_name, "echo");
        assert!(health.healthy);
    }

    #[test]
    fn in_memory_transport_returns_handler_errors() {
        let transport = InMemoryBridgeTransport::new("test-error", |method, _params| {
            Err(BridgeErrorPayload {
                code: BRIDGE_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            })
        });
        let request = BridgeRequest {
            id: new_request_id(),
            method: "nonexistent".to_string(),
            params: serde_json::json!({}),
        };
        let response = transport.call(request).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, BRIDGE_ERROR_METHOD_NOT_FOUND);
    }

    #[test]
    fn in_memory_transport_has_descriptive_descriptor() {
        let transport = InMemoryBridgeTransport::echo("my-bridge");
        let desc = transport.descriptor();
        assert!(desc.identity.contains("my-bridge"));
    }
}
