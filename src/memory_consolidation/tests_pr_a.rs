//! TDD (RED) tests for PR-A: preparation-phase memory filters.
//!
//! These tests pin the contract documented in
//! `docs/reference/cognitive-memory-preparation-filters.md` for PR-A
//! (issue #2281, problems 1 + 5). They are written **before** the
//! production code change and are expected to FAIL until PR-A lands.
//!
//! Two filters are exercised:
//!
//! 1. **Drop `goal-board:snapshot` facts.** The live goal board is
//!    already injected into the prompt by `advance.rs`, so surfacing
//!    snapshot revisions in `PreparedContext.relevant_facts` is pure
//!    redundancy.
//! 2. **Drop stale `goal-store:record` facts.** A record is stale
//!    when its slug is not present in the live goal-board (active or
//!    backlog).
//!
//! ## How these compile against pre-PR-A code
//!
//! The tests must compile against the current 3-argument
//! `preparation_memory_operations(objective, session_id, bridge)`
//! signature. They route through a thin shim
//! [`prep_with_active_slugs`] that today simply calls the 3-arg
//! function and ignores the extra argument. When PR-A switches the
//! production signature to take `&HashSet<&str>` (or whatever the
//! equivalent collection ends up being), the shim is updated to
//! forward the slugs and these tests will start passing.
//!
//! Both flavours of the shim are kept in this module so the diff for
//! PR-A is a single-file change in the production code path. The
//! tests themselves do not change.

use super::*;
use crate::bridge_subprocess::InMemoryBridgeTransport;
use crate::memory_bridge::CognitiveMemoryBridge;
use crate::session::SessionId;
use serde_json::json;
use std::collections::HashSet;

fn test_session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

/// Shim that today calls the existing 3-arg `preparation_memory_operations`.
///
/// Post-PR-A, edit this shim to forward `active_slugs` into the new
/// 4-argument production signature. The tests below already pass the
/// slugs through it.
fn prep_with_active_slugs(
    objective: &str,
    session_id: &SessionId,
    bridge: &dyn crate::cognitive_memory::CognitiveMemoryOps,
    active_slugs: &HashSet<&str>,
) -> crate::error::SimardResult<PreparedContext> {
    preparation_memory_operations_with_active_slugs(
        objective,
        session_id,
        bridge,
        Some(active_slugs),
    )
}

// ───────────────────────────────────────────────────────────────────────────
// Bridge fixtures
// ───────────────────────────────────────────────────────────────────────────

