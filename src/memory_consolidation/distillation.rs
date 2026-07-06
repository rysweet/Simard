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
//!    (`pr-pattern`, `bug-pattern`, `lesson-learned`) or `skip`. Surface-form
//!    variants of those labels (case, whitespace, `_`↔`-`) are canonicalized by
//!    [`canonical_distill_concept`] so a well-formed fact is not lost to
//!    cosmetics; genuinely off-spec labels are still dropped.
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
use crate::memory_consolidation::raw_capture;

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
/// (`SpawnFailure`, `SerializeFailure`, and `Other`) are NOT retried
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

/// Outcome of a single **capturing** runner invocation
/// ([`DistillRecipeRunner::run_all_reinforced_capturing`]): the parse result
/// plus, on a failure that produced recipe stdout, the raw pre-extraction bytes
/// so the caller can harvest a real currently-failing sample (Wave 1
/// raw-capture, 2026-07-02 operator-review priority 1).
pub struct DistillAttemptOutcome {
    /// The parse result — identical to what [`run_all_reinforced`] returns.
    ///
    /// [`run_all_reinforced`]: DistillRecipeRunner::run_all_reinforced
    pub result: SimardResult<DistillOutput>,
    /// The raw recipe stdout **exactly as the extractor received it**, present
    /// only when the runner actually produced stdout AND parsing failed. `None`
    /// for stub runners with no stdout and for spawn/terminal failures (which
    /// never yielded a parseable step output to harvest).
    pub raw_on_failure: Option<String>,
    /// How this pass's facts document parsed (issue #2669, Decision D-1) — the
    /// per-pass discriminator threaded into the `distill_success_rate` metric
    /// context so a strict parse, a trailing-comma recovery, an all-filtered
    /// zero-facts success, and a hard parse-fail deferral are each visible in
    /// `metrics.jsonl` rather than collapsed into one success/fail bit.
    pub parse_recovery: ParseRecovery,
}

/// Per-pass discriminator for HOW the distill facts document parsed, recorded
/// in the `distill_success_rate` metric context (issue #2669, Decision D-1).
///
/// Orthogonal to `recovered_after_retry` (issue #2468): `recovered_after_retry`
/// tracks a whole-runner *re-invocation* across attempts, whereas
/// `ParseRecovery` describes the parse of a **single** pass's output document.
/// Both axes are independently representable in one metric event.
///
/// The four string labels ([`ParseRecovery::as_str`]) are a **frozen** public
/// vocabulary for `metrics.jsonl` readers and must never be renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseRecovery {
    /// Strict `serde_json` parse of the agent's document succeeded as-emitted.
    StrictOk,
    /// Strict parse failed, but the string-aware trailing-comma repair pass
    /// ([`crate::recipe_output::strip_json_trailing_commas`]) recovered a
    /// parseable envelope.
    Recovered,
    /// No parseable envelope in either the strict or the repair pass — the
    /// batch defers and stays retry-eligible (never a hollow `Ok`).
    Deferred,
    /// Parsed OK, but every fact was dropped by the category filter (raw facts
    /// were present, none survived): a distinct quality signal, NOT a parse
    /// failure — see the zero-facts warn in [`parse_facts_document`].
    ZeroFacts,
}

impl ParseRecovery {
    /// The frozen `metrics.jsonl` label for this discriminator.
    pub fn as_str(self) -> &'static str {
        match self {
            ParseRecovery::StrictOk => "strict-ok",
            ParseRecovery::Recovered => "recovered",
            ParseRecovery::Deferred => "deferred",
            ParseRecovery::ZeroFacts => "zero-facts",
        }
    }
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

    /// Like [`run_all_reinforced`](Self::run_all_reinforced), but on failure ALSO
    /// surfaces the raw pre-extraction recipe stdout so the caller can harvest a
    /// real currently-failing sample (Wave 1 raw-capture).
    ///
    /// The default delegates to [`run_all_reinforced`](Self::run_all_reinforced)
    /// and supplies no raw (`raw_on_failure = None`), so stub/fact-only runners
    /// keep working unchanged. Only [`RecipeRunnerSubprocess`] overrides it to
    /// thread the full stdout through — it is the only runner that HAS the bytes
    /// the diagnostic exists to capture.
    ///
    /// NOTE for future runners: `run_all_reinforced` and this method must not be
    /// left to delegate to *each other*. `RecipeRunnerSubprocess` overrides BOTH
    /// (its `run_all_reinforced` calls this method, which does the real work). A
    /// runner that overrides ONLY `run_all_reinforced` to call back into this
    /// default would recurse infinitely — override both, or neither.
    fn run_all_reinforced_capturing(
        &self,
        episodes: &[CognitiveEpisode],
        strict_json: bool,
    ) -> DistillAttemptOutcome {
        let result = self.run_all_reinforced(episodes, strict_json);
        // Stub/fact-only runners construct their output directly (no facts-file
        // parse), so a success is a strict parse by definition and a failure
        // deferred; the subprocess runner overrides this with the real tier.
        let parse_recovery = if result.is_ok() {
            ParseRecovery::StrictOk
        } else {
            ParseRecovery::Deferred
        };
        DistillAttemptOutcome {
            result,
            raw_on_failure: None,
            parse_recovery,
        }
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
    // Set on the successful attempt only (the sole `break` out of this loop);
    // all other arms `continue` or `return`, so it is always initialised before
    // use without a throwaway default.
    let parse_recovery;
    let output = loop {
        attempt += 1;
        // Reinforce the response format on any attempt past the first.
        let strict_json = attempt > 1;
        let DistillAttemptOutcome {
            result,
            raw_on_failure,
            parse_recovery: attempt_recovery,
        } = runner.run_all_reinforced_capturing(&episodes, strict_json);
        match result {
            Ok(o) => {
                parse_recovery = attempt_recovery;
                break o;
            }
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
                record_distill_success_metric(
                    false,
                    Some(class),
                    pulled,
                    0,
                    attempt,
                    false,
                    ParseRecovery::Deferred,
                );
                // Issue #2528: mirror the distill failure into the unified
                // telemetry facade alongside the human log lines above, so the
                // status snapshot's distill-fail rate is a structured signal
                // rather than a journald grep.
                crate::telemetry::counter_add(
                    crate::telemetry::names::DISTILL_RUNS,
                    1,
                    &[(crate::telemetry::names::ATTR_RESULT, "parse_fail")],
                );
                // Wave 1 (2026-07-02 operator-review priority 1): env-gated,
                // default-off raw-capture of a SURVIVING parse failure so a real
                // currently-failing sample can be harvested into a regression
                // test. `capture_parse_failure` self-gates to the toggle AND to
                // `failure_class == "parse-failure"`, so calling it on every
                // escalation is safe — a spawn/terminal/serialize failure writes
                // nothing. When the runner surfaced the raw stdout we capture it
                // verbatim ("exactly as the extractor received it"); otherwise
                // (stub runners with no stdout) we fall back to the classified
                // error string. Best-effort: a capture error never escalates.
                let cfg = raw_capture::RawCaptureConfig::from_env();
                let raw_for_capture = raw_on_failure.unwrap_or_else(|| e.to_string());
                let meta = raw_capture::CaptureMeta {
                    failure_class: class.as_str(),
                    recipe_exited_ok: class.recipe_exited_ok(),
                    attempt,
                    recovered_after_retry: false,
                    input_count: pulled,
                    fact_count: 0,
                };
                let _ = raw_capture::capture_parse_failure(&cfg, &meta, &raw_for_capture);
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

    // Issue #2528: mirror the distill outcome into the unified telemetry facade
    // (OTel counters + in-process registry) ALONGSIDE the human log line above,
    // so `simard status` reads structured signals instead of grepping journald.
    crate::telemetry::counter_add(
        crate::telemetry::names::DISTILL_RUNS,
        1,
        &[(crate::telemetry::names::ATTR_RESULT, "ok")],
    );
    crate::telemetry::counter_add(
        crate::telemetry::names::DISTILL_FACTS,
        u64::from(stored),
        &[],
    );
    crate::telemetry::counter_add(
        crate::telemetry::names::DISTILL_PROCEDURES,
        u64::from(stored_procs),
        &[],
    );
    crate::telemetry::counter_add(
        crate::telemetry::names::DISTILL_EPISODES_MARKED,
        u64::from(marked),
        &[],
    );

    // The recipe ran and its output parsed (#2461): record a success event so
    // distill_success_rate / distill_parse_success_rate are measurable. NOTE:
    // this point is only reached after the storage writes above succeeded; a
    // downstream memory-write failure propagates as `Err` (a separate subsystem,
    // out of this recipe-stage metric's scope — see record_distill_success_metric).
    // `attempt` / `recovered_after_retry` (#2468) let the metric distinguish a
    // first-attempt success from one recovered by an in-cycle retry.
    record_distill_success_metric(
        true,
        None,
        pulled,
        stored,
        attempt,
        recovered_after_retry,
        parse_recovery,
    );

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
/// leading prefix of the `SimardError::RpcError` message emitted at each
/// runner/parse site in this module. Covers every `Err` `run_all` can surface.
/// Post-#2622/#2619 the distill result is read from the agent's dedicated facts
/// file, so a parse failure now manifests as a *missing / empty / unparseable
/// facts document* rather than a stdout scan miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistillFailureClass {
    /// `recipe-runner-rs` could not be spawned (binary missing/structural), or
    /// the per-invocation facts output tempdir could not be created.
    SpawnFailure,
    /// The recipe **process** exited non-zero (#2461 t=7411 case).
    CopilotTerminalFailure,
    /// The recipe process exited 0 but the agent's facts document was missing,
    /// empty, or not a parseable `{ "facts": [...] }` object (issues
    /// #2622/#2619). This is the only failure class that actually *reached* the
    /// agent output, so it is the denominator gate for
    /// `distill_parse_success_rate`.
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
            Self::ParseFailure => "parse-failure",
            Self::SerializeFailure => "serialize-failure",
            Self::Other => "other",
        }
    }

    /// `true` when the recipe **process** exited 0. False for spawn/terminal
    /// (never started / non-zero exit) and serialize (never spawned).
    pub fn recipe_exited_ok(self) -> bool {
        matches!(self, Self::ParseFailure)
    }

    /// `true` when the run actually *reached* the agent's facts document (the
    /// process exited 0 and we tried to read + parse the file). This is the
    /// denominator gate for `distill_parse_success_rate` — only `ParseFailure`
    /// qualifies among failures.
    pub fn reached_parsing(self) -> bool {
        matches!(self, Self::ParseFailure)
    }

    /// `true` for failure classes worth a bounded in-cycle retry (issue #2468):
    /// a parse miss (exited 0, facts document missing/empty/unparseable) or a
    /// non-zero recipe exit. These are observed to recover on a second attempt —
    /// often with JSON format reinforcement. Structural classes
    /// (spawn/serialize, and `Other`) are deterministic and escalate immediately.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::ParseFailure | Self::CopilotTerminalFailure)
    }
}

