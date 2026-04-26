//! LLM-driven multi-step planning for the engineer loop.
//!
//! Provides [`Plan`] and [`PlanStep`] types, [`plan_objective`] (LLM-based
//! planning), and [`execute_plan`] (sequential execution with verification).

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::base_types::BaseTypeTurnInput;
use crate::engineer_loop::{AnalyzedAction, RepoInspection};
use crate::error::{SimardError, SimardResult};
use crate::identity::OperatingMode;
use crate::session_builder::{LlmProvider, SessionBuilder};

/// A single step in an LLM-generated plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub action: AnalyzedAction,
    pub target: String,
    pub expected_outcome: String,
    pub verification_command: String,
}

/// An ordered sequence of [`PlanStep`]s produced by [`plan_objective`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    steps: Vec<PlanStep>,
}

impl Plan {
    pub fn new(steps: Vec<PlanStep>) -> Self {
        Self { steps }
    }
    pub fn steps(&self) -> &[PlanStep] {
        &self.steps
    }
    pub fn len(&self) -> usize {
        self.steps.len()
    }
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Outcome of a single plan step's verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStepResult {
    pub step: PlanStep,
    pub passed: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Result of executing an entire plan via [`execute_plan`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanExecutionResult {
    pub completed: Vec<PlanStepResult>,
    pub stopped_early: bool,
}

const PLANNING_INSTRUCTIONS: &str = include_str!("../../prompt_assets/simard/engineer_planning.md");

fn build_planning_prompt(objective: &str, inspection: &RepoInspection) -> String {
    let files = if inspection.changed_files.is_empty() {
        "none".to_string()
    } else {
        inspection.changed_files.join(", ")
    };
    let dirty = if inspection.worktree_dirty {
        "dirty"
    } else {
        "clean"
    };
    let goals: Vec<&str> = inspection
        .active_goals
        .iter()
        .map(|g| g.title.as_str())
        .collect();
    let goals_list = if goals.is_empty() {
        "none".to_string()
    } else {
        goals.join("; ")
    };

    format!(
        "{}\n\nObjective: {objective}\nBranch: {branch}\nWorktree: {dirty}\n\
         Changed files: {files}\nActive goals: {goals_list}",
        PLANNING_INSTRUCTIONS.trim(),
        objective = objective,
        branch = inspection.branch,
    )
}

/// Attempt to locate a JSON array within a mixed text/JSON response.
///
/// LLMs sometimes wrap valid JSON in markdown fences, prose preambles, or
/// trailing commentary.  This function tries progressively looser extraction
/// strategies:
///   1. Direct parse of the full (trimmed) text.
///   2. Strip markdown ```json … ``` fences.
///   3. Find the first `[` … last `]` substring and parse that.
///
/// Returns `Err(PlanningUnavailable)` only when none of these strategies
/// produce a valid `Vec<PlanStep>`.
fn parse_plan_response(text: &str) -> SimardResult<Plan> {
    let trimmed = text.trim();
    let denoised = strip_log_noise_lines(trimmed);
    let skipped = skip_preamble(denoised);

    // Strategy 1: direct parse (after skipping any non-JSON preamble such as
    // the Copilot SDK adapter dispatch metadata line — see issue #944).
    if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(skipped) {
        return Ok(Plan::new(steps));
    }

    // Strategy 2: strip markdown fences. Re-apply preamble skipping after
    // defencing to handle preamble-then-fence sequences like
    // "Here is the plan:\n```json\n[...]\n```".
    let defenced = strip_markdown_fences(skipped);
    if defenced != skipped {
        let defenced_skipped = skip_preamble(defenced);
        if let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(defenced_skipped) {
            return Ok(Plan::new(steps));
        }
    }

    // Strategy 3: locate outermost JSON array brackets in the preamble-skipped
    // slice. This also recovers responses where an earlier `{` in the preamble
    // would otherwise misanchor strategies 1 and 2.
    if let Some(json_text) = extract_json_array(skipped)
        && let Ok(steps) = serde_json::from_str::<Vec<PlanStep>>(json_text)
    {
        tracing::info!(
            "recovered JSON plan from mixed LLM response ({} bytes of surrounding text stripped)",
            trimmed.len() - json_text.len()
        );
        return Ok(Plan::new(steps));
    }

    Err(SimardError::PlanningUnavailable {
        reason: format!(
            "failed to parse LLM plan response after trying direct, fenced, and bracket-extraction strategies. \
             Response begins with: {:?}",
            &trimmed[..trimmed.len().min(120)]
        ),
    })
}

/// Strip leading lines that are clearly diagnostic log output emitted by the
/// wrapped LLM process (e.g. the amplihack launcher's
/// "Warning: Could not prepare Copilot environment: [Errno 2] ...") before
/// the actual JSON plan response begins. This complements `skip_preamble`
/// which only advances to the first `[`/`{` byte and can misanchor on
/// square brackets inside error messages like `[Errno 2]`. See issue #1175.
///
/// A line is considered noise when it starts (case-insensitively) with any
/// of the well-known log level prefixes. Stops at the first line that does
/// NOT match a noise prefix.
fn strip_log_noise_lines(s: &str) -> &str {
    const NOISE_PREFIXES: &[&str] = &[
        "warning:", "error:", "info:", "debug:", "notice:", "trace:", "warn ", "error ", "info ",
        "debug ",
    ];
    let mut cursor = 0usize;
    for line in s.split_inclusive('\n') {
        let trimmed_line = line.trim_start();
        if trimmed_line.is_empty() {
            cursor += line.len();
            continue;
        }
        let lower = trimmed_line.to_ascii_lowercase();
        let is_noise = NOISE_PREFIXES.iter().any(|p| lower.starts_with(p));
        if is_noise {
            cursor += line.len();
        } else {
            break;
        }
    }
    &s[cursor..]
}

/// Skip any non-JSON preamble at the start of `s` by advancing to the
/// earliest `[` or `{`. Returns the original slice when neither delimiter
/// is present. Used to tolerate adapter dispatch lines (e.g. the Copilot SDK
/// "Copilot SDK adapter dispatched objective-metadata via …" prefix from
/// issue #944) before JSON plan payloads.
fn skip_preamble(s: &str) -> &str {
    let bracket = s.find('[');
    let brace = s.find('{');
    let pos = match (bracket, brace) {
        (Some(b), Some(c)) => b.min(c),
        (Some(b), None) => b,
        (None, Some(c)) => c,
        (None, None) => return s,
    };
    &s[pos..]
}

/// Remove markdown code fences from text. Handles ```json and bare ```.
fn strip_markdown_fences(text: &str) -> &str {
    let inner = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text);
    inner.strip_suffix("```").unwrap_or(inner).trim()
}

/// Find the first `[` and last `]` in the text and return the substring.
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

/// Ask the LLM for a multi-step plan to accomplish `objective`.
///
/// Returns `Err(PlanningUnavailable)` when no LLM session can be opened
/// or the response is unparseable. There is **no keyword fallback** —
/// the cycle must report the planning failure rather than execute a
/// fabricated plan.
pub fn plan_objective(objective: &str, inspection: &RepoInspection) -> SimardResult<Plan> {
    let provider = LlmProvider::resolve()?;
    let mut session = SessionBuilder::new(OperatingMode::Engineer, provider)
        .node_id("engineer-planner")
        .address("engineer-planner://local")
        .adapter_tag("engineer-planner")
        .open()
        .map_err(|reason| SimardError::PlanningUnavailable { reason })?;

    let prompt = build_planning_prompt(objective, inspection);
    let outcome = session
        .run_turn(BaseTypeTurnInput::objective_only(prompt))
        .map_err(|e| SimardError::PlanningUnavailable {
            reason: format!("LLM turn failed: {e}"),
        })?;
    let _ = session.close();
    // `outcome.plan` is adapter-emitted telemetry ("Copilot SDK adapter
    // dispatched ... via 'amplihack copilot' on 'single-process' (turn N)").
    // The actual LLM response text lives in `outcome.execution_summary`.
    // Reading the wrong field here meant the planner parsed the telemetry
    // string, failed, and silently degraded to keyword analysis (issue #1062).
    parse_plan_response(&outcome.execution_summary)
}

/// Execute a plan sequentially, running each step's verification command.
/// Stops on the first verification failure and returns partial results.
pub fn execute_plan(plan: &Plan, repo_root: &Path) -> PlanExecutionResult {
    let mut completed = Vec::with_capacity(plan.len());
    let mut stopped_early = false;

    for step in plan.steps() {
        if step.verification_command.is_empty() {
            completed.push(PlanStepResult {
                step: step.clone(),
                passed: true,
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
            continue;
        }
        match Command::new("sh")
            .arg("-c")
            .arg(&step.verification_command)
            .current_dir(repo_root)
            .output()
        {
            Ok(out) => {
                let exit_code = out.status.code().unwrap_or(-1);
                let passed = exit_code == 0;
                completed.push(PlanStepResult {
                    step: step.clone(),
                    passed,
                    exit_code,
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                });
                if !passed {
                    stopped_early = true;
                    break;
                }
            }
            Err(e) => {
                completed.push(PlanStepResult {
                    step: step.clone(),
                    passed: false,
                    exit_code: -1,
                    stdout: String::new(),
                    stderr: format!("failed to run verification command: {e}"),
                });
                stopped_early = true;
                break;
            }
        }
    }
    PlanExecutionResult {
        completed,
        stopped_early,
    }
}

#[cfg(test)]
mod tests;
