//! Thin Rust rail for the agentic **Overseer health-review** ([standing]).
//!
//! Simard reviews her OWN process health with a DETERMINISTIC TICK OF AGENTIC
//! STEPS + PROMPTS — not a Rust "failure counter." This module is the only new
//! Rust the feature needs, and it is deliberately thin:
//!
//! 1. [`HealthReviewer`] is the rail seam: one call returns a
//!    [`HealthReviewOutcome`] — the typed [`Intervention`]s the recipe reasoned
//!    to (a `LaunchRecipe` for a systemic fix, an `EscalateBlockedGoal` for a
//!    per-goal escalation), plus the pass's one-line verdict `summary` — which
//!    the acting Overseer gates + dispatches through its EXISTING capabilities
//!    and surfaces on `ObservedState.health_review_status` (so a HEALTHY pass is
//!    an observable "reviewed, nothing to do", never a silent no-op).
//! 2. [`parse_health_review_output`] parses the recipe's plain-text DECISION
//!    markers into those interventions — a mechanical rail, not judgment.
//! 3. [`RecipeHealthReviewer`] invokes the `overseer-health-review` recipe
//!    through an injectable [`HealthReviewRecipeRunner`] seam.
//!
//! What this module deliberately does NOT do (the retired anti-pattern): it never
//! reads a journal, never counts failures, never encodes an N-identical-failure
//! threshold, and never wires `record_step_failure` into a failure-origin site.
//! The journal already contains every failure regardless of which module raised
//! it, and the AGENT reading it sees them all — exactly as the operator diagnosed
//! the 286x actor-binding crash-loop by hand in a handful of journal reads. The
//! observation and the remediation JUDGMENT live entirely in the recipe's agent
//! step; the rail only schedules the tick and dispatches the typed decisions.
//! See `docs/concepts/overseer-agentic-health-review.md`.
//!
//! ## Degraded-pass recovery: the shared escalation ladder
//!
//! A single agent pass can occasionally emit a truncated/malformed report that
//! lacks the REQUIRED `HEALTH_REVIEW_COMPLETE` terminal marker. Rather than
//! silently degrade that weak case to "no remediation" on the FIRST miss, the
//! rail spends EXTRA compute only there: on a base parse-miss it drives a
//! bounded escalation ladder — a schema-repair re-prompt, then a higher-effort
//! tier — reusing the SAME composable primitives every other recipe-backed brain
//! phase uses ([`build_phase_escalation_note`](crate::ooda_brain::build_phase_escalation_note),
//! [`EscalationConfig`](crate::ooda_brain::EscalationConfig) with its shared
//! `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS` knob + hard cap, and
//! [`LadderRung`](crate::ooda_brain::LadderRung)). The ladder is a bounded RETRY
//! on a degraded parse — NOT a failure counter and NOT an N-identical-failure
//! threshold; the health JUDGMENT still lives entirely in the recipe. It is
//! fail-closed end to end: a base runner/infra fault still degrades with NO
//! ladder (the base pass must succeed before it can be judged degraded), a
//! rung's own invocation fault stops the ladder, and an exhausted ladder takes
//! no remediation — never a fabricated launch or escalation.

use std::path::Path;

use serde::Deserialize;

use crate::error::{SimardError, SimardResult};
use crate::ooda_brain::{EscalationConfig, LadderRung, build_phase_escalation_note};
use crate::overseer::capabilities::RecipeBrief;
use crate::overseer::intervention::Intervention;

/// Adapter tag for error/telemetry attribution on the health-review rail.
const HEALTH_REVIEW_ADAPTER_TAG: &str = "overseer-health-review";
/// The recipe this rail invokes (resolved hot-reload-first, then in-tree).
const HEALTH_REVIEW_RECIPE_FILENAME: &str = "overseer-health-review.yaml";

/// The default `reason` marker attached to an escalation whose JSON omitted one.
/// Internal telemetry only — never the operator-facing text.
const DEFAULT_ESCALATE_REASON: &str = "health-review";
/// The default `target_repo` for a systemic-fix launch (process health).
const DEFAULT_TARGET_REPO: &str = "rysweet/Simard";

// ─────────────────────────── rail request ──────────────────────────────────

/// What the rail hands the `overseer-health-review` recipe: the systemd `--user`
/// unit whose journal to read, the state root, the repo root, and the
/// (rail-owned) escalation note. All bounded strings — the recipe reads the
/// (unbounded) journal/status/goal-list ITSELF with its bash tool, so nothing
/// large ever rides `argv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthReviewRequest {
    /// The systemd `--user` unit whose journal the agent reads
    /// (`simard-ooda.service`).
    pub service_unit: String,
    /// Path to `~/.simard` (telemetry / daemon logs live here).
    pub state_root: String,
    /// Repository root of the local `rysweet/Simard` checkout.
    pub repo_path: String,
    /// Empty on the base pass; carries a higher-effort / repair instruction on
    /// escalation-ladder retries. Rail-owned, never a caller parameter.
    pub escalation_note: String,
}

/// Seam: invoke the `overseer-health-review` recipe and return its final opaque
/// step output. Injectable so the rail is unit-testable with a fake — no
/// subprocess, no journal, no `simard`. The production impl spawns the recipe
/// runner; only [`parse_health_review_output`] (a MECHANICAL marker parser)
/// interprets the returned string.
pub trait HealthReviewRecipeRunner: Send + Sync {
    /// Run one health-review pass and return the recipe's final step output.
    fn run(&self, request: &HealthReviewRequest) -> SimardResult<String>;
}

/// The observable OUTCOME of one health-review pass: the typed remediation
/// [`Intervention`]s to gate + dispatch, PLUS the agent's one-line
/// `HEALTH_REVIEW_COMPLETE=<summary>` verdict.
///
/// `summary` is `Some(_)` exactly when a pass produced an HONEST verdict — a
/// clean base parse OR a rung the bounded escalation ladder recovered — so a
/// HEALTHY pass (zero interventions) still leaves an OBSERVABLE trace instead of
/// degrading to a silent no-op. It is `None` when the pass DEGRADED end to end
/// (a base infra fault, a truncated report the ladder could not recover, a
/// disabled ladder, or a rung fault), so the rail surfaces the weak pass LOUD
/// and never fabricates a verdict. Mirrors how `merge_reasoning_status` surfaces
/// WHY reasoning ran rather than leaving a silent gap (#4097).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthReviewOutcome {
    /// Zero or more typed interventions to gate + dispatch. An empty vec with a
    /// `Some` summary is a HEALTHY pass (nothing to do) — never fabricated work.
    pub interventions: Vec<Intervention>,
    /// The recipe's `HEALTH_REVIEW_COMPLETE=<summary>` verdict when the pass
    /// parsed; `None` on a degraded pass (no honest verdict to surface).
    pub summary: Option<String>,
}

