//! Episode distillation: extract semantic facts from batches of recent
//! episodes via an LLM recipe.
//!
//! See `docs/architecture/episode-distillation.md` for the full design
//! (issue #2281, PR-B). This module:
//!
//! 1. Pulls up to [`DISTILL_BATCH_SIZE`] undistilled episodes from the
//!    cognitive-memory backend, newest first.
//! 2. If fewer than [`DISTILL_MIN_EPISODES`] are present, skips the
//!    pass entirely — no LLM call, no markers.
//! 3. Otherwise invokes a pluggable [`DistillRecipeRunner`] that
//!    classifies each episode into one of three concept labels
//!    (`pr-pattern`, `bug-pattern`, `lesson-learned`) or `skip`.
//! 4. Self-assesses each candidate fact's reliability (issue #2433,
//!    BGML's ISAO) and GATES on `Fact.confidence`: low-reliability facts
//!    are quarantined (not promoted) and a weaker new fact never clobbers
//!    a stronger existing fact on the same concept. Surviving facts are
//!    stored via `store_fact_with_provenance` with the *computed*
//!    confidence rather than a constant.
//! 5. Marks EVERY input episode (including those classified `skip`) as
//!    distilled so the same low-value batch is not re-fed to the LLM.
//!
//! On recipe error: NO facts are stored AND NO markers are set; the
//! batch is fully eligible for retry on the next pass.

use std::path::Path;
use std::process::Command;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::CognitiveEpisode;

/// Maximum number of episodes pulled per distillation pass.
pub const DISTILL_BATCH_SIZE: u32 = 50;

/// Minimum number of undistilled episodes that must be present for a
/// pass to fire. Below this, the pass is a no-op (no LLM call, no
/// markers). Distillation is many-to-few; running it on a handful of
/// episodes wastes an LLM call for little quality gain.
pub const DISTILL_MIN_EPISODES: u32 = 20;

/// Legacy per-fact confidence baseline. Before issue #2433 every distilled
/// fact was written with this exact constant, so `Fact.confidence` carried no
/// information. It is retained as the **nominal baseline**: a fully-grounded,
/// known-concept, well-formed fact scores at or above this value under
/// [`assess_fact_reliability`], preserving downstream filtering behaviour for
/// good facts while letting weak ones drop below the gate.
pub const DISTILL_FACT_CONFIDENCE: f64 = 0.7;

/// ISAO reliability gate threshold (issue #2433). Candidate facts whose
/// self-assessed reliability score is below this are **quarantined** — not
/// promoted into semantic memory — rather than written with a blind constant
/// confidence. Tuned so a fact with valid in-batch provenance, a known
/// concept label, and non-trivial content clears the bar, while a fact with
/// hallucinated provenance or empty content does not.
pub const DISTILL_RELIABILITY_THRESHOLD: f64 = 0.5;

/// The closed concept-label set the distillation recipe is constrained to.
/// A fact whose concept is outside this set is off-spec and loses the
/// concept-validity component of its reliability score.
pub const KNOWN_DISTILL_CONCEPTS: &[&str] = &["pr-pattern", "bug-pattern", "lesson-learned"];

/// Maximum number of *in-cycle* retries of the distill recipe on a **transient**
/// failure (issue #2468). The pass makes at most `DISTILL_PARSE_RETRY_MAX + 1`
/// runner invocations: one initial attempt plus this many retries.
///
/// A transient miss — the recipe exited `0` but its step output carried no
/// parseable `{ "facts": [...] }` object ([`DistillFailureClass::ParseFailure`]),
/// or the recipe process exited non-zero
/// ([`DistillFailureClass::CopilotTerminalFailure`]) — previously deferred the
/// whole batch for a full consolidation cycle. A single bounded retry, with
/// JSON-format reinforcement threaded into the recipe (see
/// [`DistillRecipeRunner::run_all_reinforced`]), recovers most of these within
/// the same pass, turning a dropped batch into stored facts. Structural classes
/// (`SpawnFailure`, `SerializeFailure`, `RecipeReportedFailure`) are NOT retried
/// — they escalate immediately. Kept at `1` so the recovery stays bounded and a
/// genuinely broken recipe still surfaces promptly.
pub const DISTILL_PARSE_RETRY_MAX: u32 = 1;

/// A single semantic fact emitted by the recipe runner for one batch
/// of episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledFact {
    /// One of `pr-pattern`, `bug-pattern`, `lesson-learned`. The
    /// recipe is constrained to this label set by its prompt.
    pub concept: String,
    /// Free-text content of the fact. Stored verbatim via `store_fact`.
    pub content: String,
    /// `node_id` of the source episode (used to compose the
    /// `source_id` of the resulting fact as `distill:{id}` for
    /// provenance).
    pub source_episode_id: String,
}

/// A recurring action sequence distilled from a batch of episodes
/// (issue #2327, R5). Stored via `store_procedure_with_provenance` so a
/// `PROCEDURE_DERIVES_FROM` edge links the procedure back to the episodes it
/// was distilled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledProcedure {
    /// Procedure name (e.g. `ci-fix:auto`). Upsert-by-name on store.
    pub name: String,
    /// Ordered steps of the procedure.
    pub steps: Vec<String>,
    /// `node_id`s of the episodes this procedure was distilled from
    /// (threaded as provenance).
    pub source_episode_ids: Vec<String>,
}

/// The full output of one distillation pass: facts AND procedures
/// (issue #2327, R5). Additive over the legacy fact-only shape — a
/// fact-only runner yields an empty `procedures` vector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistillOutput {
    pub facts: Vec<DistilledFact>,
    pub procedures: Vec<DistilledProcedure>,
}

/// Pluggable LLM-side runner for the distillation recipe.
///
/// The trait exists so tests can substitute a deterministic stub.
/// Production code uses [`RecipeRunnerSubprocess`] which shells out
/// to `recipe-runner-rs`.
pub trait DistillRecipeRunner {
    /// Legacy fact-only entry point. Required so existing fact-only
    /// runners (and the subprocess runner's parse) keep working.
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>>;

    /// Full entry point emitting BOTH facts and procedures (issue #2327,
    /// R5). The default wraps [`run`](Self::run) with an empty procedure
    /// list, so a runner that only implements `run` distils facts exactly
    /// as before — back-compatible by construction.
    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        Ok(DistillOutput {
            facts: self.run(episodes)?,
            procedures: Vec::new(),
        })
    }

    /// Retry-aware entry point used by the bounded in-cycle retry loop (issue
    /// #2468). Identical to [`run_all`](Self::run_all), but when
    /// `strict_json = true` the runner is asked to reinforce the response
    /// format (e.g. by threading a `strict_json_instruction` context var into
    /// the recipe so the agent replies with ONLY the `{ "facts": [...] }`
    /// object). This is used on a retry after a transient parse miss.
    ///
    /// The default ignores `strict_json` and delegates to
    /// [`run_all`](Self::run_all), so test stubs and fact-only runners keep
    /// working unchanged; only [`RecipeRunnerSubprocess`] overrides it to thread
    /// the reinforcement flag into the subprocess invocation.
    fn run_all_reinforced(
        &self,
        episodes: &[CognitiveEpisode],
        strict_json: bool,
    ) -> SimardResult<DistillOutput> {
        let _ = strict_json;
        self.run_all(episodes)
    }
}

/// Report describing what one distillation pass actually did.
///
/// Two terminal shapes:
///
/// - **Skipped**: `input_count == fact_count == marked_count == 0`
///   (under threshold). Use [`DistillReport::skipped`] / `was_skipped`.
/// - **Ran**: `input_count >= DISTILL_MIN_EPISODES`. `fact_count`
///   is the number of `store_fact` calls; `marked_count` is the number
///   of `mark_episode_distilled` calls (equal to `input_count` on
///   success, `0` on recipe error).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DistillReport {
    /// Number of undistilled episodes pulled from the store.
    pub input_count: u32,
    /// Number of facts emitted by the recipe.
    pub fact_count: u32,
    /// Number of procedures emitted by the recipe (issue #2327, R5).
    pub procedure_count: u32,
    /// Number of episodes marked distilled after the pass.
    pub marked_count: u32,
    /// Number of candidate facts blocked by the ISAO reliability gate
    /// (issue #2433): quarantined for low self-assessed reliability OR
    /// skipped to avoid clobbering a higher-confidence existing fact.
    /// `fact_count + quarantined_count` is the total candidate count the
    /// recipe emitted for this pass.
    pub quarantined_count: u32,
}

impl DistillReport {
    /// The pass was skipped under threshold; no work was done.
    pub fn skipped() -> Self {
        Self::default()
    }

    /// `true` when the pass did not fire (input/output/marks all zero).
    pub fn was_skipped(&self) -> bool {
        self.input_count == 0
            && self.fact_count == 0
            && self.procedure_count == 0
            && self.marked_count == 0
            && self.quarantined_count == 0
    }

    /// Reduction ratio (`1 - fact_count / input_count`) as a fraction
    /// in `[0.0, 1.0]`. Returns `0.0` when `input_count == 0` (the
    /// skipped case) to avoid a divide-by-zero in the observability
    /// log line.
    pub fn reduction(&self) -> f64 {
        if self.input_count == 0 {
            0.0
        } else {
            1.0 - (self.fact_count as f64 / self.input_count as f64)
        }
    }
}

