//! TDD (RED) regression tests for issue #2298 — procedural-memory
//! non-idempotency defect ("procedural memory is frozen").
//!
//! ## Symptom
//!
//! Every OODA consolidation cycle re-stores the *identical* procedures
//! (`consolidate:ad-hoc`, `pr-merge:adopt-tdd`, `pr-merge:fix-broken-features`).
//! The daemon logs `OODA consolidation: stored procedure '…'` on every
//! cycle, compression stays at 0%, and only the 5 bootstrap procedures
//! are ever recalled because the store keeps growing duplicate nodes.
//!
//! ## Confirmed root cause
//!
//! [`LibraryCognitiveMemory::store_procedure`](super::ops) issues an
//! unconditional `CREATE (p:Procedure { id: <fresh new_id> … })` with no
//! dedup on `name`, and the `Procedure` table keys on `id` (not `name`),
//! so the DB does not dedup either. Calling it twice with the same
//! `(name, steps, prerequisites)` produces two nodes.
//!
//! The PR-B episode-distillation invariants (`mark_episode_distilled` /
//! `list_undistilled_episodes`, issue #2281) are already correct — these
//! tests guard them as a regression but do not change them.
//!
//! ## Expected RED signal (pre-fix)
//!
//! * `store_procedure_is_idempotent_on_exact_name` — recall count is 2,
//!   expected 1. FAILS.
//! * `store_procedure_returns_stable_id_for_duplicate` — the second call
//!   returns a different `proc_…` id. FAILS.
//! * `repeated_ooda_consolidation_does_not_re_store_procedures` — each of
//!   the three symptom procedures has 2 nodes after two cycles. FAILS.
//! * `consolidation_cycle_is_idempotent_and_keeps_episodes_distilled` —
//!   the procedure-count assertion fails (duplicates); the episode-mark
//!   assertions already pass (PR-B). FAILS overall on duplication.
//!
//! ## Expected GREEN signal (post-fix)
//!
//! `store_procedure` becomes an idempotent upsert keyed on exact `name`:
//! a second store with the same name creates no new node and returns the
//! existing id. All tests below pass; genuinely new names still create
//! new nodes (`store_procedure_preserves_distinct_named_procedures`).
//!
//! These tests target `LibraryCognitiveMemory::in_memory()` directly so
//! the native override — where the bug lives — is exercised without the
//! bridge/IPC/mock layers (whose stub `store_procedure` impls would hide
//! the defect).

use super::{CognitiveMemoryOps, LibraryCognitiveMemory};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

/// Count the procedures whose `name` is *exactly* `name`.
///
/// Uses the `"*"` wildcard recall (returns every procedure, `LIMIT`-only)
/// with a generous limit and then filters in Rust to exact equality. We
/// deliberately avoid `recall_procedure(name, …)` because its `CONTAINS`
/// matcher would also surface superstring names and mask duplicates.
fn count_exact(mem: &LibraryCognitiveMemory, name: &str) -> usize {
    mem.recall_procedure("*", 10_000)
        .expect("recall_procedure(\"*\") must succeed")
        .into_iter()
        .filter(|p| p.name == name)
        .count()
}

/// AC#1 — Re-storing a procedure with an identical name MUST NOT create a
/// second node. The store is an upsert keyed on exact `name`.
///
/// Pre-fix: `store_procedure` runs an unconditional `CREATE` with a fresh
/// id, so the wildcard recall returns two rows for the same name → the
/// `assert_eq!(…, 1)` fails. This is the primary RED gate.
#[test]
fn store_procedure_is_idempotent_on_exact_name() {
    let mem = test_mem();
    let name = "consolidate:ad-hoc | triggers: consolidate,memory,distill,g";
    let steps = [
        "assess working memory".to_string(),
        "distill episodes".to_string(),
    ];

    mem.store_procedure(name, &steps, &[]).expect("first store");
    mem.store_procedure(name, &steps, &[])
        .expect("second (identical) store");

    assert_eq!(
        count_exact(&mem, name),
        1,
        "storing the same-named procedure twice must leave exactly one \
         node (idempotent upsert on `name`); duplicate nodes are the \
         issue #2298 'frozen procedural memory' defect"
    );
}

