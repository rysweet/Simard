//! Outside-in (black-box) integration test for semantic fact recall in OODA
//! preparation. Originating issue: #2302 ("facts always zero" — semantic memory
//! never recalled in OODA prep).
//!
//! This file lives in `tests/` so it compiles as an EXTERNAL consumer of the
//! `simard` crate: it touches only the public API surface, exactly as the
//! daemon (or any downstream user) does. It does NOT reach into private
//! internals or `#[cfg(test)]`-only helpers.
//!
//! It reproduces the real daemon path that logs `prepared context (N facts, …)`
//! and asserts N > 0 (the behaviour #2302 fixed). Each scenario is a discrete
//! `#[test]` so `cargo test` runs and gates them.

use std::collections::HashSet;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::goals::{GoalRecord, GoalStatus};
use simard::preparation_memory_operations_with_active_slugs;
use simard::session::{SessionId, SessionPhase};

const GOAL_STORE_FACT_CONCEPT: &str = "goal-store:record";

fn session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

/// The most basic user-facing behaviour #2302 fixes: a realistic multi-word
/// objective whose full text is NOT a verbatim substring of any stored fact
/// must still recall a fact that shares one keyword. Pre-fix this returned 0
/// rows (the whole-string CONTAINS defect).
#[test]
fn keyword_recall_via_multiword_objective() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    // Content shares "auth"/"module" with the objective but never the full
    // objective string verbatim.
    mem.store_fact(
        "ci-pattern",
        "the auth module integration tests are flaky under heavy load",
        0.8,
        &[],
        "episode-1",
    )
    .unwrap();

    let objective = "investigate the failing auth module CI on the daemon";
    let facts = mem.search_facts(objective, 10, 0.0).unwrap();

    assert!(
        facts.iter().any(|f| f.concept == "ci-pattern"),
        "objective {objective:?} must recall the keyword-sharing fact \
         (recalled {} fact(s) — 0 is the #2302 defect)",
        facts.len()
    );
}

/// The full OODA preparation path — the integration point that emits
/// `prepared context (N facts, …)`. Exercises:
///
/// - a compound objective split on "; " into fragments,
/// - a learned keyword fact recalled via tokenized per-fragment search,
/// - a long decoy fact sharing only ONE keyword,
/// - an active `goal-store:record` fact surfaced via the exact-concept load
///   path and kept by the active-slug filter.
///
/// Asserts `PreparedContext.relevant_facts` is non-empty AND carries both the
/// keyword fact and the goal fact.
#[test]
fn preparation_path_recalls_keyword_decoy_and_goal_facts() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    mem.store_fact(
        "ci-pattern",
        "the auth module integration tests are flaky under heavy load",
        0.8,
        &[],
        "episode-1",
    )
    .unwrap();

    // Long decoy that shares exactly one keyword ("database") with a fragment;
    // whole-string CONTAINS could never have matched it.
    mem.store_fact(
        "perf-note",
        "the database connection pool saturates when many sessions reconnect at once",
        0.7,
        &[],
        "episode-2",
    )
    .unwrap();

    let goal = GoalRecord {
        slug: "fix-auth".to_string(),
        title: "Stabilize auth module tests".to_string(),
        rationale: "flaky CI blocks merges".to_string(),
        status: GoalStatus::Active,
        priority: 1,
        owner_identity: "simard".to_string(),
        source_session_id: session_id(),
        updated_in: SessionPhase::Reflection,
        evidence: Vec::new(),
        labels: Vec::new(),
        wip_refs: Vec::new(),
    };
    mem.store_fact(
        GOAL_STORE_FACT_CONCEPT,
        &serde_json::to_string(&goal).unwrap(),
        1.0,
        &[],
        "goal-store",
    )
    .unwrap();

    // Compound, realistic objective (mirrors how OODA joins goal fragments with
    // "; "). None of these fragments is a verbatim substring of any stored fact.
    let objective =
        "investigate the failing auth module CI; reduce database connection churn under load";
    let active: HashSet<&str> = ["fix-auth"].into_iter().collect();

    let ctx = preparation_memory_operations_with_active_slugs(
        objective,
        &session_id(),
        &mem,
        Some(&active),
    )
    .unwrap();

    let concepts: Vec<&str> = ctx
        .relevant_facts
        .iter()
        .map(|f| f.concept.as_str())
        .collect();

    assert!(
        !ctx.relevant_facts.is_empty(),
        "prepared context must carry facts (0 is the #2302 defect); concepts={concepts:?}"
    );
    assert!(
        concepts.contains(&"ci-pattern"),
        "keyword fact must be recalled; concepts={concepts:?}"
    );
    assert!(
        concepts.contains(&"perf-note"),
        "second-fragment keyword decoy must be recalled; concepts={concepts:?}"
    );
    assert!(
        concepts.contains(&GOAL_STORE_FACT_CONCEPT),
        "active goal fact must be surfaced and kept by the active-slug filter; \
         concepts={concepts:?}"
    );
}
