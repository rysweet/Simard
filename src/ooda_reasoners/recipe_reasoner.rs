//! Unified recipe-runner-backed brain — single struct [`RecipeReasoner`] that
//! implements all three OODA brain traits (`ActReasoner`, `DecideReasoner`,
//! `OrientReasoner`), parameterised by recipe filename and adapter tag.
//!
//! Consolidates the formerly separate `RecipeDecideReasoner`,
//! `RecipeOrientReasoner`, and `RecipeEngineerLifecycleReasoner` (issue #2132).
//! The principle: "one agent, one identity, one brain — different recipes
//! for different circumstances."
//!
//! Each trait impl invokes `recipe-runner-rs` as a subprocess with `-c`
//! context vars, then parses via trivial first-word / first-number
//! extractors (issue #2144 — no keyword scanners, no JSON extraction).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::decide::{DecideContext, DecideJudgment, DecideReasoner};
use super::orient::{
    FAILURE_PENALTY_PER_CONSECUTIVE, OrientContext, OrientJudgment, OrientReasoner,
};
use super::sanitize::sanitize_context_var;
use super::{ActReasoner, EngineerLifecycleCtx, EngineerLifecycleDecision, ReasonerPhase};
use crate::error::{SimardError, SimardResult};

#[cfg(test)]
use super::orient::DeterministicFallbackOrientReasoner;

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

/// Shared metric emitted once per recipe-backed brain phase invocation
/// (decide / orient / merge-judge) so the verdict/decision parse-success rate
/// is measurable across the whole class from `metrics.jsonl` (issue #2429).
///
/// `value` is always `1.0`; the context JSON carries `{phase, outcome, …}` where
/// `outcome` is `"parsed"` (a real verdict/decision was extracted — including a
/// ladder recovery) or `"defaulted"` (no parseable verdict; a deterministic
/// fallback was applied — the bug surface). The derived
/// `parse_success_rate{phase} = parsed / (parsed + defaulted)` is what the
/// monitoring path alerts on.
const VERDICT_PARSE_METRIC: &str = "brain_verdict_parsed_total";

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
pub(crate) fn extract_recipe_decision_output(
    stdout: &[u8],
    adapter_tag: &str,
) -> SimardResult<String> {
    let mut envelope: RecipeEnvelope =
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

    // `pop()` moves the terminal step's `output` out of the owned envelope
    // (dropped on return anyway) instead of cloning the (potentially multi-KB)
    // decision text on every brain invocation.
    envelope
        .step_results
        .pop()
        .map(|s| s.output)
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
    // Built lazily so the Base rung allocates nothing — base behaviour stays
    // byte-identical to pre-#2432.
    let schema_repair = || {
        let prior = truncate(prior_output.trim(), MAX_RATIONALE_CHARS);
        format!(
            "## ⚠️ SCHEMA REPAIR (retry) ## \
             Your previous response could not be parsed: its FIRST WORD was not a valid decision variant. \
             Previous response: <<<{prior}>>> \
             Respond again now. The VERY FIRST WORD of your reply MUST be exactly one of: {LIFECYCLE_VARIANT_LIST}. \
             Output that variant word first, then your rationale."
        )
    };
    match rung {
        LadderRung::Base => String::new(),
        LadderRung::SchemaRepair => schema_repair(),
        LadderRung::Escalate => format!(
            "{} ## HIGH-EFFORT RETRY ## \
             This is a final, higher-effort attempt. Reason carefully, step by step, about the \
             engineer's state BEFORE answering, then output the single variant word first.",
            schema_repair()
        ),
    }
}

/// Generic `escalation_note` builder shared by the decide / orient / merge-judge
/// phases (issue #2419 family / #2429). Same three-rung structure as the
/// lifecycle [`build_escalation_note`] — empty on `Base` (byte-identical base
/// behaviour), a schema-repair note on `SchemaRepair`, and schema-repair plus a
/// higher-effort tier on `Escalate` — but parameterised by the phase's own
/// output contract so each brain reminds the model of the correct shape (an
/// action word / a bare decimal / a `{"verdict": …}` JSON object).
///
/// `repair_instruction` describes the required output shape; `high_effort` is
/// the extra reasoning instruction appended on the final rung.
pub(crate) fn build_phase_escalation_note(
    rung: LadderRung,
    prior_output: &str,
    repair_instruction: &str,
    high_effort: &str,
) -> String {
    // Built lazily so the Base rung allocates nothing.
    let schema_repair = || {
        let prior = truncate(prior_output.trim(), MAX_RATIONALE_CHARS);
        format!(
            "## ⚠️ SCHEMA REPAIR (retry) ## \
             Your previous response could not be parsed. \
             Previous response: <<<{prior}>>> \
             Respond again now. {repair_instruction}"
        )
    };
    match rung {
        LadderRung::Base => String::new(),
        LadderRung::SchemaRepair => schema_repair(),
        LadderRung::Escalate => format!(
            "{} ## HIGH-EFFORT RETRY ## This is a final, higher-effort attempt. {high_effort}",
            schema_repair()
        ),
    }
}

/// Closed action-keyword list echoed into the decide schema-repair note. Kept in
/// sync with the `parse_action_outcome` match and the `ooda-decide.yaml` OPTIONS.
const DECIDE_VARIANT_LIST: &str = "poll_developer_activity, consolidate_memory, run_improvement, extract_ideas, safe_update, research_query, run_gym_eval, build_skill, launch_session, advance_goal";

/// Build the decide-phase `escalation_note` for a ladder rung (issue #2421).
fn build_decide_escalation_note(rung: LadderRung, prior_output: &str) -> String {
    build_phase_escalation_note(
        rung,
        prior_output,
        &format!(
            "The VERY FIRST WORD of your reply MUST be exactly one of: {DECIDE_VARIANT_LIST}. \
             Output that action word first, then your rationale."
        ),
        "Reason carefully about the goal_id and reason BEFORE answering, then output the single \
         action word first.",
    )
}

/// Build the orient-phase `escalation_note` for a ladder rung (issue #2421).
fn build_orient_escalation_note(rung: LadderRung, prior_output: &str) -> String {
    build_phase_escalation_note(
        rung,
        prior_output,
        "The VERY FIRST TOKEN of your reply MUST be a bare decimal number between 0.0 and the \
         base urgency (e.g. `0.42`) — the adjusted urgency. Output that decimal first, then your \
         rationale. Do NOT output a timing value or any other number first.",
        "Reason carefully about the failure history BEFORE answering, then output the single bare \
         decimal first.",
    )
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

/// Why the confidence-gated escalation ladder stopped. Drives a precise
/// `cause` label on the `brain_lifecycle_decision` metric so the three
/// non-recovery terminations are distinguishable in telemetry: a ladder that
/// genuinely ran out of rungs (`Exhausted`) reads differently from one cut
/// short because a rung's own invocation failed (`InvokeError`) or one that
/// was switched off by configuration (`Disabled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderTermination {
    /// A rung produced a parseable decision — `Repaired` / `Escalated`.
    Recovered,
    /// Every configured rung was tried; none parsed. Deterministic default.
    Exhausted,
    /// A rung's own invocation failed; the ladder stopped early and fell to
    /// the deterministic default (the successful base attempt already gave a
    /// usable, if low-confidence, signal — this is NOT a hard error).
    InvokeError,
    /// The ladder was disabled (`max_escalations == 0`); no rung was tried.
    Disabled,
}

impl LadderTermination {
    /// The `cause` label recorded on the lifecycle-decision metric.
    pub fn cause_label(self) -> &'static str {
        match self {
            LadderTermination::Recovered => "ladder_recovered",
            LadderTermination::Exhausted => "ladder_exhausted",
            LadderTermination::InvokeError => "ladder_invoke_error",
            LadderTermination::Disabled => "ladder_disabled",
        }
    }
}

/// Seam over the raw lifecycle recipe invocation so the escalation ladder is
/// unit-testable without a live `recipe-runner-rs`. Production wires
/// [`RecipeReasoner`]; tests wire a scripted stub. Returns the raw decision text
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
/// parse-miss), the total number of brain invocations made (base + rungs), and
/// the [`LadderTermination`] reason (so the caller can record a precise `cause`
/// — distinguishing true exhaustion from an early stop caused by a rung's own
/// invocation failure, or a disabled ladder).
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
) -> (
    EngineerLifecycleDecision,
    LifecycleParseOutcome,
    u32,
    LadderTermination,
) {
    // Thin lifecycle-specific wrapper over the generic [`run_brain_ladder`]
    // backbone (issue #2419 family / #2429): the lifecycle phase owns the
    // invoke (recipe-runner `LadderAttempt`), the parser
    // ([`parse_lifecycle_outcome`]), the deterministic default
    // ([`default_continue_skipping`]), and the decision-label closure; the
    // generic core owns the bounded rung loop, the loud logging, and the
    // [`LadderTermination`] accounting. This keeps the decide / orient /
    // merge-judge phases on the SAME ladder rather than reinventing it.
    run_brain_ladder(
        &ctx.goal_id,
        base_raw,
        base_outcome,
        cfg,
        |rung, prior| {
            let attempt = LadderAttempt {
                rung,
                prior_output: prior,
            };
            invoker.invoke_lifecycle(ctx, &attempt)
        },
        parse_lifecycle_outcome,
        default_continue_skipping,
        |d| lifecycle_decision_choice(d).to_string(),
    )
}

