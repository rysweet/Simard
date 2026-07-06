//! Deterministic **fact-yield benchmark** for episode distillation (perpetual
//! cognition sub-goal: *raise distillation fact-yield*).
//!
//! ## What this measures — and what it does NOT claim
//!
//! This is a **deterministic regression benchmark**, not an estimate of
//! real-world LLM yield. It measures the *parse + reliability-gate* fact-yield —
//! surviving facts per input episode — over a **fixed, hand-authored sample
//! corpus**. It exercises the deterministic portion of a distillation pass,
//! everything downstream of the (non-deterministic) LLM recipe call:
//!
//! 1. `parse_facts_document` — parses the recipe's JSON envelope and applies
//!    the concept filter ([`RecipeEnvelope::into_facts`]).
//! 2. the ISAO reliability gate — [`assess_fact_reliability`] against
//!    [`DISTILL_RELIABILITY_THRESHOLD`].
//!
//! The numbers are a property of this corpus (which deliberately contains five
//! recoverable surface-form-variant labels), NOT a measurement of how often
//! concept-variant drops occur in production. Treat `baseline_yield=0.120 →
//! improved_yield=0.320` as "the concept filter no longer drops these five
//! legitimate facts", not as a global production-yield delta.
//!
//! The corpus is deliberately **dedup-neutral against an empty store** (every
//! candidate has distinct `concept + content`), so the production dedup guard —
//! which only blocks an equal-or-stronger *identical* prior — never fires. The
//! parse+gate survivor count therefore equals the full-pass promoted count for a
//! FRESH memory; that equality is not merely asserted here but *exercised* by
//! [`distillation_tests::full_pass_promotes_canonicalized_surface_variants_through_dedup`],
//! which routes this same corpus through the real
//! `distill_recent_episodes_with_runner` (parse → gate → dedup guard → store)
//! and confirms all eight survive.
//!
//! ## `structural_precision`, not semantic truth
//!
//! Precision here is **structural**: a promoted fact is "correct" iff it is
//! grounded in the batch, non-empty, and carries a recognized concept — exactly
//! the reliability gate's admission invariant. It does NOT assert the fact's
//! content is semantically supported by the cited episode (the gate never claimed
//! to). The point this benchmark proves is narrow but real: canonicalization does
//! not *weaken* that structural invariant — it admits only facts the gate would
//! already accept had the label been spelled canonically, and never an off-spec,
//! ungrounded, or empty candidate.
//!
//! ## Baseline vs improved (self-contained before/after)
//!
//! The benchmark computes two numbers over the SAME corpus:
//!
//! * **baseline** — an in-test oracle that replicates the *legacy exact-match*
//!   concept filter (`concept == "pr-pattern" | "bug-pattern" | "lesson-learned"`).
//! * **improved** — the REAL production path (`parse_facts_document`), which
//!   now canonicalizes surface-form concept variants before the filter.
//!
//! It asserts the improved path promotes strictly more facts than the baseline
//! (higher parse+gate yield) while keeping **structural_precision at 1.0** and
//! never dropping a fact the baseline kept (superset). If the canonicalization is
//! reverted, `improved` collapses to `baseline` and the strict-improvement
//! assertion fails — the permanent regression guard.

use crate::memory_cognitive::CognitiveEpisode;
use crate::memory_consolidation::distillation::{
    DISTILL_RELIABILITY_THRESHOLD, DistilledFact, KNOWN_DISTILL_CONCEPTS, assess_fact_reliability,
    parse_facts_document,
};

/// Number of episodes in the fixed consolidation-input batch.
pub(crate) const CORPUS_EPISODE_COUNT: usize = 25;

/// Parse+gate survivor count under the legacy exact-match concept filter. This
/// is the **recorded baseline** the improvement is measured against.
pub(crate) const BASELINE_PROMOTED: usize = 3;

/// Parse+gate survivor count under the canonicalizing concept filter (the
/// shipped change). Recovers the five surface-form-variant facts the exact-match
/// filter silently dropped. The full-pass test
/// `distillation_tests::full_pass_promotes_canonicalized_surface_variants_through_dedup`
/// confirms all eight are actually promoted (survive the dedup guard + storage).
pub(crate) const IMPROVED_PROMOTED: usize = 8;

/// Build the fixed episode batch: ids `ep-000`..`ep-024`. Grounding is decided
/// purely by whether a fact cites one of these ids.
fn corpus_episodes() -> Vec<CognitiveEpisode> {
    (0..CORPUS_EPISODE_COUNT)
        .map(|i| CognitiveEpisode {
            node_id: format!("ep-{i:03}"),
            content: format!("episode {i} body"),
            source_label: "distill-bench".to_string(),
            temporal_index: i as i64,
            compressed: false,
        })
        .collect()
}

