//! TDD (RED) regression tests for issue #4731 — memory-IPC **write-path
//! resilience** against a mid-write peer reset (EPIPE / `Broken pipe`).
//!
//! ## The defect
//!
//! The memory-ipc RPC client repeatedly failed in production with
//! `memory-ipc: connection error: ... write-len: Broken pipe (os error 32)`,
//! clustered around large distillation payload writes and OODA cycle
//! transitions. The peer closes the socket mid-write under load, so a single
//! large frame write hits `EPIPE` and the whole memory write is lost — a
//! *silent* dropped write, the exact class the launcher's no-silent-fallback
//! rule exists to prevent.
//!
//! ## The contract these tests pin (design-ready requirements)
//!
//! 1. On a **write-half** `BrokenPipe`/`EPIPE` (errno 32), the client
//!    [`RemoteCognitiveMemory::call`](super::RemoteCognitiveMemory) must
//!    **reconnect** (fresh connect + inline Ping/Pong handshake, timeouts
//!    re-applied) and **idempotently re-send** the framed request, bounded to
//!    3 attempts with brief backoff. A large payload that hits a mid-write
//!    reset must be **durably delivered** on the reconnect — no data loss,
//!    no truncation. *(test `mid_write_reset_reconnects_and_delivers_payload`)*
//! 2. When **every** attempt hits a reset, the client must **surface** a
//!    [`SimardError::RpcTransportError`] after exhausting its bounded retries —
//!    never a silent `Ok`, never an alternate-transport fallback.
//!    *(test `persistent_reset_surfaces_rpc_transport_error_never_silent`)*
//! 3. If the post-reset reconnect handshake does **not** return exactly
//!    `Pong`, the client must abort with `RpcTransportError` and must **not**
//!    resend the real payload onto an unverified peer.
//!    *(test `non_pong_reconnect_aborts_without_resending_payload`)*
//! 4. Retry/exhaustion diagnostics must carry only transport metadata
//!    (endpoint, errno/ErrorKind, attempt) and **never** the payload bytes.
//!    *(test `surfaced_error_never_leaks_payload_bytes`)*
//!
//! ## Why these fail today (RED)
//!
//! The current `call()` performs a single `write_frame` with no reconnect. So:
//! * (1) returns `Err` instead of the durable `Ok` → RED.
//! * (2)/(3) never open a second connection, so the "a reconnect was
//!   attempted" assertion (`connections >= 2`) fails → RED.
//! * (4) additionally asserts a retry was attempted → RED.
//!
//! All four turn GREEN once the bounded write-half reconnect+retry lands.
//!
//! ## Harness
//!
//! A hermetic, scripted **raw `UnixListener` mock server** (no real store, no
//! env, no global state) that drives each accepted connection through a fixed
//! [`ConnScript`]. The `HandshakeThenReset` script completes the client's
//! handshake (so `connect()` succeeds) and then drops the socket, forcing the
//! client's *next* large-frame write to hit `EPIPE`. This reproduces the
//! production "peer closes mid-write under load" failure deterministically.

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::SimardError;

use super::{MemoryRequest, MemoryResponse, RemoteCognitiveMemory};

// ---------------------------------------------------------------------------
// Payload sizing
// ---------------------------------------------------------------------------

/// A payload large enough to overflow the Unix-socket send buffer, so the
/// client's `write_all` blocks and then observes the peer's mid-write close as
/// `EPIPE` (rather than completing into a kernel buffer). 2 MiB is comfortably
/// under the 8 MiB `MAX_FRAME` cap yet far larger than the default send buffer.
const PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// Sentinel woven into the payload so a confidentiality test can prove the
/// surfaced error never echoes payload bytes.
const PAYLOAD_SENTINEL: &str = "SENTINEL_SECRET_PAYLOAD_MARKER_4731";

fn big_episode_content() -> String {
    let mut s = String::with_capacity(PAYLOAD_BYTES + PAYLOAD_SENTINEL.len());
    s.push_str(PAYLOAD_SENTINEL);
    s.extend(std::iter::repeat_n('x', PAYLOAD_BYTES));
    s
}