/// Generic confidence-gated escalation ladder backbone shared by every
/// recipe-backed brain phase (engineer-lifecycle, decide, orient, merge-judge —
/// issue #2419 family / #2429).
///
/// On a base parse-miss the phase spends EXTRA compute ONLY on that weak case: a
/// bounded sequence of re-prompts (schema-repair, then a higher-effort tier)
/// driven by `invoke(rung, prior_output)`. The deterministic, *loud* `default()`
/// is reached only AFTER every rung is exhausted (or a rung's own invocation
/// fails) — never a silent default-on-first-miss.
///
/// Phase-specific behaviour is injected via closures so the core stays
/// decision-type-agnostic:
/// - `invoke(rung, prior_output)` runs the recipe for a rung (it owns building
///   the phase's `escalation_note`) and returns the raw final-step output;
/// - `parse(raw)` classifies the output into `(decision, outcome)`; an outcome
///   for which `is_parse_failure()` holds drives escalation;
/// - `default()` is the deterministic fallback decision;
/// - `decision_label(&decision)` is a short tag used only for logging.
///
/// Returns the final decision, its outcome (`Repaired`/`Escalated` on recovery,
/// else the original parse-miss), the total brain invocations (base + rungs),
/// and the [`LadderTermination`] reason.
#[allow(clippy::too_many_arguments)]
pub fn run_brain_ladder<D>(
    goal_id: &str,
    base_raw: &str,
    base_outcome: LifecycleParseOutcome,
    cfg: &EscalationConfig,
    invoke: impl Fn(LadderRung, &str) -> SimardResult<String>,
    parse: impl Fn(&str) -> (D, LifecycleParseOutcome),
    default: impl Fn() -> D,
    decision_label: impl Fn(&D) -> String,
) -> (D, LifecycleParseOutcome, u32, LadderTermination) {
    let mut prior = base_raw.to_string();
    let mut attempts = 1u32; // the base attempt already happened
    let mut invoke_failed = false;

    for rung_idx in 1..=cfg.max_escalations {
        let rung = if rung_idx == 1 {
            LadderRung::SchemaRepair
        } else {
            LadderRung::Escalate
        };
        attempts += 1;

        tracing::warn!(
            target: "simard::ooda_reasoners",
            goal = %goal_id,
            rung = ?rung,
            attempt = attempts,
            base_outcome = base_outcome.label(),
            "brain decision parse-miss → escalating (confidence-gated ladder, issue #2432)"
        );
        eprintln!(
            "[simard] BRAIN ESCALATION goal={} rung={:?} attempt={} (parse-miss recovery)",
            goal_id, rung, attempts
        );

        match invoke(rung, &prior) {
            Err(e) => {
                tracing::warn!(
                    target: "simard::ooda_reasoners",
                    goal = %goal_id,
                    rung = ?rung,
                    error = %e,
                    "brain escalation attempt failed to invoke; stopping ladder, using deterministic default"
                );
                eprintln!(
                    "[simard] BRAIN ESCALATION goal={} rung={:?} invoke failed: {e} — falling back to default",
                    goal_id, rung
                );
                invoke_failed = true;
                break;
            }
            Ok(raw2) => {
                let (decision, oc) = parse(&raw2);
                if !oc.is_parse_failure() {
                    let recovered = match rung {
                        LadderRung::SchemaRepair => LifecycleParseOutcome::Repaired,
                        LadderRung::Escalate => LifecycleParseOutcome::Escalated,
                        LadderRung::Base => oc,
                    };
                    let label = decision_label(&decision);
                    tracing::info!(
                        target: "simard::ooda_reasoners",
                        goal = %goal_id,
                        rung = ?rung,
                        attempt = attempts,
                        decision = %label,
                        "brain decision RECOVERED via escalation ladder (issue #2432)"
                    );
                    eprintln!(
                        "[simard] BRAIN ESCALATION goal={} RECOVERED decision={} via {:?} (attempt {})",
                        goal_id, label, rung, attempts
                    );
                    return (decision, recovered, attempts, LadderTermination::Recovered);
                }
                // Still a parse-miss — feed the latest malformed output into the
                // next rung's repair note.
                prior = raw2;
            }
        }
    }

    // Ladder exhausted, disabled, or an escalation invoke failed: fall to the
    // deterministic default, preserving the original parse-miss outcome for the
    // metric numerator. The `termination` reason records *which* of those three
    // paths we took so the metric `cause` label stays accurate.
    let termination = if cfg.max_escalations == 0 {
        LadderTermination::Disabled
    } else if invoke_failed {
        LadderTermination::InvokeError
    } else {
        LadderTermination::Exhausted
    };
    if matches!(termination, LadderTermination::Exhausted) {
        // Issue #2528: the decide/orient escalation ladder was fully exhausted
        // with no parseable decision — surface it as a structured telemetry
        // signal (read by `simard status`) alongside the log lines below.
        crate::telemetry::counter_add(crate::telemetry::names::BRAIN_LADDER_EXHAUSTED, 1, &[]);
    }
    if cfg.max_escalations > 0 {
        tracing::warn!(
            target: "simard::ooda_reasoners",
            goal = %goal_id,
            attempts,
            base_outcome = base_outcome.label(),
            termination = ?termination,
            "brain escalation ladder ended without a parseable decision; deterministic default"
        );
        eprintln!(
            "[simard] BRAIN ESCALATION goal={} ladder ended ({}) after {attempts} attempts — deterministic default",
            goal_id,
            termination.cause_label()
        );
    }
    (default(), base_outcome, attempts, termination)
}