/// The rail: run one health-review pass and return the typed remediation
/// [`Intervention`]s the recipe reasoned to, plus its verdict summary. Holds NO
/// health state and reads no journal — the observation and the judgment both
/// live inside the recipe.
pub trait HealthReviewer {
    /// Run one health-review pass.
    ///
    /// - `Ok(outcome)` — the parsed interventions (`LaunchRecipe` /
    ///   `EscalateBlockedGoal`) to gate + dispatch, plus the verdict `summary`.
    ///   An empty `interventions` with `summary = Some(_)` is `HEALTHY` (nothing
    ///   to do this tick) — never fabricated work; a `summary = None` marks a
    ///   pass that degraded to no remediation.
    /// - `Err(_)` — reserved for a caller-visible fault; the default rail is
    ///   fail-closed and prefers a degraded `Ok(_)` over fabricating a
    ///   remediation.
    fn review(&self) -> SimardResult<HealthReviewOutcome>;
}

// ─────────────────────────── decision markers ──────────────────────────────

/// JSON payload of a `LAUNCH_RECIPE=<json>` decision marker.
#[derive(Debug, Deserialize)]
struct LaunchDecision {
    task_description: String,
    #[serde(default = "default_target_repo")]
    target_repo: String,
    #[serde(default)]
    sequence_group: Option<String>,
}

/// JSON payload of an `ESCALATE_GOAL=<json>` decision marker.
#[derive(Debug, Deserialize)]
struct EscalateDecision {
    goal_id: String,
    problem: String,
    next_step: String,
    #[serde(default)]
    why: String,
    #[serde(default = "default_escalate_reason")]
    reason: String,
    #[serde(default)]
    link: Option<String>,
}

fn default_target_repo() -> String {
    DEFAULT_TARGET_REPO.to_string()
}

fn default_escalate_reason() -> String {
    DEFAULT_ESCALATE_REASON.to_string()
}

/// The parsed outcome of one health-review pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthReviewReport {
    /// The typed remediation interventions to gate + dispatch, in emission order.
    pub interventions: Vec<Intervention>,
    /// The recipe's one-line `HEALTH_REVIEW_COMPLETE=<summary>` text.
    pub summary: String,
}

/// Parse the recipe's plain-text DECISION markers into typed interventions.
///
/// This is the MECHANICAL rail: it moves the recipe's typed DECISIONS onto the
/// capability path; it re-derives no judgment. Recognised markers, one per line:
///
/// ```text
/// HEALTHY
/// LAUNCH_RECIPE={"task_description":"…","target_repo":"rysweet/Simard","sequence_group":null}
/// ESCALATE_GOAL={"goal_id":"…","problem":"…","next_step":"…","why":"…","reason":"…","link":null}
/// HEALTH_REVIEW_COMPLETE=<one-line summary>
/// ```
///
/// Fail-closed and forward-compatible:
/// - The terminal `HEALTH_REVIEW_COMPLETE=` marker is REQUIRED (mirrors
///   disk-health's required marker). Its absence is an `Err` — the caller
///   degrades to "no remediation" rather than acting on a truncated pass.
/// - A `LAUNCH_RECIPE=` / `ESCALATE_GOAL=` line whose JSON is malformed or whose
///   required plain-English fields are empty is SKIPPED with a warning — never a
///   fabricated intervention with missing text. Other decisions on the pass
///   still apply.
/// - Unknown lines are ignored (a `HEALTHY` line carries no payload).
/// - Benign markdown decoration an agent commonly wraps a decision line in — a
///   leading `-`/`*`/`+`/`N.`/`N)` list bullet, a `>` blockquote caret, or
///   surrounding inline-code backticks — is stripped BEFORE marker matching
///   ([`strip_marker_decoration`]) so a well-formed decision is DISPATCHED
///   rather than silently dropped. This never invents a marker: only the three
///   distinctive markers are ever acted on, and a bulleted line of prose still
///   matches nothing.
pub fn parse_health_review_output(stdout: &str) -> Result<HealthReviewReport, String> {
    let mut interventions: Vec<Intervention> = Vec::new();
    let mut summary: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = strip_marker_decoration(line);
        if trimmed.is_empty() {
            continue;
        }
        if let Some(payload) = trimmed.strip_prefix("HEALTH_REVIEW_COMPLETE=") {
            // The LAST terminal marker wins (an agent may echo intermediate
            // progress); keep the most recent non-empty summary.
            let s = payload.trim();
            if !s.is_empty() {
                summary = Some(s.to_string());
            }
        } else if let Some(iv) = trimmed
            .strip_prefix("LAUNCH_RECIPE=")
            .and_then(|p| parse_launch_decision(p.trim()))
        {
            // A malformed decision is skipped-with-warning inside the parser.
            interventions.push(iv);
        } else if let Some(iv) = trimmed
            .strip_prefix("ESCALATE_GOAL=")
            .and_then(|p| parse_escalate_decision(p.trim()))
        {
            interventions.push(iv);
        }
        // Bare `HEALTHY` and any other line: ignored (forward-compat).
    }

    let summary =
        summary.ok_or_else(|| "missing HEALTH_REVIEW_COMPLETE terminal marker".to_string())?;

    Ok(HealthReviewReport {
        interventions,
        summary,
    })
}