/// Classify a distillation `SimardError` into a [`DistillFailureClass`].
///
/// Discrimination anchors on the **stable leading prefix** this module emits
/// for each class (`SimardError::RpcError(msg)` → `msg.starts_with(...)`),
/// NOT on `contains`. Several messages embed foreign text (terminal failures
/// carry up to 200 chars each of recipe stderr/stdout; parse failures carry the
/// facts-document excerpt), and that variable tail always trails the fixed
/// prefix — so anchoring keeps a non-zero exit from being misread as a
/// parse-failure and corrupting `distill_parse_success_rate`.
///
/// The prefixes mirror the `RpcError` sites in this file. Post-#2622/#2619 a
/// parse failure surfaces as a missing / empty / unparseable **facts document**
/// (the agent's dedicated file), all of which map to `ParseFailure`.
pub fn classify_distill_error(err: &SimardError) -> DistillFailureClass {
    let SimardError::RpcError(msg) = err else {
        return DistillFailureClass::Other;
    };
    if msg.starts_with("distill: recipe-runner-rs spawn failed")
        || msg.starts_with("distill: failed to create facts output tempdir")
    {
        DistillFailureClass::SpawnFailure
    } else if msg.starts_with("distill: recipe exited with") {
        DistillFailureClass::CopilotTerminalFailure
    } else if msg.starts_with("distill: failed to serialize episodes payload") {
        DistillFailureClass::SerializeFailure
    } else if msg.starts_with("distill: facts output file was not written")
        || msg.starts_with("distill: facts document was empty")
        || msg.starts_with("distill: facts document did not contain a parseable")
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
    parse_recovery: ParseRecovery,
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
        // Append-only per-pass parse discriminator (issue #2669, Decision D-1):
        // orthogonal to `recovered_after_retry`; every key above is unchanged.
        "parse_recovery": parse_recovery.as_str(),
    })
    .to_string()
}

/// Record the per-pass distill reliability metrics: `distill_success_rate` for
/// every pass that ran the recipe, plus a first-class `distill_parse_success_rate`
/// for the subset that reached output parsing (issue #2512).
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
/// so the mean over passes is the success rate. `distill_parse_success_rate` is
/// emitted ONLY for passes that reached parsing (`parse_attempted == true`), so
/// its plain mean is exactly the parse-success rate — isolating the "exited 0 but
/// unparseable" mode (#2461 t=7517, plus the #2512 launch-banner-prefixed
/// envelope) from the "exited non-zero" mode (t=7411), which never reached
/// parsing and emits no parse-rate event. Both share the same context payload
/// (`parse_attempted` / `parse_success` flags remain for back-compatible
/// derivation). This makes the previously-silent non-fatal distill failures
/// visible in `metrics.jsonl` without committing any point-in-time findings.
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
    parse_recovery: ParseRecovery,
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
        parse_recovery,
    );
    if let Err(e) = crate::self_metrics::record_metric("distill_success_rate", value, &context) {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_success_rate metric (distillation unaffected)",
        );
    }

    // First-class `distill_parse_success_rate` (issue #2512). Previously this
    // rate was only *derivable* by post-filtering `distill_success_rate` events
    // on `parse_attempted == true` — fragile and easy to compute wrong. Emit a
    // dedicated event ONLY for passes that actually reached output parsing
    // (`parse_attempted`), so the simple mean of `distill_parse_success_rate`
    // values is exactly the parse-success rate (successes vs. parse attempts).
    // This is the metric the launch-banner parse fix drives toward 1.0, mirroring
    // how #2504 was validated for the decide/orient brain. Recipe-reported /
    // terminal / spawn / serialize failures never reached parsing and are
    // intentionally excluded from the denominator (they emit no parse-rate event).
    let parse_attempted = success || class.is_some_and(|c| c.reached_parsing());
    if parse_attempted
        && let Err(e) =
            crate::self_metrics::record_metric("distill_parse_success_rate", value, &context)
    {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_parse_success_rate metric (distillation unaffected)",
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
const STRICT_JSON_INSTRUCTION: &str = "Your previous attempt did not leave a parseable facts file. Write ONLY the JSON object \
     {\"facts\":[...]} (and \"procedures\" if any) to the facts file whose path is given above — \
     no prose, no markdown fence, no thinking, nothing before or after the object, and do \
     not rely on printing to the terminal.";

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
        let document = self.invoke_recipe(episodes, false)?;
        let parsed = parse_facts(&document);
        crate::recipe_output::record_parse_outcome("distill", parsed.is_ok());
        parsed
    }

    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        let document = self.invoke_recipe(episodes, false)?;
        let parsed = parse_facts_document(&document);
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
        // writes ONLY the `{ "facts": [...] }` object to the facts file.
        //
        // Delegates to the capturing variant so the parse + metric logic lives in
        // exactly one place; the harvested document is simply dropped here.
        self.run_all_reinforced_capturing(episodes, strict_json)
            .result
    }

    fn run_all_reinforced_capturing(
        &self,
        episodes: &[CognitiveEpisode],
        strict_json: bool,
    ) -> DistillAttemptOutcome {
        // A spawn/terminal/missing-file failure produced no facts document, so
        // there is nothing to harvest — surface the error with no raw.
        let document = match self.invoke_recipe(episodes, strict_json) {
            Ok(document) => document,
            Err(e) => {
                return DistillAttemptOutcome {
                    result: Err(e),
                    raw_on_failure: None,
                    parse_recovery: ParseRecovery::Deferred,
                };
            }
        };
        let parsed = parse_facts_document_with_recovery(&document);
        crate::recipe_output::record_parse_outcome("distill", parsed.is_ok());
        match parsed {
            Ok((output, parse_recovery)) => DistillAttemptOutcome {
                result: Ok(output),
                raw_on_failure: None,
                parse_recovery,
            },
            // On a parse failure keep the exact facts-file bytes the parser saw so
            // Wave 1 raw-capture can persist a real currently-failing sample.
            Err(e) => DistillAttemptOutcome {
                result: Err(e),
                raw_on_failure: Some(document),
                parse_recovery: ParseRecovery::Deferred,
            },
        }
    }
}

