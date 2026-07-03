//! Native in-process adapter transport.
//!
//! [`NativeServerTransport`] implements [`ServerTransport`] by dispatching
//! method calls to registered Rust handler functions, eliminating the need
//! to spawn a Python subprocess.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};
use crate::server_transport::{
    SERVER_ERROR_METHOD_NOT_FOUND, ServerErrorPayload, ServerRequest, ServerResponse,
    ServerTransport,
};

/// A method handler receives JSON params and returns a JSON result or error.
pub type MethodHandler = Arc<dyn Fn(&Value) -> Result<Value, ServerErrorPayload> + Send + Sync>;

/// An adapter transport that dispatches calls to registered Rust functions.
///
/// This replaces the subprocess-based transport by running adapter logic
/// directly in the Simard process. Each adapter method is registered as a
/// closure that receives JSON params and returns a JSON result.
pub struct NativeServerTransport {
    adapter_name: String,
    handlers: HashMap<String, MethodHandler>,
}

impl NativeServerTransport {
    pub fn new(adapter_name: impl Into<String>) -> Self {
        let name = adapter_name.into();
        let mut transport = Self {
            adapter_name: name.clone(),
            handlers: HashMap::new(),
        };
        // Always register the health check handler.
        let health_name = name;
        transport.register(
            "bridge.health",
            Arc::new(move |_params| {
                Ok(serde_json::json!({
                    "server_name": health_name,
                    "healthy": true,
                }))
            }),
        );
        transport
    }

    /// Register a handler for a method name.
    pub fn register(&mut self, method: impl Into<String>, handler: MethodHandler) {
        self.handlers.insert(method.into(), handler);
    }
}

impl ServerTransport for NativeServerTransport {
    fn call(&self, request: ServerRequest) -> SimardResult<ServerResponse> {
        let handler = match self.handlers.get(&request.method) {
            Some(h) => h,
            None => {
                return Ok(ServerResponse {
                    id: request.id,
                    result: None,
                    error: Some(ServerErrorPayload {
                        code: SERVER_ERROR_METHOD_NOT_FOUND,
                        message: format!(
                            "method '{}' is not registered on native adapter '{}'",
                            request.method, self.adapter_name
                        ),
                    }),
                });
            }
        };

        match handler(&request.params) {
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
            format!("adapter:native:{}", self.adapter_name),
            format!("server_transport::native::{}", self.adapter_name),
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
    use crate::server_transport::{ServerHealth, new_request_id, unpack_server_response};

    #[test]
    fn native_transport_health_check() {
        let transport = NativeServerTransport::new("test-native");
        let health = transport.health().unwrap();
        assert_eq!(health.server_name, "test-native");
        assert!(health.healthy);
    }

    #[test]
    fn native_transport_dispatches_registered_handler() {
        let mut transport = NativeServerTransport::new("test");
        transport.register("echo", Arc::new(|params| Ok(params.clone())));
        let request = ServerRequest {
            id: new_request_id(),
            method: "echo".to_string(),
            params: serde_json::json!({"hello": "world"}),
        };
        let response = transport.call(request).unwrap();
        let result: serde_json::Value = unpack_server_response("test", "echo", response).unwrap();
        assert_eq!(result["hello"], "world");
    }

    #[test]
    fn native_transport_returns_method_not_found() {
        let transport = NativeServerTransport::new("test");
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
    fn native_transport_returns_handler_error() {
        let mut transport = NativeServerTransport::new("test");
        transport.register(
            "fail",
            Arc::new(|_| {
                Err(ServerErrorPayload {
                    code: -32603,
                    message: "something broke".to_string(),
                })
            }),
        );
        let request = ServerRequest {
            id: new_request_id(),
            method: "fail".to_string(),
            params: serde_json::json!({}),
        };
        let response = transport.call(request).unwrap();
        assert_eq!(response.error.unwrap().message, "something broke");
    }

    #[test]
    fn native_transport_descriptor_contains_adapter_name() {
        let transport = NativeServerTransport::new("my-adapter");
        let desc = transport.descriptor();
        assert!(desc.identity.contains("my-adapter"));
        assert!(desc.identity.contains("native"));
    }

    #[test]
    fn native_transport_health_via_trait() {
        let transport = NativeServerTransport::new("test");
        let health: ServerHealth = transport.health().unwrap();
        assert!(health.healthy);
    }
}