/// Strip benign markdown decoration an agent commonly wraps a single decision
/// line in, so a well-formed marker survives ordinary formatting instead of
/// being silently dropped by the strict prefix match. Removes, in order:
///
/// 1. any leading `>` blockquote carets (each optionally space-padded),
/// 2. a SINGLE leading list bullet — `-`/`*`/`+` or an ordered `N.`/`N)` — that
///    is followed by whitespace (so a genuine `-`-prefixed marker is never
///    mistaken as content, and prose bullets simply match no marker), and
/// 3. one layer of surrounding inline-code backticks (```` ``` ```` or `` ` ``).
///
/// It is decoration-only and fail-closed: the returned slice is still matched
/// against the three DISTINCTIVE markers, so this can never invent a decision —
/// a decorated line of prose normalises to prose and matches nothing.
fn strip_marker_decoration(line: &str) -> &str {
    let mut s = line.trim();
    // 1. Leading blockquote carets, possibly repeated (`> > `).
    loop {
        let t = s.trim_start();
        match t.strip_prefix('>') {
            Some(rest) => s = rest,
            None => {
                s = t;
                break;
            }
        }
    }
    // 2. A single leading list bullet (unordered or ordered).
    s = strip_leading_bullet(s.trim_start()).unwrap_or_else(|| s.trim_start());
    // 3. One layer of surrounding inline-code backticks (triple before single).
    s = s.trim();
    for fence in ["```", "`"] {
        if let Some(inner) = s.strip_prefix(fence) {
            s = inner.strip_suffix(fence).unwrap_or(inner);
            break;
        }
    }
    s.trim()
}

/// Strip a SINGLE leading list bullet — unordered (`-`/`*`/`+`) or ordered
/// (`N.`/`N)`) — plus its trailing whitespace, or `None` when `s` is not
/// bulleted. The trailing-whitespace requirement keeps a bare marker (or prose)
/// that merely starts with a bullet character from being mis-stripped.
fn strip_leading_bullet(s: &str) -> Option<&str> {
    for bullet in ['-', '*', '+'] {
        if let Some(rest) = s.strip_prefix(bullet)
            && rest.starts_with(char::is_whitespace)
        {
            return Some(rest.trim_start());
        }
    }
    let digit_len = s.chars().take_while(char::is_ascii_digit).count();
    if digit_len > 0 {
        let after = &s[digit_len..];
        if let Some(rest) = after.strip_prefix('.').or_else(|| after.strip_prefix(')'))
            && rest.starts_with(char::is_whitespace)
        {
            return Some(rest.trim_start());
        }
    }
    None
}

/// Parse one `LAUNCH_RECIPE=` JSON payload into an [`Intervention::LaunchRecipe`],
/// or `None` (logged) when it is malformed or its `task_description` is empty.
fn parse_launch_decision(json: &str) -> Option<Intervention> {
    let decision: LaunchDecision = match serde_json::from_str(json) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                target: "overseer::health_review",
                error = %e,
                "health-review: skipping a malformed LAUNCH_RECIPE decision (fabricating nothing)"
            );
            return None;
        }
    };
    if decision.task_description.trim().is_empty() {
        tracing::warn!(
            target: "overseer::health_review",
            "health-review: skipping a LAUNCH_RECIPE decision with an empty task_description"
        );
        return None;
    }
    let target_repo = if decision.target_repo.trim().is_empty() {
        DEFAULT_TARGET_REPO.to_string()
    } else {
        decision.target_repo
    };
    let sequence_group = decision.sequence_group.filter(|g| !g.trim().is_empty());
    Some(Intervention::LaunchRecipe {
        brief: RecipeBrief {
            task_description: decision.task_description,
            target_repo,
            sequence_group,
        },
    })
}

/// Parse one `ESCALATE_GOAL=` JSON payload into an
/// [`Intervention::EscalateBlockedGoal`], or `None` (logged) when it is
/// malformed or a required plain-English field is empty. An escalation with an
/// empty `goal_id`/`problem`/`next_step` would surface a meaningless message to
/// the operator, so it is dropped fail-closed.
fn parse_escalate_decision(json: &str) -> Option<Intervention> {
    let decision: EscalateDecision = match serde_json::from_str(json) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(
                target: "overseer::health_review",
                error = %e,
                "health-review: skipping a malformed ESCALATE_GOAL decision (fabricating nothing)"
            );
            return None;
        }
    };
    if decision.goal_id.trim().is_empty()
        || decision.problem.trim().is_empty()
        || decision.next_step.trim().is_empty()
    {
        tracing::warn!(
            target: "overseer::health_review",
            goal_id = %decision.goal_id,
            "health-review: skipping an ESCALATE_GOAL decision missing goal_id/problem/next_step"
        );
        return None;
    }
    let reason = if decision.reason.trim().is_empty() {
        DEFAULT_ESCALATE_REASON.to_string()
    } else {
        decision.reason
    };
    let link = decision.link.filter(|l| !l.trim().is_empty());
    Some(Intervention::EscalateBlockedGoal {
        goal_id: decision.goal_id,
        reason,
        why: decision.why,
        problem: decision.problem,
        next_step: decision.next_step,
        link,
    })
}

// ─────────────────────────── recipe-backed rail ────────────────────────────

/// Recipe-runner-backed [`HealthReviewer`] over an injectable seam.
pub struct RecipeHealthReviewer<R: HealthReviewRecipeRunner> {
    runner: R,
    service_unit: String,
    state_root: String,
    repo_path: String,
    /// Bound on the degraded-pass escalation ladder. Reuses the SHARED brain
    /// ladder config (`SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS`, hard-capped);
    /// `max_escalations == 0` disables the ladder (byte-identical to a single
    /// base pass).
    escalation: EscalationConfig,
}

impl<R: HealthReviewRecipeRunner> RecipeHealthReviewer<R> {
    /// Build the rail over a concrete [`HealthReviewRecipeRunner`], pinning the
    /// bounded context vars the recipe substitutes. The degraded-pass escalation
    /// ladder is bounded by the shared [`EscalationConfig::from_env`] (the same
    /// `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS` knob every brain phase reads).
    pub fn new(runner: R, service_unit: String, state_root: String, repo_path: String) -> Self {
        Self {
            runner,
            service_unit,
            state_root,
            repo_path,
            escalation: EscalationConfig::from_env(),
        }
    }

    /// Override the escalation-ladder bound (used by tests to drive the ladder
    /// deterministically without env mutation).
    #[cfg(test)]
    pub fn with_escalation_config(mut self, escalation: EscalationConfig) -> Self {
        self.escalation = escalation;
        self
    }

    /// Borrow the underlying runner (used by tests to inspect the seam).
    pub fn runner(&self) -> &R {
        &self.runner
    }

