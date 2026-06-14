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
//! 4. Stores each emitted fact via `store_fact`.
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

/// Default per-fact confidence written via `store_fact`. The recipe
/// itself does not produce confidence scores; this is the canonical
/// "moderately confident, machine-distilled" value used by all PR-B
/// facts so downstream filtering can recognize the source.
pub const DISTILL_FACT_CONFIDENCE: f64 = 0.7;

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

/// Pluggable LLM-side runner for the distillation recipe.
///
/// The trait exists so tests can substitute a deterministic stub.
/// Production code uses [`RecipeRunnerSubprocess`] which shells out
/// to `recipe-runner-rs`.
pub trait DistillRecipeRunner {
    fn run(&self, episodes: &[CognitiveEpisode]) -> SimardResult<Vec<DistilledFact>>;
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
    /// Number of episodes marked distilled after the pass.
    pub marked_count: u32,
}

impl DistillReport {
    /// The pass was skipped under threshold; no work was done.
    pub fn skipped() -> Self {
        Self::default()
    }

    /// `true` when the pass did not fire (input/output/marks all zero).
    pub fn was_skipped(&self) -> bool {
        self.input_count == 0 && self.fact_count == 0 && self.marked_count == 0
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
    let facts = match runner.run(&episodes) {
        Ok(f) => f,
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

    // Store every fact. Each fact's source_id encodes the originating
    // episode for provenance (search_facts can be filtered/grepped on
    // the `distill:` prefix to identify machine-distilled facts).
    let mut stored = 0u32;
    for fact in &facts {
        let concepts = [fact.concept.clone()];
        let source = format!("distill:{}", fact.source_episode_id);
        memory.store_fact(
            &fact.concept,
            &fact.content,
            DISTILL_FACT_CONFIDENCE,
            &concepts,
            &source,
        )?;
        stored += 1;
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
        marked,
        reduction_pct,
        "distill: {pulled} episodes → {stored} facts, {marked} marked (reduction {reduction_pct:.0}%)"
    );
    eprintln!("[simard] distill: {pulled} episodes → {stored} facts, {marked} marked");

    Ok(DistillReport {
        input_count: pulled,
        fact_count: stored,
        marked_count: marked,
    })
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

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("episodes={payload_json}"))
            .output()
            .map_err(|e| {
                SimardError::BridgeError(format!("distill: recipe-runner-rs spawn failed: {e}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(SimardError::BridgeError(format!(
                "distill: recipe exited with {}: {}",
                output.status,
                truncate(&stderr, 240)
            )));
        }

        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        parse_recipe_output(&raw)
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
/// The recipe is expected to emit a JSON object of shape
/// `{ "facts": [ { "concept": "...", "content": "...",
/// "source_episode_id": "..." } ] }`. Because the underlying LLM may
/// wrap the JSON in prose, we scan for the first balanced `{...}`
/// substring containing a `"facts"` key and parse that.
///
/// Returns `Err` when no parseable object is found — the caller
/// treats `Err` as the retry-safe "no markers set" path.
fn parse_recipe_output(raw: &str) -> SimardResult<Vec<DistilledFact>> {
    let trimmed = raw.trim();
    // Fast path — recipe stdout IS the JSON object.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(trimmed) {
        return Ok(parsed.into_facts());
    }
    // Slow path — find the first balanced `{...}` substring and try
    // each one until something parses. Cheap because the output is
    // small.
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
                        return Ok(parsed.into_facts());
                    }
                }
                _ => {}
            }
        }
    }
    Err(SimardError::BridgeError(format!(
        "distill: recipe output did not contain a parseable {{ \"facts\": [...] }} object; raw: {}",
        truncate(raw, 200)
    )))
}

#[derive(serde::Deserialize)]
struct RecipeEnvelope {
    facts: Vec<RecipeFact>,
}

#[derive(serde::Deserialize)]
struct RecipeFact {
    concept: String,
    content: String,
    source_episode_id: String,
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
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn report_skipped_is_was_skipped() {
        assert!(DistillReport::skipped().was_skipped());
    }

    #[test]
    fn report_with_work_is_not_was_skipped() {
        let r = DistillReport {
            input_count: 25,
            fact_count: 3,
            marked_count: 25,
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
            marked_count: 25,
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