/// Run one distillation pass using the supplied runner.
///
/// This is the testable entry point. Production code calls
/// [`distill_recent_episodes`] which wraps the subprocess runner.
///
/// Contract (see `docs/architecture/episode-distillation.md`):
///
/// - Under threshold → returns `Ok(DistillReport::skipped())`; runner
///   is NOT invoked; no markers set; no facts stored.
/// - Above threshold + recipe success → all input episodes are marked
///   distilled (even when the recipe yielded zero facts).
/// - Above threshold + recipe error → returns `Err(...)`; no markers
///   set; no facts stored.
#[tracing::instrument(skip_all)]
pub fn distill_recent_episodes_with_runner(
    memory: &dyn CognitiveMemoryOps,
    runner: &dyn DistillRecipeRunner,
) -> SimardResult<DistillReport> {
    let episodes = memory.list_undistilled_episodes(DISTILL_BATCH_SIZE)?;
    let pulled = episodes.len() as u32;

    if pulled < DISTILL_MIN_EPISODES {
        tracing::info!(
            target: "simard::distill",
            pulled,
            min = DISTILL_MIN_EPISODES,
            "distill: {pulled} episodes pulled, below min {DISTILL_MIN_EPISODES}, skipped"
        );
        eprintln!(
            "[simard] distill: {pulled} episodes pulled, below min {DISTILL_MIN_EPISODES}, skipped"
        );
        return Ok(DistillReport::skipped());
    }

    tracing::info!(
        target: "simard::distill",
        pulled,
        batch = DISTILL_BATCH_SIZE,
        min = DISTILL_MIN_EPISODES,
        "distill: {pulled} episodes pulled (batch size {DISTILL_BATCH_SIZE}, min {DISTILL_MIN_EPISODES})"
    );
    eprintln!(
        "[simard] distill: {pulled} episodes pulled (batch size {DISTILL_BATCH_SIZE}, min {DISTILL_MIN_EPISODES})"
    );

    // Run the recipe with a bounded in-cycle retry on TRANSIENT failures only
    // (issue #2468). A transient miss — the recipe exited 0 but its step output
    // had no parseable `{ "facts": [...] }` object (`ParseFailure`), or the
    // recipe process exited non-zero (`CopilotTerminalFailure`) — previously
    // deferred the whole batch for a full consolidation cycle. We retry up to
    // `DISTILL_PARSE_RETRY_MAX` times within THIS pass, threading JSON-format
    // reinforcement into the retry (`strict_json = true`). Structural classes
    // (spawn/serialize/recipe-reported failure, and `Other`) are deterministic
    // and escalate immediately — retrying them only wastes an LLM call.
    //
    // The retry-safety invariant is preserved across every attempt: no episode
    // is marked and no fact is stored until an attempt finally parses. On final
    // failure we return `Err` WITHOUT marking, so the batch stays fully
    // retry-eligible on the next pass (the invariant under test in
    // `distillation_handles_recipe_error_without_marking`).
    let mut attempt: u32 = 0;
    let output = loop {
        attempt += 1;
        // Reinforce the response format on any attempt past the first.
        let strict_json = attempt > 1;
        match runner.run_all_reinforced(&episodes, strict_json) {
            Ok(o) => break o,
            Err(e) => {
                let class = classify_distill_error(&e);
                let retries_left = attempt <= DISTILL_PARSE_RETRY_MAX;
                if class.is_transient() && retries_left {
                    tracing::warn!(
                        target: "simard::distill",
                        pulled,
                        attempt,
                        class = class.as_str(),
                        error = %e,
                        "distill: transient failure, retrying in-cycle with format reinforcement"
                    );
                    eprintln!(
                        "[simard] distill: transient {} on attempt {attempt}, retrying in-cycle (strict json)",
                        class.as_str()
                    );
                    continue;
                }
                tracing::warn!(
                    target: "simard::distill",
                    pulled,
                    attempt,
                    class = class.as_str(),
                    error = %e,
                    "distill: {pulled} episodes pulled, recipe error, no markers set, retry next pass"
                );
                eprintln!(
                    "[simard] distill: {pulled} episodes pulled, recipe error: {e}, no markers set, retry next pass"
                );
                // Make the previously-silent non-fatal failure visible (#2461).
                // Recorded BEFORE the early return; episodes stay unmarked, so
                // the retry-safety invariant is unaffected. `attempt` lets the
                // metric distinguish a first-attempt failure from an exhausted
                // retry; a failed pass never "recovered".
                record_distill_success_metric(false, Some(class), pulled, 0, attempt, false);
                return Err(e);
            }
        }
    };
    let recovered_after_retry = attempt > 1;
    let DistillOutput { facts, procedures } = output;

    // ISAO-style reliability gate (issue #2433, BGML §IV). Before promoting a
    // distilled fact into semantic memory we SELF-ASSESS its reliability and
    // gate on `Fact.confidence` instead of writing a blind constant:
    //   * low-reliability candidates are QUARANTINED (not stored) so they can
    //     never corrupt the integrity of past experience, and
    //   * a weaker new fact never CLOBBERS a higher-confidence existing fact on
    //     the same concept (mirrors the cross-session dedup guard in
    //     `memory_consolidation::mod`).
    // Surviving facts are written with the *computed* confidence, turning the
    // formerly-dormant `confidence` column into a live consolidation→recall
    // signal. The originating episode id is still threaded through as
    // provenance (`source_episode_ids`) for the DERIVES_FROM edge (issue #2325).
    let mut stored = 0u32;
    let mut quarantined = 0u32;
    for fact in &facts {
        let confidence = assess_fact_reliability(fact, &episodes, &facts);

        if confidence < DISTILL_RELIABILITY_THRESHOLD {
            quarantined += 1;
            tracing::warn!(
                target: "simard::distill",
                concept = %fact.concept,
                source_episode_id = %fact.source_episode_id,
                confidence,
                threshold = DISTILL_RELIABILITY_THRESHOLD,
                "distill: quarantined low-reliability fact (below threshold), not promoted"
            );
            eprintln!(
                "[simard] distill: quarantined low-reliability fact concept={} confidence={:.2} < {:.2}",
                fact.concept, confidence, DISTILL_RELIABILITY_THRESHOLD
            );
            continue;
        }

        // Protect past experience: do not let a weaker new fact supersede a
        // stronger existing *version of the same fact* (BGML ISAO integrity).
        //
        // The match is on fact IDENTITY (concept + content), NOT the concept
        // label alone. The distillation recipe emits at most three concept
        // labels (`pr-pattern` / `bug-pattern` / `lesson-learned`) and every
        // grounded, well-formed fact scores identically, so a concept-only
        // guard would treat each distilled fact as a duplicate of the first one
        // stored under that label and quarantine every subsequent DISTINCT fact
        // — silently neutering distillation after the first pass (issue #2433).
        // Identity matching instead blocks only a genuine re-distillation of the
        // *same* content at a lower-or-equal confidence, while letting distinct
        // lessons that happen to share a label accumulate.
        //
        // `search_facts` is queried with the new confidence as `min_confidence`,
        // so it returns only priors strong enough to block; production
        // `search_facts` matches the concept label against the same live graph
        // these writes target, so an in-pass sibling is visible here too. The
        // explicit `>=` comparison is belt-and-suspenders against a backend that
        // ignores the `min_confidence` filter.
        let existing = memory
            .search_facts(&fact.concept, 5, confidence)
            .unwrap_or_default();
        let new_content = fact.content.trim();
        if existing
            .iter()
            .any(|f| f.content.trim() == new_content && f.confidence >= confidence)
        {
            quarantined += 1;
            tracing::info!(
                target: "simard::distill",
                concept = %fact.concept,
                confidence,
                "distill: an equal-or-stronger copy of this fact already exists; not downgrading prior"
            );
            eprintln!(
                "[simard] distill: kept stronger prior for concept={} (new confidence={:.2} would downgrade an identical fact)",
                fact.concept, confidence
            );
            continue;
        }

        let source = format!("distill:{}", fact.source_episode_id);
        memory.store_fact_with_provenance(
            &fact.concept,
            &fact.content,
            confidence,
            &source,
            Some(std::slice::from_ref(&fact.concept)),
            None,
            std::slice::from_ref(&fact.source_episode_id),
        )?;
        stored += 1;
    }

    // Before/after measurement of the gate's block-rate (issue #2433
    // acceptance). Best-effort, no-op under `cfg!(test)` so unit tests never
    // append to the operator's real metrics file.
    record_reliability_gate_metric(facts.len() as u32, quarantined, stored);

    // Store every procedure via the provenance write path (issue #2327, R5).
    // `source_episode_ids` are threaded so a `PROCEDURE_DERIVES_FROM` edge links
    // the procedure back to the recurring episodes it was distilled from. The
    // upsert-by-name in the backend reinforces an existing procedure rather than
    // duplicating it (#2298).
    let mut stored_procs = 0u32;
    for proc in &procedures {
        memory.store_procedure_with_provenance(
            &proc.name,
            &proc.steps,
            &[],
            &proc.source_episode_ids,
        )?;
        stored_procs += 1;
    }

    // Mark EVERY input episode distilled — even those the recipe
    // classified `skip` (they contribute no fact but must not be
    // re-fed to the LLM on the next pass). The mark-everything rule
    // is the prompt-replay-loop guard documented in
    // `episode-distillation.md` §"Pipeline overview".
    let mut marked = 0u32;
    for ep in &episodes {
        memory.mark_episode_distilled(&ep.node_id)?;
        marked += 1;
    }

    let reduction_pct = (1.0 - stored as f64 / pulled as f64) * 100.0;
    tracing::info!(
        target: "simard::distill",
        pulled,
        stored,
        stored_procs,
        marked,
        reduction_pct,
        "distill: {pulled} episodes → {stored} facts, {stored_procs} procedures, {marked} marked (reduction {reduction_pct:.0}%)"
    );
    eprintln!(
        "[simard] distill: {pulled} episodes → {stored} facts, {stored_procs} procedures, {marked} marked"
    );

    // The recipe ran and its output parsed (#2461): record a success event so
    // distill_success_rate / distill_parse_success_rate are measurable. NOTE:
    // this point is only reached after the storage writes above succeeded; a
    // downstream memory-write failure propagates as `Err` (a separate subsystem,
    // out of this recipe-stage metric's scope — see record_distill_success_metric).
    // `attempt` / `recovered_after_retry` (#2468) let the metric distinguish a
    // first-attempt success from one recovered by an in-cycle retry.
    record_distill_success_metric(true, None, pulled, stored, attempt, recovered_after_retry);

    Ok(DistillReport {
        input_count: pulled,
        fact_count: stored,
        procedure_count: stored_procs,
        marked_count: marked,
        quarantined_count: quarantined,
    })
}

