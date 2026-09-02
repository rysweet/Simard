//! Prompt-first journal generation (issue #2606, guideline G3).
//!
//! The preferred production path for the narrative report and its plain-language
//! rewrite is agentic, not hand-rolled Rust: a [`RecipeDrafter`] runs the
//! `journal-narrative` recipe to write the professional, third-person report from
//! the day's structured context, and a [`RecipeReviewer`] runs the
//! `journal-plain-language` recipe to rewrite that draft so a non-engineer can read
//! it. Both shell out to `recipe-runner-rs` exactly like the episode-distiller
//! ([`crate::memory_consolidation::distillation`]).
//!
//! ## E2BIG-safe transport (issues #2640/#2692)
//!
//! The day's context and the draft are *unbounded* — a busy 24 h of episodic
//! memories, PR summaries, and a full narrative rewrite. Inlining them as
//! `-c day_context=<…>` / `-c draft=<…>` argv tokens overflowed the kernel's
//! per-argument limit and `execve` failed with `E2BIG` ("Argument list too
//! long", `errno 7`) BEFORE `recipe-runner-rs` ever started — the live,
//! once-per-hour journal failure. Both values are now delivered through the
//! shared file channel ([`crate::recipe_context_file::ContextFile`]): the payload
//! is written to a private temp file and only a short `<key>_path=<abs>` rides on
//! argv, so `ARG_MAX` is irrelevant and the recipe reads the full payload from
//! the file (mirrors the distiller's `facts_output_path`).
//!
//! ## Clean result channel (bug #2679)
//!
//! The agent's RESULT is likewise read from a dedicated result **file** it is
//! told to write (`-c narrative_output=<abs>` / `-c plain_output=<abs>`), never
//! scraped from `recipe-runner-rs` stdout. Raw stdout carries the copilot
//! launcher banner (`WARN nested amplihack session`, `INFO launching copilot`,
//! `ℹ NODE_OPTIONS=…`) and the agent's own box-drawing tool-call trace
//! (`● Read draft.ctx`, `│ …`, `└ N lines read`); scraping it made all of that
//! the LEADING text of the stored journal — the raw-stdout-scrape antipattern
//! already fixed for goal decomposition (issue #2708). [`harvest_narrative_file`]
//! reads the narrative from the clean file and treats stdout as inert, and
//! [`select_recipe`] refuses a stale hot-reloaded recipe that lacks the
//! result-file contract marker so a pre-#2679 asset can never shadow the fix.
//!
//! ## No silent fallback (operator rule: fallback == silent failure)
//!
//! A genuine spawn failure is NOT swallowed into a bare `warn!`. It is classified
//! ([`crate::overseer::diagnosis::classify_spawn_failure`]) and recorded into the
//! Overseer's [`crate::overseer::failure_sink`] so the next Observe pass lifts it
//! into a corrective `Signal::StepFailureDiagnosed`; the degrade is logged at
//! `error` level. The deterministic fallback ([`TemplateDrafter`] /
//! [`scrub_jargon`]) is a readable last resort only: its "Remembered moments"
//! section drops raw error-log episodes so a degraded journal can never again be
//! a dump of historical E2BIG error text. The generator's unconditional
//! secret-redaction post-pass runs regardless of which reviewer produced the
//! text, so a credential survives neither path.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{SimardError, SimardResult};
use crate::journal::generate::{JournalDrafter, JournalReviewer, TemplateDrafter};
use crate::journal::jargon::scrub_jargon;
use crate::journal::providers::episode_time_label;
use crate::journal::types::DayContext;
use crate::overseer::diagnosis::classify_spawn_failure;
use crate::overseer::failure_sink::record_step_failure;
use crate::recipe_context_file::ContextFile;

/// Recipe that writes the narrative engineering-and-research report draft.
const JOURNAL_DRAFT_RECIPE: &str = "journal-narrative.yaml";
/// Recipe that rewrites a draft into plain, layperson-readable language.
const JOURNAL_DEJARGON_RECIPE: &str = "journal-plain-language.yaml";
/// Adapter tag used for error attribution.
const ADAPTER_TAG: &str = "journal";

/// The `-c` context key the draft recipe writes its clean narrative FILE to.
/// Doubles as the result-file contract marker for [`select_recipe`] (bug #2679).
const NARRATIVE_OUTPUT_KEY: &str = "narrative_output";
/// The `-c` context key the plain-language recipe writes its clean rewrite FILE
/// to. Doubles as the result-file contract marker for [`select_recipe`].
const PLAIN_OUTPUT_KEY: &str = "plain_output";

