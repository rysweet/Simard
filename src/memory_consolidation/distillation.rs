//! Episode distillation: extract semantic facts from batches of recent
//! episodes via an LLM recipe, with the distiller now a **true agentic step**
//! whose writes ARE its output (issue #2679).
//!
//! See `docs/architecture/distillation-semantic-handoff.md` for the full design.
//! This module:
//!
//! 1. Pulls up to [`DISTILL_BATCH_SIZE`] undistilled episodes from the
//!    cognitive-memory backend, newest first.
//! 2. If fewer than [`DISTILL_MIN_EPISODES`] are present, skips the pass
//!    entirely — no LLM call, no markers.
//! 3. Otherwise invokes a pluggable [`DistillRecipeRunner`] as an **agentic**
//!    step ([`DistillRecipeRunner::run_agentic`]). The distiller commits each
//!    derived fact DIRECTLY through the cognitive-memory write boundary — for the
//!    real subprocess runner that is the agent calling `simard memory remember`
//!    per fact; for the deterministic test stubs it is the in-process
//!    [`DistillFactSink`] forwarding the stub's returned facts. **There is no
//!    `{ "facts": [...] }` document scraped back out of recipe stdout and
//!    hand-deserialized** — the trailing-comma / noisy-banner parse-failure mode
//!    of #2658/#2679 is therefore structurally impossible: there is nothing left
//!    to parse.
//! 4. The write boundary self-assesses each candidate fact's reliability (issue
//!    #2433, BGML's ISAO) via the shared [`crate::fact_reliability`] scorer and
//!    GATES on it: low-reliability facts are quarantined (not promoted) and a
//!    weaker new fact never clobbers a stronger existing fact of the same
//!    identity. Surviving facts are stored via `store_fact_with_provenance` with
//!    the *computed* confidence.
//! 5. Marks EVERY input episode (including those classified `skip`) as distilled
//!    so the same low-value batch is not re-fed to the LLM.
//!
//! On recipe error (a non-zero recipe exit — the ONLY failure the result path
//! can report now): NO markers are set; the batch is fully eligible for retry on
//! the next pass.

use std::path::Path;
use std::process::Command;

use crate::cognitive_memory::CognitiveMemoryOps;
use crate::error::{SimardError, SimardResult};
use crate::memory_cognitive::CognitiveEpisode;

/// Maximum number of episodes pulled per distillation pass.
pub const DISTILL_BATCH_SIZE: u32 = 50;

/// Minimum number of undistilled episodes that must be present for a pass to
/// fire. Below this, the pass is a no-op (no LLM call, no markers).
pub const DISTILL_MIN_EPISODES: u32 = 20;

/// Legacy per-fact confidence baseline. Retained as the **nominal baseline**: a
/// fully-grounded, known-concept, well-formed fact scores at or above this value
/// under the shared reliability scorer.
pub const DISTILL_FACT_CONFIDENCE: f64 = 0.7;

/// ISAO reliability gate threshold (issue #2433). Re-exported from the shared
/// [`crate::fact_reliability`] module — the single source of truth homing the
/// gate at the write boundary (issue #2679).
pub const DISTILL_RELIABILITY_THRESHOLD: f64 = crate::fact_reliability::RELIABILITY_THRESHOLD;

/// The closed concept-label set the distillation recipe is constrained to.
/// Re-exported from [`crate::fact_reliability`].
pub const KNOWN_DISTILL_CONCEPTS: &[&str] = crate::fact_reliability::KNOWN_CONCEPTS;

/// A single semantic fact emitted by the recipe runner for one batch of
/// episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledFact {
    /// One of `pr-pattern`, `bug-pattern`, `lesson-learned`. The recipe is
    /// constrained to this label set by its prompt.
    pub concept: String,
    /// Free-text content of the fact. Stored verbatim.
    pub content: String,
    /// `node_id` of the source episode (used to compose the `source_id` of the
    /// resulting fact as `distill:{id}` for provenance).
    pub source_episode_id: String,
}

/// A recurring action sequence distilled from a batch of episodes (issue #2327,
/// R5). Stored via `store_procedure_with_provenance` so a
/// `PROCEDURE_DERIVES_FROM` edge links the procedure back to its episodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DistilledProcedure {
    /// Procedure name (e.g. `ci-fix:auto`). Upsert-by-name on store.
    pub name: String,
    /// Ordered steps of the procedure.
    pub steps: Vec<String>,
    /// `node_id`s of the episodes this procedure was distilled from (threaded as
    /// provenance).
    pub source_episode_ids: Vec<String>,
}

