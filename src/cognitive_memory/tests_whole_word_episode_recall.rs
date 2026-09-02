//! End-to-end regression tests for the WORD-BOUNDARY (marker-safe) keyword gate
//! on `LibraryCognitiveMemory::search_episodes_by_keywords` — the sole backend
//! after de-fork Phase 2b (#2307).
//!
//! ## Why these exist
//!
//! `search_episodes_by_keywords` used to match every keyword by RAW
//! case-insensitive substring (`content.contains(kw)`). The sibling ranked path
//! (`recall_episodes_ranked`) was moved to a word-boundary gate, but the flat
//! scan was left on substring because its bracketed-marker callers
//! (`memory_consolidation::reflection_lessons`) depend on exact-substring match.
//! Its OTHER callers, though, pass clean natural-language tokens
//! (`creative_ideas` -> "meeting"/"conversation"/"decision"), and for those a
//! short token merely EMBEDDED in the interior/suffix of an unrelated content
//! word ("test" in "latest", "decision" in "indecision") spuriously recalled an
//! off-topic episode — the same recall-quality defect the ranked path and the
//! knowledge-pack fix (#4241) already removed.
//!
//! The gate now matches a CLEAN alphanumeric keyword at a WORD BOUNDARY (a
//! prefix of a whole content word — inflection-tolerant, so plural/verb forms
//! still recall), while a phrase or bracketed provenance MARKER keyword keeps
//! the legacy substring semantics `reflection_lessons` dedup relies on.
//!
//! These drive `LibraryCognitiveMemory::in_memory()` directly so the live
//! backend is exercised hermetically (the IPC/mock stubs would mask the defect).
//! The pure word-boundary helpers are unit-tested in
//! `library_adapter::word_boundary_gate_tests`.

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory library DB should create")
}

