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
        let bytes = msg.to_bytes().expect("serialize test message");
        assert_eq!(
            msg,
            IpcMessage::from_bytes(&bytes).expect("test operation should succeed")
        );
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
    let json: serde_json::Value =
        serde_json::from_slice(&msg.to_bytes().expect("serialize test message"))
            .expect("deserialize test JSON");
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
    let decoded = IpcMessage::from_bytes(&msg.to_bytes().expect("serialize test message"))
        .expect("deserialize test message");
    assert!(matches!(decoded, IpcMessage::TaskResult { id, outcome }
        if id == "task-99" && outcome == "completed with 3 files changed"));
}

#[test]
fn shutdown_message_serialization() {
    let bytes = IpcMessage::Shutdown
        .to_bytes()
        .expect("serialize test message");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("deserialize test JSON");
    assert_eq!(json["type"], "shutdown");
    assert_eq!(
        json.as_object()
            .expect("test operation should succeed")
            .len(),
        1
    );
}

#[test]
fn stdio_transport_send_succeeds() {
    let mut t = StdioTransport::new(Box::new(std::io::sink()), Box::new(std::io::empty()));
    t.send(b"hello").expect("send test message");
}

#[test]
fn stdio_transport_recv_eof_returns_error() {
    let mut t = StdioTransport::new(Box::new(std::io::sink()), Box::new(std::io::empty()));
    assert!(t.recv().is_err());
}

#[cfg(unix)]
#[test]
fn stdio_transport_send_recv_round_trip() {
    let (s1, s2) = std::os::unix::net::UnixStream::pair().expect("create unix stream pair");
    let mut sender = StdioTransport::new(Box::new(s1), Box::new(std::io::empty()));
    let mut receiver = StdioTransport::new(Box::new(std::io::sink()), Box::new(s2));
    let msg = IpcMessage::Ping.to_bytes().expect("serialize test message");
    sender.send(&msg).expect("send test message");
    assert_eq!(msg, receiver.recv().expect("receive test message"));
}

#[cfg(unix)]
#[test]
fn unix_socket_transport_send_recv_via_pair() {
    let (s1, s2) = std::os::unix::net::UnixStream::pair().expect("create unix stream pair");
    let mut t1 = UnixSocketTransport::from_stream(s1);
    let mut t2 = UnixSocketTransport::from_stream(s2);
    let bytes = IpcMessage::TaskAssign {
        id: "x".into(),
        objective: "y".into(),
    }
    .to_bytes()
    .expect("test operation should succeed");
    t1.send(&bytes).expect("send test message");
    assert_eq!(bytes, t2.recv().expect("receive test message"));
}

#[cfg(unix)]
#[test]
fn unix_socket_transport_bind_connect_exchange() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let sock = dir.path().join("test.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind test socket");
    let sock2 = sock.clone();
    let jh = std::thread::spawn(move || {
        UnixSocketTransport::connect(&sock2).expect("connect to test socket")
    });
    let (stream, _) = listener.accept().expect("accept connection");
    let mut server = UnixSocketTransport::from_stream(stream);
    let mut client = jh.join().expect("join thread");
    let ping = IpcMessage::Ping.to_bytes().expect("serialize test message");
    client.send(&ping).expect("send test message");
    assert_eq!(ping, server.recv().expect("receive test message"));
    let pong = IpcMessage::Pong.to_bytes().expect("serialize test message");
    server.send(&pong).expect("send test message");
    assert_eq!(pong, client.recv().expect("receive test message"));
}

#[test]
fn ipc_subprocess_handle_construction() {
    use std::process::Command;
    let child = Command::new("true").spawn().expect("spawn test process");
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
    handle.child.wait().expect("wait for child process");
}

#[cfg(unix)]
#[test]
fn spawn_subprocess_nonexistent_binary_returns_error() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let sock = dir.path().join("spawn.sock");
    let result = spawn_subprocess(std::path::Path::new("/no/binary"), "test", &sock);
    assert!(matches!(result, Err(SimardError::BridgeSpawnFailed { .. })));
}