/// Resolve the recipe YAML path. Checks, in order:
///   1. `~/.simard/prompt_assets/simard/recipes/<recipe_filename>` (hot-reload)
///   2. `<repo_root>/prompt_assets/simard/recipes/<recipe_filename>` (in-tree)
///
/// `home_override` lets tests supply a fake home directory without mutating the
/// process-wide `HOME` env var (mirrors `disk_health::resolve_recipe_path`).
/// Production passes `None`, falling back to [`dirs::home_dir`].
pub fn resolve_recipe_path(
    repo_root: &Path,
    recipe_filename: &str,
    home_override: Option<&Path>,
) -> Option<PathBuf> {
    let home = home_override.map(PathBuf::from).or_else(dirs::home_dir);
    if let Some(home) = home {
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
pub struct RecipeReasoner {
    pub(crate) recipe_path: PathBuf,
    pub(crate) agent_binary: &'static str,
    pub(crate) adapter_tag: &'static str,
}

impl RecipeReasoner {
    /// Construct if recipe file and recipe-runner-rs binary are both available.
    ///
    /// `recipe_filename` selects the YAML (e.g. `"ooda-decide.yaml"`).
    /// `adapter_tag` appears in error messages and logs (e.g. `"recipe-decide-brain"`).
    pub fn new(repo_root: &Path, recipe_filename: &str, adapter_tag: &'static str) -> Option<Self> {
        Self::new_with_home(repo_root, recipe_filename, adapter_tag, None)
    }

    /// Like [`RecipeReasoner::new`], but accepts a `home_override` for the
    /// hot-reload lookup so tests stay hermetic against the ambient
    /// `~/.simard/prompt_assets` directory. Production calls `new` (`None`).
    fn new_with_home(
        repo_root: &Path,
        recipe_filename: &str,
        adapter_tag: &'static str,
        home_override: Option<&Path>,
    ) -> Option<Self> {
        let recipe_path = resolve_recipe_path(repo_root, recipe_filename, home_override)?;
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

impl DecideReasoner for RecipeReasoner {
    /// Decide-phase action routing.
    ///
    /// Issue #2421 / #2429: invoke `recipe-runner-rs --output-format json` and
    /// parse the agent's REAL decision text (the JSON envelope's final step
    /// output) — not the text-mode SUCCESS banner whose first word `Recipe:`
    /// always silently defaulted to `advance_goal`, ignoring the LLM every
    /// cycle. On a base parse-miss, spend extra compute on the confidence-gated
    /// escalation ladder (schema-repair → high-effort) before falling — loudly —
    /// to the deterministic `advance_goal` default. A `brain_verdict_parsed_total`
    /// metric is emitted on both the parsed and the defaulted branch.
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let invoke = |rung: LadderRung, prior: &str| self.invoke_decide_raw(ctx, rung, prior);

        // Base (cheap) attempt. A genuine recipe-runner failure (spawn / nonzero
        // exit / envelope decode) still surfaces loudly as `Err` — only a
        // *parse-miss* on a successful run is low-confidence enough to escalate.
        let base_raw = invoke(LadderRung::Base, "")?;
        let (decision, outcome) = parse_action_outcome(&base_raw);
        if !outcome.is_parse_failure() {
            record_verdict_parse_metric(ReasonerPhase::Decide, &ctx.goal_id, outcome, "ok", 1);
            crate::recipe_output::record_parse_outcome("decide", true);
            return Ok(decision);
        }

        let cfg = EscalationConfig::from_env();
        let (final_decision, final_outcome, attempts, termination) = run_brain_ladder(
            &ctx.goal_id,
            &base_raw,
            outcome,
            &cfg,
            invoke,
            parse_action_outcome,
            default_advance_goal,
            |d| decide_decision_choice(d).to_string(),
        );
        // issue #2496: a parse-failure default here is the production-deadlock
        // surface — log it distinctly so it is never read as a real LLM
        // `advance_goal`/no-new-action decision, and attribute it to its precise
        // `LadderTermination` cause (previously discarded as `_termination`).
        if final_outcome.is_parse_failure() {
            warn_parse_failure_default(
                ReasonerPhase::Decide,
                &ctx.goal_id,
                final_outcome,
                termination,
            );
        }
        record_verdict_parse_metric(
            ReasonerPhase::Decide,
            &ctx.goal_id,
            final_outcome,
            termination.cause_label(),
            attempts,
        );
        crate::recipe_output::record_parse_outcome("decide", !final_outcome.is_parse_failure());
        Ok(final_decision)
    }
}

impl OrientReasoner for RecipeReasoner {
    /// Orient-phase failure-penalty demotion.
    ///
    /// Issue #2421 / #2429: invoke `recipe-runner-rs --output-format json` and
    /// parse the agent's REAL urgency decimal (the JSON envelope's final step
    /// output) — not the text-mode banner, whose timing string `(0.0s)` was
    /// scraped as a finite, in-range `0.0` and ACTIVELY demoted the goal's
    /// urgency to a value mined from the banner rather than the LLM's judgment.
    /// On a base parse-miss, run the escalation ladder, then fall — loudly — to
    /// the deterministic floor (`base_urgency − 0.2 × failure_count`). Emits the
    /// shared `brain_verdict_parsed_total` metric on both branches.
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        let invoke = |rung: LadderRung, prior: &str| self.invoke_orient_raw(ctx, rung, prior);
        let parse = |raw: &str| parse_orient_outcome(raw, ctx.base_urgency, ctx.failure_count);
        let default = || deterministic_floor(ctx.base_urgency, ctx.failure_count);

        let base_raw = invoke(LadderRung::Base, "")?;
        let (judgment, outcome) = parse(&base_raw);
        if !outcome.is_parse_failure() {
            record_verdict_parse_metric(ReasonerPhase::Orient, &ctx.goal_id, outcome, "ok", 1);
            crate::recipe_output::record_parse_outcome("orient", true);
            return Ok(judgment);
        }

        let cfg = EscalationConfig::from_env();
        let (final_judgment, final_outcome, attempts, termination) = run_brain_ladder(
            &ctx.goal_id,
            &base_raw,
            outcome,
            &cfg,
            invoke,
            parse,
            default,
            |j| format!("urgency={:.3}", j.adjusted_urgency),
        );
        // issue #2496: distinguish a deterministic-floor default reached via a
        // parse miss (e.g. the launcher version string mis-scraped as urgency)
        // from a genuinely low urgency the model emitted — log it distinctly and
        // attribute it to its `LadderTermination` cause (previously discarded).
        if final_outcome.is_parse_failure() {
            warn_parse_failure_default(
                ReasonerPhase::Orient,
                &ctx.goal_id,
                final_outcome,
                termination,
            );
        }
        record_verdict_parse_metric(
            ReasonerPhase::Orient,
            &ctx.goal_id,
            final_outcome,
            termination.cause_label(),
            attempts,
        );
        crate::recipe_output::record_parse_outcome("orient", !final_outcome.is_parse_failure());
        Ok(final_judgment)
    }
}

impl RecipeReasoner {
    /// Invoke the decide recipe once for a ladder rung, returning the agent's
    /// raw decision text (the JSON envelope's final step output). Genuine
    /// recipe-runner failures propagate as `Err` (issue #2421).
    fn invoke_decide_raw(
        &self,
        ctx: &DecideContext,
        rung: LadderRung,
        prior_output: &str,
    ) -> SimardResult<String> {
        let escalation_note = build_decide_escalation_note(rung, prior_output);
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            // issue #2421: text mode prints only a summary banner to stdout —
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
            .arg(format!("urgency={:.3}", ctx.urgency))
            .arg("-c")
            .arg(format!("reason={}", sanitize_context_var(&ctx.reason, 500)))
            // issue #2432: the (possibly empty) escalation/schema-repair note.
            .arg("-c")
            .arg(format!(
                "escalation_note={}",
                sanitize_context_var(&escalation_note, 4000)
            ))
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
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        extract_recipe_decision_output(&output.stdout, self.adapter_tag)
    }

    /// Invoke the orient recipe once for a ladder rung, returning the agent's
    /// raw urgency text (the JSON envelope's final step output). Genuine
    /// recipe-runner failures propagate as `Err` (issue #2421).
    fn invoke_orient_raw(
        &self,
        ctx: &OrientContext,
        rung: LadderRung,
        prior_output: &str,
    ) -> SimardResult<String> {
        let escalation_note = build_orient_escalation_note(rung, prior_output);
        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
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
            .arg("-c")
            .arg(format!(
                "escalation_note={}",
                sanitize_context_var(&escalation_note, 4000)
            ))
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
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        extract_recipe_decision_output(&output.stdout, self.adapter_tag)
    }
}

impl RecipeReasoner {
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

impl LifecycleInvoker for RecipeReasoner {
    fn invoke_lifecycle(
        &self,
        ctx: &EngineerLifecycleCtx,
        attempt: &LadderAttempt,
    ) -> SimardResult<String> {
        self.invoke_lifecycle_raw(ctx, attempt).map_err(|(e, _)| e)
    }
}

impl ActReasoner for RecipeReasoner {
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
            crate::recipe_output::record_parse_outcome("engineer_lifecycle", true);
            return Ok(decision);
        }

        // Parse-miss → confidence-gated escalation ladder (issue #2432). Spend
        // extra compute ONLY on this weak case.
        let cfg = EscalationConfig::from_env();
        let (final_decision, final_outcome, attempts, termination) =
            run_escalation_ladder(self, ctx, &base_raw, outcome, &cfg);
        // issue #2496: when the deterministic `continue_skipping` default is
        // reached because a transient parse miss EXHAUSTED the ladder (or a rung
        // failed to invoke), say so loudly and distinctly — it is NOT a
        // deliberate NO-ACTION decision; it is a transient parse-failure skip
        // that is re-evaluated next cycle. The conservative default itself is
        // unchanged; only its visibility improves.
        if final_outcome.is_parse_failure()
            && matches!(
                termination,
                LadderTermination::Exhausted | LadderTermination::InvokeError
            )
        {
            tracing::warn!(
                target: "simard::ooda_reasoners",
                goal = %ctx.goal_id,
                outcome_detail = final_outcome.label(),
                cause = termination.cause_label(),
                "engineer-lifecycle fell to continue_skipping via a PARSE FAILURE \
                 (ladder {}) — a TRANSIENT parse-failure skip, re-evaluated next \
                 cycle, NOT a deliberate NO-ACTION (issue #2496)",
                termination.cause_label()
            );
            eprintln!(
                "[simard] LIFECYCLE PARSE-FAILURE SKIP goal={} cause={} \
                 (transient, re-evaluated next cycle — NOT a deliberate no-action)",
                ctx.goal_id,
                termination.cause_label()
            );
        }
        // NOTE: `first_word` intentionally always reflects the *base* attempt's
        // token — it is the diagnostic record of what the cheap first pass
        // produced (e.g. the banner regression or a malformed reply), even on a
        // recovered row where `decision` reflects the recovering rung's choice.
        // `termination.cause_label()` distinguishes recovered / exhausted /
        // invoke-error / disabled so the two fields read unambiguously together.
        record_lifecycle_decision_metric(
            ctx,
            final_outcome,
            &lifecycle_first_word(&base_raw),
            lifecycle_decision_choice(&final_decision),
            termination.cause_label(),
            attempts,
        );
        crate::recipe_output::record_parse_outcome(
            "engineer_lifecycle",
            !final_outcome.is_parse_failure(),
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
///
/// Thin decision-only wrapper over [`parse_action_outcome`]; production routes
/// through the latter to capture the parse outcome for the escalation ladder
/// and the `brain_verdict_parsed_total` metric (issue #2421 / #2429).
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_action_from_text(text: &str) -> DecideJudgment {
    parse_action_outcome(text).0
}

/// Parse recipe output into a decide action AND a [`LifecycleParseOutcome`]
/// classification (issue #2421 / #2429).
///
/// The outcome distinguishes a genuinely parsed action (`Parsed` — including a
/// real `advance_goal`) from the two ways the parser falls back to
/// `AdvanceGoal`: `DefaultEmpty` (no text at all) and `DefaultMalformed` (text
/// present but the first word is not a known action). Before this split a real
/// `advance_goal` decision and a silent banner-misparse fallback were
/// indistinguishable — which is exactly what let the text-mode banner masquerade
/// as "working" (the first word `Recipe:` always defaulted to `AdvanceGoal`).
pub fn parse_action_outcome(text: &str) -> (DecideJudgment, LifecycleParseOutcome) {
    // Strip ANSI escapes + drop tracing-log / runner-banner lines first (shared
    // #2484 extractor) so a noise-obscured first-word action keyword is not
    // silently defaulted to `advance_goal`. Clean-path zero-copy preserves
    // today's behaviour on clean recipe output.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return (default_advance_goal(), LifecycleParseOutcome::DefaultEmpty);
    }
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
            return (
                ctor(truncate(trimmed, MAX_RATIONALE_CHARS)),
                LifecycleParseOutcome::Parsed,
            );
        }
    }
    (
        default_advance_goal(),
        LifecycleParseOutcome::DefaultMalformed,
    )
}

/// The loud deterministic decide default — `advance_goal` with a rationale that
/// names the parse-miss so a defaulted row is never mistaken for a real LLM
/// `advance_goal` decision.
fn default_advance_goal() -> DecideJudgment {
    DecideJudgment::AdvanceGoal {
        rationale: format!(
            "{DECIDE_ADAPTER_TAG}: no action keyword found in recipe output; defaulting to advance_goal"
        ),
    }
}

/// The snake_case action tag of a decide judgment, used only as the `decision`
/// label in escalation-ladder logging.
fn decide_decision_choice(decision: &DecideJudgment) -> &'static str {
    match decision {
        DecideJudgment::AdvanceGoal { .. } => "advance_goal",
        DecideJudgment::RunImprovement { .. } => "run_improvement",
        DecideJudgment::ConsolidateMemory { .. } => "consolidate_memory",
        DecideJudgment::ResearchQuery { .. } => "research_query",
        DecideJudgment::RunGymEval { .. } => "run_gym_eval",
        DecideJudgment::BuildSkill { .. } => "build_skill",
        DecideJudgment::LaunchSession { .. } => "launch_session",
        DecideJudgment::PollDeveloperActivity { .. } => "poll_developer_activity",
        DecideJudgment::ExtractIdeas { .. } => "extract_ideas",
        DecideJudgment::SafeUpdate { .. } => "safe_update",
    }
}