/// AC#4 — `store_procedure` MUST return a *stable* id across duplicate
/// calls: the second store of an existing name returns the id of the
/// already-stored node rather than minting a new `proc_…` id.
///
/// Pre-fix: the second call mints a fresh `Self::new_id("proc")`, so the
/// two ids differ → the `assert_eq!` fails.
#[test]
fn store_procedure_returns_stable_id_for_duplicate() {
    let mem = test_mem();
    let name = "pr-merge:adopt-tdd | triggers: pr,merge,tdd,test";
    let steps = ["write failing test".to_string(), "implement".to_string()];

    let id_first = mem.store_procedure(name, &steps, &[]).expect("first store");
    let id_second = mem
        .store_procedure(name, &steps, &[])
        .expect("second (identical) store");

    assert_eq!(
        id_first, id_second,
        "a duplicate store must return the existing node id (got first={id_first:?}, \
         second={id_second:?}); a changing id proves a new node was created"
    );
}

/// AC#2 — The idempotency fix MUST NOT over-dedup: genuinely different
/// procedure names still accumulate as distinct nodes so the system can
/// keep learning. This guards against a fix that collapses everything.
///
/// Passes both pre- and post-fix; it is the safety rail that proves the
/// dedup key is *exact name* and nothing coarser.
#[test]
fn store_procedure_preserves_distinct_named_procedures() {
    let mem = test_mem();
    let name_a = "consolidate:ad-hoc | triggers: consolidate,memory,distill,g";
    let name_b = "pr-merge:fix-broken-features | triggers: pr,merge,fix,feature";

    mem.store_procedure(name_a, &["a1".to_string()], &[])
        .expect("store A");
    mem.store_procedure(name_b, &["b1".to_string()], &[])
        .expect("store B");

    assert_eq!(count_exact(&mem, name_a), 1, "procedure A must exist once");
    assert_eq!(count_exact(&mem, name_b), 1, "procedure B must exist once");

    let total = mem.recall_procedure("*", 10_000).expect("recall all").len();
    assert_eq!(
        total, 2,
        "two distinct names must produce two distinct nodes — the fix \
         must dedup on exact name, not collapse unrelated procedures"
    );
}

/// AC#1 (headline) — Faithful reproduction of the reported symptom: the
/// three procedures named in the daemon log are "stored" on two
/// successive consolidation cycles. After the second cycle each name must
/// still resolve to exactly one node.
///
/// Pre-fix: each `store_procedure` call appends a node, so every name has
/// two nodes after two cycles → all three `assert_eq!(…, 1)` checks fail.
#[test]
fn repeated_ooda_consolidation_does_not_re_store_procedures() {
    let mem = test_mem();

    // The exact procedure identities from the issue #2298 daemon log.
    let procedures: [(&str, [String; 2]); 3] = [
        (
            "consolidate:ad-hoc | triggers: consolidate,memory,distill,g",
            ["assess working memory".to_string(), "distill".to_string()],
        ),
        (
            "pr-merge:adopt-tdd | triggers: pr,merge,tdd,test",
            ["write failing test".to_string(), "implement".to_string()],
        ),
        (
            "pr-merge:fix-broken-features | triggers: pr,merge,fix,feature",
            ["diagnose".to_string(), "repair".to_string()],
        ),
    ];

    // Two identical OODA consolidation cycles, exactly as the daemon does.
    for _cycle in 0..2 {
        for (name, steps) in &procedures {
            mem.store_procedure(name, steps, &[])
                .expect("OODA procedural store");
        }
    }

    for (name, _) in &procedures {
        assert_eq!(
            count_exact(&mem, name),
            1,
            "procedure '{name}' must exist exactly once after two identical \
             consolidation cycles; >1 means the cycle is re-storing it every \
             pass (issue #2298, 0% compression)"
        );
    }

    let total = mem.recall_procedure("*", 10_000).expect("recall all").len();
    assert_eq!(
        total, 3,
        "exactly the three distinct procedures must be present after two \
         cycles, not six"
    );
}

