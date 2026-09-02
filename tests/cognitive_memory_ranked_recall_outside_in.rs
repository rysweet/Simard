//! Outside-in (black-box) integration tests for the cognitive-memory goal.
//! Originating issues: #2440, #2434, and #2468.
//!
//! This file lives in `tests/` so it compiles as an EXTERNAL consumer of the
//! `simard` crate: it touches only the PUBLIC API surface, exactly as the
//! daemon (or any downstream user) does. It never reaches into private
//! internals or `#[cfg(test)]`-only helpers — it exercises the same entry
//! points a real caller (OODA prep, consolidation cadence, distill scheduler)
//! uses.
//!
//! Behaviours, each a `#[test]`:
//!
//!   1. (#2440) Recall is ACTUALLY ranked by weighted multi-signal scoring —
//!      not a plain whole-string CONTAINS keyword match. Proven by flipping the
//!      top result purely by changing the `RecallWeightSet`.
//!   2. (#2434 + #2440) Controlled forgetting of LIVE facts: a low-value fact
//!      fades while a high-confidence fact, a provenance-bearing fact, and a
//!      recall-reinforced low-confidence fact are all protected; the dry-run
//!      preview changes nothing.
//!   3. (#2468, superseded by #2679) Distillation is retry-SAFE: a transient
//!      failure marks NOTHING and leaves the whole batch retry-eligible, while a
//!      successful pass distils and marks the batch in one single-invocation
//!      pass. (#2468's in-cycle *parse-miss* retry was removed by #2679's
//!      agentic direct-write — nothing is parsed — but the retry-safety
//!      invariant it protected endures.)
//!
//! The forgetting and distill behaviours drive the distill/self-metrics path,
//! which records metrics under `$HOME/.simard/metrics/` outside `#[cfg(test)]`.
//! Those two tests scope `HOME` to a per-test `TempDir` via an RAII guard and
//! are serialised under the crate's `cognitive_memory` env-mutation key so the
//! `set_var`/`remove_var` calls never race another test's HOME.

use std::cell::Cell;

use serial_test::serial;
use simard::cognitive_memory::{
    CognitiveMemoryOps, ForgetReport, LibraryCognitiveMemory, MemoryKind, RecallWeightSet,
    forgetting_score,
};
use simard::error::{SimardError, SimardResult};
use simard::memory_cognitive::CognitiveEpisode;
use simard::memory_consolidation::distillation::{
    DistillRecipeRunner, DistilledFact, distill_recent_episodes_with_runner,
};
use tempfile::TempDir;

/// RAII guard that redirects `HOME` to a fresh `TempDir` and restores the prior
/// value on drop (including panic unwind). Must be paired with
/// `#[serial(cognitive_memory)]` because it mutates process-global env.
struct HomeGuard {
    _tmp: TempDir,
    prev: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let tmp = TempDir::new().expect("scratch HOME");
        let prev = std::env::var_os("HOME");
        // SAFETY: env mutation is serialised by `#[serial(cognitive_memory)]`;
        // the prior value is restored in `Drop`.
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }
        Self { _tmp: tmp, prev }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: restore under the same serial key; runs on normal return and
        // on panic unwind.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

/// All live (non-archived) fact concepts, via the public wildcard recall path.
fn live_concepts(mem: &LibraryCognitiveMemory) -> Vec<String> {
    mem.search_facts("*", 256, 0.0)
        .expect("wildcard recall")
        .into_iter()
        .map(|f| f.concept)
        .collect()
}

