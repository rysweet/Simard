//! TDD (RED) tests for the shared `crate::fact_reliability` scorer (issue
//! #2679).
//!
//! ## Why this module exists
//!
//! Post-#2679 the distillation RESULT path stops relying on parsing entirely:
//! the distiller agentic step writes each fact DIRECTLY through the
//! cognitive-memory gated write boundary. The ISAO reliability gate therefore
//! moves from its old post-parse location in
//! `distill_recent_episodes_with_runner` to the *write boundary* and is applied
//! **per fact**. Two seams reach that boundary:
//!
//!   1. the IPC server's `StoreFactGated` dispatch arm (the authoritative,
//!      server-side gate for the real subprocess path), and
//!   2. the in-process `DistillFactSink` used by the deterministic test stubs.
//!
//! To keep those two seams in lock-step — and to *reduce* the
//! `memory_consolidation` fork per the G2 memory-architecture constraint — the
//! scorer is extracted into one shared, pure module `crate::fact_reliability`.
//! Both seams call the SAME `score_fact_reliability`, so a fact scores
//! identically no matter which boundary writes it.
//!
//! ## Key contract change vs. the legacy `assess_fact_reliability`
//!
//! The legacy scorer took `&[CognitiveEpisode]` and `&[DistilledFact]` (the
//! whole pass batch) so it could compute a **corroboration** term. That batch is
//! unavailable in a per-fact IPC call, so the shared scorer is a pure function
//! of exactly one fact plus a resolved `grounded: bool`:
//!
//! ```ignore
//! pub fn score_fact_reliability(concept: &str, content: &str, grounded: bool) -> f64;
//! ```
//!
//! Grounding is resolved *before* the call (batch-membership for the in-process
//! sink; store-existence for the IPC handler) and the corroboration term is
//! deliberately dropped: it is disposition-neutral (it only nudges an
//! already-storable 0.9 → 1.0, never flips store↔quarantine), so excluding it
//! lets both seams agree on every store/quarantine decision.
//!
//! These tests reference `crate::fact_reliability::*`, which does not exist yet.
//! The unresolved-path errors are the intended TDD red signal.

use crate::fact_reliability::{
    KNOWN_CONCEPTS, RELIABILITY_THRESHOLD, canonical_concept, fact_passes_gate,
    score_fact_reliability,
};

