//! Outside-in regression gate for the "episodic recall returns zero" defect
//! (issue #2299).
//!
//! **Why this exists.** The unit tests in `src/cognitive_memory/ops.rs` pin the
//! low-level `search_episodes_by_keywords` contract. This file tests the
//! *user-observable* surface one layer up: the OODA preparation phase
//! (`preparation_memory_operations`) that an operator actually watches in the
//! logs. That function tokenises the objective, calls the keyword recall path,
//! applies the self-session noise filter, and emits the exact symptom line:
//!
//! ```text
//! [simard] preparation: N procedures, M episodes recalled (R raw, S session-filtered)
//! ```
//!
//! The defect made that line read `0 episodes recalled (0 raw, 0 session-filtered)`
//! every cycle because the Cypher `CONTAINS` clause was case-sensitive while
//! `tokenize_objective` lowercases every keyword, so a lowercased keyword never
//! matched mixed-case stored `content`. These tests drive the public crate API
//! exactly as the running agent does and assert the recall is non-empty.
//!
//! Run observably with:
//! ```bash
//! cargo test --test episodic_recall_outside_in -- --nocapture
//! ```
//! `--nocapture` surfaces the real `[simard] preparation: …` stderr line so the
//! fix is visible, not just asserted.

use simard::cognitive_memory::{CognitiveMemoryOps, NativeCognitiveMemory};
use simard::memory_consolidation::preparation_memory_operations;
use simard::session::SessionId;

/// Hermetic in-memory cognitive store. No disk, no env, no `$HOME` leakage —
/// the same backend the `ops.rs` unit tests use, so the Cypher recall path is
/// exercised for real.
fn mem() -> NativeCognitiveMemory {
    NativeCognitiveMemory::in_memory().expect("in-memory cognitive store should open")
}

/// Deterministic session id (literal UUID, no clock/PID derivation) so the
/// `session-` self-noise filter has a stable label to act on.
fn session() -> SessionId {
    SessionId::parse("session-00000000-0000-0000-0000-000000000001")
        .expect("literal session id should parse")
}

/// Scenario 1 (simple) — the basic user-facing behaviour the PR restores.
///
/// Store one mixed-case episode under a NON-`session-` label, then run the
/// preparation phase with an objective whose lowercased keywords overlap the
/// stored content. Before the fix this recalled zero episodes; after the fix
/// the episode comes back (raw count > 0).
#[test]
fn preparation_recalls_episode_case_insensitively() {
    let mem = mem();
    let session = session();

    mem.store_episode("Deploy the Authentication Service", "ooda-objective", None)
        .expect("store_episode should succeed");

    // Objective text as an operator would phrase it (mixed case). The
    // tokenizer lowercases it to [deploy, authentication, service]; the
    // recall path must match the mixed-case stored content.
    let ctx = preparation_memory_operations("Deploy the Authentication Service", &session, &mem)
        .expect("preparation should succeed");

    eprintln!(
        "[scenario-1] episodic_recall = {} episode(s)",
        ctx.episodic_recall.len()
    );

    assert!(
        !ctx.episodic_recall.is_empty(),
        "episodic recall must be non-empty (raw count > 0) — this is the #2299 defect"
    );
    assert_eq!(
        ctx.episodic_recall[0].content, "Deploy the Authentication Service",
        "the recalled episode must be the one that was stored"
    );
}

/// Scenario 2 (complex) — multi-keyword union, self-session noise filtering,
/// non-matching exclusion, and a no-false-match regression, all through the
/// same public preparation entry point.
///
/// Stored corpus (mixed case, verbatim):
///   * "Deploy the Authentication Service"      label ooda-objective   → kept
///   * "Configure the Payment Gateway"          label ooda-objective   → kept
///   * "Restart the Logging Daemon"             label ooda-objective   → excluded (no keyword)
///   * "Authentication retry on the payment hop" label session-…       → matched but session-filtered
///
/// Objective: "Investigate authentication and payment failures" →
/// tokens include `authentication` and `payment`. Three episodes match the
/// keywords (two ooda + one session), but the session-labelled one is dropped
/// as self-noise, so the prepared context keeps exactly the two ooda episodes.
#[test]
fn preparation_unions_keywords_filters_session_noise_and_excludes_nonmatches() {
    let mem = mem();
    let session = session();

    mem.store_episode("Deploy the Authentication Service", "ooda-objective", None)
        .unwrap();
    mem.store_episode("Configure the Payment Gateway", "ooda-objective", None)
        .unwrap();
    mem.store_episode("Restart the Logging Daemon", "ooda-objective", None)
        .unwrap();
    // Self-session echo: matches the `authentication`/`payment` keywords but
    // must be filtered out because its label starts with `session-`.
    mem.store_episode(
        "Authentication retry on the payment hop",
        "session-00000000-0000-0000-0000-000000000001",
        None,
    )
    .unwrap();

    let ctx = preparation_memory_operations(
        "Investigate authentication and payment failures",
        &session,
        &mem,
    )
    .expect("preparation should succeed");

    let mut kept: Vec<&str> = ctx
        .episodic_recall
        .iter()
        .map(|e| e.content.as_str())
        .collect();
    kept.sort_unstable();

    eprintln!("[scenario-2] kept episodes = {kept:?}");

    assert_eq!(
        kept,
        vec![
            "Configure the Payment Gateway",
            "Deploy the Authentication Service"
        ],
        "must union both keyword matches case-insensitively, drop the session-\
         labelled self-noise, and exclude the non-matching logging episode"
    );
    assert!(
        ctx.episodic_recall
            .iter()
            .all(|e| !e.source_label.starts_with("session-")),
        "no session-labelled self-noise episode may survive the recall filter"
    );

    // Regression: an objective whose (valid, >=3-char) tokens match nothing
    // must recall zero — the fix must not degrade into a match-all.
    let none = preparation_memory_operations("Xyzzy plugh quux corge", &session, &mem)
        .expect("preparation should succeed for a non-matching objective");
    eprintln!(
        "[scenario-2] non-matching objective recalled {} episode(s)",
        none.episodic_recall.len()
    );
    assert!(
        none.episodic_recall.is_empty(),
        "a non-matching objective must recall zero episodes (no match-all regression)"
    );
}
