//! Conformance tests for [`LibraryCognitiveMemory`](super::LibraryCognitiveMemory),
//! the sole cognitive-memory backend (de-fork Phase 2b, issue #2307).
//!
//! The backend-agnostic `scenario_*` helpers below encode the
//! [`CognitiveMemoryOps`](super::CognitiveMemoryOps) contract; the `library`
//! module drives every one of them through a `Box<dyn CognitiveMemoryOps>`
//! backed by the library adapter. Originally written test-first against the
//! now-deleted native backend (Phase 2a, #86); the native runner has been
//! removed along with the fork.
//!
//! ## What is asserted (and what is not)
//!
//! Ids may differ between backends, so the scenarios assert on **counts,
//! content, and search hits — never on id strings**. Per the approved Phase 2a
//! design (`docs/architecture/cognitive-memory-library-adapter.md`), legitimate
//! behavioral differences are documented and asserted in their *tolerant* form
//! rather than forcing native semantics into the adapter. These divergences all
//! feed amplihack-memory-lib#85:
//!
//! * **`check_triggers` (A3):** native is case-sensitive whole-substring,
//!   read-only, and re-fires on every call; the library is tokenized/lowercased
//!   keyword-overlap, mutates the matched status to `"triggered"`, and fires
//!   once. The scenario asserts only the **first-fire** equivalence (a matching
//!   prospective fires; non-matching content does not). It never calls
//!   `check_triggers` twice on the same instance, so the re-fire / case /
//!   tokenization differences are documented, not asserted.
//! * **`consolidate_episodes` (A6):** native marks sources `compressed = 1` in
//!   place (`epi`-id); the library creates a separate `ConsolidatedEpisode`
//!   (`con`-id). This changes `episodic_count`. The scenario asserts only that a
//!   consolidation **artifact is produced** and the source episodes remain
//!   recallable; the count/id-scheme differences are not asserted.
//! * **`get_statistics` (A7):** the library returns `HashMap<String, usize>`
//!   folded into the typed DTO; keys it does not emit default to `0`. The
//!   scenario asserts only the four fields **both** backends populate
//!   (`semantic`, `procedural`, `prospective`, `episodic`) and deliberately does
//!   not assert `sensory_count` / `working_count`.
//! * **Distillation (A5 — re-enabled in de-fork Phase 2b, issue #2307):** the
//!   library now exposes `mark_episode_distilled` / `list_undistilled_episodes`
//!   with a persistent `distilled` flag, so the adapter DELEGATES to them
//!   instead of degrading to a no-op. `distillation_round_trip`,
//!   `list_undistilled_newest_first_and_respects_limit`, and
//!   `distillation_persists_across_reopen` (library-only) pin the re-enabled
//!   behavior: freshly stored episodes are undistilled, marking excludes an
//!   episode from the undistilled set, the listing is newest-first and honours
//!   `limit`, and the flag survives checkpoint + reopen. Keyword/prefix episode
//!   recall (`search_episodes_by_keywords` / `search_episodes_starting_with`) is
//!   implemented in-adapter via recent-recall + filter, so it *is* exercised
//!   cross-backend.
//!
//! All filesystem use goes through `TempDir`; these tests never read, write, or
//! migrate the live daemon store at `~/.simard/cognitive`.

use super::CognitiveMemoryOps;

// ============================================================================
// Backend-agnostic conformance scenarios
//
// Each takes `&dyn CognitiveMemoryOps` and encodes one slice of the contract.
// The native tests below drive every one of these (so they are never dead
// code in the default build); the gated library tests drive the identical set.
// ============================================================================

/// Store a fact, then recall it by concept keyword.
fn scenario_store_and_search_fact(mem: &dyn CognitiveMemoryOps) {
    let id = mem
        .store_fact("rust", "systems language", 0.9, &[], "test")
        .expect("store_fact");
    assert!(!id.is_empty(), "store_fact must return a non-empty id");

    let facts = mem.search_facts("rust", 10, 0.0).expect("search_facts");
    assert_eq!(facts.len(), 1, "exactly one fact should match 'rust'");
    assert_eq!(facts[0].concept, "rust");
    assert_eq!(facts[0].content, "systems language");
    assert!(
        (facts[0].confidence - 0.9).abs() < f64::EPSILON,
        "confidence must round-trip"
    );
}

