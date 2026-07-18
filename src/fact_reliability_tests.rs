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
    normalize_source_episode_id, score_fact_reliability,
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

/// Punctuation/symbol-only content carries no more information than an empty
/// string, so it extends the same HARD gate: score 0.0 and quarantine, even
/// when grounded and labelled with a known concept (which would otherwise reach
/// 0.5 + 0.3 + 0.1 = 0.9 under the old raw-token count that treated `"..."` as
/// three "words"). This is the fact-yield-quality fix: a wall of punctuation
/// must never be promoted into semantic memory.
#[test]
fn punctuation_only_content_is_a_hard_zero_even_when_grounded() {
    for junk in ["... ... ...", "- - -", "??? !!! ///", "—— —— ——", ". , ; :"] {
        assert_eq!(
            score_fact_reliability("bug-pattern", junk, true),
            0.0,
            "punctuation/symbol-only content {junk:?} carries no information and is quarantined"
        );
        assert!(
            !fact_passes_gate("bug-pattern", junk, true),
            "punctuation/symbol-only content {junk:?} must never clear the gate"
        );
    }
}

/// Degenerate repetition of a single word is one *distinct* informative word,
/// not three, so it earns only the partial short-content weight (0.15), not the
/// full 0.3. Under the old raw-token count `"the the the"` scored the full
/// content weight; scoring distinct informative words closes that loophole.
#[test]
fn repeated_single_word_earns_only_partial_content_weight() {
    // grounded + 1 distinct word (0.15) + known concept (0.1) = 0.75, not 0.9.
    let s = score_fact_reliability("lesson-learned", "the the the the", true);
    assert!(
        (s - 0.75).abs() < 1e-9,
        "repeated single word is one distinct informative word (partial weight), got {s}"
    );
    // Case/punctuation variants of the same word still collapse to one distinct
    // word, so surface churn cannot inflate the count to the full weight.
    let variants = score_fact_reliability("lesson-learned", "Recall recall, recall. RECALL", true);
    assert!(
        (variants - 0.75).abs() < 1e-9,
        "case/punctuation variants of one word stay one distinct word, got {variants}"
    );
}