/// Self-assess the reliability of one distilled fact (issue #2433, BGML's
/// *information self-assessment ownership*, §IV). Returns a confidence score in
/// `[0.0, 1.0]` from cheap, locally-available signals — no extra LLM call:
///
/// | Signal | Weight | Rationale |
/// |--------|--------|-----------|
/// | **Provenance grounding** | 0.5 | The cited `source_episode_id` must be one of the episodes actually fed to the recipe this pass. A source outside the batch is unverifiable / hallucinated provenance — the strongest unreliability signal. |
/// | **Content quality** | ≤0.3 | Empty / whitespace-only content carries no information and is a HARD gate (score `0.0`); otherwise ≥3 words earns the full weight. |
/// | **Concept validity** | 0.1 | The recipe is constrained to [`KNOWN_DISTILL_CONCEPTS`]; an off-set concept means the model went off-spec. |
/// | **Corroboration** | 0.1 | ≥2 distilled facts agreeing on the same concept this pass — independent agreement across source episodes. Awarded ONLY to grounded facts so hallucinated provenance can't ride on a sibling's corroboration. |
///
/// A nominal fact (grounded, ≥3 words, known concept) scores `0.9` — at or
/// above the legacy [`DISTILL_FACT_CONFIDENCE`] baseline — so good facts keep
/// their downstream behaviour. Because grounding (0.5) is necessary to clear
/// [`DISTILL_RELIABILITY_THRESHOLD`] (0.5), a hallucinated-provenance fact tops
/// out at `0.4` (content + concept) — even WITH corroboration — and an empty
/// fact scores `0.0`; both are quarantined.
pub fn assess_fact_reliability(
    fact: &DistilledFact,
    episodes: &[CognitiveEpisode],
    batch_facts: &[DistilledFact],
) -> f64 {
    // (0) Hard gate: empty / whitespace-only content carries no information and
    // is quarantined unconditionally, regardless of how trustworthy its
    // provenance looks (issue #2433). Without this, a grounded-but-empty fact
    // (0.5 grounding + 0.1 concept = 0.6) would clear the gate, violating the
    // documented "empty content is quarantined" invariant.
    let words = fact.content.split_whitespace().count();
    if words == 0 {
        return 0.0;
    }

    let mut score = 0.0_f64;

    // (1) Provenance grounding — the dominant, *necessary* signal. A source
    // outside the batch is unverifiable / hallucinated provenance. The weights
    // below are tuned so that WITHOUT this 0.5 a fact can never reach
    // `DISTILL_RELIABILITY_THRESHOLD` (0.5): an ungrounded fact tops out at
    // 0.3 content + 0.1 concept = 0.4 and is always quarantined.
    let grounded = episodes.iter().any(|e| e.node_id == fact.source_episode_id);
    if grounded {
        score += 0.5;
    }

    // (2) Content quality (content is non-empty here — see the hard gate above).
    if words >= 3 {
        score += 0.3;
    } else {
        score += 0.15;
    }

    // (3) Concept validity.
    let concept = fact.concept.trim();
    if KNOWN_DISTILL_CONCEPTS
        .iter()
        .any(|c| c.eq_ignore_ascii_case(concept))
    {
        score += 0.1;
    }

    // (4) Corroboration: ≥2 facts this pass agreeing on the same concept.
    // Awarded ONLY to grounded facts — an ungrounded (hallucinated-provenance)
    // fact must not ride on the corroboration of legitimate same-concept facts
    // to sneak over the gate (it would otherwise reach 0.3 + 0.1 + 0.1 = 0.5).
    if grounded {
        let corroboration = batch_facts
            .iter()
            .filter(|f| f.concept.eq_ignore_ascii_case(&fact.concept))
            .count();
        if corroboration >= 2 {
            score += 0.1;
        }
    }

    score.clamp(0.0, 1.0)
}

/// Build the JSON `context` payload for the `distill_reliability_gate` metric.
/// Separated from the I/O so the payload shape is unit-testable without
/// touching the real `metrics.jsonl`.
fn build_reliability_gate_context(candidate_facts: u32, quarantined: u32, promoted: u32) -> String {
    let block_rate = if candidate_facts == 0 {
        0.0
    } else {
        quarantined as f64 / candidate_facts as f64
    };
    serde_json::json!({
        "candidate_facts": candidate_facts,
        "promoted": promoted,
        "quarantined": quarantined,
        "block_rate": block_rate,
        "threshold": DISTILL_RELIABILITY_THRESHOLD,
    })
    .to_string()
}

/// Record one `distill_reliability_gate` metric event per distillation pass so
/// the block-rate (`quarantined / candidate_facts`) is measurable from
/// `metrics.jsonl` (issue #2433 acceptance: a before/after measurement of the
/// gate). The metric `value` is the block-rate fraction.
///
/// Best-effort: a metrics-write failure is logged, never propagated. No-op
/// under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl`.
fn record_reliability_gate_metric(candidate_facts: u32, quarantined: u32, promoted: u32) {
    if cfg!(test) {
        return;
    }
    let block_rate = if candidate_facts == 0 {
        0.0
    } else {
        quarantined as f64 / candidate_facts as f64
    };
    let context = build_reliability_gate_context(candidate_facts, quarantined, promoted);
    if let Err(e) =
        crate::self_metrics::record_metric("distill_reliability_gate", block_rate, &context)
    {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_reliability_gate metric (distillation unaffected)",
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// distill_success_rate instrumentation (issue #2461)
// ───────────────────────────────────────────────────────────────────────────
//
// A distillation failure is *non-fatal*: `dispatch_consolidate_memory` folds
// the `Err` into a human-readable string and still reports the action
// successful. Without a counter the failure frequency — and therefore the
// silent degradation of semantic recall — is invisible. We record a
// `distill_success_rate` metric event per pass that actually ran the recipe so
// both the success rate and the parse-success rate are measurable from
// `metrics.jsonl`, mirroring `record_reliability_gate_metric` (#2433).

/// Machine-readable class of a distillation failure, derived from the stable
/// leading prefix of the `SimardError::BridgeError` message emitted at each
/// runner/parse site in this module. Covers every `Err` `run_all` can surface,
/// including the **production** `--output-format json` envelope path (where a
/// parse failure manifests as a *step-output* parse error, not the legacy
/// bare-stdout one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistillFailureClass {
    /// `recipe-runner-rs` could not be spawned (binary missing/structural).
    SpawnFailure,
    /// The recipe **process** exited non-zero (#2461 t=7411 case).
    CopilotTerminalFailure,
    /// The recipe process exited 0 but the run did not yield a usable
    /// completed `distill` step (envelope `success=false`, or no completed
    /// step). The recipe ran but produced no parseable agent output to parse.
    RecipeReportedFailure,
    /// The recipe process exited 0 and a step ran, but its output had no
    /// parseable `{ "facts": [...] }` object (#2461 t=7517 case). This is the
    /// only failure class that actually *reached* output parsing.
    ParseFailure,
    /// The episodes payload failed to serialize (structural; ~unreachable
    /// since the payload is built from infallible `json!` values).
    SerializeFailure,
    /// Any other error (e.g. a backend error surfaced through the runner).
    Other,
}

impl DistillFailureClass {
    /// Stable label used in the metric context.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpawnFailure => "spawn-failure",
            Self::CopilotTerminalFailure => "copilot-terminal-failure",
            Self::RecipeReportedFailure => "recipe-reported-failure",
            Self::ParseFailure => "parse-failure",
            Self::SerializeFailure => "serialize-failure",
            Self::Other => "other",
        }
    }

    /// `true` when the recipe **process** exited 0. False for spawn/terminal
    /// (never started / non-zero exit) and serialize (never spawned).
    pub fn recipe_exited_ok(self) -> bool {
        matches!(self, Self::ParseFailure | Self::RecipeReportedFailure)
    }

    /// `true` when the run actually *reached output parsing* (a step ran and
    /// its output was parsed). This is the denominator gate for
    /// `distill_parse_success_rate` — only `ParseFailure` qualifies among
    /// failures (`RecipeReportedFailure` exited 0 but produced no step output
    /// to parse).
    pub fn reached_parsing(self) -> bool {
        matches!(self, Self::ParseFailure)
    }

    /// `true` for failure classes worth a bounded in-cycle retry (issue #2468):
    /// a parse miss (exited 0, output unparseable) or a non-zero recipe exit.
    /// These are observed to recover on a second attempt — often with JSON
    /// format reinforcement. Structural classes (spawn/serialize/recipe-reported
    /// failure, and `Other`) are deterministic and escalate immediately instead.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::ParseFailure | Self::CopilotTerminalFailure)
    }
}

