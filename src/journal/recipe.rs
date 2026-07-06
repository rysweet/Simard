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

use serde::Deserialize;

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

/// Resolve a journal recipe path, hot-reload location first then in-tree:
///   1. `<home>/.simard/prompt_assets/simard/recipes/<filename>`
///   2. `<repo_root>/prompt_assets/simard/recipes/<filename>`
///
/// Returns `None` when neither exists (the caller then uses the deterministic
/// fallback).
fn resolve_recipe_path(repo_root: &Path, filename: &str) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(filename);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(filename);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// The JSON envelope `recipe-runner-rs --output-format json` prints.
#[derive(Debug, Deserialize)]
struct RecipeEnvelope {
    success: bool,
    step_results: Vec<RecipeStepResult>,
}

/// A single step's result inside the [`RecipeEnvelope`].
#[derive(Debug, Deserialize)]
struct RecipeStepResult {
    output: String,
}

/// A resolved journal recipe plus the agent binary to run it with.
struct JournalRecipe {
    recipe_path: PathBuf,
    agent_binary: &'static str,
}

impl JournalRecipe {
    /// Construct a runner when all preconditions hold: the recipe file resolves,
    /// an agent binary is configured, and `recipe-runner-rs` is on `PATH`.
    /// Returns `None` otherwise (the caller falls back to deterministic).
    fn new(repo_root: &Path, filename: &str) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, filename)?;
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
        })
    }

    /// Run the recipe with the given context vars and return the final step's
    /// output text (trimmed). Every context value is *unbounded*, so each is
    /// delivered through the shared file channel ([`ContextFile`]): the payload
    /// goes to a private temp file and only the short `<key>_path=<abs>` rides on
    /// argv, so a full day's context can never overflow `ARG_MAX` and fail the
    /// spawn with `E2BIG` (issues #2640/#2692). Errors on spawn/exit failure, a
    /// bad or `success == false` envelope, or empty output.
    ///
    /// A pre-exec spawn failure (the E2BIG defect and its siblings) is an
    /// `io::Error` with no child, so it is classified and recorded into the
    /// Overseer's failure sink here — never swallowed — before it propagates.
    fn run(&self, ctx: &[(&str, String)]) -> SimardResult<String> {
        let mut cmd = Command::new("recipe-runner-rs");
        cmd.arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary);
        // Route every context var through the file channel; the guards must
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
        let output = cmd.output().map_err(|e| {
            // Pre-exec spawn failure (E2BIG / ENOSPC / ENOMEM / …). Record a
            // structured diagnosis for the Overseer to act on — the "no silent
            // fallback" invariant — then surface the error to the caller.
            record_step_failure(classify_spawn_failure(&e));
            invocation_failed(format!("recipe-runner-rs spawn failed: {e}"))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(invocation_failed(format!(
                "recipe exited with {}: {}",
                output.status,
                truncate(stderr.trim(), 200)
            )));
        }
        let envelope: RecipeEnvelope = serde_json::from_slice(&output.stdout)
            .map_err(|e| invocation_failed(format!("bad JSON envelope: {e}")))?;
        if !envelope.success {
            return Err(invocation_failed(
                "recipe reported success=false".to_string(),
            ));
        }
        envelope
            .step_results
            .last()
            .map(|s| s.output.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invocation_failed("recipe produced no output".to_string()))
    }
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
        JournalRecipe::new(repo_root, JOURNAL_DRAFT_RECIPE).map(|recipe| Self {
            recipe,
            fallback: TemplateDrafter,
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
        JournalRecipe::new(repo_root, JOURNAL_DEJARGON_RECIPE).map(|recipe| Self { recipe })
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
