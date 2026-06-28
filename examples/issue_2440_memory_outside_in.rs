//! Outside-in (black-box) verification harness for the cognitive-memory goal
//! covering issues #2440, #2434, and #2468.
//!
//! This binary lives in `examples/` so it compiles as an EXTERNAL consumer of
//! the `simard` crate: it can only touch the PUBLIC API surface, exactly as the
//! daemon (or any downstream user) does. It never reaches into private
//! internals or `#[cfg(test)]` helpers — it exercises the same entry points a
//! real caller (OODA prep, consolidation cadence, distill scheduler) uses.
//!
//! Scenarios, each a user-visible behaviour the goal introduces:
//!
//!   1. (#2440, simple) Recall is ACTUALLY ranked by weighted multi-signal
//!      scoring — not a plain whole-string CONTAINS keyword match. Proven by
//!      flipping the top result purely by changing the `RecallWeightSet`.
//!   2. (#2434 + #2440, complex) Controlled forgetting of LIVE facts: a
//!      low-value fact fades while a high-confidence fact, a provenance-bearing
//!      fact, and a recall-reinforced low-confidence fact are all protected; the
//!      dry-run preview changes nothing.
//!   3. (#2468, complex) A TRANSIENT distill parse miss is retried in-cycle and
//!      the batch is stored — instead of deferring the whole batch a full cycle
//!      — while an exhausted-retry failure still marks NOTHING (retry-safe).
//!
//! Exit code 0 = all scenarios PASS, non-zero = at least one FAILED.

use std::cell::Cell;

use simard::cognitive_memory::{
    CognitiveMemoryOps, FORGET_MIN_IMPORTANCE, ForgetReport, LibraryCognitiveMemory, MemoryKind,
    RecallWeightSet, forgetting_score,
};
use simard::error::{SimardError, SimardResult};
use simard::memory_cognitive::CognitiveEpisode;
use simard::memory_consolidation::distillation::{
    DistillRecipeRunner, DistilledFact, distill_recent_episodes_with_runner,
};

/// All live (non-archived) fact concepts, via the public wildcard recall path.
fn live_concepts(mem: &LibraryCognitiveMemory) -> Vec<String> {
    mem.search_facts("*", 256, 0.0)
        .expect("wildcard recall")
        .into_iter()
        .map(|f| f.concept)
        .collect()
}

/// Scenario 1 (simple): ranked multi-signal recall (#2440).
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
fn scenario_1_ranked_recall() -> bool {
    println!("── Scenario 1 (#2440): ranked multi-signal recall ──");
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

    println!("  query               : {query:?}");
    println!("  recalled (count)    : {}", by_conf.len());
    println!("  top @ confidence wts: {conf_top:?}  (high-confidence fact)");
    println!("  top @ usage wts     : {usage_top:?}  (recall-reinforced fact)");

    let both_recalled = by_conf.len() == 2 && by_usage.len() == 2;
    let conf_wins = conf_top == "alpha-confident";
    let usage_wins = usage_top == "beta-reinforced";
    let order_flips = conf_top != usage_top;

    println!("  both facts recalled           : {both_recalled}");
    println!("  confidence-led top correct    : {conf_wins}");
    println!("  usage-led top correct         : {usage_wins}");
    println!("  ranking responds to weights   : {order_flips}");

    let ok = both_recalled && conf_wins && usage_wins && order_flips;
    println!(
        "  RESULT              : {}\n",
        if ok {
            "PASS"
        } else {
            "FAIL (recall not weight-ranked — the #2440 defect)"
        }
    );
    ok
}