/// Classify a distillation `SimardError` into a [`DistillFailureClass`].
///
/// Discrimination anchors on the **stable leading prefix** this module emits
/// for each class (`SimardError::BridgeError(msg)` → `msg.starts_with(...)`),
/// NOT on `contains`. Several messages embed foreign text (terminal failures
/// carry up to 200 chars each of recipe stderr/stdout; parse failures carry the
/// agent output excerpt), and that variable tail always trails the fixed prefix
/// — so anchoring keeps a non-zero exit from being misread as a parse-failure
/// and corrupting `distill_parse_success_rate`.
///
/// The prefixes mirror the seven `BridgeError` sites in this file; the
/// production `--output-format json` path's step-output parse error
/// (`distill: \`distill\` step output did not contain a parseable …`) is the
/// t=7517 manifestation and MUST map to `ParseFailure`, since production never
/// reaches the legacy bare-stdout `did not yield a parseable` path.
pub fn classify_distill_error(err: &SimardError) -> DistillFailureClass {
    let SimardError::BridgeError(msg) = err else {
        return DistillFailureClass::Other;
    };
    if msg.starts_with("distill: recipe-runner-rs spawn failed") {
        DistillFailureClass::SpawnFailure
    } else if msg.starts_with("distill: recipe exited with") {
        DistillFailureClass::CopilotTerminalFailure
    } else if msg.starts_with("distill: failed to serialize episodes payload") {
        DistillFailureClass::SerializeFailure
    } else if msg.starts_with("distill: recipe-runner reported failure")
        || msg.starts_with("distill: recipe envelope had no completed")
    {
        DistillFailureClass::RecipeReportedFailure
    } else if msg.starts_with("distill: `distill` step output did not contain a parseable")
        || msg.starts_with("distill: recipe run did not yield a parseable")
    {
        DistillFailureClass::ParseFailure
    } else {
        DistillFailureClass::Other
    }
}

/// Build the `distill_success_rate` metric context for one pass that ran the
/// recipe. `class` is `None` on success, `Some(_)` on failure.
///
/// `attempt` (1-based) is the number of runner invocations this pass made, and
/// `recovered_after_retry` is `true` only when a success followed at least one
/// transient retry (issue #2468) — together they let `distill_parse_success_rate`
/// distinguish a first-attempt success from one recovered by an in-cycle retry.
fn build_distill_success_context(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
    attempt: u32,
    recovered_after_retry: bool,
) -> String {
    let recipe_exited_ok = success || class.is_some_and(|c| c.recipe_exited_ok());
    // Parsing was attempted iff a step ran and its output was parsed — true on
    // success and for a parse-failure, but NOT for a recipe-reported failure
    // that exited 0 without producing step output.
    let parse_attempted = success || class.is_some_and(|c| c.reached_parsing());
    serde_json::json!({
        "outcome": if success { "success" } else { "failure" },
        "recipe_exited_ok": recipe_exited_ok,
        "parse_attempted": parse_attempted,
        "parse_success": success,
        "failure_class": class.map(|c| c.as_str()),
        "input_count": input_count,
        "fact_count": fact_count,
        "attempt": attempt,
        "recovered_after_retry": recovered_after_retry,
    })
    .to_string()
}

/// Record one `distill_success_rate` metric event for one distillation pass.
///
/// **Scope: the recipe + output-parse stage** — exactly the failure surface of
/// issue #2461 (the distill *recipe* exiting non-zero, or exiting 0 with
/// unparseable output). It is recorded for every pass that ran the recipe
/// (success OR a recipe/parse failure); below-threshold skips are excluded.
/// Downstream memory-write failures (`store_fact_with_provenance`,
/// `store_procedure_with_provenance`, `mark_episode_distilled`) are a *separate*
/// subsystem and are intentionally NOT folded into this metric: they propagate
/// as `Err` from `distill_recent_episodes_with_runner` and are surfaced through
/// the normal error path, so counting them here would conflate backend-write
/// reliability with recipe reliability. (This mirrors the placement of
/// `record_reliability_gate_metric`, which likewise covers only the recipe pass.)
///
/// The metric `value` is `1.0` on success and `0.0` on a recipe/parse failure,
/// so the mean over passes is the success rate. The context's `parse_attempted`
/// / `parse_success` flags additionally yield `distill_parse_success_rate`,
/// isolating the "exited 0 but unparseable" mode (#2461 t=7517) from the
/// "exited non-zero" mode (t=7411). This makes the previously-silent non-fatal
/// distill failures visible in `metrics.jsonl` without committing any
/// point-in-time findings.
///
/// Best-effort: a metrics-write failure is logged, never propagated. No-op
/// under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl`.
fn record_distill_success_metric(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
    attempt: u32,
    recovered_after_retry: bool,
) {
    if cfg!(test) {
        return;
    }
    let value = if success { 1.0 } else { 0.0 };
    let context = build_distill_success_context(
        success,
        class,
        input_count,
        fact_count,
        attempt,
        recovered_after_retry,
    );
    if let Err(e) = crate::self_metrics::record_metric("distill_success_rate", value, &context) {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_success_rate metric (distillation unaffected)",
        );
    }
}

/// Production entry point: run one distillation pass using the
/// `recipe-runner-rs` subprocess.
///
/// Returns `Ok(DistillReport::skipped())` when the runner cannot be
/// constructed (e.g. `recipe-runner-rs` not on PATH, no recipe file,
/// no agent binary). This matches the "skip gracefully" behaviour of
/// other recipe shims (`RecipeProgressChecker`, `RecipeMergeJudge`)
/// so distillation never blocks the OODA cycle.
#[tracing::instrument(skip_all)]
pub fn distill_recent_episodes(
    memory: &dyn CognitiveMemoryOps,
    repo_root: &Path,
) -> SimardResult<DistillReport> {
    match RecipeRunnerSubprocess::new(repo_root) {
        Some(runner) => distill_recent_episodes_with_runner(memory, &runner),
        None => {
            tracing::info!(
                target: "simard::distill",
                "distill: recipe-runner-rs unavailable or recipe file missing; skipping pass"
            );
            Ok(DistillReport::skipped())
        }
    }
}

const RECIPE_FILENAME: &str = "distill-episodes.yaml";

/// Format-reinforcement sentence threaded into the distill recipe on a retry
/// after a transient parse miss (issue #2468). Interpolated into the recipe via
/// the `{{strict_json_instruction}}` substitution; empty on the first attempt.
const STRICT_JSON_INSTRUCTION: &str = "Your previous reply was not parseable. Respond with ONLY the JSON object \
     {\"facts\":[...]} (and \"procedures\" if any) — no prose, no markdown fence, \
     no thinking, nothing before or after the object.";

fn resolve_recipe_path(repo_root: &Path) -> Option<std::path::PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Concrete subprocess-based recipe runner. Shells out to
/// `recipe-runner-rs` with the episodes JSON inlined as a single
/// `-c episodes=<json>` context entry.
pub struct RecipeRunnerSubprocess {
    recipe_path: std::path::PathBuf,
    agent_binary: &'static str,
}

impl RecipeRunnerSubprocess {
    /// Construct a runner if all preconditions are met. Returns
    /// `None` when:
    ///
    /// - The recipe file is not found in either the hot-reload or
    ///   in-tree location.
    /// - No agent binary is configured (no `AMPLIHACK_AGENT_BINARY`
    ///   env var, no fallback path).
    /// - `recipe-runner-rs` is not on `PATH`.
    pub fn new(repo_root: &Path) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
        })
    }
}

impl DistillRecipeRunner for RecipeRunnerSubprocess {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        let raw = self.invoke_recipe(episodes, false)?;
        let parsed = parse_recipe_output(&raw);
        crate::recipe_output::record_parse_outcome("distill", parsed.is_ok());
        parsed
    }

    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        let raw = self.invoke_recipe(episodes, false)?;
        let parsed = parse_recipe_output_full(&raw);
        crate::recipe_output::record_parse_outcome("distill", parsed.is_ok());
        parsed
    }

    fn run_all_reinforced(
        &self,
        episodes: &[CognitiveEpisode],
        strict_json: bool,
    ) -> SimardResult<DistillOutput> {
        // Issue #2468: on a retry (`strict_json = true`) thread a non-empty
        // `strict_json_instruction` context var into the recipe so the agent
        // reply is reinforced to be ONLY the `{ "facts": [...] }` object.
        let raw = self.invoke_recipe(episodes, strict_json)?;
        let parsed = parse_recipe_output_full(&raw);
        crate::recipe_output::record_parse_outcome("distill", parsed.is_ok());
        parsed
    }
}

