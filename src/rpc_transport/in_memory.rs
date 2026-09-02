use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::rpc::{RpcErrorPayload, RpcRequest, RpcResponse, RpcTransport};

/// Handler function type for in-memory rpc transports.
type RpcHandler =
    dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, RpcErrorPayload> + Send + Sync;

/// A rpc transport backed by an in-memory handler function, for testing.
///
/// The handler receives a method name and params, and returns a result value
/// or an error payload.
pub struct InMemoryRpcTransport {
    endpoint_name: String,
    handler: Box<RpcHandler>,
}

impl InMemoryRpcTransport {
    pub fn new(
        endpoint_name: impl Into<String>,
        handler: impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, RpcErrorPayload>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            endpoint_name: endpoint_name.into(),
            handler: Box::new(handler),
        }
    }

    /// Create a transport that echoes the params back as the result.
    pub fn echo(endpoint_name: impl Into<String>) -> Self {
        Self::new(endpoint_name, |_method, params| Ok(params.clone()))
    }
}

impl RpcTransport for InMemoryRpcTransport {
    fn call(&self, request: RpcRequest) -> SimardResult<RpcResponse> {
        match (self.handler)(&request.method, &request.params) {
            Ok(result) => Ok(RpcResponse {
                id: request.id,
                result: Some(result),
                error: None,
            }),
            Err(error) => Ok(RpcResponse {
                id: request.id,
                result: None,
                error: Some(error),
            }),
        }
    }

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor::for_runtime_type::<Self>(
            format!("rpc:in-memory:{}", self.endpoint_name),
            format!("rpc::in-memory::{}", self.endpoint_name),
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
    use crate::rpc::{RPC_ERROR_METHOD_NOT_FOUND, RpcHealth, new_request_id, unpack_rpc_response};

    #[test]
    fn in_memory_echo_transport_roundtrips_params() {
        let transport = InMemoryRpcTransport::echo("test-echo");
        let request = RpcRequest {
            id: new_request_id(),
            method: "bridge.health".to_string(),
            params: serde_json::json!({"server_name": "echo", "healthy": true}),
        };
        let response = transport.call(request).unwrap();
        let health: RpcHealth = unpack_rpc_response("test", "bridge.health", response).unwrap();
        assert_eq!(health.server_name, "echo");
        assert!(health.healthy);
    }

    #[test]
    fn in_memory_transport_returns_handler_errors() {
        let transport = InMemoryRpcTransport::new("test-error", |method, _params| {
            Err(RpcErrorPayload {
                code: RPC_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            })
        });
        let request = RpcRequest {
            id: new_request_id(),
            method: "nonexistent".to_string(),
            params: serde_json::json!({}),
        };
        let response = transport.call(request).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, RPC_ERROR_METHOD_NOT_FOUND);
    }

    #[test]
    fn in_memory_transport_has_descriptive_descriptor() {
        let transport = InMemoryRpcTransport::echo("my-rpc");
        let desc = transport.descriptor();
        assert!(desc.identity.contains("my-rpc"));
    }
}
