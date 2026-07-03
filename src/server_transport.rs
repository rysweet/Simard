use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{SimardError, SimardResult};
use crate::metadata::BackendDescriptor;

/// A request sent from Simard to a server.
///
/// The wire format is one JSON object per line on stdin:
/// `{"id":"<uuid>","method":"<name>","params":{...}}\n`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerRequest {
    pub id: String,
    pub method: String,
    pub params: Value,
}

/// A response received from a server on stdout.
///
/// Exactly one of `result` or `error` is present.
/// `{"id":"<uuid>","result":{...}}\n`
/// `{"id":"<uuid>","error":{"code":<int>,"message":"..."}}\n`
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ServerErrorPayload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerErrorPayload {
    pub code: i32,
    pub message: String,
}

/// Well-known server error codes, loosely following JSON-RPC conventions.
// FROZEN WIRE VALUE: numeric JSON-RPC error codes are an external contract.
pub const SERVER_ERROR_METHOD_NOT_FOUND: i32 = -32601; // FROZEN WIRE VALUE
pub const SERVER_ERROR_INTERNAL: i32 = -32603; // FROZEN WIRE VALUE
pub const SERVER_ERROR_TIMEOUT: i32 = -32000; // FROZEN WIRE VALUE
pub const SERVER_ERROR_TRANSPORT: i32 = -32001; // FROZEN WIRE VALUE

/// The JSON-RPC method a server exposes for health checks.
// FROZEN WIRE VALUE: JSON-RPC method name on the wire.
pub const HEALTH_METHOD: &str = "bridge.health";

/// Health status reported by a server in response to [`HEALTH_METHOD`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerHealth {
    pub server_name: String,
    pub healthy: bool,
}

/// Identifies a server by name for error reporting and descriptor use.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ServerId(pub String);

impl Display for ServerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The transport layer for communicating with a server.
///
/// Implementations manage connection lifecycle (subprocess, socket, etc.)
/// and handle the JSON-line framing.
pub trait ServerTransport: Send + Sync {
    /// Send a request and block until a response arrives or an error occurs.
    fn call(&self, request: ServerRequest) -> SimardResult<ServerResponse>;

    /// Return descriptive metadata for reflection.
    fn descriptor(&self) -> BackendDescriptor;

    /// Check whether the server is alive and responsive.
    fn health(&self) -> SimardResult<ServerHealth> {
        let request = ServerRequest {
            id: new_request_id(),
            method: HEALTH_METHOD.to_string(),
            params: Value::Object(serde_json::Map::new()),
        };
        let response = self.call(request)?;
        match response.result {
            Some(value) => {
                serde_json::from_value(value).map_err(|error| SimardError::ServerProtocolError {
                    adapter: "health".to_string(),
                    reason: format!("malformed health response: {error}"),
                })
            }
            None => {
                let message = response
                    .error
                    .map(|e| e.message)
                    .unwrap_or_else(|| "no result in health response".to_string());
                Err(SimardError::ServerCallFailed {
                    adapter: "health".to_string(),
                    method: HEALTH_METHOD.to_string(),
                    reason: message,
                })
            }
        }
    }
}

/// Unpack a `ServerResponse` into a typed result or a `SimardError`.
pub fn unpack_server_response<T: serde::de::DeserializeOwned>(
    adapter_name: &str,
    method: &str,
    response: ServerResponse,
) -> SimardResult<T> {
    if let Some(error) = response.error {
        return Err(SimardError::ServerCallFailed {
            adapter: adapter_name.to_string(),
            method: method.to_string(),
            reason: error.message,
        });
    }
    let value = response
        .result
        .ok_or_else(|| SimardError::ServerProtocolError {
            adapter: adapter_name.to_string(),
            reason: format!("response to '{method}' has neither result nor error"),
        })?;
    serde_json::from_value(value).map_err(|error| SimardError::ServerProtocolError {
        adapter: adapter_name.to_string(),
        reason: format!("cannot deserialize '{method}' result: {error}"),
    })
}

/// Generate a compact unique request id.
pub fn new_request_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_request_serializes_to_single_json_line() {
        let request = ServerRequest {
            id: "test-001".to_string(),
            method: "memory.store_fact".to_string(),
            params: serde_json::json!({"concept": "rust", "confidence": 0.9}),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains('\n'));
        let parsed: ServerRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "memory.store_fact");
    }

    #[test]
    fn adapter_response_success_omits_error_field() {
        let response = ServerResponse {
            id: "test-002".to_string(),
            result: Some(serde_json::json!({"ok": true})),
            error: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("error"));
    }

    #[test]
    fn adapter_response_error_omits_result_field() {
        let response = ServerResponse {
            id: "test-003".to_string(),
            result: None,
            error: Some(ServerErrorPayload {
                code: SERVER_ERROR_INTERNAL,
                message: "database locked".to_string(),
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("result"));
        assert!(json.contains("database locked"));
    }

    #[test]
    fn unpack_server_response_deserializes_typed_result() {
        let response = ServerResponse {
            id: "test-004".to_string(),
            result: Some(serde_json::json!({"server_name": "test", "healthy": true})),
            error: None,
        };
        let health: ServerHealth =
            unpack_server_response("test", "bridge.health", response).unwrap();
        assert_eq!(health.server_name, "test");
        assert!(health.healthy);
    }

    #[test]
    fn unpack_server_response_returns_error_for_adapter_error() {
        let response = ServerResponse {
            id: "test-005".to_string(),
            result: None,
            error: Some(ServerErrorPayload {
                code: SERVER_ERROR_METHOD_NOT_FOUND,
                message: "no such method".to_string(),
            }),
        };
        let result: SimardResult<ServerHealth> =
            unpack_server_response("test", "bridge.health", response);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("no such method"));
    }

    #[test]
    fn unpack_server_response_rejects_empty_response() {
        let response = ServerResponse {
            id: "test-006".to_string(),
            result: None,
            error: None,
        };
        let result: SimardResult<ServerHealth> =
            unpack_server_response("test", "bridge.health", response);
        assert!(result.is_err());
    }

    #[test]
    fn new_request_id_is_unique() {
        let a = new_request_id();
        let b = new_request_id();
        assert_ne!(a, b);
    }
}