// ---------------------------------------------------------------------------
// Orient parse: first decimal float → deterministic floor
// ---------------------------------------------------------------------------

/// Parse recipe output for the first decimal float (e.g. `0.42`).
/// Falls to the deterministic floor when no valid float is found.
///
/// Thin judgment-only wrapper over [`parse_orient_outcome`]; production routes
/// through the latter to capture the parse outcome for the escalation ladder
/// and the `brain_verdict_parsed_total` metric (issue #2421 / #2429).
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_orient_from_text(text: &str, base_urgency: f64, failure_count: u32) -> OrientJudgment {
    parse_orient_outcome(text, base_urgency, failure_count).0
}

/// Parse recipe output into an orient judgment AND a [`LifecycleParseOutcome`]
/// classification (issue #2421 / #2429).
///
/// `Parsed` when a valid in-range decimal was extracted; otherwise the
/// deterministic floor is returned and the outcome is `DefaultEmpty` (no text)
/// or `DefaultMalformed` (text present but no valid decimal). This is what makes
/// the orient banner-misparse measurable: the text-mode banner's `(0.0s)` timing
/// would otherwise be scraped as a real `0.0` urgency and counted as `Parsed`.
pub fn parse_orient_outcome(
    text: &str,
    base_urgency: f64,
    failure_count: u32,
) -> (OrientJudgment, LifecycleParseOutcome) {
    // Strip ANSI escapes + drop tracing-log / runner-banner lines first (shared
    // #2484 extractor): a banner timing string like `(0.0s)` or a decimal inside
    // a tracing line must not be scraped as the urgency float and demote the
    // goal. Clean-path zero-copy preserves today's behaviour on clean output.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let s: &str = cleaned.as_ref();
    // Hoist trim above the scanner — avoids re-trimming on each candidate.
    let trimmed = s.trim();
    let bytes = s.as_bytes();
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
                    && let Ok(val) = s[start..i].parse::<f64>()
                {
                    // Inline validation before allocating rationale string.
                    if val.is_finite() && (0.0..=1.0).contains(&val) && val <= base_urgency + 1e-9 {
                        return (
                            OrientJudgment {
                                adjusted_urgency: val,
                                rationale: truncate(trimmed, MAX_RATIONALE_CHARS),
                                confidence: 1.0,
                                demotion_applied: base_urgency - val,
                            },
                            LifecycleParseOutcome::Parsed,
                        );
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    let miss = if trimmed.is_empty() {
        LifecycleParseOutcome::DefaultEmpty
    } else {
        LifecycleParseOutcome::DefaultMalformed
    };
    (deterministic_floor(base_urgency, failure_count), miss)
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
    // Strip ANSI escapes + drop tracing-log / runner-banner lines first (shared
    // #2484 extractor) so a noise-obscured first-word decision keyword is not
    // silently defaulted to `continue_skipping` — the #2419-family non-progress
    // loop. Clean-path zero-copy preserves today's behaviour on clean output.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let trimmed = cleaned.trim();
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
            target: "simard::ooda_reasoners",
            error = %e,
            outcome = outcome.label(),
            "failed to record brain_lifecycle_decision metric (decision unaffected)",
        );
    }
}

/// The `outcome` label for the [`VERDICT_PARSE_METRIC`]: `"parsed"` when a real
/// verdict/decision was extracted (base parse, or a ladder `Repaired`/
/// `Escalated` recovery), `"defaulted"` when a deterministic fallback was
/// applied (`DefaultEmpty`/`DefaultMalformed`/`Error` — the bug surface).
fn verdict_outcome_label(outcome: LifecycleParseOutcome) -> &'static str {
    if outcome.is_parse_failure() {
        "defaulted"
    } else {
        "parsed"
    }
}

/// Build the JSON `context` payload for the shared `brain_verdict_parsed_total`
/// metric. Separated from the I/O so the payload shape can be unit-tested
/// without touching the real `metrics.jsonl`.
fn build_verdict_parse_context(
    phase: ReasonerPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    cause: &str,
    attempts: u32,
) -> String {
    serde_json::json!({
        "phase": phase.as_str(),
        // Coarse parsed/defaulted signal — the numerator/denominator for
        // `parse_success_rate{phase}` (issue #2429).
        "outcome": verdict_outcome_label(outcome),
        // Fine-grained classification (parsed / repaired / escalated /
        // default_empty / default_malformed / error) for drill-down.
        "outcome_detail": outcome.label(),
        "is_parse_failure": outcome.is_parse_failure(),
        // issue #2496: the `LadderTermination::cause_label()` of the run
        // (`ladder_recovered` / `ladder_exhausted` / `ladder_invoke_error` /
        // `ladder_disabled`), or `ok` when the base attempt parsed without
        // entering the ladder. Decide and orient now wire the ladder's
        // termination through to this field (it was previously discarded as
        // `_termination`), so a `defaulted` row attributes the default to its
        // precise terminal path: `is_parse_failure=true` with
        // `cause=ladder_exhausted` is a transient parse miss, NOT a model that
        // chose to do nothing.
        "cause": cause,
        "goal_id": goal_id,
        // Total brain invocations spent (base + escalation rungs).
        "attempts": attempts,
    })
    .to_string()
}

/// Record one `brain_verdict_parsed_total` metric event (value `1.0`) per
/// recipe-backed brain phase invocation, on BOTH the parsed and the defaulted
/// branch, so the per-phase parse-success rate has a denominator (issue #2429).
///
/// Best-effort: a metrics-write failure is logged, never propagated — the brain
/// decision must not fail because telemetry could not be persisted.
///
/// No-op under `cfg!(test)` so unit tests never append to the operator's real
/// `~/.simard/metrics/metrics.jsonl`.
pub(crate) fn record_verdict_parse_metric(
    phase: ReasonerPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    cause: &str,
    attempts: u32,
) {
    let context = build_verdict_parse_context(phase, goal_id, outcome, cause, attempts);
    if cfg!(test) {
        return;
    }
    if let Err(e) = crate::self_metrics::record_metric(VERDICT_PARSE_METRIC, 1.0, &context) {
        tracing::warn!(
            target: "simard::ooda_reasoners",
            error = %e,
            phase = phase.as_str(),
            outcome = outcome.label(),
            "failed to record brain_verdict_parsed_total metric (decision unaffected)",
        );
    }
}

