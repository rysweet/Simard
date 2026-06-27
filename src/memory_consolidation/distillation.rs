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

    // Run the recipe. On error, return immediately WITHOUT marking
    // any episodes — the batch is fully retry-eligible on the next
    // pass. This is the retry-safety invariant under test in
    // `distillation_handles_recipe_error_without_marking`.
    let output = match runner.run_all(&episodes) {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                target: "simard::distill",
                pulled,
                error = %e,
                "distill: {pulled} episodes pulled, recipe error, no markers set, retry next pass"
            );
            eprintln!(
                "[simard] distill: {pulled} episodes pulled, recipe error: {e}, no markers set, retry next pass"
            );
            return Err(e);
        }
    };
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
        let raw = self.invoke_recipe(episodes)?;
        parse_recipe_output(&raw)
    }

    fn run_all(&self, episodes: &[CognitiveEpisode]) -> SimardResult<DistillOutput> {
        let raw = self.invoke_recipe(episodes)?;
        parse_recipe_output_full(&raw)
    }
}

impl RecipeRunnerSubprocess {
    /// Shell out to `recipe-runner-rs` with the episodes payload and return
    /// the recipe's raw stdout. Shared by [`run`](DistillRecipeRunner::run)
    /// and [`run_all`](DistillRecipeRunner::run_all).
    fn invoke_recipe(&self, episodes: &[CognitiveEpisode]) -> SimardResult<String> {
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
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("--output-format")
            .arg("json")
            .arg("-c")
            .arg(format!("episodes={payload_json}"))
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

/// Scan `text` for the first balanced `{...}` substring that deserializes as a
/// [`RecipeEnvelope`] (a bare `{ "facts": [...], "procedures": [...] }` object),
/// tolerating leading/trailing prose. Returns `None` if none is found.
///
/// The scan is iterative (no recursion) so pathologically deep brace nesting
/// terminates without a stack overflow; serde's own recursion limit bounds the
/// per-candidate parse.
fn scan_for_facts_object(text: &str) -> Option<DistillOutput> {
    let trimmed = text.trim();
    // Fast path — the text IS the JSON object.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(trimmed) {
        return Some(parsed.into_output());
    }
    // Slow path — find the first balanced `{...}` substring that parses.
    if let Some(start) = trimmed.find('{') {
        let mut depth = 0i32;
        let bytes = trimmed.as_bytes();
        for i in start..bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0
                        && let Ok(parsed) =
                            serde_json::from_str::<RecipeEnvelope>(&trimmed[start..=i])
                    {
                        return Some(parsed.into_output());
                    }
                }
                _ => {}
            }
        }
    }
    None
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
}