/// Journal recipe candidates in descending priority: hot-reload location first,
/// then the in-tree asset:
///   1. `<home>/.simard/prompt_assets/simard/recipes/<filename>`
///   2. `<repo_root>/prompt_assets/simard/recipes/<filename>`
fn recipe_candidates(repo_root: &Path, filename: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::with_capacity(2);
    if let Some(home) = dirs::home_dir() {
        candidates.push(
            home.join(".simard")
                .join("prompt_assets/simard/recipes")
                .join(filename),
        );
    }
    candidates.push(
        repo_root
            .join("prompt_assets/simard/recipes")
            .join(filename),
    );
    candidates
}

/// Whether the recipe at `path` declares the post-#2679 clean-result-file
/// contract — it mentions the `<output_key>` context var (e.g.
/// `narrative_output`) the agent must write its final report to. An unreadable
/// file is treated as non-compatible so it can never win selection.
fn recipe_declares_result_file(path: &Path, output_key: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|contents| contents.contains(output_key))
        .unwrap_or(false)
}

/// Choose which recipe to run from `candidates` (descending priority).
///
/// Returns the first candidate that exists AND declares the clean-result-file
/// contract for `output_key`, so a stale (pre-#2679) hot-reloaded recipe — one
/// that still relies on scraped stdout — can never shadow a contract-aware one.
/// If no candidate declares the contract, falls back to the highest-priority
/// existing recipe (the run then surfaces a loud "result file was not written"
/// error rather than silently scraping stdout). Returns `None` only when no
/// candidate exists at all (the caller then uses the deterministic fallback).
fn select_recipe(candidates: &[PathBuf], output_key: &str) -> Option<PathBuf> {
    let existing: Vec<&PathBuf> = candidates.iter().filter(|p| p.is_file()).collect();
    existing
        .iter()
        .find(|p| recipe_declares_result_file(p, output_key))
        .or_else(|| existing.first())
        .map(|p| (*p).clone())
}

/// A resolved journal recipe, the agent binary to run it with, and the `-c`
/// context key the recipe writes its CLEAN result FILE to (bug #2679).
struct JournalRecipe {
    recipe_path: PathBuf,
    agent_binary: &'static str,
    /// Result-file context key (`narrative_output` / `plain_output`): the agent
    /// is told this absolute path and writes ONLY its final report there, so the
    /// narrative is read from a clean file — never scraped from noisy stdout.
    output_key: &'static str,
}

impl JournalRecipe {
    /// Construct a runner when all preconditions hold: a contract-aware recipe
    /// file resolves (declares the `output_key` clean-result-file marker), an
    /// agent binary is configured, and `recipe-runner-rs` is on `PATH`.
    /// Returns `None` otherwise (the caller falls back to deterministic).
    fn new(repo_root: &Path, filename: &str, output_key: &'static str) -> Option<Self> {
        let candidates = recipe_candidates(repo_root, filename);
        let recipe_path = select_recipe(&candidates, output_key)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if Command::new("recipe-runner-rs")
            .arg("--version")
            .env("AMPLIHACK_AGENT_BINARY", agent_binary)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return None;
        }
        Some(Self {
            recipe_path,
            agent_binary,
            output_key,
        })
    }

    /// Run the recipe with the given context vars and return the agent's CLEAN
    /// narrative, read from the dedicated result **file** it was told to write —
    /// NEVER from `recipe-runner-rs` stdout (bug #2679). Raw stdout carries the
    /// copilot launcher banner (`WARN nested amplihack session`, `INFO launching
    /// copilot`, `ℹ NODE_OPTIONS=…`) and the agent's own box-drawing tool-call
    /// trace (`● Read draft.ctx`, `│ …`, `└ N lines read`); scraping it made all
    /// of that the LEADING text of the stored journal. Now a fresh per-invocation
    /// tempdir (mode 0700 via `tempfile`) supplies a unique result path passed as
    /// `-c <output_key>=<abs>`, and the recipe prompt writes ONLY the final report
    /// there — stdout is inert.
    ///
    /// Every INPUT context value is *unbounded*, so each is delivered through the
    /// shared file channel ([`ContextFile`]): the payload goes to a private temp
    /// file and only the short `<key>_path=<abs>` rides on argv, so a full day's
    /// context can never overflow `ARG_MAX` and fail the spawn with `E2BIG`
    /// (issues #2640/#2692). Errors loudly on spawn/exit failure or a
    /// missing/empty/oversized result file; there is deliberately NO stdout
    /// fallback.
    ///
    /// A pre-exec spawn failure (the E2BIG defect and its siblings) is an
    /// `io::Error` with no child, so it is classified and recorded into the
    /// Overseer's failure sink here — never swallowed — before it propagates.
    fn run(&self, ctx: &[(&str, String)]) -> SimardResult<String> {
        let mut cmd = Command::new("recipe-runner-rs");
        cmd.arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary);
        // Route every INPUT context var through the file channel; the guards must
        // outlive `output()` so the files exist while the recipe reads them.
        let mut guards: Vec<ContextFile> = Vec::with_capacity(ctx.len());
        for (key, value) in ctx {
            let cf = ContextFile::write(ADAPTER_TAG, key, value).map_err(|e| {
                // A temp-file write failure (e.g. ENOSPC) is a spawn-class
                // failure too: classify + record before degrading.
                record_step_failure(classify_spawn_failure(&e));
                invocation_failed(format!(
                    "recipe-runner-rs context-file write failed for {key}: {e}"
                ))
            })?;
            cmd.arg("-c").arg(cf.arg_value());
            guards.push(cf);
        }

        // Dedicated, per-invocation result file the agent writes its clean final
        // report to (bug #2679). The fresh tempdir (mode 0700 via `tempfile`)
        // gives a unique absolute path with no cross-invocation races, and is
        // removed when `result_dir` drops at the end of this call — AFTER the
        // narrative has been read by `harvest_narrative_file` below. This mirrors
        // the decomposition clean-result-channel fix
        // ([`crate::goal_curation::decompose`], issue #2708).
        let result_dir = tempfile::Builder::new()
            .prefix("simard-journal-result-")
            .tempdir()
            .map_err(|e| {
                // A tempdir-create failure (e.g. ENOSPC) is a spawn-class failure:
                // classify + record before degrading, like the input channel.
                record_step_failure(classify_spawn_failure(&e));
                invocation_failed(format!("failed to create journal result-file tempdir: {e}"))
            })?;
        let result_path = result_dir.path().join(format!("{}.md", self.output_key));
        let result_path_arg = result_path.to_string_lossy().into_owned();
        cmd.arg("-c")
            .arg(format!("{}={result_path_arg}", self.output_key));

        let output = cmd.output().map_err(|e| {
            // Pre-exec spawn failure (E2BIG / ENOSPC / ENOMEM / …). Record a
            // structured diagnosis for the Overseer to act on — the "no silent
            // fallback" invariant — then surface the error to the caller.
            record_step_failure(classify_spawn_failure(&e));
            invocation_failed(format!("recipe-runner-rs spawn failed: {e}"))
        })?;

        // Read the CLEAN narrative from the dedicated result file — never from
        // stdout. `result_dir` is still alive, so the file exists while it is
        // read; it is unlinked when this call returns.
        harvest_narrative_file(&output, &result_path)
    }
}