/// Emit a loud, distinct log when a recipe-backed brain phase reaches its
/// deterministic default **because a transient parse miss exhausted the
/// escalation ladder** — NOT because the model deliberately chose to do nothing
/// (issue #2496). Keeping the two events distinct is what stops a
/// poisoned-input stall (every active goal misparsing the Copilot launch-log
/// preamble) from masquerading as healthy "the brain decided to take no action"
/// behaviour while goals with real work sit idle.
///
/// The deterministic default itself is unchanged — it remains the rarely-needed
/// safety net; only its visibility and attribution improve. Logs a bounded
/// classification + the `LadderTermination` cause, never the raw agent preamble
/// (which embeds env-derived `NODE_OPTIONS`/binary paths), so dropped launcher
/// noise does not leak environment detail into logs.
fn warn_parse_failure_default(
    phase: ReasonerPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    termination: LadderTermination,
) {
    tracing::warn!(
        target: "simard::ooda_reasoners",
        phase = phase.as_str(),
        goal = %goal_id,
        outcome_detail = outcome.label(),
        cause = termination.cause_label(),
        "brain phase fell to its deterministic default via a PARSE FAILURE \
         (ladder {}) — NOT a model 'no action' decision; a transient parse miss, \
         re-evaluated next cycle (issue #2496)",
        termination.cause_label()
    );
    eprintln!(
        "[simard] BRAIN PARSE-FAILURE DEFAULT phase={} goal={} outcome={} cause={} \
         (transient miss, NOT a real no-action decision)",
        phase.as_str(),
        goal_id,
        outcome.label(),
        termination.cause_label()
    );
}