/// Scenario 2 (complex): controlled forgetting of live facts (#2434), with the
/// recall→forgetting protection loop (#2440).
///
/// Four live facts share a store; only the genuinely low-value one must fade:
///   * `noise-decay`   — conf 0.05, no provenance, never recalled → FORGET.
///   * `core-durable`  — conf 0.95 → protected (above the forgetting floor).
///   * `grounded-prov` — conf 0.05 BUT provenance-bearing → protected (safety).
///   * `warm-recalled` — conf 0.05 BUT recall-reinforced → protected (#2440 loop).
///
/// The dry-run preview must report the candidate yet change nothing; the live
/// run must forget exactly the one low-value fact and leave the other three.
fn scenario_2_controlled_forgetting() -> bool {
    println!("── Scenario 2 (#2434 + #2440): controlled forgetting of live facts ──");
    let mem = LibraryCognitiveMemory::in_memory().expect("in-memory store");

    // Sanity-pin the public forgetting signal: a stale, low-confidence,
    // never-used fact must out-forget a fresh, high-confidence, used one, and
    // both scores stay bounded in [0, 1].
    let now = chrono::Utc::now();
    let stale_low = forgetting_score(0.05, 0, None, now);
    let fresh_high = forgetting_score(0.95, 25, Some(now), now);
    let floor = forgetting_score(FORGET_MIN_IMPORTANCE, 0, None, now);
    println!(
        "  forgetting_score: stale_low={stale_low:.3} > fresh_high={fresh_high:.3} (floor={floor:.3})"
    );
    let signal_ok = stale_low > fresh_high
        && (0.0..=1.0).contains(&stale_low)
        && (0.0..=1.0).contains(&fresh_high);

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
    println!("  live before        : {before:?}");

    // Mandatory safety: dry-run previews candidates but mutates nothing.
    let preview: ForgetReport = mem.forget_low_value_facts(true).unwrap();
    println!(
        "  dry-run            : candidates={} archived={} deleted={} live {}→{}",
        preview.candidates,
        preview.archived,
        preview.deleted,
        preview.live_before,
        preview.live_after
    );
    let after_preview = live_concepts(&mem);
    let dry_run_safe = preview.dry_run
        && preview.candidates == 1
        && preview.archived == 0
        && preview.deleted == 0
        && preview.live_before == preview.live_after
        && after_preview.len() == before.len();

    // Live run: forget exactly the one low-value, unprotected, un-reinforced fact.
    let report = mem.forget_low_value_facts(false).unwrap();
    println!(
        "  live run           : candidates={} archived={} deleted={} live {}→{}",
        report.candidates, report.archived, report.deleted, report.live_before, report.live_after
    );
    let after = live_concepts(&mem);
    println!("  live after         : {after:?}");

    let forgot_one = !report.dry_run && (report.archived + report.deleted) == 1;
    let dropped_noise = !after.contains(&"noise-decay".to_string());
    let kept_core = after.contains(&"core-durable".to_string());
    let kept_prov = after.contains(&"grounded-prov".to_string());
    let kept_warm = after.contains(&"warm-recalled".to_string());
    let shrank = after.len() == before.len() - 1;

    println!("  forgetting_score sane         : {signal_ok}");
    println!("  dry-run changed nothing       : {dry_run_safe}");
    println!("  forgot exactly one fact       : {forgot_one}");
    println!("  low-value fact forgotten      : {dropped_noise}");
    println!("  high-confidence fact kept     : {kept_core}");
    println!("  provenance fact kept (safety) : {kept_prov}");
    println!("  recall-warmed fact kept (loop): {kept_warm}");

    let ok = signal_ok
        && dry_run_safe
        && forgot_one
        && dropped_noise
        && kept_core
        && kept_prov
        && kept_warm
        && shrank;
    println!(
        "  RESULT             : {}\n",
        if ok { "PASS" } else { "FAIL" }
    );
    ok
}

/// A runner that fails transiently (parse miss) for its first `fail_first`
/// attempts, then — if reached — succeeds with one grounded, gate-passing fact.
/// Mirrors the real `recipe-runner-rs` parse-miss error shape so the production
/// `classify_distill_error` path marks it `ParseFailure` (transient).
struct FlakyParseRunner {
    attempts: Cell<u32>,
    fail_first: u32,
}