/// The full output of one distillation pass: facts AND procedures (issue #2327,
/// R5). Additive over the legacy fact-only shape — a fact-only runner yields an
/// empty `procedures` vector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistillOutput {
    pub facts: Vec<DistilledFact>,
    pub procedures: Vec<DistilledProcedure>,
}

// ───────────────────────────────────────────────────────────────────────────
// Write-boundary sink (issue #2679): the in-process seam that applies the SAME
// reliability gate as the IPC server's `StoreFactGated` handler.
// ───────────────────────────────────────────────────────────────────────────

/// The write boundary a distilled fact/procedure is committed through.
///
/// Post-#2679 the distiller no longer returns a document Simard parses; it
/// *commits* each record. Two seams implement this trait: the real subprocess
/// runner delegates to the daemon's IPC gate (the agent calls
/// `simard memory remember`), and the deterministic test stubs are routed to
/// the in-process [`InProcessFactSink`], which applies the identical shared
/// [`crate::fact_reliability`] gate so a fact stores/quarantines the same way no
/// matter which boundary writes it.
pub trait DistillFactSink {
    /// Ground, score, gate, dedup, and (if it survives) persist one fact.
    /// Returns `true` when the fact was STORED, `false` when it was quarantined
    /// (low reliability or an equal-or-stronger prior already exists).
    fn commit_fact(&mut self, fact: &DistilledFact) -> SimardResult<bool>;

    /// Persist one procedure with its source-episode provenance.
    fn commit_procedure(&mut self, procedure: &DistilledProcedure) -> SimardResult<()>;
}

/// Per-pass tally of what the write boundary accepted (issue #2679), returned by
/// [`DistillRecipeRunner::run_agentic`] so the pass report and telemetry reflect
/// the gate's decisions even on the real agentic path (where the facts are
/// written by the agent through the daemon, not by the in-process sink).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DistillCommit {
    /// Facts the gate accepted and stored.
    pub facts: u32,
    /// Facts the gate quarantined (low reliability or dedup skip).
    pub quarantined: u32,
    /// Procedures stored.
    pub procedures: u32,
}

/// In-process implementation of [`DistillFactSink`] used by the deterministic
/// test stubs (and any run-only runner routed through the default
/// [`DistillRecipeRunner::run_agentic`]). Grounds a fact by **batch membership**
/// — the store-existence check the IPC server does has no meaning in-process, so
/// the batch it was distilled from is the ground truth — then applies the shared
/// reliability gate, so its store/quarantine decision matches the server seam.
struct InProcessFactSink<'a> {
    memory: &'a dyn CognitiveMemoryOps,
    /// Node ids of the batch's episodes, precomputed once so per-fact grounding
    /// is an O(1) set lookup rather than an O(batch) scan for every fact.
    episode_ids: std::collections::HashSet<&'a str>,
}

impl<'a> InProcessFactSink<'a> {
    fn new(memory: &'a dyn CognitiveMemoryOps, batch: &'a [CognitiveEpisode]) -> Self {
        let episode_ids = batch.iter().map(|e| e.node_id.as_str()).collect();
        Self {
            memory,
            episode_ids,
        }
    }
}

