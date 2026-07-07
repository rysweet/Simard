//! TDD (RED) tests for the additive IPC surface that carries the #2679 semantic
//! agent→agent handoff.
//!
//! Post-#2679 the distiller agentic step no longer prints a `{ "facts": [...] }`
//! envelope for Simard to scrape back out of noisy recipe stdout. Instead it
//! writes each fact DIRECTLY through the cognitive-memory write boundary. That
//! boundary is the OODA daemon's memory IPC socket, extended here with two
//! **additive** requests and one response variant:
//!
//!   * `MemoryRequest::StoreFactGated` — a per-fact write that the *server*
//!     grounds, scores, clamps, dedups, and either stores or quarantines. The
//!     server is the single authoritative gate: it never trusts the client's
//!     `confidence` or `source_episode_ids` as fact.
//!   * `MemoryRequest::StoreProcedureProvenance` — a procedure write carrying
//!     its source episode ids for the `PROCEDURE_DERIVES_FROM` edge.
//!   * `MemoryResponse::FactWrite(FactWriteOutcome)` — reports the *server's*
//!     disposition (`stored` / `quarantined`) and the confidence it computed, so
//!     the caller (and the `simard memory remember` CLI) can surface the result
//!     WITHOUT any document for Simard to deserialize.
//!
//! The pre-existing variants (`Ping`, `StoreFact`, …) are untouched — this is a
//! back-compatible extension, so the stub and legacy runners keep working.
//!
//! These tests reference symbols that do not exist yet
//! (`StoreFactGated`, `StoreProcedureProvenance`, `FactWriteOutcome`,
//! `MAX_FRAME`, `RemoteCognitiveMemory::remember_fact_gated`, …); the
//! unresolved-path / missing-variant errors are the intended TDD red signal.

// `MAX_FRAME` is a compile-time constant, so the bound-window assertions in
// `max_frame_cap_is_bounded_and_generous_for_one_fact` are constant-valued by
// design — that IS the contract (pin the const's sane window). Allow the lint so
// the `-D warnings` gate stays green without weakening the assertion.
#![allow(clippy::assertions_on_constants)]

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

use super::{
    FactWriteOutcome, MAX_FRAME, MemoryRequest, MemoryResponse, RemoteCognitiveMemory, read_frame,
    spawn_server, write_frame,
};

// ───────────────────────────────────────────────────────────────────────────
// Additive protocol variants round-trip through serde unchanged
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn store_fact_gated_round_trips_all_fields() {
    let req = MemoryRequest::StoreFactGated {
        concept: "bug-pattern".into(),
        content: "empty outcome list panics cycle".into(),
        // A hostile/optimistic client confidence the server must IGNORE.
        confidence: 0.99,
        tags: vec!["bug-pattern".into()],
        source_id: "distill:epi_00007".into(),
        source_episode_ids: vec!["epi_00007".into()],
        pass_id: "pass-abc123".into(),
    };
    let bytes = serde_json::to_vec(&req).expect("serialize");
    let back: MemoryRequest = serde_json::from_slice(&bytes).expect("deserialize");
    match back {
        MemoryRequest::StoreFactGated {
            concept,
            content,
            confidence,
            tags,
            source_id,
            source_episode_ids,
            pass_id,
        } => {
            assert_eq!(concept, "bug-pattern");
            assert_eq!(content, "empty outcome list panics cycle");
            assert_eq!(confidence, 0.99);
            assert_eq!(tags, vec!["bug-pattern".to_string()]);
            assert_eq!(source_id, "distill:epi_00007");
            assert_eq!(source_episode_ids, vec!["epi_00007".to_string()]);
            assert_eq!(pass_id, "pass-abc123");
        }
        other => panic!("expected StoreFactGated, got {other:?}"),
    }
}

#[test]
fn store_procedure_provenance_round_trips_all_fields() {
    let req = MemoryRequest::StoreProcedureProvenance {
        name: "ci-fix:auto".into(),
        steps: vec!["re-run".into(), "inspect logs".into()],
        prerequisites: vec![],
        source_episode_ids: vec!["epi_1".into(), "epi_2".into()],
        pass_id: "pass-abc123".into(),
    };
    let bytes = serde_json::to_vec(&req).expect("serialize");
    let back: MemoryRequest = serde_json::from_slice(&bytes).expect("deserialize");
    match back {
        MemoryRequest::StoreProcedureProvenance {
            name,
            steps,
            source_episode_ids,
            ..
        } => {
            assert_eq!(name, "ci-fix:auto");
            assert_eq!(steps.len(), 2);
            assert_eq!(
                source_episode_ids,
                vec!["epi_1".to_string(), "epi_2".to_string()]
            );
        }
        other => panic!("expected StoreProcedureProvenance, got {other:?}"),
    }
}

