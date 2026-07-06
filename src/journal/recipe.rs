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
//! Robustness first: each pass **degrades per call** to its deterministic
//! equivalent (the [`TemplateDrafter`] report / the glossary
//! [`scrub_jargon`] reviewer) on any failure — missing recipe assets, no agent
//! binary, `recipe-runner-rs` not on `PATH`, a spawn/exit error, or empty
//! output — so a language-model hiccup can never fail or stall a journal tick.
//! The generator's unconditional secret-redaction post-pass runs regardless of
//! which reviewer produced the text, so a credential survives neither path.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};
use crate::journal::generate::{JournalDrafter, JournalReviewer, TemplateDrafter};
use crate::journal::jargon::scrub_jargon;
use crate::journal::providers::episode_time_label;
use crate::journal::types::DayContext;

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
    /// output text (trimmed). Errors on spawn/exit failure, a bad or
    /// `success == false` envelope, or empty output.
    fn run(&self, ctx: &[(&str, String)]) -> SimardResult<String> {
        let mut cmd = Command::new("recipe-runner-rs");
        cmd.arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary);
        for (key, value) in ctx {
            cmd.arg("-c").arg(format!("{key}={value}"));
        }
        let output = cmd
            .output()
            .map_err(|e| invocation_failed(format!("recipe-runner-rs spawn failed: {e}")))?;
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
                tracing::warn!(
                    target: "simard::journal",
                    error = %e,
                    "journal draft recipe failed; using the deterministic report drafter"
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
                tracing::warn!(
                    target: "simard::journal",
                    error = %e,
                    "journal de-jargon recipe failed; using the glossary reviewer"
                );
                scrub_jargon(draft)
            }
        }
    }
}
