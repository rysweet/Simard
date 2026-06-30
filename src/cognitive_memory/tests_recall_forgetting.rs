//! TDD (RED) tests for PR-2 (issue #2440): ranked multi-signal recall +
//! forgetting signal.
//!
//! These tests are written **before** the production change and pin the
//! contract; they FAIL to compile / assert until PR-2 lands. The ranked-recall
//! infrastructure already exists (`recall_facts_ranked`, `RecallWeightSet`,
//! `reinforce_access`, `MemoryKind`); what #2440 still needs is:
//!
//!   * `pub fn forgetting_score(confidence: f64, usage_count: i64,
//!      last_accessed_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f64`
//!     in `crate::cognitive_memory` — a pure, bounded `[0.0, 1.0]` helper where a
//!     **higher** score means **more forgettable** (low recency × low
//!     importance/confidence × low usage). Single source of truth, reused by
//!     #2434's hygiene pass.
//!   * `CognitiveMemoryOps::recall_facts_ranked_reinforced(query, limit,
//!      min_confidence, weights)` — scores like `recall_facts_ranked` but, after
//!     scoring, reinforces (`reinforce_access`) the returned top-k only, so
//!     recall-intent reads feed the usage/recency signals on later cycles. The
//!     pure `recall_facts_ranked` stays non-reinforcing for structural reads.
//!   * `recall_procedure` ordered by `usage_count` DESC (then recency) instead of
//!     the flat library order.
//!
//! Until those symbols exist this module fails to build — the intended,
//! deterministic TDD red signal (mirrors `distillation_tests.rs`).

use super::{
    CognitiveMemoryOps, LibraryCognitiveMemory, MemoryKind, RecallWeightSet, forgetting_score,
};
use chrono::{Duration, Utc};

fn test_mem() -> LibraryCognitiveMemory {
    LibraryCognitiveMemory::in_memory().expect("in-memory DB should create")
}

// ─── forgetting_score: pure helper ───────────────────────────────────────────

/// A stale, low-confidence, never-used fact must be MORE forgettable than a
/// fresh, high-confidence, frequently-used one. Scores stay in `[0.0, 1.0]`.
#[test]
fn forgetting_score_ranks_stale_low_value_above_fresh_high_value() {
    let now = Utc::now();
    let fresh_high = forgetting_score(0.95, 25, Some(now - Duration::minutes(5)), now);
    let stale_low = forgetting_score(0.05, 0, None, now);

    assert!(
        stale_low > fresh_high,
        "stale low-value fact ({stale_low}) must out-forget fresh high-value ({fresh_high})"
    );
    assert!(
        (0.0..=1.0).contains(&fresh_high),
        "forgetting score must be bounded in [0,1]; got {fresh_high}"
    );
    assert!(
        (0.0..=1.0).contains(&stale_low),
        "forgetting score must be bounded in [0,1]; got {stale_low}"
    );
}

/// Recency protects: with confidence/usage held equal, an older last-access is
/// more forgettable than a recent one.
#[test]
fn recent_access_lowers_forgetting_score() {
    let now = Utc::now();
    let recent = forgetting_score(0.5, 3, Some(now - Duration::hours(1)), now);
    let old = forgetting_score(0.5, 3, Some(now - Duration::days(60)), now);
    assert!(
        old > recent,
        "older last-access ({old}) must be more forgettable than recent ({recent})"
    );
}

/// Usage protects: with confidence/recency held equal, a frequently-used fact is
/// less forgettable than an unused one.
#[test]
fn higher_usage_lowers_forgetting_score() {
    let now = Utc::now();
    let heavy = forgetting_score(0.5, 50, Some(now - Duration::days(10)), now);
    let light = forgetting_score(0.5, 0, Some(now - Duration::days(10)), now);
    assert!(
        light > heavy,
        "unused ({light}) must be more forgettable than frequently-used ({heavy})"
    );
}

// ─── reinforcing recall entry point (A3 / AC#2) ──────────────────────────────

