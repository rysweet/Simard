use super::memory_ipc::*;
use super::*;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn memory_request_roundtrip_ping() {
    let req = MemoryRequest::Ping;
    let bytes = serde_json::to_vec(&req).unwrap();
    let back: MemoryRequest = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(back, MemoryRequest::Ping));
}

#[test]
fn memory_response_roundtrip_error() {
    let resp = MemoryResponse::Error("boom".into());
    let bytes = serde_json::to_vec(&resp).unwrap();
    let back: MemoryResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(back, MemoryResponse::Error(ref s) if s == "boom"));
}

#[test]
fn default_socket_path_under_dot_simard() {
    let p = default_socket_path();
    assert!(p.to_string_lossy().contains("/.simard/memory.sock"));
}

#[test]
fn reap_stale_lock_noop_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    let reaped = reap_stale_open_lock(dir.path()).unwrap();
    assert!(!reaped);
}

#[test]
fn reap_stale_lock_removes_file_with_dead_pid() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("cognitive_memory.ladybug.open.lock");
    // Use PID 1 doesn't work (it's always alive) so use a value that's
    // definitely not a live pid: one with a high number that's unlikely.
    // But `is_pid_alive` uses kill(0) which is unreliable. Use an empty
    // file and rely on flock_held=false path instead.
    std::fs::write(&lock, b"").unwrap();
    let reaped = reap_stale_open_lock(dir.path()).unwrap();
    assert!(reaped);
    assert!(!lock.exists());
}

#[test]
fn server_client_roundtrip_with_in_memory_backend() {
    use crate::cognitive_memory::NativeCognitiveMemory;

    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");

    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(NativeCognitiveMemory::in_memory().expect("in-memory db"));
    let _handle = spawn_server(sock.clone(), mem).expect("spawn server");

    // Give server a moment to start accepting.
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let stats = client.get_statistics().expect("get_statistics");
    assert_eq!(stats.sensory_count, 0);

    let id = client
        .store_fact("gravity", "things fall", 0.9, &["physics".into()], "src-1")
        .expect("store_fact");
    assert!(!id.is_empty());

    let facts = client.search_facts("gravity", 10, 0.0).expect("search");
    assert!(!facts.is_empty());
}