impl RecipeRunnerSubprocess {
    /// Shell out to `recipe-runner-rs` with the episodes payload and return
    /// the recipe's raw stdout. Shared by [`run`](DistillRecipeRunner::run)
    /// and [`run_all`](DistillRecipeRunner::run_all).
    ///
    /// `strict_json` (issue #2468) threads a `strict_json_instruction` context
    /// var into the recipe: empty on the first attempt, and a "respond with ONLY
    /// the `{ \"facts\": [...] }` object" reinforcement sentence on a retry after
    /// a transient parse miss. The recipe interpolates it via the pure
    /// `{{strict_json_instruction}}` substitution, so no conditional templating
    /// engine is needed.
    fn invoke_recipe(
        &self,
        episodes: &[CognitiveEpisode],
        strict_json: bool,
    ) -> SimardResult<String> {
        // Serialize episodes as a compact JSON array. Each entry
        // exposes the four fields the recipe prompt references:
        // `id`, `source_label`, `temporal_index`, `content`.
        let payload: Vec<serde_json::Value> = episodes
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.node_id,
                    "source_label": e.source_label,
                    "temporal_index": e.temporal_index,
                    "content": e.content,
                })
            })
            .collect();
        let payload_json = serde_json::to_string(&payload).map_err(|e| {
            SimardError::BridgeError(format!(
                "distill: failed to serialize episodes payload: {e}"
            ))
        })?;

        // `--output-format json` is REQUIRED: in the default `text` mode the
        // runner's stdout is only a human status banner and the distill agent's
        // `{ "facts": [...] }` payload never reaches us, so every parse fails and
        // the pass silently no-ops (issue #2401). In `json` mode stdout carries a
        // structured envelope whose `step_results[].output` holds the agent's
        // output, which `parse_recipe_output_full` mines. The agent binary is
        // still selected via the proven `AMPLIHACK_AGENT_BINARY` env var.
        //
        // `strict_json_instruction` (issue #2468) is always passed so the
        // recipe's `{{strict_json_instruction}}` substitution resolves: empty on
        // the first attempt, and a format-reinforcement sentence on a retry.
        let strict_json_instruction = if strict_json {
            STRICT_JSON_INSTRUCTION
        } else {
            ""
        };
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("--output-format")
            .arg("json")
            .arg("-c")
            .arg(format!("episodes={payload_json}"))
            .arg("-c")
            .arg(format!("strict_json_instruction={strict_json_instruction}"))
            .output()
            .map_err(|e| {
                SimardError::BridgeError(format!("distill: recipe-runner-rs spawn failed: {e}"))
            })?;

        if !output.status.success() {
            // On failure the runner exits non-zero AND emits the structured
            // error inside the JSON envelope on stdout (stderr may be empty), so
            // surface both — never a silent or context-free failure.
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            return Err(SimardError::BridgeError(format!(
                "distill: recipe exited with {}: stderr={} stdout={}",
                output.status,
                truncate(stderr.trim(), 200),
                truncate(stdout.trim(), 200)
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// Parse the recipe's stdout into a list of [`DistilledFact`].
///
/// Thin facts-only wrapper over [`parse_recipe_output_full`] retained for the
/// legacy [`DistillRecipeRunner::run`] entry point and its unit tests.
pub(crate) fn parse_recipe_output(raw: &str) -> SimardResult<Vec<DistilledFact>> {
    parse_recipe_output_full(raw).map(|o| o.facts)
}

/// Parse `recipe-runner-rs`'s stdout into a [`DistillOutput`] (facts AND
/// procedures).
///
/// Three tolerant tiers, in order (issue #2401):
///
/// 1. **Runner envelope (production path).** With `--output-format json` the
///    runner emits `{ "recipe_name", "success", "step_results": [...], ... }`.
///    We require `success == true`, select the `distill` step (or, if renamed,
///    the last `completed` step), and mine the agent payload out of that
///    step's `output` — which is itself a JSON *string* that may carry leading
///    prose (e.g. a `NODE_OPTIONS` banner) before the `{ "facts": ... }`
///    object, so we balanced-brace scan it. A future runner that emits `output`
///    as a JSON object is handled too.
/// 2. **Bare-object fallback.** If stdout is not a runner envelope, scan it
///    directly for an embedded `{ "facts": ... }` object. Keeps the legacy
///    fact-only mock/unit-test contract (and prose-wrapped agent output)
///    working.
/// 3. **Explicit failure.** If neither tier yields a facts object — including
///    the `--output-format text` status banner and any `success == false`
///    envelope — return `Err`. The caller treats `Err` as the retry-safe
///    "no markers set" path; there is never a hollow `Ok`.
pub(crate) fn parse_recipe_output_full(raw: &str) -> SimardResult<DistillOutput> {
    let trimmed = raw.trim();

    // Tier 1 — recipe-runner-rs JSON envelope. `RecipeRunnerEnvelope` requires
    // both `success` and `step_results`, so a bare `{ "facts": ... }` object
    // (which has neither) fails this parse and falls through to Tier 2.
    if let Ok(envelope) = serde_json::from_str::<RecipeRunnerEnvelope>(trimmed) {
        return envelope.into_distill_output();
    }

    // Tier 2 — tolerant fallback: an embedded bare `{ "facts": ... }` object in
    // arbitrary prose (legacy contract; also covers any non-envelope stdout).
    if let Some(output) = scan_for_facts_object(trimmed) {
        return Ok(output);
    }

    // Tier 3 — explicit, bounded failure.
    Err(SimardError::BridgeError(format!(
        "distill: recipe run did not yield a parseable {{ \"facts\": [...] }} object: {}",
        truncate(raw, 200)
    )))
}

/// Scan `text` for a balanced `{...}` substring that deserializes as a
/// [`RecipeEnvelope`] (a bare `{ "facts": [...], "procedures": [...] }` object),
/// tolerating leading/trailing prose. Returns `None` if none is found.
///
/// When several balanced objects are present the **last non-empty** one that
/// parses wins: a leading banner/status object (e.g. a `NODE_OPTIONS` notice,
/// or a thinking trace the agent emitted before its answer) no longer shadows
/// the real facts object that trails it — this is the t=7517 / #2461
/// parse-failure mode where a first-object scan locked onto the banner and the
/// whole pass silently no-opped. Preferring the last *non-empty* object (and
/// only falling back to an empty `{"facts":[]}` when none carry
/// facts/procedures) keeps a legitimate "nothing worth distilling" answer while
/// preventing an accidental trailing empty object from discarding earlier facts.
///
/// The scan is iterative (no recursion) so pathologically deep brace nesting
/// terminates without a stack overflow; serde's own recursion limit bounds the
/// per-candidate parse. Brace scanning is string-aware (it ignores `{`/`}`
/// inside JSON string literals, respecting `\"` escapes) so a brace inside a
/// fact's `content` cannot corrupt depth accounting.
fn scan_for_facts_object(text: &str) -> Option<DistillOutput> {
    // Try two cleaned views of the noisy step output, mirroring the shared
    // `recipe_output::extract_json_payload` dual pass (issue #2484):
    //
    // 1. `strip_recipe_noise` strips ANSI escapes AND drops whole tracing/log
    //    and runner-banner lines — this removes the `\x1b[2m<RFC3339 timestamp>`
    //    log prefix that made `serde_json` reject the span (the t=7919 / #2461 /
    //    #2484 distill parse-failure) and recovers a payload whose pretty body
    //    has an interleaved tracing line.
    // 2. `strip_ansi` is line-preserving — it recovers a payload that sits on
    //    the SAME physical line as a leading timestamp/log prefix, which view 1
    //    would otherwise drop wholesale.
    //
    // Both views are clean-path zero-copy (`Cow::Borrowed`) when stdout carries
    // no `ESC` byte and no droppable line, so adopting the shared helper does
    // not change behaviour on clean output. A non-empty facts object from
    // either view wins; an empty `{"facts":[]}` "nothing to distill" answer is
    // kept only as a fallback when neither view yields a populated object.
    let mut empty_fallback: Option<DistillOutput> = None;
    for cleaned in [
        crate::recipe_output::strip_recipe_noise(text),
        crate::recipe_output::strip_ansi(text),
    ] {
        if let Some(output) = scan_cleaned_for_facts(cleaned.trim()) {
            if !output.facts.is_empty() || !output.procedures.is_empty() {
                return Some(output);
            }
            if empty_fallback.is_none() {
                empty_fallback = Some(output);
            }
        }
    }
    empty_fallback
}

/// Scan an already noise-stripped `trimmed` string for a `{ "facts": [...] }`
/// object, preferring the LAST *non-empty* balanced top-level object (the
/// agent's final answer trails any banner/thinking output) and falling back to
/// the last parseable empty object. The ANSI/log/banner stripping that produces
/// `trimmed` lives in [`scan_for_facts_object`].
fn scan_cleaned_for_facts(trimmed: &str) -> Option<DistillOutput> {
    // Fast path — the text IS the JSON object.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(trimmed) {
        return Some(parsed.into_output());
    }
    // Slow path — among every balanced top-level `{...}` substring, prefer the
    // facts from the LAST *non-empty* one that parses. Falling back to the last
    // parseable object only when none carry facts/procedures preserves a
    // legitimate `{"facts":[]}` "nothing worth distilling" answer while never
    // letting an accidental trailing empty object shadow an earlier populated one.
    let mut empty_fallback: Option<DistillOutput> = None;
    for (start, end) in balanced_object_spans(trimmed).into_iter().rev() {
        if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(&trimmed[start..end]) {
            let output = parsed.into_output();
            if !output.facts.is_empty() || !output.procedures.is_empty() {
                return Some(output);
            }
            if empty_fallback.is_none() {
                empty_fallback = Some(output);
            }
        }
    }
    empty_fallback
}

/// Return the byte spans `[start, end)` of every top-level balanced `{...}`
/// object in `s`, in source order.
///
/// String-aware **inside objects**: once a `{` has opened (depth > 0), `{`/`}`
/// bytes inside a JSON string literal (respecting `\"` escapes) are ignored.
/// Quotes encountered at depth 0 (e.g. an unmatched quote in leading prose)
/// are treated as ordinary characters so they cannot swallow the object's `{`
/// opener. Unbalanced trailing `{` are silently dropped.
fn balanced_object_spans(s: &str) -> Vec<(usize, usize)> {
    let bytes = s.as_bytes();
    let mut spans = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' if depth > 0 => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            b'}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    spans.push((start, i + 1));
                }
            }
            _ => {}
        }
    }
    spans
}

