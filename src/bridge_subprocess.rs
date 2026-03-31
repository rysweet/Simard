use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::bridge::{
    BRIDGE_ERROR_TIMEOUT, BRIDGE_ERROR_TRANSPORT, BridgeErrorPayload, BridgeRequest, BridgeResponse,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};

use super::bridge::BridgeTransport;

/// A bridge transport that spawns a Python subprocess and communicates via
/// newline-delimited JSON on stdin/stdout.
///
/// The subprocess is expected to read one JSON request per line from stdin
/// and write one JSON response per line to stdout, matching by `id`.
///
/// Lifecycle: the subprocess is spawned on first `call()` and killed on drop.
pub struct SubprocessBridgeTransport {
    bridge_name: String,
    python_script: PathBuf,
    extra_args: Vec<String>,
    timeout: Duration,
    state: Mutex<TransportState>,
}

struct TransportState {
    child: Option<ManagedChild>,
}

struct ManagedChild {
    process: Child,
    stdin: std::io::BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl SubprocessBridgeTransport {
    /// Create a new subprocess bridge transport.
    ///
    /// - `bridge_name`: human-readable name for error reporting
    /// - `python_script`: path to the Python bridge server script
    /// - `extra_args`: additional arguments passed to the script
    /// - `timeout`: maximum time to wait for a response
    pub fn new(
        bridge_name: impl Into<String>,
        python_script: impl Into<PathBuf>,
        extra_args: Vec<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            bridge_name: bridge_name.into(),
            python_script: python_script.into(),
            extra_args,
            timeout,
            state: Mutex::new(TransportState { child: None }),
        }
    }

    fn spawn_child(&self) -> SimardResult<ManagedChild> {
        let mut command = Command::new("python3");
        command
            .arg(&self.python_script)
            .args(&self.extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .map_err(|error| SimardError::BridgeSpawnFailed {
                bridge: self.bridge_name.clone(),
                reason: format!(
                    "failed to spawn python3 {}: {error}",
                    self.python_script.display()
                ),
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SimardError::BridgeSpawnFailed {
                bridge: self.bridge_name.clone(),
                reason: "child stdin is not available".to_string(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SimardError::BridgeSpawnFailed {
                bridge: self.bridge_name.clone(),
                reason: "child stdout is not available".to_string(),
            })?;
        Ok(ManagedChild {
            process: child,
            stdin: std::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    fn ensure_child<'a>(
        &self,
        state: &'a mut TransportState,
    ) -> SimardResult<&'a mut ManagedChild> {
        if state.child.is_none() {
            state.child = Some(self.spawn_child()?);
        }
        // Safe: the `if` guard above guarantees `child` is `Some` at this point.
        Ok(state
            .child
            .as_mut()
            .expect("child was set in the guard above"))
    }

    fn send_request(child: &mut ManagedChild, request: &BridgeRequest) -> SimardResult<()> {
        let mut line =
            serde_json::to_string(request).map_err(|error| SimardError::BridgeProtocolError {
                bridge: "subprocess".to_string(),
                reason: format!("failed to serialize request: {error}"),
            })?;
        line.push('\n');
        child.stdin.write_all(line.as_bytes()).map_err(|error| {
            SimardError::BridgeTransportError {
                bridge: "subprocess".to_string(),
                reason: format!("failed to write to child stdin: {error}"),
            }
        })?;
        child
            .stdin
            .flush()
            .map_err(|error| SimardError::BridgeTransportError {
                bridge: "subprocess".to_string(),
                reason: format!("failed to flush child stdin: {error}"),
            })?;
        Ok(())
    }

    fn read_response(
        child: &mut ManagedChild,
        expected_id: &str,
        timeout: Duration,
        bridge_name: &str,
    ) -> SimardResult<BridgeResponse> {
        let deadline = Instant::now() + timeout;
        let mut line_buf = String::new();
        loop {
            if Instant::now() > deadline {
                return Ok(BridgeResponse {
                    id: expected_id.to_string(),
                    result: None,
                    error: Some(BridgeErrorPayload {
                        code: BRIDGE_ERROR_TIMEOUT,
                        message: format!("bridge '{bridge_name}' timed out after {timeout:?}"),
                    }),
                });
            }
            line_buf.clear();
            let bytes_read = child.stdout.read_line(&mut line_buf).map_err(|error| {
                SimardError::BridgeTransportError {
                    bridge: bridge_name.to_string(),
                    reason: format!("failed to read from child stdout: {error}"),
                }
            })?;
            if bytes_read == 0 {
                return Err(SimardError::BridgeTransportError {
                    bridge: bridge_name.to_string(),
                    reason: "child process closed stdout (process exited?)".to_string(),
                });
            }
            let trimmed = line_buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let response: BridgeResponse = serde_json::from_str(trimmed).map_err(|error| {
                SimardError::BridgeProtocolError {
                    bridge: bridge_name.to_string(),
                    reason: format!("malformed response line: {error}"),
                }
            })?;
            if response.id == expected_id {
                return Ok(response);
            }
        }
    }
}

impl BridgeTransport for SubprocessBridgeTransport {
    fn call(&self, request: BridgeRequest) -> SimardResult<BridgeResponse> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| SimardError::StoragePoisoned {
                store: format!("bridge:{}", self.bridge_name),
            })?;
        let expected_id = request.id.clone();
        let child = self.ensure_child(&mut state)?;
        Self::send_request(child, &request)?;
        let response = Self::read_response(child, &expected_id, self.timeout, &self.bridge_name)?;
        if let Some(ref error) = response.error
            && error.code == BRIDGE_ERROR_TRANSPORT
        {
            state.child = None;
        }
        Ok(response)
    }

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor::for_runtime_type::<Self>(
            format!(
                "bridge:subprocess:{}:{}",
                self.bridge_name,
                self.python_script.display()
            ),
            format!("bridge::subprocess::{}", self.bridge_name),
            Freshness::now().unwrap_or(Freshness {
                state: crate::metadata::FreshnessState::Stale,
                observed_at_unix_ms: 0,
            }),
        )
    }
}

impl Drop for SubprocessBridgeTransport {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(ref mut managed) = state.child
        {
            let _ = managed.process.kill();
            let _ = managed.process.wait();
        }
    }
}

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

    #[test]
    fn subprocess_transport_descriptor_includes_script_path() {
        let transport = SubprocessBridgeTransport::new(
            "test",
            "/tmp/test_bridge.py",
            vec![],
            Duration::from_secs(5),
        );
        let desc = transport.descriptor();
        assert!(desc.identity.contains("test_bridge.py"));
    }
}
