//! F2b (issue #4929) — memory-ipc client single-shot reconnect on broken pipe.
//!
//! Live evidence showed the daemon journal flooded with
//! `memory-ipc: connection error: … write-len: Broken pipe` and no recovery:
//! once a `RemoteCognitiveMemory`'s `UnixStream` was severed (daemon restart,
//! peer hangup mid-frame), every subsequent `call` failed permanently because
//! the client never re-established the connection.
//!
//! The fix makes `RemoteCognitiveMemory::call` perform an **at-most-once**
//! reconnect to the same stored `socket_path` and retry the single in-flight
//! request. A second failure surfaces a structured `Err` — **no retry loop, no
//! silent fallback**.
//!
//! These tests drive the real Unix-socket wire against a hand-rolled "flaky
//! server" that deliberately severs connections, so they exercise the client's
//! reconnect logic end-to-end (not a mock of it). `call_recovers_…` asserts the
//! landed reconnect path: after the server severs the connection mid-frame, the
//! single-shot reconnect + retry transparently recovers instead of returning
//! `Err`.
//!
//! Robustness: every `accept` in the flaky server is **time-bounded**
//! (`accept_within`), so if the client ever regressed and failed to reconnect,
//! the server thread still terminates instead of blocking a `join` forever.
//! That keeps a regression a clean failure, never a hung test binary.
//!
//! Hermetic: each test binds its own `TempDir` socket, mutates no env, and
//! touches no shared global state — so no `#[serial]` key is required.

use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::cognitive_memory::CognitiveMemoryOps;

use super::{MemoryRequest, MemoryResponse, RemoteCognitiveMemory, read_frame, write_frame};

/// Accept the next connection within `budget`, or return `None`. Polls a
/// non-blocking listener so a reconnect that never arrives cannot wedge the
/// server thread (and therefore cannot hang a `join`).
fn accept_within(listener: &UnixListener, budget: Duration) -> Option<UnixStream> {
    listener
        .set_nonblocking(true)
        .expect("set listener non-blocking");
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("restore blocking stream");
                return Some(stream);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("flaky server accept error: {e}"),
        }
    }
    None
}

/// Serve one framed request on `stream` and reply with `resp`.
fn serve_one(stream: &mut UnixStream, resp: &MemoryResponse) {
    let _req = read_frame(stream).expect("server: read request frame");
    let bytes = serde_json::to_vec(resp).expect("server: serialize response");
    write_frame(stream, &bytes).expect("server: write response frame");
}

/// Read one request frame and then abruptly drop the connection WITHOUT
/// responding, simulating a broken pipe mid-request (daemon crash / restart).
fn read_then_sever(stream: &mut UnixStream) {
    let _req = read_frame(stream).expect("server: read request frame before severing");
    // Returning drops `stream` → the client's pending read sees EOF/broken pipe.
}

/// A severed connection followed by a live listener on the SAME socket path is
/// transparently recovered by a single reconnect + retry: the call returns Ok.
///
/// Exercises the landed F2b path: the client returns `Ok` on a broken pipe by
/// reconnecting once and retrying, so the `expect` below holds (and if it ever
/// regressed the failure is clean — the server thread is time-bounded and never
/// hangs the test).
#[test]
fn call_recovers_from_broken_pipe_with_one_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let listener = UnixListener::bind(&sock).expect("bind flaky server");

    let (done_tx, done_rx) = mpsc::channel::<bool>();
    let server = thread::spawn(move || {
        // conn1: Ping/Pong handshake, then read the real request and SEVER.
        let mut c1 =
            accept_within(&listener, Duration::from_secs(5)).expect("accept conn1 (handshake)");
        serve_one(&mut c1, &MemoryResponse::Pong); // client connect() handshake
        read_then_sever(&mut c1); // client's prune_expired_sensory() first attempt
        drop(c1);

        // conn2: the client's single reconnect + retry. If the client ever
        // regressed and never reconnected, this times out and the server
        // reports "no reconnect" rather than blocking forever.
        match accept_within(&listener, Duration::from_secs(3)) {
            Some(mut c2) => {
                serve_one(&mut c2, &MemoryResponse::Count(0));
                let _ = done_tx.send(true);
            }
            None => {
                let _ = done_tx.send(false);
            }
        }
    });

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect + Ping handshake");

    // This call's first attempt hits the severed conn1; the client must
    // reconnect once to `sock` and retry, ending in Ok.
    let got = client
        .prune_expired_sensory()
        .expect("broken-pipe call must transparently recover via one reconnect");
    assert_eq!(got, 0, "the retried request's payload must round-trip");

    let reconnected = done_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("server thread reported an outcome");
    assert!(
        reconnected,
        "client must have reconnected to the same socket path (server saw conn2)"
    );
    server.join().expect("flaky server thread");
}

