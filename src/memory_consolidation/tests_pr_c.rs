//! TDD (RED) tests for PR-C: episodic recall in preparation.
//!
//! Covers the contract documented in
//! `docs/reference/cognitive-memory-episodic-recall.md`. Expected to
//! FAIL until PR-C lands the following changes:
//!
//! * `PreparedContext.episodic_recall: Vec<CognitiveEpisode>` field
//!   (struct in `src/memory_consolidation/mod.rs`).
//! * `CognitiveMemoryOps::search_episodes_by_keywords(keywords, limit)`
//!   trait method with default no-op impl.
//! * Tokenizer + self-session filter logic inside
//!   `preparation_memory_operations` that produces non-empty
//!   `episodic_recall` for objectives containing trigger keywords.
//!
//! ## How these compile against pre-PR-C code
//!
//! The tests route through a thin shim
//! [`prep_returning_recall`] that adapts whatever the production
//! signature evolves to. The shim's job is to return a vector of
//! recalled episodes for assertion. Today, since `episodic_recall`
//! does not exist on `PreparedContext`, the shim cannot extract it
//! and the tests fail to compile — that is the intended RED signal.
//!
//! PR-C's first commit adds the `episodic_recall` field with a
//! default empty initializer and edits this shim to read it; tests
//! then compile but fail at assertions until the recall logic lands.

use super::*;
use crate::memory_client::CognitiveMemoryClient;
use crate::memory_cognitive::CognitiveEpisode;
use crate::rpc_transport::InMemoryRpcTransport;
use crate::session::SessionId;
use serde_json::json;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

/// Shim that calls preparation and surfaces the future
/// `episodic_recall` field as a return value. Pre-PR-C the field
/// does not exist; this shim's body is the single edit PR-C performs
/// to make these tests compile.
fn prep_returning_recall(
    objective: &str,
    session_id: &SessionId,
    memory: &dyn crate::cognitive_memory::CognitiveMemoryOps,
) -> crate::error::SimardResult<(PreparedContext, Vec<CognitiveEpisode>)> {
    let ctx = preparation_memory_operations(objective, session_id, memory)?;
    // Pre-PR-C: PreparedContext has no `episodic_recall` field, so
    // this clone-and-return surfaces an empty vec. Post-PR-C: edit
    // the next line to `ctx.episodic_recall.clone()` and the
    // assertions below take effect.
    let recall = ctx.episodic_recall.clone();
    Ok((ctx, recall))
}

// ───────────────────────────────────────────────────────────────────────────
// Client fixtures
// ───────────────────────────────────────────────────────────────────────────