impl DistillFactSink for InProcessFactSink<'_> {
    fn commit_fact(&mut self, fact: &DistilledFact) -> SimardResult<bool> {
        // Normalize the cited id the same way the IPC server's grounding does
        // (`any_episode_exists` trims each cited id), so a cited id an LLM
        // re-emitted with stray surrounding whitespace still grounds — the two
        // seams must decide every fact's disposition identically. Episode node
        // ids never carry whitespace, so this is a no-op for a well-formed id.
        let source_episode_id =
            crate::fact_reliability::normalize_source_episode_id(&fact.source_episode_id);

        // Grounding is batch-membership for the in-process seam (O(1) set lookup)
        // over the normalized cited id — precisely symmetric with the production
        // store-existence seam, which trims each cited id before matching. Batch
        // node ids are canonical/un-padded, so normalizing only the cited id keeps
        // both write boundaries' store/quarantine disposition identical.
        let grounded = self.episode_ids.contains(source_episode_id);

        // Score → threshold → dedup → persist through the single shared gate, so
        // this in-process seam and the IPC server's `StoreFactGated` handler
        // decide every fact's disposition identically. The normalized id also
        // threads provenance so the `DERIVES_FROM` edge resolves rather than
        // dangling on a whitespace-padded key.
        let source = format!("distill:{source_episode_id}");
        let decision = crate::fact_reliability::commit_gated_fact(
            self.memory,
            &fact.concept,
            &fact.content,
            grounded,
            &source,
            std::slice::from_ref(&fact.concept),
            &[source_episode_id.to_string()],
        )?;

        match decision {
            crate::fact_reliability::FactGateDecision::Stored { .. } => Ok(true),
            // A below-threshold score and a dedup skip are BOTH quarantines; the
            // confidence tells them apart (a dedup skip cleared the threshold).
            crate::fact_reliability::FactGateDecision::Quarantined { confidence }
                if confidence < DISTILL_RELIABILITY_THRESHOLD =>
            {
                tracing::warn!(
                    target: "simard::distill",
                    concept = %fact.concept,
                    source_episode_id = %fact.source_episode_id,
                    confidence,
                    threshold = DISTILL_RELIABILITY_THRESHOLD,
                    "distill: quarantined low-reliability fact (below threshold), not promoted"
                );
                Ok(false)
            }
            crate::fact_reliability::FactGateDecision::Quarantined { confidence } => {
                tracing::info!(
                    target: "simard::distill",
                    concept = %fact.concept,
                    confidence,
                    "distill: an equal-or-stronger copy of this fact already exists; not downgrading prior"
                );
                Ok(false)
            }
        }
    }

    fn commit_procedure(&mut self, procedure: &DistilledProcedure) -> SimardResult<()> {
        self.memory.store_procedure_with_provenance(
            &procedure.name,
            &procedure.steps,
            &[],
            &procedure.source_episode_ids,
        )?;
        Ok(())
    }
}

/// Pluggable LLM-side runner for the distillation recipe.
///
/// The trait exists so tests can substitute a deterministic stub. Production
/// code uses [`RecipeRunnerSubprocess`] which shells out to `recipe-runner-rs`.
pub trait DistillRecipeRunner {
    /// Legacy fact-only entry point. Required so existing fact-only runners (and
    /// the test stubs) keep working — the default [`run_agentic`](Self::run_agentic)
    /// memories the facts they return into the in-process sink.
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>>;

    /// Full entry point emitting BOTH facts and procedures (issue #2327, R5). The
    /// default wraps [`run`](Self::run) with an empty procedure list, so a
    /// fact-only runner distils facts exactly as before.
    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        Ok(DistillOutput {
            facts: self.run(episodes)?,
            procedures: Vec::new(),
        })
    }

    /// Agentic entry point (issue #2679): the runner performs its distillation
    /// and commits each resulting fact/procedure through `sink` — the write
    /// boundary — rather than returning a document Simard must parse.
    ///
    /// The **default** memories a run-only stub's returned facts/procedures into
    /// the in-process [`InProcessFactSink`], so a stub that implements only
    /// [`run`](Self::run) (or [`run_all`](Self::run_all)) keeps working unchanged
    /// — the additive-trait contract. [`RecipeRunnerSubprocess`] **overrides**
    /// this: the distiller agent writes each fact DIRECTLY through the memory
    /// tool during the recipe run, so there is no returned document and nothing
    /// for the sink to receive; the override only interprets the recipe's exit
    /// status.
    fn run_agentic(
        &self,
        episodes: &[CognitiveEpisode],
        sink: &mut dyn DistillFactSink,
    ) -> SimardResult<DistillCommit> {
        let DistillOutput { facts, procedures } = self.run_all(episodes)?;
        let mut commit = DistillCommit::default();
        for fact in &facts {
            if sink.commit_fact(fact)? {
                commit.facts += 1;
            } else {
                commit.quarantined += 1;
            }
        }
        for procedure in &procedures {
            sink.commit_procedure(procedure)?;
            commit.procedures += 1;
        }
        Ok(commit)
    }
}

/// Report describing what one distillation pass actually did.
///
/// Two terminal shapes:
///
/// - **Skipped**: all counts zero (under threshold). Use
///   [`DistillReport::skipped`] / [`was_skipped`](DistillReport::was_skipped).
/// - **Ran**: `input_count >= DISTILL_MIN_EPISODES`. `fact_count` is the number
///   of facts committed through the in-process sink; `marked_count` is the number
///   of episodes marked distilled (equal to `input_count` on success).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DistillReport {
    /// Number of undistilled episodes pulled from the store.
    pub input_count: u32,
    /// Number of facts committed to semantic memory.
    pub fact_count: u32,
    /// Number of procedures committed (issue #2327, R5).
    pub procedure_count: u32,
    /// Number of episodes marked distilled after the pass.
    pub marked_count: u32,
    /// Number of candidate facts blocked by the ISAO reliability gate (issue
    /// #2433): quarantined for low self-assessed reliability OR skipped to avoid
    /// clobbering a higher-confidence existing fact.
    pub quarantined_count: u32,
}