/// Reconnect is **at most once**: if the reconnected connection also breaks,
/// the client does NOT loop forever — it surfaces a structured `Err` and
/// returns promptly.
///
/// This holds for BOTH the current client (immediate Err, no reconnect) and the
/// fixed client (one reconnect, then Err); it is a safety guard against an
/// infinite reconnect loop, detected via a completion timeout.
#[test]
fn call_does_not_retry_forever_when_reconnect_also_breaks() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let listener = UnixListener::bind(&sock).expect("bind flaky server");

    // Detached: the server drains up to two connections then exits on its own
    // time-bounded accepts. We never `join` it, so it can never hang the test.
    thread::spawn(move || {
        // conn1: handshake, read request, sever.
        if let Some(mut c1) = accept_within(&listener, Duration::from_secs(5)) {
            serve_one(&mut c1, &MemoryResponse::Pong);
            read_then_sever(&mut c1);
            drop(c1);
        }
        // conn2: the single allowed reconnect — read the retried request, sever
        // again. A correct at-most-once client stops here; a buggy looping
        // client would keep trying conn3, conn4, … (caught by the timeout below).
        if let Some(mut c2) = accept_within(&listener, Duration::from_secs(3)) {
            read_then_sever(&mut c2);
            drop(c2);
        }
    });

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect + Ping handshake");

    // Run the call on a worker thread so a hypothetical infinite reconnect loop
    // is detected as a timeout rather than hanging the whole test binary.
    let (res_tx, res_rx) = mpsc::channel();
    thread::spawn(move || {
        let r = client.prune_expired_sensory();
        let _ = res_tx.send(r.is_err());
    });

    let is_err = res_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("call must return promptly — no infinite reconnect loop");
    assert!(
        is_err,
        "a second broken pipe after the one allowed reconnect must surface Err, not Ok"
    );
}

/// A NON-idempotent request (`store_episode`) whose bytes were already delivered
/// must NOT be re-sent on the heal reconnect (issue #4929 review). The server
/// reads the request (i.e. it may have applied it) and then severs before
/// responding; a blind re-send would mint a SECOND episode. The client must
/// reconnect to heal the poisoned stream but surface `Err` WITHOUT writing a
/// second request frame.
///
/// The server thread reports whether a frame arrived on the reconnect. A frame
/// there is a regression (duplicate write); a timeout/EOF is the correct
/// "reconnected but did not re-send" behavior.
#[test]
fn non_idempotent_post_delivery_failure_is_not_resent() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let listener = UnixListener::bind(&sock).expect("bind flaky server");

    let (resent_tx, resent_rx) = mpsc::channel::<bool>();
    let server = thread::spawn(move || {
        // conn1: Ping/Pong handshake, read the StoreEpisode request (the server
        // may now have applied it), then SEVER without responding.
        let mut c1 =
            accept_within(&listener, Duration::from_secs(5)).expect("accept conn1 (handshake)");
        serve_one(&mut c1, &MemoryResponse::Pong);
        read_then_sever(&mut c1);
        drop(c1);

        // conn2: the client may reconnect to HEAL the stream, but must not
        // re-send. If any frame arrives here the client duplicated the write.
        match accept_within(&listener, Duration::from_secs(3)) {
            Some(mut c2) => {
                c2.set_read_timeout(Some(Duration::from_millis(500)))
                    .expect("set conn2 read timeout");
                let resent = read_frame(&mut c2).is_ok();
                let _ = resent_tx.send(resent);
            }
            // No reconnect at all is also acceptable — nothing was re-sent.
            None => {
                let _ = resent_tx.send(false);
            }
        }
    });

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect + Ping handshake");

    // `store_episode` mints a fresh episode id per call → non-idempotent. Its
    // first attempt is delivered then severed, so the client must surface Err
    // WITHOUT re-sending (which would duplicate the episode).
    let res = client.store_episode("payload", "test", None);
    assert!(
        res.is_err(),
        "a non-idempotent post-delivery failure must surface Err, not a retried Ok"
    );

    let resent = resent_rx
        .recv_timeout(Duration::from_secs(8))
        .expect("server thread reported an outcome");
    assert!(
        !resent,
        "client must NOT re-send a non-idempotent request after a post-delivery failure (would duplicate the write)"
    );
    server.join().expect("flaky server thread");
}

