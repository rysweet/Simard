//! Unified recipe-runner-backed brain — single struct [`RecipeBrain`] that
//! implements all three OODA brain traits (`OodaBrain`, `OodaDecideBrain`,
//! `OodaOrientBrain`), parameterised by recipe filename and adapter tag.
//!
//! Consolidates the formerly separate `RecipeDecideBrain`,
//! `RecipeOrientBrain`, and `RecipeEngineerLifecycleBrain` (issue #2132).
//! The principle: "one agent, one identity, one brain — different recipes
//! for different circumstances."
//!
//! Each trait impl invokes `recipe-runner-rs` as a subprocess with `-c`
//! context vars, then parses via trivial first-word / first-number
//! extractors (issue #2144 — no keyword scanners, no JSON extraction).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::decide::{DecideContext, DecideJudgment, OodaDecideBrain};
use super::orient::{
    FAILURE_PENALTY_PER_CONSECUTIVE, OodaOrientBrain, OrientContext, OrientJudgment,
};
use super::sanitize::sanitize_context_var;
use super::{EngineerLifecycleCtx, EngineerLifecycleDecision, OodaBrain};
use crate::error::{SimardError, SimardResult};

#[cfg(test)]
use super::orient::DeterministicOrientBrain;

// Phase-specific adapter tags used in parse function error/fallback messages.
const DECIDE_ADAPTER_TAG: &str = "recipe-decide-brain";
const ORIENT_ADAPTER_TAG: &str = "recipe-orient-brain";
const LIFECYCLE_ADAPTER_TAG: &str = "recipe-engineer-lifecycle-brain";

/// Cap on raw response text embedded in error messages and rationale fields.
const MAX_RATIONALE_CHARS: usize = 500;

/// Metric name emitted once per `decide_engineer_lifecycle` invocation so the
/// lifecycle-brain parse-failure rate is measurable from `metrics.jsonl`
/// (issue #2419). `value` is always `1.0`; the `outcome` label in the context
/// JSON is the numerator/denominator signal.
const LIFECYCLE_DECISION_METRIC: &str = "brain_lifecycle_decision";

/// Cap on the `first_word` token recorded in the metric context. Generous
/// enough to capture any legitimate variant name plus stray punctuation, but
/// bounded so a runaway model response can't bloat the metrics file.
const METRIC_FIRST_WORD_CHARS: usize = 64;

// ---------------------------------------------------------------------------
// recipe-runner-rs JSON envelope (issue #2419)
//
// recipe-runner-rs in its DEFAULT `text` output mode prints only a human
// summary banner ("Recipe: <name> ... SUCCESS ...") to stdout — the agent
// step's actual decision text is NOT on stdout. Parsing the first word of
// that banner always yields "Recipe:", which matches no lifecycle variant, so
// every decision silently defaulted to `continue_skipping` (~99.6% of calls).
//
// The fix mirrors the already-correct `disk_health.rs` path: invoke with
// `--output-format json` and pull the real decision text out of the JSON
// envelope's final step result before first-word extraction.
// ---------------------------------------------------------------------------

/// JSON envelope returned by `recipe-runner-rs --output-format json`.
#[derive(Debug, Deserialize)]
struct RecipeEnvelope {
    success: bool,
    #[serde(default)]
    step_results: Vec<RecipeStepResult>,
}

/// A single step's result inside the [`RecipeEnvelope`].
#[derive(Debug, Deserialize)]
struct RecipeStepResult {
    #[allow(dead_code)] // Part of the JSON contract; asserted in tests.
    #[serde(default)]
    step_id: String,
    #[serde(default)]
    output: String,
}

/// Extract the decision text the agent actually produced from the
/// `recipe-runner-rs --output-format json` stdout envelope.
///
/// Returns the FINAL step's `output` (the decision step is always terminal in
/// the OODA brain recipes). Surfaces an [`SimardError::AdapterInvocationFailed`]
/// — rather than silently returning empty text — when the envelope cannot be
/// decoded, the recipe reported `success=false`, or no step produced output.
/// This keeps a broken recipe-runner visible instead of masquerading as a
/// `default_empty` parse.
fn extract_recipe_decision_output(stdout: &[u8], adapter_tag: &str) -> SimardResult<String> {
    let envelope: RecipeEnvelope =
        serde_json::from_slice(stdout).map_err(|e| SimardError::AdapterInvocationFailed {
            base_type: adapter_tag.to_string(),
            reason: format!("failed to deserialize recipe JSON output: {e}"),
        })?;

    if !envelope.success {
        return Err(SimardError::AdapterInvocationFailed {
            base_type: adapter_tag.to_string(),
            reason: "recipe reported success=false in JSON output".to_string(),
        });
    }

    envelope
        .step_results
        .last()
        .map(|s| s.output.clone())
        .ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: adapter_tag.to_string(),
            reason: "no step results in recipe JSON output".to_string(),
        })
}

// ---------------------------------------------------------------------------
// Lifecycle decision outcome classification (issue #2419)
// ---------------------------------------------------------------------------

/// Outcome of a single `decide_engineer_lifecycle` parse, used as the
/// `outcome` label on the [`LIFECYCLE_DECISION_METRIC`] metric so the
/// parse-failure rate (`outcome != parsed`) is measurable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleParseOutcome {
    /// First word matched a known variant — a real decision was parsed.
    Parsed,
    /// Recipe output was empty/whitespace-only → defaulted to
    /// `continue_skipping`.
    DefaultEmpty,
    /// Recipe output was non-empty but the first word matched no variant →
    /// defaulted to `continue_skipping`.
    DefaultMalformed,
    /// recipe-runner-rs invocation or envelope decoding failed — no decision
    /// could be obtained. Produced on the error path of
    /// `decide_engineer_lifecycle`, not by the pure parser.
    Error,
    /// A real decision was recovered by a SCHEMA-REPAIR re-prompt after the
    /// base attempt produced a parse-miss (issue #2432, BGML progress-aware
    /// escalation). Counts as a success — it is the whole point of the ladder:
    /// it converts what would have been a silent `default_*` into a real
    /// decision, dropping the parse-failure rate.
    Repaired,
    /// A real decision was recovered by a higher-effort ESCALATED re-prompt
    /// (schema-repair + step-by-step reasoning tier) after schema-repair alone
    /// still parse-missed (issue #2432). Also a success.
    Escalated,
}

impl LifecycleParseOutcome {
    /// Stable label string recorded in the metric context.
    pub fn label(self) -> &'static str {
        match self {
            LifecycleParseOutcome::Parsed => "parsed",
            LifecycleParseOutcome::DefaultEmpty => "default_empty",
            LifecycleParseOutcome::DefaultMalformed => "default_malformed",
            LifecycleParseOutcome::Error => "error",
            LifecycleParseOutcome::Repaired => "repaired",
            LifecycleParseOutcome::Escalated => "escalated",
        }
    }

    /// Whether this outcome counts toward the parse-failure numerator
    /// (everything except a real decision). `Repaired` and `Escalated` are
    /// real decisions recovered by the ladder, so they do NOT count as
    /// failures — that is how the escalation ladder reduces the measured
    /// default/parse-failure rate (issue #2432).
    pub fn is_parse_failure(self) -> bool {
        matches!(
            self,
            LifecycleParseOutcome::DefaultEmpty
                | LifecycleParseOutcome::DefaultMalformed
                | LifecycleParseOutcome::Error
        )
    }
}

// ---------------------------------------------------------------------------
// Confidence-gated escalation ladder (issue #2432, BGML progress-aware module)
//
// On a base parse-miss (`DefaultEmpty`/`DefaultMalformed` — the brain's coarse
// judgment was unparseable, i.e. low confidence) we spend EXTRA compute ONLY on
// that weak case: a bounded sequence of re-prompts that (1) feed the malformed
// output back asking for a valid first-word variant (schema-repair) and (2)
// escalate to a higher-effort reasoning tier. The deterministic
// `continue_skipping` default is reached only AFTER the ladder is exhausted —
// replacing the previous silent default-on-first-miss behaviour (#2419 family).
// ---------------------------------------------------------------------------

/// One rung of the escalation ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderRung {
    /// Base attempt — cheap parse, exactly as the #2419 path.
    Base,
    /// Schema-repair re-prompt: feed the malformed prior output back asking for
    /// a valid first-word variant. Same reasoning tier.
    SchemaRepair,
    /// Schema-repair PLUS a higher-effort, step-by-step reasoning tier bump.
    Escalate,
}

/// Parameters for a single lifecycle recipe invocation. `prior_output` is the
/// malformed text fed back on repair/escalate rungs (empty on `Base`).
pub struct LadderAttempt<'a> {
    pub rung: LadderRung,
    pub prior_output: &'a str,
}

impl LadderAttempt<'_> {
    /// The base (cheap) attempt — no repair note.
    pub fn base() -> LadderAttempt<'static> {
        LadderAttempt {
            rung: LadderRung::Base,
            prior_output: "",
        }
    }

    /// The `escalation_note` context var fed to the recipe for this rung.
    /// Empty on `Base` so base behaviour is byte-identical to pre-#2432.
    pub fn escalation_note(&self) -> String {
        build_escalation_note(self.rung, self.prior_output)
    }
}

/// The closed variant token list, echoed into the schema-repair note so the
/// model is reminded of the exact accepted first words. Kept in sync with the
/// recipe `OPTIONS` section and `rustyclawd::VALID_VARIANTS`.
const LIFECYCLE_VARIANT_LIST: &str = "continue_skipping, reclaim_and_redispatch, deprioritize, open_tracking_issue, mark_goal_blocked, consider_self_update";

/// Build the `escalation_note` injected into the recipe prompt for a given
/// rung. Pinned wording — see the `escalation_note_*` content-pin tests.
pub fn build_escalation_note(rung: LadderRung, prior_output: &str) -> String {
    if matches!(rung, LadderRung::Base) {
        return String::new();
    }
    let prior = truncate(prior_output.trim(), MAX_RATIONALE_CHARS);
    let repair = format!(
        "## ⚠️ SCHEMA REPAIR (retry) ## \
         Your previous response could not be parsed: its FIRST WORD was not a valid decision variant. \
         Previous response: <<<{prior}>>> \
         Respond again now. The VERY FIRST WORD of your reply MUST be exactly one of: {LIFECYCLE_VARIANT_LIST}. \
         Output that variant word first, then your rationale."
    );
    match rung {
        LadderRung::Base => String::new(),
        LadderRung::SchemaRepair => repair,
        LadderRung::Escalate => format!(
            "{repair} ## HIGH-EFFORT RETRY ## \
             This is a final, higher-effort attempt. Reason carefully, step by step, about the \
             engineer's state BEFORE answering, then output the single variant word first."
        ),
    }
}