/// The core recall-quality contract: a clean keyword that only appears EMBEDDED
/// in an unrelated word must NOT surface that episode, while the same keyword
/// DOES surface an episode where it appears at a word boundary.
#[test]
fn embedded_token_no_longer_spuriously_recalls() {
    let mem = test_mem();

    // Off-topic: "decision" is embedded in "indecision" (substring would match).
    mem.store_episode(
        "The team showed indecision about the rollout scope",
        "engineer-loop",
        None,
    )
    .expect("store off-topic episode");

    // On-topic: "decision" appears as a genuine whole word.
    let on_topic = mem
        .store_episode(
            "Recorded the decision to migrate the store",
            "engineer-loop",
            None,
        )
        .expect("store on-topic episode");

    let hits = mem
        .search_episodes_by_keywords(&["decision".to_string()], 10)
        .expect("search_episodes_by_keywords");

    assert_eq!(
        hits.len(),
        1,
        "keyword 'decision' must match only the word-boundary episode, not the one \
         where it is merely embedded in 'indecision'; got: {:?}",
        hits.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    assert_eq!(
        hits[0].node_id, on_topic,
        "the surfaced episode must be the word-boundary match"
    );
}

/// A short suffix-embedded token ("test" inside "latest") must not recall, the
/// canonical case the raw-substring scan got wrong.
#[test]
fn suffix_embedded_short_token_does_not_recall() {
    let mem = test_mem();

    mem.store_episode("Shipped the latest greatest build", "engineer-loop", None)
        .expect("store off-topic episode");
    let on_topic = mem
        .store_episode(
            "Ran the integration test suite green",
            "engineer-loop",
            None,
        )
        .expect("store on-topic episode");

    let hits = mem
        .search_episodes_by_keywords(&["test".to_string()], 10)
        .expect("search test");

    assert_eq!(
        hits.len(),
        1,
        "keyword 'test' must not match 'latest' (suffix embedding); got: {:?}",
        hits.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    assert_eq!(hits[0].node_id, on_topic);
}

/// Recall is preserved for genuine plural/verb inflections (word-boundary prefix
/// match), so the tightening does not reintroduce the "0 raw episodes" gap
/// (#2299).
#[test]
fn inflected_forms_still_recall() {
    let mem = test_mem();

    mem.store_episode(
        "Notes captured across three meetings today",
        "engineer-loop",
        None,
    )
    .expect("store episode");
    mem.store_episode(
        "The consolidation pass recalled the fact twice",
        "engineer-loop",
        None,
    )
    .expect("store episode");

    let meeting_hits = mem
        .search_episodes_by_keywords(&["meeting".to_string()], 10)
        .expect("search meetings");
    assert_eq!(
        meeting_hits.len(),
        1,
        "keyword 'meeting' must still recall an episode saying 'meetings' (plural inflection)"
    );

    let recall_hits = mem
        .search_episodes_by_keywords(&["recall".to_string()], 10)
        .expect("search recall");
    assert_eq!(
        recall_hits.len(),
        1,
        "keyword 'recall' must still recall an episode saying 'recalled' (verb inflection)"
    );
}

/// The real `creative_ideas` caller tokens ("meeting"/"conversation"/"decision")
/// recall only the on-topic conversation episode, not an off-topic one that
/// merely embeds one of the tokens.
#[test]
fn creative_ideas_tokens_recall_only_on_topic() {
    let mem = test_mem();

    mem.store_episode(
        "Reviewed the latest indecision about vendors",
        "engineer-loop",
        None,
    )
    .expect("store off-topic episode");
    let on_topic = mem
        .store_episode(
            "Notes from the planning conversation and the final decision",
            "engineer-loop",
            None,
        )
        .expect("store on-topic episode");

    let hits = mem
        .search_episodes_by_keywords(
            &[
                "meeting".to_string(),
                "conversation".to_string(),
                "decision".to_string(),
            ],
            10,
        )
        .expect("search creative-ideas tokens");

    assert_eq!(
        hits.len(),
        1,
        "the creative-ideas tokens must recall only the on-topic conversation episode; got: {:?}",
        hits.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    assert_eq!(hits[0].node_id, on_topic);
}

/// A bracketed provenance marker keyword keeps SUBSTRING semantics: it is
/// embedded in a longer content line and must still be found (the contract
/// `reflection_lessons::occurrence_already_reflected` /
/// `count_recurring_failures` depend on), while a different marker does not
/// match.
#[test]
fn bracketed_marker_keyword_still_matches_as_substring() {
    let mem = test_mem();

    mem.store_episode(
        "reflection:failure blocked pr [reflect-occ=ci-timeout] recorded",
        "engineer-loop",
        None,
    )
    .expect("store marker episode");

    let hit = mem
        .search_episodes_by_keywords(&["[reflect-occ=ci-timeout]".to_string()], 10)
        .expect("search marker");
    assert_eq!(
        hit.len(),
        1,
        "a bracketed provenance marker must still match as a substring inside a longer line"
    );

    let miss = mem
        .search_episodes_by_keywords(&["[reflect-occ=other-class]".to_string()], 10)
        .expect("search other marker");
    assert!(
        miss.is_empty(),
        "a different marker must not match — marker recall stays exact"
    );
}

/// Case-insensitivity (the #2299 case-folding fix) is preserved by the
/// word-boundary path: a lowercased keyword matches ALL-CAPS whole-word content.
#[test]
fn whole_word_recall_is_case_insensitive() {
    let mem = test_mem();

    mem.store_episode("DEPLOYED THE AUTHENTICATION SERVICE", "engineer-loop", None)
        .expect("store episode");

    let hits = mem
        .search_episodes_by_keywords(&["authentication".to_string()], 5)
        .expect("search authentication");
    assert_eq!(
        hits.len(),
        1,
        "a lowercased keyword must match ALL-CAPS whole-word content (case-folding, #2299)"
    );
    assert!(hits[0].content.contains("AUTHENTICATION"));
}

/// Recall-precision regression: a lone sub-threshold (single-char) clean keyword
/// is recall noise — as a word-boundary PREFIX it matches every episode holding
/// a word that starts with that character. It must be dropped, while a genuine
/// multi-char keyword still recalls at a word boundary.
#[test]
fn lone_sub_threshold_keyword_recalls_nothing() {
    let mem = test_mem();
    mem.store_episode("the sync service restarted", "engineer-loop", None)
        .expect("store sync episode");
    mem.store_episode("storage layer migrated", "engineer-loop", None)
        .expect("store storage episode");

    let noise = mem
        .search_episodes_by_keywords(&["s".to_string()], 10)
        .expect("search lone s");
    assert!(
        noise.is_empty(),
        "a lone 's' keyword must not prefix-match every s-word episode, got {} hit(s)",
        noise.len()
    );

    let real = mem
        .search_episodes_by_keywords(&["sync".to_string()], 10)
        .expect("search sync");
    assert_eq!(
        real.len(),
        1,
        "a multi-char keyword still recalls at a word boundary"
    );
    assert!(real[0].content.contains("sync"));
}
