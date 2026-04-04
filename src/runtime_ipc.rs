//! IPC transport layer for multi-process subprocess spawning.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Child;

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};

/// Transport abstraction for inter-process communication.
pub trait IpcTransport: Send {
    fn send(&mut self, msg: &[u8]) -> SimardResult<()>;
    fn recv(&mut self) -> SimardResult<Vec<u8>>;
}

/// IPC message protocol for subprocess coordination.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage {
    Ping,
    Pong,
    TaskAssign { id: String, objective: String },
    TaskResult { id: String, outcome: String },
    Shutdown,
}

impl IpcMessage {
    pub fn to_bytes(&self) -> SimardResult<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| ipc_err("serialize", &e))
    }

    pub fn from_bytes(data: &[u8]) -> SimardResult<Self> {
        serde_json::from_slice(data).map_err(|e| ipc_err("deserialize", &e))
    }
}

fn ipc_err(action: &str, err: &dyn std::fmt::Display) -> SimardError {
    SimardError::BridgeTransportError {
        bridge: "ipc".to_string(),
        reason: format!("{action}: {err}"),
    }
}

fn io_err(bridge: &str, action: &str, err: &std::io::Error) -> SimardError {
    SimardError::BridgeTransportError {
        bridge: bridge.to_string(),
        reason: format!("{action}: {err}"),
    }
}

/// Transport over stdin/stdout using newline-delimited JSON.
pub struct StdioTransport {
    writer: Box<dyn Write + Send>,
    reader: BufReader<Box<dyn Read + Send>>,
}

impl StdioTransport {
    pub fn new(writer: Box<dyn Write + Send>, reader: Box<dyn Read + Send>) -> Self {
        Self {
            writer,
            reader: BufReader::new(reader),
        }
    }
}

impl IpcTransport for StdioTransport {
    fn send(&mut self, msg: &[u8]) -> SimardResult<()> {
        self.writer
            .write_all(msg)
            .map_err(|e| io_err("stdio", "write", &e))?;
        self.writer
            .write_all(b"\n")
            .map_err(|e| io_err("stdio", "write-nl", &e))?;
        self.writer
            .flush()
            .map_err(|e| io_err("stdio", "flush", &e))
    }

    fn recv(&mut self) -> SimardResult<Vec<u8>> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| io_err("stdio", "read", &e))?;
        if n == 0 {
            return Err(SimardError::BridgeTransportError {
                bridge: "stdio".to_string(),
                reason: "EOF".to_string(),
            });
        }
        Ok(line.trim_end_matches('\n').as_bytes().to_vec())
    }
}

/// Transport over Unix domain sockets with 4-byte big-endian length-prefixed framing.
#[cfg(unix)]
pub struct UnixSocketTransport {
    stream: std::os::unix::net::UnixStream,
}

#[cfg(unix)]
impl UnixSocketTransport {
    pub fn from_stream(stream: std::os::unix::net::UnixStream) -> Self {
        Self { stream }
    }

    pub fn connect(path: &std::path::Path) -> SimardResult<Self> {
        let stream = std::os::unix::net::UnixStream::connect(path)
            .map_err(|e| io_err("unix-socket", "connect", &e))?;
        Ok(Self { stream })
    }
}

#[cfg(unix)]
impl IpcTransport for UnixSocketTransport {
    fn send(&mut self, msg: &[u8]) -> SimardResult<()> {
        let len = u32::try_from(msg.len()).map_err(|_| SimardError::BridgeTransportError {
            bridge: "unix-socket".to_string(),
            reason: format!("message too large: {} bytes", msg.len()),
        })?;
        self.stream
            .write_all(&len.to_be_bytes())
            .map_err(|e| io_err("unix-socket", "write-len", &e))?;
        self.stream
            .write_all(msg)
            .map_err(|e| io_err("unix-socket", "write-payload", &e))?;
        self.stream
            .flush()
            .map_err(|e| io_err("unix-socket", "flush", &e))
    }

    fn recv(&mut self) -> SimardResult<Vec<u8>> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .map_err(|e| io_err("unix-socket", "read-len", &e))?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        self.stream
            .read_exact(&mut buf)
            .map_err(|e| io_err("unix-socket", "read-payload", &e))?;
        Ok(buf)
    }
}

/// Handle to a spawned IPC subprocess.
pub struct IpcSubprocessHandle {
    pub child: Child,
    pub transport: Box<dyn IpcTransport>,
    pub identity_name: String,
    pub socket_path: Option<PathBuf>,
}