/// Bound on how far the escalation ladder climbs. Configurable via
/// `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS`, hard-capped so a misconfiguration
/// can never turn the brain into an unbounded retry loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EscalationConfig {
    /// Number of escalation rungs attempted AFTER a base parse-miss. `0`
    /// disables the ladder (pre-#2432 default-on-first-miss behaviour).
    pub max_escalations: u32,
}

impl EscalationConfig {
    /// Default rungs: schema-repair, then schema-repair + high-effort.
    pub const DEFAULT_MAX_ESCALATIONS: u32 = 2;
    /// Absolute ceiling regardless of env configuration.
    pub const HARD_CAP: u32 = 3;
    /// Env var that overrides [`DEFAULT_MAX_ESCALATIONS`](Self::DEFAULT_MAX_ESCALATIONS).
    pub const ENV_VAR: &'static str = "SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS";

    /// Read the cap from the environment, clamped to `[0, HARD_CAP]`.
    pub fn from_env() -> Self {
        Self {
            max_escalations: parse_max_escalations(std::env::var(Self::ENV_VAR).ok().as_deref()),
        }
    }
}

/// Parse the `SIMARD_BRAIN_ESCALATION_MAX_ATTEMPTS` value into a bounded rung
/// count. Unset / unparseable → [`EscalationConfig::DEFAULT_MAX_ESCALATIONS`];
/// always clamped to [`EscalationConfig::HARD_CAP`] so no configuration can
/// produce an unbounded retry loop. Pure so it is unit-testable without env
/// mutation.
fn parse_max_escalations(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(EscalationConfig::DEFAULT_MAX_ESCALATIONS)
        .min(EscalationConfig::HARD_CAP)
}

impl Default for EscalationConfig {
    fn default() -> Self {
        Self {
            max_escalations: Self::DEFAULT_MAX_ESCALATIONS,
        }
    }
}

/// Seam over the raw lifecycle recipe invocation so the escalation ladder is
/// unit-testable without a live `recipe-runner-rs`. Production wires
/// [`RecipeBrain`]; tests wire a scripted stub. Returns the raw decision text
/// (the recipe's final step output); errors propagate.
pub trait LifecycleInvoker {
    fn invoke_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
        attempt: &LadderAttempt,
    ) -> SimardResult<String>;
}

