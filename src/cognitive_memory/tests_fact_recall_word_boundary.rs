//! End-to-end regression tests for the WORD-BOUNDARY (marker-safe) relevance
//! gate on `LibraryCognitiveMemory::search_facts` — the FACT-recall analogue of
//! the episodic gate pinned by `tests_whole_word_episode_recall`.
//!
//! ## Why these exist
//!
//! The upstream library's `search_facts` matches a query token by RAW
//! case-insensitive SUBSTRING against a fact's concept OR content. For the
//! marker/concept callers (`journal:YYYY-MM-DD`, `goal-edge:…`, `bug-pattern`)
//! that is exactly right, but the natural-language callers — most importantly
//! `base_type_turn::prepare_turn_context`, which recalls facts by the turn
//! OBJECTIVE to build working context — got facts floated in on the
//! INTERIOR/SUFFIX of an unrelated word: `act` recalled "re**act**or" and
//! "artif**act**", `own` recalled "d**own**load", `test` recalled "la**test**".
//! Those off-topic facts crowd the capped recall the OODA cycle feeds to
//! reasoning, dragging fact recall precision (and effective distillation
//! fact-yield) down — the same defect the episodic recall gate (PR #4241
//! lineage) already removed.
//!
//! `search_facts` now gates a CLEAN alphanumeric query token at a WORD BOUNDARY
//! (a prefix of a whole word in the concept OR content — inflection-tolerant, so
//! stemmed queries still recall), while a concept label or colon marker keeps
//! the library's exact substring semantics its callers store and re-filter on.
//!
//! These drive `LibraryCognitiveMemory::in_memory()` directly so the live
//! backend is exercised hermetically. The pure gate helpers are unit-tested in
//! `library_adapter::fact_query_gate_tests`.

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory library DB should create")
}

fn contents(mem: &LibraryCognitiveMemory, query: &str) -> Vec<String> {
    mem.search_facts(query, 64, 0.0)
        .expect("search_facts")
        .into_iter()
        .map(|f| f.content)
        .collect()
}

/// The core recall-quality contract: a clean natural-language token that only
/// appears EMBEDDED in an unrelated word must NOT surface that fact, while the
/// same token DOES surface a fact where it appears at a word boundary.
#[test]
fn interior_substring_no_longer_spuriously_recalls_fact() {
    let mem = test_mem();
    mem.store_fact("bug-pattern", "the reactor overheated badly", 0.9, &[], "s")
        .expect("store reactor");
    mem.store_fact("bug-pattern", "download the latest artifact", 0.9, &[], "s")
        .expect("store download");
    mem.store_fact("lesson-learned", "act quickly on failures", 0.9, &[], "s")
        .expect("store act");

    // `act` embeds in "re(act)or" and "artif(act)" and suffixes nothing here —
    // only the genuine word-boundary fact must survive.
    let hits = contents(&mem, "act");
    assert_eq!(
        hits,
        vec!["act quickly on failures".to_string()],
        "interior-substring facts must be gated out, got {hits:?}"
    );

    // `own` embeds in "d(own)load"; nothing legitimately matches.
    assert!(
        contents(&mem, "own").is_empty(),
        "`own` must not recall via the interior of 'download'"
    );

    // `test` embeds in "la(test)"; nothing legitimately matches.
    assert!(
        contents(&mem, "test").is_empty(),
        "`test` must not recall via the interior of 'latest'"
    );
}

/// Word-boundary and inflectional recall the live path depends on are preserved:
/// a whole-word query, a prefix query, and a stem all still recall.
#[test]
fn word_boundary_and_inflectional_recall_preserved() {
    let mem = test_mem();
    mem.store_fact("bug-pattern", "the reactor overheated badly", 0.9, &[], "s")
        .expect("store reactor");
    mem.store_fact(
        "lesson-learned",
        "deployed the payment service",
        0.9,
        &[],
        "s",
    )
    .expect("store deployed");

    assert_eq!(contents(&mem, "reactor").len(), 1, "whole-word recall");
    assert_eq!(contents(&mem, "react").len(), 1, "prefix recall");
    // Stemmed query still recalls the inflected form.
    assert_eq!(
        contents(&mem, "deploy"),
        vec!["deployed the payment service".to_string()],
        "inflectional recall must survive the gate"
    );
}

/// A clean token that is a word-boundary prefix of the CONCEPT (not the content)
/// still recalls — the library searches both fields, so the gate must too.
#[test]
fn clean_token_matches_concept_field() {
    let mem = test_mem();
    mem.store_fact("pr-pattern", "unrelated body text here", 0.9, &[], "s")
        .expect("store pr-pattern fact");

    // "pr" is a word-boundary prefix of the concept "pr-pattern".
    assert_eq!(
        contents(&mem, "pr"),
        vec!["unrelated body text here".to_string()],
        "a concept-field word-boundary hit must survive the gate"
    );
}