/// Maximum size (bytes) accepted for a journal result file. A runaway agent that
/// writes an enormous file must be rejected loudly *before* the read, never
/// allowed to OOM the process. Mirrors the decomposition seam's
/// `MAX_SUBGOALS_FILE_BYTES` guard.
const MAX_NARRATIVE_FILE_BYTES: u64 = 1024 * 1024;

/// Post-process a finished `recipe-runner-rs` invocation into the journal
/// narrative, reading it from the dedicated result **file** the agent was told
/// to write (`-c <output_key>=…`) — NEVER from stdout (bug #2679).
///
/// * A non-zero exit is a **loud** terminal `journal` failure carrying the
///   truncated stderr and stdout, so a failed run is never a silent success.
/// * On a clean (exit-0) run the narrative is read from `path`. A missing,
///   empty/whitespace-only, or oversized file is a **loud** `journal` error.
///   There is deliberately NO stdout fallback: scraping stdout is exactly the
///   launcher-banner / tool-trace contamination this fix removes.
/// * The file is decoded lossily (`from_utf8_lossy`), so a malformed agent write
///   can never panic the reader.
///
/// Split out of [`JournalRecipe::run`] so the "stdout noise is inert" contract
/// is hermetically testable without spawning a subprocess.
pub(crate) fn harvest_narrative_file(
    output: &std::process::Output,
    path: &Path,
) -> SimardResult<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(invocation_failed(format!(
            "recipe-runner-rs exited with {}: stderr={} stdout={}",
            output.status,
            truncate(stderr.trim(), 200),
            truncate(stdout.trim(), 200)
        )));
    }

    // Size guard BEFORE the read. A missing file surfaces here as a loud journal
    // error (the agent produced no result), never a stdout fallback.
    let meta = std::fs::metadata(path).map_err(|e| {
        invocation_failed(format!(
            "journal result file {} was not written by the agent: {e}",
            path.display()
        ))
    })?;
    if meta.len() > MAX_NARRATIVE_FILE_BYTES {
        return Err(invocation_failed(format!(
            "journal result file {} is {} bytes, exceeding the {MAX_NARRATIVE_FILE_BYTES}-byte cap",
            path.display(),
            meta.len()
        )));
    }

    // Read raw bytes and decode lossily: a malformed agent write (invalid UTF-8)
    // must be handled, never panic the reader (unlike `read_to_string`).
    let bytes = std::fs::read(path).map_err(|e| {
        invocation_failed(format!(
            "journal result file {} could not be read: {e}",
            path.display()
        ))
    })?;
    let narrative = String::from_utf8_lossy(&bytes).trim().to_string();
    if narrative.is_empty() {
        return Err(invocation_failed(format!(
            "journal result file {} was empty; the agent wrote no narrative",
            path.display()
        )));
    }
    Ok(narrative)
}

