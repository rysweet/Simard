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
/// "source_episode_id": "..." } ] }`. Two real-world sources of noise
/// sit in front of (or around) that object and must be tolerated:
///
/// 1. The underlying LLM may wrap the JSON in prose.
/// 2. The Copilot CLI prepends ANSI-colored launch / INFO log lines
///    (e.g. `launching copilot`, `NODE_OPTIONS=…`, and ISO-timestamp
///    lines wrapped in `\x1b[2m…\x1b[0m` SGR escapes) ahead of the
///    JSON — see issue #2496.
///
/// To recover the payload we first strip ANSI/VT escape sequences, then
/// scan for the first balanced top-level `{...}` substring that
/// deserializes into the `{ "facts": [...] }` envelope. The scan
/// re-anchors at every `{` so brace-bearing log preamble (a stray log
/// line that happens to contain its own `{...}`) does not defeat it,
/// and it is string-literal aware so braces inside JSON string values
/// do not throw off the depth count.
///
/// Returns `Err` when no parseable object is found — the caller
/// treats `Err` as the retry-safe "no markers set" path.
fn parse_recipe_output(raw: &str) -> SimardResult<Vec<DistilledFact>> {
    // Strip ANSI/VT escape sequences first so leading colored log noise
    // (Copilot CLI launch banner, dim-styled timestamps) cannot wedge
    // the brace scan.
    let cleaned = strip_ansi_escapes(raw);
    let trimmed = cleaned.trim();

    // Fast path — the (cleaned) stdout IS the JSON object.
    if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(trimmed) {
        return Ok(parsed.into_facts());
    }

    // Slow path — tolerate arbitrary leading/trailing log noise by
    // scanning for the first balanced top-level object that matches the
    // envelope contract.
    if let Some(parsed) = scan_for_facts_object(trimmed) {
        return Ok(parsed.into_facts());
    }

    Err(SimardError::BridgeError(format!(
        "distill: recipe output did not contain a parseable {{ \"facts\": [...] }} object; raw: {}",
        truncate(raw, 200)
    )))
}