impl IpcSubprocessHandle {
    pub fn new(
        child: Child,
        transport: Box<dyn IpcTransport>,
        identity_name: String,
        socket_path: Option<PathBuf>,
    ) -> Self {
        Self {
            child,
            transport,
            identity_name,
            socket_path,
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

/// Spawn a subprocess with Unix socket IPC transport.
#[cfg(unix)]
pub fn spawn_subprocess(
    binary_path: &std::path::Path,
    identity_name: &str,
    socket_path: &std::path::Path,
) -> SimardResult<IpcSubprocessHandle> {
    use std::process::{Command, Stdio};

    let listener = std::os::unix::net::UnixListener::bind(socket_path).map_err(|e| {
        SimardError::BridgeSpawnFailed {
            bridge: "ipc".to_string(),
            reason: format!("bind {}: {e}", socket_path.display()),
        }
    })?;

    let child = Command::new(binary_path)
        .arg("--ipc-socket")
        .arg(socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "ipc".to_string(),
            reason: format!("spawn {}: {e}", binary_path.display()),
        })?;

    let (stream, _) = listener
        .accept()
        .map_err(|e| SimardError::BridgeSpawnFailed {
            bridge: "ipc".to_string(),
            reason: format!("accept: {e}"),
        })?;

    let mut transport = UnixSocketTransport::from_stream(stream);
    transport.send(&IpcMessage::Ping.to_bytes()?)?;
    let response = IpcMessage::from_bytes(&transport.recv()?)?;
    if response != IpcMessage::Pong {
        return Err(SimardError::BridgeSpawnFailed {
            bridge: "ipc".to_string(),
            reason: format!("health check: expected Pong, got {response:?}"),
        });
    }

    Ok(IpcSubprocessHandle::new(
        child,
        Box::new(transport),
        identity_name.to_string(),
        Some(socket_path.to_path_buf()),
    ))
}

/// Gracefully shut down a subprocess.
pub fn shutdown_subprocess(mut handle: IpcSubprocessHandle) -> SimardResult<()> {
    if let Ok(bytes) = IpcMessage::Shutdown.to_bytes() {
        let _ = handle.transport.send(&bytes);
    }
    handle
        .child
        .wait()
        .map_err(|e| SimardError::ActionExecutionFailed {
            action: format!("shutdown '{}'", handle.identity_name),
            reason: e.to_string(),
        })?;
    if let Some(path) = &handle.socket_path {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_message_all_variants_round_trip() {
        for msg in [
            IpcMessage::Ping,
            IpcMessage::Pong,
            IpcMessage::TaskAssign {
                id: "t1".into(),
                objective: "build".into(),
            },
            IpcMessage::TaskResult {
                id: "t1".into(),
                outcome: "ok".into(),
            },
            IpcMessage::Shutdown,
        ] {
            let bytes = msg.to_bytes().unwrap();
            assert_eq!(msg, IpcMessage::from_bytes(&bytes).unwrap());
        }
    }

    #[test]
    fn ipc_message_invalid_bytes_returns_error() {
        assert!(IpcMessage::from_bytes(b"not json").is_err());
    }

    #[test]
    fn ipc_message_tagged_json_format() {
        let msg = IpcMessage::TaskAssign {
            id: "a".into(),
            objective: "b".into(),
        };
        let json: serde_json::Value = serde_json::from_slice(&msg.to_bytes().unwrap()).unwrap();
        assert_eq!(json["type"], "task_assign");
        assert_eq!(json["id"], "a");
        assert_eq!(json["objective"], "b");
    }

    #[test]
    fn ipc_message_task_result_fields_preserved() {
        let msg = IpcMessage::TaskResult {
            id: "task-99".into(),
            outcome: "completed with 3 files changed".into(),
        };
        let decoded = IpcMessage::from_bytes(&msg.to_bytes().unwrap()).unwrap();
        assert!(matches!(decoded, IpcMessage::TaskResult { id, outcome }
            if id == "task-99" && outcome == "completed with 3 files changed"));
    }

    #[test]
    fn shutdown_message_serialization() {
        let bytes = IpcMessage::Shutdown.to_bytes().unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["type"], "shutdown");
        assert_eq!(json.as_object().unwrap().len(), 1);
    }

    #[test]
    fn stdio_transport_send_succeeds() {
        let mut t = StdioTransport::new(Box::new(std::io::sink()), Box::new(std::io::empty()));
        t.send(b"hello").unwrap();
    }

    #[test]
    fn stdio_transport_recv_eof_returns_error() {
        let mut t = StdioTransport::new(Box::new(std::io::sink()), Box::new(std::io::empty()));
        assert!(t.recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stdio_transport_send_recv_round_trip() {
        let (s1, s2) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut sender = StdioTransport::new(Box::new(s1), Box::new(std::io::empty()));
        let mut receiver = StdioTransport::new(Box::new(std::io::sink()), Box::new(s2));
        let msg = IpcMessage::Ping.to_bytes().unwrap();
        sender.send(&msg).unwrap();
        assert_eq!(msg, receiver.recv().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_transport_send_recv_via_pair() {
        let (s1, s2) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut t1 = UnixSocketTransport::from_stream(s1);
        let mut t2 = UnixSocketTransport::from_stream(s2);
        let bytes = IpcMessage::TaskAssign {
            id: "x".into(),
            objective: "y".into(),
        }
        .to_bytes()
        .unwrap();
        t1.send(&bytes).unwrap();
        assert_eq!(bytes, t2.recv().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_transport_bind_connect_exchange() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let sock2 = sock.clone();
        let jh = std::thread::spawn(move || UnixSocketTransport::connect(&sock2).unwrap());
        let (stream, _) = listener.accept().unwrap();
        let mut server = UnixSocketTransport::from_stream(stream);
        let mut client = jh.join().unwrap();
        let ping = IpcMessage::Ping.to_bytes().unwrap();
        client.send(&ping).unwrap();
        assert_eq!(ping, server.recv().unwrap());
        let pong = IpcMessage::Pong.to_bytes().unwrap();
        server.send(&pong).unwrap();
        assert_eq!(pong, client.recv().unwrap());
    }

    #[test]
    fn ipc_subprocess_handle_construction() {
        use std::process::Command;
        let child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        let transport = StdioTransport::new(Box::new(std::io::sink()), Box::new(std::io::empty()));
        let mut handle = IpcSubprocessHandle::new(
            child,
            Box::new(transport),
            "test-agent".into(),
            Some(PathBuf::from("/tmp/t.sock")),
        );
        assert_eq!(handle.pid(), pid);
        assert_eq!(handle.identity_name, "test-agent");
        assert_eq!(handle.socket_path, Some(PathBuf::from("/tmp/t.sock")));
        handle.child.wait().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn spawn_subprocess_nonexistent_binary_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("spawn.sock");
        let result = spawn_subprocess(std::path::Path::new("/no/binary"), "test", &sock);
        assert!(matches!(result, Err(SimardError::BridgeSpawnFailed { .. })));
    }
}
