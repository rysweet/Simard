//! Parity / conformance tests: `NativeCognitiveMemory` vs `LibraryCognitiveMemory`.
//!
//! De-fork Phase 2a (SAFE INTEGRATION), issue #86. Written **test-first**: the
//! native branch is the executable specification for the
//! [`CognitiveMemoryOps`](super::CognitiveMemoryOps) contract and runs on every
//! build. The library branch drives the **same** backend-agnostic scenarios
//! through a `Box<dyn CognitiveMemoryOps>` so the assertions are identical, and
//! is compiled only behind the opt-in `library-memory` cargo feature.
//!
//! ## TDD state
//!
//! Until the implementation step lands
//! `src/cognitive_memory/library_adapter.rs` (the `LibraryCognitiveMemory`
//! adapter), its `#[cfg(feature = "library-memory")] pub use` re-export in
//! `mod.rs`, and repoints the `amplihack-memory` dependency at the persistent
//! commit, the library branch is **RED** — `cargo test --features
//! library-memory` will not compile because `super::LibraryCognitiveMemory`
//! does not yet exist. The **default** build/test
//! (`cargo build` / `cargo test`, no feature) stays green: the gated module is
//! excluded and only the native scenarios compile and run.
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
//! * **Distillation gap (A5):** `mark_episode_distilled` /
//!   `list_undistilled_episodes` have no library equivalent at the pinned
//!   commit. `mark_episode_distilled` inherits the trait's *contractually safe
//!   no-op default*; `list_undistilled_episodes` is overridden to degrade
//!   *loudly* — it emits a one-time warning, then returns empty (it does **not**
//!   panic). `native_distillation_tracks_distilled_flag` documents the real
//!   native behavior; `distillation_gap_degrades_to_noop` (library-only) pins
//!   the empty/no-error degradation. Keyword/prefix episode recall
//!   (`search_episodes_by_keywords` / `search_episodes_starting_with`) is
//!   implemented in-adapter via recent-recall + filter, so it *is* exercised
//!   cross-backend.
//!
//! All filesystem use goes through `TempDir`; these tests never read, write, or
//! migrate the live daemon store at `~/.simard/cognitive_memory.ladybug`.

use super::{CognitiveMemoryOps, NativeCognitiveMemory};

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
/// data round-trips. Used by both backends with their respective on-disk
/// constructors (`NativeCognitiveMemory::open` / `LibraryCognitiveMemory::open`).
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
// Native backend — always compiled; the executable specification.
// ============================================================================

fn native_mem() -> NativeCognitiveMemory {
    NativeCognitiveMemory::in_memory().expect("native in-memory DB should create")
}

#[test]
fn native_store_and_search_fact() {
    scenario_store_and_search_fact(&native_mem());
}

#[test]
fn native_search_facts_min_confidence() {
    scenario_search_facts_min_confidence(&native_mem());
}

#[test]
fn native_store_and_recall_procedure() {
    scenario_store_and_recall_procedure(&native_mem());
}

#[test]
fn native_store_and_recall_episode() {
    scenario_store_and_recall_episode(&native_mem());
}

#[test]
fn native_trigger_first_fire() {
    scenario_trigger_first_fire(&native_mem());
}

#[test]
fn native_episode_consolidation() {
    scenario_episode_consolidation(&native_mem());
}

#[test]
fn native_statistics() {
    scenario_statistics(&native_mem());
}

/// Native distillation works (the real implementation the library backend
/// degrades away from). Documents the A5 gap by exercising the native side.
#[test]
fn native_distillation_tracks_distilled_flag() {
    let mem = native_mem();
    let id = mem
        .store_episode("distill me", "test", None)
        .expect("store_episode");

    let before = mem
        .list_undistilled_episodes(10)
        .expect("list_undistilled_episodes");
    assert!(
        before.iter().any(|e| e.node_id == id),
        "a freshly stored episode must start undistilled"
    );

    mem.mark_episode_distilled(&id)
        .expect("mark_episode_distilled");

    let after = mem
        .list_undistilled_episodes(10)
        .expect("list_undistilled_episodes after mark");
    assert!(
        after.iter().all(|e| e.node_id != id),
        "a distilled episode must drop out of the undistilled set"
    );
}

#[test]
#[serial_test::serial(cognitive_memory)]
fn native_persistence_across_reopen() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().to_path_buf();
    assert_persistence_round_trip(
        || Box::new(NativeCognitiveMemory::open(&path).unwrap()) as Box<dyn CognitiveMemoryOps>,
        || Box::new(NativeCognitiveMemory::open(&path).unwrap()) as Box<dyn CognitiveMemoryOps>,
    );
}

// ============================================================================
// Library backend — gated behind `library-memory`.
//
// RED until the implementation step adds `LibraryCognitiveMemory` (and its
// `#[cfg(feature = "library-memory")] pub use` re-export) and repoints the
// `amplihack-memory` dependency. Drives the identical scenarios so the contract
// is proven equivalent (or its divergences explicitly documented).
// ============================================================================

#[cfg(feature = "library-memory")]
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

    /// A5 gap: the library has no distilled mutation/filter API at the pinned
    /// commit. `mark_episode_distilled` inherits the trait's no-op default;
    /// `list_undistilled_episodes` is overridden to degrade *loudly* (one-time
    /// warning) but still returns empty. This pins the degradation:
    /// `mark_episode_distilled` must not error and `list_undistilled_episodes`
    /// returns empty — **not** a panic. Tracked upstream as amplihack-memory-lib#85.
    #[test]
    fn distillation_gap_degrades_to_noop() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mem = library_mem(&tmp);
        let id = mem
            .store_episode("distill me", "test", None)
            .expect("store_episode");

        mem.mark_episode_distilled(&id)
            .expect("mark_episode_distilled no-op must not error or panic");

        let undistilled = mem
            .list_undistilled_episodes(10)
            .expect("list_undistilled_episodes no-op must not error or panic");
        assert!(
            undistilled.is_empty(),
            "library backend degrades distillation to empty (loud-once); documented for #85"
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
}