/// Client that returns three episodes via the (future)
/// `memory.search_episodes_by_keywords` method:
///
/// * `epi_a` (label `goal-curator`, contains "merge")
/// * `epi_b` (label `distill:epi_xx`, contains "merge")
/// * `epi_c` (label `session-12345`, contains "merge")  ← must be filtered
fn keyword_recall_memory() -> CognitiveMemoryClient {
    let transport = InMemoryRpcTransport::new("kw-recall", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        "memory.search_episodes_by_keywords" => Ok(json!({
            "episodes": [
                {
                    "node_id": "epi_a",
                    "content": "merged PR #2278 with squashed CI fix",
                    "source_label": "goal-curator",
                    "temporal_index": 3,
                    "compressed": false,
                },
                {
                    "node_id": "epi_b",
                    "content": "pr-merge pattern: enable auto-merge before final review",
                    "source_label": "distill:epi_xx",
                    "temporal_index": 2,
                    "compressed": false,
                },
                {
                    "node_id": "epi_c",
                    "content": "merge PR #2281 starting now",
                    "source_label": "session-12345",
                    "temporal_index": 1,
                    "compressed": false,
                },
            ]
        })),
        _ => Err(crate::rpc::RpcErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryClient::new(Box::new(transport))
}

/// Client whose `search_episodes_by_keywords` MUST NOT be called.
/// Used by the "no tokens" edge case: a short or stopword-only
/// objective must short-circuit before issuing the trait call.
fn no_recall_memory() -> CognitiveMemoryClient {
    let transport = InMemoryRpcTransport::new("no-recall", |method, _params| match method {
        "memory.search_facts" => Ok(json!({"facts": []})),
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        "memory.search_episodes_by_keywords" => {
            panic!("search_episodes_by_keywords must not be called when no tokens are derived")
        }
        _ => Err(crate::rpc::RpcErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryClient::new(Box::new(transport))
}

/// Client that captures the keyword list it receives via
/// `search_episodes_by_keywords` for tokenizer assertions.
fn capturing_recall_memory() -> (
    CognitiveMemoryClient,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let cap = captured.clone();
    let transport =
        InMemoryRpcTransport::new("capture-recall", move |method, params| match method {
            "memory.search_facts" => Ok(json!({"facts": []})),
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            "memory.search_episodes_by_keywords" => {
                if let Some(arr) = params.get("keywords").and_then(|v| v.as_array()) {
                    let mut sink = cap.lock().unwrap();
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            sink.push(s.to_string());
                        }
                    }
                }
                Ok(json!({"episodes": []}))
            }
            _ => Err(crate::rpc::RpcErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
    (CognitiveMemoryClient::new(Box::new(transport)), captured)
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

/// Happy path: objective with trigger keywords + matching episodes →
/// `PreparedContext.episodic_recall` is non-empty.
#[test]
fn preparation_injects_episodic_recall() {
    let memory = keyword_recall_memory();
    let (_, recall) = prep_returning_recall("merge PR #2281", &test_session_id(), &memory).unwrap();

    assert!(
        !recall.is_empty(),
        "episodic_recall must be populated when episode store has matching keywords"
    );
    // At least the two non-session episodes survive the filter.
    let ids: std::collections::HashSet<&str> = recall.iter().map(|e| e.node_id.as_str()).collect();
    assert!(
        ids.contains("epi_a"),
        "goal-curator episode 'epi_a' must be present; got: {ids:?}"
    );
    assert!(
        ids.contains("epi_b"),
        "distill episode 'epi_b' must be present; got: {ids:?}"
    );
}

/// Self-session noise filter: episodes whose `source_label` starts
/// with `session-` MUST be excluded from `episodic_recall`. They
/// were written by the very session loop now preparing — surfacing
/// them creates a self-reinforcing loop.
#[test]
fn preparation_excludes_self_session_noise() {
    let memory = keyword_recall_memory();
    let (_, recall) = prep_returning_recall("merge PR #2281", &test_session_id(), &memory).unwrap();

    for ep in &recall {
        assert!(
            !ep.source_label.starts_with("session-"),
            "episode '{}' has source_label '{}' which starts with 'session-' \
             and must be filtered out",
            ep.node_id,
            ep.source_label,
        );
    }
}

/// Tokenizer rules: objective text is lowercased, split on
/// non-alphanumeric, tokens < 3 chars dropped, stopwords dropped,
/// deduped. Test verifies the keyword list reaching the trait method
/// satisfies all four rules.
///
/// Objective: "the merge PR #2281 and PR review with cargo CI"
/// Expected tokens (after rules): {merge, 2281, review, cargo}
///   - "the", "and", "with" → stopword drops
///   - "pr" → < 3 chars drops (3-char minimum)
///   - "ci" → < 3 chars drops
///   - second "pr" → dedup (was already dropped)
///   - "#" stripped from "#2281" → "2281" kept (digits count as
///     alphanumeric and length >= 3)
#[test]
fn preparation_tokenizes_and_strips_stopwords() {
    let (memory, captured) = capturing_recall_memory();
    let _ = prep_returning_recall(
        "the merge PR #2281 and PR review with cargo CI",
        &test_session_id(),
        &memory,
    )
    .unwrap();

    let tokens: std::collections::HashSet<String> =
        captured.lock().unwrap().iter().cloned().collect();

    assert!(
        tokens.contains("merge"),
        "tokenizer must keep 'merge'; got: {tokens:?}"
    );
    assert!(
        tokens.contains("2281"),
        "tokenizer must keep '2281' (digits, len 4); got: {tokens:?}"
    );
    assert!(
        tokens.contains("review"),
        "tokenizer must keep 'review'; got: {tokens:?}"
    );
    assert!(
        tokens.contains("cargo"),
        "tokenizer must keep 'cargo'; got: {tokens:?}"
    );

    assert!(
        !tokens.contains("the"),
        "stopword 'the' must be dropped; got: {tokens:?}"
    );
    assert!(
        !tokens.contains("and"),
        "stopword 'and' must be dropped; got: {tokens:?}"
    );
    assert!(
        !tokens.contains("with"),
        "stopword 'with' must be dropped; got: {tokens:?}"
    );
    assert!(
        !tokens.contains("pr"),
        "short token 'pr' (len 2) must be dropped; got: {tokens:?}"
    );
    assert!(
        !tokens.contains("ci"),
        "short token 'ci' (len 2) must be dropped; got: {tokens:?}"
    );

    // Dedup: "PR" appears twice in the objective; either both are
    // dropped (len < 3) so dedup is moot, or one survives — but never
    // two copies.
    let pr_count = captured
        .lock()
        .unwrap()
        .iter()
        .filter(|t| t.as_str() == "pr")
        .count();
    assert!(
        pr_count <= 1,
        "tokens must be deduplicated; 'pr' appeared {pr_count} times"
    );
}

/// Short / stopword-only objective produces NO tokens → trait method
/// is NOT called and `episodic_recall` stays empty. The `no_recall_memory`
/// panics if the trait method fires, proving the short-circuit.
#[test]
fn preparation_emits_no_recall_when_objective_yields_no_tokens() {
    let memory = no_recall_memory();
    let (_, recall) = prep_returning_recall("the and or", &test_session_id(), &memory).unwrap();

    assert!(
        recall.is_empty(),
        "stopword-only objective must yield empty episodic_recall; got {} entries",
        recall.len()
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Issue #2308 follow-up: end-to-end episode store + recall on the *library*
// backend (the sole backend), not a mock memory. Confirms that an episode
// whose content shares a keyword with the objective is actually persisted and
// recalled through the real preparation path, and that it is counted by
// `get_statistics().episodic_count` (the number `simard memory stats` reports).
// ───────────────────────────────────────────────────────────────────────────

/// Store one episode that shares the keyword "authentication" with the
/// objective, run the real preparation recall path against
/// `LibraryCognitiveMemory`, and assert it is recalled AND counted.
#[test]
fn library_episode_sharing_objective_keyword_is_recalled_and_counted_e2e() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

    let tmp = tempfile::tempdir().expect("tempdir");
    let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open library store");

    // source_label must NOT start with "session-" or preparation filters it as
    // self-session noise.
    mem.store_episode(
        "Resolved the authentication timeout in the login service",
        "engineer-cycle",
        None,
    )
    .expect("store_episode");

    let objective = "investigate authentication regressions";
    let (ctx, recall) =
        prep_returning_recall(objective, &test_session_id(), &mem).expect("preparation");

    assert!(
        !recall.is_empty(),
        "an episode sharing keyword 'authentication' with the objective must be \
         recalled end-to-end on the library backend; episodic_recall was empty"
    );
    assert!(
        ctx.episodic_recall
            .iter()
            .any(|e| e.content.to_lowercase().contains("authentication")),
        "recalled episode content must contain the shared keyword"
    );

    let stats = mem.get_statistics().expect("get_statistics");
    assert!(
        stats.episodic_count > 0,
        "the stored episode must be counted by get_statistics (this is what \
         `simard memory stats` reports); episodic_count = {}",
        stats.episodic_count
    );
}

/// A populated episode store can still recall nothing for an *unrelated*
/// objective — documenting that the live "0 episodes recalled" line is a
/// relevance outcome, not a storage failure. `episodic_count` stays > 0.
#[test]
fn library_unrelated_objective_recalls_zero_but_count_stays_positive_e2e() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

    let tmp = tempfile::tempdir().expect("tempdir");
    let mem = LibraryCognitiveMemory::open(tmp.path()).expect("open library store");

    mem.store_episode(
        "Provisioned the staging kubernetes cluster",
        "engineer-cycle",
        None,
    )
    .expect("store_episode");

    let objective = "refactor authentication tokens";
    let (_ctx, recall) =
        prep_returning_recall(objective, &test_session_id(), &mem).expect("preparation");

    assert!(
        recall.is_empty(),
        "no stored episode shares a keyword with this objective, so recall is empty"
    );

    let stats = mem.get_statistics().expect("get_statistics");
    assert!(
        stats.episodic_count > 0,
        "stored episode count must remain > 0 even when nothing is recalled; got {}",
        stats.episodic_count
    );
}
