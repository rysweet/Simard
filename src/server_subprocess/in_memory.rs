use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::server_transport::{ServerErrorPayload, ServerRequest, ServerResponse, ServerTransport};

/// Handler function type for in-memory adapter transports.
type AdapterHandler =
    dyn Fn(&str, &serde_json::Value) -> Result<serde_json::Value, ServerErrorPayload> + Send + Sync;

/// An adapter transport backed by an in-memory handler function, for testing.
///
/// The handler receives a method name and params, and returns a result value
/// or an error payload.
pub struct InMemoryServerTransport {
    adapter_name: String,
    handler: Box<AdapterHandler>,
}

impl InMemoryServerTransport {
    pub fn new(
        adapter_name: impl Into<String>,
        handler: impl Fn(&str, &serde_json::Value) -> Result<serde_json::Value, ServerErrorPayload>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            adapter_name: adapter_name.into(),
            handler: Box::new(handler),
        }
    }

    /// Create a transport that echoes the params back as the result.
    pub fn echo(adapter_name: impl Into<String>) -> Self {
        Self::new(adapter_name, |_method, params| Ok(params.clone()))
    }
}

impl ServerTransport for InMemoryServerTransport {
    fn call(&self, request: ServerRequest) -> SimardResult<ServerResponse> {
        match (self.handler)(&request.method, &request.params) {
            Ok(result) => Ok(ServerResponse {
                id: request.id,
                result: Some(result),
                error: None,
            }),
            Err(error) => Ok(ServerResponse {
                id: request.id,
                result: None,
                error: Some(error),
            }),
        }
    }

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor::for_runtime_type::<Self>(
            format!("adapter:in-memory:{}", self.adapter_name),
            format!("server_transport::in-memory::{}", self.adapter_name),
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
    use crate::server_transport::{
        SERVER_ERROR_METHOD_NOT_FOUND, ServerHealth, new_request_id, unpack_server_response,
    };

    #[test]
    fn in_memory_echo_transport_roundtrips_params() {
        let transport = InMemoryServerTransport::echo("test-echo");
        let request = ServerRequest {
            id: new_request_id(),
            method: "bridge.health".to_string(),
            params: serde_json::json!({"server_name": "echo", "healthy": true}),
        };
        let response = transport.call(request).unwrap();
        let health: ServerHealth =
            unpack_server_response("test", "bridge.health", response).unwrap();
        assert_eq!(health.server_name, "echo");
        assert!(health.healthy);
    }

    #[test]
    fn in_memory_transport_returns_handler_errors() {
        let transport = InMemoryServerTransport::new("test-error", |method, _params| {
            Err(ServerErrorPayload {
                code: SERVER_ERROR_METHOD_NOT_FOUND,
                message: format!("unknown method: {method}"),
            })
        });
        let request = ServerRequest {
            id: new_request_id(),
            method: "nonexistent".to_string(),
            params: serde_json::json!({}),
        };
        let response = transport.call(request).unwrap();
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, SERVER_ERROR_METHOD_NOT_FOUND);
    }

    #[test]
    fn in_memory_transport_has_descriptive_descriptor() {
        let transport = InMemoryServerTransport::echo("my-adapter");
        let desc = transport.descriptor();
        assert!(desc.identity.contains("my-adapter"));
    }
}
