//! TDD (RED) tests for the server-side write-boundary gate (issue #2679, gate
//! seam #1 of 2).
//!
//! When the distiller agentic step commits a fact through the daemon socket, the
//! IPC **server** — not the client, not Simard's distillation module — is the
//! single authoritative gate. The `StoreFactGated` dispatch arm must, per fact:
//!
//!   1. **Ground** the fact by confirming at least one of its
//!      `source_episode_ids` resolves to a real episode node in the store (a
//!      store-existence check via `CognitiveMemoryOps::episode_exists`), then
//!   2. **Score** it with the shared `crate::fact_reliability` scorer using that
//!      resolved `grounded` flag (NEVER the client's `confidence` hint), then
//!   3. **Quarantine** anything below `RELIABILITY_THRESHOLD` (ungrounded,
//!      empty-content, …) — writing nothing, and
//!   4. **Dedup**: never let a weaker-or-equal restatement clobber an existing
//!      equal-or-stronger fact of the same identity, then
//!   5. **Persist** survivors via `store_fact_with_provenance` with the
//!      *server-computed* confidence and the source-episode provenance edges.
//!
//! The disposition flows back as `MemoryResponse::FactWrite(FactWriteOutcome)`
//! — there is no document for Simard to deserialize anywhere in the path, so the
//! trailing-comma / noisy-stdout parse-failure mode of #2658/#2679 is
//! structurally impossible here.
//!
//! These tests call the private `super::dispatch` directly against a real
//! in-memory library backend and assert observable outcomes. They reference
//! `MemoryRequest::StoreFactGated`, `MemoryResponse::FactWrite`, and
//! `episode_exists`, none of which exist yet — the intended TDD red signal.
//!
//! Case IDs map to the design's security test plan: SEC-T1 (ungrounded),
//! SEC-T2 (empty content), SEC-T3 (grounded happy path), SEC-T5 (dedup).

use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

use super::dispatch;
use crate::memory_ipc::{FactWriteOutcome, MemoryRequest, MemoryResponse};

/// Build a `StoreFactGated` request with a single source episode id and a
/// deliberately optimistic client confidence the server must ignore.
fn gated(concept: &str, content: &str, source_episode_id: &str) -> MemoryRequest {
    MemoryRequest::StoreFactGated {
        concept: concept.into(),
        content: content.into(),
        confidence: 0.99, // hostile hint — server must re-derive
        tags: vec![concept.into()],
        source_id: format!("distill:{source_episode_id}"),
        source_episode_ids: vec![source_episode_id.into()],
        pass_id: "pass-gate-test".into(),
    }
}

fn expect_fact_write(resp: MemoryResponse) -> FactWriteOutcome {
    match resp {
        MemoryResponse::FactWrite(o) => o,
        other => panic!("expected FactWrite response, got {other:?}"),
    }
}

// ── SEC-T3: grounded, known-concept, well-formed fact is stored ─────────────

#[test]
fn sec_t3_grounded_known_concept_fact_is_stored_with_server_confidence() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");

    let resp = dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated(
            "bug-pattern",
            "empty outcome list panics cycle",
            &episode_id,
        ),
    );
    let outcome = expect_fact_write(resp);

    assert!(
        outcome.stored,
        "grounded, well-formed, known-concept fact must be stored"
    );
    assert!(!outcome.quarantined);
    assert!(
        (outcome.confidence - 0.9).abs() < 1e-9,
        "server must persist its OWN computed confidence (0.9), not the client's 0.99; got {}",
        outcome.confidence
    );

    let facts = mem.search_facts("bug-pattern", 10, 0.0).expect("search");
    let stored = facts
        .iter()
        .find(|f| f.content == "empty outcome list panics cycle")
        .expect("the stored fact must be retrievable");
    assert!(
        (stored.confidence - 0.9).abs() < 1e-9,
        "the persisted fact must carry the server-computed confidence, not the client hint"
    );
}

// ── Seam-parity: a whitespace-padded cited id still grounds + stores ─────────

/// A distiller can re-emit a real episode id with stray surrounding whitespace
/// (a trailing newline copied from context). The server must normalize the cited
/// id (trim) before the exact grounding match so the fact still grounds and
/// stores — identical to the in-process `DistillFactSink` seam, and never leaking
/// a padded key into the persisted `DERIVES_FROM` provenance edge. Guards the
/// grounding/provenance surface-form robustness that keeps the two write-boundary
/// seams deciding identically.
#[test]
fn grounded_when_cited_episode_id_has_surrounding_whitespace() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");

    // Cite the SAME real id but wrapped in whitespace the LLM appended.
    let padded = format!("  {episode_id}\n");
    let resp = dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated("bug-pattern", "empty outcome list panics cycle", &padded),
    );
    let outcome = expect_fact_write(resp);

    assert!(
        outcome.stored,
        "a whitespace-padded cited id of a real episode must still ground and store"
    );
    assert!(!outcome.quarantined);
    assert!(
        (outcome.confidence - 0.9).abs() < 1e-9,
        "grounded + ≥3 words + known concept must score 0.9; got {}",
        outcome.confidence
    );

    let facts = mem.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(
        facts
            .iter()
            .any(|f| f.content == "empty outcome list panics cycle"),
        "the grounded fact must be retrievable from semantic memory"
    );
}