impl DistillReport {
    /// The pass was skipped under threshold; no work was done.
    pub fn skipped() -> Self {
        Self::default()
    }

    /// `true` when the pass did not fire (all counts zero).
    pub fn was_skipped(&self) -> bool {
        self.input_count == 0
            && self.fact_count == 0
            && self.procedure_count == 0
            && self.marked_count == 0
            && self.quarantined_count == 0
    }

    /// Reduction ratio (`1 - fact_count / input_count`) in `[0.0, 1.0]`. Returns
    /// `0.0` when `input_count == 0`.
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
/// Contract (see `docs/architecture/distillation-semantic-handoff.md`):
///
/// - Under threshold → returns `Ok(DistillReport::skipped())`; runner is NOT
///   invoked; no markers set; no facts stored.
/// - Above threshold + recipe success → the agentic step's facts have been
///   committed and all input episodes are marked distilled.
/// - Above threshold + recipe error → returns `Err(...)`; no markers set (the
///   batch stays fully retry-eligible on the next pass).
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
        return Ok(DistillReport::skipped());
    }

    tracing::info!(
        target: "simard::distill",
        pulled,
        batch = DISTILL_BATCH_SIZE,
        min = DISTILL_MIN_EPISODES,
        "distill: {pulled} episodes pulled (batch size {DISTILL_BATCH_SIZE}, min {DISTILL_MIN_EPISODES})"
    );

    // Run the distiller as an agentic step. It commits its facts/procedures
    // through the sink (in-process stubs) or the memory tool (real subprocess).
    // Retry-safety invariant: on failure NO episode is marked, so the batch
    // stays fully retry-eligible next pass.
    let mut sink = InProcessFactSink::new(memory, &episodes);
    let commit = match runner.run_agentic(&episodes, &mut sink) {
        Ok(commit) => commit,
        Err(e) => {
            let class = classify_distill_error(&e);
            tracing::warn!(
                target: "simard::distill",
                pulled,
                class = class.as_str(),
                error = %e,
                "distill: {pulled} episodes pulled, recipe error, no markers set, retry next pass"
            );
            record_distill_success_metric(false, Some(class), pulled, 0);
            // NOTE (#2679): a failed pass is surfaced via `distill_success_rate`
            // (value 0.0) above and by propagating `Err`. We deliberately do NOT
            // emit a `simard.distill.runs` counter on failure: that counter now
            // tracks only completed agentic commits (`result="ok"`), and the
            // `parse_fail` result it used to carry no longer exists because there
            // is no parse to fail.
            return Err(e);
        }
    };

    let stored = commit.facts;
    let quarantined = commit.quarantined;
    let stored_procs = commit.procedures;

    // Before/after measurement of the gate's block-rate (issue #2433). Best-
    // effort, no-op under `cfg!(test)`.
    record_reliability_gate_metric(stored + quarantined, quarantined, stored);

    // Mark EVERY input episode distilled — even those the recipe classified
    // `skip`. The mark-everything rule is the prompt-replay-loop guard.
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

    // Issue #2528: mirror the distill outcome into the unified telemetry facade.
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

    record_distill_success_metric(true, None, pulled, stored);

    // Durable per-pass fact-yield (promoted facts per input episode) so the
    // real distiller's yield is observable/trendable over time — emitted only on
    // a completed pass, where `stored`/`quarantined` reflect real gate decisions.
    record_fact_yield_metric(pulled, stored, quarantined);

    Ok(DistillReport {
        input_count: pulled,
        fact_count: stored,
        procedure_count: stored_procs,
        marked_count: marked,
        quarantined_count: quarantined,
    })
}

/// Block-rate of the reliability gate for one pass: fraction of candidate facts
/// quarantined. `0.0` when there were no candidates.
fn gate_block_rate(quarantined: u32, candidate_facts: u32) -> f64 {
    if candidate_facts == 0 {
        0.0
    } else {
        quarantined as f64 / candidate_facts as f64
    }
}