impl RecipeRunnerSubprocess {
    /// Shell out to `recipe-runner-rs` with the episodes payload and return
    /// the recipe's raw stdout. Shared by [`run`](DistillRecipeRunner::run)
    /// and [`run_all`](DistillRecipeRunner::run_all).
    ///
    /// `strict_json` (issue #2468) threads a `strict_json_instruction` context
    /// var into the recipe: empty on the first attempt, and a "write ONLY the
    /// `{ \"facts\": [...] }` object to the facts file" reinforcement sentence on
    /// a retry after a transient parse miss. The recipe interpolates it via the
    /// pure `{{strict_json_instruction}}` substitution, so no conditional
    /// templating engine is needed.
    ///
    /// Returns the **contents of the dedicated facts file** the distill agent was
    /// instructed to write — NOT the recipe's stdout. This is the issues
    /// #2622/#2619 fix: the copilot launcher banner (`… launching copilot
    /// binary=… version="GitHub Copilot CLI …"`) and other log noise land on
    /// stdout and were previously captured AS the distill step's `output`, so a
    /// stdout scan for `{ "facts": [...] }` matched the banner and every pass
    /// failed with `parse-failure`. Routing the agent's answer through a private
    /// file (`-c facts_output_path=…`) makes that contamination structurally
    /// impossible: stdout is never read for the result.
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
            SimardError::RpcError(format!(
                "distill: failed to serialize episodes payload: {e}"
            ))
        })?;

        // Dedicated, per-invocation facts file the distill agent writes its JSON
        // envelope to (issues #2622/#2619). A fresh tempdir (mode 0700 via the
        // `tempfile` crate) gives a unique absolute path with no cross-invocation
        // races; the directory and its contents are removed when `facts_dir`
        // drops at the end of this call, AFTER we have read the file. The agent
        // is told this absolute path via `-c facts_output_path=…` and the recipe
        // prompt instructs it to write ONLY the envelope there.
        let facts_dir = tempfile::Builder::new()
            .prefix("simard-distill-")
            .tempdir()
            .map_err(|e| {
                SimardError::RpcError(format!(
                    "distill: failed to create facts output tempdir: {e}"
                ))
            })?;
        let facts_path = facts_dir.path().join("facts.json");
        let facts_path_arg = facts_path.to_string_lossy().into_owned();

        // `strict_json_instruction` (issue #2468) is always passed so the
        // recipe's `{{strict_json_instruction}}` substitution resolves: empty on
        // the first attempt, and a format-reinforcement sentence on a retry. The
        // agent binary is still selected via the proven `AMPLIHACK_AGENT_BINARY`
        // env var. `--output-format json` is retained only so a runner-level
        // failure surfaces a structured error on stdout for the message below;
        // the distill result itself is read from `facts_path`, never stdout.
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
            .arg("-c")
            .arg(format!("facts_output_path={facts_path_arg}"))
            .output()
            .map_err(|e| {
                SimardError::RpcError(format!("distill: recipe-runner-rs spawn failed: {e}"))
            })?;

        harvest_facts_file(&output, &facts_path)
    }
}

