//! Characterization tests pinning the THREE deliberately-divergent "text
//! relevance" definitions in the cognition recall/measurement stack
//! (issue [#4378](https://github.com/rysweet/Simard/issues/4378)).
//!
//! Three layers each answer "is this fact relevant to the query?" differently,
//! by design (see `docs/reference/recall-precision-hybrid-api.md` §"Relationship
//! to the served word-boundary gate and the ranker"):
//!
//!   1. **Served recall gate — word-boundary.** `search_facts` gates a clean
//!      natural-language query token at a WORD BOUNDARY
//!      (`fact_shares_query_relevance` / `needle_matches_word`), so an interior
//!      substring hit (`act` in "re**act**or") is NOT relevant.
//!   2. **Ranked recall — ungated.** `recall_facts_ranked` scores every live
//!      fact by a weighted keyword-Jaccard-dominated sum with NO keyword gate,
//!      so the `recall_precision_at_k` self-metric can measure ranking quality
//!      (`precision_at_k < 1.0` is meaningful only because the set is ungated).
//!   3. **Precision metric — substring proxy.** `metrics::precision_at_k`
//!      delegates its relevance oracle to the upstream
//!      `amplihack_memory::measurement` primitive (guideline G2 — not forked
//!      here), whose judgment is a case-insensitive SUBSTRING, matching an
//!      interior hit the served word-boundary gate (#1) rejects.
//!
//! These tests are **teeth, not a bug**: they pin the divergence so it cannot
//! silently widen and so any future *convergence* (which USER_PREFERENCES routes
//! to CONSENSUS_WORKFLOW as a relevance-definition change) is a deliberate,
//! test-visible edit rather than an accidental drift. They assert against the
//! PUBLIC `CognitiveMemoryOps` surface only — no private relevance helper is
//! reached into — so they stay valid across adapter refactors.

use super::{CognitiveMemoryOps, LibraryCognitiveMemory, RecallWeightSet};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// The load-bearing divergence: for a query token that only appears in the
/// INTERIOR of a content word, the served word-boundary gate (#1) and the
/// substring-proxy precision metric (#3, computed over the ungated ranker #2)
/// give OPPOSITE relevance answers for the SAME fact.
///
/// `search_facts("act", …)` (served) returns NOTHING — "act" is not a
/// word-boundary match of "reactor" — while `recall_facts_ranked("act", …)`
/// (ungated) returns the fact and `metrics::precision_at_k` scores that returned
/// set at a perfect `1.0`, because the substring proxy counts "re**act**or" as
/// relevant. So the `recall_precision_at_k` signal can read `1.0` for a query
/// the user is served zero facts for. Pinning this makes the fidelity gap the
/// issue describes an executable invariant.
#[test]
fn served_gate_and_precision_metric_diverge_on_interior_substring() {
    let mem = test_mem();
    // Only content whose word CONTAINS "act" in its interior ("reactor"); no
    // word-boundary "act" token anywhere.
    mem.store_fact("reactor design", "reactor pattern notes", 0.9, &[], "src")
        .expect("store reactor fact");

    // (#1) Served recall gate: word-boundary → the interior "act" hit is NOT
    // relevant, so the user is served nothing.
    let served = mem
        .search_facts("act", 10, 0.0)
        .expect("served search_facts");
    assert!(
        served.is_empty(),
        "served word-boundary gate (#1) must EXCLUDE an interior-substring-only \
         match ('act' in 'reactor'); got {} fact(s)",
        served.len()
    );

    // (#2) Ranked recall is UNGATED: the same fact is returned (it is the
    // corpus), regardless of text-boundary relevance. This is intentional and
    // load-bearing for the precision measurement infra.
    let ranked = mem
        .recall_facts_ranked("act", 10, 0.0, RecallWeightSet::default())
        .expect("ranked recall");
    assert_eq!(
        ranked.len(),
        1,
        "ranked recall (#2) is ungated → the fact is in the measured set"
    );

    // (#3) Precision metric's substring oracle counts the interior "act" hit as
    // relevant → perfect precision over the ranker's output, diverging from the
    // served (#1) answer of zero facts.
    let precision = super::metrics::precision_at_k("act", &ranked, ranked.len());
    assert_eq!(
        precision,
        Some(1.0),
        "precision metric (#3) substring proxy must count 'act' in 'reactor' as \
         relevant → 1.0, even though the served gate (#1) recalled 0 facts"
    );
}

/// Complement to the divergence test: the three definitions AGREE for a
/// whole-word query token, so the divergence pinned above is SPECIFIC to
/// interior/suffix substring hits, not a wholesale disagreement. A "kafka" token
/// is a word-boundary match, an ungated ranker hit, AND a substring hit — all
/// three layers call the fact relevant.
#[test]
fn all_three_definitions_agree_on_word_boundary_match() {
    let mem = test_mem();
    mem.store_fact("kafka streaming", "backpressure", 0.9, &[], "src")
        .expect("store kafka fact");

    // (#1) Served gate: "kafka" is a word-boundary prefix of the concept word
    // "kafka" → relevant, so the fact IS served.
    let served = mem
        .search_facts("kafka", 10, 0.0)
        .expect("served search_facts");
    assert_eq!(
        served.len(),
        1,
        "served word-boundary gate (#1) recalls a whole-word 'kafka' match"
    );

    // (#2) Ranked recall returns it (ungated), and
    // (#3) the substring proxy also counts it relevant → precision 1.0. All
    // three agree.
    let ranked = mem
        .recall_facts_ranked("kafka", 10, 0.0, RecallWeightSet::default())
        .expect("ranked recall");
    assert_eq!(ranked.len(), 1, "ranked recall returns the fact");
    assert_eq!(
        super::metrics::precision_at_k("kafka", &ranked, ranked.len()),
        Some(1.0),
        "precision metric agrees the whole-word match is relevant"
    );
}
