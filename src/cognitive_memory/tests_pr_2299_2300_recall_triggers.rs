//! Re-validation regression tests for the episodic-recall (#2299) and
//! prospective-trigger (#2300) defects against the **library** cognitive-memory
//! backend ([`LibraryCognitiveMemory`]) — the sole backend after de-fork
//! Phase 2b (#2308, issue #2307).
//!
//! ## Why these exist
//!
//! #2299 and #2300 were originally fixed inside Simard's now-deleted native
//! fork (`src/cognitive_memory/ops.rs`, PRs #2301 and #2303). The de-fork
//! (#2308) deleted that fork and made `amplihack-memory-lib` the sole backend,
//! so the original native fixes no longer protect the live path. These tests
//! re-prove both defects are resolved on the library backend and lock the
//! behaviour against regression.
//!
//! They drive the exact recall/trigger entry points the live OODA preparation
//! pass uses (`src/memory_consolidation/mod.rs`):
//!
//! * #2299 — `tokenize_objective(objective)` → `search_episodes_by_keywords`,
//!   asserting the **raw** recall count is `> 0` (the symptom logged "0 raw").
//! * #2300 — `store_prospective` (slug-derived `trigger_condition`) →
//!   `check_triggers(objective)` with realistic free-text objective, asserting
//!   the trigger fires (the symptom logged "0 triggers").
//!
//! These target `LibraryCognitiveMemory::in_memory()` directly so the live
//! backend is exercised hermetically, without the bridge/IPC/mock layers (whose
//! stub trait methods would mask the defect).

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};
use crate::memory_consolidation::tokenize_objective;

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory library DB should create")
}

// ─────────────────────────────────────────────────────────────────────────
// #2299 — episodic recall must return > 0 raw on the library backend
// ─────────────────────────────────────────────────────────────────────────

/// AC (#2299) — Store an episode whose content holds a known keyword in MIXED
/// case under a non-`session-` label, then recall it through the *live* path:
/// `tokenize_objective(objective)` (lowercases + drops stopwords) feeds
/// `search_episodes_by_keywords`. The raw recall count MUST be `> 0`.
///
/// The reported symptom was the preparation log forever printing
/// "0 episodes recalled (0 raw, 0 session-filtered)". The native defect was
/// case-sensitive substring matching: `tokenize_objective` lowercases tokens
/// while `store_episode` keeps verbatim case, so a case-sensitive `CONTAINS`
/// matched nothing. The library adapter's `search_episodes_by_keywords`
/// lowercases *both* sides; this test pins that on the library backend so the
/// fix survives the de-fork.
#[test]
fn episodic_recall_returns_nonzero_raw_for_objective_keyword() {
    let mem = test_mem();

    // Mixed-case content with the shared keywords in UPPER case; a non-`session-`
    // source label so the "raw" count is not confounded by downstream
    // session-filtering. UPPER-casing the shared keywords means the lowercased
    // objective tokens can only match via the adapter's case-folding — a
    // case-sensitive matcher would find nothing here.
    mem.store_episode(
        "Refactored the OODA Consolidation pass to harden DURABLE RECALL",
        "engineer-loop",
        None,
    )
    .expect("store_episode");

    // Realistic free-text objective; shares keywords ("durable", "recall") with
    // the stored episode but in different case and surrounded by other words.
    let objective = "Improve cognitive memory persistence so durable recall survives restarts";
    let tokens = tokenize_objective(objective);
    assert!(
        tokens.iter().any(|t| t == "durable" || t == "recall"),
        "sanity: tokenizer must yield the shared keyword(s); got {tokens:?}"
    );

    let raw = mem
        .search_episodes_by_keywords(&tokens, 5)
        .expect("search_episodes_by_keywords");

    assert!(
        !raw.is_empty(),
        "episodic recall must return > 0 raw episodes when the objective shares \
         a keyword with stored episode content (issue #2299, the '0 raw' \
         defect); tokens={tokens:?}"
    );
}