/// Post-process a finished `recipe-runner-rs` invocation into the distill
/// agent's facts document, reading it from the dedicated facts file — NEVER from
/// stdout (issues #2622/#2619).
///
/// * A non-zero exit is surfaced as an explicit terminal error carrying both the
///   truncated stderr and stdout, so a failed run is never silent.
/// * On a clean (exit-0) run the agent's answer is read from `facts_path`. A
///   missing file means the agent produced no output this attempt — an explicit,
///   retry-eligible error. There is deliberately NO stdout fallback: scraping
///   stdout is exactly the launcher-banner contamination this fix removes.
///
/// Split out of [`RecipeRunnerSubprocess::invoke_recipe`] so the "stdout noise is
/// ignored" contract is hermetically testable without spawning a subprocess.
fn harvest_facts_file(output: &std::process::Output, facts_path: &Path) -> SimardResult<String> {
    if !output.status.success() {
        // On failure the runner exits non-zero AND emits the structured error
        // inside the JSON envelope on stdout (stderr may be empty), so surface
        // both — never a silent or context-free failure.
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        return Err(SimardError::RpcError(format!(
            "distill: recipe exited with {}: stderr={} stdout={}",
            output.status,
            truncate(stderr.trim(), 200),
            truncate(stdout.trim(), 200)
        )));
    }

    std::fs::read_to_string(facts_path).map_err(|e| {
        SimardError::RpcError(format!(
            "distill: facts output file was not written by the agent ({}): {e}",
            facts_path.display()
        ))
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}

/// Parse the distill agent's facts document into a list of [`DistilledFact`].
///
/// Thin facts-only wrapper over [`parse_facts_document`] retained for the legacy
/// [`DistillRecipeRunner::run`] entry point and its unit tests.
pub(crate) fn parse_facts(document: &str) -> SimardResult<Vec<DistilledFact>> {
    parse_facts_document(document).map(|o| o.facts)
}

/// Parse the distill agent's **facts document** into a [`DistillOutput`] (facts
/// AND procedures).
///
/// `document` is the contents of the dedicated facts file the distill agent was
/// instructed to write (`-c facts_output_path=…`), NOT recipe-runner stdout.
/// Because the agent writes ONLY its JSON envelope to that private file, the
/// copilot launcher banner and log lines that contaminate stdout can never reach
/// here — this is the structural fix for the issues #2622/#2619 `parse-failure`
/// mode where the launcher banner was captured as the distill step output and a
/// stdout scan for `{ "facts": [...] }` matched the banner instead of the answer.
///
/// Parsing is deliberately simple, reflecting the clean channel:
///
/// 1. An empty document means the agent produced no output — an explicit,
///    retry-eligible `Err` (never a hollow `Ok`; no silent stdout fallback).
/// 2. Otherwise [`scan_cleaned_for_facts`] deserializes the
///    `{ "facts": [...], "procedures": [...] }` envelope, tolerating a Markdown
///    code fence or a little leading/trailing prose the agent may wrap around its
///    own answer in the file (field/format leniency on a clean channel — NOT the
///    launcher-banner stdout scraping this fix removed).
/// 3. If no facts object is present, return `Err`; the caller treats `Err` as
///    the retry-safe "no markers set" path.
pub(crate) fn parse_facts_document(document: &str) -> SimardResult<DistillOutput> {
    parse_facts_document_with_recovery(document).map(|(output, _)| output)
}

/// Like [`parse_facts_document`] but also returns the [`ParseRecovery`] tier —
/// the per-pass discriminator (issue #2669, Decision D-1) the caller threads
/// into the `distill_success_rate` metric so a strict parse, a trailing-comma
/// recovery, and an all-filtered zero-facts success are each distinguishable in
/// `metrics.jsonl` (a hard parse-fail is `Deferred`, attached by the caller on
/// the returned `Err`).
///
/// The parse is **strict first, repair second**:
///
/// 1. Empty document → explicit `Err` (never a hollow `Ok`).
/// 2. Strict [`scan_cleaned_for_facts`] over the document as-emitted → `StrictOk`.
/// 3. On a strict miss, a *string-aware*
///    [`crate::recipe_output::strip_json_trailing_commas`] recovery
///    pass repairs the single most common malformation the LLM distiller emits
///    (a trailing comma after the last array element or object member — issue
///    #2669, previously a 100% parse-fail for that shape) and re-scans →
///    `Recovered`. The repair honours string literals, so a comma inside a
///    fact's content is untouched.
/// 4. Still nothing parseable → `Err` (`Deferred`): genuinely malformed input is
///    never masked, so the batch defers and retries.
///
/// A successfully-parsed envelope whose facts were ALL dropped by the category
/// filter (raw facts present, none survived) is a SUCCESS reported as
/// `ZeroFacts` with a distinct warn — see [`finish_parse`] — so "parsed OK, 0
/// kept" is never conflated with the parse-fail signature this fix targets.
pub(crate) fn parse_facts_document_with_recovery(
    document: &str,
) -> SimardResult<(DistillOutput, ParseRecovery)> {
    let trimmed = document.trim();
    if trimmed.is_empty() {
        return Err(SimardError::RpcError(
            "distill: facts document was empty; the agent produced no output".to_string(),
        ));
    }
    // Strict pass — the document exactly as the agent emitted it.
    if let Some((output, raw_facts)) = scan_cleaned_for_facts(trimmed) {
        return Ok(finish_parse(output, raw_facts, ParseRecovery::StrictOk));
    }
    // Recovery pass — applied ONLY after the strict pass failed. Repair JSON
    // trailing commas (string-aware, shared `recipe_output` helper) and re-scan.
    // A clean document never reaches here (it parsed strictly), and genuinely
    // malformed input still yields no parseable object below, so recovery cannot
    // mask broken agent output. `Cow::Owned` ⇒ a real trailing comma was
    // dropped; a `Cow::Borrowed` (nothing repaired) can only re-fail the same
    // strict scan, so we skip the redundant re-scan.
    let repaired = crate::recipe_output::strip_json_trailing_commas(trimmed);
    if matches!(repaired, std::borrow::Cow::Owned(_))
        && let Some((output, raw_facts)) = scan_cleaned_for_facts(repaired.as_ref())
    {
        return Ok(finish_parse(output, raw_facts, ParseRecovery::Recovered));
    }
    Err(SimardError::RpcError(format!(
        "distill: facts document did not contain a parseable {{ \"facts\": [...] }} object: {}",
        truncate(trimmed, 200)
    )))
}

/// Finalize a parsed envelope: detect the "parsed OK but every fact was dropped
/// by the category filter" case (raw facts present, none survived) and surface
/// it as a distinct [`ParseRecovery::ZeroFacts`] with a warn on `simard::distill`
/// (issue #2669, Brick C), so a zero-yield success is never conflated with a
/// parse failure. A legitimately empty batch (`{"facts":[]}` — zero raw facts)
/// is a normal success and does NOT warn.
fn finish_parse(
    output: DistillOutput,
    raw_fact_count: usize,
    tier: ParseRecovery,
) -> (DistillOutput, ParseRecovery) {
    if raw_fact_count > 0 && output.facts.is_empty() {
        tracing::warn!(
            target: "simard::distill",
            input_facts = raw_fact_count,
            kept_facts = 0_usize,
            "distill: envelope parsed but all facts were dropped by the category filter \
             (pr-pattern|bug-pattern|lesson-learned); not a parse failure"
        );
        return (output, ParseRecovery::ZeroFacts);
    }
    (output, tier)
}

/// Scan an already noise-stripped `trimmed` string for a `{ "facts": [...] }`
/// object, preferring (in order) the LAST balanced object candidate that carries
/// a **grounded-capable** fact — one with a non-empty `source_episode_id` — then
/// the last otherwise-non-empty object, then the last parseable empty object.
/// Candidates are the balanced `{...}` substrings returned by
/// [`crate::recipe_output::balanced_objects`] (string-aware, and resilient to an
/// unmatched `{` in leading prose). The empty-document guard and the caller
/// contract live in [`parse_facts_document`].
///
/// The grounded-capable tier exists because field-level leniency
/// ([`de_lenient_string`]) lets a *source-less* facts object now parse as
/// "non-empty"; without this tier a trailing source-less object (which the
/// reliability gate would later quarantine wholesale) could shadow an earlier
/// fully-attributed answer and silently discard its promotable facts. Preferring
/// the grounded-capable object keeps the agent's real, attributed answer winning
/// while still recovering a source-less answer when that is all the output holds.
///
/// Returns the recovered [`DistillOutput`] **and the raw fact count** of the
/// winning envelope (facts present in the JSON *before* the category filter), so
/// the caller can distinguish a legitimately empty batch from an envelope whose
/// facts were all category-filtered away (issue #2669, Brick C zero-facts warn).
fn scan_cleaned_for_facts(trimmed: &str) -> Option<(DistillOutput, usize)> {
    // Fast path — the text IS the JSON object.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(trimmed) {
        let raw_facts = parsed.facts.len();
        return Some((parsed.into_output(), raw_facts));
    }
    // Slow path — among every balanced `{...}` substring (string-aware, so a
    // brace inside a JSON string cannot split an object), scanned from the END
    // so the agent's final answer is reached before any leading banner/thinking
    // object. The shared `recipe_output::balanced_objects` helper restarts at
    // the next `{` after an unmatched/never-closing brace, so a stray `{` in the
    // distill agent's leading prose (e.g. a code fragment like `fn f() {` while
    // it reasons about code episodes) can no longer demote the real answer to a
    // nested object and silently drop it (issue #2508). Three preference tiers
    // (best first):
    //   1. last object with a grounded-capable fact (non-empty source_episode_id),
    //   2. last otherwise-non-empty object (facts/procedures present),
    //   3. last parseable empty `{"facts":[]}` ("nothing worth distilling").
    let mut nonempty_fallback: Option<(DistillOutput, usize)> = None;
    let mut empty_fallback: Option<(DistillOutput, usize)> = None;
    for span in crate::recipe_output::balanced_objects(trimmed)
        .into_iter()
        .rev()
    {
        if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(span) {
            let raw_facts = parsed.facts.len();
            let output = parsed.into_output();
            if output
                .facts
                .iter()
                .any(|f| !f.source_episode_id.trim().is_empty())
            {
                return Some((output, raw_facts));
            }
            if !output.facts.is_empty() || !output.procedures.is_empty() {
                if nonempty_fallback.is_none() {
                    nonempty_fallback = Some((output, raw_facts));
                }
            } else if empty_fallback.is_none() {
                empty_fallback = Some((output, raw_facts));
            }
        }
    }
    nonempty_fallback.or(empty_fallback)
}

#[derive(serde::Deserialize)]
struct RecipeEnvelope {
    facts: Vec<RecipeFact>,
    #[serde(default)]
    procedures: Vec<RecipeProcedure>,
}

/// Deserialize a fact/procedure field that the distiller agent is *supposed* to
/// emit as a JSON string but, in practice, intermittently omits, nulls, or
/// emits as a bare scalar (the Copilot CLI agent does this for individual
/// `facts[]` entries — observed live in production, e.g. episode t=9664; see
/// issue #2506).
///
/// Without this, a single fact missing `source_episode_id` (or carrying a
/// `null`/numeric value) made `serde` reject the **entire** `{ "facts": [...] }`
/// envelope, so [`parse_facts_document`] found no parseable object and the whole
/// batch was silently dropped with the recurring
/// `` facts document did not contain a parseable { "facts": [...] } `` error —
/// burning an LLM call every cycle while the same batch deferred forever.
///
/// Tolerating field-level noise lets the **well-formed** facts in a batch be
/// recovered instead of one malformed sibling sinking all of them. Quality is
/// not weakened: a recovered fact with an empty/unknown `source_episode_id` is
/// ungrounded and the existing reliability gate ([`assess_fact_reliability`])
/// quarantines it; an empty `concept` is dropped by [`RecipeEnvelope::into_facts`];
/// empty `content` is quarantined by the reliability hard gate. Coercion is
/// limited to **scalars** (string / number / bool); a `null` or a non-scalar
/// (array / object) — both of which are malformed for a field the recipe
/// promises as a plain string — collapses to the empty string so the existing
/// gates drop/quarantine it rather than letting structured JSON text smuggle a
/// fact past them.
fn de_lenient_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        // Null and any non-scalar (array/object) → empty: a malformed value for
        // a promised-string field must not become non-empty `content`/`concept`
        // that could clear the empty-content/known-concept gates.
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            String::new()
        }
    })
}

#[derive(serde::Deserialize)]
struct RecipeFact {
    #[serde(default, deserialize_with = "de_lenient_string")]
    concept: String,
    #[serde(default, deserialize_with = "de_lenient_string")]
    content: String,
    #[serde(default, deserialize_with = "de_lenient_string")]
    source_episode_id: String,
}

#[derive(serde::Deserialize)]
struct RecipeProcedure {
    #[serde(default, deserialize_with = "de_lenient_string")]
    name: String,
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    source_episode_ids: Vec<String>,
}