/// Drive the confidence-gated escalation ladder after a base parse-miss.
///
/// `base_raw` is the (already-invoked) base attempt's raw output and
/// `base_outcome` its parse-miss classification. Returns the final decision,
/// its outcome (`Repaired`/`Escalated` on recovery, else the original
/// parse-miss), and the total number of brain invocations made (base + rungs).
///
/// Bounded by `cfg.max_escalations`; each rung is logged loudly; the
/// deterministic `continue_skipping` default is returned only when every rung
/// is exhausted (or an escalation invocation itself fails).
pub fn run_escalation_ladder(
    invoker: &dyn LifecycleInvoker,
    ctx: &EngineerLifecycleCtx,
    base_raw: &str,
    base_outcome: LifecycleParseOutcome,
    cfg: &EscalationConfig,
) -> (EngineerLifecycleDecision, LifecycleParseOutcome, u32) {
    let mut prior = base_raw.to_string();
    let mut attempts = 1u32; // the base attempt already happened

    for rung_idx in 1..=cfg.max_escalations {
        let rung = if rung_idx == 1 {
            LadderRung::SchemaRepair
        } else {
            LadderRung::Escalate
        };
        attempts += 1;

        tracing::warn!(
            target: "simard::ooda_brain",
            goal = %ctx.goal_id,
            rung = ?rung,
            attempt = attempts,
            base_outcome = base_outcome.label(),
            "brain decision parse-miss → escalating (confidence-gated ladder, issue #2432)"
        );
        eprintln!(
            "[simard] BRAIN ESCALATION goal={} rung={:?} attempt={} (parse-miss recovery)",
            ctx.goal_id, rung, attempts
        );

        let attempt = LadderAttempt {
            rung,
            prior_output: &prior,
        };
        match invoker.invoke_lifecycle(ctx, &attempt) {
            Err(e) => {
                tracing::warn!(
                    target: "simard::ooda_brain",
                    goal = %ctx.goal_id,
                    rung = ?rung,
                    error = %e,
                    "brain escalation attempt failed to invoke; stopping ladder, using deterministic default"
                );
                eprintln!(
                    "[simard] BRAIN ESCALATION goal={} rung={:?} invoke failed: {e} — falling back to default",
                    ctx.goal_id, rung
                );
                break;
            }
            Ok(raw2) => {
                let (decision, oc) = parse_lifecycle_outcome(&raw2);
                if !oc.is_parse_failure() {
                    let recovered = match rung {
                        LadderRung::SchemaRepair => LifecycleParseOutcome::Repaired,
                        LadderRung::Escalate => LifecycleParseOutcome::Escalated,
                        LadderRung::Base => oc,
                    };
                    tracing::info!(
                        target: "simard::ooda_brain",
                        goal = %ctx.goal_id,
                        rung = ?rung,
                        attempt = attempts,
                        decision = lifecycle_decision_choice(&decision),
                        "brain decision RECOVERED via escalation ladder (issue #2432)"
                    );
                    eprintln!(
                        "[simard] BRAIN ESCALATION goal={} RECOVERED decision={} via {:?} (attempt {})",
                        ctx.goal_id,
                        lifecycle_decision_choice(&decision),
                        rung,
                        attempts
                    );
                    return (decision, recovered, attempts);
                }
                // Still a parse-miss — feed the latest malformed output into the
                // next rung's repair note.
                prior = raw2;
            }
        }
    }

    // Ladder exhausted, disabled, or an escalation invoke failed: fall to the
    // deterministic default, preserving the original parse-miss outcome for the
    // metric numerator.
    if cfg.max_escalations > 0 {
        tracing::warn!(
            target: "simard::ooda_brain",
            goal = %ctx.goal_id,
            attempts,
            base_outcome = base_outcome.label(),
            "brain escalation ladder exhausted without a parseable decision; deterministic default"
        );
        eprintln!(
            "[simard] BRAIN ESCALATION goal={} ladder exhausted after {attempts} attempts — deterministic default",
            ctx.goal_id
        );
    }
    (default_continue_skipping(), base_outcome, attempts)
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<recipe_filename>` (hot-reload)
///   2. `<repo_root>/prompt_assets/simard/recipes/<recipe_filename>` (in-tree)
pub fn resolve_recipe_path(repo_root: &Path, recipe_filename: &str) -> Option<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        let hot = home
            .join(".simard")
            .join("prompt_assets/simard/recipes")
            .join(recipe_filename);
        if hot.is_file() {
            return Some(hot);
        }
    }
    let in_tree = repo_root
        .join("prompt_assets/simard/recipes")
        .join(recipe_filename);
    if in_tree.is_file() {
        return Some(in_tree);
    }
    None
}

/// Truncate a string to at most `max` characters, appending '…' if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    // Fast path: byte length ≤ max implies char count ≤ max (chars ≤ bytes).
    if s.len() <= max {
        return s.to_string();
    }
    match s.char_indices().nth(max) {
        Some((byte_offset, _)) => format!("{}…", &s[..byte_offset]),
        None => s.to_string(),
    }
}

/// Unified recipe-runner-backed brain. Three instances with different
/// `(recipe_filename, adapter_tag)` replace the three former structs.
pub struct RecipeBrain {
    pub(crate) recipe_path: PathBuf,
    pub(crate) agent_binary: &'static str,
    pub(crate) adapter_tag: &'static str,
}

impl RecipeBrain {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    ///
    /// `recipe_filename` selects the YAML (e.g. `"ooda-decide.yaml"`).
    /// `adapter_tag` appears in error messages and logs (e.g. `"recipe-decide-brain"`).
    pub fn new(repo_root: &Path, recipe_filename: &str, adapter_tag: &'static str) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, recipe_filename)?;
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
            adapter_tag,
        })
    }
}

impl OodaDecideBrain for RecipeBrain {
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        // KNOWN LIMITATION (issue #2421): this phase still reads recipe-runner-rs
        // DEFAULT `text` stdout (the summary banner), so `parse_action_from_text`
        // always sees `Recipe:` and silently defaults to `AdvanceGoal`. It shares
        // the exact root cause fixed for the lifecycle phase in issue #2419 but is
        // deferred there to bound the behavioral blast radius (top-level action
        // routing). Tracked + to be fixed via `--output-format json` + envelope
        // extraction in #2421.
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("urgency={:.3}", ctx.urgency))
            .arg("-c")
            .arg(format!("reason={}", sanitize_context_var(&ctx.reason, 500)))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_action_from_text(&raw))
    }
}

impl OodaOrientBrain for RecipeBrain {
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        // KNOWN LIMITATION (issue #2421): like `judge_decision` above, this reads
        // recipe-runner-rs DEFAULT `text` stdout. Worse than a benign default —
        // `parse_orient_from_text` scans the banner for the first decimal in
        // [0, base_urgency] and the banner's timing string (e.g. `(0.0s)`) yields
        // `0.0`, silently demoting urgency to a scraped timing value rather than
        // the LLM's judgment. Same #2419 root cause; fix tracked in #2421.
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("base_urgency={:.3}", ctx.base_urgency))
            .arg("-c")
            .arg(format!(
                "base_reason={}",
                sanitize_context_var(&ctx.base_reason, 500)
            ))
            .arg("-c")
            .arg(format!("failure_count={}", ctx.failure_count))
            .output()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("recipe-runner-rs spawn failed: {e}"),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, 500)
                ),
            });
        }

        let raw = String::from_utf8(output.stdout)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        Ok(parse_orient_from_text(
            &raw,
            ctx.base_urgency,
            ctx.failure_count,
        ))
    }
}

impl RecipeBrain {
    /// Invoke the engineer-lifecycle recipe once for the given ladder rung and
    /// return the raw decision text (the recipe's final step output). On
    /// failure returns the error PLUS a stable `cause` label
    /// (`spawn_failed` / `nonzero_exit` / `envelope_decode_failed`) for the
    /// `brain_lifecycle_decision` metric.
    ///
    /// The `escalation_note` context var (empty on `LadderRung::Base`) carries
    /// the schema-repair / high-effort instruction; it is rendered by the
    /// recipe's `{{escalation_note}}` placeholder. Passing it on every call
    /// keeps base behaviour byte-identical to the #2419 path.
    fn invoke_lifecycle_raw(
        &self,
        ctx: &EngineerLifecycleCtx,
        attempt: &LadderAttempt,
    ) -> Result<String, (SimardError, &'static str)> {
        let sentinel = ctx
            .sentinel_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "<none>".to_string());
        let minutes = if ctx.minutes_since_last_update_attempt == u64::MAX {
            "never".to_string()
        } else {
            ctx.minutes_since_last_update_attempt.to_string()
        };
        let escalation_note = attempt.escalation_note();

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            // issue #2419: text mode prints only a summary banner to stdout —
            // the agent decision text is only exposed via the JSON envelope.
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!(
                "goal_description={}",
                sanitize_context_var(&ctx.goal_description, 500)
            ))
            .arg("-c")
            .arg(format!("cycle_number={}", ctx.cycle_number))
            .arg("-c")
            .arg(format!(
                "consecutive_skip_count={}",
                ctx.consecutive_skip_count
            ))
            .arg("-c")
            .arg(format!("failure_count={}", ctx.failure_count))
            .arg("-c")
            .arg(format!(
                "worktree_path={}",
                sanitize_context_var(&ctx.worktree_path.display().to_string(), 500)
            ))
            .arg("-c")
            .arg(format!(
                "worktree_mtime_secs_ago={}",
                ctx.worktree_mtime_secs_ago
            ))
            .arg("-c")
            .arg(format!("sentinel_pid={sentinel}"))
            .arg("-c")
            .arg(format!(
                "last_engineer_log_tail={}",
                sanitize_context_var(&ctx.last_engineer_log_tail, 2000)
            ))
            .arg("-c")
            .arg(format!("commits_behind={}", ctx.commits_behind))
            .arg("-c")
            .arg(format!(
                "in_flight_engineer_count={}",
                ctx.in_flight_engineer_count
            ))
            .arg("-c")
            .arg(format!("minutes_since_last_update_attempt={minutes}"))
            // issue #2432: the (possibly empty) escalation/schema-repair note.
            .arg("-c")
            .arg(format!(
                "escalation_note={}",
                sanitize_context_var(&escalation_note, 4000)
            ))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err((
                    SimardError::AdapterInvocationFailed {
                        base_type: self.adapter_tag.to_string(),
                        reason: format!("recipe-runner-rs spawn failed: {e}"),
                    },
                    "spawn_failed",
                ));
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err((
                SimardError::AdapterInvocationFailed {
                    base_type: self.adapter_tag.to_string(),
                    reason: format!(
                        "recipe exited with {}: {}",
                        output.status,
                        truncate(&stderr, MAX_RATIONALE_CHARS)
                    ),
                },
                "nonzero_exit",
            ));
        }

        extract_recipe_decision_output(&output.stdout, self.adapter_tag)
            .map_err(|e| (e, "envelope_decode_failed"))
    }
}

impl LifecycleInvoker for RecipeBrain {
    fn invoke_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
        attempt: &LadderAttempt,
    ) -> SimardResult<String> {
        self.invoke_lifecycle_raw(ctx, attempt).map_err(|(e, _)| e)
    }
}

impl OodaBrain for RecipeBrain {
    fn decide_engineer_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
    ) -> SimardResult<EngineerLifecycleDecision> {
        // Base (cheap) attempt — identical to the #2419 path. A genuine
        // recipe-runner failure still surfaces loudly as `Err` (it must NOT be
        // masked by the ladder): only a *parse-miss* on a successful run is
        // low-confidence enough to escalate.
        let base_raw = match self.invoke_lifecycle_raw(ctx, &LadderAttempt::base()) {
            Ok(raw) => raw,
            Err((e, cause)) => {
                record_lifecycle_decision_metric(
                    ctx,
                    LifecycleParseOutcome::Error,
                    "",
                    "none",
                    cause,
                    1,
                );
                return Err(e);
            }
        };

        let (decision, outcome) = parse_lifecycle_outcome(&base_raw);
        if !outcome.is_parse_failure() {
            // Parsed on the first try — no extra compute spent.
            record_lifecycle_decision_metric(
                ctx,
                outcome,
                &lifecycle_first_word(&base_raw),
                lifecycle_decision_choice(&decision),
                "ok",
                1,
            );
            return Ok(decision);
        }

        // Parse-miss → confidence-gated escalation ladder (issue #2432). Spend
        // extra compute ONLY on this weak case.
        let cfg = EscalationConfig::from_env();
        let (final_decision, final_outcome, attempts) =
            run_escalation_ladder(self, ctx, &base_raw, outcome, &cfg);
        let cause = if final_outcome.is_parse_failure() {
            "ladder_exhausted"
        } else {
            "ladder_recovered"
        };
        record_lifecycle_decision_metric(
            ctx,
            final_outcome,
            &lifecycle_first_word(&base_raw),
            lifecycle_decision_choice(&final_decision),
            cause,
            attempts,
        );
        Ok(final_decision)
    }
}

// ---------------------------------------------------------------------------
// Parse functions — trivial first-word / first-number extractors.
// No keyword scanning, no JSON extraction, no fallback chains.
// The recipe prompts instruct the LLM to output the action word first.
// ---------------------------------------------------------------------------

/// Parse recipe stdout for an action keyword as the first word (decide phase).
/// Case-insensitive match on the first whitespace-delimited token.
/// Defaults to `advance_goal` if the first word is unrecognised.
pub fn parse_action_from_text(text: &str) -> DecideJudgment {
    let trimmed = text.trim();
    let first_word = trimmed.split_whitespace().next().unwrap_or("");

    // Rationale allocation is deferred into the match arm — avoids a
    // wasted heap alloc on the (no-match) default path.
    type Ctor = fn(String) -> DecideJudgment;
    let pairs: &[(&str, Ctor)] = &[
        ("poll_developer_activity", |r| {
            DecideJudgment::PollDeveloperActivity { rationale: r }
        }),
        ("consolidate_memory", |r| {
            DecideJudgment::ConsolidateMemory { rationale: r }
        }),
        ("run_improvement", |r| DecideJudgment::RunImprovement {
            rationale: r,
        }),
        ("extract_ideas", |r| DecideJudgment::ExtractIdeas {
            rationale: r,
        }),
        ("safe_update", |r| DecideJudgment::SafeUpdate {
            rationale: r,
        }),
        ("research_query", |r| DecideJudgment::ResearchQuery {
            rationale: r,
        }),
        ("run_gym_eval", |r| DecideJudgment::RunGymEval {
            rationale: r,
        }),
        ("build_skill", |r| DecideJudgment::BuildSkill {
            rationale: r,
        }),
        ("launch_session", |r| DecideJudgment::LaunchSession {
            rationale: r,
        }),
        ("advance_goal", |r| DecideJudgment::AdvanceGoal {
            rationale: r,
        }),
    ];
    for (kw, ctor) in pairs {
        if first_word.eq_ignore_ascii_case(kw) {
            return ctor(truncate(trimmed, MAX_RATIONALE_CHARS));
        }
    }
    DecideJudgment::AdvanceGoal {
        rationale: format!(
            "{DECIDE_ADAPTER_TAG}: no action keyword found in recipe output; defaulting to advance_goal"
        ),
    }
}

// ---------------------------------------------------------------------------
// Orient parse: first decimal float → deterministic floor
// ---------------------------------------------------------------------------

/// Parse recipe output for the first decimal float (e.g. `0.42`).
/// Falls to the deterministic floor when no valid float is found.
pub fn parse_orient_from_text(text: &str, base_urgency: f64, failure_count: u32) -> OrientJudgment {
    // Hoist trim above the scanner — avoids re-trimming on each candidate.
    let trimmed = text.trim();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
                let after_dot = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > after_dot
                    && let Ok(val) = text[start..i].parse::<f64>()
                {
                    // Inline validation before allocating rationale string.
                    if val.is_finite() && (0.0..=1.0).contains(&val) && val <= base_urgency + 1e-9 {
                        return OrientJudgment {
                            adjusted_urgency: val,
                            rationale: truncate(trimmed, MAX_RATIONALE_CHARS),
                            confidence: 1.0,
                            demotion_applied: base_urgency - val,
                        };
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    deterministic_floor(base_urgency, failure_count)
}

/// Compute the deterministic floor judgment.
fn deterministic_floor(base_urgency: f64, failure_count: u32) -> OrientJudgment {
    let penalty = FAILURE_PENALTY_PER_CONSECUTIVE * failure_count as f64;
    let adjusted = (base_urgency - penalty).max(0.0);
    OrientJudgment {
        adjusted_urgency: adjusted,
        rationale: format!(
            "{ORIENT_ADAPTER_TAG}: deterministic floor — {failure_count} failure(s), \
             urgency {base_urgency:.2} − {penalty:.2}",
        ),
        confidence: 1.0,
        demotion_applied: base_urgency - adjusted,
    }
}

// ---------------------------------------------------------------------------
// Lifecycle parse: first-word extraction → ContinueSkipping default
// ---------------------------------------------------------------------------

/// Parse recipe output for a lifecycle decision variant as the first word.
/// Case-insensitive match. Defaults to `ContinueSkipping`.
///
/// Thin decision-only wrapper over [`parse_lifecycle_outcome`]. Retained as
/// the documented parser entry point used by the operator replay runbook
/// (`docs/howto/diagnose-brain-decision-parse-failures.md`, Step 3) and the
/// `parse_*_from_text` reference trio. Production now routes through
/// [`parse_lifecycle_outcome`] to capture the parse outcome for the
/// `brain_lifecycle_decision` metric, so in non-test builds this wrapper has
/// no internal caller.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_lifecycle_from_text(text: &str) -> EngineerLifecycleDecision {
    parse_lifecycle_outcome(text).0
}

/// Parse recipe output into a lifecycle decision AND a
/// [`LifecycleParseOutcome`] classification.
///
/// The outcome distinguishes a genuinely parsed decision (`Parsed`) from the
/// two distinct ways the parser falls back to `ContinueSkipping`:
/// `DefaultEmpty` (no text at all) and `DefaultMalformed` (text present but
/// the first word is not a known variant). This is what makes the
/// parse-failure rate measurable per issue #2419 — before this split, a real
/// `continue_skipping` decision and a silent fallback were indistinguishable.
pub fn parse_lifecycle_outcome(text: &str) -> (EngineerLifecycleDecision, LifecycleParseOutcome) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (
            default_continue_skipping(),
            LifecycleParseOutcome::DefaultEmpty,
        );
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    let rest = || truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);

    // Use eq_ignore_ascii_case instead of to_ascii_lowercase() — avoids a
    // heap-allocated String on every call.
    let decision = if first_word.eq_ignore_ascii_case("continue_skipping") {
        EngineerLifecycleDecision::ContinueSkipping { rationale: rest() }
    } else if first_word.eq_ignore_ascii_case("deprioritize") {
        EngineerLifecycleDecision::Deprioritize { rationale: rest() }
    } else if first_word.eq_ignore_ascii_case("consider_self_update") {
        EngineerLifecycleDecision::ConsiderSelfUpdate { rationale: rest() }
    } else if first_word.eq_ignore_ascii_case("reclaim_and_redispatch") {
        EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale: rest(),
            redispatch_context: String::new(),
        }
    } else if first_word.eq_ignore_ascii_case("open_tracking_issue") {
        let rest = rest();
        EngineerLifecycleDecision::OpenTrackingIssue {
            title: "OODA stuck".to_string(),
            body: rest.clone(),
            rationale: rest,
        }
    } else if first_word.eq_ignore_ascii_case("mark_goal_blocked") {
        let rest = rest();
        EngineerLifecycleDecision::MarkGoalBlocked {
            reason: rest.clone(),
            rationale: rest,
        }
    } else {
        return (
            default_continue_skipping(),
            LifecycleParseOutcome::DefaultMalformed,
        );
    };
    (decision, LifecycleParseOutcome::Parsed)
}

fn default_continue_skipping() -> EngineerLifecycleDecision {
    EngineerLifecycleDecision::ContinueSkipping {
        rationale: format!(
            "{LIFECYCLE_ADAPTER_TAG}: no decision keyword found in recipe output; defaulting to continue_skipping"
        ),
    }
}

/// The first whitespace-delimited token of `text`, bounded for metric storage.
fn lifecycle_first_word(text: &str) -> String {
    truncate(
        text.split_whitespace().next().unwrap_or(""),
        METRIC_FIRST_WORD_CHARS,
    )
}

/// The snake_case `choice` tag of a decision, matching the
/// `EngineerLifecycleDecision` serde representation. Used as the `decision`
/// field of the metric context.
fn lifecycle_decision_choice(decision: &EngineerLifecycleDecision) -> &'static str {
    match decision {
        EngineerLifecycleDecision::ContinueSkipping { .. } => "continue_skipping",
        EngineerLifecycleDecision::ReclaimAndRedispatch { .. } => "reclaim_and_redispatch",
        EngineerLifecycleDecision::Deprioritize { .. } => "deprioritize",
        EngineerLifecycleDecision::OpenTrackingIssue { .. } => "open_tracking_issue",
        EngineerLifecycleDecision::MarkGoalBlocked { .. } => "mark_goal_blocked",
        EngineerLifecycleDecision::ConsiderSelfUpdate { .. } => "consider_self_update",
    }
}

/// Build the JSON `context` payload for the `brain_lifecycle_decision` metric.
///
/// Separated from the I/O so the payload shape can be unit-tested without
/// touching the real `metrics.jsonl`.
fn build_lifecycle_metric_context(
    ctx: &EngineerLifecycleCtx,
    outcome: LifecycleParseOutcome,
    first_word: &str,
    decision_choice: &str,
    cause: &str,
    attempts: u32,
) -> String {
    serde_json::json!({
        "goal_id": ctx.goal_id,
        "outcome": outcome.label(),
        "is_parse_failure": outcome.is_parse_failure(),
        "first_word": first_word,
        "consecutive_skip_count": ctx.consecutive_skip_count,
        "decision": decision_choice,
        "cause": cause,
        // issue #2432: total brain invocations spent (base + escalation rungs).
        "attempts": attempts,
    })
    .to_string()
}

/// Record one `brain_lifecycle_decision` metric event (value `1.0`) per
/// `decide_engineer_lifecycle` invocation so the parse-failure rate
/// (`outcome != "parsed"`) is measurable from `metrics.jsonl` (issue #2419).
/// The `attempts` field (issue #2432) records how many brain invocations the
/// escalation ladder spent.
///
/// Best-effort: a metrics-write failure is logged, never propagated — the
/// brain decision must not fail because telemetry could not be persisted.
///
/// No-op under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl` (which would corrupt the very
/// before/after measurement this metric exists to capture).
fn record_lifecycle_decision_metric(
    ctx: &EngineerLifecycleCtx,
    outcome: LifecycleParseOutcome,
    first_word: &str,
    decision_choice: &str,
    cause: &str,
    attempts: u32,
) {
    let context =
        build_lifecycle_metric_context(ctx, outcome, first_word, decision_choice, cause, attempts);
    if cfg!(test) {
        return;
    }
    if let Err(e) = crate::self_metrics::record_metric(LIFECYCLE_DECISION_METRIC, 1.0, &context) {
        tracing::warn!(
            target: "simard::ooda_brain",
            error = %e,
            outcome = outcome.label(),
            "failed to record brain_lifecycle_decision metric (decision unaffected)",
        );
    }
}

// ---------------------------------------------------------------------------
// Tests — behavioral contracts for the unified RecipeBrain struct.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_brain::EngineerLifecycleCtx;
    use crate::ooda_brain::decide::DecideContext;
    use crate::ooda_brain::orient::OrientContext;
    use std::sync::Arc;

    // ===================================================================
    // resolve_recipe_path — parameterised by filename
    // ===================================================================

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_repo() {
        let result = resolve_recipe_path(Path::new("/nonexistent"), "ooda-decide.yaml");
        assert!(
            result.is_none(),
            "must return None when neither hot-reload nor in-tree path exists"
        );
    }

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_filename() {
        let result = resolve_recipe_path(Path::new("/tmp"), "does-not-exist.yaml");
        assert!(
            result.is_none(),
            "must return None when the recipe filename doesn't match any file"
        );
    }

    #[test]
    fn resolve_recipe_path_finds_in_tree_recipe() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        let recipe_file = recipe_dir.join("ooda-decide.yaml");
        std::fs::write(&recipe_file, "# test recipe").unwrap();

        let result = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        assert_eq!(
            result,
            Some(recipe_file),
            "must find the in-tree recipe file"
        );
    }

    #[test]
    fn resolve_recipe_path_uses_filename_parameter() {
        // Verify that different filenames resolve to different paths
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();

        // Create two recipe files
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# decide").unwrap();
        std::fs::write(recipe_dir.join("ooda-orient.yaml"), "# orient").unwrap();

        let decide_path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        let orient_path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml");

        assert_ne!(
            decide_path, orient_path,
            "different filenames must resolve to different paths"
        );
        assert!(
            decide_path
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-decide")
        );
        assert!(
            orient_path
                .as_ref()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-orient")
        );
    }

    // ===================================================================
    // RecipeBrain::new — constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_decide_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-decide.yaml",
            "recipe-decide-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_orient_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-orient.yaml",
            "recipe-orient-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_lifecycle_recipe_missing() {
        let brain = RecipeBrain::new(
            Path::new("/nonexistent"),
            "ooda-engineer-lifecycle.yaml",
            "recipe-engineer-lifecycle-brain",
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_stores_adapter_tag() {
        // Create a temporary recipe file so path resolution succeeds
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# test").unwrap();

        // Even if recipe-runner-rs isn't available, the adapter_tag contract
        // is that when construction succeeds, the tag is stored. We test
        // this via the error message from a trait call (see judge_*_error tests).
        // Constructor may return None if binary missing — that's expected.
        // This test documents the intent; the binary check makes it
        // environment-dependent.
        let _brain = RecipeBrain::new(tmp.path(), "ooda-decide.yaml", "recipe-decide-brain");
        // If construction succeeded, verify the tag is stored
        // If it returned None (no binary), that's OK for this environment
    }

    // ===================================================================
    // Trait impls — error messages include adapter_tag
    // ===================================================================

    #[test]
    fn judge_decision_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let ctx = DecideContext {
            goal_id: "test-goal".to_string(),
            urgency: 0.7,
            reason: "test reason".to_string(),
        };
        let err = brain.judge_decision(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-decide-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    #[test]
    fn judge_orientation_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = OrientContext {
            goal_id: "test-goal".into(),
            base_urgency: 0.7,
            base_reason: "test reason".into(),
            failure_count: 1,
        };
        let err = brain.judge_orientation(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-orient-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    #[test]
    fn decide_engineer_lifecycle_error_includes_adapter_tag() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = EngineerLifecycleCtx {
            goal_id: "test-goal".into(),
            goal_description: "test".into(),
            cycle_number: 1,
            consecutive_skip_count: 0,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp/wt"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: Some(42),
            last_engineer_log_tail: "ok".into(),
            commits_behind: 0,
            in_flight_engineer_count: 1,
            minutes_since_last_update_attempt: u64::MAX,
        };
        let err = brain.decide_engineer_lifecycle(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-engineer-lifecycle-brain"),
            "error should contain the adapter tag; got: {msg}"
        );
    }

    // ===================================================================
    // Trait impls — error type is AdapterInvocationFailed
    // ===================================================================

    #[test]
    fn judge_decision_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let ctx = DecideContext {
            goal_id: "g1".into(),
            urgency: 0.5,
            reason: "test".into(),
        };
        let err = brain.judge_decision(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    #[test]
    fn judge_orientation_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = OrientContext {
            goal_id: "g1".into(),
            base_urgency: 0.5,
            base_reason: "test".into(),
            failure_count: 1,
        };
        let err = brain.judge_orientation(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    #[test]
    fn decide_lifecycle_spawn_failure_is_adapter_invocation_failed() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = EngineerLifecycleCtx {
            goal_id: "g1".into(),
            goal_description: "test".into(),
            cycle_number: 1,
            consecutive_skip_count: 0,
            failure_count: 0,
            worktree_path: PathBuf::from("/tmp"),
            worktree_mtime_secs_ago: 60,
            sentinel_pid: None,
            last_engineer_log_tail: String::new(),
            commits_behind: 0,
            in_flight_engineer_count: 0,
            minutes_since_last_update_attempt: u64::MAX,
        };
        let err = brain.decide_engineer_lifecycle(&ctx).unwrap_err();
        assert!(
            matches!(err, SimardError::AdapterInvocationFailed { .. }),
            "spawn failure must be AdapterInvocationFailed; got: {err:?}"
        );
    }

    // ===================================================================
    // Type erasure — RecipeBrain implements all three traits
    // ===================================================================

    #[test]
    fn recipe_brain_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecipeBrain>();
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_brain() {
        // This test verifies the type relationship at compile time.
        // Runtime: the brain has a fake path, so trait calls would fail,
        // but Arc wrapping must compile.
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaBrain> = Arc::new(brain);
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_decide_brain() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaDecideBrain> = Arc::new(brain);
    }

    #[test]
    fn recipe_brain_can_be_arc_dyn_ooda_orient_brain() {
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OodaOrientBrain> = Arc::new(brain);
    }

    // ===================================================================
    // Shared helpers — truncate
    // ===================================================================

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_exact_length_unchanged() {
        assert_eq!(truncate("12345", 5), "12345");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world", 5);
        assert_eq!(result, "hello…");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn truncate_unicode_boundary_safe() {
        // "héllo" — 'é' is 2 bytes. Truncating to 3 chars should give "hél…"
        let result = truncate("héllo", 3);
        assert_eq!(result, "hél…");
    }

    #[test]
    fn truncate_max_zero() {
        let result = truncate("hello", 0);
        assert_eq!(result, "…");
    }

    #[test]
    fn truncate_preserves_full_multibyte_string() {
        // String with only multibyte chars, length (in chars) ≤ max
        let s = "日本語"; // 3 chars, 9 bytes
        assert_eq!(truncate(s, 5), s);
    }

    // ===================================================================
    // Wiring contract — the three "instances" that brains.rs creates
    // ===================================================================

    #[test]
    fn decide_brain_instance_uses_correct_recipe_filename() {
        // Verify the filename parameter is "ooda-decide.yaml"
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# decide").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml");
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-decide.yaml"));
    }

    #[test]
    fn orient_brain_instance_uses_correct_recipe_filename() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-orient.yaml"), "# orient").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml");
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-orient.yaml"));
    }

    #[test]
    fn lifecycle_brain_instance_uses_correct_recipe_filename() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(
            recipe_dir.join("ooda-engineer-lifecycle.yaml"),
            "# lifecycle",
        )
        .unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-engineer-lifecycle.yaml");
        assert!(path.is_some());
        assert!(
            path.unwrap()
                .to_str()
                .unwrap()
                .contains("ooda-engineer-lifecycle.yaml")
        );
    }

    // ===================================================================
    // Different adapter_tags produce different error messages
    // ===================================================================

    #[test]
    fn different_adapter_tags_produce_different_errors() {
        let decide_brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let orient_brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-orient-brain",
        };
        let ctx = DecideContext {
            goal_id: "g1".into(),
            urgency: 0.5,
            reason: "test".into(),
        };
        let orient_ctx = OrientContext {
            goal_id: "g1".into(),
            base_urgency: 0.5,
            base_reason: "test".into(),
            failure_count: 1,
        };

        let decide_err = format!("{}", decide_brain.judge_decision(&ctx).unwrap_err());
        let orient_err = format!(
            "{}",
            orient_brain.judge_orientation(&orient_ctx).unwrap_err()
        );

        assert_ne!(
            decide_err, orient_err,
            "different adapter_tags must produce different error messages"
        );
        assert!(decide_err.contains("recipe-decide-brain"));
        assert!(orient_err.contains("recipe-orient-brain"));
    }

    // ===================================================================
    // Security invariant: sanitize_context_var is used
    // ===================================================================
    // (These are structural contracts — the implementation must use
    // sanitize_context_var for all user-controlled context vars.
    // We can't unit-test this directly without subprocess mocking,
    // but the error-path tests above verify the subprocess plumbing
    // is wired through the correct code path.)

    // ===================================================================
    // Security invariant: truncate on stderr
    // ===================================================================
    // (Verified by the error messages being bounded. The implementation
    // must call truncate(&stderr, 500) on all error paths.)

    // ===================================================================
    // parse_action_from_text — first-word extraction (parsers eliminated)
    // ===================================================================

    mod parse_action_tests {
        use super::super::*;
        use crate::ooda_loop::ActionKind;

        // === First-word extraction: keyword as first word ===

        #[test]
        fn first_word_advance_goal() {
            let j = parse_action_from_text("advance_goal this is a code change");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn first_word_consolidate_memory() {
            let j = parse_action_from_text("consolidate_memory reduce context overhead");
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn first_word_run_improvement() {
            let j = parse_action_from_text("run_improvement code quality needs work");
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn first_word_poll_developer_activity() {
            let j = parse_action_from_text("poll_developer_activity check recent commits");
            assert_eq!(j.action_kind(), ActionKind::PollDeveloperActivity);
        }

        #[test]
        fn first_word_extract_ideas() {
            let j = parse_action_from_text("extract_ideas from codebase analysis");
            assert_eq!(j.action_kind(), ActionKind::ExtractIdeas);
        }

        #[test]
        fn first_word_safe_update() {
            let j = parse_action_from_text("safe_update binary is behind origin");
            assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
        }

        #[test]
        fn first_word_research_query() {
            let j = parse_action_from_text("research_query need more context on API");
            assert_eq!(j.action_kind(), ActionKind::ResearchQuery);
        }

        #[test]
        fn first_word_run_gym_eval() {
            let j = parse_action_from_text("run_gym_eval low scores warrant evaluation");
            assert_eq!(j.action_kind(), ActionKind::RunGymEval);
        }

        #[test]
        fn first_word_build_skill() {
            let j = parse_action_from_text("build_skill agent needs new capabilities");
            assert_eq!(j.action_kind(), ActionKind::BuildSkill);
        }

        #[test]
        fn first_word_launch_session() {
            let j = parse_action_from_text("launch_session start working on this task");
            assert_eq!(j.action_kind(), ActionKind::LaunchSession);
        }

        #[test]
        fn all_ten_keywords_as_first_word() {
            let cases = vec![
                ("advance_goal rest", ActionKind::AdvanceGoal),
                ("consolidate_memory rest", ActionKind::ConsolidateMemory),
                ("run_improvement rest", ActionKind::RunImprovement),
                (
                    "poll_developer_activity rest",
                    ActionKind::PollDeveloperActivity,
                ),
                ("extract_ideas rest", ActionKind::ExtractIdeas),
                ("safe_update rest", ActionKind::SafeUpdate),
                ("research_query rest", ActionKind::ResearchQuery),
                ("run_gym_eval rest", ActionKind::RunGymEval),
                ("build_skill rest", ActionKind::BuildSkill),
                ("launch_session rest", ActionKind::LaunchSession),
            ];
            for (text, expected) in cases {
                let j = parse_action_from_text(text);
                assert_eq!(
                    j.action_kind(),
                    expected,
                    "first word of '{text}' should map to {expected:?}"
                );
            }
        }

        // === Case insensitivity on first word ===

        #[test]
        fn first_word_case_insensitive_upper() {
            let j = parse_action_from_text("CONSOLIDATE_MEMORY reduce overhead");
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn first_word_case_insensitive_mixed() {
            let j = parse_action_from_text("Run_Improvement code quality");
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn first_word_case_insensitive_all_caps() {
            let j = parse_action_from_text("ADVANCE_GOAL proceed with goal");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Default behavior ===

        #[test]
        fn no_keyword_first_word_defaults_to_advance_goal() {
            let j = parse_action_from_text("I think the goal should proceed normally.");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
            assert!(
                j.rationale().contains("no action keyword"),
                "rationale should explain default: {}",
                j.rationale()
            );
        }

        #[test]
        fn empty_text_defaults_to_advance_goal() {
            let j = parse_action_from_text("");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
            assert!(j.rationale().contains("no action keyword"));
        }

        #[test]
        fn whitespace_only_defaults_to_advance_goal() {
            let j = parse_action_from_text("   \n\t  ");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Keyword NOT first word => default (new behavior) ===

        #[test]
        fn keyword_not_first_word_defaults_to_advance_goal() {
            // With first-word extraction, keywords buried in prose don't match
            let j = parse_action_from_text("I recommend consolidate_memory for this.");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn keyword_at_end_defaults_to_advance_goal() {
            let j = parse_action_from_text("The action should be safe_update");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn keyword_in_multiline_prose_defaults_to_advance_goal() {
            let text =
                "Looking at the state:\n- Memory fragmented\n\nRecommend: consolidate_memory";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Rationale ===

        #[test]
        fn rationale_contains_remaining_text() {
            let j = parse_action_from_text("run_improvement because code quality is poor");
            assert!(
                j.rationale().contains("code quality"),
                "rationale should contain text after keyword: {}",
                j.rationale()
            );
        }

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("consolidate_memory because {}", "x".repeat(1000));
            let j = parse_action_from_text(&long_text);
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
            assert!(
                j.rationale().chars().count() <= MAX_RATIONALE_CHARS + 1,
                "rationale should be truncated to ~{} chars (+1 for ellipsis), got {}",
                MAX_RATIONALE_CHARS,
                j.rationale().chars().count()
            );
        }

        #[test]
        fn first_word_only_no_rationale_text() {
            let j = parse_action_from_text("advance_goal");
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        // === Leading whitespace ===

        #[test]
        fn leading_whitespace_trimmed() {
            let j = parse_action_from_text("  safe_update binary is behind");
            assert_eq!(j.action_kind(), ActionKind::SafeUpdate);
        }

        #[test]
        fn leading_newline_trimmed() {
            let j = parse_action_from_text("\n\nrun_gym_eval scores are low");
            assert_eq!(j.action_kind(), ActionKind::RunGymEval);
        }

        // === No keyword is a substring of another (structural) ===

        #[test]
        fn no_keyword_is_substring_of_another() {
            let keywords = [
                "advance_goal",
                "consolidate_memory",
                "run_improvement",
                "poll_developer_activity",
                "extract_ideas",
                "safe_update",
                "research_query",
                "run_gym_eval",
                "build_skill",
                "launch_session",
            ];
            for (i, a) in keywords.iter().enumerate() {
                for (j, b) in keywords.iter().enumerate() {
                    if i != j {
                        assert!(!a.contains(b), "keyword '{a}' contains '{b}'");
                    }
                }
            }
        }

        // === Realistic LLM outputs (new format: keyword first) ===

        #[test]
        fn realistic_advance_goal() {
            let text = "advance_goal — standard development, recent commits show progress";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }

        #[test]
        fn realistic_consolidate_memory() {
            let text = "consolidate_memory\nMemory compaction will reduce context overhead.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::ConsolidateMemory);
        }

        #[test]
        fn realistic_run_improvement() {
            let text = "run_improvement code quality metrics are below threshold";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::RunImprovement);
        }

        #[test]
        fn realistic_no_keyword_just_prose() {
            let text = "The goal appears to be making steady progress.";
            let j = parse_action_from_text(text);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
        }
    }

    // ===================================================================
    // parse_orient_from_text — bare-float + floor (JSON tier eliminated)
    // ===================================================================

    mod parse_orient_tests {
        use super::super::*;

        // === Bare float extraction ===

        #[test]
        fn bare_float_alone() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        #[test]
        fn bare_float_with_rationale() {
            let text = "0.35 transient failure, moderate demotion";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn bare_float_in_prose() {
            let text = "The adjusted urgency should be 0.35 given failures.";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.35).abs() < 1e-9);
        }

        #[test]
        fn bare_float_at_end() {
            let text = "result: 0.50";
            let j = parse_orient_from_text(text, 0.8, 1);
            assert!((j.adjusted_urgency - 0.50).abs() < 1e-9);
        }

        #[test]
        fn bare_float_zero() {
            let j = parse_orient_from_text("0.0", 0.8, 4);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn bare_float_confidence_always_one() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn bare_float_demotion_computed() {
            let j = parse_orient_from_text("0.42", 0.8, 1);
            let expected = 0.8 - 0.42;
            assert!((j.demotion_applied - expected).abs() < 1e-9);
        }

        #[test]
        fn bare_float_rationale_includes_text() {
            let text = "0.35 because of transient failures";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!(j.rationale.contains("transient") || j.rationale.contains("0.35"));
        }

        // === Clamping: float above base_urgency or out of range ===

        #[test]
        fn float_above_base_clamped_to_base() {
            let j = parse_orient_from_text("0.9", 0.5, 1);
            assert!(
                j.adjusted_urgency <= 0.5 + 1e-9,
                "urgency {} should be clamped to base 0.5",
                j.adjusted_urgency
            );
        }

        #[test]
        fn float_above_one_clamped() {
            let j = parse_orient_from_text("1.5", 0.8, 1);
            assert!(j.adjusted_urgency <= 1.0 + 1e-9);
            assert!(j.adjusted_urgency <= 0.8 + 1e-9);
        }

        #[test]
        fn float_negative_not_matched_falls_to_floor() {
            // "-0.3" — the scanner starts at digits, sees '0.3' after the minus
            // The minus is not part of the pattern. Scanner finds 0.3 as a bare float.
            let j = parse_orient_from_text("-0.3", 0.8, 2);
            assert!(
                (j.adjusted_urgency - 0.3).abs() < 1e-9
                    || (j.adjusted_urgency - (0.8_f64 - 0.4).max(0.0)).abs() < 1e-9,
            );
        }

        // === No float => deterministic floor ===

        #[test]
        fn no_float_falls_to_floor() {
            let j = parse_orient_from_text("cannot determine urgency", 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn empty_string_falls_to_floor() {
            let j = parse_orient_from_text("", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn whitespace_only_falls_to_floor() {
            let j = parse_orient_from_text("   \n\t  ", 0.8, 2);
            let floor = (0.8_f64 - 0.4).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn integer_not_matched_falls_to_floor() {
            let j = parse_orient_from_text("42", 0.8, 2);
            let floor = (0.8_f64 - 0.2 * 2.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn dot_five_no_leading_digit_falls_to_floor() {
            // ".5" has no leading digit — the scanner requires N.N format
            let j = parse_orient_from_text(".5", 0.8, 1);
            let floor = (0.8_f64 - 0.2).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn one_dot_no_trailing_digit_falls_to_floor() {
            // "1." has no trailing digit — the scanner requires digits after dot
            let j = parse_orient_from_text("1.", 0.8, 1);
            let floor = (0.8_f64 - 0.2).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }

        #[test]
        fn multi_float_first_valid_wins() {
            // "version 2.0 adjusted to 0.42" — 2.0 is out of range, scanner skips to 0.42
            let j = parse_orient_from_text("version 2.0 adjusted to 0.42", 0.8, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        #[test]
        fn first_valid_float_in_range_wins() {
            // Both 0.9 and 0.42 are valid, but 0.9 > base 0.5, so scanner skips to 0.42
            let j = parse_orient_from_text("0.9 or 0.42", 0.5, 1);
            assert!((j.adjusted_urgency - 0.42).abs() < 1e-9);
        }

        // === Floor formula ===

        #[test]
        fn floor_formula_basic() {
            let j = parse_orient_from_text("no number here", 0.8, 2);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_clamped_to_zero() {
            let j = parse_orient_from_text("nothing", 0.3, 5);
            assert!(j.adjusted_urgency.abs() < 1e-9);
        }

        #[test]
        fn floor_zero_failures() {
            let j = parse_orient_from_text("nothing", 1.0, 0);
            assert!((j.adjusted_urgency - 1.0).abs() < 1e-9);
        }

        #[test]
        fn floor_one_failure() {
            let j = parse_orient_from_text("nothing", 0.6, 1);
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_demotion_applied() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.demotion_applied - 0.4).abs() < 1e-9);
        }

        #[test]
        fn floor_confidence_one() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!((j.confidence - 1.0).abs() < 1e-9);
        }

        #[test]
        fn floor_rationale_describes_formula() {
            let j = parse_orient_from_text("nothing", 0.8, 2);
            assert!(
                j.rationale.contains(ORIENT_ADAPTER_TAG) || j.rationale.contains("deterministic"),
                "floor rationale must identify the adapter or strategy; got: {}",
                j.rationale
            );
        }

        // === Invariants ===

        #[test]
        fn adjusted_always_le_base() {
            let scenarios: &[(&str, f64, u32)] =
                &[("0.9", 0.5, 1), ("0.42", 0.8, 1), ("nothing", 0.8, 2)];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(
                    j.adjusted_urgency <= *base + 1e-9,
                    "text={text} base={base}: urgency {} should be <= base",
                    j.adjusted_urgency
                );
            }
        }

        #[test]
        fn adjusted_always_in_unit_range() {
            let scenarios: &[(&str, f64, u32)] =
                &[("1.5", 0.8, 1), ("0.42", 0.8, 1), ("nothing", 0.8, 2)];
            for (text, base, failures) in scenarios {
                let j = parse_orient_from_text(text, *base, *failures);
                assert!(
                    j.adjusted_urgency >= 0.0 && j.adjusted_urgency <= 1.0,
                    "text={text}: urgency {} should be in [0,1]",
                    j.adjusted_urgency
                );
            }
        }

        // === Rationale ===

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("0.42 because {}", "x".repeat(1000));
            let j = parse_orient_from_text(&long_text, 0.8, 1);
            assert!(j.rationale.chars().count() <= MAX_RATIONALE_CHARS + 1);
        }

        // === No JSON extraction (parser eliminated) ===

        #[test]
        fn json_text_uses_bare_float_not_json_parser() {
            // JSON text with float inside — bare float scanner finds 0.4
            let text = r#"{"adjusted_urgency": 0.4, "rationale": "test"}"#;
            let j = parse_orient_from_text(text, 0.8, 2);
            // Should find 0.4 as first decimal float pattern
            assert!((j.adjusted_urgency - 0.4).abs() < 1e-9);
            // Rationale is full text, NOT the JSON "rationale" field
            assert!(
                j.rationale.contains("adjusted_urgency"),
                "rationale should be full text, not extracted JSON field; got: {}",
                j.rationale
            );
        }

        // === Matches deterministic fallback brain ===

        #[test]
        fn floor_matches_deterministic_fallback_brain() {
            use super::super::super::orient::OrientContext;
            use super::super::DeterministicOrientBrain;
            let ctx = OrientContext {
                goal_id: "g1".into(),
                base_urgency: 0.8,
                base_reason: "test".into(),
                failure_count: 3,
            };
            let fallback = DeterministicOrientBrain::compute(&ctx);
            let recipe_floor = parse_orient_from_text("nothing", 0.8, 3);
            assert!((recipe_floor.adjusted_urgency - fallback.adjusted_urgency).abs() < 1e-9);
        }

        // === Realistic outputs ===

        #[test]
        fn realistic_bare_float_first_token() {
            let text = "0.45 moderate demotion for transient CI failures";
            let j = parse_orient_from_text(text, 0.8, 2);
            assert!((j.adjusted_urgency - 0.45).abs() < 1e-9);
        }

        #[test]
        fn realistic_no_number() {
            let text = "Significantly demote due to chronic infrastructure failures.";
            let j = parse_orient_from_text(text, 0.8, 3);
            let floor = (0.8_f64 - 0.2 * 3.0).max(0.0);
            assert!((j.adjusted_urgency - floor).abs() < 1e-9);
        }
    }

    // ===================================================================
    // parse_lifecycle_from_text — first-word extraction (parsers eliminated)
    // ===================================================================

    mod parse_lifecycle_tests {
        use super::super::*;

        // === First-word extraction: variant as first word ===

        #[test]
        fn first_word_continue_skipping() {
            let text = "continue_skipping engineer is healthy and making progress";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(rationale.contains("healthy") || rationale.contains("progress"));
                }
                other => panic!("expected ContinueSkipping, got {other:?}"),
            }
        }

        #[test]
        fn first_word_reclaim_and_redispatch() {
            let text = "reclaim_and_redispatch worktree wedged for 7 hours";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ReclaimAndRedispatch {
                    rationale,
                    redispatch_context,
                } => {
                    assert!(rationale.contains("wedged"));
                    assert!(redispatch_context.is_empty());
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        #[test]
        fn first_word_deprioritize() {
            let text = "deprioritize chronic failure, no progress in 10 cycles";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("chronic") || rationale.contains("failure"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn first_word_open_tracking_issue() {
            let text = "open_tracking_issue engineer panicked on cycle 12, OOM in worker";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    rationale,
                    title,
                    body,
                } => {
                    assert_eq!(title, "OODA stuck");
                    assert!(body.contains("panicked") || body.contains("OOM"));
                    assert!(rationale.contains("panicked") || rationale.contains("OOM"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn first_word_mark_goal_blocked() {
            let text = "mark_goal_blocked needs API key from user, cannot proceed";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { rationale, reason } => {
                    assert!(reason.contains("API key") || reason.contains("cannot proceed"));
                    assert!(rationale.contains("API key") || rationale.contains("cannot proceed"));
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn first_word_consider_self_update() {
            let text = "consider_self_update binary is 5 commits behind origin/main";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ConsiderSelfUpdate { rationale } => {
                    assert!(rationale.contains("5 commits") || rationale.contains("behind"));
                }
                other => panic!("expected ConsiderSelfUpdate, got {other:?}"),
            }
        }

        // === Case insensitivity on first word ===

        #[test]
        fn first_word_case_insensitive_upper() {
            let text = "DEPRIORITIZE this stale goal";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("case-insensitive first word should match; got {other:?}"),
            }
        }

        #[test]
        fn first_word_case_insensitive_mixed() {
            let text = "Continue_Skipping everything is fine";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("case-insensitive first word should match; got {other:?}"),
            }
        }

        // === Default behavior ===

        #[test]
        fn no_keyword_first_word_defaults_to_continue_skipping() {
            let text = "The engineer appears to be making progress normally.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { rationale } => {
                    assert!(
                        rationale.contains("no decision keyword")
                            || rationale.contains(LIFECYCLE_ADAPTER_TAG),
                    );
                }
                other => panic!("no keyword first word -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn empty_text_defaults_to_continue_skipping() {
            let d = parse_lifecycle_from_text("");
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("empty text -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn whitespace_only_defaults_to_continue_skipping() {
            let d = parse_lifecycle_from_text("   \n\t  ");
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("whitespace-only -> ContinueSkipping; got {other:?}"),
            }
        }

        // === Keyword NOT first word => default (new behavior) ===

        #[test]
        fn keyword_in_prose_defaults_to_continue_skipping() {
            // With first-word extraction, keywords buried in prose don't match
            let text = "I think we should deprioritize this cycle.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("keyword not first word -> ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn old_marker_format_defaults_to_continue_skipping() {
            // Old DECISION: marker format no longer recognized
            let text = "DECISION: deprioritize\nRATIONALE: test";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("DECISION: marker should not be parsed; got {other:?}"),
            }
        }

        // === Extra fields use defaults ===

        #[test]
        fn open_tracking_issue_title_defaults_to_ooda_stuck() {
            let text = "open_tracking_issue something went wrong";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { title, .. } => {
                    assert_eq!(title, "OODA stuck");
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn open_tracking_issue_body_is_remaining_text() {
            let text = "open_tracking_issue engineer OOM on cycle 12";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue { body, .. } => {
                    assert!(body.contains("OOM") || body.contains("cycle"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn mark_goal_blocked_reason_is_remaining_text() {
            let text = "mark_goal_blocked needs API key from user";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::MarkGoalBlocked { reason, .. } => {
                    assert!(reason.contains("API key"));
                }
                other => panic!("expected MarkGoalBlocked, got {other:?}"),
            }
        }

        #[test]
        fn reclaim_redispatch_context_always_empty() {
            let text = "reclaim_and_redispatch wedged for hours";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ReclaimAndRedispatch {
                    redispatch_context, ..
                } => {
                    assert!(redispatch_context.is_empty());
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        // === Rationale ===

        #[test]
        fn rationale_is_remaining_text() {
            let text = "deprioritize chronic failure with no progress for many cycles";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.contains("chronic") || rationale.contains("failure"));
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        #[test]
        fn rationale_truncated_for_long_text() {
            let long_text = format!("deprioritize {}", "x".repeat(2000));
            let d = parse_lifecycle_from_text(&long_text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { rationale } => {
                    assert!(rationale.chars().count() <= MAX_RATIONALE_CHARS + 1);
                }
                other => panic!("expected Deprioritize, got {other:?}"),
            }
        }

        // === Leading whitespace ===

        #[test]
        fn leading_whitespace_trimmed() {
            let text = "  continue_skipping  everything is fine";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("leading whitespace should be trimmed; got {other:?}"),
            }
        }

        #[test]
        fn leading_newline_trimmed() {
            let text = "\n\ndeprioritize goal is stuck";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("leading newline should be trimmed; got {other:?}"),
            }
        }

        // === Realistic outputs (new format: keyword first) ===

        #[test]
        fn realistic_continue_skipping() {
            let text =
                "continue_skipping\nThe engineer is making steady progress. Last commit 15min ago.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("expected ContinueSkipping; got {other:?}"),
            }
        }

        #[test]
        fn realistic_deprioritize() {
            let text = "deprioritize goal stuck for 10 cycles, redirect attention";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::Deprioritize { .. } => {}
                other => panic!("expected Deprioritize; got {other:?}"),
            }
        }

        #[test]
        fn realistic_open_tracking_issue() {
            let text =
                "open_tracking_issue\nEngineer OOM at 03:14 UTC. Recurring — needs investigation.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::OpenTrackingIssue {
                    title,
                    body,
                    rationale,
                } => {
                    assert_eq!(title, "OODA stuck");
                    assert!(body.contains("OOM") || body.contains("investigation"));
                    assert!(rationale.contains("OOM") || rationale.contains("investigation"));
                }
                other => panic!("expected OpenTrackingIssue, got {other:?}"),
            }
        }

        #[test]
        fn realistic_no_decision() {
            let text = "The engineer seems to be working fine. I see recent commits.";
            let d = parse_lifecycle_from_text(text);
            match &d {
                EngineerLifecycleDecision::ContinueSkipping { .. } => {}
                other => panic!("no keyword -> ContinueSkipping; got {other:?}"),
            }
        }

        // === Sentinel/minutes helper tests (kept from original) ===

        #[test]
        fn sentinel_pid_none_renders_as_none_tag() {
            let sentinel: Option<i32> = None;
            let rendered = sentinel
                .map(|p| p.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            assert_eq!(rendered, "<none>");
        }

        #[test]
        fn minutes_max_renders_as_never() {
            let minutes = u64::MAX;
            let rendered = if minutes == u64::MAX {
                "never".to_string()
            } else {
                minutes.to_string()
            };
            assert_eq!(rendered, "never");
        }

        #[test]
        fn minutes_normal_renders_as_number() {
            let minutes: u64 = 42;
            let rendered = if minutes == u64::MAX {
                "never".to_string()
            } else {
                minutes.to_string()
            };
            assert_eq!(rendered, "42");
        }
    }

    // ===================================================================
    // Issue #2419 — outcome classification, JSON envelope extraction, and
    // the brain_lifecycle_decision metric context.
    // ===================================================================

    mod issue_2419_tests {
        use super::super::*;
        use crate::ooda_brain::EngineerLifecycleCtx;
        use std::path::PathBuf;

        fn sample_ctx() -> EngineerLifecycleCtx {
            EngineerLifecycleCtx {
                goal_id: "fix-the-thing".into(),
                goal_description: "desc".into(),
                cycle_number: 7,
                consecutive_skip_count: 12,
                failure_count: 0,
                worktree_path: PathBuf::from("/tmp/wt"),
                worktree_mtime_secs_ago: 60,
                sentinel_pid: Some(42),
                last_engineer_log_tail: "tail".into(),
                commits_behind: 0,
                in_flight_engineer_count: 1,
                minutes_since_last_update_attempt: u64::MAX,
            }
        }

        // --- Outcome branch 1: parsed (happy-path keyword extraction) ---

        #[test]
        fn outcome_parsed_real_decision() {
            let (decision, outcome) =
                parse_lifecycle_outcome("reclaim_and_redispatch worktree idle 7h, log truncated");
            assert_eq!(outcome, LifecycleParseOutcome::Parsed);
            assert!(!outcome.is_parse_failure());
            assert_eq!(outcome.label(), "parsed");
            match decision {
                EngineerLifecycleDecision::ReclaimAndRedispatch { rationale, .. } => {
                    assert!(rationale.contains("idle"));
                }
                other => panic!("expected ReclaimAndRedispatch, got {other:?}"),
            }
        }

        #[test]
        fn outcome_parsed_for_every_variant() {
            let cases = [
                ("continue_skipping healthy", "continue_skipping"),
                ("reclaim_and_redispatch wedged", "reclaim_and_redispatch"),
                ("deprioritize stale", "deprioritize"),
                ("open_tracking_issue panic", "open_tracking_issue"),
                ("mark_goal_blocked no key", "mark_goal_blocked"),
                ("consider_self_update behind", "consider_self_update"),
            ];
            for (text, choice) in cases {
                let (decision, outcome) = parse_lifecycle_outcome(text);
                assert_eq!(
                    outcome,
                    LifecycleParseOutcome::Parsed,
                    "'{text}' must classify as parsed"
                );
                assert_eq!(lifecycle_decision_choice(&decision), choice);
            }
        }

        // --- Outcome branch 2: default_empty ---

        #[test]
        fn outcome_default_empty_for_empty_string() {
            let (decision, outcome) = parse_lifecycle_outcome("");
            assert_eq!(outcome, LifecycleParseOutcome::DefaultEmpty);
            assert!(outcome.is_parse_failure());
            assert_eq!(outcome.label(), "default_empty");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
        }

        #[test]
        fn outcome_default_empty_for_whitespace_only() {
            let (_, outcome) = parse_lifecycle_outcome("   \n\t  ");
            assert_eq!(outcome, LifecycleParseOutcome::DefaultEmpty);
        }

        // --- Outcome branch 3: default_malformed ---

        #[test]
        fn outcome_default_malformed_for_unknown_first_word() {
            let (decision, outcome) = parse_lifecycle_outcome("OK the engineer looks fine to me");
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert!(outcome.is_parse_failure());
            assert_eq!(outcome.label(), "default_malformed");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
        }

        #[test]
        fn outcome_default_malformed_for_text_mode_banner_regression() {
            // This is the EXACT shape recipe-runner-rs emits to stdout in its
            // default `text` mode. Before issue #2419 the brain parsed this
            // banner directly, so the first word was always "Recipe:" → every
            // decision silently defaulted. This regression test pins that the
            // banner classifies as a parse failure (so the metric counts it),
            // and the JSON-envelope fix below proves the real decision is
            // recovered instead.
            let banner = "Recipe: ooda-engineer-lifecycle (v1.0.0)\nSteps: 1\n\n\
                          Recipe 'ooda-engineer-lifecycle': SUCCESS (0.0s)\n  \
                          [completed] engineer-lifecycle-decision (0.0s)\n";
            let (decision, outcome) = parse_lifecycle_outcome(banner);
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert_eq!(lifecycle_first_word(banner), "Recipe:");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
        }

        // --- Outcome branch 4: error (label + numerator semantics) ---

        #[test]
        fn outcome_error_label_and_failure_semantics() {
            assert_eq!(LifecycleParseOutcome::Error.label(), "error");
            assert!(LifecycleParseOutcome::Error.is_parse_failure());
        }

        // --- JSON envelope extraction (the root-cause fix) ---

        #[test]
        fn envelope_extraction_recovers_real_decision() {
            // A realistic --output-format json envelope. The decision text the
            // banner hid is in step_results[].output; extracting it and parsing
            // yields a real (non-default) decision.
            let json = r#"{
                "recipe_name": "ooda-engineer-lifecycle",
                "success": true,
                "step_results": [
                    {"step_id": "engineer-lifecycle-decision",
                     "output": "reclaim_and_redispatch worktree idle 7h",
                     "error": "", "duration": 0.01}
                ],
                "context": {"lifecycle_result": "reclaim_and_redispatch worktree idle 7h"}
            }"#;
            let extracted =
                extract_recipe_decision_output(json.as_bytes(), LIFECYCLE_ADAPTER_TAG).unwrap();
            assert_eq!(extracted, "reclaim_and_redispatch worktree idle 7h");
            let (_, outcome) = parse_lifecycle_outcome(&extracted);
            assert_eq!(
                outcome,
                LifecycleParseOutcome::Parsed,
                "the JSON-envelope fix must recover a parseable decision"
            );
        }

        #[test]
        fn envelope_extraction_uses_final_step() {
            // Multi-step recipe: the decision is the terminal step's output.
            let json = r#"{
                "success": true,
                "step_results": [
                    {"step_id": "pre", "output": "prelude noise", "error": "", "duration": 0.0},
                    {"step_id": "decide", "output": "deprioritize stale goal", "error": "", "duration": 0.0}
                ]
            }"#;
            let extracted =
                extract_recipe_decision_output(json.as_bytes(), LIFECYCLE_ADAPTER_TAG).unwrap();
            assert_eq!(extracted, "deprioritize stale goal");
        }

        #[test]
        fn envelope_extraction_errors_on_success_false() {
            let json = r#"{"success": false, "step_results": []}"#;
            let err =
                extract_recipe_decision_output(json.as_bytes(), LIFECYCLE_ADAPTER_TAG).unwrap_err();
            assert!(matches!(err, SimardError::AdapterInvocationFailed { .. }));
        }

        #[test]
        fn envelope_extraction_errors_on_empty_step_results() {
            let json = r#"{"success": true, "step_results": []}"#;
            let err =
                extract_recipe_decision_output(json.as_bytes(), LIFECYCLE_ADAPTER_TAG).unwrap_err();
            assert!(matches!(err, SimardError::AdapterInvocationFailed { .. }));
        }

        #[test]
        fn envelope_extraction_errors_on_garbage() {
            // The text-mode banner is NOT valid JSON — decoding must fail
            // loudly (error outcome) rather than be mistaken for empty output.
            let banner = b"Recipe: x\nSUCCESS\n";
            let err = extract_recipe_decision_output(banner, LIFECYCLE_ADAPTER_TAG).unwrap_err();
            assert!(matches!(err, SimardError::AdapterInvocationFailed { .. }));
        }

        // --- Metric context payload ---

        #[test]
        fn metric_context_has_required_fields() {
            let ctx = sample_ctx();
            let payload = build_lifecycle_metric_context(
                &ctx,
                LifecycleParseOutcome::Parsed,
                "reclaim_and_redispatch",
                "reclaim_and_redispatch",
                "ok",
                1,
            );
            let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
            assert_eq!(v["goal_id"], "fix-the-thing");
            assert_eq!(v["outcome"], "parsed");
            assert_eq!(v["is_parse_failure"], false);
            assert_eq!(v["first_word"], "reclaim_and_redispatch");
            assert_eq!(v["consecutive_skip_count"], 12);
            assert_eq!(v["decision"], "reclaim_and_redispatch");
            assert_eq!(v["cause"], "ok");
            assert_eq!(v["attempts"], 1);
        }

        #[test]
        fn metric_context_marks_failures() {
            let ctx = sample_ctx();
            for outcome in [
                LifecycleParseOutcome::DefaultEmpty,
                LifecycleParseOutcome::DefaultMalformed,
                LifecycleParseOutcome::Error,
            ] {
                let payload =
                    build_lifecycle_metric_context(&ctx, outcome, "", "continue_skipping", "ok", 1);
                let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
                assert_eq!(v["outcome"], outcome.label());
                assert_eq!(
                    v["is_parse_failure"],
                    true,
                    "{} must count toward the parse-failure numerator",
                    outcome.label()
                );
            }
        }

        #[test]
        fn first_word_is_bounded() {
            let huge = format!("{} rest of response", "z".repeat(500));
            let fw = lifecycle_first_word(&huge);
            assert!(fw.chars().count() <= METRIC_FIRST_WORD_CHARS + 1);
        }
    }

    // ===================================================================
    // Issue #2432 — confidence-gated escalation ladder (schema-repair +
    // higher-effort tier bump) replacing the zero-retry parse→default.
    // ===================================================================

    mod issue_2432_tests {
        use super::super::*;
        use crate::ooda_brain::EngineerLifecycleCtx;
        use std::collections::VecDeque;
        use std::path::PathBuf;
        use std::sync::Mutex;

        fn sample_ctx() -> EngineerLifecycleCtx {
            EngineerLifecycleCtx {
                goal_id: "fix-the-thing".into(),
                goal_description: "desc".into(),
                cycle_number: 7,
                consecutive_skip_count: 12,
                failure_count: 0,
                worktree_path: PathBuf::from("/tmp/wt"),
                worktree_mtime_secs_ago: 60,
                sentinel_pid: Some(42),
                last_engineer_log_tail: "tail".into(),
                commits_behind: 0,
                in_flight_engineer_count: 1,
                minutes_since_last_update_attempt: u64::MAX,
            }
        }

        /// A scripted [`LifecycleInvoker`]: each call pops the next queued
        /// response (`Ok(text)` or a synthesized adapter error) and records the
        /// rung + rendered escalation note it was asked for.
        struct ScriptedInvoker {
            responses: Mutex<VecDeque<Result<String, ()>>>,
            seen_rungs: Mutex<Vec<LadderRung>>,
            seen_notes: Mutex<Vec<String>>,
        }

        impl ScriptedInvoker {
            fn new(responses: Vec<Result<String, ()>>) -> Self {
                Self {
                    responses: Mutex::new(responses.into_iter().collect()),
                    seen_rungs: Mutex::new(Vec::new()),
                    seen_notes: Mutex::new(Vec::new()),
                }
            }
            fn rungs(&self) -> Vec<LadderRung> {
                self.seen_rungs.lock().unwrap().clone()
            }
            fn notes(&self) -> Vec<String> {
                self.seen_notes.lock().unwrap().clone()
            }
        }

        impl LifecycleInvoker for ScriptedInvoker {
            fn invoke_lifecycle(
                &self,
                _ctx: &EngineerLifecycleCtx,
                attempt: &LadderAttempt,
            ) -> SimardResult<String> {
                self.seen_rungs.lock().unwrap().push(attempt.rung);
                self.seen_notes
                    .lock()
                    .unwrap()
                    .push(attempt.escalation_note());
                match self.responses.lock().unwrap().pop_front() {
                    Some(Ok(s)) => Ok(s),
                    Some(Err(())) => Err(SimardError::AdapterInvocationFailed {
                        base_type: "test".into(),
                        reason: "scripted invoke failure".into(),
                    }),
                    None => panic!("ScriptedInvoker called more times than scripted"),
                }
            }
        }

        // --- escalation note (pinned wording) ---

        #[test]
        fn escalation_note_base_is_empty() {
            assert_eq!(build_escalation_note(LadderRung::Base, "anything"), "");
        }

        #[test]
        fn escalation_note_schema_repair_pins_wording() {
            let note = build_escalation_note(LadderRung::SchemaRepair, "OK looks fine to me");
            assert!(note.contains("SCHEMA REPAIR"), "note: {note}");
            assert!(
                note.contains("FIRST WORD"),
                "note must remind about first word"
            );
            assert!(
                note.contains("continue_skipping")
                    && note.contains("reclaim_and_redispatch")
                    && note.contains("consider_self_update"),
                "note must echo the full variant list"
            );
            assert!(
                note.contains("OK looks fine to me"),
                "note must feed the malformed prior output back"
            );
        }

        #[test]
        fn escalation_note_escalate_adds_high_effort() {
            let note = build_escalation_note(LadderRung::Escalate, "junk");
            assert!(note.contains("SCHEMA REPAIR"), "escalate still repairs");
            assert!(
                note.contains("HIGH-EFFORT"),
                "escalate adds the effort tier"
            );
        }

        // --- outcome semantics ---

        #[test]
        fn repaired_and_escalated_are_not_parse_failures() {
            assert_eq!(LifecycleParseOutcome::Repaired.label(), "repaired");
            assert_eq!(LifecycleParseOutcome::Escalated.label(), "escalated");
            assert!(!LifecycleParseOutcome::Repaired.is_parse_failure());
            assert!(!LifecycleParseOutcome::Escalated.is_parse_failure());
        }

        // --- config bound ---

        #[test]
        fn config_parse_defaults_and_clamps() {
            assert_eq!(
                parse_max_escalations(None),
                EscalationConfig::DEFAULT_MAX_ESCALATIONS
            );
            assert_eq!(
                parse_max_escalations(Some("garbage")),
                EscalationConfig::DEFAULT_MAX_ESCALATIONS
            );
            assert_eq!(parse_max_escalations(Some("0")), 0);
            assert_eq!(parse_max_escalations(Some("1")), 1);
            // Hard cap: no configuration can produce an unbounded loop.
            assert_eq!(
                parse_max_escalations(Some("99")),
                EscalationConfig::HARD_CAP
            );
            assert_eq!(parse_max_escalations(Some("  2 ")), 2);
        }

        // --- ladder behaviour ---

        /// A parse-miss recovered by the FIRST (schema-repair) rung yields a
        /// real decision, the `Repaired` outcome, and exactly 2 invocations
        /// (base + 1 rung).
        #[test]
        fn ladder_recovers_via_schema_repair() {
            let invoker =
                ScriptedInvoker::new(vec![Ok("reclaim_and_redispatch worktree idle 7h".into())]);
            let (decision, outcome, attempts) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "OK the engineer looks fine", // base parse-miss text
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::Repaired);
            assert!(!outcome.is_parse_failure());
            assert_eq!(attempts, 2, "base + one schema-repair rung");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ReclaimAndRedispatch { .. }
            ));
            assert_eq!(invoker.rungs(), vec![LadderRung::SchemaRepair]);
        }

        /// A parse-miss that survives schema-repair is recovered by the SECOND
        /// (higher-effort) rung → `Escalated`, 3 invocations.
        #[test]
        fn ladder_escalates_to_second_rung() {
            let invoker = ScriptedInvoker::new(vec![
                Ok("still not a variant word".into()), // schema-repair misses
                Ok("deprioritize stale goal".into()),  // escalate recovers
            ]);
            let (decision, outcome, attempts) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "garbage",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::Escalated);
            assert_eq!(attempts, 3, "base + schema-repair + escalate");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::Deprioritize { .. }
            ));
            assert_eq!(
                invoker.rungs(),
                vec![LadderRung::SchemaRepair, LadderRung::Escalate]
            );
        }

        /// Bounded cap: when every rung still parse-misses, the ladder is
        /// exhausted and falls to the deterministic default — and never
        /// invokes more than `max_escalations` rungs.
        #[test]
        fn ladder_bounded_cap_exhausts_to_default() {
            let invoker = ScriptedInvoker::new(vec![Ok("nope".into()), Ok("still nope".into())]);
            let (decision, outcome, attempts) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "banner noise",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(), // 2 rungs
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert!(outcome.is_parse_failure());
            assert_eq!(attempts, 3, "base + exactly 2 bounded rungs");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
            assert_eq!(invoker.rungs().len(), 2, "no more than the configured cap");
        }

        /// Ladder disabled (`max_escalations == 0`): no escalation invocations,
        /// straight to default — the pre-#2432 behaviour.
        #[test]
        fn ladder_disabled_runs_no_escalations() {
            let invoker = ScriptedInvoker::new(vec![]);
            let (decision, outcome, attempts) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "banner noise",
                LifecycleParseOutcome::DefaultEmpty,
                &EscalationConfig { max_escalations: 0 },
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultEmpty);
            assert_eq!(attempts, 1, "only the base attempt");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
            assert!(invoker.rungs().is_empty(), "no rungs invoked when disabled");
        }

        /// An escalation invocation that itself FAILS must not surface as a hard
        /// error — the ladder stops and uses the deterministic default (a base
        /// success already gave us a usable, if low-confidence, signal).
        #[test]
        fn ladder_invoke_error_falls_back_to_default() {
            let invoker = ScriptedInvoker::new(vec![Err(())]);
            let (decision, outcome, attempts) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "garbage",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert_eq!(attempts, 2, "base + the failed rung, then stop");
            assert!(matches!(
                decision,
                EngineerLifecycleDecision::ContinueSkipping { .. }
            ));
            assert_eq!(invoker.rungs(), vec![LadderRung::SchemaRepair]);
        }

        /// The schema-repair rung feeds the exact malformed prior output back
        /// into its note so the model can fix it.
        #[test]
        fn ladder_feeds_prior_output_into_repair_note() {
            let invoker = ScriptedInvoker::new(vec![Ok("continue_skipping healthy".into())]);
            let cfg = EscalationConfig { max_escalations: 1 };
            let _ = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "the model rambled without a variant word",
                LifecycleParseOutcome::DefaultMalformed,
                &cfg,
            );
            let notes = invoker.notes();
            assert_eq!(notes.len(), 1);
            assert!(
                notes[0].contains("the model rambled without a variant word"),
                "repair note must carry the prior output; got {}",
                notes[0]
            );
        }
    }
}