/// Concept / colon-marker queries keep the library's exact substring semantics —
/// the many marker callers (`journal:`, `goal-edge:`, hyphenated concepts) are
/// unaffected because such a query has no clean token and bypasses the gate.
#[test]
fn concept_and_marker_queries_keep_substring_semantics() {
    let mem = test_mem();
    // Journal-style facts keyed by a colon-marker concept.
    mem.store_fact("journal:2026-07-18", "{\"body\":\"monday\"}", 0.9, &[], "s")
        .expect("store journal fact");
    mem.store_fact("bug-pattern", "a genuine bug pattern lesson", 0.9, &[], "s")
        .expect("store bug-pattern fact");

    // Exact colon-marker lookup still resolves (raw substring path).
    assert_eq!(
        contents(&mem, "journal:2026-07-18"),
        vec!["{\"body\":\"monday\"}".to_string()],
        "colon-marker lookup must be preserved verbatim"
    );

    // The `journal` enumeration token (clean) still finds the journal fact via a
    // word-boundary prefix of the "journal:2026-07-18" concept.
    assert_eq!(
        contents(&mem, "journal"),
        vec!["{\"body\":\"monday\"}".to_string()],
        "clean `journal` token must word-boundary match the journal concept"
    );

    // A hyphenated concept lookup is a raw token → exact substring, unchanged.
    assert_eq!(
        contents(&mem, "bug-pattern"),
        vec!["a genuine bug pattern lesson".to_string()],
        "hyphenated concept lookup must be preserved verbatim"
    );
}

/// The gate defers truncation: it queries the backend unbounded and caps AFTER
/// filtering, so a relevant fact ranked behind an interior-substring false
/// positive is not dropped before the gate runs, and the returned set still
/// honours `limit`.
#[test]
fn gate_defers_truncation_and_still_honours_limit() {
    let mem = test_mem();
    // Two interior-substring false positives for the query `act`, then two
    // genuine word-boundary matches.
    mem.store_fact("bug-pattern", "reactor one", 0.9, &[], "s")
        .expect("fp1");
    mem.store_fact("bug-pattern", "reactor two", 0.9, &[], "s")
        .expect("fp2");
    mem.store_fact("lesson-learned", "act now please", 0.9, &[], "s")
        .expect("tp1");
    mem.store_fact("lesson-learned", "actionable follow up", 0.9, &[], "s")
        .expect("tp2");

    // With a limit of 2, a naive "cap before gate" could return the two
    // interior-substring "reactor" facts and then filter them ALL out, yielding
    // zero. Deferring truncation must instead surface the two genuine matches.
    let hits = mem.search_facts("act", 2, 0.0).expect("search");
    assert_eq!(hits.len(), 2, "two genuine word-boundary matches expected");
    for f in &hits {
        assert!(
            f.content == "act now please" || f.content == "actionable follow up",
            "unexpected content survived the gate: {:?}",
            f.content
        );
    }
}

/// Wildcard / empty queries are untouched by the gate (they map to the library's
/// "return all" path).
#[test]
fn wildcard_and_empty_queries_bypass_gate() {
    let mem = test_mem();
    mem.store_fact("bug-pattern", "reactor overheated", 0.9, &[], "s")
        .expect("store");
    mem.store_fact("lesson-learned", "download artifact", 0.9, &[], "s")
        .expect("store");

    assert_eq!(contents(&mem, "*").len(), 2, "wildcard returns all facts");
    assert_eq!(contents(&mem, "").len(), 2, "empty query returns all facts");
    assert_eq!(
        contents(&mem, "   ").len(),
        2,
        "blank query returns all facts"
    );
}

/// Recall-precision regression: a lone sub-threshold (single-char) clean query
/// token must recall NOTHING rather than prefix-match every fact containing a
/// word starting with that character — and, crucially, must not fall through to
/// the library's raw-substring `search_facts`, where a lone "s" would substring
/// every fact holding an s-word. A genuine multi-char token still recalls.
#[test]
fn lone_sub_threshold_clean_token_recalls_nothing() {
    let mem = test_mem();
    mem.store_fact("bug-pattern", "the session state was lost", 0.9, &[], "s")
        .expect("store session fact");
    mem.store_fact("lesson-learned", "storage sync succeeded", 0.9, &[], "s")
        .expect("store storage fact");

    // A lone "s" matches every s-word (session, state, storage, sync, succeeded)
    // under both the prefix gate and the raw substring path — it must be gated
    // out entirely as recall noise.
    assert!(
        contents(&mem, "s").is_empty(),
        "a lone 's' must recall no facts, got {:?}",
        contents(&mem, "s")
    );

    // A real query token still recalls at a word boundary.
    assert_eq!(
        contents(&mem, "session"),
        vec!["the session state was lost".to_string()],
        "a multi-char token still recalls its whole-word fact"
    );

    // A mixed query drops the lone "s" but keeps the real token: it recalls the
    // session fact only, not the unrelated storage/sync fact.
    assert_eq!(
        contents(&mem, "s session"),
        vec!["the session state was lost".to_string()],
        "the dropped lone 's' must not float the storage fact in"
    );
}