/// The fixed recipe-output envelope (a stand-in for one LLM distillation
/// response), shared with the full-pass test in `distillation_tests` so both
/// measure the identical corpus. Thirteen candidate facts spanning every
/// yield/precision case:
///
/// * 3 canonical, grounded, well-formed — kept by BOTH filters.
/// * 5 surface-form variants (case / whitespace / underscore), grounded,
///   well-formed — dropped by the exact-match filter, RECOVERED by
///   canonicalization. This is the yield gain.
/// * 3 genuinely off-spec concepts — dropped by BOTH (precision guard: the
///   canonicalizer must NOT admit these).
/// * 1 canonical concept but ungrounded provenance — passes both filters, then
///   quarantined by the reliability gate (precision guard).
/// * 1 canonical concept but empty content — passes both filters, then
///   quarantined by the reliability hard gate (precision guard).
pub(crate) const CORPUS_RECIPE_JSON: &str = r#"{"facts":[
  {"concept":"pr-pattern","content":"rebase feature branches before merging to keep history linear","source_episode_id":"ep-000"},
  {"concept":"bug-pattern","content":"null dereference when the input batch is empty","source_episode_id":"ep-001"},
  {"concept":"lesson-learned","content":"write the failing parser test before touching the scanner","source_episode_id":"ep-002"},
  {"concept":"PR-Pattern","content":"squash fixups so each commit builds green","source_episode_id":"ep-003"},
  {"concept":" bug-pattern ","content":"off by one on the last chunk boundary","source_episode_id":"ep-004"},
  {"concept":"Lesson-Learned","content":"measure before optimizing the hot path","source_episode_id":"ep-005"},
  {"concept":"pr_pattern","content":"request review from the module owner","source_episode_id":"ep-006"},
  {"concept":"BUG-PATTERN","content":"unbounded retry loop on transient network error","source_episode_id":"ep-007"},
  {"concept":"made-up-label","content":"this concept is off spec and must be dropped","source_episode_id":"ep-008"},
  {"concept":"skip","content":"no durable signal in this episode","source_episode_id":"ep-009"},
  {"concept":"observation","content":"random unstructured note with no label","source_episode_id":"ep-010"},
  {"concept":"pr-pattern","content":"cites an episode outside this batch so provenance is unverifiable","source_episode_id":"ep-999"},
  {"concept":"bug-pattern","content":"   ","source_episode_id":"ep-011"}
]}"#;

/// A parsed candidate fact straight from the recipe JSON, BEFORE any concept
/// filtering — the raw material the baseline oracle filters itself.
struct RawCandidate {
    concept: String,
    content: String,
    source_episode_id: String,
}