/// Combined invariant (AC#1 + AC#3) — A consolidation pass that (a) stores
/// procedures via procedural learning and (b) marks the processed episodes
/// distilled must be idempotent on *both* axes when re-run on the same
/// inputs:
///
/// 1. the procedure store does not grow (no duplicate nodes), and
/// 2. the processed episodes stay marked distilled and never reappear in
///    `list_undistilled_episodes`.
///
/// Pre-fix: invariant (1) is violated — the procedure count doubles — so
/// the procedure assertion fails. Invariant (2) already holds (PR-B,
/// issue #2281) and is guarded here against future regression.
#[test]
fn consolidation_cycle_is_idempotent_and_keeps_episodes_distilled() {
    let mem = test_mem();

    // Seed an episode window that a consolidation cycle would process.
    let episode_ids: Vec<String> = (0..4)
        .map(|i| {
            mem.store_episode(&format!("episode {i}: did some work"), "test", None)
                .expect("store_episode")
        })
        .collect();

    let proc_name = "consolidate:ad-hoc | triggers: consolidate,memory,distill,g";
    let proc_steps = ["assess".to_string(), "distill".to_string()];

    // One consolidation cycle: learn the procedure, then mark every
    // episode in the window as distilled. Runs identically each cycle.
    let run_cycle = |mem: &LibraryCognitiveMemory| {
        mem.store_procedure(proc_name, &proc_steps, &[])
            .expect("procedural learning store");
        for id in &episode_ids {
            mem.mark_episode_distilled(id)
                .expect("mark_episode_distilled");
        }
    };

    run_cycle(&mem);
    run_cycle(&mem); // identical second cycle — must be a no-op everywhere

    // Invariant 1: procedural store did not grow.
    assert_eq!(
        count_exact(&mem, proc_name),
        1,
        "two identical consolidation cycles must leave exactly one \
         '{proc_name}' node; duplicates are the issue #2298 defect"
    );

    // Invariant 2: every processed episode is (and stays) distilled.
    let undistilled = mem
        .list_undistilled_episodes(100)
        .expect("list_undistilled_episodes");
    let undistilled_ids: std::collections::HashSet<&str> =
        undistilled.iter().map(|e| e.node_id.as_str()).collect();
    for id in &episode_ids {
        assert!(
            !undistilled_ids.contains(id.as_str()),
            "episode {id} was processed by the cycle and must remain \
             marked distilled (absent from list_undistilled_episodes); \
             still-undistilled rows would be re-distilled every cycle"
        );
    }
    assert!(
        undistilled.is_empty(),
        "all 4 processed episodes must be distilled after the cycle; \
         still undistilled: {undistilled_ids:?}"
    );
}

/// Reinforcement signal — the existing-name path is an idempotent *upsert*,
/// not a pure no-op. Re-storing a procedure with an identical name bumps the
/// stored node's `usage_count` so the recurrence signal that
/// `recall_procedure` ranks on is preserved across cycles. Node count stays
/// at one (guarded by the tests above); only the counter advances.
///
/// This locks the documented contract (see
/// `docs/reference/cognitive-memory-procedural-idempotency.md`): idempotency
/// is defined over the *node count*, deliberately not over `usage_count`.
#[test]
fn store_procedure_reinforces_usage_count_on_duplicate() {
    let mem = test_mem();
    let name = "consolidate:ad-hoc | triggers: consolidate,memory,distill,g";
    let steps = ["assess".to_string(), "distill".to_string()];

    mem.store_procedure(name, &steps, &[])
        .expect("first store (create)");
    mem.store_procedure(name, &steps, &[])
        .expect("second store (reinforce)");
    mem.store_procedure(name, &steps, &[])
        .expect("third store (reinforce)");

    let proc = mem
        .recall_procedure("*", 10_000)
        .expect("recall all")
        .into_iter()
        .find(|p| p.name == name)
        .expect("procedure must exist");

    assert_eq!(
        proc.usage_count, 2,
        "three identical stores = one create (usage_count 0) plus two \
         reinforcements (+1 each); usage_count must record the 2 recurrences \
         while the node count stays at exactly one"
    );
}