/// Bridge that returns five `goal-board:snapshot` revisions plus one
/// unrelated `bug-pattern` fact when queried by the objective text or
/// by the goal-store concept. PR-A filter 1 should drop all five
/// snapshots.
fn snapshot_revisions_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("snapshot-revs", |method, params| match method {
        "memory.search_facts" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            // Both the per-fragment objective search AND the
            // goal-store search return snapshot revisions in this
            // fixture, because in production the snapshots have been
            // observed leaking into per-fragment matches too.
            let mut facts: Vec<serde_json::Value> = (0..5)
                .map(|i| {
                    json!({
                        "node_id": format!("snap_{:04}", i),
                        "concept": "goal-board:snapshot",
                        "content": format!("{{\"revision\": {}, \"active\": []}}", i),
                        "confidence": 1.0,
                        "source_id": "goal-curator",
                        "tags": ["goal-board"],
                    })
                })
                .collect();
            // Add one truly useful fact to verify other concepts are
            // not impacted by the snapshot filter.
            if query != "goal-store:record" {
                facts.push(json!({
                    "node_id": "bug_001",
                    "concept": "bug-pattern",
                    "content": "panic when cycle.rs receives empty outcome list",
                    "confidence": 0.8,
                    "source_id": "distill:epi_x",
                    "tags": ["bug"],
                }));
            }
            Ok(json!({ "facts": facts }))
        }
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Bridge that returns three `goal-store:record` facts: two with slugs
/// the caller will pass in `active_slugs` ("alpha", "beta") and one
/// with a stale slug ("ghost-of-tdd") that should be filtered out.
fn mixed_goal_store_bridge() -> CognitiveMemoryBridge {
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::SessionPhase;

    let make_record = |slug: &str, title: &str| GoalRecord {
        slug: slug.to_string(),
        title: title.to_string(),
        rationale: "rationale".to_string(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "test".to_string(),
        source_session_id: SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef")
            .unwrap(),
        updated_in: SessionPhase::Persistence,
        evidence: Vec::new(),
    };

    let alpha = make_record("alpha", "Alpha");
    let beta = make_record("beta", "Beta");
    let stale = make_record("ghost-of-tdd", "Ghost-of-TDD (stale)");

    let transport =
        InMemoryBridgeTransport::new("mixed-goal-store", move |method, params| match method {
            "memory.search_facts" => {
                let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
                if query == "goal-store:record" {
                    Ok(json!({
                        "facts": [
                            {
                                "node_id": "n_alpha",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&alpha).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"],
                            },
                            {
                                "node_id": "n_beta",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&beta).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"],
                            },
                            {
                                "node_id": "n_stale",
                                "concept": "goal-store:record",
                                "content": serde_json::to_string(&stale).unwrap(),
                                "confidence": 1.0,
                                "source_id": "goal-store",
                                "tags": ["goal-store"],
                            },
                        ]
                    }))
                } else {
                    Ok(json!({"facts": []}))
                }
            }
            "memory.check_triggers" => Ok(json!({"prospectives": []})),
            "memory.recall_procedure" => Ok(json!({"procedures": []})),
            "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
            "memory.push_working" => Ok(json!({"id": "wrk_1"})),
            _ => Err(crate::bridge::BridgeErrorPayload {
                code: -32601,
                message: format!("unknown: {method}"),
            }),
        });
    CognitiveMemoryBridge::new(Box::new(transport))
}

/// Bridge that returns a `lesson-learned` and a `pr-pattern` fact —
/// neither of which is a `goal-store:record` or `goal-board:snapshot`.
/// Both filters should leave these untouched.
fn diverse_concepts_bridge() -> CognitiveMemoryBridge {
    let transport = InMemoryBridgeTransport::new("diverse", |method, _params| match method {
        "memory.search_facts" => Ok(json!({
            "facts": [
                {
                    "node_id": "lsn_001",
                    "concept": "lesson-learned",
                    "content": "prefer fixture builders over inline JSON",
                    "confidence": 0.75,
                    "source_id": "distill:epi_a",
                    "tags": [],
                },
                {
                    "node_id": "prp_001",
                    "concept": "pr-pattern",
                    "content": "ci green + scope clean + docs touched + merge-ready skill",
                    "confidence": 0.85,
                    "source_id": "distill:epi_b",
                    "tags": [],
                },
            ]
        })),
        "memory.check_triggers" => Ok(json!({"prospectives": []})),
        "memory.recall_procedure" => Ok(json!({"procedures": []})),
        "memory.search_episodes_by_keywords" => Ok(json!({"episodes": []})),
        "memory.push_working" => Ok(json!({"id": "wrk_1"})),
        _ => Err(crate::bridge::BridgeErrorPayload {
            code: -32601,
            message: format!("unknown: {method}"),
        }),
    });
    CognitiveMemoryBridge::new(Box::new(transport))
}

// ───────────────────────────────────────────────────────────────────────────
// Tests
// ───────────────────────────────────────────────────────────────────────────

