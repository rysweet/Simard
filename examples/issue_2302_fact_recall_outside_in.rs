//! Outside-in (black-box) verification harness for issue #2302:
//! "facts always zero" — semantic memory never recalls in OODA prep.
//!
//! This binary lives in `examples/` so it compiles as an EXTERNAL consumer
//! of the `simard` crate: it can only touch the public API surface, exactly
//! as the daemon (or any downstream user) does. It does NOT reach into
//! private internals or test helpers.
//!
//! It reproduces the real daemon path that logs
//! `prepared context (N facts, …)` and asserts N > 0 after the fix.
//!
//! Exit code 0 = all scenarios PASS, non-zero = at least one FAILED.

use std::collections::HashSet;

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::goals::{GoalRecord, GoalStatus};
use simard::preparation_memory_operations_with_active_slugs;
use simard::session::{SessionId, SessionPhase};

const GOAL_STORE_FACT_CONCEPT: &str = "goal-store:record";

fn session_id() -> SessionId {
    SessionId::parse("session-01234567-89ab-cdef-0123-456789abcdef").unwrap()
}

/// Scenario 1 (simple): the most basic user-facing behaviour the PR fixes.
/// A realistic multi-word objective whose full text is NOT a verbatim
/// substring of any stored fact must still recall a fact that shares one
/// keyword. Pre-fix this returned 0 rows (the whole-string CONTAINS).
fn scenario_1_keyword_recall() -> bool {
    println!("── Scenario 1: keyword fact recall via multi-word objective ──");
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    // Content shares "auth"/"module" with the objective but never the
    // full objective string verbatim.
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

    println!("  objective : {objective:?}");
    println!("  recalled  : {} fact(s)", facts.len());
    for f in &facts {
        println!("    - concept={:?}", f.concept);
    }

    let ok = facts.iter().any(|f| f.concept == "ci-pattern");
    println!(
        "  RESULT    : {}\n",
        if ok {
            "PASS"
        } else {
            "FAIL (0 facts — the #2302 defect)"
        }
    );
    ok
}

/// Scenario 2 (complex): the full OODA preparation path, the integration
/// point that emits `prepared context (N facts, …)`. Exercises:
///
/// - a compound objective split on "; " into fragments,
/// - a learned keyword fact recalled via tokenized per-fragment search,
/// - a long decoy fact sharing only ONE keyword,
/// - an active `goal-store:record` fact surfaced via the exact-concept
///   load path and kept by the active-slug filter.
///
/// Asserts PreparedContext.relevant_facts is non-empty AND carries both the
/// keyword fact and the goal fact.
fn scenario_2_preparation_path() -> bool {
    println!("── Scenario 2: full OODA preparation path (compound objective) ──");
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    mem.store_fact(
        "ci-pattern",
        "the auth module integration tests are flaky under heavy load",
        0.8,
        &[],
        "episode-1",
    )
    .unwrap();

    // Long decoy that shares exactly one keyword ("database") with a
    // fragment; whole-string CONTAINS could never have matched it.
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
    };
    mem.store_fact(
        GOAL_STORE_FACT_CONCEPT,
        &serde_json::to_string(&goal).unwrap(),
        1.0,
        &[],
        "goal-store",
    )
    .unwrap();

    // Compound, realistic objective (mirrors how OODA joins goal fragments
    // with "; "). None of these fragments is a verbatim substring of any
    // stored fact.
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

    // Mirror the daemon's log line so the before/after is unmistakable.
    println!(
        "  prepared context ({} facts) concepts={:?}",
        ctx.relevant_facts.len(),
        concepts
    );

    let has_keyword = concepts.contains(&"ci-pattern");
    let has_decoy = concepts.contains(&"perf-note");
    let has_goal = concepts.contains(&GOAL_STORE_FACT_CONCEPT);
    let non_empty = !ctx.relevant_facts.is_empty();

    println!("  facts > 0            : {non_empty}");
    println!("  keyword fact present : {has_keyword}");
    println!("  2nd-fragment recall  : {has_decoy}");
    println!("  goal fact present    : {has_goal}");

    let ok = non_empty && has_keyword && has_decoy && has_goal;
    println!("  RESULT    : {}\n", if ok { "PASS" } else { "FAIL" });
    ok
}

fn main() {
    println!("=== issue #2302 outside-in fact-recall verification ===\n");
    let s1 = scenario_1_keyword_recall();
    let s2 = scenario_2_preparation_path();

    println!("=== SUMMARY ===");
    println!(
        "Scenario 1 (simple)  : {}",
        if s1 { "PASS" } else { "FAIL" }
    );
    println!(
        "Scenario 2 (complex) : {}",
        if s2 { "PASS" } else { "FAIL" }
    );

    if s1 && s2 {
        println!("\nALL SCENARIOS PASSED");
        std::process::exit(0);
    } else {
        eprintln!("\nONE OR MORE SCENARIOS FAILED");
        std::process::exit(1);
    }
}