/// Build the JSON `context` payload for the `distill_reliability_gate` metric.
fn build_reliability_gate_context(candidate_facts: u32, quarantined: u32, promoted: u32) -> String {
    serde_json::json!({
        "candidate_facts": candidate_facts,
        "promoted": promoted,
        "quarantined": quarantined,
        "block_rate": gate_block_rate(quarantined, candidate_facts),
        "threshold": DISTILL_RELIABILITY_THRESHOLD,
    })
    .to_string()
}

/// Record one `distill_reliability_gate` metric event per pass so the block-rate
/// (`quarantined / candidate_facts`) is measurable. Best-effort; no-op under
/// `cfg!(test)`.
fn record_reliability_gate_metric(candidate_facts: u32, quarantined: u32, promoted: u32) {
    if cfg!(test) {
        return;
    }
    let block_rate = gate_block_rate(quarantined, candidate_facts);
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
// distill_fact_yield instrumentation (perpetual-cognition goal)
// ───────────────────────────────────────────────────────────────────────────

/// Fact-yield of one distillation pass: promoted facts per input episode
/// (`fact_count / input_count`). `0.0` when no episodes were pulled — a guard
/// against a division-by-zero that the [`DISTILL_MIN_EPISODES`] skip already
/// makes unreachable on the emitting path.
///
/// This is the deterministic *definition* of fact-yield the docs use
/// ("facts-per-episode-batch"); it is emitted per pass as the durable
/// [`record_fact_yield_metric`] series so the yield of the real (non-deterministic
/// LLM) distiller is observable and trendable over time, the way
/// `recall_precision_at_k` makes ranked-recall quality observable.
fn fact_yield(fact_count: u32, input_count: u32) -> f64 {
    if input_count == 0 {
        0.0
    } else {
        f64::from(fact_count) / f64::from(input_count)
    }
}

/// Build the JSON `context` payload for the `distill_fact_yield` metric. Carries
/// the raw counts so a consumer can recompute the ratio or segment by pass size,
/// and `quarantined` so a low yield can be attributed to gate blocks vs. a
/// low-signal batch.
fn build_fact_yield_context(input_count: u32, fact_count: u32, quarantined: u32) -> String {
    serde_json::json!({
        "input_count": input_count,
        "fact_count": fact_count,
        "quarantined": quarantined,
        "fact_yield": fact_yield(fact_count, input_count),
    })
    .to_string()
}

/// Record one `distill_fact_yield` metric event per **completed** pass so
/// distillation fact-yield (promoted facts per input episode) is a first-class,
/// trendable self-metric rather than only inert context on `distill_success_rate`.
/// A regression in the real distiller's yield is then visible as a falling mean.
/// Best-effort; no-op under `cfg!(test)` so unit tests never append to the
/// operator's real `metrics.jsonl`.
fn record_fact_yield_metric(input_count: u32, fact_count: u32, quarantined: u32) {
    if cfg!(test) {
        return;
    }
    let value = fact_yield(fact_count, input_count);
    let context = build_fact_yield_context(input_count, fact_count, quarantined);
    if let Err(e) = crate::self_metrics::record_metric("distill_fact_yield", value, &context) {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_fact_yield metric (distillation unaffected)",
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// distill_success_rate instrumentation (issue #2461; parse class removed #2679)
// ───────────────────────────────────────────────────────────────────────────

/// Machine-readable class of a distillation failure, derived from the stable
/// leading prefix of the `SimardError::RpcError` message emitted at each runner
/// site in this module.
///
/// Post-#2679 the distiller writes facts DIRECTLY through the memory tool, so
/// there is no facts document to parse: the `ParseFailure` class of #2461/#2658
/// is **gone** — no code path can produce it. The only failure the result path
/// can now surface is a non-zero recipe exit ([`CopilotTerminalFailure`]) or a
/// spawn/serialize/other structural failure.
///
/// [`CopilotTerminalFailure`]: DistillFailureClass::CopilotTerminalFailure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistillFailureClass {
    /// `recipe-runner-rs` could not be spawned (binary missing/structural), or
    /// the per-invocation episodes tempfile could not be created.
    SpawnFailure,
    /// The recipe **process** exited non-zero.
    CopilotTerminalFailure,
    /// The episodes payload failed to serialize (structural; ~unreachable).
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
            Self::SerializeFailure => "serialize-failure",
            Self::Other => "other",
        }
    }

    /// `true` for failure classes worth a bounded in-cycle retry: a non-zero
    /// recipe exit is observed to recover on a second attempt. Structural classes
    /// (spawn/serialize, and `Other`) are deterministic and escalate immediately.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::CopilotTerminalFailure)
    }
}