/// AC (#2299, pointed) — A purely case-mismatched recall: content stored in
/// ALL-CAPS, recalled with a lowercased single keyword. This is the minimal
/// reproduction of the case-sensitivity defect and would return zero under the
/// pre-fix case-sensitive matcher.
#[test]
fn episodic_recall_is_case_insensitive_on_library_backend() {
    let mem = test_mem();

    mem.store_episode("DEPLOYED THE AUTHENTICATION SERVICE", "engineer-loop", None)
        .expect("store_episode");

    let raw = mem
        .search_episodes_by_keywords(&["authentication".to_string()], 5)
        .expect("search_episodes_by_keywords");

    assert_eq!(
        raw.len(),
        1,
        "a lowercased keyword must match ALL-CAPS episode content \
         (case-insensitive substring recall, issue #2299)"
    );
    assert!(
        raw[0].content.contains("AUTHENTICATION"),
        "the matched episode must be the one that was stored"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// #2300 — prospective triggers must fire on the library backend
// ─────────────────────────────────────────────────────────────────────────

/// AC (#2300) — Store a prospective whose `trigger_condition` is a goal slug
/// with dashes turned to spaces (exactly what
/// `goals::cognitive_memory_store::prospective_trigger_for` derives), then probe
/// `check_triggers` with a realistic free-text OODA objective that mentions the
/// goal phrase (mixed case, embedded in a longer sentence rather than passed
/// verbatim). The trigger MUST fire.
///
/// The reported symptom was the preparation log forever printing "0 triggers".
/// The native defect combined case-sensitive matching with a probe that did not
/// carry the slug phrase. The library's `check_triggers` matches on lowercased
/// keyword overlap, so a realistic objective that shares tokens with the trigger
/// condition fires it; this test pins that on the library backend.
#[test]
fn prospective_trigger_fires_for_realistic_objective() {
    let mem = test_mem();

    // Slug `improve-cognitive-memory-persistence` → trigger condition
    // `improve cognitive memory persistence` (dashes→spaces); the description
    // carries the `goal:` prefix the live goal store uses.
    mem.store_prospective(
        "goal: improve cognitive memory persistence",
        "improve cognitive memory persistence",
        "resume work on the goal",
        1,
    )
    .expect("store_prospective");

    // Realistic OODA objective: free text, MIXED case, the goal phrase embedded
    // in a longer sentence rather than passed verbatim/lowercased.
    let objective =
        "Continue to Improve Cognitive Memory persistence and durable recall this OODA cycle";

    let triggered = mem.check_triggers(objective).expect("check_triggers");

    assert!(
        triggered.iter().any(|p| p.description.starts_with("goal:")),
        "a stored goal prospective must fire when the OODA objective mentions \
         its slug phrase (issue #2300, the '0 triggers' defect); got {} \
         triggers: {:?}",
        triggered.len(),
        triggered.iter().map(|p| &p.description).collect::<Vec<_>>()
    );
}

/// AC (#2300, keyword-overlap) — The trigger must fire even when the objective
/// shares the trigger's keywords without containing the slug phrase as a
/// contiguous substring. This proves the live match is tokenized keyword
/// overlap (the library's contract), not whole-phrase `CONTAINS`, so realistic
/// OODA objectives that paraphrase the goal still fire.
#[test]
fn prospective_trigger_fires_on_keyword_overlap_without_contiguous_phrase() {
    let mem = test_mem();

    mem.store_prospective(
        "goal: fix broken authentication",
        "fix broken authentication",
        "resume work on the goal",
        1,
    )
    .expect("store_prospective");

    // The trigger tokens (fix / broken / authentication) all appear, but never
    // as the contiguous phrase "fix broken authentication".
    let objective =
        "Authentication is broken in the login flow; we need to fix the regression today";

    let triggered = mem.check_triggers(objective).expect("check_triggers");

    assert!(
        triggered.iter().any(|p| p.description.starts_with("goal:")),
        "the goal prospective must fire on keyword overlap even without the \
         contiguous slug phrase (issue #2300); got {} triggers: {:?}",
        triggered.len(),
        triggered.iter().map(|p| &p.description).collect::<Vec<_>>()
    );
}