/// `min_confidence` must drop facts below the threshold.
fn scenario_search_facts_min_confidence(mem: &dyn CognitiveMemoryOps) {
    mem.store_fact("low", "low confidence note", 0.10, &[], "test")
        .expect("store low fact");
    mem.store_fact("high", "high confidence note", 0.90, &[], "test")
        .expect("store high fact");

    let hits = mem
        .search_facts("confidence", 10, 0.5)
        .expect("search_facts with min_confidence");
    assert_eq!(hits.len(), 1, "min_confidence 0.5 must drop the 0.10 fact");
    assert_eq!(hits[0].concept, "high");
}

/// Store a procedure, then recall it by name; steps must be preserved verbatim.
fn scenario_store_and_recall_procedure(mem: &dyn CognitiveMemoryOps) {
    let steps = vec!["compile".to_string(), "test".to_string()];
    let id = mem
        .store_procedure("build", &steps, &[])
        .expect("store_procedure");
    assert!(!id.is_empty(), "store_procedure must return a non-empty id");

    let procs = mem.recall_procedure("build", 5).expect("recall_procedure");
    assert_eq!(procs.len(), 1, "exactly one procedure should match 'build'");
    assert_eq!(procs[0].name, "build");
    assert_eq!(procs[0].steps, steps, "steps must be preserved");
}

/// Store episodes, then recall them by keyword. Lowercase keyword/content keeps
/// the documented case-sensitivity divergence (A5) out of this assertion.
fn scenario_store_and_recall_episode(mem: &dyn CognitiveMemoryOps) {
    mem.store_episode("alpha event one", "test", None)
        .expect("store alpha episode");
    mem.store_episode("beta event two", "test", None)
        .expect("store beta episode");

    let alpha = mem
        .search_episodes_by_keywords(&["alpha".to_string()], 10)
        .expect("search_episodes_by_keywords(alpha)");
    assert!(
        !alpha.is_empty(),
        "an episode containing 'alpha' must be recallable by keyword"
    );
    assert!(
        alpha.iter().all(|e| e.content.contains("alpha")),
        "keyword recall must only return content matching 'alpha', got {:?}",
        alpha.iter().map(|e| &e.content).collect::<Vec<_>>()
    );
    assert!(
        alpha.iter().any(|e| e.content == "alpha event one"),
        "the stored 'alpha event one' episode must be among the hits"
    );

    let beta = mem
        .search_episodes_by_keywords(&["beta".to_string()], 10)
        .expect("search_episodes_by_keywords(beta)");
    assert!(
        beta.iter().any(|e| e.content == "beta event two"),
        "the stored 'beta event two' episode must be recallable"
    );
}

/// First-fire trigger equivalence only (A3): a matching prospective fires once;
/// non-matching content does not. Re-fire / case / tokenization are NOT asserted.
fn scenario_trigger_first_fire(mem: &dyn CognitiveMemoryOps) {
    mem.store_prospective("watch errors", "error", "alert", 5)
        .expect("store_prospective");

    let fired = mem
        .check_triggers("an error occurred")
        .expect("check_triggers(match)");
    assert!(
        fired.iter().any(|p| p.description == "watch errors"),
        "a matching prospective must fire on the first check_triggers"
    );

    let quiet = mem
        .check_triggers("all good")
        .expect("check_triggers(no match)");
    assert!(
        quiet.iter().all(|p| p.description != "watch errors"),
        "non-matching content must not fire the 'watch errors' trigger"
    );
}