/// Remove ANSI/VT100 escape sequences from `input`.
///
/// Handles the CSI form used for color/SGR codes (`ESC [ … final`,
/// where `final` is a byte in `0x40..=0x7e`) — which is what the
/// Copilot CLI emits around its launch banner and timestamps — plus
/// any other `ESC`-introduced sequence by dropping the `ESC` and the
/// byte that follows it. Operates byte-wise: ANSI escape bytes are all
/// ASCII (`ESC` = `0x1b`, finals < `0x80`) so multibyte UTF-8 content
/// (whose bytes are all `>= 0x80`) is copied through untouched.
fn strip_ansi_escapes(input: &str) -> String {
    const ESC: u8 = 0x1b;
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == ESC {
            // CSI sequence: ESC [ <params/intermediates> <final 0x40..=0x7e>.
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1; // consume the final byte
                }
                continue;
            }
            // Any other escape (e.g. a two-byte C1 form like `ESC M`):
            // drop the `ESC` and, when present, a single following ASCII
            // byte. Never consume a byte `>= 0x80` — that would split a
            // multibyte UTF-8 character (no valid escape uses one).
            i += 1;
            if i < bytes.len() && bytes[i] < 0x80 {
                i += 1;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Scan `text` for the first balanced top-level `{...}` substring that
/// deserializes into a [`RecipeEnvelope`] (i.e. carries a `facts` key).
///
/// Re-anchors at every `{` so non-envelope objects in the preamble are
/// skipped, and tracks JSON string state so braces inside string values
/// are ignored when matching.
fn scan_for_facts_object(text: &str) -> Option<RecipeEnvelope> {
    let bytes = text.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find('{') {
        let start = search_from + rel;
        match matching_brace_end(bytes, start) {
            Some(end) => {
                if let Ok(parsed) = serde_json::from_str::<RecipeEnvelope>(&text[start..=end]) {
                    return Some(parsed);
                }
                // Balanced but not the envelope we want (e.g. a JSON log
                // line). Resume just past this object rather than at
                // `start + 1` so its interior is not re-scanned — keeps
                // the scan linear over a run of balanced log objects and
                // matches the "first balanced top-level object" contract.
                search_from = end + 1;
            }
            // Unbalanced from this `{` (e.g. a stray brace in a log
            // line). Advance to the next `{` and try again rather than
            // giving up — a self-contained object may still follow.
            None => search_from = start + 1,
        }
    }
    None
}

/// Return the index of the `}` that closes the `{` at `start`, or
/// `None` if the braces never balance. String-literal aware: `{`/`}`
/// inside a double-quoted JSON string (honoring `\` escapes) do not
/// affect the depth count.
fn matching_brace_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
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
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
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

    // ── issue #2496 regression: tolerate Copilot CLI launch/ANSI preamble ──

    #[test]
    fn parse_recipe_output_tolerates_ansi_copilot_preamble() {
        // Captured-shape payload from the live failure (episodes
        // t=9269/t=9271): the Copilot CLI prepends an ANSI-colored launch
        // banner plus `NODE_OPTIONS=…` and dim-styled ISO-timestamp INFO
        // lines ahead of the JSON. The `\x1b[…m` SGR escapes and the log
        // preamble must be stripped/skipped so the facts object parses.
        let raw = concat!(
            "\u{1b}[1mlaunching copilot\u{1b}[0m\n",
            "\u{1b}[2mNODE_OPTIONS=--max-old-space-size=4096\u{1b}[0m\n",
            "\u{1b}[2m2026-06-29T12:26:24.123Z\u{1b}[0m \u{1b}[36mINFO\u{1b}[0m starting distill step\n",
            "{\"facts\":[{\"concept\":\"bug-pattern\",",
            "\"content\":\"strip ANSI before parsing distill output\",",
            "\"source_episode_id\":\"epi_9271\"}]}\n",
        );
        let facts = parse_recipe_output(raw).expect("ANSI/log preamble must not defeat the parser");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert_eq!(facts[0].source_episode_id, "epi_9271");
    }

    #[test]
    fn parse_recipe_output_skips_non_facts_json_log_line() {
        // A structured log line (its own balanced JSON object with no
        // `facts` key) precedes the real payload. The scan must re-anchor
        // past it and land on the envelope rather than failing on the
        // first `{...}` it encounters.
        let raw = concat!(
            "\u{1b}[2m{\"level\":\"info\",\"msg\":\"launching copilot\"}\u{1b}[0m\n",
            "{\"facts\":[{\"concept\":\"pr-pattern\",",
            "\"content\":\"re-anchor brace scan\",\"source_episode_id\":\"epi_9269\"}]}",
        );
        let facts = parse_recipe_output(raw).expect("must skip the non-facts log object");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "pr-pattern");
        assert_eq!(facts[0].source_episode_id, "epi_9269");
    }

    #[test]
    fn parse_recipe_output_handles_braces_inside_string_values() {
        // `content` legitimately contains `{` and `}`; the string-literal
        // aware brace matcher must not let those throw off depth tracking
        // and truncate the object early.
        let raw = concat!(
            "\u{1b}[2mlaunching copilot\u{1b}[0m\n",
            "{\"facts\":[{\"concept\":\"lesson-learned\",",
            "\"content\":\"prefer HashMap<K, {V}> over a raw { Vec }\",",
            "\"source_episode_id\":\"epi_1\"}]}",
        );
        let facts =
            parse_recipe_output(raw).expect("braces inside strings must not break matching");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "lesson-learned");
        assert!(facts[0].content.contains("{V}"));
    }

    #[test]
    fn parse_recipe_output_clean_input_is_unchanged() {
        // Behaviour for already-clean (no-ANSI, no-preamble) output is
        // unchanged: a pretty-printed multi-fact envelope still parses.
        let raw = r#"{
            "facts": [
                {"concept":"pr-pattern","content":"a","source_episode_id":"epi_1"},
                {"concept":"bug-pattern","content":"b","source_episode_id":"epi_2"}
            ]
        }"#;
        let facts = parse_recipe_output(raw).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].concept, "pr-pattern");
        assert_eq!(facts[1].concept, "bug-pattern");
    }

    #[test]
    fn parse_recipe_output_skips_multiple_leading_json_log_objects() {
        // Several balanced JSON log objects precede the envelope. The
        // scan must walk past each (resuming past its close) and land on
        // the first top-level object that carries a `facts` key.
        let raw = concat!(
            "\u{1b}[2m{\"ts\":\"t1\",\"msg\":\"launching copilot\"}\u{1b}[0m\n",
            "{\"ts\":\"t2\",\"msg\":\"NODE_OPTIONS set\"}\n",
            "{\"ts\":\"t3\",\"level\":\"info\",\"msg\":\"distill step start\"}\n",
            "{\"facts\":[{\"concept\":\"bug-pattern\",",
            "\"content\":\"walk past leading log objects\",",
            "\"source_episode_id\":\"epi_9271\"}]}",
        );
        let facts =
            parse_recipe_output(raw).expect("must skip leading log objects and find the envelope");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "bug-pattern");
        assert_eq!(facts[0].source_episode_id, "epi_9271");
    }

    #[test]
    fn parse_recipe_output_recovers_after_unbalanced_brace_in_log_line() {
        // A log line containing a stray, unclosed `{` precedes the JSON.
        // The scan must re-anchor past the dangling brace and still
        // recover the envelope on the following line.
        let raw = concat!(
            "INFO loading config { from disk\n",
            "{\"facts\":[{\"concept\":\"lesson-learned\",",
            "\"content\":\"re-anchor past dangling brace\",",
            "\"source_episode_id\":\"epi_1\"}]}",
        );
        let facts =
            parse_recipe_output(raw).expect("dangling `{` in a log line must not defeat recovery");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].concept, "lesson-learned");
    }

    #[test]
    fn strip_ansi_escapes_removes_sgr_and_preserves_text() {
        let s = "\u{1b}[2mdim\u{1b}[0m plain \u{1b}[36mcyan\u{1b}[0m";
        assert_eq!(strip_ansi_escapes(s), "dim plain cyan");
    }

    #[test]
    fn strip_ansi_escapes_preserves_utf8_payload() {
        // Multibyte UTF-8 bytes are all >= 0x80 and must survive the
        // byte-wise escape stripping intact.
        let s = "\u{1b}[2mcafé — résumé\u{1b}[0m";
        assert_eq!(strip_ansi_escapes(s), "café — résumé");
    }

    #[test]
    fn strip_ansi_escapes_non_csi_does_not_split_utf8() {
        // A bare ESC immediately followed by a multibyte char must drop
        // only the ESC, never the UTF-8 lead byte (which would corrupt
        // the character). A two-byte C1 form (ESC + ASCII final) drops
        // both.
        assert_eq!(strip_ansi_escapes("a\u{1b}éb"), "aéb");
        assert_eq!(strip_ansi_escapes("a\u{1b}Mb"), "ab");
    }

    #[test]
    fn matching_brace_end_ignores_braces_in_strings() {
        let s = r#"{"k":"a } b { c"}trailing"#;
        let end = matching_brace_end(s.as_bytes(), 0).expect("must balance");
        assert_eq!(&s[0..=end], r#"{"k":"a } b { c"}"#);
    }
}