/// Lock the idempotency classification that governs the post-delivery retry
/// decision (issue #4929 review): reads/`Ping`/effect-idempotent mutations and
/// the server-deduped `StoreFactGated` are retry-safe; row-minting, fire-once,
/// and destructive requests are not.
#[test]
fn retry_safe_classification_matches_request_semantics() {
    // Retry-safe: pure reads, liveness, effect-idempotent mutations, deduped write.
    for req in [
        MemoryRequest::Ping,
        MemoryRequest::PruneExpiredSensory,
        MemoryRequest::GetWorking {
            task_id: "t".into(),
        },
        MemoryRequest::ClearWorking {
            task_id: "t".into(),
        },
        MemoryRequest::SearchFacts {
            query: "q".into(),
            limit: 1,
            min_confidence: 0.0,
        },
        MemoryRequest::ListAllEpisodes { limit: 1 },
        MemoryRequest::GetStatistics,
        MemoryRequest::StoreFactGated {
            concept: "c".into(),
            content: "x".into(),
            confidence: 0.5,
            tags: vec![],
            source_id: "s".into(),
            source_episode_ids: vec![],
            pass_id: "p".into(),
        },
    ] {
        assert!(
            req.is_retry_safe(),
            "{req:?} should be retry-safe (idempotent / server-deduped)"
        );
    }

    // NOT retry-safe: row-minting writes, fire-once trigger check, state
    // transitions with side effects, and the destructive ledger drain.
    for req in [
        MemoryRequest::RecordSensory {
            modality: "m".into(),
            raw_data: "d".into(),
            ttl_seconds: 1,
        },
        MemoryRequest::PushWorking {
            slot_type: "s".into(),
            content: "c".into(),
            task_id: "t".into(),
            relevance: 0.5,
        },
        MemoryRequest::StoreEpisode {
            content: "c".into(),
            source_label: "s".into(),
            metadata: None,
        },
        MemoryRequest::ConsolidateEpisodes { batch_size: 1 },
        MemoryRequest::StoreFact {
            concept: "c".into(),
            content: "x".into(),
            confidence: 0.5,
            tags: vec![],
            source_id: "s".into(),
        },
        MemoryRequest::StoreProcedure {
            name: "n".into(),
            steps: vec![],
            prerequisites: vec![],
        },
        MemoryRequest::StoreProspective {
            description: "d".into(),
            trigger_condition: "t".into(),
            action_on_trigger: "a".into(),
            priority: 0,
        },
        MemoryRequest::CheckTriggers {
            content: "c".into(),
        },
        MemoryRequest::ResolveProspective {
            node_id: "n".into(),
        },
        MemoryRequest::DrainPassLedger {
            pass_id: "p".into(),
        },
    ] {
        assert!(
            !req.is_retry_safe(),
            "{req:?} should NOT be retry-safe (row-minting / fire-once / destructive)"
        );
    }
}