/// Filter 1: every `goal-board:snapshot` fact must be dropped from
/// `relevant_facts`. The unrelated `bug-pattern` fact must survive.
///
/// **Expected to FAIL pre-PR-A** because the production code today
/// returns all 5 snapshot revisions in the prepared context.
#[test]
fn preparation_drops_goal_board_snapshot_revisions() {
    let bridge = snapshot_revisions_bridge();
    let active: HashSet<&str> = HashSet::new();
    let ctx = prep_with_active_slugs("unrelated objective", &test_session_id(), &bridge, &active)
        .unwrap();

    let snapshots: Vec<_> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "goal-board:snapshot")
        .collect();
    assert_eq!(
        snapshots.len(),
        0,
        "goal-board:snapshot facts must be filtered from PreparedContext.relevant_facts, \
         found {}: {:?}",
        snapshots.len(),
        snapshots
            .iter()
            .map(|f| f.node_id.clone())
            .collect::<Vec<_>>()
    );

    // The unrelated useful fact must still be present.
    let bug_patterns: Vec<_> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "bug-pattern")
        .collect();
    assert!(
        !bug_patterns.is_empty(),
        "non-snapshot, non-goal-store facts must pass through the snapshot filter unchanged"
    );
}

/// Filter 2: a `goal-store:record` fact whose slug is NOT in
/// `active_slugs` must be dropped. Active slugs survive.
///
/// **Expected to FAIL pre-PR-A** for two reasons:
/// (a) the production function does not yet take an `active_slugs`
/// parameter, so the shim currently ignores it; and
/// (b) even if it took the argument, the stale-slug filter does
/// not exist yet — the existing slug-dedup pass would still keep
/// `ghost-of-tdd` because it is the only revision of its slug.
#[test]
fn preparation_drops_stale_goal_store_records() {
    let bridge = mixed_goal_store_bridge();
    let mut active: HashSet<&str> = HashSet::new();
    active.insert("alpha");
    active.insert("beta");

    let ctx = prep_with_active_slugs("unrelated objective", &test_session_id(), &bridge, &active)
        .unwrap();

    let slugs: Vec<String> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "goal-store:record")
        .filter_map(|f| {
            serde_json::from_str::<crate::goals::GoalRecord>(&f.content)
                .ok()
                .map(|r| r.slug)
        })
        .collect();

    assert!(
        !slugs.iter().any(|s| s == "ghost-of-tdd"),
        "stale goal-store record (slug not in active_slugs) must be filtered out; \
         saw slugs: {slugs:?}"
    );
}

/// Regression guard for filter 2: `goal-store:record` facts whose
/// slug IS in `active_slugs` must remain in the prepared context.
///
/// **Expected to FAIL pre-PR-A** because (a) the active_slugs
/// parameter is not yet plumbed through, and (b) the test also
/// asserts the absence of the stale slug, which is not yet enforced.
#[test]
fn preparation_keeps_active_goal_store_records() {
    let bridge = mixed_goal_store_bridge();
    let mut active: HashSet<&str> = HashSet::new();
    active.insert("alpha");
    active.insert("beta");

    let ctx = prep_with_active_slugs("unrelated objective", &test_session_id(), &bridge, &active)
        .unwrap();

    let slugs: std::collections::HashSet<String> = ctx
        .relevant_facts
        .iter()
        .filter(|f| f.concept == "goal-store:record")
        .filter_map(|f| {
            serde_json::from_str::<crate::goals::GoalRecord>(&f.content)
                .ok()
                .map(|r| r.slug)
        })
        .collect();

    assert!(
        slugs.contains("alpha"),
        "active slug 'alpha' must be retained; saw slugs: {slugs:?}"
    );
    assert!(
        slugs.contains("beta"),
        "active slug 'beta' must be retained; saw slugs: {slugs:?}"
    );
    assert!(
        !slugs.contains("ghost-of-tdd"),
        "stale slug must be filtered out (companion assertion to drop test)"
    );
}