/// Consolidation produces an artifact and leaves sources recallable (A6). The
/// `episodic_count` and id scheme legitimately differ and are NOT asserted.
fn scenario_episode_consolidation(mem: &dyn CognitiveMemoryOps) {
    for i in 0..4 {
        mem.store_episode(&format!("alpha consolidation source {i}"), "test", None)
            .expect("store source episode");
    }

    let summary_id = mem.consolidate_episodes(10).expect("consolidate_episodes");
    assert!(
        summary_id.is_some(),
        "consolidating >= 2 episodes must yield a summary artifact"
    );

    let sources = mem
        .search_episodes_by_keywords(&["alpha".to_string()], 10)
        .expect("recall sources after consolidation");
    assert!(
        !sources.is_empty(),
        "source episodes must remain recallable after consolidation"
    );
}

/// Statistics for the four fields both backends populate (A7). `sensory_count`
/// and `working_count` are deliberately not asserted.
fn scenario_statistics(mem: &dyn CognitiveMemoryOps) {
    mem.store_fact("rust", "systems language", 0.9, &[], "test")
        .expect("store_fact");
    mem.store_procedure("build", &["compile".to_string()], &[])
        .expect("store_procedure");
    mem.store_prospective("watch", "error", "alert", 1)
        .expect("store_prospective");
    mem.store_episode("one episode", "test", None)
        .expect("store_episode");

    let stats = mem.get_statistics().expect("get_statistics");
    assert_eq!(stats.semantic_count, 1, "one fact stored");
    assert_eq!(stats.procedural_count, 1, "one procedure stored");
    assert_eq!(stats.prospective_count, 1, "one prospective stored");
    assert_eq!(stats.episodic_count, 1, "one episode stored");
}

/// Write through `create()`, drop, reopen through `reopen()`, and assert the
/// data round-trips. Used with the library backend's on-disk constructor
/// (`LibraryCognitiveMemory::open`).
fn assert_persistence_round_trip<C, R>(create: C, reopen: R)
where
    C: FnOnce() -> Box<dyn CognitiveMemoryOps>,
    R: FnOnce() -> Box<dyn CognitiveMemoryOps>,
{
    {
        let mem = create();
        mem.store_fact("rust", "systems language", 0.95, &[], "test")
            .expect("store_fact before reopen");
        mem.store_procedure(
            "release",
            &[
                "compile".to_string(),
                "test".to_string(),
                "deploy".to_string(),
            ],
            &[],
        )
        .expect("store_procedure before reopen");
        // Flush before drop so the reopen below observes the writes. Both
        // backends now implement `checkpoint` meaningfully (native issues a WAL
        // CHECKPOINT; the library flushes via `close`), so a failure here is a
        // real durability bug — assert it rather than discarding it.
        mem.checkpoint().expect("checkpoint before reopen");
    } // drop closes the store

    let mem = reopen();
    let facts = mem
        .search_facts("rust", 10, 0.0)
        .expect("search_facts after reopen");
    assert_eq!(facts.len(), 1, "fact must survive reopen");
    assert_eq!(facts[0].concept, "rust");
    assert_eq!(facts[0].content, "systems language");

    let procs = mem
        .recall_procedure("release", 5)
        .expect("recall_procedure after reopen");
    assert_eq!(procs.len(), 1, "procedure must survive reopen");
    assert_eq!(procs[0].name, "release");
    assert_eq!(
        procs[0].steps,
        vec![
            "compile".to_string(),
            "test".to_string(),
            "deploy".to_string()
        ],
        "procedure steps must survive reopen"
    );
}

// ============================================================================
// Conformance tests — library backend (the sole backend, de-fork Phase 2b).
//
// Drives the backend-agnostic scenarios above through `LibraryCognitiveMemory`,
// the only `CognitiveMemoryOps` implementation after the native fork's deletion.
// ============================================================================

mod library {
    use super::{
        assert_persistence_round_trip, scenario_episode_consolidation,
        scenario_search_facts_min_confidence, scenario_statistics,
        scenario_store_and_recall_episode, scenario_store_and_recall_procedure,
        scenario_store_and_search_fact, scenario_trigger_first_fire,
    };
    use crate::cognitive_memory::{CognitiveMemoryOps, LibraryCognitiveMemory};

    /// `LibraryCognitiveMemory::open(state_root)` opens a persistent library
    /// store under a dedicated sub-path of `state_root` (a `TempDir` here). It
    /// never touches `~/.simard`.
    fn library_mem(tmp: &tempfile::TempDir) -> LibraryCognitiveMemory {
        LibraryCognitiveMemory::open(tmp.path()).expect("open library adapter at TempDir")
    }