impl DistillRecipeRunner for FlakyParseRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        let n = self.attempts.get() + 1;
        self.attempts.set(n);
        if n <= self.fail_first {
            // EXACT production prefix for a parse miss → classified ParseFailure
            // (transient): recipe exited 0 but its output had no parseable
            // `{ "facts": [...] }` object.
            return Err(SimardError::BridgeError(
                "distill: `distill` step output did not contain a parseable `{ \"facts\": [...] }` object".to_string(),
            ));
        }
        // Grounded (+0.5), >=3 words (+0.3), known concept (+0.1) = 0.9 ≥ 0.5 gate.
        Ok(vec![DistilledFact {
            concept: "lesson-learned".to_string(),
            content: "a transient parse miss is recovered by an in-cycle retry".to_string(),
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

/// Scenario 3 (complex): transient distill parse miss is retried in-cycle (#2468).
///
/// Pre-#2468, a single transient parse miss deferred the WHOLE batch for a full
/// consolidation cycle (0 facts stored, 0 episodes marked). The fix retries the
/// transient class once in-cycle. Two sub-cases prove it is both effective and
/// safe:
///
///   A. transient-then-success → the batch is distilled in ONE pass (fact stored,
///      every episode marked), and the runner was invoked exactly twice.
///   B. always-transient → the pass returns `Err` and marks NOTHING, so the
///      whole batch stays retry-eligible (the retry-safety invariant).
fn scenario_3_distill_transient_retry() -> bool {
    println!("── Scenario 3 (#2468): transient distill parse miss retried in-cycle ──");

    // Sub-case A: recovers on the in-cycle retry.
    let mem_a = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    seed_episodes(&mem_a, 22);
    let undistilled_before = mem_a.list_undistilled_episodes(50).unwrap().len();

    let runner_a = FlakyParseRunner {
        attempts: Cell::new(0),
        fail_first: 1,
    };
    let report_a = distill_recent_episodes_with_runner(&mem_a, &runner_a)
        .expect("transient miss must recover in-cycle, not error");
    let invoked_a = runner_a.attempts.get();
    let undistilled_after = mem_a.list_undistilled_episodes(50).unwrap().len();

    println!(
        "  A transient→success: input={} facts={} marked={} runner_invoked={} undistilled {}→{}",
        report_a.input_count,
        report_a.fact_count,
        report_a.marked_count,
        invoked_a,
        undistilled_before,
        undistilled_after
    );
    let a_stored = report_a.fact_count >= 1;
    let a_marked_all = report_a.marked_count == report_a.input_count && report_a.input_count == 22;
    let a_retried_once = invoked_a == 2;
    let a_not_deferred = undistilled_after == 0;

    // Sub-case B: retries exhausted → Err, and NOTHING is marked (retry-safe).
    let mem_b = LibraryCognitiveMemory::in_memory().expect("in-memory store");
    seed_episodes(&mem_b, 22);
    let runner_b = FlakyParseRunner {
        attempts: Cell::new(0),
        fail_first: u32::MAX,
    };
    let result_b = distill_recent_episodes_with_runner(&mem_b, &runner_b);
    let invoked_b = runner_b.attempts.get();
    let still_undistilled = mem_b.list_undistilled_episodes(50).unwrap().len();
    let facts_b = live_concepts(&mem_b).len();

    println!(
        "  B always-transient : is_err={} runner_invoked={} still_undistilled={} facts_stored={}",
        result_b.is_err(),
        invoked_b,
        still_undistilled,
        facts_b
    );
    let b_errored = result_b.is_err();
    let b_bounded_retry = invoked_b == 2; // 1 initial + DISTILL_PARSE_RETRY_MAX(1)
    let b_marked_nothing = still_undistilled == 22 && facts_b == 0;

    println!("  A: facts stored               : {a_stored}");
    println!("  A: all episodes marked        : {a_marked_all}");
    println!("  A: retried exactly once       : {a_retried_once}");
    println!("  A: batch NOT deferred         : {a_not_deferred}");
    println!("  B: exhausted retry errors     : {b_errored}");
    println!("  B: retry bounded (2 attempts) : {b_bounded_retry}");
    println!("  B: nothing marked/stored      : {b_marked_nothing}");

    let ok = a_stored
        && a_marked_all
        && a_retried_once
        && a_not_deferred
        && b_errored
        && b_bounded_retry
        && b_marked_nothing;
    println!(
        "  RESULT             : {}\n",
        if ok {
            "PASS"
        } else {
            "FAIL (batch deferred/dropped on transient miss — the #2468 defect)"
        }
    );
    ok
}

fn main() {
    // Keep the run hermetic: the distill path records self-metrics under
    // `$HOME/.simard/metrics/` outside `#[cfg(test)]`. Point HOME at a scratch
    // dir so the operator's real metrics file is never touched.
    let scratch = std::env::temp_dir().join(format!("simard-oi-2440-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("scratch home");
    // SAFETY: single-threaded `main` before any threads are spawned.
    unsafe {
        std::env::set_var("HOME", &scratch);
    }

    println!("=== cognitive-memory outside-in verification (#2440 / #2434 / #2468) ===\n");
    let s1 = scenario_1_ranked_recall();
    let s2 = scenario_2_controlled_forgetting();
    let s3 = scenario_3_distill_transient_retry();

    // Best-effort cleanup of the scratch home.
    let _ = std::fs::remove_dir_all(&scratch);

    println!("=== SUMMARY ===");
    println!(
        "Scenario 1 (#2440 ranked recall)        : {}",
        if s1 { "PASS" } else { "FAIL" }
    );
    println!(
        "Scenario 2 (#2434 controlled forgetting): {}",
        if s2 { "PASS" } else { "FAIL" }
    );
    println!(
        "Scenario 3 (#2468 distill retry)        : {}",
        if s3 { "PASS" } else { "FAIL" }
    );

    if s1 && s2 && s3 {
        println!("\nALL SCENARIOS PASSED");
        std::process::exit(0);
    } else {
        eprintln!("\nONE OR MORE SCENARIOS FAILED");
        std::process::exit(1);
    }
}