/// Canonicalize a recipe-emitted concept label to one of the fixed
/// [`KNOWN_DISTILL_CONCEPTS`], or `None` if it is genuinely off-spec.
///
/// The distillation recipe's prompt constrains the label to the closed set
/// `{pr-pattern, bug-pattern, lesson-learned}`, but an LLM routinely varies the
/// *surface form* of a label it clearly intends: title/upper case
/// (`"PR-Pattern"`, `"BUG-PATTERN"`), surrounding whitespace or quotes/sentence
/// punctuation (`" bug-pattern "`, `"pr-pattern."`), and space/underscore
/// separators (`"pr_pattern"`, `"lesson learned"`). The legacy exact-match
/// filter silently dropped every such fact — a well-formed, grounded fact lost
/// purely to cosmetics, depressing distillation fact-yield.
///
/// Canonicalization recovers those facts (higher yield) **without weakening
/// precision**: normalization only folds case, trims surrounding
/// whitespace/quotes/sentence-punctuation, and unifies `_`/space→`-` (collapsing
/// repeated hyphens) before an EXACT match against the three labels. A concept
/// that does not normalize to one of them — `"made-up-label"`, `"skip"`,
/// `"pr-patterns"`, `"pull-request"` — still returns `None` and is dropped. The
/// three labels are lexically distinct, so no genuinely different concept can
/// alias onto another. The returned value is the canonical lower-hyphen form,
/// so the stored concept is uniform for downstream dedup/recall regardless of
/// how the model spelled it.
pub(crate) fn canonical_distill_concept(raw: &str) -> Option<&'static str> {
    // Fold case, then strip surrounding whitespace and the quote/sentence
    // punctuation a model sometimes wraps a label in.
    let trimmed = raw
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '.' | ',' | ':' | ';')
        })
        .to_ascii_lowercase();

    // Unify separators (`_` and interior spaces behave as `-`) and collapse runs
    // of hyphens, then trim any leading/trailing hyphens the folding produced.
    let mut canon = String::with_capacity(trimmed.len());
    let mut prev_hyphen = false;
    for ch in trimmed.chars() {
        let c = if ch == '_' || ch == ' ' { '-' } else { ch };
        if c == '-' {
            if !prev_hyphen {
                canon.push('-');
            }
            prev_hyphen = true;
        } else {
            canon.push(c);
            prev_hyphen = false;
        }
    }
    let canon = canon.trim_matches('-');

    match canon {
        "pr-pattern" => Some("pr-pattern"),
        "bug-pattern" => Some("bug-pattern"),
        "lesson-learned" => Some("lesson-learned"),
        _ => None,
    }
}