/// Ranked multi-signal recall (#2440).
///
/// Pre-#2440, `search_facts` was a plain CONTAINS keyword match — it could not
/// rank by relevance and was blind to the declared `RecallWeightSet` signals.
/// The decisive, implementation-agnostic proof that recall is now genuinely
/// *ranked by weighted signals* is that the SAME query over the SAME two facts
/// (identical text, so text-relevance is held equal) returns a DIFFERENT top
/// result when only the weights change:
///
///   * Confidence-only weights → the high-confidence fact wins.
///   * Usage-only weights      → the recall-reinforced (high-usage) fact wins.
///
/// A plain CONTAINS match would ignore weights entirely and never reorder.
///
/// Pure in-memory; no HOME/metrics side-effects, so no serial/guard needed.
#[test]
fn recall_is_ranked_by_weighted_signals() {
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    // Identical text → equal text-relevance. They differ ONLY in the non-text
    // signals (confidence and usage) so the ranking flip is unambiguously
    // attributable to the weighted multi-signal scorer.
    let phrase = "shared deployment rollback runbook";
    mem.store_fact("alpha-confident", phrase, 0.90, &[], "s-alpha")
        .unwrap();
    let beta = mem
        .store_fact("beta-reinforced", phrase, 0.20, &[], "s-beta")
        .unwrap();
    // Recall-reinforce ONLY beta (precise, per-fact) so usage/recency rise.
    for _ in 0..10 {
        mem.reinforce_access(&beta, MemoryKind::Fact)
            .expect("reinforce beta");
    }

    let query = phrase;
    let conf_only = RecallWeightSet {
        text_relevance: 0.0,
        confidence: 1.0,
        importance: 0.0,
        recency: 0.0,
        usage: 0.0,
        graph: 0.0,
    };
    let usage_only = RecallWeightSet {
        text_relevance: 0.0,
        confidence: 0.0,
        importance: 0.0,
        recency: 0.0,
        usage: 1.0,
        graph: 0.0,
    };

    let by_conf = mem.recall_facts_ranked(query, 10, 0.0, conf_only).unwrap();
    let by_usage = mem.recall_facts_ranked(query, 10, 0.0, usage_only).unwrap();

    let conf_top = by_conf.first().map(|f| f.concept.as_str()).unwrap_or("∅");
    let usage_top = by_usage.first().map(|f| f.concept.as_str()).unwrap_or("∅");

    assert_eq!(
        by_conf.len(),
        2,
        "both facts must be recalled under confidence weights"
    );
    assert_eq!(
        by_usage.len(),
        2,
        "both facts must be recalled under usage weights"
    );
    assert_eq!(
        conf_top, "alpha-confident",
        "confidence-only weights must rank the high-confidence fact first"
    );
    assert_eq!(
        usage_top, "beta-reinforced",
        "usage-only weights must rank the recall-reinforced fact first"
    );
    assert_ne!(
        conf_top, usage_top,
        "ranking must respond to weights (order flips) — a CONTAINS match never reorders (#2440)"
    );
}