/// Three genuinely distinct informative words earn the full content weight,
/// confirming the informative-word count agrees with the raw-token count on
/// honest content — only degenerate content changes disposition.
#[test]
fn three_distinct_informative_words_earn_full_content_weight() {
    // grounded (0.5) + ≥3 distinct words (0.3) + known concept (0.1) = 0.9.
    let s = score_fact_reliability("bug-pattern", "retry saturates the socket", true);
    assert!(
        (s - 0.9).abs() < 1e-9,
        "three+ distinct informative words earn the full content weight, got {s}"
    );
    // Numbers count as informative words too.
    let numeric = score_fact_reliability("pr-pattern", "1 2 3", true);
    assert!(
        (numeric - 0.9).abs() < 1e-9,
        "distinct numeric tokens are informative words, got {numeric}"
    );
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
// normalize_source_episode_id: the shared grounding/provenance id key both seams
// use so a whitespace-padded cited id grounds and threads provenance identically
// ───────────────────────────────────────────────────────────────────────────

/// Surrounding whitespace an LLM might append to a re-emitted id is trimmed, so
/// the *exact* grounding match still succeeds and the fact is not silently
/// quarantined as ungrounded (lost fact-yield).
#[test]
fn normalize_source_episode_id_trims_surrounding_whitespace() {
    for raw in [
        " epi_00001",
        "epi_00001 ",
        "  epi_00001  ",
        "epi_00001\n",
        "\tepi_00001\r\n",
    ] {
        assert_eq!(
            normalize_source_episode_id(raw),
            "epi_00001",
            "surrounding whitespace must be trimmed for {raw:?}"
        );
    }
}

/// A well-formed id (episode node ids are UUID-v7 / ULID and never carry
/// whitespace) is returned unchanged — the normalization is a no-op for every
/// real id, so it cannot alter an already-grounding fact's disposition.
#[test]
fn normalize_source_episode_id_is_noop_for_clean_id() {
    for raw in [
        "epi_00001",
        "0192f8c1-5a3e-7abc-9def-0123456789ab",
        "ep-123",
    ] {
        assert_eq!(normalize_source_episode_id(raw), raw);
    }
}

/// Interior whitespace is deliberately preserved: `"ep 123"` is a genuinely
/// different / malformed id and must stay ungrounded rather than silently fold
/// onto `"ep123"` — matching the conservative surface-form policy of
/// `canonical_concept` / `dedup_content_key`. An all-whitespace id normalizes to
/// the empty string (which no episode resolves, so it is correctly ungrounded).
#[test]
fn normalize_source_episode_id_preserves_interior_and_empties_blank() {
    assert_eq!(normalize_source_episode_id("ep 123"), "ep 123");
    assert_eq!(normalize_source_episode_id("   "), "");
    assert_eq!(normalize_source_episode_id(""), "");
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
// dedup_content_key: whitespace-robust identity for the dedup step. A fact
// restated with only interior/surrounding whitespace variation is the SAME
// fact and must not be promoted twice (redundant facts inflate memory and
// dilute recall precision).
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn dedup_content_key_collapses_interior_and_edge_whitespace() {
    use crate::fact_reliability::dedup_content_key;

    let canonical = "empty outcome list panics cycle";
    // Leading/trailing whitespace, a double interior space, a tab, and a
    // wrapped newline all fold onto the same single-spaced key.
    assert_eq!(
        dedup_content_key("  empty outcome list panics cycle  "),
        canonical
    );
    assert_eq!(
        dedup_content_key("empty  outcome list panics cycle"),
        canonical
    );
    assert_eq!(
        dedup_content_key("empty\toutcome list panics cycle"),
        canonical
    );
    assert_eq!(
        dedup_content_key("empty outcome list\npanics cycle"),
        canonical
    );
}

#[test]
fn dedup_content_key_preserves_case_and_distinct_words() {
    use crate::fact_reliability::dedup_content_key;

    // Case is significant (identifiers / error strings) → NOT folded.
    assert_ne!(
        dedup_content_key("CI fails on flaky test"),
        dedup_content_key("ci fails on flaky test")
    );
    // Genuinely different content keeps a different key.
    assert_ne!(
        dedup_content_key("off-by-one in retry loop"),
        dedup_content_key("empty outcome list panics cycle")
    );
    // Empty / whitespace-only content normalizes to the empty key.
    assert_eq!(dedup_content_key("   \t\n "), "");
}

/// A grounded, known-concept fact that restates an already-stored fact with ONLY
/// interior-whitespace variation is recognized as the same fact and quarantined
/// as a dedup (its score still clears the threshold), so no redundant near-
/// duplicate is promoted. Before this fix, exact `content.trim()` equality
/// missed the whitespace variant and stored a second copy.
#[test]
fn commit_gated_fact_dedups_whitespace_variant_restatement() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::{FactGateDecision, commit_gated_fact};

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode("episode payload for dedup test", "engineer-cycle", None)
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let tags = [String::from("bug-pattern")];
    let episode_ids = [ep.clone()];

    // (1) Store the canonical fact.
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
    assert!(stored.stored(), "canonical fact must be stored first");

    // (2) Restate it with a double interior space + surrounding whitespace →
    // dedup quarantine (same lesson), and its score still clears the threshold
    // so it is distinguishable from a low-reliability block.
    let variant = commit_gated_fact(
        &mem,
        "bug-pattern",
        "  empty  outcome list panics cycle ",
        true,
        &source,
        &tags,
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        !variant.stored(),
        "a whitespace-only restatement must dedup, not create a redundant fact"
    );
    assert!(
        variant.confidence() >= RELIABILITY_THRESHOLD,
        "a dedup quarantine cleared the threshold; only the prior blocks it"
    );

    // (3) A genuinely different fact under the same concept is still stored.
    let distinct = commit_gated_fact(
        &mem,
        "bug-pattern",
        "off-by-one in retry loop drops last item",
        true,
        &source,
        &tags,
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        matches!(distinct, FactGateDecision::Stored { .. }),
        "distinct content must still be promoted"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// concept_identity: fold known-concept surface variants to one stable label so
// the separator-sensitive dedup lookup can't miss a prior stored under a
// different surface form; leave off-spec labels verbatim.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn concept_identity_folds_known_and_preserves_offspec() {
    use crate::fact_reliability::concept_identity;
    // Every surface variant of a closed-set concept folds to the canonical label.
    for raw in [
        "bug-pattern",
        "bug_pattern",
        "Bug-Pattern",
        "BUG PATTERN",
        " bug-pattern ",
    ] {
        assert_eq!(
            concept_identity(raw),
            "bug-pattern",
            "known-concept surface variant {raw:?} must fold to the canonical label"
        );
    }
    assert_eq!(concept_identity("pr_pattern"), "pr-pattern");
    assert_eq!(concept_identity("Lesson Learned"), "lesson-learned");
    // A genuinely off-spec label is preserved verbatim (never rewritten/merged).
    assert_eq!(concept_identity("architecture-note"), "architecture-note");
    assert_eq!(concept_identity("custom_label"), "custom_label");
}

/// The dedup step must recognize a prior stored under a DIFFERENT surface form of
/// the SAME closed-set concept. The store's `search_facts` concept lookup is
/// case-insensitive but separator-sensitive, so a fact stored under
/// `"bug-pattern"` is invisible to a raw `"bug_pattern"` query. Before folding the
/// concept to its identity label at the write boundary, the underscore variant
/// missed the prior and a redundant near-duplicate was promoted — inflating
/// semantic memory and dragging down recall precision.
#[test]
fn commit_gated_fact_dedups_concept_surface_variant() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::commit_gated_fact;

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode(
            "episode payload for concept-variant dedup",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];
    let content = "empty outcome list panics cycle";

    // (1) Store the fact under the canonical hyphenated concept.
    let stored = commit_gated_fact(
        &mem,
        "bug-pattern",
        content,
        true,
        &source,
        &[String::from("bug-pattern")],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        stored.stored(),
        "canonical-concept fact must be stored first"
    );

    // (2) Restate the SAME lesson under an underscore surface variant of the SAME
    // concept → dedup quarantine (not a redundant second fact). Its score still
    // clears the threshold, so it is distinguishable from a low-reliability block.
    let variant = commit_gated_fact(
        &mem,
        "bug_pattern",
        content,
        true,
        &source,
        &[String::from("bug_pattern")],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        !variant.stored(),
        "an underscore surface variant of the same concept must dedup, not create a duplicate"
    );
    assert!(
        variant.confidence() >= RELIABILITY_THRESHOLD,
        "a dedup quarantine cleared the threshold; only the prior blocks it"
    );

    // The store holds exactly ONE fact for this lesson, under the canonical label.
    let found = mem.search_facts("bug-pattern", 10, 0.0).unwrap_or_default();
    let matches: Vec<_> = found.iter().filter(|f| f.content == content).collect();
    assert_eq!(
        matches.len(),
        1,
        "exactly one copy of the lesson must exist, got {}: {:?}",
        matches.len(),
        matches
            .iter()
            .map(|f| f.concept.clone())
            .collect::<Vec<_>>()
    );
}

/// A known-concept fact committed under a non-canonical surface form is STORED
/// under the canonical label, so recall consolidates every surface variant of the
/// lesson under one concept rather than splitting it across `"bug-pattern"` and
/// `"bug_pattern"`.
#[test]
fn commit_gated_fact_stores_canonical_concept_label() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::{FactGateDecision, commit_gated_fact};

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode(
            "episode payload for canonical-store",
            "engineer-cycle",
            None,
        )
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];

    let stored = commit_gated_fact(
        &mem,
        "Bug_Pattern",
        "off-by-one in retry loop drops last item",
        true,
        &source,
        &[String::from("Bug_Pattern")],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        matches!(stored, FactGateDecision::Stored { .. }),
        "a well-formed known-concept fact must be stored, got {stored:?}"
    );

    // The stored fact carries the CANONICAL concept label, not the raw surface form.
    let found = mem.search_facts("bug-pattern", 10, 0.0).unwrap_or_default();
    assert_eq!(
        found.iter().filter(|f| f.concept == "bug-pattern").count(),
        1,
        "the fact must be stored under the canonical 'bug-pattern' label"
    );
    assert!(
        found.iter().all(|f| f.concept != "Bug_Pattern"),
        "the raw non-canonical surface form must NOT be persisted"
    );
}