impl RecipeEnvelope {
    fn into_facts(self) -> Vec<DistilledFact> {
        // Keep only facts whose concept canonicalizes to one of the three
        // documented labels so the recipe cannot sneak new labels past the
        // contract — but tolerate the LLM's surface-form variation (case /
        // whitespace / `_`↔`-`) of a label it clearly intends. Off-spec
        // concepts (incl. `skip` if it ever lands here) still return `None` and
        // are dropped; recovered facts are stored under the canonical label.
        self.facts
            .into_iter()
            .filter_map(|f| {
                canonical_distill_concept(&f.concept).map(|concept| DistilledFact {
                    concept: concept.to_string(),
                    content: f.content,
                    source_episode_id: f.source_episode_id,
                })
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
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "pr-pattern");
    }

    #[test]
    fn parse_recipe_output_extracts_json_from_prose() {
        let raw = r#"Sure, here is the JSON:
            {"facts":[{"concept":"bug-pattern","content":"y","source_episode_id":"epi_2"}]}
            That's all."#;
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
    }

    #[test]
    fn parse_recipe_output_drops_unknown_concepts() {
        let raw = r#"{"facts":[
            {"concept":"made-up-label","content":"a","source_episode_id":"epi_1"},
            {"concept":"lesson-learned","content":"b","source_episode_id":"epi_2"}
        ]}"#;
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1, "made-up-label must be filtered out");
        assert_eq!(facts[0].concept, "lesson-learned");
    }

    #[test]
    fn canonical_concept_accepts_surface_form_variants() {
        // Case, surrounding whitespace/quotes/punctuation, and `_`/space
        // separators of a clearly-intended label all normalize to the canonical
        // lower-hyphen form (higher fact-yield without lowering precision).
        for (raw, want) in [
            ("pr-pattern", "pr-pattern"),
            ("PR-Pattern", "pr-pattern"),
            ("BUG-PATTERN", "bug-pattern"),
            (" bug-pattern ", "bug-pattern"),
            ("Lesson-Learned", "lesson-learned"),
            ("pr_pattern", "pr-pattern"),
            ("lesson learned", "lesson-learned"),
            ("pr--pattern", "pr-pattern"),
            ("\"pr-pattern\"", "pr-pattern"),
            ("bug-pattern.", "bug-pattern"),
        ] {
            assert_eq!(
                canonical_distill_concept(raw),
                Some(want),
                "surface variant {raw:?} should canonicalize to {want:?}"
            );
        }
    }

    #[test]
    fn canonical_concept_rejects_offspec_labels() {
        // Precision guard: anything that does not normalize to exactly one of the
        // three labels is still dropped — the canonicalizer must not admit
        // near-misses, plurals, or unrelated labels.
        for raw in [
            "made-up-label",
            "skip",
            "observation",
            "pr-patterns",
            "pull-request",
            "bug",
            "pattern",
            "",
            "   ",
            "pr-pattern-v2",
            "prpattern",
        ] {
            assert_eq!(
                canonical_distill_concept(raw),
                None,
                "off-spec label {raw:?} must be dropped"
            );
        }
    }

    #[test]
    fn into_facts_recovers_surface_variant_but_drops_offspec() {
        // End-to-end through the production parser: a case/whitespace/underscore
        // variant is recovered (and stored canonical); an off-spec label is
        // dropped.
        let raw = r#"{"facts":[
            {"concept":"PR-Pattern","content":"squash fixups","source_episode_id":"epi_1"},
            {"concept":" bug_pattern ","content":"off by one","source_episode_id":"epi_2"},
            {"concept":"made-up-label","content":"nope","source_episode_id":"epi_3"}
        ]}"#;
        let facts = parse_facts(raw).unwrap();
        assert_eq!(
            facts.len(),
            2,
            "two surface variants recovered, off-spec dropped"
        );
        assert_eq!(facts[0].concept, "pr-pattern");
        assert_eq!(facts[1].concept, "bug-pattern");
    }

    #[test]
    fn parse_recipe_output_errors_when_no_object() {
        let raw = "no json here at all";
        assert!(parse_facts(raw).is_err());
    }

    // ── issue #2461: last-object-wins + string-aware extraction ──────────

    #[test]
    fn parse_recipe_output_prefers_last_facts_object_over_leading_banner() {
        // The t=7517 / #2461 failure mode: a leading non-facts JSON banner
        // precedes the real facts object. A first-object scan locked onto the
        // banner and the pass silently no-opped; "last object wins" recovers it.
        let raw = r#"{"recipe_name":"distill","status":"starting","note":"banner"}
            {"facts":[{"concept":"lesson-learned","content":"prefer last object","source_episode_id":"epi_3"}]}"#;
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "lesson-learned");
        assert_eq!(facts[0].content, "prefer last object");
    }

    #[test]
    fn parse_recipe_output_tolerates_braces_inside_fact_content() {
        // A fact whose content contains literal braces must not corrupt the
        // string-aware brace scanner.
        let raw = r#"thinking... {"facts":[{"concept":"bug-pattern","content":"handler for {req} leaks }","source_episode_id":"epi_4"}]}"#;
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "handler for {req} leaks }");
    }

    #[test]
    fn parse_recipe_output_handles_unmatched_quote_in_leading_prose() {
        // A stray double-quote in prose BEFORE the JSON must not put the
        // scanner into string mode and swallow the object's `{` opener.
        let raw = "He said \"the answer is below:\n{\"facts\":[{\"concept\":\"pr-pattern\",\"content\":\"x\",\"source_episode_id\":\"epi_9\"}]}";
        let facts = parse_facts(raw).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "pr-pattern");
    }

    // ── issue #2508: unmatched `{` in leading prose must not swallow the answer ──
    // The balanced-object scan reuses the shared `recipe_output::balanced_objects`
    // helper (string-aware, restarts past an unmatched/never-closing `{`); that
    // helper's own span-level unit tests live in `recipe_output::extract`,
    // including `balanced_objects_skips_unmatched_leading_brace`. The end-to-end
    // distillation recovery and the non-fatal fallback are pinned here.

    #[test]
    fn parse_recipe_output_recovers_from_unbalanced_brace_in_leading_prose() {
        // The distill agent reasons about CODE episodes, so its preamble can
        // carry a code fragment with an unmatched `{` (e.g. `fn handler() {`)
        // before it emits the JSON answer. The old single-`depth` scan anchored
        // on that stray brace and silently dropped the real facts object,
        // no-opping distillation. The scanner must recover the trailing answer.
        let raw = concat!(
            "Looking at the episodes, the leak is in `fn handler() {` where the guard is missing.\n",
            "Here is the distilled output:\n",
            r#"{"facts":[{"concept":"bug-pattern","content":"missing guard in handler","#,
            r#""source_episode_id":"epi_9900"}]}"#,
        );
        let facts = parse_facts(raw).expect("answer after an unmatched `{` must parse");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert_eq!(facts[0].content, "missing guard in handler");
        assert_eq!(facts[0].source_episode_id, "epi_9900");
    }

    #[test]
    fn parse_recipe_output_banner_only_fails_non_fatally_without_panic() {
        // Genuinely answer-less output — only the Copilot CLI launch banner,
        // no `{ "facts": [...] }` object anywhere — must return Err (the
        // retry-safe "no markers set" path) without panicking. This is the
        // t=9900 shape when the agent emits its launch preamble and no answer.
        let raw = concat!(
            "2026-06-30T09:00:00.000000Z  INFO launching copilot binary=copilot ",
            "version=\"GitHub Copilot CLI 1.0.66-2\"\n",
            "\u{2139} NODE_OPTIONS=--max-old-space-size=8192 (saved preference)\n",
            "Run 'copilot update' to update\n",
        );
        assert!(parse_facts(raw).is_err());
        // And the empty / whitespace-only shapes likewise fail without panic.
        assert!(parse_facts("").is_err());
        assert!(parse_facts("   \n \t ").is_err());
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
                "distill: failed to create facts output tempdir: permission denied",
                SpawnFailure,
            ),
            (
                "distill: recipe exited with exit status: 1: stderr= stdout=",
                CopilotTerminalFailure,
            ),
            // Post-#2622/#2619 parse failures: the process exited 0 but the
            // agent's facts document was missing, empty, or unparseable.
            (
                "distill: facts output file was not written by the agent (/tmp/x/facts.json): No such file or directory (os error 2)",
                ParseFailure,
            ),
            (
                "distill: facts document was empty; the agent produced no output",
                ParseFailure,
            ),
            (
                "distill: facts document did not contain a parseable { \"facts\": [...] } object: hi",
                ParseFailure,
            ),
            (
                "distill: failed to serialize episodes payload: oops",
                SerializeFailure,
            ),
            ("something unexpected", Other),
        ];
        for (msg, expected) in cases {
            let err = SimardError::RpcError(msg.to_string());
            assert_eq!(classify_distill_error(&err), expected, "msg: {msg}");
        }
    }

    #[test]
    fn classify_anchors_on_prefix_not_embedded_stdout() {
        // A terminal failure embeds the recipe's stdout, which may itself
        // contain the parse-failure phrase; prefix anchoring must keep it a
        // terminal failure so recipe_exited_ok stays false.
        let terminal = SimardError::RpcError(
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
        let first =
            build_distill_success_context(true, None, 20, 3, 1, false, ParseRecovery::StrictOk);
        let v: serde_json::Value = serde_json::from_str(&first).unwrap();
        assert_eq!(v["attempt"], 1);
        assert_eq!(v["recovered_after_retry"], false);
        assert_eq!(v["parse_success"], true);

        // Recovered on the second attempt after a transient miss.
        let recovered =
            build_distill_success_context(true, None, 20, 3, 2, true, ParseRecovery::StrictOk);
        let v2: serde_json::Value = serde_json::from_str(&recovered).unwrap();
        assert_eq!(v2["attempt"], 2);
        assert_eq!(v2["recovered_after_retry"], true);
        assert_eq!(v2["parse_success"], true);
    }

    #[test]
    fn parse_failure_is_the_only_class_that_reached_parsing() {
        use DistillFailureClass::*;
        // recipe_exited_ok: process exited 0 → only parse-failure.
        assert!(ParseFailure.recipe_exited_ok());
        assert!(!CopilotTerminalFailure.recipe_exited_ok());
        assert!(!SpawnFailure.recipe_exited_ok());
        assert!(!SerializeFailure.recipe_exited_ok());
        // reached_parsing: only parse-failure actually reached the agent output.
        assert!(ParseFailure.reached_parsing());
        assert!(!CopilotTerminalFailure.reached_parsing());
    }

    // ── issue #2461: distill_success_rate metric context ────────────────

    #[test]
    fn success_metric_context_marks_parse_attempted_and_parse_success() {
        let payload =
            build_distill_success_context(true, None, 25, 3, 1, false, ParseRecovery::StrictOk);
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
            ParseRecovery::Deferred,
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
            ParseRecovery::Deferred,
        );
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["recipe_exited_ok"], false);
        assert_eq!(v["parse_attempted"], false);
        assert_eq!(v["parse_success"], false);
        assert_eq!(v["failure_class"], "copilot-terminal-failure");
    }

    #[test]
    fn missing_facts_document_parse_failure_is_counted_in_parse_rate() {
        // Regression for the issues #2622/#2619 path: a missing/empty/unparseable
        // facts document (process exited 0) MUST classify as parse-failure so it
        // lands in the distill_parse_success_rate denominator (parse_attempted =
        // true).
        let err = SimardError::RpcError(
            "distill: facts document did not contain a parseable \
             { \"facts\": [...] } object: launcher banner..."
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
            ParseRecovery::Deferred,
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
        let facts = parse_facts(raw).unwrap();
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
        let facts = parse_facts(r#"{"facts":[]}"#).unwrap();
        assert!(facts.is_empty());
    }
}

/// Episode t=9664 (live OODA consolidation): the distill agent intermittently
/// omits / nulls / scalar-types a single `facts[]` field (most often
/// `source_episode_id`), which made `serde` reject the **whole** envelope so the
/// entire batch was silently dropped and re-deferred every cycle. These tests
/// pin the field-tolerant deserialization ([`de_lenient_string`]) as it applies
/// to the agent's **facts document** (issues #2622/#2619): one malformed fact no
/// longer sinks its well-formed siblings, and an ungrounded recovered fact is
/// still gated by [`assess_fact_reliability`] rather than promoted blindly.
#[cfg(test)]
mod issue_t9664_field_tolerance_tests {
    use super::*;

    /// The headline regression: a facts document in which one fact is
    /// **well-formed** and a sibling **omits `source_episode_id`**. Before the
    /// fix the missing field made the whole `{ "facts": [...] }` object
    /// unparseable and the entire batch was dropped; now the document parses and
    /// the well-formed fact is recovered.
    #[test]
    fn distill_recovers_wellformed_fact_when_sibling_omits_source_episode_id() {
        let document = "{\"facts\":[\
               {\"concept\":\"bug-pattern\",\"content\":\"distill parser must tolerate a fact missing source_episode_id\",\"source_episode_id\":\"epi_9664\"},\
               {\"concept\":\"lesson-learned\",\"content\":\"a sibling fact omitted its source_episode_id\"}\
             ]}";
        let out = parse_facts_document(document)
            .expect("t=9664 document (one field-incomplete fact) must parse");
        // Both facts are recovered by the parser (the reliability gate, applied
        // later in the pass, decides promotion vs. quarantine — see
        // `assess_fact_reliability`). The grounded one is intact.
        assert!(
            out.facts.iter().any(|f| f.concept == "bug-pattern"
                && f.source_episode_id == "epi_9664"
                && f.content == "distill parser must tolerate a fact missing source_episode_id"),
            "the well-formed grounded fact must survive a malformed sibling: {:?}",
            out.facts
        );
        // The field-incomplete sibling parses with an empty source_episode_id
        // (ungrounded) rather than poisoning the batch.
        assert!(
            out.facts
                .iter()
                .any(|f| f.concept == "lesson-learned" && f.source_episode_id.is_empty()),
            "the field-incomplete sibling must parse with an empty source_episode_id: {:?}",
            out.facts
        );
    }

    /// A bare facts object whose only fact omits `source_episode_id` must parse
    /// instead of erroring — the minimal reproduction of the schema-strictness
    /// gap.
    #[test]
    fn parse_recipe_output_tolerates_fact_missing_source_episode_id() {
        let raw = r#"{"facts":[{"concept":"bug-pattern","content":"missing id field"}]}"#;
        let facts = parse_facts(raw).expect("a fact missing source_episode_id must parse");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert!(facts[0].source_episode_id.is_empty());
    }

    /// `null` and bare-scalar field values are coerced rather than rejected:
    /// `source_episode_id: null` and a numeric `source_episode_id` are common
    /// LLM deviations that previously sank the whole envelope.
    #[test]
    fn parse_recipe_output_tolerates_null_and_scalar_fields() {
        let null_field = r#"{"facts":[{"concept":"lesson-learned","content":"id was null","source_episode_id":null}]}"#;
        let facts = parse_facts(null_field).expect("null source_episode_id must parse");
        assert_eq!(facts.len(), 1);
        assert!(facts[0].source_episode_id.is_empty());

        let numeric_field = r#"{"facts":[{"concept":"pr-pattern","content":"id was numeric","source_episode_id":9664}]}"#;
        let facts = parse_facts(numeric_field).expect("numeric source_episode_id must parse");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].source_episode_id, "9664");
    }

    /// Field tolerance must NOT weaken quality: a recovered fact whose
    /// `source_episode_id` does not match any batch episode is ungrounded and
    /// the reliability gate keeps quarantining it (score < threshold).
    #[test]
    fn recovered_fact_with_missing_provenance_is_still_quarantined_by_the_gate() {
        let ungrounded = DistilledFact {
            concept: "bug-pattern".to_string(),
            content: "three or more words present here".to_string(),
            source_episode_id: String::new(),
        };
        let score = assess_fact_reliability(&ungrounded, &[], std::slice::from_ref(&ungrounded));
        assert!(
            score < DISTILL_RELIABILITY_THRESHOLD,
            "ungrounded recovered fact must stay below the promotion threshold, got {score}"
        );
    }

    /// Field leniency must not let a trailing *source-less* facts object shadow
    /// an earlier *grounded* one. Before the grounded-preference tier, the
    /// source-less object — now parseable thanks to `de_lenient_string` — would
    /// win as the "last non-empty" object and the earlier attributed fact would
    /// be silently discarded. The grounded-capable answer must win.
    #[test]
    fn grounded_object_wins_over_trailing_sourceless_object() {
        let step_output = concat!(
            "{\"facts\":[{\"concept\":\"bug-pattern\",\"content\":\"the attributed answer\",\"source_episode_id\":\"epi_1\"}]}\n",
            "{\"facts\":[{\"concept\":\"bug-pattern\",\"content\":\"a later source-less restatement\"}]}"
        );
        let facts = parse_facts(step_output).expect("a grounded object must be recoverable");
        assert_eq!(
            facts.len(),
            1,
            "exactly the grounded object's facts: {facts:?}"
        );
        assert_eq!(facts[0].source_episode_id, "epi_1");
        assert_eq!(facts[0].content, "the attributed answer");
    }

    /// A non-scalar (array/object) value for a promised-string field collapses to
    /// empty rather than being stringified, so it cannot smuggle structured JSON
    /// text past the empty-content / known-concept gates. The envelope still
    /// parses (the batch is not dropped); the malformed fact is simply gateable.
    #[test]
    fn non_scalar_field_values_collapse_to_empty() {
        let raw = r#"{"facts":[{"concept":"bug-pattern","content":["a","b"],"source_episode_id":{"x":1}}]}"#;
        let facts = parse_facts(raw).expect("non-scalar fields must not sink the envelope");
        assert_eq!(facts.len(), 1);
        assert!(
            facts[0].content.is_empty(),
            "array content must collapse to empty, got {:?}",
            facts[0].content
        );
        assert!(
            facts[0].source_episode_id.is_empty(),
            "object source_episode_id must collapse to empty, got {:?}",
            facts[0].source_episode_id
        );
        // Empty content → reliability hard gate quarantines it (score 0.0).
        let score = assess_fact_reliability(&facts[0], &[], &facts);
        assert_eq!(score, 0.0, "empty-content fact must score 0.0");
    }
}