/// The `recipe-runner-rs` 0.3.6 `--output-format json` envelope. Only the
/// fields the parser needs are modelled; unknown fields (`recipe_name`,
/// `duration`, `context`, …) are ignored. Both `success` and `step_results`
/// are REQUIRED so this type does not accidentally match a bare facts object.
#[derive(serde::Deserialize)]
struct RecipeRunnerEnvelope {
    success: bool,
    step_results: Vec<RecipeRunnerStepResult>,
}

/// One entry of `step_results[]`. `output` is a [`serde_json::Value`] because
/// the runner emits it as a JSON *string* today but a future version could emit
/// it as an object — the parser handles both.
#[derive(serde::Deserialize)]
struct RecipeRunnerStepResult {
    #[serde(default)]
    step_id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    output: serde_json::Value,
    #[serde(default)]
    error: String,
}

impl RecipeRunnerEnvelope {
    /// Extract the distill agent's facts/procedures from the envelope.
    ///
    /// `success == false` short-circuits to `Err` BEFORE any step output is
    /// read — a failed run is never trusted, even if it somehow carries a
    /// well-formed payload.
    fn into_distill_output(self) -> SimardResult<DistillOutput> {
        if !self.success {
            return Err(SimardError::BridgeError(format!(
                "distill: recipe-runner reported failure (success=false): {}",
                truncate(self.first_error().trim(), 200)
            )));
        }
        let step = self.select_distill_step().ok_or_else(|| {
            SimardError::BridgeError(
                "distill: recipe envelope had no completed `distill` step".to_string(),
            )
        })?;
        extract_step_output(&step.output).ok_or_else(|| {
            SimardError::BridgeError(format!(
                "distill: `distill` step output did not contain a parseable \
                 {{ \"facts\": [...] }} object; output: {}",
                truncate(step_output_excerpt(&step.output).trim(), 200)
            ))
        })
    }

    /// Select the step to read facts from: the one with `step_id == "distill"`,
    /// or — tolerating a future step rename — the last `completed` step.
    fn select_distill_step(&self) -> Option<&RecipeRunnerStepResult> {
        self.step_results
            .iter()
            .find(|s| s.step_id == "distill")
            .or_else(|| {
                self.step_results
                    .iter()
                    .rev()
                    .find(|s| s.status == "completed")
            })
    }

    /// First non-empty step error, for failure messages.
    fn first_error(&self) -> String {
        self.step_results
            .iter()
            .map(|s| s.error.as_str())
            .find(|e| !e.trim().is_empty())
            .unwrap_or("<no step error reported>")
            .to_string()
    }
}

/// Mine a [`DistillOutput`] from a step's `output` value. A JSON *string* is
/// balanced-brace scanned (it may carry leading prose); a JSON *object*
/// carrying `facts` is deserialized directly.
fn extract_step_output(output: &serde_json::Value) -> Option<DistillOutput> {
    match output {
        serde_json::Value::String(s) => scan_for_facts_object(s),
        serde_json::Value::Object(_) => serde_json::from_value::<RecipeEnvelope>(output.clone())
            .ok()
            .map(|e| e.into_output()),
        _ => None,
    }
}