// ---------------------------------------------------------------------------
// Tests — behavioral contracts for the unified RecipeReasoner struct.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooda_reasoners::EngineerLifecycleCtx;
    use crate::ooda_reasoners::decide::DecideContext;
    use crate::ooda_reasoners::orient::OrientContext;
    use std::sync::Arc;

    // ===================================================================
    // resolve_recipe_path — parameterised by filename
    // ===================================================================

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_repo() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let result = resolve_recipe_path(
            Path::new("/nonexistent"),
            "ooda-decide.yaml",
            Some(home.path()),
        );
        assert!(
            result.is_none(),
            "must return None when neither hot-reload nor in-tree path exists"
        );
    }

    #[test]
    fn resolve_recipe_path_returns_none_for_nonexistent_filename() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let result =
            resolve_recipe_path(Path::new("/tmp"), "does-not-exist.yaml", Some(home.path()));
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

        let result = resolve_recipe_path(tmp.path(), "ooda-decide.yaml", Some(tmp.path()));
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

        let decide_path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml", Some(tmp.path()));
        let orient_path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml", Some(tmp.path()));

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
    // RecipeReasoner::new — constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_decide_recipe_missing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let brain = RecipeReasoner::new_with_home(
            Path::new("/nonexistent"),
            "ooda-decide.yaml",
            "recipe-decide-brain",
            Some(home.path()),
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_orient_recipe_missing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let brain = RecipeReasoner::new_with_home(
            Path::new("/nonexistent"),
            "ooda-orient.yaml",
            "recipe-orient-brain",
            Some(home.path()),
        );
        assert!(brain.is_none());
    }

    #[test]
    fn new_returns_none_when_lifecycle_recipe_missing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let brain = RecipeReasoner::new_with_home(
            Path::new("/nonexistent"),
            "ooda-engineer-lifecycle.yaml",
            "recipe-engineer-lifecycle-brain",
            Some(home.path()),
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
        let _brain = RecipeReasoner::new(tmp.path(), "ooda-decide.yaml", "recipe-decide-brain");
        // If construction succeeded, verify the tag is stored
        // If it returned None (no binary), that's OK for this environment
    }

    // ===================================================================
    // Trait impls — error messages include adapter_tag
    // ===================================================================

    #[test]
    fn judge_decision_error_includes_adapter_tag() {
        let brain = RecipeReasoner {
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
        let brain = RecipeReasoner {
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
        let brain = RecipeReasoner {
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
        let brain = RecipeReasoner {
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
        let brain = RecipeReasoner {
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
        let brain = RecipeReasoner {
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
    // Type erasure — RecipeReasoner implements all three traits
    // ===================================================================

    #[test]
    fn recipe_reasoner_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecipeReasoner>();
    }

    #[test]
    fn recipe_reasoner_can_be_arc_dyn_ooda_reasoners() {
        // This test verifies the type relationship at compile time.
        // Runtime: the brain has a fake path, so trait calls would fail,
        // but Arc wrapping must compile.
        let brain = RecipeReasoner {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn ActReasoner> = Arc::new(brain);
    }

    #[test]
    fn recipe_reasoner_can_be_arc_dyn_ooda_decide_reasoner() {
        let brain = RecipeReasoner {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn DecideReasoner> = Arc::new(brain);
    }

    #[test]
    fn recipe_reasoner_can_be_arc_dyn_ooda_orient_reasoner() {
        let brain = RecipeReasoner {
            recipe_path: PathBuf::from("/fake"),
            agent_binary: "copilot",
            adapter_tag: "test",
        };
        let _arc: Arc<dyn OrientReasoner> = Arc::new(brain);
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
    fn decide_reasoner_instance_uses_correct_recipe_filename() {
        // Verify the filename parameter is "ooda-decide.yaml"
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-decide.yaml"), "# decide").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml", Some(tmp.path()));
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-decide.yaml"));
    }

    #[test]
    fn orient_reasoner_instance_uses_correct_recipe_filename() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let recipe_dir = tmp.path().join("prompt_assets/simard/recipes");
        std::fs::create_dir_all(&recipe_dir).unwrap();
        std::fs::write(recipe_dir.join("ooda-orient.yaml"), "# orient").unwrap();

        let path = resolve_recipe_path(tmp.path(), "ooda-orient.yaml", Some(tmp.path()));
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

        let path =
            resolve_recipe_path(tmp.path(), "ooda-engineer-lifecycle.yaml", Some(tmp.path()));
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
        let decide_reasoner = RecipeReasoner {
            recipe_path: PathBuf::from("/nonexistent/recipe.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-decide-brain",
        };
        let orient_reasoner = RecipeReasoner {
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

        let decide_err = format!("{}", decide_reasoner.judge_decision(&ctx).unwrap_err());
        let orient_err = format!(
            "{}",
            orient_reasoner.judge_orientation(&orient_ctx).unwrap_err()
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
            use super::super::DeterministicFallbackOrientReasoner;
            let ctx = OrientContext {
                goal_id: "g1".into(),
                base_urgency: 0.8,
                base_reason: "test".into(),
                failure_count: 3,
            };
            let fallback = DeterministicFallbackOrientReasoner::compute(&ctx);
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
        use crate::ooda_reasoners::EngineerLifecycleCtx;
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
        use crate::ooda_reasoners::EngineerLifecycleCtx;
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

        #[test]
        fn ladder_termination_cause_labels_are_distinct() {
            assert_eq!(
                LadderTermination::Recovered.cause_label(),
                "ladder_recovered"
            );
            assert_eq!(
                LadderTermination::Exhausted.cause_label(),
                "ladder_exhausted"
            );
            assert_eq!(
                LadderTermination::InvokeError.cause_label(),
                "ladder_invoke_error"
            );
            assert_eq!(LadderTermination::Disabled.cause_label(), "ladder_disabled");
            // The four non-equal terminations must yield four distinct labels so
            // telemetry can tell exhaustion apart from an invoke-error stop.
            let labels = [
                LadderTermination::Recovered.cause_label(),
                LadderTermination::Exhausted.cause_label(),
                LadderTermination::InvokeError.cause_label(),
                LadderTermination::Disabled.cause_label(),
            ];
            let unique: std::collections::BTreeSet<_> = labels.iter().collect();
            assert_eq!(unique.len(), labels.len(), "cause labels must be distinct");
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
            let (decision, outcome, attempts, termination) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "OK the engineer looks fine", // base parse-miss text
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::Repaired);
            assert_eq!(termination, LadderTermination::Recovered);
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
            let (decision, outcome, attempts, termination) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "garbage",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::Escalated);
            assert_eq!(termination, LadderTermination::Recovered);
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
            let (decision, outcome, attempts, termination) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "banner noise",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(), // 2 rungs
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert_eq!(termination, LadderTermination::Exhausted);
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
            let (decision, outcome, attempts, termination) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "banner noise",
                LifecycleParseOutcome::DefaultEmpty,
                &EscalationConfig { max_escalations: 0 },
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultEmpty);
            assert_eq!(termination, LadderTermination::Disabled);
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
            let (decision, outcome, attempts, termination) = run_escalation_ladder(
                &invoker,
                &sample_ctx(),
                "garbage",
                LifecycleParseOutcome::DefaultMalformed,
                &EscalationConfig::default(),
            );
            assert_eq!(outcome, LifecycleParseOutcome::DefaultMalformed);
            assert_eq!(attempts, 2, "base + the failed rung, then stop");
            assert_eq!(
                termination,
                LadderTermination::InvokeError,
                "an early stop caused by a rung's own invoke failure must be \
                 distinguishable from true exhaustion in the metric cause"
            );
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

    // =================================================================
    // issue_2421_tests — decide + orient verdict/decision parse cluster
    //
    // The decide (`judge_decision`) and orient (`judge_orientation`) trait
    // impls share the EXACT #2419 root cause: they read recipe-runner-rs
    // DEFAULT `text` stdout (the SUCCESS banner) instead of the
    // `--output-format json` envelope's final step output.
    //
    //   - decide: the banner's first word is always `Recipe:`, which matches
    //     no action keyword, so `parse_action_from_text` silently defaults to
    //     `AdvanceGoal` every cycle (the LLM is ignored).
    //   - orient: WORSE than a benign default — `parse_orient_from_text` scans
    //     the banner for the first in-range decimal, and the timing string
    //     `(0.0s)` yields `0.0`, ACTIVELY demoting the goal's urgency to a
    //     value scraped from the banner rather than the LLM's judgment.
    //
    // These tests pin both the banner misparse (the bug) and the JSON-envelope
    // recovery (the fix) at the parse seam, using the existing
    // `extract_recipe_decision_output` helper. The live recipe-runner boundary
    // is covered by `tests/gadugi/decide-orient-brain-parse.sh`.
    // =================================================================
    mod issue_2421_tests {
        use super::super::*;
        use crate::ooda_loop::ActionKind;

        const DECIDE_BANNER: &str = "Recipe: ooda-decide (v1.0.0)\nSteps: 1\n\nRecipe 'ooda-decide': SUCCESS (0.0s)\n  [completed] decide-action (0.0s)\n\n";
        const ORIENT_BANNER: &str = "Recipe: ooda-orient (v1.0.0)\nSteps: 1\n\nRecipe 'ooda-orient': SUCCESS (0.0s)\n  [completed] orient-decision (0.0s)\n\n";

        // --- decide: banner misparse pin + JSON-envelope recovery -----------

        #[test]
        fn decide_banner_first_word_is_never_a_valid_action() {
            // Regression pin for the silent-`AdvanceGoal` half of #2421: the
            // banner's first word `Recipe:` matches no action, so the parser
            // falls back to AdvanceGoal with the explicit "no action keyword"
            // rationale. This proves the caller MUST NOT feed it the banner.
            let j = parse_action_from_text(DECIDE_BANNER);
            assert_eq!(j.action_kind(), ActionKind::AdvanceGoal);
            assert!(
                j.rationale().contains("no action keyword"),
                "banner must classify as a default, not a real decision; got: {}",
                j.rationale()
            );
        }

        #[test]
        fn decide_json_envelope_recovers_real_action() {
            // The fix: extract the final step output from the json envelope and
            // parse THAT — recovering a real (non-default) action the banner
            // hid.
            let envelope = r#"{
                "recipe_name": "ooda-decide",
                "success": true,
                "step_results": [
                    {"step_id": "decide-action",
                     "output": "consolidate_memory context overhead is high this cycle",
                     "error": "", "duration": 0.0}
                ]
            }"#;
            let extracted =
                extract_recipe_decision_output(envelope.as_bytes(), DECIDE_ADAPTER_TAG).unwrap();
            let j = parse_action_from_text(&extracted);
            assert_eq!(
                j.action_kind(),
                ActionKind::ConsolidateMemory,
                "the JSON-envelope fix must recover the LLM's real action, not AdvanceGoal"
            );
        }

        #[test]
        fn decide_banner_is_not_a_valid_json_envelope() {
            // Proves the decide phase must pass `--output-format json`: the
            // text banner can never decode as an envelope, so a missing flag
            // fails loudly instead of silently defaulting.
            let err = extract_recipe_decision_output(DECIDE_BANNER.as_bytes(), DECIDE_ADAPTER_TAG)
                .unwrap_err();
            assert!(matches!(err, SimardError::AdapterInvocationFailed { .. }));
        }

        // --- orient: active urgency corruption pin + JSON-envelope recovery -

        #[test]
        fn orient_banner_timing_actively_corrupts_urgency() {
            // Regression pin for the WORST half of #2421: the banner's `(0.0s)`
            // timing yields a finite, in-range `0.0`, so the parser returns
            // `adjusted_urgency = 0.0`, demoting the goal to a value scraped
            // from the banner. Critically, 0.0 is NOT the deterministic floor
            // (which would be base_urgency 0.8 for 0 failures) — proving the
            // value was scraped from the banner, not computed.
            let base_urgency = 0.8;
            let failure_count = 0;
            let j = parse_orient_from_text(ORIENT_BANNER, base_urgency, failure_count);
            assert_eq!(
                j.adjusted_urgency, 0.0,
                "banner '(0.0s)' timing is scraped as urgency 0.0 — the bug"
            );
            // The deterministic floor for 0 failures would have preserved the
            // base urgency; 0.0 != 0.8 confirms active corruption (not a floor).
            assert!(
                (j.adjusted_urgency - base_urgency).abs() > 1e-9,
                "corrupted 0.0 must differ from the deterministic floor (0.8)"
            );
        }

        #[test]
        fn orient_json_envelope_recovers_real_urgency() {
            // The fix: extract the final step output (the LLM's real decimal)
            // from the json envelope rather than scraping the banner timing.
            let envelope = r#"{
                "recipe_name": "ooda-orient",
                "success": true,
                "step_results": [
                    {"step_id": "orient-decision",
                     "output": "0.65 goal remains high urgency despite one transient failure",
                     "error": "", "duration": 0.0}
                ]
            }"#;
            let extracted =
                extract_recipe_decision_output(envelope.as_bytes(), ORIENT_ADAPTER_TAG).unwrap();
            let j = parse_orient_from_text(&extracted, 0.8, 0);
            assert!(
                (j.adjusted_urgency - 0.65).abs() < 1e-9,
                "must recover the LLM urgency 0.65, got {}",
                j.adjusted_urgency
            );
        }

        #[test]
        fn orient_extraction_isolates_llm_decimal_from_timing_banner() {
            // The envelope JSON text itself contains a `(0.0s)` timing string
            // (in an earlier step's output), but extraction returns ONLY the
            // FINAL step output `0.7`, so the parsed urgency is the LLM's
            // decimal — never the timing noise.
            let envelope = r#"{
                "success": true,
                "step_results": [
                    {"step_id": "prep", "output": "ran in (0.0s)", "error": "", "duration": 0.0},
                    {"step_id": "orient-decision", "output": "0.7", "error": "", "duration": 0.0}
                ]
            }"#;
            let extracted =
                extract_recipe_decision_output(envelope.as_bytes(), ORIENT_ADAPTER_TAG).unwrap();
            assert_eq!(extracted, "0.7");
            let j = parse_orient_from_text(&extracted, 0.8, 0);
            assert!(
                (j.adjusted_urgency - 0.7).abs() < 1e-9,
                "extraction must isolate the LLM decimal from timing noise; got {}",
                j.adjusted_urgency
            );
        }
    }

    // =================================================================
    // issue_2419_family_phase_tests — production wiring for the decide /
    // orient / merge-judge structured-transport fix (#2421 / #2428 / #2429).
    //
    // The `issue_2421_tests` above pin the PARSE seam (banner misparse vs.
    // JSON-envelope recovery). These pin the rest of the production contract:
    //   - parse-OUTCOME classification (Parsed vs DefaultEmpty/DefaultMalformed)
    //     that drives the escalation ladder and the `brain_verdict_parsed_total`
    //     metric numerator/denominator;
    //   - the phase escalation-note builders (the `{{escalation_note}}` seam);
    //   - the shared verdict-parse metric context shape;
    //   - the generic `run_brain_ladder` backbone working for an ARBITRARY
    //     decision type (proving decide/orient/merge ride the same ladder the
    //     lifecycle phase does, rather than a reinvented one).
    // =================================================================
    mod issue_2419_family_phase_tests {
        use super::super::*;
        use crate::ooda_reasoners::{DecideJudgment, ReasonerPhase};

        // --- decide parse-outcome classification (#2421 / #2429) ----------

        #[test]
        fn decide_outcome_empty_is_default_empty() {
            let (j, oc) = parse_action_outcome("   ");
            assert!(matches!(j, DecideJudgment::AdvanceGoal { .. }));
            assert_eq!(oc, LifecycleParseOutcome::DefaultEmpty);
            assert!(oc.is_parse_failure());
        }

        #[test]
        fn decide_outcome_banner_first_word_is_default_malformed() {
            // The recipe-runner text-mode SUCCESS banner must be classified as a
            // parse-miss (DefaultMalformed), NOT counted as a real `advance_goal`
            // decision. After #2484 noise-stripping the pure-noise lines
            // (`Recipe:`, `Steps:`, `[completed]`) are dropped, but the runner's
            // `Recipe '<name>': SUCCESS` summary line survives — so the banner
            // still reaches the parser as present-but-unrecognised text and is
            // correctly flagged DefaultMalformed (never a silent advance_goal).
            let banner = "Recipe: ooda-decide (v1.0.0)\nSteps: 1\n\n\
                          Recipe 'ooda-decide': SUCCESS (0.0s)\n  \
                          [completed] decide-next-action (0.0s)\n";
            let (j, oc) = parse_action_outcome(banner);
            assert!(matches!(j, DecideJudgment::AdvanceGoal { .. }));
            assert_eq!(oc, LifecycleParseOutcome::DefaultMalformed);
            assert!(oc.is_parse_failure());
        }

        #[test]
        fn decide_outcome_pure_noise_banner_is_default_empty() {
            // A banner consisting ONLY of droppable noise lines (no surviving
            // `Recipe '<name>': SUCCESS` summary) strips to empty and is
            // classified DefaultEmpty — still a loud parse-failure that defaults
            // to advance_goal and escalates, never a real decision (#2484).
            let (j, oc) = parse_action_outcome(
                "Recipe: ooda-decide SUCCESS (0.0s)\n  [completed] decide (0.0s)\n",
            );
            assert!(matches!(j, DecideJudgment::AdvanceGoal { .. }));
            assert_eq!(oc, LifecycleParseOutcome::DefaultEmpty);
            assert!(oc.is_parse_failure());
        }

        #[test]
        fn decide_outcome_real_advance_goal_is_parsed_not_default() {
            // A genuine `advance_goal` LLM decision must read as Parsed — the
            // whole reason the outcome is split out from the judgment.
            let (j, oc) = parse_action_outcome("advance_goal engineer assigned, continue");
            assert!(matches!(j, DecideJudgment::AdvanceGoal { .. }));
            assert_eq!(oc, LifecycleParseOutcome::Parsed);
            assert!(!oc.is_parse_failure());
        }

        #[test]
        fn decide_outcome_known_keyword_is_parsed() {
            let (j, oc) = parse_action_outcome("consolidate_memory overhead high");
            assert!(matches!(j, DecideJudgment::ConsolidateMemory { .. }));
            assert_eq!(oc, LifecycleParseOutcome::Parsed);
        }

        #[test]
        fn decide_decision_choice_covers_all_variants() {
            assert_eq!(
                decide_decision_choice(&DecideJudgment::AdvanceGoal {
                    rationale: String::new()
                }),
                "advance_goal"
            );
            assert_eq!(
                decide_decision_choice(&DecideJudgment::ConsolidateMemory {
                    rationale: String::new()
                }),
                "consolidate_memory"
            );
            assert_eq!(
                decide_decision_choice(&DecideJudgment::SafeUpdate {
                    rationale: String::new()
                }),
                "safe_update"
            );
        }

        // --- orient parse-outcome classification (#2421 / #2429) ----------

        #[test]
        fn orient_outcome_valid_decimal_is_parsed() {
            let (j, oc) = parse_orient_outcome("0.65 still urgent", 0.8, 0);
            assert!((j.adjusted_urgency - 0.65).abs() < 1e-9);
            assert_eq!(oc, LifecycleParseOutcome::Parsed);
        }

        #[test]
        fn orient_outcome_no_decimal_is_default_malformed_floor() {
            let (j, oc) = parse_orient_outcome("high urgency, no number here", 0.8, 1);
            assert_eq!(oc, LifecycleParseOutcome::DefaultMalformed);
            assert!(oc.is_parse_failure());
            // Deterministic floor for 1 failure: 0.8 − 0.2 = 0.6.
            assert!((j.adjusted_urgency - 0.6).abs() < 1e-9);
        }

        #[test]
        fn orient_outcome_empty_is_default_empty_floor() {
            let (_, oc) = parse_orient_outcome("   ", 0.8, 0);
            assert_eq!(oc, LifecycleParseOutcome::DefaultEmpty);
        }

        // --- escalation-note builders (the {{escalation_note}} seam) -------

        #[test]
        fn decide_escalation_note_empty_on_base() {
            assert_eq!(
                build_decide_escalation_note(LadderRung::Base, "anything"),
                ""
            );
        }

        #[test]
        fn decide_escalation_note_repair_lists_actions_and_prior() {
            let n = build_decide_escalation_note(LadderRung::SchemaRepair, "Recipe: banner");
            assert!(n.contains("SCHEMA REPAIR"), "note: {n}");
            assert!(
                n.contains("advance_goal") && n.contains("consolidate_memory"),
                "note must echo the action list"
            );
            assert!(
                n.contains("Recipe: banner"),
                "note must feed prior output back"
            );
        }

        #[test]
        fn orient_escalation_note_repair_demands_decimal() {
            let n = build_orient_escalation_note(LadderRung::SchemaRepair, "junk");
            assert!(n.contains("SCHEMA REPAIR"), "note: {n}");
            assert!(
                n.to_lowercase().contains("decimal"),
                "note must demand a decimal"
            );
            assert!(n.contains("junk"), "note must feed prior output back");
        }

        #[test]
        fn phase_escalation_note_escalate_adds_high_effort() {
            let n =
                build_phase_escalation_note(LadderRung::Escalate, "p", "REPAIR_INSTR", "HE_INSTR");
            assert!(n.contains("SCHEMA REPAIR"), "escalate still repairs");
            assert!(n.contains("HIGH-EFFORT"), "escalate adds the effort tier");
            assert!(n.contains("REPAIR_INSTR") && n.contains("HE_INSTR"));
        }

        // --- shared verdict-parse metric context (#2429) ------------------

        #[test]
        fn verdict_metric_context_parsed_defaulted_and_recovered() {
            let parsed = build_verdict_parse_context(
                ReasonerPhase::Decide,
                "g1",
                LifecycleParseOutcome::Parsed,
                "ok",
                1,
            );
            let v: serde_json::Value = serde_json::from_str(&parsed).unwrap();
            assert_eq!(v["phase"], "decide");
            assert_eq!(v["outcome"], "parsed");
            assert_eq!(v["is_parse_failure"], false);
            assert_eq!(v["cause"], "ok");
            assert_eq!(v["attempts"], 1);

            let defaulted = build_verdict_parse_context(
                ReasonerPhase::MergeJudge,
                "pr-9",
                LifecycleParseOutcome::DefaultMalformed,
                LadderTermination::Exhausted.cause_label(),
                3,
            );
            let v2: serde_json::Value = serde_json::from_str(&defaulted).unwrap();
            assert_eq!(v2["phase"], "merge_judge");
            assert_eq!(v2["outcome"], "defaulted");
            assert_eq!(v2["is_parse_failure"], true);
            assert_eq!(v2["outcome_detail"], "default_malformed");
            // issue #2496: a defaulted row attributes the default to its cause.
            assert_eq!(v2["cause"], "ladder_exhausted");

            // A ladder recovery counts as parsed — that is how the ladder drops
            // the measured default rate.
            let repaired = build_verdict_parse_context(
                ReasonerPhase::Orient,
                "g",
                LifecycleParseOutcome::Repaired,
                LadderTermination::Recovered.cause_label(),
                2,
            );
            let v3: serde_json::Value = serde_json::from_str(&repaired).unwrap();
            assert_eq!(v3["outcome"], "parsed");
            assert_eq!(v3["outcome_detail"], "repaired");
            assert_eq!(v3["cause"], "ladder_recovered");
        }

        // --- generic ladder backbone for an ARBITRARY decision type -------

        #[test]
        fn generic_ladder_recovers_for_arbitrary_decision_type() {
            // run_brain_ladder is decision-type-agnostic: a String decision that
            // only "parses" when the text is "good". The first rung returns it.
            let cfg = EscalationConfig { max_escalations: 2 };
            let (decision, outcome, attempts, term) = run_brain_ladder(
                "g",
                "bad-base",
                LifecycleParseOutcome::DefaultMalformed,
                &cfg,
                |_rung, _prior| Ok("good".to_string()),
                |raw: &str| {
                    if raw == "good" {
                        ("GOOD".to_string(), LifecycleParseOutcome::Parsed)
                    } else {
                        ("DEF".to_string(), LifecycleParseOutcome::DefaultMalformed)
                    }
                },
                || "DEF".to_string(),
                |d: &String| d.clone(),
            );
            assert_eq!(decision, "GOOD");
            assert_eq!(outcome, LifecycleParseOutcome::Repaired);
            assert_eq!(attempts, 2, "base + 1 recovering rung");
            assert_eq!(term, LadderTermination::Recovered);
        }

        #[test]
        fn generic_ladder_exhausts_to_loud_default_for_arbitrary_type() {
            let cfg = EscalationConfig { max_escalations: 2 };
            let (decision, outcome, attempts, term) = run_brain_ladder(
                "g",
                "bad",
                LifecycleParseOutcome::DefaultMalformed,
                &cfg,
                |_r, _p| Ok("still-bad".to_string()),
                |_raw: &str| ("DEF".to_string(), LifecycleParseOutcome::DefaultMalformed),
                || "DEF".to_string(),
                |d: &String| d.clone(),
            );
            assert_eq!(decision, "DEF");
            assert!(outcome.is_parse_failure());
            assert_eq!(attempts, 3, "base + 2 exhausted rungs");
            assert_eq!(term, LadderTermination::Exhausted);
        }
    }
}

/// Issue #2496: the production-deadlock regression cluster. The Copilot CLI
/// `1.0.66-2` launch-log preamble (now stripped at the shared `recipe_output`
/// chokepoint) must not shadow the decide/orient first-word/first-float token,
/// and a parse-failure default must attribute to its `LadderTermination` cause
/// — distinct from a genuine decision.
#[cfg(test)]
mod issue_2496_decide_orient_launcher_tests {
    use super::*;

    const ESC: char = '\u{1b}';

    /// A real Copilot CLI `1.0.66-2` launch-log preamble (ANSI-coloured) wrapped
    /// around `answer`, exactly as captured in the recipe envelope's
    /// `step_results[].output`.
    fn noisy_capture(answer: &str) -> String {
        format!(
            "{ESC}[2m\u{2139}{ESC}[0m NODE_OPTIONS=--max-old-space-size=32768 \
             (saved preference). To change: /home/azureuser/.amplihack/config\n\
             {ESC}[34mINFO{ESC}[0m launching copilot \
             binary=/home/azureuser/.npm-global/bin/copilot \
             version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
             Run 'copilot update' to check for updates.\n\
             {answer}"
        )
    }

    #[test]
    fn decide_parses_real_action_behind_launcher_preamble() {
        let raw = noisy_capture("run_improvement The brain-parse deadlock fix is ready.");
        let (decision, outcome) = parse_action_outcome(&raw);
        assert_eq!(
            outcome,
            LifecycleParseOutcome::Parsed,
            "launcher preamble must not force the default_malformed miss"
        );
        assert!(
            matches!(decision, DecideJudgment::RunImprovement { .. }),
            "the model's real action must be read, not launcher noise"
        );
    }

    #[test]
    fn orient_reads_real_urgency_not_the_version_string() {
        // base_urgency 0.80; the model's real urgency is 0.42. The version
        // string `1.0.66-2` is on a dropped launcher line, so it can never be
        // mined as the urgency float ahead of the model's judgment.
        let raw = noisy_capture("0.42 demoting slightly after one failure.");
        let (judgment, outcome) = parse_orient_outcome(&raw, 0.80, 1);
        assert_eq!(outcome, LifecycleParseOutcome::Parsed);
        assert!(
            (judgment.adjusted_urgency - 0.42).abs() < 1e-9,
            "must read the model's 0.42, got {}",
            judgment.adjusted_urgency
        );
    }

    #[test]
    fn all_goals_default_malformed_stall_no_longer_reproduces() {
        // The production deadlock: the SAME noisy capture across a batch of
        // active goals previously misparsed to `default_malformed` for EVERY
        // goal, exhausting the ladder and spawning zero engineers. With launcher
        // stripping, each goal's real decision now parses.
        let goals = [
            ("g-1", "run_improvement fix the flaky test"),
            ("g-2", "advance_goal open the next PR"),
            ("g-3", "consolidate_memory distil the last episode"),
            ("g-4", "launch_session start the engineer"),
        ];
        for (goal, answer) in goals {
            let raw = noisy_capture(answer);
            let (_decision, outcome) = parse_action_outcome(&raw);
            assert_eq!(
                outcome,
                LifecycleParseOutcome::Parsed,
                "goal {goal} must parse its real decision, not stall on default_malformed"
            );
        }
    }

    /// End-to-end production-flow regression (#2432/#2496): the Copilot launch-log
    /// preamble arrives INSIDE the recipe-runner-rs JSON envelope's terminal
    /// `step_results[].output` (the captured agent stdout), exactly as the daemon
    /// receives it. Decoding the envelope via [`extract_recipe_decision_output`]
    /// and then parsing that output must recover the model's real decision — the
    /// full path that deadlocked in production, not merely the inner parse helper.
    #[test]
    fn decide_recovers_real_action_through_full_envelope_with_preamble() {
        let noisy = noisy_capture("run_improvement the brain-parse deadlock fix is ready");
        let envelope = serde_json::json!({
            "success": true,
            "step_results": [
                {"step_id": "decide-action", "output": noisy, "error": "", "duration": 0.0}
            ]
        })
        .to_string();
        let extracted =
            extract_recipe_decision_output(envelope.as_bytes(), DECIDE_ADAPTER_TAG).unwrap();
        let (decision, outcome) = parse_action_outcome(&extracted);
        assert_eq!(
            outcome,
            LifecycleParseOutcome::Parsed,
            "the preamble inside the envelope step output must not force a default_malformed miss"
        );
        assert!(
            matches!(decision, DecideJudgment::RunImprovement { .. }),
            "the model's real action must survive the full envelope→clean→parse path"
        );
    }

    /// Orient counterpart of the full-envelope regression. With `base_urgency`
    /// at the ceiling (1.0), the version string's `1.0` token IS in range and
    /// `<= base_urgency`, so without launcher stripping the orient first-float
    /// scanner scrapes `1.0` from `GitHub Copilot CLI 1.0.66-2` and silently
    /// overrides the model's real demotion decision (0.42) with "no demotion".
    /// Stripping the launcher line is what lets the real 0.42 be read — this is
    /// the exact orient half of the #2496 version-string-scraping deadlock.
    #[test]
    fn orient_recovers_real_urgency_through_full_envelope_with_preamble() {
        let noisy = noisy_capture("0.42 demote sharply after one transient failure");
        let envelope = serde_json::json!({
            "success": true,
            "step_results": [
                {"step_id": "orient-decision", "output": noisy, "error": "", "duration": 0.0}
            ]
        })
        .to_string();
        let extracted =
            extract_recipe_decision_output(envelope.as_bytes(), ORIENT_ADAPTER_TAG).unwrap();
        let (judgment, outcome) = parse_orient_outcome(&extracted, 1.0, 1);
        assert_eq!(outcome, LifecycleParseOutcome::Parsed);
        assert!(
            (judgment.adjusted_urgency - 0.42).abs() < 1e-9,
            "must read the model's 0.42 through the full envelope path, not the version \
             string's 1.0; got {}",
            judgment.adjusted_urgency
        );
    }

    #[test]
    fn launcher_only_capture_is_a_distinct_parse_failure() {
        // A capture that is ONLY the launcher preamble (the model produced no
        // answer) must still classify as a parse failure — the loud, attributable
        // default, never a silent success masquerading as a real "no action".
        let raw = noisy_capture("");
        let (_decision, outcome) = parse_action_outcome(&raw);
        assert!(
            outcome.is_parse_failure(),
            "an answer-less launcher capture is a parse failure, not a real decision"
        );
    }

    #[test]
    fn parse_failure_default_carries_its_termination_cause() {
        // The decide/orient `cause` wiring: a parse-failure default attributes to
        // its `LadderTermination`, distinct from a genuine decision (`ok`).
        let exhausted = build_verdict_parse_context(
            ReasonerPhase::Decide,
            "g",
            LifecycleParseOutcome::DefaultMalformed,
            LadderTermination::Exhausted.cause_label(),
            3,
        );
        let v: serde_json::Value = serde_json::from_str(&exhausted).unwrap();
        assert_eq!(v["is_parse_failure"], true);
        assert_eq!(v["cause"], "ladder_exhausted");

        let invoke_err = build_verdict_parse_context(
            ReasonerPhase::Orient,
            "g",
            LifecycleParseOutcome::DefaultMalformed,
            LadderTermination::InvokeError.cause_label(),
            2,
        );
        let v2: serde_json::Value = serde_json::from_str(&invoke_err).unwrap();
        assert_eq!(v2["cause"], "ladder_invoke_error");

        // A genuine parsed decision is tagged `ok`, never a ladder cause.
        let parsed = build_verdict_parse_context(
            ReasonerPhase::Decide,
            "g",
            LifecycleParseOutcome::Parsed,
            "ok",
            1,
        );
        let v3: serde_json::Value = serde_json::from_str(&parsed).unwrap();
        assert_eq!(v3["is_parse_failure"], false);
        assert_eq!(v3["cause"], "ok");
    }
}