/// Classify a distillation `SimardError` into a [`DistillFailureClass`].
///
/// Discrimination anchors on the **stable leading prefix** this module emits for
/// each class (`SimardError::RpcError(msg)` → `msg.starts_with(...)`), NOT on
/// `contains`: a terminal failure carries up to 200 chars each of recipe
/// stderr/stdout, and that variable tail always trails the fixed prefix.
pub fn classify_distill_error(err: &SimardError) -> DistillFailureClass {
    let SimardError::RpcError(msg) = err else {
        return DistillFailureClass::Other;
    };
    if msg.starts_with("distill: recipe-runner-rs spawn failed")
        || msg.starts_with("distill: failed to create episodes tempfile")
    {
        DistillFailureClass::SpawnFailure
    } else if msg.starts_with("distill: recipe exited with") {
        DistillFailureClass::CopilotTerminalFailure
    } else if msg.starts_with("distill: failed to serialize episodes payload") {
        DistillFailureClass::SerializeFailure
    } else {
        DistillFailureClass::Other
    }
}

/// Build the `distill_success_rate` metric context for one pass that ran the
/// recipe. `class` is `None` on success, `Some(_)` on failure.
fn build_distill_success_context(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
) -> String {
    serde_json::json!({
        "outcome": if success { "success" } else { "failure" },
        "failure_class": class.map(|c| c.as_str()),
        "input_count": input_count,
        "fact_count": fact_count,
    })
    .to_string()
}

/// Record the per-pass `distill_success_rate` metric (issue #2461) for every pass
/// that ran the recipe. Best-effort; no-op under `cfg!(test)`.
fn record_distill_success_metric(
    success: bool,
    class: Option<DistillFailureClass>,
    input_count: u32,
    fact_count: u32,
) {
    if cfg!(test) {
        return;
    }
    let value = if success { 1.0 } else { 0.0 };
    let context = build_distill_success_context(success, class, input_count, fact_count);
    if let Err(e) = crate::self_metrics::record_metric("distill_success_rate", value, &context) {
        tracing::warn!(
            target: "simard::distill",
            error = %e,
            "failed to record distill_success_rate metric (distillation unaffected)",
        );
    }
}

/// Production entry point: run one distillation pass using the `recipe-runner-rs`
/// subprocess.
///
/// Returns `Ok(DistillReport::skipped())` when the runner cannot be constructed
/// (e.g. `recipe-runner-rs` not on PATH, no recipe file, no agent binary).
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

/// Interpret a finished `recipe-runner-rs` invocation by its **exit status
/// alone** (issue #2679).
///
/// This is the entire result-path interpretation now: a clean (exit-0) run is
/// SUCCESS regardless of what landed on stdout — the distiller committed its
/// facts through the memory write boundary, so an empty or banner/ANSI/trailing-
/// comma-polluted terminal is expected, not an error. There is **no parse**, so
/// noisy stdout can no longer fail the pipeline. A non-zero exit is the only
/// failure the result path can report; it is surfaced with truncated
/// stderr/stdout context (never silently swallowed).
pub fn interpret_recipe_exit(output: &std::process::Output) -> SimardResult<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(SimardError::RpcError(format!(
        "distill: recipe exited with {}: stderr={} stdout={}",
        output.status,
        truncate(stderr.trim(), 200),
        truncate(stdout.trim(), 200)
    )))
}

/// Concrete subprocess-based recipe runner. Shells out to `recipe-runner-rs`,
/// delivering the episodes JSON through the shared file channel
/// (`-c episodes_path=<abs>`) so an unbounded batch can never overflow `ARG_MAX`
/// (issues #2640/#2692), and pointing the distiller agent at the memory socket
/// so its `simard memory remember` calls reach the daemon's write-boundary gate
/// (issue #2679).
pub struct RecipeRunnerSubprocess {
    recipe_path: std::path::PathBuf,
    agent_binary: &'static str,
    /// The memory IPC socket the distiller agent's `simard memory remember`
    /// calls must reach. Resolved once at construction from the daemon's state
    /// root and exported to the recipe subprocess so the agent commits into the
    /// same store this pass marks.
    memory_socket: std::path::PathBuf,
}