// ---------------------------------------------------------------------------
// Raw framing (4-byte big-endian length prefix + JSON body), matching the
// module wire format, implemented independently here so the mock never depends
// on the code under test for its transport behaviour.
// ---------------------------------------------------------------------------

fn read_frame_raw(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    Ok(body)
}

fn write_frame_raw(stream: &mut UnixStream, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len()).expect("mock frame within u32");
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(payload)?;
    stream.flush()
}

fn write_response(stream: &mut UnixStream, resp: &MemoryResponse) -> io::Result<()> {
    let bytes = serde_json::to_vec(resp).expect("serialize mock response");
    write_frame_raw(stream, &bytes)
}

// ---------------------------------------------------------------------------
// Scripted mock server
// ---------------------------------------------------------------------------

/// Per-connection behaviour for the mock server.
#[derive(Clone, Copy)]
enum ConnScript {
    /// Complete the client's Ping→Pong handshake, then drop the socket. The
    /// client's *subsequent* large-frame write hits a mid-write `EPIPE`. Used
    /// both for the `connect()` socket (whose later `store_episode` write is
    /// what resets) and for "every attempt resets" scenarios.
    HandshakeThenReset,
    /// Complete the handshake, then read the full request frame (capturing it
    /// for durable-delivery assertions) and answer with `Id("stored-ok")`.
    HandshakeThenServe,
    /// Answer the reconnect handshake with a **non-`Pong`** frame, then drop.
    /// Verifies the client refuses to resend onto an unverified peer.
    BadHandshake,
}

#[derive(Default)]
struct MockState {
    /// Number of accepted connections (each reconnect adds one).
    connections: usize,
    /// Request frames the mock actually *accepted and read in full* on a
    /// serving connection. Length 0 means no server ever received the payload.
    served_payloads: Vec<Vec<u8>>,
    /// Number of connections that presented a non-`Pong` reconnect handshake.
    bad_handshakes: usize,
}

struct MockServer {
    sock: PathBuf,
    state: Arc<Mutex<MockState>>,
    running: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl MockServer {
    /// Bind a socket in a fresh `TempDir` and spawn the accept loop. `scripts`
    /// are applied to accepted connections in order; any connection beyond the
    /// list uses `default` (so "reset forever" scenarios need only a default).
    fn spawn(scripts: Vec<ConnScript>, default: ConnScript) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let sock = dir.path().join("memory.sock");
        let listener = UnixListener::bind(&sock).expect("bind mock socket");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");

        let state = Arc::new(Mutex::new(MockState::default()));
        let running = Arc::new(AtomicBool::new(true));

        let state_t = Arc::clone(&state);
        let running_t = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("mock-memory-ipc-4731".into())
            .spawn(move || accept_loop(listener, scripts, default, state_t, running_t))
            .expect("spawn mock accept loop");

        Self {
            sock,
            state,
            running,
            join: Some(join),
            _dir: dir,
        }
    }

    fn socket_path(&self) -> &std::path::Path {
        &self.sock
    }

    fn snapshot(&self) -> (usize, usize, usize) {
        let st = self.state.lock().expect("mock state lock");
        (st.connections, st.served_payloads.len(), st.bad_handshakes)
    }

    /// The single request frame a serving connection accepted, decoded as a
    /// [`MemoryRequest`]. Panics if the mock did not serve exactly one frame.
    fn only_served_request(&self) -> MemoryRequest {
        let st = self.state.lock().expect("mock state lock");
        assert_eq!(
            st.served_payloads.len(),
            1,
            "expected exactly one fully-served request frame"
        );
        serde_json::from_slice(&st.served_payloads[0]).expect("decode served request")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn accept_loop(
    listener: UnixListener,
    scripts: Vec<ConnScript>,
    default: ConnScript,
    state: Arc<Mutex<MockState>>,
    running: Arc<AtomicBool>,
) {
    let mut index = 0usize;
    while running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _addr)) => {
                // Accepted sockets are blocking on Linux regardless of the
                // listener's flag, but make the intent explicit.
                let _ = stream.set_nonblocking(false);
                let script = scripts.get(index).copied().unwrap_or(default);
                index += 1;
                {
                    let mut st = state.lock().expect("mock state lock");
                    st.connections += 1;
                }
                // A handler error just ends this connection; the client sees it
                // as the reset/close the scenario intends.
                let _ = handle_conn(&mut stream, script, &state);
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => break,
        }
    }
}