fn invocation_failed(reason: String) -> SimardError {
    SimardError::AdapterInvocationFailed {
        base_type: ADAPTER_TAG.to_string(),
        reason,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}…")
    }
}

/// Serialize a [`DayContext`] into the JSON payload the draft recipe consumes.
///
/// Episodic memories are pre-sorted oldest-to-newest and carry a human-readable
/// timestamp label, and the prepared-context substance (facts/triggers/
/// procedures) is passed verbatim so the model summarises *what* they were.
fn day_context_json(day: &DayContext) -> String {
    let mut moments: Vec<_> = day.episodes.iter().collect();
    moments.sort_by_key(|e| e.temporal_index);
    let episodes: Vec<_> = moments
        .iter()
        .map(|e| {
            serde_json::json!({
                "time": episode_time_label(e.temporal_index),
                "content": e.content,
            })
        })
        .collect();
    let prs: Vec<_> = day
        .prs
        .iter()
        .map(|p| {
            serde_json::json!({
                "number": p.number,
                "summary": p.plain_summary,
                "outcome": p.outcome,
            })
        })
        .collect();
    let memory_growth = day.memory_growth.map(|m| {
        serde_json::json!({
            "facts_added": m.facts_added,
            "episodes_added": m.episodes_added,
        })
    });
    let payload = serde_json::json!({
        "date": day.date.format("%Y-%m-%d").to_string(),
        "episodes": episodes,
        "prs": prs,
        "goals": day.goals,
        "deploys": day.deploys,
        "overseer_events": day.overseer_events,
        "facts": day.facts,
        "triggers": day.triggers,
        "procedures": day.procedures,
        "notable": day.notable,
        "memory_growth": memory_growth,
    });
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Prompt-first drafter: runs the `journal-narrative` recipe, degrading to the
/// deterministic [`TemplateDrafter`] report on any failure.
pub struct RecipeDrafter {
    recipe: JournalRecipe,
    fallback: TemplateDrafter,
}

impl RecipeDrafter {
    /// Build a recipe drafter for `repo_root`, or `None` when the recipe assets
    /// / runner are unavailable.
    pub fn for_repo(repo_root: &Path) -> Option<Self> {
        JournalRecipe::new(repo_root, JOURNAL_DRAFT_RECIPE, NARRATIVE_OUTPUT_KEY).map(|recipe| {
            Self {
                recipe,
                fallback: TemplateDrafter,
            }
        })
    }
}

impl JournalDrafter for RecipeDrafter {
    fn draft(&self, day: &DayContext) -> String {
        match self.recipe.run(&[("day_context", day_context_json(day))]) {
            Ok(text) => text,
            Err(e) => {
                // Loud, not swallowed: the spawn-failure arm of `run` already
                // recorded a structured diagnosis into the Overseer failure sink;
                // log the degrade at `error` so it is visible in the operator log
                // too. The deterministic fallback is a readable last resort.
                tracing::error!(
                    target: "simard::journal",
                    error = %e,
                    "journal draft recipe failed and was recorded for the Overseer; \
                     degrading to the deterministic report drafter"
                );
                self.fallback.draft(day)
            }
        }
    }
}

/// Prompt-first reviewer: runs the `journal-plain-language` recipe, degrading to the
/// deterministic glossary [`scrub_jargon`] reviewer on any failure.
pub struct RecipeReviewer {
    recipe: JournalRecipe,
}

impl RecipeReviewer {
    /// Build a recipe reviewer for `repo_root`, or `None` when the recipe assets
    /// / runner are unavailable.
    pub fn for_repo(repo_root: &Path) -> Option<Self> {
        JournalRecipe::new(repo_root, JOURNAL_DEJARGON_RECIPE, PLAIN_OUTPUT_KEY)
            .map(|recipe| Self { recipe })
    }
}

impl JournalReviewer for RecipeReviewer {
    fn review(&self, draft: &str) -> String {
        match self.recipe.run(&[("draft", draft.to_string())]) {
            Ok(text) => text,
            Err(e) => {
                // Loud, not swallowed: `run` recorded the structured diagnosis
                // for the Overseer; log the degrade at `error`. The glossary
                // scrubber is the readable last-resort de-jargon pass.
                tracing::error!(
                    target: "simard::journal",
                    error = %e,
                    "journal de-jargon recipe failed and was recorded for the Overseer; \
                     degrading to the glossary reviewer"
                );
                scrub_jargon(draft)
            }
        }
    }
}