    /// Invoke the recipe once for a ladder rung with the given `escalation_note`
    /// (empty on the base pass). All context vars stay bounded — the recipe reads
    /// the unbounded journal/status/goal-list ITSELF.
    fn run_pass(&self, escalation_note: &str) -> SimardResult<String> {
        let request = HealthReviewRequest {
            service_unit: self.service_unit.clone(),
            state_root: self.state_root.clone(),
            repo_path: self.repo_path.clone(),
            escalation_note: escalation_note.to_string(),
        };
        self.runner.run(&request)
    }

    /// Drive the bounded escalation ladder after a BASE parse-miss (a degraded
    /// pass missing the required terminal marker). Reuses the shared
    /// [`build_phase_escalation_note`] / [`EscalationConfig`] / [`LadderRung`]
    /// primitives — a schema-repair re-prompt, then a higher-effort tier — and
    /// returns the recovered [`HealthReviewOutcome`] (interventions + verdict
    /// summary), or a DEGRADED outcome (`summary = None`, no interventions) when
    /// the ladder is disabled, a rung's own invocation faults, or every rung is
    /// exhausted. Never fabricates work and never fabricates a verdict.
    fn escalate_after_parse_miss(
        &self,
        base_output: &str,
        base_reason: &str,
    ) -> SimardResult<HealthReviewOutcome> {
        let max = self.escalation.max_escalations;
        if max == 0 {
            tracing::warn!(
                target: "overseer::health_review",
                reason = %base_reason,
                "health-review: degraded base pass and escalation ladder disabled; taking no remediation (fabricating nothing)"
            );
            return Ok(HealthReviewOutcome::default());
        }

        // Feed each rung the latest malformed output so the schema-repair note
        // quotes what actually came back.
        let mut prior = base_output.to_string();
        for rung_idx in 1..=max {
            let rung = if rung_idx == 1 {
                LadderRung::SchemaRepair
            } else {
                LadderRung::Escalate
            };
            let note = build_health_review_escalation_note(rung, &prior);
            tracing::warn!(
                target: "overseer::health_review",
                rung = ?rung,
                attempt = rung_idx + 1,
                reason = %base_reason,
                "health-review: degraded pass → escalating (bounded retry ladder)"
            );
            match self.run_pass(&note) {
                Err(e) => {
                    // A rung's own invocation faulted — stop the ladder and
                    // degrade to no remediation (never fabricate on a fault).
                    tracing::warn!(
                        target: "overseer::health_review",
                        rung = ?rung,
                        error = %e,
                        "health-review: escalation rung failed to invoke; stopping ladder, taking no remediation"
                    );
                    return Ok(HealthReviewOutcome::default());
                }
                Ok(output) => match parse_health_review_output(&output) {
                    Ok(report) => {
                        tracing::info!(
                            target: "overseer::health_review",
                            rung = ?rung,
                            attempt = rung_idx + 1,
                            decisions = report.interventions.len(),
                            summary = %report.summary,
                            "health-review: RECOVERED a degraded pass via the escalation ladder"
                        );
                        return Ok(HealthReviewOutcome {
                            interventions: report.interventions,
                            summary: Some(report.summary),
                        });
                    }
                    // Still degraded — carry the latest output into the next rung.
                    Err(_) => prior = output,
                },
            }
        }

        tracing::warn!(
            target: "overseer::health_review",
            attempts = max + 1,
            reason = %base_reason,
            "health-review: escalation ladder exhausted with no parseable pass; taking no remediation (fabricating nothing)"
        );
        Ok(HealthReviewOutcome::default())
    }
}

/// Build the health-review `escalation_note` for a ladder rung, reminding the
/// agent of the REQUIRED terminal-marker contract (the reason a pass degrades).
/// Empty on [`LadderRung::Base`] so the base pass is byte-identical to a plain
/// single invocation.
fn build_health_review_escalation_note(rung: LadderRung, prior_output: &str) -> String {
    build_phase_escalation_note(
        rung,
        prior_output,
        "Re-run the health review and emit the typed DECISION markers as PLAIN TEXT \
         (no code fences), then END with EXACTLY one non-empty terminal line \
         `HEALTH_REVIEW_COMPLETE=<one-line summary>`. If nothing is wrong, emit `HEALTHY` \
         then that terminal marker.",
        "Re-read the journal / `simard status` / `simard goal list` carefully BEFORE deciding, \
         then emit the decision markers followed by the required HEALTH_REVIEW_COMPLETE terminal line.",
    )
}

impl<R: HealthReviewRecipeRunner> HealthReviewer for RecipeHealthReviewer<R> {
    fn review(&self) -> SimardResult<HealthReviewOutcome> {
        // Base pass — empty escalation note (byte-identical to pre-ladder).
        let output = match self.run_pass("") {
            Ok(output) => output,
            Err(e) => {
                // Fail-closed: a base recipe/infra fault degrades to "no
                // remediation", logged. It never aborts the tick, never
                // fabricates work, and does NOT enter the ladder — the base pass
                // must succeed before it can be judged merely degraded. The
                // outcome carries NO verdict summary so the rail surfaces the
                // degraded pass LOUD instead of a silent no-op.
                tracing::warn!(
                    target: "overseer::health_review",
                    error = %e,
                    "health-review: recipe run failed; degrading to no remediation (fabricating nothing)"
                );
                return Ok(HealthReviewOutcome::default());
            }
        };

        match parse_health_review_output(&output) {
            Ok(report) => {
                tracing::info!(
                    target: "overseer::health_review",
                    decisions = report.interventions.len(),
                    summary = %report.summary,
                    "health-review pass parsed"
                );
                Ok(HealthReviewOutcome {
                    interventions: report.interventions,
                    summary: Some(report.summary),
                })
            }
            Err(reason) => {
                // A degraded base pass (missing terminal marker): spend extra
                // compute ONLY on this weak case via the bounded escalation
                // ladder before giving up — never a silent degrade-on-first-miss.
                self.escalate_after_parse_miss(&output, &reason)
            }
        }
    }
}

// ─────────────────── production recipe-runner (thin) ───────────────────────