// ── SEC-T1: ungrounded fact is quarantined regardless of client confidence ──

#[test]
fn sec_t1_ungrounded_fact_is_quarantined_even_with_high_client_confidence() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    // No episode with this id exists → episode_exists must return false →
    // ungrounded → score tops out at 0.4 < 0.5 → quarantined. The client's
    // confidence of 0.99 must NOT rescue it.
    let resp = dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated(
            "bug-pattern",
            "three or more words here",
            "epi_ghost_never_stored",
        ),
    );
    let outcome = expect_fact_write(resp);

    assert!(!outcome.stored, "an ungrounded fact must never be stored");
    assert!(
        outcome.quarantined,
        "an ungrounded fact must be quarantined"
    );

    let facts = mem.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(
        facts.is_empty(),
        "no ungrounded fact may leak into semantic memory (anti-hallucination boundary)"
    );
}

// ── SEC-T2: empty / whitespace content is a hard quarantine even if grounded ─

#[test]
fn sec_t2_empty_content_is_quarantined_even_when_grounded() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let episode_id = mem
        .store_episode("some real episode content", "engineer-cycle", None)
        .expect("store_episode");

    for empty in ["", "   \t\n "] {
        let resp = dispatch(
            &mem as &dyn CognitiveMemoryOps,
            gated("bug-pattern", empty, &episode_id),
        );
        let outcome = expect_fact_write(resp);
        assert!(
            !outcome.stored && outcome.quarantined,
            "grounded-but-empty content must be quarantined (hard gate), content={empty:?}"
        );
    }

    let facts = mem.search_facts("bug-pattern", 10, 0.0).expect("search");
    assert!(facts.is_empty(), "no empty-content fact may be stored");
}

// ── SEC-T5: dedup — an equal-or-stronger prior is never clobbered ───────────

#[test]
fn sec_t5_equal_or_stronger_prior_is_not_clobbered() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let episode_id = mem
        .store_episode(
            "empty outcome list panicked the cycle",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");

    // First write stores the fact at the server-computed 0.9.
    let first = expect_fact_write(dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated(
            "bug-pattern",
            "empty outcome list panics cycle",
            &episode_id,
        ),
    ));
    assert!(first.stored, "the first grounded fact must be stored");

    // An identical restatement scores the same 0.9; the existing fact is
    // equal-or-stronger, so the gate must NOT downgrade/duplicate it.
    let second = expect_fact_write(dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated(
            "bug-pattern",
            "empty outcome list panics cycle",
            &episode_id,
        ),
    ));
    assert!(
        !second.stored,
        "a duplicate of an equal-or-stronger existing fact must not be re-stored"
    );

    let facts: Vec<_> = mem
        .search_facts("bug-pattern", 10, 0.0)
        .expect("search")
        .into_iter()
        .filter(|f| f.content == "empty outcome list panics cycle")
        .collect();
    assert_eq!(
        facts.len(),
        1,
        "dedup must keep exactly one copy of the fact, not accumulate restatements"
    );
}

// ── Off-spec concept still stores when grounded + well-formed (concept is a
//    nudge, not a gate) ───────────────────────────────────────────────────────

#[test]
fn grounded_offspec_concept_still_stores() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let episode_id = mem
        .store_episode("observed a novel thing", "engineer-cycle", None)
        .expect("store_episode");

    let outcome = expect_fact_write(dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated("observation", "three or more words here", &episode_id),
    ));
    assert!(
        outcome.stored,
        "a grounded, well-worded fact clears the gate at 0.8 even with an off-spec concept"
    );
    assert!(
        (outcome.confidence - 0.8).abs() < 1e-9,
        "off-spec concept loses only the 0.1 concept weight; got {}",
        outcome.confidence
    );
}

// ── The gate never emits a legacy "parse" response and never trusts the client
//    confidence to promote an ungrounded fact ────────────────────────────────

#[test]
fn client_confidence_cannot_promote_across_the_gate() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    // Grounded but 1-word content: server score = 0.5 grounding + 0.15 short +
    // 0.1 concept = 0.75, independent of the client's 0.99. We assert the
    // reported confidence is the server's, proving the hint is discarded.
    let episode_id = mem
        .store_episode("short", "engineer-cycle", None)
        .expect("store_episode");
    let outcome = expect_fact_write(dispatch(
        &mem as &dyn CognitiveMemoryOps,
        gated("pr-pattern", "squash", &episode_id),
    ));
    assert!(outcome.stored);
    assert!(
        (outcome.confidence - 0.75).abs() < 1e-9,
        "server confidence for grounded 1-word known-concept must be 0.75, not the client 0.99; got {}",
        outcome.confidence
    );
}