fn handle_conn(
    stream: &mut UnixStream,
    script: ConnScript,
    state: &Arc<Mutex<MockState>>,
) -> io::Result<()> {
    match script {
        ConnScript::HandshakeThenReset => {
            handshake_pong(stream)?;
            // Drop (return) closes the socket. The client's next large-frame
            // write on this connection now hits EPIPE mid-write.
            Ok(())
        }
        ConnScript::HandshakeThenServe => {
            handshake_pong(stream)?;
            let frame = read_frame_raw(stream)?;
            {
                let mut st = state.lock().expect("mock state lock");
                st.served_payloads.push(frame);
            }
            write_response(stream, &MemoryResponse::Id("stored-ok".into()))
        }
        ConnScript::BadHandshake => {
            // Consume the reconnect Ping, then answer with a NON-Pong frame.
            let _ = read_frame_raw(stream)?;
            {
                let mut st = state.lock().expect("mock state lock");
                st.bad_handshakes += 1;
            }
            write_response(
                stream,
                &MemoryResponse::Error("mock-refuses-handshake".into()),
            )
            // Return/drop without ever reading a second frame: proves the
            // client did NOT resend the payload after a failed handshake.
        }
    }
}

/// Read one request frame (expected `Ping`) and answer `Pong`, completing a
/// client handshake so `connect()` / reconnect succeeds.
fn handshake_pong(stream: &mut UnixStream) -> io::Result<()> {
    let frame = read_frame_raw(stream)?;
    // Best-effort decode; the client only ever sends Ping here, but we don't
    // hard-fail the mock on an unexpected shape.
    let _req: Result<MemoryRequest, _> = serde_json::from_slice(&frame);
    write_response(stream, &MemoryResponse::Pong)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (1) A large write that hits a mid-write peer reset must be reconnected and
/// re-sent so the payload is **durably delivered** with no loss/truncation.
///
/// RED today: the current single-shot `call()` returns `Err` on the EPIPE, so
/// `store_episode` never yields the durable `Ok("stored-ok")`.
#[test]
fn mid_write_reset_reconnects_and_delivers_payload() {
    // conn1: serves connect() handshake, then resets the store_episode write.
    // conn2: serves the reconnected, re-sent request in full.
    let mock = MockServer::spawn(
        vec![
            ConnScript::HandshakeThenReset,
            ConnScript::HandshakeThenServe,
        ],
        ConnScript::HandshakeThenServe,
    );
    let client =
        RemoteCognitiveMemory::connect(mock.socket_path()).expect("connect + Ping handshake");

    let content = big_episode_content();
    let id = client
        .store_episode(&content, "distillation", None)
        .expect("large write must survive a mid-write reset and be delivered");

    assert_eq!(
        id, "stored-ok",
        "the durable response must come from the reconnected server"
    );

    let (connections, served, _bad) = mock.snapshot();
    assert!(
        connections >= 2,
        "client must reconnect after the mid-write reset (saw {connections} connection(s))"
    );
    assert_eq!(
        served, 1,
        "the payload must be delivered exactly once after reconnect"
    );

    // No data loss: the fully-served frame must carry the entire payload.
    match mock.only_served_request() {
        MemoryRequest::StoreEpisode { content: got, .. } => {
            assert_eq!(
                got.len(),
                content.len(),
                "delivered payload was truncated (data loss)"
            );
            assert_eq!(got, content, "delivered payload differs from what was sent");
        }
        other => panic!("expected StoreEpisode after reconnect, got {other:?}"),
    }
}

/// (2) When every attempt resets, the client must exhaust its bounded retries
/// and **surface** an `RpcTransportError` — never silently succeed, never
/// silently drop the write.
///
/// RED today: no reconnect is attempted, so `connections` stays at 1.
#[test]
fn persistent_reset_surfaces_rpc_transport_error_never_silent() {
    let mock = MockServer::spawn(vec![], ConnScript::HandshakeThenReset);
    let client =
        RemoteCognitiveMemory::connect(mock.socket_path()).expect("connect + Ping handshake");

    let content = big_episode_content();
    let err = client
        .store_episode(&content, "distillation", None)
        .expect_err("a persistently-resetting peer must surface an error, never a silent Ok");

    assert!(
        matches!(err, SimardError::RpcTransportError { .. }),
        "exhaustion must surface RpcTransportError, got: {err:?}"
    );

    let (connections, served, _bad) = mock.snapshot();
    assert!(
        connections >= 2,
        "client must attempt at least one reconnect before giving up (saw {connections})"
    );
    assert_eq!(
        served, 0,
        "no server ever accepted the payload — it must be surfaced as an error, not dropped silently"
    );
}

/// (3) If the reconnect handshake does not return exactly `Pong`, the client
/// must abort with `RpcTransportError` and must **not** resend the payload onto
/// the unverified peer.
///
/// RED today: no reconnect happens, so the BadHandshake connection is never
/// opened (`connections` stays at 1).
#[test]
fn non_pong_reconnect_aborts_without_resending_payload() {
    // conn1: serves connect() handshake, resets the write.
    // conn2: answers the reconnect handshake with a non-Pong frame.
    let mock = MockServer::spawn(
        vec![ConnScript::HandshakeThenReset, ConnScript::BadHandshake],
        ConnScript::BadHandshake,
    );
    let client =
        RemoteCognitiveMemory::connect(mock.socket_path()).expect("connect + Ping handshake");

    let content = big_episode_content();
    let err = client
        .store_episode(&content, "distillation", None)
        .expect_err("a non-Pong reconnect handshake must surface an error");

    assert!(
        matches!(err, SimardError::RpcTransportError { .. }),
        "a failed reconnect handshake must surface RpcTransportError, got: {err:?}"
    );

    let (connections, served, bad) = mock.snapshot();
    assert!(
        connections >= 2,
        "client must attempt the reconnect (saw {connections} connection(s))"
    );
    assert!(
        bad >= 1,
        "the reconnect handshake must have been exercised and rejected"
    );
    assert_eq!(
        served, 0,
        "the client must NOT resend the payload after an unverified (non-Pong) handshake"
    );
}

/// (4) The surfaced transport error (and thus any diagnostics derived from it)
/// must never echo the payload bytes. Also asserts a reconnect was attempted so
/// this specifically guards the retry/exhaustion path.
///
/// RED today: the `connections >= 2` retry assertion fails (no reconnect).
#[test]
fn surfaced_error_never_leaks_payload_bytes() {
    let mock = MockServer::spawn(vec![], ConnScript::HandshakeThenReset);
    let client =
        RemoteCognitiveMemory::connect(mock.socket_path()).expect("connect + Ping handshake");

    let content = big_episode_content();
    let err = client
        .store_episode(&content, "distillation", None)
        .expect_err("persistent reset must surface an error");

    let rendered = err.to_string();
    assert!(
        !rendered.contains(PAYLOAD_SENTINEL),
        "surfaced error must not leak payload bytes; rendered = {rendered:?}"
    );
    assert!(
        rendered.len() < 4096,
        "surfaced error must be a bounded transport diagnostic, not the payload; len = {}",
        rendered.len()
    );

    let (connections, served, _bad) = mock.snapshot();
    assert!(
        connections >= 2,
        "confidentiality must hold on the retry path — a reconnect must have been attempted (saw {connections})"
    );
    assert_eq!(served, 0, "no server accepted the payload");
}