/// Parse `CORPUS_RECIPE_JSON` into raw candidates without applying any concept
/// filter. Used only by the baseline oracle so it can replicate the *legacy*
/// exact-match behaviour independently of the production parser.
fn raw_candidates() -> Vec<RawCandidate> {
    let v: serde_json::Value =
        serde_json::from_str(CORPUS_RECIPE_JSON).expect("benchmark corpus JSON must be valid");
    v["facts"]
        .as_array()
        .expect("corpus must have a facts array")
        .iter()
        .map(|f| RawCandidate {
            concept: f["concept"].as_str().unwrap_or_default().to_string(),
            content: f["content"].as_str().unwrap_or_default().to_string(),
            source_episode_id: f["source_episode_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// The reliability gate: keep only facts whose self-assessed confidence clears
/// [`DISTILL_RELIABILITY_THRESHOLD`]. This is the exact promotion predicate the
/// production pass applies (minus the dedup guard, which the dedup-neutral
/// corpus never triggers).
fn gate_survivors(facts: &[DistilledFact], episodes: &[CognitiveEpisode]) -> Vec<DistilledFact> {
    facts
        .iter()
        .filter(|f| assess_fact_reliability(f, episodes, facts) >= DISTILL_RELIABILITY_THRESHOLD)
        .cloned()
        .collect()
}

/// Legacy baseline oracle: replicate the *exact-match* concept filter that
/// `into_facts` used before canonicalization, then gate. Independent of the
/// production parser so the before/after comparison is honest.
fn baseline_promoted(episodes: &[CognitiveEpisode]) -> Vec<DistilledFact> {
    let filtered: Vec<DistilledFact> = raw_candidates()
        .into_iter()
        .filter(|c| {
            matches!(
                c.concept.as_str(),
                "pr-pattern" | "bug-pattern" | "lesson-learned"
            )
        })
        .map(|c| DistilledFact {
            concept: c.concept,
            content: c.content,
            source_episode_id: c.source_episode_id,
        })
        .collect();
    gate_survivors(&filtered, episodes)
}

/// Improved path: the REAL production parse+filter (`parse_facts_document`,
/// which now canonicalizes concepts), then the same gate.
fn improved_promoted(episodes: &[CognitiveEpisode]) -> Vec<DistilledFact> {
    let output = parse_facts_document(CORPUS_RECIPE_JSON)
        .expect("benchmark corpus must parse into a facts object");
    gate_survivors(&output.facts, episodes)
}

/// A promoted fact is "structurally correct" iff it is grounded in the batch,
/// has non-empty content, and carries a recognized concept label — exactly the
/// reliability gate's admission invariant. This is a STRUCTURAL check, not a
/// judgement that the fact's content is semantically supported by the cited
/// episode; the point is that canonicalization does not weaken the invariant.
fn is_correct(fact: &DistilledFact, episodes: &[CognitiveEpisode]) -> bool {
    let grounded = episodes.iter().any(|e| e.node_id == fact.source_episode_id);
    let non_empty = !fact.content.trim().is_empty();
    let known_concept = KNOWN_DISTILL_CONCEPTS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(fact.concept.trim()));
    grounded && non_empty && known_concept
}

/// Structural precision: fraction of promoted facts that satisfy the gate's
/// structural admission invariant (see [`is_correct`]). 1.0 means the promotion
/// set contains no ungrounded, empty, or off-spec fact.
fn structural_precision(promoted: &[DistilledFact], episodes: &[CognitiveEpisode]) -> f64 {
    if promoted.is_empty() {
        return 1.0;
    }
    let correct = promoted.iter().filter(|f| is_correct(f, episodes)).count();
    correct as f64 / promoted.len() as f64
}

#[test]
fn fact_yield_benchmark_records_baseline_and_proves_improvement() {
    let episodes = corpus_episodes();

    let baseline = baseline_promoted(&episodes);
    let improved = improved_promoted(&episodes);

    let baseline_yield = baseline.len() as f64 / episodes.len() as f64;
    let improved_yield = improved.len() as f64 / episodes.len() as f64;
    let baseline_structural_precision = structural_precision(&baseline, &episodes);
    let improved_structural_precision = structural_precision(&improved, &episodes);

    // Emit the recorded numbers so CI logs (`-- --nocapture`) show the baseline
    // and the measured improvement on every run. These are parse+gate survivor
    // counts over the FIXED corpus, not a real-world end-to-end LLM yield.
    println!(
        "[fact-yield-bench] corpus_episodes={} candidate_facts={} \
         baseline_promoted={} improved_promoted={} \
         baseline_yield={:.3} improved_yield={:.3} \
         baseline_structural_precision={:.3} improved_structural_precision={:.3}",
        episodes.len(),
        raw_candidates().len(),
        baseline.len(),
        improved.len(),
        baseline_yield,
        improved_yield,
        baseline_structural_precision,
        improved_structural_precision,
    );

    // 1. Recorded baseline is stable.
    assert_eq!(
        baseline.len(),
        BASELINE_PROMOTED,
        "recorded baseline fact-yield drifted; corpus or exact-match oracle changed"
    );

    // 2. The shipped change raises measured yield to the recorded improved number.
    assert_eq!(
        improved.len(),
        IMPROVED_PROMOTED,
        "improved fact-yield drifted from the recorded number"
    );

    // 3. Yield strictly improved (the core acceptance signal). Reverting the
    //    concept canonicalization makes `improved == baseline` and fails here.
    assert!(
        improved.len() > baseline.len(),
        "fact-yield did not improve: improved={} baseline={}",
        improved.len(),
        baseline.len()
    );

    // 4. Structural precision is NOT lowered — both paths promote only facts that
    //    satisfy the gate's grounded/non-empty/known-concept invariant.
    assert_eq!(
        baseline_structural_precision, 1.0,
        "baseline structural precision must be 1.0"
    );
    assert_eq!(
        improved_structural_precision, 1.0,
        "improved structural precision regressed below 1.0 — canonicalization admitted an off-spec/ungrounded/empty fact"
    );
    assert!(
        improved_structural_precision >= baseline_structural_precision,
        "structural precision regressed: improved={improved_structural_precision} baseline={baseline_structural_precision}"
    );

    // 5. No regression: every fact the baseline promoted is still promoted.
    for b in &baseline {
        assert!(
            improved
                .iter()
                .any(|i| i.content == b.content && i.source_episode_id == b.source_episode_id),
            "improved path dropped a baseline-promoted fact: {b:?}"
        );
    }
}

#[test]
fn fact_yield_benchmark_recovers_only_surface_variants_not_offspec() {
    // The five recovered facts (improved − baseline) must all be the grounded,
    // well-formed, surface-variant-concept facts — never an off-spec, ungrounded,
    // or empty candidate. This pins that the yield gain is precision-safe.
    let episodes = corpus_episodes();
    let baseline = baseline_promoted(&episodes);
    let improved = improved_promoted(&episodes);

    let recovered: Vec<&DistilledFact> = improved
        .iter()
        .filter(|i| {
            !baseline
                .iter()
                .any(|b| b.content == i.content && b.source_episode_id == i.source_episode_id)
        })
        .collect();

    assert_eq!(
        recovered.len(),
        IMPROVED_PROMOTED - BASELINE_PROMOTED,
        "unexpected number of recovered facts"
    );
    for r in recovered {
        assert!(
            is_correct(r, &episodes),
            "a recovered fact is not a correct (grounded/non-empty/known-concept) fact: {r:?}"
        );
        // Every promoted concept is stored in canonical (lower-hyphen) form.
        assert!(
            matches!(
                r.concept.as_str(),
                "pr-pattern" | "bug-pattern" | "lesson-learned"
            ),
            "recovered fact concept was not canonicalized: {:?}",
            r.concept
        );
    }
}

/// Deterministic **before/after parse-recovery benchmark** for issue #2658.
///
/// A single trailing comma in the distiller agent's JSON (the most common LLM
/// JSON defect) made strict `serde_json` reject the WHOLE facts object, so the
/// batch was deferred every cycle and `distill_parse_success_rate` collapsed
/// toward 0 (the overseer's "residual 100% parse-failure"). Over a batch of
/// trailing-comma documents reproducing that shape, the strict baseline fails
/// EVERY one (parse-failure-rate = 1.000) while the shipped tolerant parser
/// (`parse_facts_document`, the real production path) recovers EVERY one
/// (parse-failure-rate = 0.000).
///
/// This is the deterministic, in-process analog of the live
/// `distill_parse_success_rate` self-metric, which
/// `record_parse_outcome("distill", parsed.is_ok())` drives from the same
/// `parse_facts_document` return value on every real pass — so a production
/// distill pass over trailing-comma output moves that metric off the floor by
/// the exact mechanism proven here.
#[test]
fn distill_parse_failure_rate_benchmark_before_1000_after_0000() {
    // A batch of realistic trailing-comma distill facts documents (bare and
    // pretty-printed) that each carry one well-formed, grounded fact.
    let doc = |ep: &str, pretty: bool| -> String {
        if pretty {
            format!(
                "{{\n  \"facts\": [\n    {{\"concept\": \"lesson-learned\", \"content\": \"c\", \"source_episode_id\": \"{ep}\"}},\n  ],\n}}"
            )
        } else {
            format!(
                "{{\"facts\":[{{\"concept\":\"pr-pattern\",\"content\":\"c\",\"source_episode_id\":\"{ep}\"}},],}}"
            )
        }
    };
    let batch: Vec<String> = vec![
        doc("epi_1", false),
        doc("epi_2", true),
        doc("epi_3", false),
        doc("epi_4", true),
        doc("epi_5", false),
    ];
    let n = batch.len() as f64;

    // BEFORE — the strict baseline (what the pre-#2658 parser effectively did:
    // reject the whole facts object on any trailing comma). Every one fails.
    let before_failures = batch
        .iter()
        .filter(|raw| serde_json::from_str::<serde_json::Value>(raw).is_err())
        .count();
    let before_failure_rate = before_failures as f64 / n;

    // AFTER — the shipped tolerant parser on the real production entry point.
    let after_failures = batch
        .iter()
        .filter(|raw| {
            parse_facts_document(raw)
                .map(|o| o.facts.is_empty())
                .unwrap_or(true)
        })
        .count();
    let after_failure_rate = after_failures as f64 / n;

    println!(
        "[distill-parse-recovery-bench #2658] batch={} before_failure_rate={before_failure_rate:.3} \
         after_failure_rate={after_failure_rate:.3}",
        batch.len()
    );

    assert_eq!(
        before_failure_rate, 1.000,
        "baseline strict parse must fail EVERY trailing-comma document (the residual 100% shape)"
    );
    assert_eq!(
        after_failure_rate, 0.000,
        "the tolerant parser must recover EVERY trailing-comma document (fact-yield restored)"
    );
}