#[test]
fn fact_write_response_round_trips() {
    let resp = MemoryResponse::FactWrite(FactWriteOutcome {
        stored: true,
        quarantined: false,
        confidence: 0.9,
        node_id: Some("sem_42".into()),
    });
    let bytes = serde_json::to_vec(&resp).expect("serialize");
    let back: MemoryResponse = serde_json::from_slice(&bytes).expect("deserialize");
    match back {
        MemoryResponse::FactWrite(o) => {
            assert!(o.stored);
            assert!(!o.quarantined);
            assert_eq!(o.confidence, 0.9);
            assert_eq!(o.node_id.as_deref(), Some("sem_42"));
        }
        other => panic!("expected FactWrite, got {other:?}"),
    }
}

/// The existing variants must still round-trip after the additive extension —
/// belt-and-suspenders that the new arms did not perturb the serde tag layout.
#[test]
fn preexisting_store_fact_variant_unaffected_by_extension() {
    let req = MemoryRequest::StoreFact {
        concept: "gravity".into(),
        content: "things fall".into(),
        confidence: 0.9,
        tags: vec!["physics".into()],
        source_id: "src-1".into(),
    };
    let bytes = serde_json::to_vec(&req).unwrap();
    let back: MemoryRequest = serde_json::from_slice(&bytes).unwrap();
    assert!(matches!(back, MemoryRequest::StoreFact { .. }));
}

// ───────────────────────────────────────────────────────────────────────────
// DOS-1: read_frame must reject an oversized length prefix (MAX_FRAME cap)
// ───────────────────────────────────────────────────────────────────────────

/// A hostile / corrupt client can send a 4-byte length prefix claiming a
/// multi-gigabyte body. Pre-#2679 `read_frame` did `vec![0u8; len]` and would
/// try to allocate it. The hardened frame reader must reject any length that
/// exceeds `MAX_FRAME` BEFORE allocating or reading the body.
#[test]
fn read_frame_rejects_length_over_max_frame() {
    let bogus_len = (MAX_FRAME as u64 + 1) as u32; // just over the cap
    let mut framed = Vec::new();
    framed.extend_from_slice(&bogus_len.to_be_bytes());
    // Deliberately provide NO body — a correct implementation rejects on the
    // length alone and never blocks trying to read `bogus_len` bytes.
    let mut cursor = Cursor::new(framed);
    let result = read_frame(&mut cursor);
    assert!(
        result.is_err(),
        "read_frame must reject a length prefix exceeding MAX_FRAME without allocating the body"
    );
}

/// A frame at or under the cap still round-trips: the cap only rejects abuse,
/// never legitimate per-fact payloads (a single fact is a few hundred bytes).
#[test]
fn write_then_read_frame_round_trips_under_cap() {
    let payload = br#"{"op":"ping"}"#;
    assert!(
        payload.len() < MAX_FRAME,
        "test payload must be under the cap"
    );
    let mut buf = Vec::new();
    write_frame(&mut buf, payload).expect("write_frame");
    let mut cursor = Cursor::new(buf);
    let got = read_frame(&mut cursor).expect("read_frame under cap must succeed");
    assert_eq!(got, payload);
}

#[test]
fn max_frame_cap_is_bounded_and_generous_for_one_fact() {
    // The cap must be large enough for a single distilled fact (well under a
    // megabyte) yet small enough to bound a malicious allocation. Pin a sane
    // window rather than an exact value so tuning stays free.
    assert!(
        MAX_FRAME >= 64 * 1024,
        "MAX_FRAME must comfortably fit one fact write"
    );
    assert!(
        MAX_FRAME <= 64 * 1024 * 1024,
        "MAX_FRAME must bound a hostile allocation"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Client wrapper: remember_fact_gated round-trips through a real server + gate
// ───────────────────────────────────────────────────────────────────────────

/// Spin up a real IPC server over an in-memory library backend, store a real
/// episode so the fact is *grounded* by store-existence, then commit a fact via
/// the new inherent client wrapper. The server gate stores it and reports the
/// disposition — with NO envelope for Simard to parse anywhere in the path.
#[test]
fn remember_fact_gated_stores_a_grounded_fact_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));

    // Seed a real episode; its returned id is what makes a citing fact grounded.
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");

    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let outcome = client
        .remember_fact_gated(
            "bug-pattern",
            "empty outcome list panics cycle",
            // Client-supplied confidence is a hint the server must not trust.
            0.99,
            &["bug-pattern".to_string()],
            "distill:something",
            std::slice::from_ref(&episode_id),
            "pass-xyz",
        )
        .expect("remember_fact_gated call");

    assert!(
        outcome.stored,
        "a grounded, well-formed fact must be stored"
    );
    assert!(!outcome.quarantined);
    // The server RE-derived confidence from the gate, not the client's 0.99.
    assert!(
        (outcome.confidence - 0.9).abs() < 1e-9,
        "server must report its own computed confidence (0.9), not the client hint; got {}",
        outcome.confidence
    );

    // The fact is now retrievable — proving it really reached semantic memory.
    let facts = client.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(
        facts
            .iter()
            .any(|f| f.content == "empty outcome list panics cycle"),
        "the committed fact must be present in semantic memory"
    );
}