/// Controlled forgetting of live facts (#2434), with the recall→forgetting
/// protection loop (#2440).
///
/// Four live facts share a store; only the genuinely low-value one must fade:
///   * `noise-decay`   — conf 0.05, no provenance, never recalled → FORGET.
///   * `core-durable`  — conf 0.95 → protected (above the forgetting floor).
///   * `grounded-prov` — conf 0.05 BUT provenance-bearing → protected (safety).
///   * `warm-recalled` — conf 0.05 BUT recall-reinforced → protected (#2440 loop).
///
/// The dry-run preview must report the candidate yet change nothing; the live
/// run must forget exactly the one low-value fact and leave the other three.
///
/// Drives the self-metrics path → redirect HOME + serialise env mutation.
#[test]
#[serial(cognitive_memory)]
fn forgetting_protects_high_value_and_reinforced_facts() {
    let _home = HomeGuard::new();
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    // Sanity-pin the public forgetting signal: a stale, low-confidence,
    // never-used fact must out-forget a fresh, high-confidence, used one, and
    // both scores stay bounded in [0, 1].
    let now = chrono::Utc::now();
    let stale_low = forgetting_score(0.05, 0, None, now);
    let fresh_high = forgetting_score(0.95, 25, Some(now), now);
    assert!(
        stale_low > fresh_high,
        "stale low-value fact must out-forget a fresh high-value one \
         (stale_low={stale_low:.3}, fresh_high={fresh_high:.3})"
    );
    assert!(
        (0.0..=1.0).contains(&stale_low) && (0.0..=1.0).contains(&fresh_high),
        "forgetting_score must stay bounded in [0, 1]"
    );

    let ep = mem
        .store_episode(
            "observed a flaky retry in the deploy step",
            "engineer",
            None,
        )
        .unwrap();

    mem.store_fact(
        "noise-decay",
        "throwaway low value scratch note",
        0.05,
        &[],
        "src",
    )
    .unwrap();
    mem.store_fact(
        "core-durable",
        "durable high value architecture decision",
        0.95,
        &[],
        "src",
    )
    .unwrap();
    mem.store_fact_with_provenance(
        "grounded-prov",
        "flaky retry observed in the deploy step",
        0.05,
        "src",
        None,
        None,
        &[ep],
    )
    .unwrap();
    let warm = mem
        .store_fact(
            "warm-recalled",
            "frequently recalled low confidence note",
            0.05,
            &[],
            "src",
        )
        .unwrap();

    // Recall-reinforce ONLY `warm-recalled` (precise, per-fact), so its
    // usage/recency rise and its forgetting_score drops below the floor — the
    // recall→forgetting loop that protects warm low-confidence knowledge. A
    // wildcard reinforcing recall would touch every fact, so reinforce the exact
    // node directly, exactly as a recall-intent caller does at point-of-use.
    for _ in 0..3 {
        mem.reinforce_access(&warm, MemoryKind::Fact)
            .expect("reinforce warm-recalled");
    }

    let before = live_concepts(&mem);

    // Mandatory safety: dry-run previews candidates but mutates nothing.
    let preview: ForgetReport = mem.forget_low_value_facts(true).unwrap();
    let after_preview = live_concepts(&mem);
    assert!(preview.dry_run, "dry-run report must be flagged dry_run");
    assert_eq!(preview.candidates, 1, "exactly one forgetting candidate");
    assert_eq!(preview.archived, 0, "dry-run archives nothing");
    assert_eq!(preview.deleted, 0, "dry-run deletes nothing");
    assert_eq!(
        preview.live_before, preview.live_after,
        "dry-run must not change the live count"
    );
    assert_eq!(
        after_preview.len(),
        before.len(),
        "dry-run must not change live facts"
    );

    // Live run: forget exactly the one low-value, unprotected, un-reinforced fact.
    let report = mem.forget_low_value_facts(false).unwrap();
    let after = live_concepts(&mem);

    assert!(!report.dry_run, "live run must not be flagged dry_run");
    assert_eq!(
        report.archived + report.deleted,
        1,
        "live run must forget exactly one fact"
    );
    assert!(
        !after.contains(&"noise-decay".to_string()),
        "the low-value fact must be forgotten; live after={after:?}"
    );
    assert!(
        after.contains(&"core-durable".to_string()),
        "high-confidence fact must be kept; live after={after:?}"
    );
    assert!(
        after.contains(&"grounded-prov".to_string()),
        "provenance-bearing fact must be kept (safety); live after={after:?}"
    );
    assert!(
        after.contains(&"warm-recalled".to_string()),
        "recall-warmed fact must be kept (#2440 loop); live after={after:?}"
    );
    assert_eq!(
        after.len(),
        before.len() - 1,
        "exactly one fact forgotten; before={before:?} after={after:?}"
    );
}

/// A scripted distill runner exercising the public pass through
/// [`DistillRecipeRunner::run`]. When `fail` is set it returns a transient-class
/// recipe error (mirroring a non-zero `recipe-runner-rs` exit); otherwise it
/// yields one grounded, gate-passing fact. It records how many times the pass
/// invoked it so the single-invocation contract is observable.
struct ScriptedRunner {
    attempts: Cell<u32>,
    fail: bool,
}

impl ScriptedRunner {
    fn succeeding() -> Self {
        Self {
            attempts: Cell::new(0),
            fail: false,
        }
    }

    fn failing() -> Self {
        Self {
            attempts: Cell::new(0),
            fail: true,
        }
    }
}