    #[test]
    fn store_and_search_fact() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_store_and_search_fact(&library_mem(&tmp));
    }

    #[test]
    fn search_facts_min_confidence() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_search_facts_min_confidence(&library_mem(&tmp));
    }

    #[test]
    fn store_and_recall_procedure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_store_and_recall_procedure(&library_mem(&tmp));
    }

    #[test]
    fn store_and_recall_episode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_store_and_recall_episode(&library_mem(&tmp));
    }

    #[test]
    fn trigger_first_fire() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_trigger_first_fire(&library_mem(&tmp));
    }

    #[test]
    fn episode_consolidation() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_episode_consolidation(&library_mem(&tmp));
    }

    #[test]
    fn statistics() {
        let tmp = tempfile::tempdir().expect("tempdir");
        scenario_statistics(&library_mem(&tmp));
    }

    /// De-fork Phase 2b (issue #2307): the library now tracks the `distilled`
    /// flag, so the adapter delegates instead of degrading. A freshly stored
    /// episode starts undistilled; marking it excludes it from the undistilled
    /// set while leaving its siblings.
    #[test]
    fn distillation_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = library_mem(&tmp);

        let id_a = mem
            .store_episode("alpha", "test", None)
            .expect("store alpha");
        let id_b = mem.store_episode("beta", "test", None).expect("store beta");
        let id_c = mem
            .store_episode("gamma", "test", None)
            .expect("store gamma");

        let before: std::collections::HashSet<String> = mem
            .list_undistilled_episodes(10)
            .expect("list undistilled")
            .into_iter()
            .map(|e| e.node_id)
            .collect();
        assert!(before.contains(&id_a), "alpha must start undistilled");
        assert!(before.contains(&id_b), "beta must start undistilled");
        assert!(before.contains(&id_c), "gamma must start undistilled");

        mem.mark_episode_distilled(&id_b)
            .expect("mark beta distilled");

        let after: std::collections::HashSet<String> = mem
            .list_undistilled_episodes(10)
            .expect("list undistilled after mark")
            .into_iter()
            .map(|e| e.node_id)
            .collect();
        assert!(after.contains(&id_a), "alpha must remain undistilled");
        assert!(
            !after.contains(&id_b),
            "beta must be excluded after mark_episode_distilled"
        );
        assert!(after.contains(&id_c), "gamma must remain undistilled");
    }

    /// `list_undistilled_episodes` returns newest-first (temporal index
    /// descending) and honours `limit`.
    #[test]
    fn list_undistilled_newest_first_and_respects_limit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = library_mem(&tmp);

        mem.store_episode("first", "test", None)
            .expect("store first");
        mem.store_episode("second", "test", None)
            .expect("store second");
        mem.store_episode("third", "test", None)
            .expect("store third");

        let all = mem.list_undistilled_episodes(10).expect("list undistilled");
        let contents: Vec<&str> = all.iter().map(|e| e.content.as_str()).collect();
        assert_eq!(
            contents,
            vec!["third", "second", "first"],
            "undistilled episodes must be newest-first by temporal index"
        );

        let limited = mem
            .list_undistilled_episodes(2)
            .expect("list undistilled limited");
        assert_eq!(limited.len(), 2, "limit=2 must cap the result at 2 rows");
        assert_eq!(
            limited[0].content, "third",
            "the newest episode must come first even when the list is limited"
        );
    }

    /// The `distilled` flag is durable: it survives `checkpoint` + reopen of the
    /// persistent library store.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn distillation_persists_across_reopen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        let distilled_id = {
            let mem = LibraryCognitiveMemory::open(&root).expect("open library store");
            mem.store_episode("keep me", "test", None)
                .expect("store keep");
            let distill = mem
                .store_episode("distill me", "test", None)
                .expect("store distill");
            mem.mark_episode_distilled(&distill)
                .expect("mark distilled");
            mem.checkpoint().expect("checkpoint before reopen");
            distill
        }; // drop closes the store

        let mem = LibraryCognitiveMemory::open(&root).expect("reopen library store");
        let undistilled: Vec<String> = mem
            .list_undistilled_episodes(10)
            .expect("list undistilled after reopen")
            .into_iter()
            .map(|e| e.node_id)
            .collect();
        assert!(
            !undistilled.contains(&distilled_id),
            "the distilled flag must persist across reopen (distilled episode stays excluded)"
        );
        assert_eq!(
            undistilled.len(),
            1,
            "exactly the one still-undistilled episode must remain after reopen"
        );
    }

    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn persistence_across_reopen() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        assert_persistence_round_trip(
            || {
                Box::new(LibraryCognitiveMemory::open(&root).unwrap())
                    as Box<dyn CognitiveMemoryOps>
            },
            || {
                Box::new(LibraryCognitiveMemory::open(&root).unwrap())
                    as Box<dyn CognitiveMemoryOps>
            },
        );
    }

    // ========================================================================
    // Issue #2798 — creative-idea prospective persistence: engine read-after-write
    // (Layer A) and engine write-through durability (Layer C).
    //
    // The dashboard "Creative Ideas" tab was ALWAYS EMPTY even though the thread
    // reported "10 persisted" each run. These two tests pin the ENGINE half of
    // the diagnosis (memory-architecture policy G2): they prove the lbug-backed
    // `LibraryCognitiveMemory` engine already (A) serves a freshly-stored
    // creative-idea prospective row to a *separate* on-disk view and (C) retains
    // it durably across a non-graceful reopen WITH NO EXPLICIT CHECKPOINT —
    // because the engine's WAL is write-through and replayed on open. Both pass
    // on the pinned engine, so the always-empty bug is NOT an engine defect and
    // needs NO `amplihack-memory-lib` change: it lives entirely in the one
    // Simard-side seam that diverged — the dashboard state-root resolver (D1),
    // which made the reader open a *different* store than the daemon wrote to.
    //
    // Layer C also refutes the initial "prospective writes are buffer-only and
    // lost on SIGKILL unless the thread checkpoints each batch" hypothesis: they
    // are not buffer-only, so no per-batch checkpoint is added to the thread
    // (ruthless simplicity — the minimal correct fix is D1 alone). If Layer A or
    // C ever fails RED, durability HAS regressed in the engine and the fix
    // escalates to `amplihack-memory-lib` (Simard bumps its pinned dep), never a
    // Simard-side fork or a compensating checkpoint.
    //
    // See src/operator_commands_dashboard/tests_state_root_parity.rs for Layer B
    // (the reader-seam / resolver-divergence regression that DOES fail RED on the
    // unpatched bug).
    // ========================================================================

    use crate::cognitive_memory::creative_idea::{
        CREATIVE_IDEA_TRIGGER, CreativeIdea, CreativeIdeaStore, IdeaContext,
        ProspectiveCreativeIdeaStore,
    };

    /// A minimal provenance context for a synthetic creative idea.
    fn idea_ctx() -> IdeaContext {
        IdeaContext {
            source: "creative-ideas-thread".to_string(),
            goals_snapshot: vec![],
            observation_digest: "digest".to_string(),
            rationale: "recall precision plateaued".to_string(),
        }
    }

    /// Count creative-idea rows (filtered by the retrieval sentinel) visible to
    /// a store view over `mem`.
    fn creative_idea_count(mem: &dyn CognitiveMemoryOps) -> usize {
        ProspectiveCreativeIdeaStore::new(mem)
            .list(u32::MAX)
            .expect("list creative ideas")
            .len()
    }

    /// **Layer A — engine read-after-write (G2 boundary).** A creative idea
    /// persisted through `ProspectiveCreativeIdeaStore::store` (which calls the
    /// engine's `store_prospective` under the `CREATIVE_IDEA_TRIGGER` sentinel)
    /// must be immediately observable both on the same handle AND through a
    /// *separate* on-disk view opened on the same `state_root` — the engine does
    /// not hide its own un-checkpointed prospective writes from a fresh reader.
    ///
    /// This is the control that proves the always-empty tab is a Simard-side
    /// seam bug, not an engine bug: if this test is GREEN the engine is fine and
    /// no `amplihack-memory-lib` change is warranted (see the module note).
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn creative_idea_read_after_write_visible_to_separate_view() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        // Writer handle (the daemon's live writer, in spirit).
        let writer = LibraryCognitiveMemory::open(&root).expect("open writer");
        let store = ProspectiveCreativeIdeaStore::new(&writer);
        let mut idea = CreativeIdea::new("improve recall ranking", idea_ctx(), 1);
        idea.node_id = store.store(&idea).expect("store creative idea");
        assert!(!idea.node_id.is_empty(), "store must return a node id");

        // Same-handle read-after-write.
        assert_eq!(
            creative_idea_count(&writer),
            1,
            "the writer handle must see its own freshly-persisted creative idea"
        );

        // A SEPARATE on-disk view (the dashboard reader, in spirit) opened while
        // the writer is still alive must also observe the idea — the engine
        // serves un-checkpointed prospective writes to a fresh reader, so a
        // divergent view on the SAME path is never the source of the empty tab.
        let reader = LibraryCognitiveMemory::open(&root).expect("open separate reader view");
        assert_eq!(
            creative_idea_count(&reader),
            1,
            "a separate on-disk view on the same state_root must observe the \
             persisted creative idea (engine read-after-write holds — G2 boundary)"
        );
        let raw = reader
            .list_all_prospective(u32::MAX)
            .expect("list_all_prospective");
        assert!(
            raw.iter()
                .any(|n| n.trigger_condition == CREATIVE_IDEA_TRIGGER),
            "the persisted row must carry the CREATIVE_IDEA_TRIGGER retrieval sentinel"
        );
    }

    /// **Layer C — engine write-through durability across a non-graceful
    /// restart (#2798).** A creative idea is persisted and then the writer suffers
    /// a *non-graceful* process exit: we `std::mem::forget` the handle so its
    /// `Drop` (which would otherwise run an implicit graceful checkpoint) never
    /// fires, clear the tier-2 store cache, and cold-reopen from disk. The idea
    /// must still be listable — with **no explicit `checkpoint()` anywhere** —
    /// because the engine's WAL is write-through and replayed on open.
    ///
    /// The `forget` is what makes this a real `SIGKILL`-during-deploy simulation
    /// rather than a false GREEN off a graceful-drop checkpoint. This pins the
    /// engine property the whole diagnosis rests on: prospective creative-idea
    /// writes are durable by themselves, so the always-empty tab was never a
    /// durability defect (it was the D1 resolver divergence) and the thread adds
    /// no compensating per-batch checkpoint. If this fails RED, WAL write-through
    /// has regressed in the engine — a `amplihack-memory-lib` fix (G2), not a
    /// Simard-side checkpoint.
    #[test]
    #[serial_test::serial(cognitive_memory)]
    fn creative_idea_survives_nongraceful_restart_without_checkpoint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();

        {
            let writer = LibraryCognitiveMemory::open(&root).expect("open writer");
            let store = ProspectiveCreativeIdeaStore::new(&writer);
            let idea = CreativeIdea::new("auto-delete stale worktrees", idea_ctx(), 7);
            store.store(&idea).expect("store creative idea");

            // Simulate a SIGKILL: skip the graceful Drop (and its checkpoint)
            // entirely. NO explicit checkpoint — durability must come from the
            // engine's write-through WAL alone.
            std::mem::forget(writer);
        }

        // Force a genuine cold reopen from disk (no shared cached handle).
        crate::memory_ipc::clear_tier2_store_cache();
        let _ = crate::memory_ipc::reap_stale_open_lock(&root);

        let reopened = LibraryCognitiveMemory::open(&root).expect("cold reopen after restart");
        assert_eq!(
            creative_idea_count(&reopened),
            1,
            "a persisted creative idea must survive a non-graceful daemon restart \
             via the engine's write-through WAL with no explicit checkpoint \
             (persist -> SIGKILL -> reopen -> list is non-empty)"
        );
    }
}