/// A fact citing an episode id that does NOT exist in the store is ungrounded
/// and must be quarantined by the server gate — even though the client claimed
/// a high confidence. This is the anti-hallucination guarantee at the boundary.
#[test]
fn remember_fact_gated_quarantines_an_ungrounded_fact() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let outcome = client
        .remember_fact_gated(
            "bug-pattern",
            "three or more words here",
            0.99,
            &[],
            "distill:ghost",
            &["epi_does_not_exist".to_string()],
            "pass-xyz",
        )
        .expect("remember_fact_gated call");

    assert!(!outcome.stored, "an ungrounded fact must not be stored");
    assert!(
        outcome.quarantined,
        "an ungrounded fact must be quarantined"
    );

    let facts = client.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(
        facts.is_empty(),
        "no ungrounded fact may leak into semantic memory"
    );
}

/// The procedure wrapper commits through the provenance write path and returns
/// the new node id (no document to parse).
#[test]
fn remember_procedure_provenance_returns_an_id() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
    let episode_id = mem
        .store_episode(
            "re-ran ci, inspected logs, fixed flake",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let id = client
        .remember_procedure_provenance(
            "ci-fix:auto",
            &["re-run".to_string(), "inspect logs".to_string()],
            &[],
            std::slice::from_ref(&episode_id),
            "pass-xyz",
        )
        .expect("remember_procedure_provenance call");
    assert!(
        !id.is_empty(),
        "a stored procedure must return a non-empty id"
    );
}

/// Grounding symmetry with the fact gate (issue #2679): a procedure that CITES
/// source episodes none of which resolve has fabricated provenance and MUST be
/// rejected server-side — its `PROCEDURE_DERIVES_FROM` edges would otherwise
/// dangle. (A procedure citing no sources is unaffected; there is nothing to
/// fabricate.)
#[test]
fn remember_procedure_provenance_rejects_ungrounded_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let result = client.remember_procedure_provenance(
        "ci-fix:ghost",
        &["re-run".to_string()],
        &[],
        &["epi_does_not_exist".to_string()],
        "pass-xyz",
    );
    assert!(
        result.is_err(),
        "a procedure whose cited provenance does not resolve must be rejected"
    );

    // Nothing leaked into procedural memory.
    let procs = client.recall_procedure("ci-fix", 10).expect("recall");
    assert!(
        procs.is_empty(),
        "no ungrounded procedure may leak into procedural memory"
    );
}

/// Batch grounding (issue #2679): a fact grounds iff *at least one* of its cited
/// episode ids resolves. This pins the `any_episode_exists` batch semantics — a
/// mix of a bogus id and a real id must still ground (and store) the fact.
#[test]
fn remember_fact_gated_grounds_when_any_cited_episode_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
    let real_id = mem
        .store_episode("observed a real failure", "engineer-cycle", None)
        .expect("store_episode");
    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");
    let outcome = client
        .remember_fact_gated(
            "bug-pattern",
            "grounded via one real cited episode",
            0.0,
            &["bug-pattern".to_string()],
            "distill:mixed",
            &["epi_does_not_exist".to_string(), real_id.clone()],
            "pass-mixed",
        )
        .expect("remember_fact_gated call");

    assert!(
        outcome.stored,
        "a fact citing at least one resolving episode must be grounded and stored"
    );
    assert!(!outcome.quarantined);
}

/// Issue #2679 write ledger: the server counts facts the gate ACCEPTED for a
/// distillation `pass_id`, and `drain_pass_ledger` returns then clears that
/// count — the only way the distiller (which gets no returned document) can
/// report how many facts a pass committed. A quarantined fact is NOT counted.
#[test]
fn drain_pass_ledger_returns_only_gate_accepted_facts_then_clears() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("memory.sock");
    let mem: Arc<dyn CognitiveMemoryOps> =
        Arc::new(LibraryCognitiveMemory::in_memory().expect("in-memory db"));
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let _handle = spawn_server(sock.clone(), Arc::clone(&mem)).expect("spawn server");
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let client = RemoteCognitiveMemory::connect(&sock).expect("connect");

    // A process-unique pass id so this test never collides with the shared
    // global ledger used by the other round-trip tests.
    let pass_id = format!("ledger-test-{}", std::process::id());

    // One grounded fact (accepted) and one ungrounded fact (quarantined).
    let stored = client
        .remember_fact_gated(
            "bug-pattern",
            "empty outcome list panics cycle",
            0.99,
            &["bug-pattern".to_string()],
            "distill:x",
            std::slice::from_ref(&episode_id),
            &pass_id,
        )
        .expect("gated write");
    assert!(stored.stored);
    let quarantined = client
        .remember_fact_gated(
            "bug-pattern",
            "three or more words here",
            0.99,
            &[],
            "distill:ghost",
            &["epi_does_not_exist".to_string()],
            &pass_id,
        )
        .expect("gated write");
    assert!(quarantined.quarantined);

    // Only the accepted fact is counted; the drain returns it exactly once.
    assert_eq!(
        client.drain_pass_ledger(&pass_id).expect("drain"),
        1,
        "the ledger counts only gate-accepted facts for this pass"
    );
    assert_eq!(
        client.drain_pass_ledger(&pass_id).expect("drain again"),
        0,
        "draining clears the pass entry — a second drain is empty"
    );
}