// ───────────────────────────────────────────────────────────────────────────
// Threshold + known-concept constants keep their #2433 values
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn threshold_is_half_and_known_concepts_are_the_closed_set() {
    assert_eq!(
        RELIABILITY_THRESHOLD, 0.5,
        "the promotion threshold is unchanged by #2679 — only its call site moves"
    );
    assert_eq!(
        KNOWN_CONCEPTS,
        &["pr-pattern", "bug-pattern", "lesson-learned"],
        "the closed concept-label set is preserved verbatim from #2433"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Pure per-fact scorer — documented value table (no batch / corroboration arg)
// ───────────────────────────────────────────────────────────────────────────

/// A fully-nominal fact (grounded, ≥3 content words, known concept) scores
/// exactly 0.9 — at or above the legacy `DISTILL_FACT_CONFIDENCE` baseline — so
/// good facts keep their downstream recall behaviour without the corroboration
/// bonus.
#[test]
fn nominal_grounded_known_concept_scores_point_nine() {
    let s = score_fact_reliability("bug-pattern", "empty outcome list panics cycle", true);
    assert!(
        (s - 0.9).abs() < 1e-9,
        "grounded + ≥3 words + known concept must score 0.9, got {s}"
    );
    assert!(fact_passes_gate(
        "bug-pattern",
        "empty outcome list panics cycle",
        true
    ));
}

/// Grounding (0.5) is *necessary* to clear the threshold. An ungrounded fact
/// tops out at 0.3 content + 0.1 concept = 0.4 and is always quarantined — with
/// NO corroboration escape hatch (the term is gone).
#[test]
fn ungrounded_fact_tops_out_below_threshold() {
    let s = score_fact_reliability("bug-pattern", "three or more words here", false);
    assert!(
        (s - 0.4).abs() < 1e-9,
        "ungrounded fact must score 0.4 (content+concept only), got {s}"
    );
    assert!(
        s < RELIABILITY_THRESHOLD,
        "an ungrounded fact must never clear the promotion gate"
    );
    assert!(!fact_passes_gate(
        "bug-pattern",
        "three or more words here",
        false
    ));
}

/// Empty / whitespace-only content is a HARD gate: score 0.0 regardless of how
/// trustworthy the provenance looks (a grounded-but-empty fact must NOT clear
/// the gate at 0.5 grounding + 0.1 concept = 0.6).
#[test]
fn empty_content_is_a_hard_zero_even_when_grounded() {
    assert_eq!(
        score_fact_reliability("bug-pattern", "", true),
        0.0,
        "empty content is quarantined unconditionally"
    );
    assert_eq!(
        score_fact_reliability("bug-pattern", "   \t\n ", true),
        0.0,
        "whitespace-only content is quarantined unconditionally"
    );
    assert!(!fact_passes_gate("bug-pattern", "", true));
}

/// Short (1–2 word) grounded content earns only the partial content weight:
/// 0.5 grounding + 0.15 short-content + 0.1 concept = 0.75.
#[test]
fn grounded_short_content_known_concept_scores_point_seven_five() {
    let s = score_fact_reliability("pr-pattern", "squash fixups", true);
    assert!(
        (s - 0.75).abs() < 1e-9,
        "grounded + <3 words + known concept must score 0.75, got {s}"
    );
    assert!(fact_passes_gate("pr-pattern", "squash fixups", true));
}

/// An off-spec concept loses the 0.1 concept-validity component but a grounded,
/// well-worded fact still clears the gate (0.5 + 0.3 = 0.8). Concept validity is
/// a nudge, not a gate.
#[test]
fn grounded_unknown_concept_still_clears_gate() {
    let s = score_fact_reliability("made-up-label", "three or more words here", true);
    assert!(
        (s - 0.8).abs() < 1e-9,
        "grounded + ≥3 words + unknown concept must score 0.8, got {s}"
    );
    assert!(fact_passes_gate(
        "made-up-label",
        "three or more words here",
        true
    ));
}

/// The scorer is a *pure* function of its three arguments: identical inputs
/// always yield identical scores (no batch, no time, no global state). This is
/// the property that makes the stub-sink seam and the IPC-handler seam agree.
#[test]
fn scorer_is_pure_and_deterministic() {
    let a = score_fact_reliability("lesson-learned", "prefer keyword overlap for recall", true);
    let b = score_fact_reliability("lesson-learned", "prefer keyword overlap for recall", true);
    assert_eq!(
        a, b,
        "the scorer must be deterministic for parity across seams"
    );
}

/// Concept validity is case-insensitive (surface-form variants of a known label
/// still earn the concept weight) but a genuinely off-spec label does not.
#[test]
fn concept_validity_is_case_insensitive() {
    let good = score_fact_reliability("BUG-PATTERN", "three or more words here", true);
    assert!(
        (good - 0.9).abs() < 1e-9,
        "an upper-case known concept must still earn the concept weight, got {good}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// canonical_concept moved here from distillation (shared by both seams)
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn canonical_concept_folds_surface_variants() {
    for (raw, want) in [
        ("pr-pattern", "pr-pattern"),
        ("PR-Pattern", "pr-pattern"),
        ("BUG-PATTERN", "bug-pattern"),
        (" bug-pattern ", "bug-pattern"),
        ("Lesson-Learned", "lesson-learned"),
        ("pr_pattern", "pr-pattern"),
        ("lesson learned", "lesson-learned"),
        ("pr--pattern", "pr-pattern"),
    ] {
        assert_eq!(canonical_concept(raw), Some(want), "variant {raw:?}");
    }
}

#[test]
fn canonical_concept_rejects_offspec() {
    for raw in [
        "made-up-label",
        "skip",
        "pr-patterns",
        "pull-request",
        "",
        "   ",
    ] {
        assert_eq!(
            canonical_concept(raw),
            None,
            "off-spec {raw:?} must be dropped"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Gate parity: fact_passes_gate is exactly score >= RELIABILITY_THRESHOLD
// ───────────────────────────────────────────────────────────────────────────

/// `fact_passes_gate` must be a thin predicate over `score_fact_reliability`
/// so both write-boundary seams share the identical store/quarantine decision.
#[test]
fn gate_predicate_matches_scored_threshold_across_a_matrix() {
    let cases = [
        ("bug-pattern", "empty outcome list panics cycle", true),
        ("bug-pattern", "three or more words here", false),
        ("bug-pattern", "", true),
        ("pr-pattern", "squash fixups", true),
        ("made-up-label", "three or more words here", true),
        ("lesson-learned", "x", false),
    ];
    for (concept, content, grounded) in cases {
        let expected = score_fact_reliability(concept, content, grounded) >= RELIABILITY_THRESHOLD;
        assert_eq!(
            fact_passes_gate(concept, content, grounded),
            expected,
            "gate predicate must equal (score >= threshold) for ({concept:?}, {content:?}, {grounded})"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Shared write-boundary gate: commit_gated_fact drives score → threshold →
// dedup → persist for BOTH seams, so the two can never drift.
// ───────────────────────────────────────────────────────────────────────────

/// A grounded, well-formed, known-concept fact clears the gate and is persisted;
/// re-committing the identical fact is a dedup quarantine (its score still
/// clears the threshold); and an ungrounded empty fact is a low-reliability
/// quarantine (its score is below the threshold). These three dispositions are
/// exactly what the IPC server seam and the in-process `DistillFactSink` seam
/// now share, verbatim, by calling this one function.
#[test]
fn commit_gated_fact_stores_dedups_and_quarantines() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::{FactGateDecision, commit_gated_fact};

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode("episode payload for gate test", "engineer-cycle", None)
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let tags = [String::from("bug-pattern")];
    let episode_ids = [ep.clone()];

    // (1) Grounded, ≥3 words, known concept → stored with the gate-computed
    // confidence (never a client hint), returning the new node id.
    let stored = commit_gated_fact(
        &mem,
        "bug-pattern",
        "empty outcome list panics cycle",
        true,
        &source,
        &tags,
        &episode_ids,
    )
    .expect("commit must not error");
    let FactGateDecision::Stored {
        confidence,
        node_id,
    } = stored.clone()
    else {
        panic!("expected Stored, got {stored:?}");
    };
    assert!(stored.stored());
    assert!(confidence >= RELIABILITY_THRESHOLD);
    assert!(!node_id.is_empty());

    // (2) Identical fact again → dedup quarantine; its score still clears the
    // threshold, so a caller can tell it apart from a low-reliability block.
    let dup = commit_gated_fact(
        &mem,
        "bug-pattern",
        "empty outcome list panics cycle",
        true,
        &source,
        &tags,
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(!dup.stored());
    assert!(
        dup.confidence() >= RELIABILITY_THRESHOLD,
        "a dedup quarantine cleared the threshold; only the prior blocks it"
    );

    // (3) Ungrounded empty fact → low-reliability quarantine (score below the
    // threshold), nothing written.
    let blocked = commit_gated_fact(
        &mem,
        "bug-pattern",
        "   ",
        false,
        &source,
        &tags,
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(!blocked.stored());
    assert!(
        blocked.confidence() < RELIABILITY_THRESHOLD,
        "an ungrounded empty fact scores below the threshold"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Concept-label canonicalization at the shared write boundary: a recognized
// surface-form variant is stored under (and recalled by) its canonical label,
// and two surface-variant restatements of the same fact dedup against each
// other. An off-spec label is stored verbatim (nudge, not gate).
// ───────────────────────────────────────────────────────────────────────────

/// A fact whose concept is a surface-form variant of a known label
/// ("PR-Pattern") is persisted under the canonical label ("pr-pattern") and is
/// therefore recalled by the canonical label — recall no longer fragments across
/// the LLM's surface-form variance.
#[test]
fn commit_gated_fact_stores_variant_concept_under_canonical_label() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::commit_gated_fact;

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode(
            "episode payload for canonicalization test",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];

    let stored = commit_gated_fact(
        &mem,
        "PR-Pattern",
        "prefer small focused pull requests",
        true,
        &source,
        &[],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(stored.stored(), "a grounded well-formed fact stores");

    // The persisted fact carries the canonical label, not the raw "PR-Pattern".
    // Use the wildcard "return all" path so the assertion checks the stored
    // label directly, independent of the library's keyword-relevance ranking.
    let hits = mem.search_facts("*", 100, 0.0).expect("search_facts");
    let found = hits
        .iter()
        .find(|f| f.content.trim() == "prefer small focused pull requests")
        .expect("canonicalized fact present in store");
    assert_eq!(
        found.concept, "pr-pattern",
        "a recognized surface-form variant must persist under its canonical label"
    );
}

/// Two facts with identical content but different *surface forms* of the same
/// concept converge onto one canonical label, so the second is a dedup
/// quarantine instead of a duplicate stored under a divergent label.
#[test]
fn commit_gated_fact_dedups_across_surface_form_variants() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::{FactGateDecision, commit_gated_fact};

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode(
            "episode payload for variant-dedup test",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];
    let content = "empty outcome list panics cycle";

    let first = commit_gated_fact(
        &mem,
        "bug-pattern",
        content,
        true,
        &source,
        &[],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(first.stored(), "first insert stores");

    // Same fact, but the LLM emitted a different surface form of the label.
    let second = commit_gated_fact(
        &mem,
        "Bug_Pattern",
        content,
        true,
        &source,
        &[],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        !second.stored(),
        "a surface-form variant of an equal-or-stronger prior must dedup, not store a divergent copy"
    );
    assert!(
        matches!(second, FactGateDecision::Quarantined { confidence } if confidence >= RELIABILITY_THRESHOLD),
        "the dedup quarantine still cleared the reliability threshold"
    );

    // Exactly one fact of this concept exists in the store.
    let hits = mem.search_facts("*", 100, 0.0).expect("search_facts");
    let matching = hits
        .iter()
        .filter(|f| f.content.trim() == content && f.concept == "bug-pattern")
        .count();
    assert_eq!(
        matching, 1,
        "the two variants must converge onto a single stored fact"
    );
}

/// A genuinely off-spec concept does not canonicalize and is stored verbatim —
/// canonicalization is a nudge, not a gate, and never relabels or drops an
/// unrecognized concept.
#[test]
fn commit_gated_fact_stores_offspec_concept_verbatim() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::commit_gated_fact;

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode("episode payload for off-spec test", "engineer-cycle", None)
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];

    let stored = commit_gated_fact(
        &mem,
        "infra-observation",
        "the deploy pipeline flaked on a cold cache",
        true,
        &source,
        &[],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(stored.stored(), "a grounded well-formed fact stores");

    let hits = mem.search_facts("*", 100, 0.0).expect("search_facts");
    let found = hits
        .iter()
        .find(|f| f.content.trim() == "the deploy pipeline flaked on a cold cache")
        .expect("off-spec fact present in store");
    assert_eq!(
        found.concept, "infra-observation",
        "an off-spec concept must be stored verbatim, never relabeled"
    );
}