/// Resolve the `overseer-health-review.yaml` recipe path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<name>` (hot-reload path)
///   2. `<repo_root>/prompt_assets/simard/recipes/<name>` (in-tree)
///
/// Mirrors `ecosystem_observe::resolve_observe_recipe_path`. `home_override`
/// keeps tests hermetic against the ambient `~/.simard`; production passes `None`.
fn resolve_health_review_recipe_path(
    repo_root: &Path,
    home_override: Option<&Path>,
) -> Option<std::path::PathBuf> {
    let home = home_override
        .map(std::path::PathBuf::from)
        .or_else(dirs::home_dir);
    if let Some(home) = home {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(HEALTH_REVIEW_RECIPE_FILENAME);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(HEALTH_REVIEW_RECIPE_FILENAME);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Production [`HealthReviewRecipeRunner`]: spawns `recipe-runner-rs` on the
/// `overseer-health-review` recipe and returns its OPAQUE final-step output.
///
/// Thin by construction. It passes only the bounded context vars on `argv` (the
/// recipe reads the unbounded journal/status/goal-list ITSELF with its bash
/// tool), runs the recipe in `--output-format json`, and hands back the
/// envelope's final step output via
/// [`extract_recipe_decision_output`](crate::ooda_brain::extract_recipe_decision_output).
/// Only [`parse_health_review_output`] interprets that string.
pub struct SpawnHealthReviewRecipeRunner {
    recipe_path: std::path::PathBuf,
    agent_binary: &'static str,
}

impl SpawnHealthReviewRecipeRunner {
    /// Construct if the recipe file and `recipe-runner-rs` are both available;
    /// otherwise `None` (the rail is left unwired and the pass is skipped).
    pub fn new(repo_root: &Path) -> Option<Self> {
        Self::new_with_home(repo_root, None)
    }

    fn new_with_home(repo_root: &Path, home_override: Option<&Path>) -> Option<Self> {
        let recipe_path = resolve_health_review_recipe_path(repo_root, home_override)?;
        let agent_binary = crate::session_builder::LlmProvider::resolve_agent_binary()?;
        if std::process::Command::new("recipe-runner-rs")
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

impl HealthReviewRecipeRunner for SpawnHealthReviewRecipeRunner {
    fn run(&self, request: &HealthReviewRequest) -> SimardResult<String> {
        let output = std::process::Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("service_unit={}", request.service_unit))
            .arg("-c")
            .arg(format!("state_root={}", request.state_root))
            .arg("-c")
            .arg(format!("repo_path={}", request.repo_path))
            .arg("-c")
            .arg(format!("escalation_note={}", request.escalation_note))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: HEALTH_REVIEW_ADAPTER_TAG.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated: String = stderr.chars().take(500).collect();
            return Err(SimardError::AdapterInvocationFailed {
                base_type: HEALTH_REVIEW_ADAPTER_TAG.to_string(),
                reason: format!("recipe exited with {}: {}", output.status, truncated),
            });
        }

        crate::ooda_brain::extract_recipe_decision_output(&output.stdout, HEALTH_REVIEW_ADAPTER_TAG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── marker parser ────────────────────────────────────────────────────

    #[test]
    fn parse_healthy_pass_yields_no_interventions() {
        let out = "HEALTHY\nHEALTH_REVIEW_COMPLETE=healthy\n";
        let report = parse_health_review_output(out).expect("parses");
        assert!(report.interventions.is_empty());
        assert_eq!(report.summary, "healthy");
    }

    #[test]
    fn parse_missing_terminal_marker_is_error() {
        // A truncated pass (no terminal marker) must be an Err so the caller
        // degrades to no remediation rather than acting on a partial pass.
        let out = "HEALTHY\n";
        assert!(parse_health_review_output(out).is_err());
    }

    #[test]
    fn parse_launch_recipe_decision() {
        let out = concat!(
            r#"LAUNCH_RECIPE={"task_description":"fix actor-binding crash-loop (286x) in typed_ooda","target_repo":"rysweet/Simard","sequence_group":"ooda-core"}"#,
            "\nHEALTH_REVIEW_COMPLETE=1 systemic launch\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert_eq!(report.interventions.len(), 1);
        match &report.interventions[0] {
            Intervention::LaunchRecipe { brief } => {
                assert!(brief.task_description.contains("actor-binding"));
                assert_eq!(brief.target_repo, "rysweet/Simard");
                assert_eq!(brief.sequence_group.as_deref(), Some("ooda-core"));
            }
            other => panic!("expected LaunchRecipe, got {other:?}"),
        }
    }

    #[test]
    fn parse_launch_recipe_defaults_target_repo_and_null_sequence_group() {
        let out = concat!(
            r#"LAUNCH_RECIPE={"task_description":"bound the distillation parse-failure spike"}"#,
            "\nHEALTH_REVIEW_COMPLETE=x\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        match &report.interventions[0] {
            Intervention::LaunchRecipe { brief } => {
                assert_eq!(brief.target_repo, "rysweet/Simard");
                assert_eq!(brief.sequence_group, None);
            }
            other => panic!("expected LaunchRecipe, got {other:?}"),
        }
    }

    #[test]
    fn parse_escalate_goal_decision() {
        let out = concat!(
            r#"ESCALATE_GOAL={"goal_id":"g-42","problem":"Goal is stuck with no finish condition.","next_step":"Give it a testable done-gate.","why":"UNCLEAR-CRITERIA","reason":"health-review:no-progress","link":null}"#,
            "\nHEALTH_REVIEW_COMPLETE=1 goal escalated\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert_eq!(report.interventions.len(), 1);
        match &report.interventions[0] {
            Intervention::EscalateBlockedGoal {
                goal_id,
                reason,
                why,
                problem,
                next_step,
                link,
            } => {
                assert_eq!(goal_id, "g-42");
                assert_eq!(reason, "health-review:no-progress");
                assert_eq!(why, "UNCLEAR-CRITERIA");
                assert!(problem.starts_with("Goal is stuck"));
                assert!(next_step.starts_with("Give it"));
                assert_eq!(link.as_deref(), None);
            }
            other => panic!("expected EscalateBlockedGoal, got {other:?}"),
        }
    }

    #[test]
    fn parse_escalate_goal_defaults_reason_when_omitted() {
        let out = concat!(
            r#"ESCALATE_GOAL={"goal_id":"g-7","problem":"blocked","next_step":"decide"}"#,
            "\nHEALTH_REVIEW_COMPLETE=x\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        match &report.interventions[0] {
            Intervention::EscalateBlockedGoal { reason, why, .. } => {
                assert_eq!(reason, "health-review");
                assert_eq!(why, "");
            }
            other => panic!("expected EscalateBlockedGoal, got {other:?}"),
        }
    }

    #[test]
    fn parse_mixed_decisions_preserves_order() {
        let out = concat!(
            r#"LAUNCH_RECIPE={"task_description":"systemic fix A"}"#,
            "\n",
            r#"ESCALATE_GOAL={"goal_id":"g-1","problem":"p","next_step":"n"}"#,
            "\n",
            r#"LAUNCH_RECIPE={"task_description":"systemic fix B"}"#,
            "\nHEALTH_REVIEW_COMPLETE=2 launches, 1 escalation\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert_eq!(report.interventions.len(), 3);
        assert!(matches!(
            report.interventions[0],
            Intervention::LaunchRecipe { .. }
        ));
        assert!(matches!(
            report.interventions[1],
            Intervention::EscalateBlockedGoal { .. }
        ));
        assert!(matches!(
            report.interventions[2],
            Intervention::LaunchRecipe { .. }
        ));
    }

    #[test]
    fn parse_skips_malformed_json_but_keeps_valid_decisions() {
        let out = concat!(
            "LAUNCH_RECIPE={not valid json}\n",
            r#"ESCALATE_GOAL={"goal_id":"g-9","problem":"p","next_step":"n"}"#,
            "\nHEALTH_REVIEW_COMPLETE=1 escalation (1 decision dropped)\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        // The malformed launch is skipped; the valid escalation survives.
        assert_eq!(report.interventions.len(), 1);
        assert!(matches!(
            report.interventions[0],
            Intervention::EscalateBlockedGoal { .. }
        ));
    }

    #[test]
    fn parse_skips_launch_with_empty_task_description() {
        let out = concat!(
            r#"LAUNCH_RECIPE={"task_description":"   "}"#,
            "\nHEALTH_REVIEW_COMPLETE=nothing actionable\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert!(
            report.interventions.is_empty(),
            "empty brief is not fabricated"
        );
    }

    #[test]
    fn parse_skips_escalation_missing_plain_english_fields() {
        let out = concat!(
            r#"ESCALATE_GOAL={"goal_id":"g-1","problem":"","next_step":"n"}"#,
            "\nHEALTH_REVIEW_COMPLETE=nothing actionable\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert!(
            report.interventions.is_empty(),
            "an escalation with empty plain-English text is dropped fail-closed"
        );
    }

    #[test]
    fn parse_ignores_unknown_and_fenced_noise_lines() {
        let out = concat!(
            "```\n",
            "Some prose the agent emitted.\n",
            "HEALTHY\n",
            "```\n",
            "HEALTH_REVIEW_COMPLETE=healthy\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert!(report.interventions.is_empty());
        assert_eq!(report.summary, "healthy");
    }

    #[test]
    fn parse_tolerates_bulleted_decision_lines() {
        // Agents very commonly present their decisions as a markdown list. The
        // strict prefix match used to DROP these silently, losing remediation on
        // the critical self-heal path while still parsing "successfully".
        let out = concat!(
            r#"- LAUNCH_RECIPE={"task_description":"fix actor-binding crash-loop (286x)","target_repo":"rysweet/Simard","sequence_group":null}"#,
            "\n",
            r#"* ESCALATE_GOAL={"goal_id":"g-9","problem":"The done-gate cannot be measured.","next_step":"Pick a measurable target.","why":"unmeasurable done-gate","reason":"health-review:per-goal","link":null}"#,
            "\n",
            r#"1. LAUNCH_RECIPE={"task_description":"second systemic sweep on shared parse path","target_repo":"rysweet/Simard","sequence_group":null}"#,
            "\n- HEALTH_REVIEW_COMPLETE=2 launched, 1 escalated\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert_eq!(report.interventions.len(), 3);
        assert!(matches!(
            report.interventions[0],
            Intervention::LaunchRecipe { .. }
        ));
        assert!(matches!(
            report.interventions[1],
            Intervention::EscalateBlockedGoal { .. }
        ));
        assert!(matches!(
            report.interventions[2],
            Intervention::LaunchRecipe { .. }
        ));
        assert_eq!(report.summary, "2 launched, 1 escalated");
    }

    #[test]
    fn parse_tolerates_blockquote_and_inline_code_wrapping() {
        let out = concat!(
            "> ",
            r#"LAUNCH_RECIPE={"task_description":"fix crash-loop root cause","target_repo":"rysweet/Simard","sequence_group":null}"#,
            "\n`HEALTH_REVIEW_COMPLETE=1 systemic launch`\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert_eq!(report.interventions.len(), 1);
        assert!(matches!(
            report.interventions[0],
            Intervention::LaunchRecipe { .. }
        ));
        assert_eq!(report.summary, "1 systemic launch");
    }

    #[test]
    fn parse_decoration_never_fabricates_from_bulleted_prose() {
        // A bulleted line of ordinary prose must normalise to prose and match no
        // marker — decoration-stripping is fail-closed, never marker-inventing.
        let out = concat!(
            "- The agent observed a crash-loop and reasoned about it.\n",
            "> LAUNCH the fix soon (prose, not a marker).\n",
            "HEALTH_REVIEW_COMPLETE=healthy\n"
        );
        let report = parse_health_review_output(out).expect("parses");
        assert!(report.interventions.is_empty());
        assert_eq!(report.summary, "healthy");
    }

    #[test]
    fn strip_marker_decoration_is_identity_on_plain_lines() {
        assert_eq!(strip_marker_decoration("HEALTHY"), "HEALTHY");
        assert_eq!(
            strip_marker_decoration("  HEALTH_REVIEW_COMPLETE=ok  "),
            "HEALTH_REVIEW_COMPLETE=ok"
        );
        // A bullet char with no trailing whitespace is NOT a list bullet.
        assert_eq!(strip_marker_decoration("-notabullet"), "-notabullet");
    }

    // ── reviewer over the injectable seam ─────────────────────────────────

    enum Scripted {
        Ok(String),
        Err(String),
    }

    struct FakeRunner {
        scripted: Scripted,
        calls: Mutex<Vec<HealthReviewRequest>>,
    }

    impl FakeRunner {
        fn ok(output: &str) -> Self {
            Self {
                scripted: Scripted::Ok(output.to_string()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err(reason: &str) -> Self {
            Self {
                scripted: Scripted::Err(reason.to_string()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl HealthReviewRecipeRunner for FakeRunner {
        fn run(&self, request: &HealthReviewRequest) -> SimardResult<String> {
            self.calls.lock().unwrap().push(request.clone());
            match &self.scripted {
                Scripted::Ok(o) => Ok(o.clone()),
                Scripted::Err(r) => Err(SimardError::AdapterInvocationFailed {
                    base_type: HEALTH_REVIEW_ADAPTER_TAG.to_string(),
                    reason: r.clone(),
                }),
            }
        }
    }

    fn reviewer(runner: FakeRunner) -> RecipeHealthReviewer<FakeRunner> {
        RecipeHealthReviewer::new(
            runner,
            "simard-ooda.service".to_string(),
            "/tmp/state".to_string(),
            "/tmp/repo".to_string(),
        )
        // Pin the ladder bound so tests are hermetic against the ambient
        // SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS env var.
        .with_escalation_config(EscalationConfig { max_escalations: 2 })
    }

    #[test]
    fn review_forwards_bounded_context_vars_to_the_seam() {
        let r = reviewer(FakeRunner::ok("HEALTHY\nHEALTH_REVIEW_COMPLETE=healthy\n"));
        r.review().expect("review ok");
        assert_eq!(r.runner().call_count(), 1);
        let call = &r.runner().calls.lock().unwrap()[0];
        assert_eq!(call.service_unit, "simard-ooda.service");
        assert_eq!(call.state_root, "/tmp/state");
        assert_eq!(call.repo_path, "/tmp/repo");
        assert_eq!(call.escalation_note, "");
    }

    #[test]
    fn review_returns_parsed_interventions() {
        let out = concat!(
            r#"LAUNCH_RECIPE={"task_description":"fix systemic crash-loop"}"#,
            "\nHEALTH_REVIEW_COMPLETE=1 launch\n"
        );
        let r = reviewer(FakeRunner::ok(out));
        let out = r.review().expect("review ok");
        assert_eq!(out.interventions.len(), 1);
        assert!(matches!(
            out.interventions[0],
            Intervention::LaunchRecipe { .. }
        ));
        assert_eq!(
            out.summary.as_deref(),
            Some("1 launch"),
            "a parsed pass surfaces the HEALTH_REVIEW_COMPLETE verdict"
        );
    }

    #[test]
    fn review_degrades_to_empty_on_runner_error() {
        let r = reviewer(FakeRunner::err("boom"));
        let out = r
            .review()
            .expect("review must not surface the fault as Err");
        assert!(
            out.interventions.is_empty(),
            "a runner fault fabricates no remediation"
        );
        assert!(
            out.summary.is_none(),
            "a base runner fault surfaces NO verdict (degraded, not a silent healthy)"
        );
    }

    #[test]
    fn review_degrades_to_empty_on_missing_terminal_marker() {
        // Recipe output without the terminal marker => degraded. The rail now
        // drives the bounded escalation ladder; when every rung stays degraded
        // (FakeRunner replays the same output) it exhausts and takes no action.
        let r = reviewer(FakeRunner::ok(
            "LAUNCH_RECIPE={\"task_description\":\"x\"}\n",
        ));
        let out = r.review().expect("review ok");
        assert!(
            out.interventions.is_empty(),
            "a degraded pass (no terminal marker) takes no remediation"
        );
        assert!(
            out.summary.is_none(),
            "an exhausted degraded pass surfaces NO verdict"
        );
        // Base pass + 2 escalation rungs (the pinned max_escalations).
        assert_eq!(
            r.runner().call_count(),
            3,
            "a degraded base pass drives the bounded escalation ladder"
        );
    }

    // ── escalation ladder over a sequence-aware seam ──────────────────────

    /// A [`HealthReviewRecipeRunner`] that replays a fixed SEQUENCE of scripted
    /// outcomes (one per invocation) and records every request, so the bounded
    /// escalation ladder can be exercised rung by rung.
    struct SeqRunner {
        scripted: Mutex<std::collections::VecDeque<Scripted>>,
        calls: Mutex<Vec<HealthReviewRequest>>,
    }

    impl SeqRunner {
        fn new(scripted: Vec<Scripted>) -> Self {
            Self {
                scripted: Mutex::new(scripted.into_iter().collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
        fn note_at(&self, idx: usize) -> String {
            self.calls.lock().unwrap()[idx].escalation_note.clone()
        }
    }

    impl HealthReviewRecipeRunner for SeqRunner {
        fn run(&self, request: &HealthReviewRequest) -> SimardResult<String> {
            self.calls.lock().unwrap().push(request.clone());
            match self
                .scripted
                .lock()
                .unwrap()
                .pop_front()
                .expect("SeqRunner: more invocations than scripted outcomes")
            {
                Scripted::Ok(o) => Ok(o),
                Scripted::Err(r) => Err(SimardError::AdapterInvocationFailed {
                    base_type: HEALTH_REVIEW_ADAPTER_TAG.to_string(),
                    reason: r,
                }),
            }
        }
    }

    fn seq_reviewer(runner: SeqRunner, max_escalations: u32) -> RecipeHealthReviewer<SeqRunner> {
        RecipeHealthReviewer::new(
            runner,
            "simard-ooda.service".to_string(),
            "/tmp/state".to_string(),
            "/tmp/repo".to_string(),
        )
        .with_escalation_config(EscalationConfig { max_escalations })
    }

    const DEGRADED: &str = "LAUNCH_RECIPE={\"task_description\":\"x\"}\n"; // no terminal marker
    const RECOVERED: &str = concat!(
        r#"LAUNCH_RECIPE={"task_description":"fix systemic crash-loop"}"#,
        "\nHEALTH_REVIEW_COMPLETE=1 launch\n"
    );

    #[test]
    fn review_recovers_on_the_schema_repair_rung() {
        // Base degraded, first (schema-repair) rung recovers a valid pass.
        let r = seq_reviewer(
            SeqRunner::new(vec![
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Ok(RECOVERED.to_string()),
            ]),
            2,
        );
        let out = r.review().expect("review ok");
        assert_eq!(
            out.interventions.len(),
            1,
            "the recovered rung's interventions are used"
        );
        assert!(matches!(
            out.interventions[0],
            Intervention::LaunchRecipe { .. }
        ));
        assert_eq!(
            out.summary.as_deref(),
            Some("1 launch"),
            "a ladder-recovered rung surfaces its verdict summary"
        );
        assert_eq!(r.runner().call_count(), 2, "base + one recovery rung");
        // The base pass carries no note; the repair rung carries a schema-repair
        // note that names the required terminal-marker contract.
        assert_eq!(r.runner().note_at(0), "");
        let repair = r.runner().note_at(1);
        assert!(
            repair.contains("SCHEMA REPAIR") && repair.contains("HEALTH_REVIEW_COMPLETE"),
            "the repair note reminds the agent of the terminal-marker contract: {repair}"
        );
    }

    #[test]
    fn review_recovers_on_the_high_effort_rung() {
        // Base + schema-repair both degraded; the final high-effort rung recovers.
        let r = seq_reviewer(
            SeqRunner::new(vec![
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Ok(RECOVERED.to_string()),
            ]),
            2,
        );
        let out = r.review().expect("review ok");
        assert_eq!(out.interventions.len(), 1);
        assert!(
            out.summary.is_some(),
            "the high-effort rung recovered a verdict"
        );
        assert_eq!(r.runner().call_count(), 3, "base + two rungs");
        let high = r.runner().note_at(2);
        assert!(
            high.contains("HIGH-EFFORT"),
            "the final rung escalates to the higher-effort tier: {high}"
        );
    }

    #[test]
    fn review_exhausts_ladder_and_takes_no_remediation() {
        // Every rung stays degraded → exhausted → no remediation, no fabrication.
        let r = seq_reviewer(
            SeqRunner::new(vec![
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Ok(DEGRADED.to_string()),
            ]),
            2,
        );
        let out = r.review().expect("review ok");
        assert!(
            out.interventions.is_empty(),
            "an exhausted ladder fabricates nothing"
        );
        assert!(
            out.summary.is_none(),
            "an exhausted ladder surfaces no verdict"
        );
        assert_eq!(r.runner().call_count(), 3, "base + two exhausted rungs");
    }

    #[test]
    fn review_disabled_ladder_makes_no_retry() {
        // max_escalations == 0 disables the ladder: a degraded base pass degrades
        // immediately (byte-identical to the pre-ladder single-pass behaviour).
        let r = seq_reviewer(SeqRunner::new(vec![Scripted::Ok(DEGRADED.to_string())]), 0);
        let out = r.review().expect("review ok");
        assert!(out.interventions.is_empty());
        assert!(
            out.summary.is_none(),
            "a disabled ladder surfaces no verdict"
        );
        assert_eq!(
            r.runner().call_count(),
            1,
            "a disabled ladder never retries"
        );
    }

    #[test]
    fn review_stops_ladder_when_a_rung_faults() {
        // Base degraded, then the schema-repair rung's OWN invocation faults:
        // stop the ladder fail-closed (never fabricate on a fault), no further rung.
        let r = seq_reviewer(
            SeqRunner::new(vec![
                Scripted::Ok(DEGRADED.to_string()),
                Scripted::Err("rung spawn failed".to_string()),
            ]),
            2,
        );
        let out = r.review().expect("review ok");
        assert!(
            out.interventions.is_empty(),
            "a rung fault fabricates no remediation"
        );
        assert!(out.summary.is_none(), "a rung fault surfaces no verdict");
        assert_eq!(
            r.runner().call_count(),
            2,
            "the ladder stops at the faulting rung (no high-effort rung)"
        );
    }

    #[test]
    fn review_healthy_base_never_enters_the_ladder() {
        // A clean base pass returns immediately — no escalation invocation.
        let r = seq_reviewer(
            SeqRunner::new(vec![Scripted::Ok(
                "HEALTHY\nHEALTH_REVIEW_COMPLETE=healthy\n".to_string(),
            )]),
            2,
        );
        let out = r.review().expect("review ok");
        assert!(out.interventions.is_empty());
        assert_eq!(
            out.summary.as_deref(),
            Some("healthy"),
            "a clean HEALTHY base pass surfaces its verdict summary"
        );
        assert_eq!(
            r.runner().call_count(),
            1,
            "a healthy base pass never retries"
        );
    }

    #[test]
    fn review_base_runner_error_never_enters_the_ladder() {
        // A BASE infra/runner fault degrades with NO ladder — the base pass must
        // succeed before it can be judged merely degraded.
        let r = seq_reviewer(
            SeqRunner::new(vec![Scripted::Err("base spawn failed".to_string())]),
            2,
        );
        let out = r
            .review()
            .expect("review must not surface the fault as Err");
        assert!(out.interventions.is_empty());
        assert!(
            out.summary.is_none(),
            "a base runner fault surfaces no verdict (degraded, never a silent healthy)"
        );
        assert_eq!(
            r.runner().call_count(),
            1,
            "a base runner fault does not enter the ladder"
        );
    }

    #[test]
    fn review_base_pass_carries_no_escalation_note() {
        // Regression guard: the base pass note stays empty (byte-identical base).
        let r = seq_reviewer(SeqRunner::new(vec![Scripted::Ok(RECOVERED.to_string())]), 2);
        r.review().expect("review ok");
        assert_eq!(r.runner().note_at(0), "");
    }

    #[test]
    fn build_health_review_escalation_note_is_empty_on_base() {
        // The Base rung allocates no note so the base pass is unchanged.
        assert_eq!(
            build_health_review_escalation_note(LadderRung::Base, "prior"),
            ""
        );
    }

    #[test]
    fn resolve_recipe_path_prefers_in_tree_when_no_home_hot_copy() {
        // In-tree resolution works against the real repo checkout the tests run in.
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = resolve_health_review_recipe_path(
            repo_root,
            Some(std::path::Path::new("/nonexistent-home")),
        );
        let path = path.expect("in-tree recipe resolves");
        assert!(path.ends_with("prompt_assets/simard/recipes/overseer-health-review.yaml"));
        assert!(path.is_file());
    }
}
