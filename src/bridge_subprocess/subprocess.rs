use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::bridge::{
    BRIDGE_ERROR_TIMEOUT, BRIDGE_ERROR_TRANSPORT, BridgeErrorPayload, BridgeRequest,
    BridgeResponse,
};
use crate::error::{SimardError, SimardResult};
use crate::metadata::{BackendDescriptor, Freshness};

use crate::bridge::BridgeTransport;

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
    stdout: Arc<Mutex<BufReader<std::process::ChildStdout>>>,
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
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
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

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(BridgeResponse {
                    id: expected_id.to_string(),
                    result: None,
                    error: Some(BridgeErrorPayload {
                        code: BRIDGE_ERROR_TIMEOUT,
                        message: format!("bridge '{bridge_name}' timed out after {timeout:?}"),
                    }),
                });
            }

            let mut line_buf = String::new();
            let read_result = Self::read_line_with_timeout(child, &mut line_buf, remaining);

            match read_result {
                Ok(0) => {
                    return Err(SimardError::BridgeTransportError {
                        bridge: bridge_name.to_string(),
                        reason: "child process closed stdout (process exited?)".to_string(),
                    });
                }
                Ok(_) => {
                    let trimmed = line_buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let response: BridgeResponse =
                        serde_json::from_str(trimmed).map_err(|error| {
                            SimardError::BridgeProtocolError {
                                bridge: bridge_name.to_string(),
                                reason: format!("malformed response line: {error}"),
                            }
                        })?;
                    if response.id == expected_id {
                        return Ok(response);
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    continue; // loop will check deadline
                }
                Err(error) => {
                    return Err(SimardError::BridgeTransportError {
                        bridge: bridge_name.to_string(),
                        reason: format!("failed to read from child stdout: {error}"),
                    });
                }
            }
        }
    }

    /// Read a line from child stdout with a timeout, using a background thread
    /// to avoid blocking the caller indefinitely.
    fn read_line_with_timeout(
        child: &mut ManagedChild,
        buf: &mut String,
        timeout: Duration,
    ) -> std::io::Result<usize> {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        let stdout = Arc::clone(&child.stdout);

        std::thread::spawn(move || {
            let result = match stdout.lock() {
                Ok(mut reader) => {
                    let mut local_buf = String::new();
                    let result = reader.read_line(&mut local_buf);
                    (result, local_buf)
                }
                Err(_) => (
                    Err(std::io::Error::other("stdout mutex poisoned")),
                    String::new(),
                ),
            };
            let _ = tx.send(result);
        });

        match rx.recv_timeout(timeout) {
            Ok((result, line)) => {
                buf.push_str(&line);
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "read_line timed out waiting for subprocess output",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "reader thread disconnected unexpectedly",
            )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::BRIDGE_ERROR_TIMEOUT;

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

    #[test]
    fn read_response_returns_timeout_error_when_subprocess_is_silent() {
        // Spawn a subprocess that reads stdin but never writes to stdout.
        // read_response should return a BridgeResponse with BRIDGE_ERROR_TIMEOUT.
        let mut child_process = std::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("cat should spawn");

        let stdin = child_process.stdin.take().unwrap();
        let stdout = child_process.stdout.take().unwrap();

        let mut managed = ManagedChild {
            process: child_process,
            stdin: std::io::BufWriter::new(stdin),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
        };

        let start = std::time::Instant::now();
        let result = SubprocessBridgeTransport::read_response(
            &mut managed,
            "test-id-1",
            Duration::from_millis(200),
            "silent-bridge",
        );
        let elapsed = start.elapsed();

        // Should not hang — should return within a reasonable margin of the timeout
        assert!(
            elapsed < Duration::from_secs(3),
            "read_response should not hang; elapsed: {elapsed:?}"
        );

        let response = result.expect("timeout should return Ok with error payload, not Err");
        let error = response
            .error
            .expect("timeout response should contain an error payload");
        assert_eq!(
            error.code, BRIDGE_ERROR_TIMEOUT,
            "error code should be BRIDGE_ERROR_TIMEOUT"
        );
        assert!(
            error.message.contains("timed out"),
            "error message should mention timeout: {}",
            error.message
        );

        // Clean up the child process
        let _ = managed.process.kill();
        let _ = managed.process.wait();
    }
}