impl DistillRecipeRunner for ScriptedRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        self.attempts.set(self.attempts.get() + 1);
        if self.fail {
            // Stable transient-class prefix (`classify_distill_error` →
            // `CopilotTerminalFailure`, `is_transient() == true`): a non-zero
            // recipe exit. The pass must still mark NOTHING and stay retry-
            // eligible next cycle — the retry-safety invariant.
            return Err(SimardError::RpcError(
                "distill: recipe exited with status 1".to_string(),
            ));
        }
        // Grounded (+0.5), >=3 words (+0.3), known concept (+0.1) = 0.9 ≥ 0.5 gate.
        Ok(vec![DistilledFact {
            concept: "lesson-learned".to_string(),
            content: "a flaky CI step was fixed and the cause recorded".to_string(),
            source_episode_id: episodes[0].node_id.clone(),
        }])
    }
}

fn seed_episodes(mem: &LibraryCognitiveMemory, n: usize) {
    for i in 0..n {
        mem.store_episode(
            &format!("episode {i}: an engineer fixed a flaky CI step and recorded the cause"),
            "engineer",
            None,
        )
        .unwrap();
    }
}

/// Distillation is retry-SAFE and single-invocation per pass (#2468 → superseded
/// by #2679's agentic direct-write).
///
/// Historical context: #2468 added a bounded in-cycle retry for a transient
/// *parse* miss. #2679 then removed parsing entirely — the distiller now writes
/// each fact directly through the memory tool, so there is no document to parse
/// and no `ParseFailure` class to retry (see
/// `src/memory_consolidation/distillation.rs`). What #2468 ultimately protected
/// — and what remains a durable, outside-in-observable invariant — is the
/// **retry-safety** contract of `distill_recent_episodes_with_runner`:
///
///   A. success → the whole batch is distilled in ONE pass (a gated fact is
///      stored, every episode is marked, nothing is left undistilled), and the
///      runner is invoked exactly once.
///   B. transient failure → the pass returns `Err` and marks NOTHING, so the
///      whole batch stays fully retry-eligible on the next cycle, and the runner
///      is invoked exactly once (the pass never partially commits).
///
/// Drives the distill/self-metrics path → redirect HOME + serialise env mutation.
#[test]
#[serial(cognitive_memory)]
fn distill_pass_is_retry_safe_and_single_invocation() {
    let _home = HomeGuard::new();

    // Sub-case A: a successful pass distils and marks the whole batch.
    let mem_a = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    seed_episodes(&mem_a, 22);

    let runner_a = ScriptedRunner::succeeding();
    let report_a = distill_recent_episodes_with_runner(&mem_a, &runner_a)
        .expect("a successful runner must distil the batch, not error");
    let invoked_a = runner_a.attempts.get();
    let undistilled_after = mem_a.list_undistilled_episodes(50).unwrap().len();

    assert!(
        report_a.fact_count >= 1,
        "sub-case A: a gated fact must be stored"
    );
    assert_eq!(
        report_a.input_count, 22,
        "sub-case A: the full batch is the input"
    );
    assert_eq!(
        report_a.marked_count, report_a.input_count,
        "sub-case A: every episode must be marked distilled"
    );
    assert_eq!(
        invoked_a, 1,
        "sub-case A: the runner is invoked exactly once"
    );
    assert_eq!(
        undistilled_after, 0,
        "sub-case A: the batch must NOT be deferred"
    );

    // Sub-case B: a transient failure marks NOTHING (retry-safe) and does not
    // silently retry — the batch stays fully eligible next cycle.
    let mem_b = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    seed_episodes(&mem_b, 22);
    let runner_b = ScriptedRunner::failing();
    let result_b = distill_recent_episodes_with_runner(&mem_b, &runner_b);
    let invoked_b = runner_b.attempts.get();
    let still_undistilled = mem_b.list_undistilled_episodes(50).unwrap().len();
    let facts_b = live_concepts(&mem_b).len();

    assert!(
        result_b.is_err(),
        "sub-case B: a transient failure must surface as Err"
    );
    assert_eq!(
        invoked_b, 1,
        "sub-case B: the runner is invoked exactly once"
    );
    assert_eq!(
        still_undistilled, 22,
        "sub-case B: nothing marked — the whole batch stays retry-eligible"
    );
    assert_eq!(facts_b, 0, "sub-case B: no facts stored on a failed pass");
}