/// Issues #2622/#2619 — the distill result is read from a dedicated **facts
/// file** the agent writes, never from recipe-runner stdout. These tests pin
/// the two structural guarantees of that fix:
///
/// 1. The copilot launcher banner / log lines on stdout can NEVER contaminate
///    the parse — a valid facts file yields facts even when stdout is pure
///    launcher noise (the exact live failure shape from daemon logs 02:45–02:47).
/// 2. There is NO silent stdout fallback — a missing facts file is an explicit,
///    retry-eligible parse failure even when stdout happens to carry a
///    well-formed `{ "facts": [...] }` object.
#[cfg(test)]
mod issue_2622_file_channel_tests {
    use super::*;

    /// The launcher banner captured live (daemon logs 02:45–02:47, Copilot CLI
    /// 1.0.69-1) that was previously captured AS the distill step output and
    /// scraped for facts — matching the banner, never the answer.
    const LIVE_LAUNCHER_BANNER: &str = "2026-07-06T02:45:12.101010Z  INFO launching copilot \
         binary=/home/azureuser/.npm-global/bin/copilot version=\"GitHub Copilot CLI 1.0.69-1.\"\n\
         \u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference)\n\
         Run 'copilot update' to update\n";

    #[cfg(unix)]
    fn output_with(stdout: &[u8], code: i32) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    /// The headline regression: the runner's stdout is nothing but the copilot
    /// launcher banner, yet the agent wrote a clean facts envelope to the
    /// dedicated file — the pass MUST succeed and yield the facts (no
    /// parse-failure). This is the exact contamination shape from issues
    /// #2622/#2619.
    #[cfg(unix)]
    #[test]
    fn launcher_banner_on_stdout_does_not_cause_parse_failure() {
        let dir = tempfile::tempdir().unwrap();
        let facts_path = dir.path().join("facts.json");
        std::fs::write(
            &facts_path,
            r#"{"facts":[{"concept":"pr-pattern","content":"warm the shared cache before lbug pin bumps","source_episode_id":"epi_1"}],"procedures":[]}"#,
        )
        .unwrap();

        let output = output_with(LIVE_LAUNCHER_BANNER.as_bytes(), 0);
        let document = harvest_facts_file(&output, &facts_path)
            .expect("a launcher banner on stdout must NOT block reading the facts file");
        let out = parse_facts_document(&document).expect("the clean facts file must parse");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].concept, "pr-pattern");
        assert_eq!(out.facts[0].source_episode_id, "epi_1");
    }

    /// No silent fallback: a missing facts file is an explicit `ParseFailure`
    /// even when stdout carries a perfectly well-formed facts object — proving
    /// stdout is never scraped as a fallback result channel.
    #[cfg(unix)]
    #[test]
    fn missing_facts_file_is_parse_failure_never_stdout_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let facts_path = dir.path().join("facts.json"); // deliberately never written

        let tempting_stdout =
            br#"{"facts":[{"concept":"pr-pattern","content":"must be ignored","source_episode_id":"epi_9"}]}"#;
        let output = output_with(tempting_stdout, 0);
        let err = harvest_facts_file(&output, &facts_path)
            .expect_err("a missing facts file must be an explicit error, never a stdout fallback");
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::ParseFailure
        );
    }

    /// A non-zero recipe exit is surfaced as an explicit terminal failure that
    /// carries the stdout/stderr context — never a silent success.
    #[cfg(unix)]
    #[test]
    fn nonzero_exit_is_terminal_failure_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let facts_path = dir.path().join("facts.json");
        let output = output_with(b"boom on stdout", 3);
        let err = harvest_facts_file(&output, &facts_path)
            .expect_err("a non-zero exit must surface an explicit error");
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::CopilotTerminalFailure
        );
    }

    /// A clean, exit-0 run whose facts file carries a valid envelope is read
    /// verbatim from the file.
    #[cfg(unix)]
    #[test]
    fn clean_run_reads_facts_verbatim_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let facts_path = dir.path().join("facts.json");
        std::fs::write(&facts_path, r#"{"facts":[],"procedures":[]}"#).unwrap();
        let output = output_with(b"", 0);
        let document = harvest_facts_file(&output, &facts_path).expect("clean run must read file");
        let out = parse_facts_document(&document).expect("an empty envelope is a valid success");
        assert!(out.facts.is_empty() && out.procedures.is_empty());
    }

    /// An empty facts document (the agent created the file but wrote nothing) is
    /// an explicit parse failure — never a hollow `Ok`.
    #[test]
    fn empty_facts_document_is_parse_failure() {
        let err = parse_facts_document("   \n\t ")
            .expect_err("an empty facts document must be an explicit error");
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::ParseFailure
        );
    }

    /// A launcher banner written INTO the file (no JSON object at all) still
    /// fails explicitly — `parse_facts_document` does not scrape a banner for a
    /// `{ "facts": [...] }` object.
    #[test]
    fn banner_only_document_is_parse_failure() {
        let err = parse_facts_document(LIVE_LAUNCHER_BANNER)
            .expect_err("a banner-only document has no facts object and must error");
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::ParseFailure
        );
    }

    /// The agent may wrap its answer in a Markdown code fence inside the file;
    /// the facts must still be recovered (clean-channel format leniency).
    #[test]
    fn fenced_facts_document_still_parses() {
        let fenced = "```json\n{\"facts\":[{\"concept\":\"bug-pattern\",\"content\":\"off-by-one in the ring buffer\",\"source_episode_id\":\"epi_7\"}],\"procedures\":[]}\n```";
        let out = parse_facts_document(fenced).expect("a fenced facts document must still parse");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].source_episode_id, "epi_7");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Issue #2669: distill parse-fail recovery + observability — TDD (RED)