/// Filter contract: neither filter must drop facts whose concept is
/// neither `goal-board:snapshot` nor `goal-store:record`. Facts like
/// `pr-pattern`, `bug-pattern`, `lesson-learned` (the three PR-B
/// distillation concepts) must always survive.
///
/// **Expected to FAIL pre-PR-A** only if the filters are mis-scoped
/// to also affect other concepts. This is a regression guard so the
/// PR-A author cannot accidentally widen the filter.
#[test]
fn preparation_does_not_filter_other_concepts() {
    let bridge = diverse_concepts_bridge();
    let active: HashSet<&str> = HashSet::new();
    let ctx = prep_with_active_slugs("any objective at all", &test_session_id(), &bridge, &active)
        .unwrap();

    let concepts: std::collections::HashSet<String> = ctx
        .relevant_facts
        .iter()
        .map(|f| f.concept.clone())
        .collect();

    assert!(
        concepts.contains("lesson-learned"),
        "lesson-learned fact must pass through filters; concepts: {concepts:?}"
    );
    assert!(
        concepts.contains("pr-pattern"),
        "pr-pattern fact must pass through filters; concepts: {concepts:?}"
    );
}

/// **TDD red → green (issue #2302): the "facts always zero" defect.**
///
/// End-to-end through the real `LibraryCognitiveMemory` (not a mock
/// bridge) so the actual `search_facts` body is exercised. Stores a
/// keyword-bearing learned fact and a valid `goal-store:record` fact,
/// then prepares with a realistic multi-word objective and the live
/// active slug set. Both facts must surface in `PreparedContext`.
///
/// **Discriminating assertion:** the `ci-pattern` keyword fact. Before
/// the fix the per-fragment recall passes the whole 38-char objective to
/// `search_facts` as one `CONTAINS` needle, so no fact substring matches
/// and `ci-pattern` never lands in `relevant_facts` — the prepared
/// context shows zero learned facts on every cycle. After tokenization
/// the shared `auth`/`module` keywords recall it.
///
/// (`relevant_facts.len() > 0` and the `goal-store:record` assertion
/// pass even pre-fix because the goal-fact load uses the exact-concept
/// path, so they are not the red signal — they guard that the goal-load
/// path keeps working alongside the new keyword recall.)
#[test]
fn preparation_recalls_keyword_and_goal_facts() {
    use crate::goals::{GoalRecord, GoalStatus};
    use crate::session::SessionPhase;

    let mem = crate::cognitive_memory::LibraryCognitiveMemory::in_memory().unwrap();

    // A learned fact whose CONTENT shares the keywords "auth"/"module"
    // with the objective but does NOT contain the full objective verbatim.
    mem.store_fact(
        "ci-pattern",
        "the auth module integration tests are flaky under heavy load",
        0.8,
        &[],
        "episode-1",
    )
    .unwrap();

    // A goal record filed under the goal-store:record concept. Its slug
    // is in the live active set, so the stale-slug filter keeps it.
    let goal = GoalRecord {
        slug: "fix-auth".to_string(),
        title: "Stabilize auth module tests".to_string(),
        rationale: "flaky CI blocks merges".to_string(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "simard".to_string(),
        source_session_id: test_session_id(),
        updated_in: SessionPhase::Reflection,
        evidence: Vec::new(),
    };
    mem.store_fact(
        crate::goals::GOAL_STORE_FACT_CONCEPT,
        &serde_json::to_string(&goal).unwrap(),
        1.0,
        &[],
        "goal-store",
    )
    .unwrap();

    let objective = "investigate the failing auth module CI";
    let active: HashSet<&str> = ["fix-auth"].into_iter().collect();

    let ctx = prep_with_active_slugs(objective, &test_session_id(), &mem, &active).unwrap();

    let concepts: Vec<String> = ctx
        .relevant_facts
        .iter()
        .map(|f| f.concept.clone())
        .collect();

    assert!(
        !ctx.relevant_facts.is_empty(),
        "prepared context must contain facts (the 'facts always zero' \
         defect); concepts: {concepts:?}"
    );
    assert!(
        ctx.relevant_facts.iter().any(|f| f.concept == "ci-pattern"),
        "keyword-bearing fact must surface via tokenized per-fragment \
         recall (was always missing pre-#2302); concepts: {concepts:?}"
    );
    assert!(
        ctx.relevant_facts
            .iter()
            .any(|f| f.concept == crate::goals::GOAL_STORE_FACT_CONCEPT),
        "active goal-store:record fact must surface alongside the keyword \
         fact; concepts: {concepts:?}"
    );
}