/// A bounded, human-readable excerpt of a step `output` value for error
/// messages (avoids quoting/escaping a `Value::String` twice).
fn step_output_excerpt(output: &serde_json::Value) -> String {
    match output {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[derive(serde::Deserialize)]
struct RecipeEnvelope {
    facts: Vec<RecipeFact>,
    #[serde(default)]
    procedures: Vec<RecipeProcedure>,
}

#[derive(serde::Deserialize)]
struct RecipeFact {
    concept: String,
    content: String,
    source_episode_id: String,
}

#[derive(serde::Deserialize)]
struct RecipeProcedure {
    name: String,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    source_episode_ids: Vec<String>,
}

impl RecipeEnvelope {
    fn into_facts(self) -> Vec<DistilledFact> {
        // Keep only the three documented concepts so the recipe cannot
        // sneak new labels past the contract. Everything else (incl.
        // `skip` if it ever lands here) is dropped.
        self.facts
            .into_iter()
            .filter(|f| {
                matches!(
                    f.concept.as_str(),
                    "pr-pattern" | "bug-pattern" | "lesson-learned"
                )
            })
            .map(|f| DistilledFact {
                concept: f.concept,
                content: f.content,
                source_episode_id: f.source_episode_id,
            })
            .collect()
    }

    fn into_procedures(self) -> Vec<DistilledProcedure> {
        // Keep only procedures that name at least one source episode so a
        // `PROCEDURE_DERIVES_FROM` edge can actually be drawn, and that carry
        // at least one step. Unnamed / empty procedures are dropped.
        self.procedures
            .into_iter()
            .filter(|p| {
                !p.name.trim().is_empty() && !p.steps.is_empty() && !p.source_episode_ids.is_empty()
            })
            .map(|p| DistilledProcedure {
                name: p.name,
                steps: p.steps,
                source_episode_ids: p.source_episode_ids,
            })
            .collect()
    }

    fn into_output(self) -> DistillOutput {
        let RecipeEnvelope { facts, procedures } = self;
        DistillOutput {
            facts: RecipeEnvelope {
                facts,
                procedures: Vec::new(),
            }
            .into_facts(),
            procedures: RecipeEnvelope {
                facts: Vec::new(),
                procedures,
            }
            .into_procedures(),
        }
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn reliability_gate_metric_context_shape() {
        // 5 candidates, 2 quarantined → block_rate 0.4.
        let payload = build_reliability_gate_context(5, 2, 3);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["candidate_facts"], 5);
        assert_eq!(v["promoted"], 3);
        assert_eq!(v["quarantined"], 2);
        assert_eq!(v["block_rate"], 0.4);
        assert_eq!(v["threshold"], DISTILL_RELIABILITY_THRESHOLD);
    }

    #[test]
    fn reliability_gate_metric_context_zero_candidates_is_zero_rate() {
        let payload = build_reliability_gate_context(0, 0, 0);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["block_rate"], 0.0);
    }

    #[test]
    fn report_skipped_is_was_skipped() {
        assert!(DistillReport::skipped().was_skipped());
    }

    #[test]
    fn report_with_work_is_not_was_skipped() {
        let r = DistillReport {
            input_count: 25,
            fact_count: 3,
            procedure_count: 0,
            marked_count: 25,
            quarantined_count: 0,
        };
        assert!(!r.was_skipped());
    }

    #[test]
    fn reduction_is_zero_when_no_inputs() {
        assert_eq!(DistillReport::skipped().reduction(), 0.0);
    }

    #[test]
    fn reduction_is_88_percent_for_25_to_3() {
        let r = DistillReport {
            input_count: 25,
            fact_count: 3,
            procedure_count: 0,
            marked_count: 25,
            quarantined_count: 0,
        };
        let pct = r.reduction() * 100.0;
        assert!((pct - 88.0).abs() < 0.1, "expected ~88%, got {pct}");
    }

    #[test]
    fn parse_recipe_output_accepts_plain_object() {
        let raw =
            r#"{"facts":[{"concept":"pr-pattern","content":"x","source_episode_id":"epi_1"}]}"#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "pr-pattern");
    }

    #[test]
    fn parse_recipe_output_extracts_json_from_prose() {
        let raw = r#"Sure, here is the JSON:
            {"facts":[{"concept":"bug-pattern","content":"y","source_episode_id":"epi_2"}]}
            That's all."#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
    }

    #[test]
    fn parse_recipe_output_drops_unknown_concepts() {
        let raw = r#"{"facts":[
            {"concept":"made-up-label","content":"a","source_episode_id":"epi_1"},
            {"concept":"lesson-learned","content":"b","source_episode_id":"epi_2"}
        ]}"#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1, "made-up-label must be filtered out");
        assert_eq!(facts[0].concept, "lesson-learned");
    }

    #[test]
    fn parse_recipe_output_errors_when_no_object() {
        let raw = "no json here at all";
        assert!(parse_recipe_output(raw).is_err());
    }

    // ── issue #2461: last-object-wins + string-aware extraction ──────────

    #[test]
    fn parse_recipe_output_prefers_last_facts_object_over_leading_banner() {
        // The t=7517 / #2461 failure mode: a leading non-facts JSON banner
        // precedes the real facts object. A first-object scan locked onto the
        // banner and the pass silently no-opped; "last object wins" recovers it.
        let raw = r#"{"recipe_name":"distill","status":"starting","note":"banner"}
            {"facts":[{"concept":"lesson-learned","content":"prefer last object","source_episode_id":"epi_3"}]}"#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "lesson-learned");
        assert_eq!(facts[0].content, "prefer last object");
    }

    #[test]
    fn parse_recipe_output_tolerates_braces_inside_fact_content() {
        // A fact whose content contains literal braces must not corrupt the
        // string-aware brace scanner.
        let raw = r#"thinking... {"facts":[{"concept":"bug-pattern","content":"handler for {req} leaks }","source_episode_id":"epi_4"}]}"#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "handler for {req} leaks }");
    }

    #[test]
    fn parse_recipe_output_handles_unmatched_quote_in_leading_prose() {
        // A stray double-quote in prose BEFORE the JSON must not put the
        // scanner into string mode and swallow the object's `{` opener.
        let raw = "He said \"the answer is below:\n{\"facts\":[{\"concept\":\"pr-pattern\",\"content\":\"x\",\"source_episode_id\":\"epi_9\"}]}";
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "pr-pattern");
    }

    #[test]
    fn balanced_object_spans_finds_multiple_and_ignores_string_braces() {
        assert_eq!(
            balanced_object_spans(r#"{"a":1} junk {"b":{"c":2}}"#).len(),
            2
        );
        assert_eq!(
            balanced_object_spans(r#"{"k":"a } b { c"}"#).len(),
            1,
            "string braces must not split the object"
        );
    }

    // ── issue #2461: failure classification ─────────────────────────────

    #[test]
    fn classify_distill_error_buckets_each_failure_mode() {
        use DistillFailureClass::*;
        let cases = [
            (
                "distill: recipe-runner-rs spawn failed: no such file",
                SpawnFailure,
            ),
            (
                "distill: recipe exited with exit status: 1: stderr= stdout=",
                CopilotTerminalFailure,
            ),
            // Production `--output-format json` parse failure (t=7517): the
            // envelope parsed, the step ran, but its output was unparseable.
            (
                "distill: `distill` step output did not contain a parseable { \"facts\": [...] } object; output: hi",
                ParseFailure,
            ),
            // Legacy bare-stdout parse failure (non-envelope `run()` / mocks).
            (
                "distill: recipe run did not yield a parseable { \"facts\": [...] } object: hi",
                ParseFailure,
            ),
            // Envelope present but the run self-reported failure / no step.
            (
                "distill: recipe-runner reported failure (success=false): boom",
                RecipeReportedFailure,
            ),
            (
                "distill: recipe envelope had no completed `distill` step",
                RecipeReportedFailure,
            ),
            (
                "distill: failed to serialize episodes payload: oops",
                SerializeFailure,
            ),
            ("something unexpected", Other),
        ];
        for (msg, expected) in cases {
            let err = SimardError::BridgeError(msg.to_string());
            assert_eq!(classify_distill_error(&err), expected, "msg: {msg}");
        }
    }

    #[test]
    fn classify_anchors_on_prefix_not_embedded_stdout() {
        // A terminal failure embeds the recipe's stdout, which may itself
        // contain the parse-failure phrase; prefix anchoring must keep it a
        // terminal failure so recipe_exited_ok stays false.
        let terminal = SimardError::BridgeError(
            "distill: recipe exited with exit status: 1: stderr= stdout=recipe run did not \
             yield a parseable object"
                .to_string(),
        );
        assert_eq!(
            classify_distill_error(&terminal),
            DistillFailureClass::CopilotTerminalFailure
        );
        assert!(!classify_distill_error(&terminal).recipe_exited_ok());
    }

    #[test]
    fn classify_non_bridge_error_is_other() {
        let err = SimardError::PlanningUnavailable {
            reason: "backend down".to_string(),
        };
        assert_eq!(classify_distill_error(&err), DistillFailureClass::Other);
    }

    // ── issue #2468: retry-aware metric context ─────────────────────────────

    /// PR-1 (#2468): the `distill_success_rate` context distinguishes
    /// first-attempt success from post-retry recovery so
    /// `distill_parse_success_rate` can tell them apart. The extended
    /// `build_distill_success_context` therefore carries `attempt` (1-based) and
    /// `recovered_after_retry`.
    #[test]
    fn distill_success_context_carries_attempt_and_recovery() {
        // First-attempt success.
        let first = build_distill_success_context(true, None, 20, 3, 1, false);
        let v: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(v["attempt"], 1);
        assert_eq!(v["recovered_after_retry"], false);
        assert_eq!(v["parse_success"], true);

        // Recovered on the second attempt after a transient miss.
        let recovered = build_distill_success_context(true, None, 20, 3, 2, true);
        let v2: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(v2["attempt"], 2);
        assert_eq!(v2["recovered_after_retry"], true);
        assert_eq!(v2["parse_success"], true);
    }

    #[test]
    fn parse_failure_is_the_only_class_that_reached_parsing() {
        use DistillFailureClass::*;
        // recipe_exited_ok: process exited 0 → parse-failure AND recipe-reported.
        assert!(ParseFailure.recipe_exited_ok());
        assert!(RecipeReportedFailure.recipe_exited_ok());
        assert!(!CopilotTerminalFailure.recipe_exited_ok());
        assert!(!SpawnFailure.recipe_exited_ok());
        assert!(!SerializeFailure.recipe_exited_ok());
        // reached_parsing: only parse-failure actually parsed step output.
        assert!(ParseFailure.reached_parsing());
        assert!(!RecipeReportedFailure.reached_parsing());
        assert!(!CopilotTerminalFailure.reached_parsing());
    }

    // ── issue #2461: distill_success_rate metric context ────────────────

    #[test]
    fn success_metric_context_marks_parse_attempted_and_parse_success() {
        let payload = build_distill_success_context(true, None, 25, 3, 1, false);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["outcome"], "success");
        assert_eq!(v["recipe_exited_ok"], true);
        assert_eq!(v["parse_attempted"], true);
        assert_eq!(v["parse_success"], true);
        assert!(v["failure_class"].is_null());
        assert_eq!(v["input_count"], 25);
        assert_eq!(v["fact_count"], 3);
    }

    #[test]
    fn parse_failure_metric_context_attempted_parsing_but_failed() {
        let payload = build_distill_success_context(
            false,
            Some(DistillFailureClass::ParseFailure),
            40,
            0,
            1,
            false,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["outcome"], "failure");
        assert_eq!(v["recipe_exited_ok"], true, "parse failure means exit 0");
        assert_eq!(v["parse_attempted"], true);
        assert_eq!(v["parse_success"], false);
        assert_eq!(v["failure_class"], "parse-failure");
    }

    #[test]
    fn terminal_failure_metric_context_did_not_reach_parsing() {
        let payload = build_distill_success_context(
            false,
            Some(DistillFailureClass::CopilotTerminalFailure),
            40,
            0,
            1,
            false,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["recipe_exited_ok"], false);
        assert_eq!(v["parse_attempted"], false);
        assert_eq!(v["parse_success"], false);
        assert_eq!(v["failure_class"], "copilot-terminal-failure");
    }

    #[test]
    fn recipe_reported_failure_exited_ok_but_did_not_reach_parsing() {
        // Envelope present (process exited 0) but the run self-reported failure
        // or had no completed step: counts as a recipe attempt that exited 0,
        // but it must NOT inflate the parse-rate denominator (no step output
        // was parsed).
        let payload = build_distill_success_context(
            false,
            Some(DistillFailureClass::RecipeReportedFailure),
            40,
            0,
            1,
            false,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["recipe_exited_ok"], true, "envelope present → exit 0");
        assert_eq!(
            v["parse_attempted"], false,
            "no step output reached parsing"
        );
        assert_eq!(v["parse_success"], false);
        assert_eq!(v["failure_class"], "recipe-reported-failure");
    }

    #[test]
    fn production_envelope_step_output_parse_failure_is_counted_in_parse_rate() {
        // Regression for the production t=7517 path: the `--output-format json`
        // step-output parse error MUST classify as parse-failure so it lands in
        // the distill_parse_success_rate denominator (parse_attempted = true).
        let err = SimardError::BridgeError(
            "distill: `distill` step output did not contain a parseable \
             { \"facts\": [...] } object; output: NODE_OPTIONS banner..."
                .to_string(),
        );
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::ParseFailure
        );
        let payload = build_distill_success_context(
            false,
            Some(classify_distill_error(&err)),
            40,
            0,
            1,
            false,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["parse_attempted"], true);
        assert_eq!(v["parse_success"], false);
        assert_eq!(v["failure_class"], "parse-failure");
    }

    #[test]
    fn scan_prefers_populated_facts_over_trailing_empty_object() {
        // A populated facts object followed by an accidental trailing empty
        // object must NOT silently discard the real facts (data-loss guard).
        let raw = concat!(
            r#"{"facts":[{"concept":"lesson-learned","content":"real","source_episode_id":"epi_1"}]}"#,
            "\n",
            r#"{"facts":[]}"#
        );
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(
            facts.len(),
            1,
            "populated object must win over empty trailer"
        );
        assert_eq!(facts[0].content, "real");
    }

    #[test]
    fn scan_accepts_bare_empty_facts_as_nothing_to_distill() {
        // A lone `{"facts":[]}` is a legitimate "nothing worth distilling"
        // answer and must parse to zero facts (not an error).
        let facts = parse_recipe_output(r#"{"facts":[]}"#).unwrap();
        assert!(facts.is_empty());
    }

    // ── t=7919 / #2484: ANSI-coded tracing-log + banner noise in step output ──

    #[test]
    fn parse_recipe_output_recovers_from_ansi_log_noise() {
        // #2484 live-evidence shape: the distill step output begins with the
        // dimmed `\x1b[2m<RFC3339 timestamp>` tracing prefix, then the agent's
        // facts object on the next line. The raw span carries `ESC` bytes so it
        // is invalid JSON; the shared `strip_recipe_noise` adoption drops the
        // ANSI-coloured log line and recovers the buried payload.
        let esc = '\u{1b}';
        let raw = format!(
            "{esc}[2m2026-06-28T08:08:58.151133Z{esc}[0m {esc}[36m INFO{esc}[0m simard::distill: run\n\
             {{\"facts\":[{{\"concept\":\"lesson-learned\",\
             \"content\":\"shared extractor recovered me\",\"source_episode_id\":\"epi_8451\"}}]}}"
        );
        // The raw, un-stripped span must NOT parse (the silent-drop failure).
        assert!(
            serde_json::from_str::<serde_json::Value>(&raw).is_err(),
            "raw ANSI/log-noised span must be unparseable JSON"
        );
        let facts = parse_recipe_output(&raw).expect("shared extractor must recover facts");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "lesson-learned");
        assert_eq!(facts[0].content, "shared extractor recovered me");
    }

    #[test]
    fn parse_recipe_output_recovers_from_runner_banner() {
        // New capability over the former distill-private ANSI-only stripper:
        // `strip_recipe_noise` ALSO drops the recipe-runner text-mode summary
        // banner lines that can prefix the agent's payload, which the old
        // ANSI-only path left in place (defeating the balanced-brace scan when
        // a banner line itself carried stray braces).
        let raw = "Recipe: distill-episodes SUCCESS (36.0s)\n\
                   Steps: 1/1 completed\n\
                   [completed] distill (36.0s)\n\
                   {\"facts\":[{\"concept\":\"bug-pattern\",\
                   \"content\":\"runner banner must be dropped\",\
                   \"source_episode_id\":\"epi_9\"}]}";
        let facts = parse_recipe_output(raw).expect("banner-prefixed payload must parse");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert_eq!(facts[0].content, "runner banner must be dropped");
    }

    #[test]
    fn parse_recipe_output_recovers_facts_from_ansi_log_noise() {
        // The t=7919 cycle: the distill step output carried ANSI-coded tracing
        // log lines (the failing excerpt began `\x1b[2m2026-06-28T06:38:25…`)
        // with the agent's facts object colourised inline. The raw ESC bytes
        // interleaved with the JSON made `serde_json` reject the span, so the
        // whole batch silently deferred. ANSI stripping recovers it.
        let esc = '\u{1b}';
        let raw = format!(
            "{esc}[2m2026-06-28T06:38:25.928376Z{esc}[0m {esc}[36m DEBUG{esc}[0m thinking…\n\
             {{\"facts\":[{{\"concept\":\"{esc}[33mbug-pattern{esc}[0m\",\
             \"content\":\"ansi noise must be stripped\",\"source_episode_id\":\"epi_7\"}}]}}"
        );
        let facts = parse_recipe_output(&raw).expect("must recover facts past ANSI noise");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert_eq!(facts[0].content, "ansi noise must be stripped");
    }

    #[test]
    fn parse_recipe_output_full_recovers_facts_from_ansi_coded_step_output() {
        // Exact production t=7919 path: the `--output-format json` envelope's
        // `distill` step `output` string is ANSI-coded tracing log noise
        // (begins with the dimmed `\x1b[2m<timestamp>` prefix from the failing
        // cycle) with the agent's facts object colourised inline. Before ANSI
        // stripping this raised "step output did not contain a parseable
        // { facts } object" and the 20-episode batch deferred for a full cycle;
        // now the buried object is mined out of the step output and parsed.
        let esc = '\u{1b}';
        let step_output = format!(
            "{esc}[2m2026-06-28T06:38:25.928376Z{esc}[0m {esc}[32m INFO{esc}[0m \
             {esc}[2msimard::distill{esc}[0m: distilling 20 episodes\n\
             {{{esc}[0m\"facts\":[{{\"concept\":\"{esc}[33mlesson-learned{esc}[0m\",\
             \"content\":\"strip ansi before the facts scan\",\
             \"source_episode_id\":\"epi_42\"}}]}}\n"
        );
        let envelope = serde_json::json!({
            "recipe_name": "distill-episodes",
            "success": true,
            "step_results": [{
                "step_id": "distill",
                "status": "completed",
                "output": step_output,
                "error": ""
            }]
        })
        .to_string();
        let out = parse_recipe_output_full(&envelope).expect("ANSI-coded step output must parse");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].concept, "lesson-learned");
        assert_eq!(out.facts[0].content, "strip ansi before the facts scan");
        assert_eq!(out.facts[0].source_episode_id, "epi_42");
    }

    #[test]
    fn parse_recipe_output_full_recovers_facts_from_copilot_launch_log_preamble() {
        // Regression for issue #2496 (episode t=9271 live failure).
        //
        // The existing ANSI/banner tests above already cover dimmed
        // `simard::distill` / `thinking` tracing prefixes and the
        // recipe-runner text summary banner. This pins the distinct
        // **Copilot CLI launch** noise shape the issue captured, which no
        // other test exercises as a unit:
        //
        //   1. an ANSI-coloured `launching copilot binary=copilot` line,
        //   2. an `\u{2139} NODE_OPTIONS=--max-old-space-size=…` line, and
        //   3. a leading, ANSI-wrapped JSON *log record*
        //      (`{"level":"info",…}`) — a complete brace-balanced object that
        //      precedes the real facts object.
        //
        // Element 3 is the adversarial part: a first-object brace scan would
        // lock onto the `{"level":…}` log record (which has no `facts` field
        // and so fails the envelope parse) and never reach the trailing facts
        // object. The current parser strips the ANSI/log noise and prefers the
        // LAST non-empty facts object, so the buried payload is recovered.
        // This is fed through the production Tier-1 `--output-format json`
        // envelope path (`parse_recipe_output_full` -> `into_distill_output`
        // -> `extract_step_output` -> `scan_for_facts_object`), which is where
        // the `\`distill\` step output did not contain a parseable …` error in
        // the issue originated.
        let esc = '\u{1b}';
        let step_output = format!(
            "{esc}[2m2026-06-29T12:26:24.512Z{esc}[0m {esc}[32m INFO{esc}[0m launching copilot binary=copilot\n\
             \u{2139} {esc}[2mNODE_OPTIONS=--max-old-space-size=8192{esc}[0m\n\
             {esc}[36m{{\"level\":\"info\",\"msg\":\"session started\",\"cwd\":\"/repo\"}}{esc}[0m\n\
             {{\"facts\":[{{\"concept\":\"bug-pattern\",\
             \"content\":\"distill parser must tolerate ANSI + launch-log preamble\",\
             \"source_episode_id\":\"epi_9271\"}}]}}\n"
        );
        // The raw step-output span carries ESC bytes and a leading non-facts
        // object, so it must NOT parse as-is — the silent-drop failure mode.
        assert!(
            serde_json::from_str::<RecipeEnvelope>(step_output.trim()).is_err(),
            "raw launch-log-noised step output must be unparseable as a facts envelope"
        );
        let envelope = serde_json::json!({
            "recipe_name": "distill-episodes",
            "success": true,
            "step_results": [{
                "step_id": "distill",
                "status": "completed",
                "output": step_output,
                "error": ""
            }]
        })
        .to_string();
        let out = parse_recipe_output_full(&envelope)
            .expect("issue #2496 launch-log-preamble payload must parse");
        assert_eq!(out.facts.len(), 1, "exactly one fact recovered");
        assert_eq!(out.facts[0].concept, "bug-pattern");
        assert_eq!(
            out.facts[0].content,
            "distill parser must tolerate ANSI + launch-log preamble"
        );
        assert_eq!(out.facts[0].source_episode_id, "epi_9271");
    }
}