// ═══════════════════════════════════════════════════════════════════════════
//
// These tests pin the in-repo P0 contract (Bricks B/C/D) and are written BEFORE
// the implementation. They FAIL TO COMPILE (missing `ParseRecovery`, new
// `build_distill_success_context` arg) and/or FAIL at runtime (no recovery, no
// zero-facts warn) until the fix lands — the intended TDD red signal.
//
//   Brick B — scan_cleaned_for_facts strict-then-repair recovery tier
//   Brick C — distinct "parsed OK but all facts category-filtered" warn
//   Brick D — ParseRecovery per-pass discriminator in the metrics context
#[cfg(test)]
mod issue_2669_parse_recovery_tests {
    use super::*;
    use std::io;
    use std::sync::{Arc, Mutex};

    // ── Brick B: strict-then-repair recovery via the production parser ──────

    /// C1 (headline): a bare `{ "facts": [...] }` document with a trailing comma
    /// after the last array element — previously 100% parse-fail — now recovers
    /// at least one fact instead of deferring forever.
    #[test]
    fn bare_trailing_comma_recovers_fact() {
        let doc = r#"{"facts":[{"concept":"pr-pattern","content":"x","source_episode_id":"e1"},]}"#;
        let out = parse_facts_document(doc)
            .expect("a trailing-comma facts document must recover (Brick B)");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].concept, "pr-pattern");
        assert_eq!(out.facts[0].source_episode_id, "e1");
    }

    /// Trailing commas on BOTH the facts array and the envelope object are
    /// recovered together.
    #[test]
    fn enveloped_trailing_commas_recover() {
        let doc = r#"{"facts":[{"concept":"lesson-learned","content":"y","source_episode_id":"e2"},],"procedures":[],}"#;
        let out = parse_facts_document(doc).expect("enveloped trailing commas must recover");
        assert_eq!(out.facts.len(), 1);
        assert!(out.procedures.is_empty());
    }

    /// S2 / R1 (string-safety end-to-end): a fact whose CONTENT contains `,}`
    /// plus a genuine trailing comma. Recovery must remove only the real trailing
    /// comma and preserve the string content verbatim.
    #[test]
    fn recovery_preserves_comma_inside_fact_content() {
        let doc = r#"{"facts":[{"concept":"bug-pattern","content":"regex ,} bug","source_episode_id":"e3"},]}"#;
        let out = parse_facts_document(doc).expect("must recover without corrupting content");
        assert_eq!(out.facts.len(), 1);
        assert_eq!(
            out.facts[0].content, "regex ,} bug",
            "string interior must survive the repair byte-for-byte"
        );
    }

    /// S1 / R2 (never a hollow Ok): genuinely malformed input (not merely a
    /// trailing comma) must still be an explicit `Err`, so the batch defers and
    /// retries — recovery must not mask broken agent output.
    #[test]
    fn genuinely_malformed_still_errs() {
        for bad in [
            r#"{"facts":[{"concept":"pr-pattern","content":"x""#, // truncated
            r#"{"facts":"#,                                       // truncated
            r#"{"facts":[}"#,                                     // structurally broken
            "not json at all",
        ] {
            assert!(
                parse_facts_document(bad).is_err(),
                "malformed (non-trailing-comma) input must stay Err: {bad:?}"
            );
        }
    }

    /// The recovery tier must not disturb the clean path: a well-formed document
    /// still parses to exactly its facts.
    #[test]
    fn clean_document_unaffected_by_recovery_tier() {
        let doc = r#"{"facts":[{"concept":"pr-pattern","content":"x","source_episode_id":"e1"}]}"#;
        let out = parse_facts_document(doc).expect("clean document must still parse");
        assert_eq!(out.facts.len(), 1);
    }

    // ── Brick D: ParseRecovery discriminator + metrics context schema ──────

    /// The four frozen labels are a public vocabulary for `metrics.jsonl`
    /// readers and must never be renamed.
    #[test]
    fn parse_recovery_labels_are_frozen() {
        assert_eq!(ParseRecovery::StrictOk.as_str(), "strict-ok");
        assert_eq!(ParseRecovery::Recovered.as_str(), "recovered");
        assert_eq!(ParseRecovery::Deferred.as_str(), "deferred");
        assert_eq!(ParseRecovery::ZeroFacts.as_str(), "zero-facts");
    }

    /// The `distill_success_rate` context gains an append-only `parse_recovery`
    /// key (Decision D-1); every existing key is unchanged.
    #[test]
    fn context_carries_parse_recovery_key_appendonly() {
        let ctx = build_distill_success_context(
            true,
            None,
            10,
            3,
            1,
            false,
            ParseRecovery::Recovered, // NEW appended arg
        );
        let v: serde_json::Value = serde_json::from_str(&ctx).unwrap();
        assert_eq!(
            v["parse_recovery"], "recovered",
            "new discriminator present"
        );
        // Back-compat: existing keys untouched.
        assert_eq!(v["outcome"], "success");
        assert_eq!(v["parse_success"], true);
        assert_eq!(v["input_count"], 10);
        assert_eq!(v["fact_count"], 3);
        assert_eq!(v["attempt"], 1);
        assert_eq!(v["recovered_after_retry"], false);
    }

    /// `parse_recovery` is orthogonal to `recovered_after_retry`: both axes are
    /// independently representable in one event.
    #[test]
    fn parse_recovery_is_orthogonal_to_retry() {
        let ctx = build_distill_success_context(
            true,
            None,
            5,
            1,
            2,
            true,                    // recovered via whole-runner re-invocation (#2468)
            ParseRecovery::StrictOk, // ...but the document parsed strictly this pass
        );
        let v: serde_json::Value = serde_json::from_str(&ctx).unwrap();
        assert_eq!(v["recovered_after_retry"], true);
        assert_eq!(v["parse_recovery"], "strict-ok");
    }

    // ── Brick C: distinct zero-facts-after-filter warning ──────────────────

    /// A minimal in-memory `tracing` subscriber so a test can assert on emitted
    /// warnings without any extra dependency.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);
    impl io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_logs(body: impl FnOnce()) -> String {
        let sink = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, body);
        String::from_utf8(sink.0.lock().unwrap().clone()).unwrap()
    }

    /// Brick C: an envelope that parsed successfully but had EVERY fact dropped
    /// by the category filter emits a distinct warning on `simard::distill`, so
    /// "parsed OK, 0 facts survived" is never conflated with a parse failure.
    #[test]
    fn all_facts_filtered_emits_distinct_zero_facts_warn() {
        let logs = capture_logs(|| {
            // A syntactically valid envelope whose only fact carries an off-spec
            // concept ⇒ dropped by `canonical_distill_concept` ⇒ kept == 0.
            let out = parse_facts_document(
                r#"{"facts":[{"concept":"made-up-label","content":"c","source_episode_id":"e1"}]}"#,
            )
            .expect("a parseable envelope is a SUCCESS even when all facts are filtered");
            assert!(
                out.facts.is_empty(),
                "all facts must be category-filtered away"
            );
        });
        assert!(
            logs.contains("simard::distill"),
            "warn must target simard::distill; got: {logs:?}"
        );
        assert!(
            logs.contains("kept_facts") && logs.contains("category filter"),
            "warn must distinctly flag zero-kept-after-filter; got: {logs:?}"
        );
    }

    /// R4: a legitimately empty `{"facts":[]}` envelope (nothing worth
    /// distilling) must NOT emit the zero-facts warn — it is a normal success.
    #[test]
    fn empty_envelope_does_not_warn() {
        let logs = capture_logs(|| {
            let out = parse_facts_document(r#"{"facts":[],"procedures":[]}"#)
                .expect("empty envelope is a valid success");
            assert!(out.facts.is_empty());
        });
        assert!(
            !logs.contains("category filter"),
            "an empty batch must not fire the zero-facts warn; got: {logs:?}"
        );
    }
}