/// Belt-and-suspenders: a content-key collision under a GENUINELY DIFFERENT
/// concept must never false-block a new fact. Two facts with identical content
/// but distinct closed-set concepts are BOTH promoted — the dedup predicate now
/// requires concept-identity equality, so a shared content key alone cannot
/// quarantine a distinct-concept fact.
///
/// The shared content deliberately contains the SECOND concept's keyword
/// (`lesson-learned`) so the second commit's `search_facts("lesson-learned", …)`
/// genuinely surfaces the first (`bug-pattern`) fact as a content match — that is
/// the exact input that would false-block without the concept-identity guard.
#[test]
fn commit_gated_fact_does_not_cross_block_distinct_concepts() {
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};
    use crate::fact_reliability::{FactGateDecision, commit_gated_fact};

    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory db");
    let ep = mem
        .store_episode("episode payload for cross-concept", "engineer-cycle", None)
        .expect("store_episode");
    let source = format!("distill:{ep}");
    let episode_ids = [ep.clone()];
    // Content carries the second concept's keyword so the second commit's concept
    // search actually returns the first fact (a real content-key collision).
    let content = "lesson-learned handling of an empty retry queue drops items";

    let first = commit_gated_fact(
        &mem,
        "bug-pattern",
        content,
        true,
        &source,
        &[String::from("bug-pattern")],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(matches!(first, FactGateDecision::Stored { .. }));

    // Precondition: the second concept's search really does surface the first
    // fact as a content match, so this test exercises the concept-identity guard
    // rather than passing vacuously on an empty result set.
    let cross = mem
        .search_facts("lesson-learned", 10, 0.0)
        .unwrap_or_default();
    assert!(
        cross
            .iter()
            .any(|f| f.content == content && f.concept == "bug-pattern"),
        "the bug-pattern fact must surface under a lesson-learned content search for this test to be meaningful"
    );

    let second = commit_gated_fact(
        &mem,
        "lesson-learned",
        content,
        true,
        &source,
        &[String::from("lesson-learned")],
        &episode_ids,
    )
    .expect("commit must not error");
    assert!(
        matches!(second, FactGateDecision::Stored { .. }),
        "identical content under a distinct concept must still be promoted, got {second:?}"
    );
}