impl RecipeRunnerSubprocess {
    /// Construct a runner if all preconditions are met. Returns `None` when the
    /// recipe file is not found, no agent binary is configured, or
    /// `recipe-runner-rs` is not on `PATH`.
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
        // Resolve the memory socket the distiller must write through. This is the
        // same path the daemon publishes (`socket_path_for(state_root)`); we hand
        // it to the agent so its `simard memory remember` calls reach the live
        // gate rather than falling back to an un-gated direct open.
        let memory_socket =
            crate::memory_ipc::socket_path_for(&crate::state_root::simard_state_root());
        Some(Self {
            recipe_path,
            agent_binary,
            memory_socket,
        })
    }

    /// Shell out to `recipe-runner-rs` with the episodes payload and return the
    /// finished process `Output`. The distiller agent commits its facts DIRECTLY
    /// through the memory tool during the run (issue #2679), so this returns the
    /// raw `Output` and NEVER reads stdout for facts — [`interpret_recipe_exit`]
    /// decides success purely from the exit status.
    ///
    /// `pass_id` tags every `simard memory remember` write from this run so the
    /// daemon's per-pass ledger can report back how many facts the gate accepted
    /// (the only way to count facts on a path with no returned document).
    fn invoke_recipe(
        &self,
        episodes: &[CognitiveEpisode],
        pass_id: &str,
    ) -> SimardResult<std::process::Output> {
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

        // Route the (unbounded) episodes batch through the shared file channel:
        // up to 50 full-text episodes is far more than a single argv token can
        // hold (issues #2640/#2692). The guard lives until after `output()`.
        let episodes_cf =
            crate::recipe_context_file::ContextFile::write("distill", "episodes", &payload_json)
                .map_err(|e| {
                    SimardError::RpcError(format!(
                        "distill: episodes context-file write failed: {e}"
                    ))
                })?;

        let socket_arg = self.memory_socket.to_string_lossy().into_owned();
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            // Export the socket so the agent's `simard memory remember` resolves
            // the SAME daemon this pass marks episodes against, with no un-gated
            // fallback (issue #2679).
            .env("SIMARD_MEMORY_SOCKET", &self.memory_socket)
            // Tag this pass so the daemon's write ledger can report accepted-fact
            // counts back to this runner. The agent invokes `simard memory
            // remember` with only the content flags (no `--pass-id`), so the CLI
            // reads this env var as its pass-id source (issue #2679). This env
            // var is the ONLY channel — it is inherited by every remember
            // subprocess the agent spawns.
            .env(crate::memory_ipc::DISTILL_PASS_ID_ENV, pass_id)
            .arg("-c")
            .arg(episodes_cf.arg_value())
            .arg("-c")
            .arg(format!("memory_socket={socket_arg}"))
            .output()
            .map_err(|e| {
                SimardError::RpcError(format!("distill: recipe-runner-rs spawn failed: {e}"))
            })?;
        Ok(output)
    }

    /// Best-effort: ask the daemon how many facts the write-boundary gate
    /// accepted for `pass_id`, draining the ledger entry. Any transport failure
    /// (no daemon, socket gone) yields `0` — telemetry must never fail a pass.
    fn drain_pass_ledger(&self, pass_id: &str) -> u32 {
        match crate::memory_ipc::RemoteCognitiveMemory::connect(&self.memory_socket) {
            Ok(client) => client.drain_pass_ledger(pass_id).unwrap_or(0) as u32,
            Err(_) => 0,
        }
    }
}

/// Generate a process-unique distill pass id for the write ledger.
fn new_pass_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("distill-pass-{nanos}-{}", std::process::id())
}

impl DistillRecipeRunner for RecipeRunnerSubprocess {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>> {
        // The distiller commits its facts DIRECTLY through the memory tool during
        // the run, so there is no returned document. We only interpret the exit
        // status; on success there are no facts for Simard to hand back.
        let output = self.invoke_recipe(episodes, &new_pass_id())?;
        interpret_recipe_exit(&output)?;
        Ok(Vec::new())
    }

