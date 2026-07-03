//! Outside-in regression gate for the "prospective triggers never fire" defect
//! (issue #2300), re-validated against the library cognitive-memory backend
//! after the de-fork (#2308) deleted the native `src/cognitive_memory/ops.rs`.
//!
//! **Why this exists.** The de-fork added `tests/episodic_recall_outside_in.rs`
//! to gate the *episodic-recall* axis of the OODA preparation log line, but left
//! the *trigger* axis ungated once the native `ops.rs` #2300 tests were deleted.
//! This file restores user-observable coverage for triggers: it drives the same
//! preparation phase (`preparation_memory_operations`) an operator watches and
//! asserts the trigger count is non-zero. The defect made the prepared-context
//! line read:
//!
//! ```text
//! prepared context (0 facts, 0 triggers, 5 procedures, 0 episodes)
//! ```
//!
//! every OODA cycle. `preparation_memory_operations` passes the **raw objective**
//! to `adapter.check_triggers(objective)`; the library backend lowercases and
//! tokenises both sides and fires each matching prospective once. These tests
//! store a goal prospective exactly as `goals::cognitive_memory_store` does
//! (slug with dashes→spaces, `goal:` description prefix) and assert it surfaces
//! in `PreparedContext.triggered_prospectives`.
//!
//! Run observably with:
//! ```bash
//! cargo test --test prospective_triggers_outside_in -- --nocapture
//! ```

use simard::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
use simard::memory_consolidation::preparation_memory_operations;
use simard::session::SessionId;

/// Hermetic in-memory cognitive store backed by the library — the sole backend
/// after de-fork Phase 2b. No disk, no env, no `$HOME` leakage.
fn new_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory cognitive store should open")
}

/// Deterministic session id so the `session-` self-noise filter (episodic axis)
/// has a stable label and never interferes with the trigger assertions.
fn session() -> SessionId {
    SessionId::parse("session-00000000-0000-0000-0000-000000000001")
        .expect("literal session id should parse")
}

/// Store a goal prospective exactly as `goals::cognitive_memory_store` does for
/// an Active goal: description carries the `goal:` prefix and the
/// `trigger_condition` is the goal slug with dashes turned to spaces.
fn store_goal_prospective(mem: &LibraryCognitiveMemory, slug: &str) {
    let trigger_condition = slug.replace('-', " ");
    mem.store_prospective(
        &format!("goal: {trigger_condition}"),
        &trigger_condition,
        "resume work on the goal",
        1,
    )
    .expect("store_prospective should succeed");
}

/// Scenario 1 (simple) — the basic user-facing behaviour the de-fork must
/// preserve. Store one Active-goal prospective, then run the preparation phase
/// with a realistic mixed-case objective that mentions the goal phrase. Before
/// the fix this produced `0 triggers`; after it the prospective fires.
#[test]
fn preparation_fires_goal_prospective_for_realistic_objective() {
    let mem = new_mem();
    let session = session();

    // Goal slug `improve-cognitive-memory-persistence` → trigger condition
    // `improve cognitive memory persistence`.
    store_goal_prospective(&mem, "improve-cognitive-memory-persistence");

    // Objective as an operator would phrase it: free text, mixed case, the goal
    // phrase embedded in a longer sentence (not passed verbatim/lowercased).
    let objective =
        "Continue to Improve Cognitive Memory persistence and durable recall this OODA cycle";

    let ctx = preparation_memory_operations(objective, &session, &mem)
        .expect("preparation should succeed");

    eprintln!(
        "[scenario-1] triggered_prospectives = {} trigger(s)",
        ctx.triggered_prospectives.len()
    );

    assert!(
        ctx.triggered_prospectives
            .iter()
            .any(|p| p.description.starts_with("goal:")),
        "preparation must surface the goal prospective as a trigger (raw count > 0) — \
         this is the #2300 '0 triggers' defect; got {} trigger(s): {:?}",
        ctx.triggered_prospectives.len(),
        ctx.triggered_prospectives
            .iter()
            .map(|p| &p.description)
            .collect::<Vec<_>>()
    );
}

/// Scenario 2 (complex) — keyword-overlap firing plus a no-false-fire
/// regression, both through the same public preparation entry point.
///
/// The objective shares the trigger's keywords (fix / broken / authentication)
/// without ever stating the contiguous slug phrase, proving the library match is
/// tokenised keyword overlap (so realistic OODA paraphrases still fire). A
/// second, unrelated objective must NOT fire the trigger (no match-all).
#[test]
fn preparation_fires_on_keyword_overlap_and_not_on_unrelated_objective() {
    let mem = new_mem();
    let session = session();

    store_goal_prospective(&mem, "fix-broken-authentication");

    // Keywords present, but never the contiguous phrase "fix broken authentication".
    let related = "Authentication is broken in the login flow; we need to fix the regression today";
    let ctx =
        preparation_memory_operations(related, &session, &mem).expect("preparation should succeed");

    eprintln!(
        "[scenario-2] related objective fired {} trigger(s)",
        ctx.triggered_prospectives.len()
    );
    assert!(
        ctx.triggered_prospectives
            .iter()
            .any(|p| p.description.starts_with("goal:")),
        "the goal prospective must fire on keyword overlap even without the \
         contiguous slug phrase (issue #2300); got {:?}",
        ctx.triggered_prospectives
            .iter()
            .map(|p| &p.description)
            .collect::<Vec<_>>()
    );

    // No-false-fire regression: an unrelated objective on a FRESH store (the
    // library's `check_triggers` is a fire-once mutator, so re-probing the same
    // store would not re-fire) must surface zero goal triggers.
    let fresh = new_mem();
    store_goal_prospective(&fresh, "fix-broken-authentication");
    let unrelated = "Provision the staging database and rotate the TLS certificates";
    let none = preparation_memory_operations(unrelated, &session, &fresh)
        .expect("preparation should succeed for an unrelated objective");

    eprintln!(
        "[scenario-2] unrelated objective fired {} trigger(s)",
        none.triggered_prospectives.len()
    );
    assert!(
        !none
            .triggered_prospectives
            .iter()
            .any(|p| p.description.starts_with("goal:")),
        "an unrelated objective must not fire the goal prospective (no match-all \
         regression); got {:?}",
        none.triggered_prospectives
            .iter()
            .map(|p| &p.description)
            .collect::<Vec<_>>()
    );
}