/// `recall_facts_ranked` is a pure read (no reinforcement); the new
/// `recall_facts_ranked_reinforced` returns the same top-k AND bumps
/// `usage_count` + stamps `last_accessed_at` on exactly those returned facts.
#[test]
fn recall_facts_ranked_reinforced_bumps_returned_topk() {
    let mem = test_mem();
    mem.store_fact("deploy", "deploy the payment service", 0.9, &[], "src")
        .expect("store fact");

    // Pure ranked recall must NOT reinforce.
    let pure = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("pure ranked recall");
    assert_eq!(pure.len(), 1, "fact recalled");
    assert_eq!(
        pure[0].usage_count, 0,
        "pure recall must not reinforce usage"
    );
    assert!(
        pure[0].last_accessed_at.is_none(),
        "pure recall must not stamp last_accessed_at"
    );

    // Reinforcing recall returns the same top-k and feeds the signals.
    let reinforced = mem
        .recall_facts_ranked_reinforced("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("reinforcing recall");
    assert_eq!(reinforced.len(), 1, "reinforcing recall returns the top-k");

    let after = mem
        .recall_facts_ranked("deploy", 10, 0.0, RecallWeightSet::default())
        .expect("recall again");
    assert_eq!(
        after[0].usage_count, 1,
        "reinforcing recall must bump usage_count on the returned fact"
    );
    assert!(
        after[0].last_accessed_at.is_some(),
        "reinforcing recall must stamp last_accessed_at on the returned fact"
    );
}

// ─── procedure recall ordered by usage ───────────────────────────────────────

/// `recall_procedure` must order by `usage_count` DESC: the frequently-used
/// procedure ranks ahead of a cold one that matches the same query.
#[test]
fn recall_procedure_orders_by_usage_count() {
    let mem = test_mem();
    let steps = vec!["a".to_string(), "b".to_string()];
    let prereqs: Vec<String> = vec![];
    let cold = mem
        .store_procedure("deploy-cold-path", &steps, &prereqs)
        .expect("store cold");
    let hot = mem
        .store_procedure("deploy-hot-path", &steps, &prereqs)
        .expect("store hot");

    // Hot procedure used many times; cold once.
    for _ in 0..5 {
        mem.reinforce_access(&hot, MemoryKind::Procedure)
            .expect("reinforce hot");
    }
    mem.reinforce_access(&cold, MemoryKind::Procedure)
        .expect("reinforce cold");

    let hits = mem
        .recall_procedure("deploy", 10)
        .expect("recall procedures");
    assert!(hits.len() >= 2, "both procedures match the query");
    assert_eq!(
        hits[0].name, "deploy-hot-path",
        "most-used procedure must rank first"
    );
}

// ─── acceptance: ranked recall multi-signal ordering (#2440 AC#1) ─────────────

/// On (near-)equal text match, a recent + high-confidence + frequently-used fact
/// outranks a stale, low-confidence, unused one.
#[test]
fn recent_high_confidence_used_fact_outranks_stale_low_confidence() {
    let mem = test_mem();
    // Two facts of comparable text relevance to "kafka backpressure".
    let strong = mem
        .store_fact(
            "kafka",
            "kafka backpressure handling guide",
            0.95,
            &[],
            "s1",
        )
        .expect("store strong");
    mem.store_fact("kafka", "kafka backpressure tuning notes", 0.2, &[], "s2")
        .expect("store weak");

    // Strong fact is recent + frequently used.
    for _ in 0..5 {
        mem.reinforce_access(&strong, MemoryKind::Fact)
            .expect("reinforce strong");
    }

    let ranked = mem
        .recall_facts_ranked("kafka backpressure", 10, 0.0, RecallWeightSet::default())
        .expect("ranked recall");
    assert_eq!(ranked.len(), 2, "both facts returned");
    assert!(
        (ranked[0].confidence - 0.95).abs() < 1e-9,
        "the recent, high-confidence, frequently-used fact must rank first; got {}",
        ranked[0].confidence
    );
}