    fn run_agentic(
        &self,
        episodes: &[CognitiveEpisode],
        _sink: &mut dyn DistillFactSink,
    ) -> SimardResult<DistillCommit> {
        // The agent's `simard memory remember` writes ARE the output — the
        // in-process sink is unused on this path. Interpret the recipe by its
        // exit status alone (issue #2679): noisy/trailing-comma/banner stdout can
        // no longer fail the pipeline because nothing is parsed.
        let pass_id = new_pass_id();
        let output = self.invoke_recipe(episodes, &pass_id)?;
        interpret_recipe_exit(&output)?;
        // Report the gate-accepted fact count for this pass (best-effort). We do
        // not distinguish stored vs. quarantined on the real path (the agent's
        // per-fact CLI already surfaces each disposition); the ledger counts only
        // accepted facts.
        Ok(DistillCommit {
            facts: self.drain_pass_ledger(&pass_id),
            quarantined: 0,
            procedures: 0,
        })
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

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn reliability_gate_metric_context_shape() {
        let payload = build_reliability_gate_context(4, 1, 3);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["candidate_facts"], 4);
        assert_eq!(v["promoted"], 3);
        assert_eq!(v["quarantined"], 1);
        assert_eq!(v["block_rate"], 0.25);
        assert_eq!(v["threshold"], DISTILL_RELIABILITY_THRESHOLD);
    }

    #[test]
    fn reliability_gate_metric_context_zero_candidates_is_zero_rate() {
        let payload = build_reliability_gate_context(0, 0, 0);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["block_rate"], 0.0);
    }

    #[test]
    fn fact_yield_is_facts_per_episode() {
        // 3 facts from a 25-episode batch → 0.12 facts/episode.
        assert!((fact_yield(3, 25) - 0.12).abs() < 1e-9);
        // A perfect one-fact-per-episode pass → 1.0.
        assert_eq!(fact_yield(20, 20), 1.0);
    }

    #[test]
    fn fact_yield_is_zero_when_no_input_episodes() {
        // Guards the division; the DISTILL_MIN_EPISODES skip makes this
        // unreachable on the emitting path, but the helper must be total.
        assert_eq!(fact_yield(0, 0), 0.0);
        assert_eq!(fact_yield(5, 0), 0.0);
    }

    #[test]
    fn fact_yield_metric_context_shape() {
        let payload = build_fact_yield_context(25, 3, 2);
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(v["input_count"], 25);
        assert_eq!(v["fact_count"], 3);
        assert_eq!(v["quarantined"], 2);
        assert!((v["fact_yield"].as_f64().unwrap() - 0.12).abs() < 1e-9);
    }

    #[test]
    fn report_skipped_is_was_skipped() {
        assert!(DistillReport::skipped().was_skipped());
    }

    #[test]
    fn report_with_work_is_not_was_skipped() {
        let r = DistillReport {
            input_count: 20,
            fact_count: 2,
            procedure_count: 0,
            marked_count: 20,
            quarantined_count: 1,
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
        assert!((r.reduction() - 0.88).abs() < 1e-9);
    }

    #[test]
    fn classify_terminal_failure_anchors_on_prefix_not_embedded_stdout() {
        // A non-zero exit whose embedded stdout mentions unrelated text must
        // still classify as a terminal failure, not `Other`.
        let terminal = SimardError::RpcError(
            "distill: recipe exited with exit status: 1: stderr= stdout={\"facts\":[]}".to_string(),
        );
        assert_eq!(
            classify_distill_error(&terminal),
            DistillFailureClass::CopilotTerminalFailure
        );
        assert!(classify_distill_error(&terminal).is_transient());
    }

    #[test]
    fn classify_spawn_and_serialize_and_other() {
        assert_eq!(
            classify_distill_error(&SimardError::RpcError(
                "distill: recipe-runner-rs spawn failed: nope".into()
            )),
            DistillFailureClass::SpawnFailure
        );
        assert_eq!(
            classify_distill_error(&SimardError::RpcError(
                "distill: failed to serialize episodes payload: x".into()
            )),
            DistillFailureClass::SerializeFailure
        );
        assert_eq!(
            classify_distill_error(&SimardError::RpcError("backend blew up".into())),
            DistillFailureClass::Other
        );
    }

    #[test]
    fn interpret_exit_zero_is_ok_regardless_of_stdout() {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"{\"facts\":[{\"a\":1},],}\x1b[0m noisy banner".to_vec(),
            stderr: Vec::new(),
        };
        assert!(interpret_recipe_exit(&output).is_ok());
    }

    #[test]
    fn interpret_nonzero_exit_is_terminal_error() {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"boom".to_vec(),
        };
        let err = interpret_recipe_exit(&output).expect_err("nonzero exit must error");
        assert_eq!(
            classify_distill_error(&err),
            DistillFailureClass::CopilotTerminalFailure
        );
    }
}
