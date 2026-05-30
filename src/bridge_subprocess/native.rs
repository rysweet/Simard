//! Native in-process bridge transport.
//!
//! [`NativeBridgeTransport`] implements [`BridgeTransport`] by dispatching
//! method calls to registered Rust handler functions, eliminating the need
//! to spawn a Python subprocess.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::bridge::{
    BRIDGE_ERROR_METHOD_NOT_FOUND, BridgeErrorPayload, BridgeRequest, BridgeResponse,
    BridgeTransport,
};
use crate::error::SimardResult;
use crate::metadata::{BackendDescriptor, Freshness};

/// A method handler receives JSON params and returns a JSON result or error.
pub type MethodHandler = Arc<dyn Fn(&Value) -> Result<Value, BridgeErrorPayload> + Send + Sync>;

/// A bridge transport that dispatches calls to registered Rust functions.
///
/// This replaces the subprocess-based transport by running bridge logic
/// directly in the Simard process. Each bridge method is registered as a
/// closure that receives JSON params and returns a JSON result.
pub struct NativeBridgeTransport {
    bridge_name: String,
    handlers: HashMap<String, MethodHandler>,
}

impl NativeBridgeTransport {
    pub fn new(bridge_name: impl Into<String>) -> Self {
        let name = bridge_name.into();
        let mut transport = Self {
            bridge_name: name.clone(),
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

impl BridgeTransport for NativeBridgeTransport {
    fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse> {
        let handler = match self.handlers.get(&request.method) {
            Some(h) => h,
            None => {
                return Ok(BridgeResponse {
                    id: request.id,
                    result: None,
                    error: Some(BridgeErrorPayload {
                        code: BRIDGE_ERROR_METHOD_NOT_FOUND,
                        message: format!(
                            "method '{}' is not registered on native bridge '{}'",
                            request.method, self.bridge_name
                        ),
                    }),
                });
            }
        };

        match handler(&request.params) {
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
            format!("bridge:native:{}", self.bridge_name),
            format!("bridge::native::{}", self.bridge_name),
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
    use crate::bridge::{BridgeHealth, new_request_id, unpack_bridge_response};

    #[test]
    fn native_transport_health_check() {
        let transport = NativeBridgeTransport::new("test-native");
        let health = transport.health().unwrap();
        assert_eq!(health.server_name, "test-native");
        assert!(health.healthy);
    }

    #[test]
    fn native_transport_dispatches_registered_handler() {
        let mut transport = NativeBridgeTransport::new("test");
        transport.register("echo", Arc::new(|params| Ok(params.clone())));
        let request = BridgeRequest {
            id: new_request_id(),
            method: "echo".to_string(),
            params: serde_json::json!({"hello": "world"}),
        };
        let response = transport.call(request).unwrap();
        let result: serde_json::Value = unpack_bridge_response("test", "echo", response).unwrap();
        assert_eq!(result["hello"], "world");
    }

    #[test]
    fn native_transport_returns_method_not_found() {
        let transport = NativeBridgeTransport::new("test");
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
    fn native_transport_returns_handler_error() {
        let mut transport = NativeBridgeTransport::new("test");
        transport.register(
            "fail",
            Arc::new(|_| {
                Err(BridgeErrorPayload {
                    code: -32603,
                    message: "something broke".to_string(),
                })
            }),
        );
        let request = BridgeRequest {
            id: new_request_id(),
            method: "fail".to_string(),
            params: serde_json::json!({}),
        };
        let response = transport.call(request).unwrap();
        assert_eq!(response.error.unwrap().message, "something broke");
    }

    #[test]
    fn native_transport_descriptor_contains_bridge_name() {
        let transport = NativeBridgeTransport::new("my-bridge");
        let desc = transport.descriptor();
        assert!(desc.identity.contains("my-bridge"));
        assert!(desc.identity.contains("native"));
    }

    #[test]
    fn native_transport_health_via_trait() {
        let transport = NativeBridgeTransport::new("test");
        let health: BridgeHealth = transport.health().unwrap();
        assert!(health.healthy);
    }
}
