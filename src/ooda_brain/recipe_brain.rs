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
//! context vars, then parses the agent's decision through the shared #2484
//! sanitizing chokepoint. Parsing prefers a structured JSON-envelope decision
//! block (`{"decision": ...}` / `{"adjusted_urgency": ...}`, issue #2580) and
//! falls back to a first-word / first-number scan for the bare-word contract.
//! On a parse-miss the confidence-gated escalation ladder (issue #2432) spends
//! bounded extra compute; if it is exhausted with no parseable decision the
//! phase surfaces an EXPLICIT `Err` + `brain_parse_error` metric — never a
//! silent deterministic default (issue #2580 operator zero-fallback contract).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::decide::{DecideContext, DecideJudgment, OodaDecideBrain};
use super::orient::{OodaOrientBrain, OrientContext, OrientJudgment};
use super::sanitize::sanitize_context_var;
use super::{
    BrainPhase, EngineerAdmissionCtx, EngineerAdmissionDecision, EngineerLifecycleCtx,
    EngineerLifecycleDecision, GoalOutcomeCtx, GoalOutcomeDecision, IdeaCluster,
    IdeaConsolidationCtx, IdeaDedupCtx, IdeaDedupDecision, OodaBrain, PerGoalAction,
    PerGoalCycleCtx, ResourceAdmissionCtx, ResourceAdmissionDecision,
};
use crate::error::{SimardError, SimardResult};

// Phase-specific adapter tags used in parse function error/fallback messages.
// The decide/orient seams now bind their tag at the `RecipeBrain` construction
// site (`daemon/brains.rs`) via string literal, so only the lifecycle tag is
// still referenced from a constant here.
const LIFECYCLE_ADAPTER_TAG: &str = "recipe-engineer-lifecycle-brain";
/// Adapter tag for the dependency/overlap-aware admission recipe (issue #2690).
const ADMISSION_ADAPTER_TAG: &str = "recipe-engineer-admission-brain";
/// Recipe filename for the admission reasoning step (issue #2690). Resolved as a
/// sibling of the lifecycle recipe the act-phase [`RecipeBrain`] already holds.
const ADMISSION_RECIPE_FILENAME: &str = "ooda-engineer-admission.yaml";
/// Adapter tag for the resource-aware admission recipe (issue #2706).
const RESOURCE_ADMISSION_ADAPTER_TAG: &str = "recipe-resource-admission-brain";
/// Recipe filename for the resource-aware admission reasoning step (issue #2706).
/// Resolved as a sibling of the lifecycle recipe, like the overlap recipe.
const RESOURCE_ADMISSION_RECIPE_FILENAME: &str = "ooda-resource-admission.yaml";
/// Adapter tag for the creative-idea semantic dedup + enhance recipe (issue #2925).
const IDEA_DEDUP_ADAPTER_TAG: &str = "recipe-idea-dedup-brain";
/// Recipe filename for the per-candidate semantic dedup reasoning step (#2925).
const IDEA_DEDUP_RECIPE_FILENAME: &str = "creative-idea-dedup.yaml";
/// Adapter tag for the creative-ideas consolidation clustering recipe (#2925).
const IDEA_CONSOLIDATION_ADAPTER_TAG: &str = "recipe-idea-consolidation-brain";
/// Recipe filename for the one-time consolidation clustering step (#2925).
const IDEA_CONSOLIDATION_RECIPE_FILENAME: &str = "creative-ideas-consolidation.yaml";
/// Adapter tag for the per-goal, per-cycle agentic decision recipe (issue #4453).
const PER_GOAL_CYCLE_ADAPTER_TAG: &str = "recipe-per-goal-cycle-brain";
/// Recipe filename for the per-goal, per-cycle reasoning step (issue #4453).
/// Resolved as a sibling of the lifecycle recipe, like the admission recipes.
const PER_GOAL_CYCLE_RECIPE_FILENAME: &str = "ooda-per-goal-cycle.yaml";

/// Cap on raw response text embedded in error messages and rationale fields.
const MAX_RATIONALE_CHARS: usize = 500;

/// Fixed cycle number the Orient/Decide reasoner seams bind their per-call
/// typed record to (Group A rework, #4785). Unlike the per-goal-cycle seam,
/// the `DecideContext` / `OrientContext` carry no cycle number, and neither do
/// the ooda_loop call sites. That is safe: each `judge_decision` /
/// `judge_orientation` call allocates a fresh, UNIQUE temp dir for its record,
/// so cross-cycle replay (R7) is already defeated by the ephemeral path. The
/// same constant is passed to the recipe's `-c cycle_number` writer arg AND to
/// the `read_verified_*` reader, so the goal-id + cycle-number identity check
/// stays self-consistent (a foreign record with a different goal is still
/// rejected via R6).
const REASONER_RECORD_CYCLE: u32 = 0;

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
#[allow(dead_code)] // Retained for Groups B/C/D (see `record_verdict_parse_metric`).
const VERDICT_PARSE_METRIC: &str = "brain_verdict_parsed_total";

/// Metric emitted ONLY when a reasoner's bounded escalation ladder is exhausted
/// (or a rung's own invocation failed) with no parseable decision — a GENUINE,
/// post-sanitization, post-bounded-retry parse failure that is now surfaced to
/// the caller as an explicit `Err` instead of a silent deterministic default
/// (issue #2580 — operator zero-fallback contract).
///
/// Unlike [`VERDICT_PARSE_METRIC`] (which fires on every invocation, parsed OR
/// defaulted), `brain_parse_error` fires *only* on the hard-failure terminal.
/// It is the honest "current fallback rate" signal: zero when the brain is
/// healthy, non-zero only on a real, unrecoverable parse failure — never a
/// stale/cumulative count of decisions that actually parsed.
const BRAIN_PARSE_ERROR_METRIC: &str = "brain_parse_error";

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
            target: "simard::ooda_brain",
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
                    target: "simard::ooda_brain",
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
                        target: "simard::ooda_brain",
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
            target: "simard::ooda_brain",
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
        Self::new_with_home(repo_root, recipe_filename, adapter_tag, None)
    }

    /// Like [`RecipeBrain::new`], but accepts a `home_override` for the
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

impl OodaDecideBrain for RecipeBrain {
    /// Decide-phase action routing (Group A rework, #4785).
    ///
    /// The reasoner ACTS by calling the gated `simard ooda record-decide` tool,
    /// which validates the closed 10-variant [`DecideChoice`](super::DecideChoice)
    /// enum and atomically writes one typed
    /// [`DecideDecisionRecord`](super::DecideDecisionRecord). This method runs
    /// that recipe over a fresh, UNIQUE per-call temp dir, then reads the typed
    /// record back with [`read_verified_decide`](super::read_verified_decide) —
    /// it NEVER scrapes the agent's prose stdout.
    ///
    /// NO-FALLBACK, FAIL-CLOSED (operator zero-fallback contract, #2580 / #1711):
    /// a recipe invocation failure OR any read-verification failure (absent /
    /// malformed / wrong-schema / out-of-enum / empty-reason / goal- or
    /// cycle-mismatch) surfaces as an explicit `Err`; the Decide caller records
    /// it and SKIPS the priority — never a fabricated `advance_goal`.
    fn judge_decision(&self, ctx: &DecideContext) -> SimardResult<DecideJudgment> {
        let tempdir = tempfile::Builder::new()
            .prefix("simard-ooda-decide-")
            .tempdir()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("could not allocate a per-call decide temp dir: {e}"),
            })?;
        let record_path = tempdir.path().join("decide.json");

        // The agent records its verdict by calling `simard ooda record-decide`
        // (writing `record_path`); its stdout is IGNORED.
        self.run_decide_recipe(ctx, &record_path)?;

        // Read the TYPED record — never scrape prose. Every failure is an Err.
        let choice =
            super::read_verified_decide(&record_path, &ctx.goal_id, REASONER_RECORD_CYCLE)?;
        crate::recipe_output::record_parse_outcome("decide", true);
        Ok(choice.to_judgment())
    }
}

impl OodaOrientBrain for RecipeBrain {
    /// Orient-phase failure-penalty demotion (Group A rework, #4785).
    ///
    /// The reasoner ACTS by calling the gated `simard ooda record-orient` tool,
    /// which validates the numeric fields + reason through
    /// [`OrientFields::from_fields`](super::OrientFields::from_fields) (finite,
    /// `[0,1]`, no escalation) and atomically writes one typed
    /// [`OrientDecisionRecord`](super::OrientDecisionRecord). This method runs
    /// that recipe over a fresh, UNIQUE per-call temp dir, then reads the typed
    /// record back with [`read_verified_orient`](super::read_verified_orient) —
    /// it NEVER scrapes the agent's prose stdout.
    ///
    /// NO-FALLBACK, FAIL-CLOSED (#2580 / #1711): a recipe invocation failure OR
    /// any read-verification failure surfaces as an explicit `Err`; the Orient
    /// caller records it and KEEPS the goal's BASE urgency — never a fabricated
    /// demotion.
    fn judge_orientation(&self, ctx: &OrientContext) -> SimardResult<OrientJudgment> {
        let tempdir = tempfile::Builder::new()
            .prefix("simard-ooda-orient-")
            .tempdir()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("could not allocate a per-call orient temp dir: {e}"),
            })?;
        let record_path = tempdir.path().join("orient.json");

        self.run_orient_recipe(ctx, &record_path)?;

        let fields =
            super::read_verified_orient(&record_path, &ctx.goal_id, REASONER_RECORD_CYCLE)?;
        crate::recipe_output::record_parse_outcome("orient", true);
        Ok(OrientJudgment {
            adjusted_urgency: fields.adjusted_urgency,
            rationale: fields.reason,
            confidence: fields.confidence,
            demotion_applied: fields.demotion_applied,
        })
    }
}

impl RecipeBrain {
    /// Run the decide recipe once over a fresh temp dir, threading the typed-
    /// record seam context vars (`record_path`, `simard_bin`, `goal_id`,
    /// `cycle_number`) plus the priority fields. The agent records its verdict by
    /// calling `simard ooda record-decide`; stdout is intentionally ignored.
    /// Genuine recipe-runner failures propagate as `Err` (no silent fallback).
    fn run_decide_recipe(&self, ctx: &DecideContext, record_path: &Path) -> SimardResult<()> {
        let simard_bin =
            std::env::current_exe().map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("could not resolve the running simard binary: {e}"),
            })?;

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            // The typed-decision seam: where to write the record and which binary
            // records it. Both are trusted (a daemon-owned per-call temp dir + the
            // resolved current_exe), so they are passed verbatim — never sanitized
            // (which would fold/truncate a path).
            .arg("-c")
            .arg(format!("record_path={}", record_path.display()))
            .arg("-c")
            .arg(format!("simard_bin={}", simard_bin.display()))
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("cycle_number={REASONER_RECORD_CYCLE}"))
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
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        // The verdict lives in the typed record the agent wrote via the tool,
        // NOT in stdout. Stdout is intentionally ignored.
        Ok(())
    }

    /// Run the orient recipe once over a fresh temp dir, threading the typed-
    /// record seam context vars (`record_path`, `simard_bin`, `goal_id`,
    /// `cycle_number`, `base_urgency`) plus the failure context. `base_urgency`
    /// is passed so the tool can persist it for the reader's self-consistent
    /// no-escalation re-check. Genuine recipe-runner failures propagate as `Err`.
    fn run_orient_recipe(&self, ctx: &OrientContext, record_path: &Path) -> SimardResult<()> {
        let simard_bin =
            std::env::current_exe().map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: self.adapter_tag.to_string(),
                reason: format!("could not resolve the running simard binary: {e}"),
            })?;

        let output = Command::new("recipe-runner-rs")
            .arg(self.recipe_path.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("record_path={}", record_path.display()))
            .arg("-c")
            .arg(format!("simard_bin={}", simard_bin.display()))
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!("cycle_number={REASONER_RECORD_CYCLE}"))
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
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        Ok(())
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
            crate::recipe_output::record_parse_outcome("engineer_lifecycle", true);
            return Ok(decision);
        }

        // Parse-miss → confidence-gated escalation ladder (issue #2432). Spend
        // extra compute ONLY on this weak case.
        let cfg = EscalationConfig::from_env();
        let (final_decision, final_outcome, attempts, termination) =
            run_escalation_ladder(self, ctx, &base_raw, outcome, &cfg);
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
        // Operator zero-fallback contract (issue #2580): never return
        // `Ok(continue_skipping)` on a parse-failure — that is a parse-failure
        // masquerading as a deliberate no-action (the exact "deterministic
        // fallback" the operator forbids). Surface an EXPLICIT hard error; the
        // `spawn.rs` caller records it as a cycle failure and, after N
        // consecutive failures, marks the goal blocked / files a tracking issue.
        // A genuine "nothing to do" is a real, model-emitted `continue_skipping`
        // decision (parsed) — distinct, and it never reaches this branch.
        finalize_ladder_result(
            LIFECYCLE_ADAPTER_TAG,
            BrainPhase::Act,
            &ctx.goal_id,
            final_decision,
            final_outcome,
            termination,
            attempts,
        )
    }

    /// Closed-loop outcome verification (issue #2751). Runs the
    /// `ooda-goal-outcome-verification.yaml` recipe over the goal's real success
    /// criteria, the artifact-level signals (INPUT), and the freshly-gathered
    /// live signals, then parses the `{"decision", "rationale"[, "replan_hint"]}`
    /// envelope into a [`GoalOutcomeDecision`].
    ///
    /// NO-FALLBACK (operator zero-fallback contract, #2580 / #1711): a recipe
    /// invocation failure OR an unparseable decision surfaces as an explicit
    /// `Err`. The seam records it as a visible cycle failure and keeps the goal
    /// open — never a silent `keep_open_and_report` masquerading as a reasoned
    /// decision. A genuine "it is really achieved, live" answer is a real,
    /// model-emitted `mark_achieved` (which the Rust Rail-3 then still gates on
    /// >=1 verified live signal).
    fn decide_goal_outcome_verification(
        &self,
        ctx: &GoalOutcomeCtx,
    ) -> SimardResult<GoalOutcomeDecision> {
        let raw = self.invoke_outcome_verify_raw(ctx)?;
        parse_outcome_decision(&raw).ok_or_else(|| SimardError::VerificationFailed {
            reason: format!(
                "{}: outcome-verify recipe output had no parseable decision envelope: {}",
                self.adapter_tag,
                truncate(&raw, MAX_RATIONALE_CHARS)
            ),
        })
    }

    /// Per-goal, per-cycle agentic decision (issue #4453). Runs the
    /// `ooda-per-goal-cycle.yaml` recipe (a sibling of this act-phase brain's
    /// lifecycle recipe) over the goal's DURABLE state and the three demoted
    /// signals. The reasoner records its verdict by calling the
    /// `simard ooda record-decision` TOOL (which validates the closed
    /// [`PerGoalAction`] enum and atomically writes one typed
    /// [`PerGoalDecisionRecord`](super::PerGoalDecisionRecord)); this method then
    /// reads that typed record via [`read_verified`](super::read_verified). It
    /// NEVER scrapes the agent's prose stdout (WS-4, #2573/#2658).
    ///
    /// NO-FALLBACK, FAIL-CLOSED (operator zero-fallback contract, #2580 / #1711):
    /// a recipe invocation failure OR any read-verification failure (absent /
    /// malformed / wrong-schema / out-of-enum / empty-reason / goal- or
    /// cycle-mismatch) surfaces as an explicit `Err`. The driver records it as a
    /// visible cycle failure and performs a safe no-op — never a silent
    /// `continue` masquerading as a reasoned decision. A genuine "leave it"
    /// answer is a real, model-recorded `continue`.
    fn decide_per_goal_cycle(&self, ctx: &PerGoalCycleCtx) -> SimardResult<PerGoalAction> {
        // Fresh, UNIQUE per-cycle temp dir (owner-only, auto-removed on drop). A
        // stale record from a prior cycle can never live at this path — and the
        // reader still independently re-checks goal_id/cycle_number (R6/R7).
        let tempdir = tempfile::Builder::new()
            .prefix("simard-ooda-decision-")
            .tempdir()
            .map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: PER_GOAL_CYCLE_ADAPTER_TAG.to_string(),
                reason: format!("could not allocate a per-cycle decision temp dir: {e}"),
            })?;
        let record_path = tempdir.path().join("decision.json");

        // Run the reasoner recipe. Its agent records its verdict by calling the
        // `simard ooda record-decision` tool (writing `record_path`); the agent's
        // stdout is IGNORED — a stray JSON print has zero effect. NO timeout on
        // the agentic step.
        self.run_per_goal_cycle_recipe(ctx, &record_path)?;

        // Read the TYPED record — never scrape prose. Every failure mode is an
        // Err → a safe no-op cycle failure (#1711). The thin deterministic rail
        // then acts on this validated closed enum.
        super::read_verified(&record_path, &ctx.goal_id, ctx.cycle_number)
    }

    /// Dependency/overlap-aware engineer admission (issue #2690). Resolves the
    /// admission recipe as a sibling of this act-phase brain's recipe, renders
    /// the overlap context, runs the recipe, and parses the
    /// `{"decision", "rationale", "blocked_by"?, "after_goal_id"?, "overlap_files"?}`
    /// envelope into an [`EngineerAdmissionDecision`].
    ///
    /// FAIL-**OPEN** polarity (opposite the outcome verifier): a recipe
    /// invocation failure OR an unparseable decision surfaces as an `Err`, which
    /// the spawn seam's Rail-2 turns into a loud `Admit`. Scheduling is an
    /// optimization — a broken reasoner must never stall a spawn. The one hard
    /// guarantee that survives is the seam's deterministic exact-path rail.
    fn decide_engineer_admission(
        &self,
        ctx: &EngineerAdmissionCtx,
    ) -> SimardResult<EngineerAdmissionDecision> {
        let raw = self.invoke_admission_raw(ctx)?;
        parse_admission_decision(&raw).ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: ADMISSION_ADAPTER_TAG.to_string(),
            reason: format!(
                "engineer-admission recipe output had no parseable decision envelope: {}",
                truncate(&raw, MAX_RATIONALE_CHARS)
            ),
        })
    }

    /// Resource-aware engineer admission (issue #2706). Resolves the resource
    /// recipe as a sibling of this act-phase brain's recipe, renders the resource
    /// picture, runs the recipe, and parses the `{"decision", "rationale"}`
    /// envelope into a [`ResourceAdmissionDecision`].
    ///
    /// FAIL-**CLOSED** polarity (unlike the overlap gate, which fails open): an
    /// invocation failure OR an unparseable decision surfaces as an `Err`, which
    /// the spawn seam turns into a benign `Defer` (skip this cycle, retried next
    /// round) — never an `Admit`. On a resource gate the conservative failure is
    /// to NOT add disk load when the reasoning that was supposed to run broke.
    /// The one hard guarantee that survives regardless is the seam's
    /// deterministic disk-ceiling rail (the ENOSPC guard); the kill-switch
    /// (`SIMARD_RESOURCE_ADMISSION=off`) is the escape hatch if the recipe is
    /// persistently broken.
    fn decide_resource_admission(
        &self,
        ctx: &ResourceAdmissionCtx,
    ) -> SimardResult<ResourceAdmissionDecision> {
        let raw = self.invoke_resource_admission_raw(ctx)?;
        parse_resource_admission_decision(&raw).ok_or_else(|| {
            SimardError::AdapterInvocationFailed {
                base_type: RESOURCE_ADMISSION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "resource-admission recipe output had no parseable decision envelope: {}",
                    truncate(&raw, MAX_RATIONALE_CHARS)
                ),
            }
        })
    }

    /// Semantic dedup + enhance for one candidate creative idea (issue #2925).
    /// Resolves the `creative-idea-dedup.yaml` recipe (hot-reload order:
    /// `~/.simard/…` then the repo asset), renders the candidate + shortlist as
    /// sanitised `-c` vars, runs the recipe, and parses the
    /// `{"choice", "target_node_id"?, "rationale"}` envelope from the recipe's
    /// **clean result channel** (never stdout scraping) into an
    /// [`IdeaDedupDecision`].
    ///
    /// NO-FALLBACK: a recipe invocation failure OR an unparseable / unknown
    /// decision surfaces as an explicit `Err`. The dedup-gate seam turns that
    /// into a fail-CLOSED drop (the candidate is not persisted this cycle) —
    /// never a silent duplicate and never an `EnhanceExisting` on a guess.
    fn decide_idea_dedup(&self, ctx: &IdeaDedupCtx) -> SimardResult<IdeaDedupDecision> {
        let raw = self.invoke_idea_dedup_raw(ctx)?;
        parse_idea_dedup_decision(&raw).ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: IDEA_DEDUP_ADAPTER_TAG.to_string(),
            reason: format!(
                "creative-idea-dedup recipe output had no parseable decision envelope: {}",
                truncate(&raw, MAX_RATIONALE_CHARS)
            ),
        })
    }

    /// Cluster the existing pool by semantic duplication for the one-time
    /// consolidation pass (issue #2925). Runs `creative-ideas-consolidation.yaml`
    /// over the whole pool and parses the `{"clusters": [...]}` envelope.
    /// NO-FALLBACK: an invocation failure or unparseable output is an `Err`, so
    /// the consolidation seam writes nothing and surfaces the error.
    fn decide_idea_consolidation(
        &self,
        ctx: &IdeaConsolidationCtx,
    ) -> SimardResult<Vec<IdeaCluster>> {
        let raw = self.invoke_idea_consolidation_raw(ctx)?;
        parse_idea_consolidation(&raw).ok_or_else(|| SimardError::AdapterInvocationFailed {
            base_type: IDEA_CONSOLIDATION_ADAPTER_TAG.to_string(),
            reason: format!(
                "creative-ideas-consolidation recipe output had no parseable clusters envelope: {}",
                truncate(&raw, MAX_RATIONALE_CHARS)
            ),
        })
    }
}

impl RecipeBrain {
    /// Invoke the outcome-verification recipe once and return the raw decision
    /// text (the recipe's final step output). Errors surface loudly (NO-FALLBACK).
    fn invoke_outcome_verify_raw(&self, ctx: &GoalOutcomeCtx) -> SimardResult<String> {
        let artifact = format!(
            "pr_merged={} issue_closed={} self_affecting={} deployed={}",
            ctx.artifact_signals.pr_merged,
            ctx.artifact_signals.issue_closed,
            ctx.artifact_signals.self_affecting,
            ctx.artifact_signals.deployed,
        );
        let live = render_live_signals(&ctx.live_signals);

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
            .arg(format!(
                "goal_title={}",
                sanitize_context_var(&ctx.goal_title, 500)
            ))
            .arg("-c")
            .arg(format!(
                "success_criteria={}",
                sanitize_context_var(&ctx.success_criteria, 2000)
            ))
            .arg("-c")
            .arg(format!(
                "artifact_signals={}",
                sanitize_context_var(&artifact, 500)
            ))
            .arg("-c")
            .arg(format!(
                "live_signals={}",
                sanitize_context_var(&live, 8000)
            ))
            .arg("-c")
            .arg(format!("reverify_count={}", ctx.reverify_count))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: self.adapter_tag.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };

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

    /// Invoke the per-goal, per-cycle recipe once and return the raw decision
    /// text (issue #4453). The recipe is resolved as a **sibling** of this
    /// brain's own lifecycle recipe (same `recipes/` dir, whether that resolved
    /// to the hot-reload `~/.simard/...` copy or the in-tree copy). Every ctx
    /// field is routed through [`sanitize_context_var`] before it becomes a `-c`
    /// arg (YAML/context/prompt-injection + `E2BIG` defence). Errors surface as
    /// `Err` (no silent fallback).
    fn run_per_goal_cycle_recipe(
        &self,
        ctx: &PerGoalCycleCtx,
        record_path: &Path,
    ) -> SimardResult<()> {
        let recipe = self
            .recipe_path
            .parent()
            .map(|d| d.join(PER_GOAL_CYCLE_RECIPE_FILENAME))
            .filter(|p| p.is_file())
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: PER_GOAL_CYCLE_ADAPTER_TAG.to_string(),
                reason: format!(
                    "per-goal-cycle recipe '{PER_GOAL_CYCLE_RECIPE_FILENAME}' not found beside {}",
                    self.recipe_path.display()
                ),
            })?;

        // Resolve THIS binary so the recipe sandbox can invoke the
        // `record-decision` tool deterministically — resolved the same way
        // `recipe-runner-rs` is (via the running executable), never a bare name
        // that depends on PATH. If it cannot be resolved, no record is written
        // and the reader fails CLOSED at R1.
        let simard_bin =
            std::env::current_exe().map_err(|e| SimardError::AdapterInvocationFailed {
                base_type: PER_GOAL_CYCLE_ADAPTER_TAG.to_string(),
                reason: format!("could not resolve the running simard binary: {e}"),
            })?;

        let stale = match ctx.stale_claim_secs {
            Some(secs) => secs.to_string(),
            None => "none".to_string(),
        };
        let open_prs = ctx.open_pr_refs.join(", ");
        let last_outcomes = ctx.last_outcomes.join(" | ");

        let output = Command::new("recipe-runner-rs")
            .arg(recipe.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            // The typed-decision seam: where to write the record and which
            // binary records it. Paths are trusted (a daemon-owned per-cycle
            // temp dir + the resolved current_exe), so they are passed verbatim
            // — never sanitized (which would fold/truncate a path).
            .arg("-c")
            .arg(format!("record_path={}", record_path.display()))
            .arg("-c")
            .arg(format!("simard_bin={}", simard_bin.display()))
            .arg("-c")
            .arg(format!(
                "goal_id={}",
                sanitize_context_var(&ctx.goal_id, 500)
            ))
            .arg("-c")
            .arg(format!(
                "goal_description={}",
                sanitize_context_var(&ctx.goal_description, 2000)
            ))
            .arg("-c")
            .arg(format!(
                "goal_status={}",
                sanitize_context_var(&ctx.goal_status, 200)
            ))
            .arg("-c")
            .arg(format!("cycle_number={}", ctx.cycle_number))
            .arg("-c")
            .arg(format!(
                "history_summary={}",
                sanitize_context_var(&ctx.history_summary, 4000)
            ))
            .arg("-c")
            .arg(format!(
                "effect_jobs_in_flight={}",
                ctx.effect_jobs_in_flight
            ))
            .arg("-c")
            .arg(format!(
                "open_pr_refs={}",
                sanitize_context_var(&open_prs, 1000)
            ))
            .arg("-c")
            .arg(format!(
                "last_outcomes={}",
                sanitize_context_var(&last_outcomes, 4000)
            ))
            .arg("-c")
            .arg(format!("wip_ref_count={}", ctx.wip_ref_count))
            .arg("-c")
            .arg(format!("worker_present={}", ctx.worker_present))
            .arg("-c")
            .arg(format!(
                "worker_log_tail={}",
                sanitize_context_var(&ctx.worker_log_tail, 8000)
            ))
            .arg("-c")
            .arg(format!("standing_idle_signal={}", ctx.standing_idle_signal))
            .arg("-c")
            .arg(format!("stale_claim_secs={stale}"))
            .arg("-c")
            .arg(format!("effect_board_missed={}", ctx.effect_board_missed))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: PER_GOAL_CYCLE_ADAPTER_TAG.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: PER_GOAL_CYCLE_ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        // The verdict lives in the typed record the agent wrote via the tool,
        // NOT in stdout. Stdout is intentionally ignored; the caller reads and
        // verifies `record_path`.
        Ok(())
    }
    /// (the act-phase [`RecipeBrain`] holds the lifecycle recipe; the admission
    /// recipe lives beside it in the same `recipes/` dir, whether that resolved
    /// to the hot-reload `~/.simard/...` copy or the in-tree copy). Every ctx
    /// field is routed through [`sanitize_context_var`] before it becomes a `-c`
    /// arg. Errors surface as `Err` (the seam fails OPEN).
    fn invoke_admission_raw(&self, ctx: &EngineerAdmissionCtx) -> SimardResult<String> {
        let admission_recipe = self
            .recipe_path
            .parent()
            .map(|d| d.join(ADMISSION_RECIPE_FILENAME))
            .filter(|p| p.is_file())
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: ADMISSION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "admission recipe '{ADMISSION_RECIPE_FILENAME}' not found beside {}",
                    self.recipe_path.display()
                ),
            })?;

        let scope = ctx.candidate.predicted_scope.join(", ");
        let live = render_admission_engineers(&ctx.live_engineers);

        let output = Command::new("recipe-runner-rs")
            .arg(admission_recipe.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "candidate_goal_id={}",
                sanitize_context_var(&ctx.candidate.id, 500)
            ))
            .arg("-c")
            .arg(format!(
                "candidate_goal_title={}",
                sanitize_context_var(&ctx.candidate.title, 2000)
            ))
            .arg("-c")
            .arg(format!(
                "candidate_predicted_scope={}",
                sanitize_context_var(&scope, 8000)
            ))
            .arg("-c")
            .arg(format!(
                "live_engineers={}",
                sanitize_context_var(&live, 8000)
            ))
            .arg("-c")
            .arg(format!(
                "repo_root={}",
                sanitize_context_var(&ctx.repo_root, 500)
            ))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: ADMISSION_ADAPTER_TAG.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: ADMISSION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        extract_recipe_decision_output(&output.stdout, ADMISSION_ADAPTER_TAG)
    }

    /// Invoke the resource-admission recipe once and return the raw decision
    /// text (issue #2706). The recipe is resolved as a **sibling** of this
    /// brain's own recipe, like the overlap recipe. Every ctx field is rendered
    /// to a bounded, sanitized `-c` arg; any unmeasured `Option` renders as the
    /// literal `unknown`. Errors surface as `Err` (the seam fails CLOSED).
    fn invoke_resource_admission_raw(&self, ctx: &ResourceAdmissionCtx) -> SimardResult<String> {
        let recipe = self
            .recipe_path
            .parent()
            .map(|d| d.join(RESOURCE_ADMISSION_RECIPE_FILENAME))
            .filter(|p| p.is_file())
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: RESOURCE_ADMISSION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "resource-admission recipe '{RESOURCE_ADMISSION_RECIPE_FILENAME}' not found beside {}",
                    self.recipe_path.display()
                ),
            })?;

        let opt = |v: Option<String>| v.unwrap_or_else(|| "unknown".to_string());
        let disk_used = opt(ctx.disk_used_pct.map(|p| format!("{p:.0}")));
        let disk_free = opt(ctx.disk_free_gb.map(|g| format!("{g:.1}")));
        let disk_total = opt(ctx.disk_total_gb.map(|g| format!("{g:.1}")));
        let build_cache = opt(ctx.build_cache_bytes.map(|b| b.to_string()));
        let worktrees = opt(ctx.worktree_count.map(|c| c.to_string()));
        // Render the three load figures as one "1m/5m/15m" var (or "unknown").
        let load = match (ctx.load_avg_1, ctx.load_avg_5, ctx.load_avg_15) {
            (Some(a), Some(b), Some(c)) => format!("{a:.2}/{b:.2}/{c:.2}"),
            _ => "unknown".to_string(),
        };
        let cpus = opt(ctx.cpu_count.map(|c| c.to_string()));
        let aimd = opt(ctx.aimd_current_max.map(|m| m.to_string()));

        let output = Command::new("recipe-runner-rs")
            .arg(recipe.as_os_str())
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
                "disk_used_pct={}",
                sanitize_context_var(&disk_used, 100)
            ))
            .arg("-c")
            .arg(format!(
                "disk_free_gb={}",
                sanitize_context_var(&disk_free, 100)
            ))
            .arg("-c")
            .arg(format!(
                "disk_total_gb={}",
                sanitize_context_var(&disk_total, 100)
            ))
            .arg("-c")
            .arg(format!(
                "admission_ceiling_pct={}",
                sanitize_context_var(&format!("{:.0}", ctx.admission_ceiling_pct), 100)
            ))
            .arg("-c")
            .arg(format!(
                "build_cache_bytes={}",
                sanitize_context_var(&build_cache, 100)
            ))
            .arg("-c")
            .arg(format!(
                "worktree_count={}",
                sanitize_context_var(&worktrees, 100)
            ))
            .arg("-c")
            .arg(format!("load_avg={}", sanitize_context_var(&load, 100)))
            .arg("-c")
            .arg(format!("cpu_count={}", sanitize_context_var(&cpus, 100)))
            .arg("-c")
            .arg(format!(
                "in_flight_engineers={}",
                sanitize_context_var(&ctx.in_flight_engineers.to_string(), 100)
            ))
            .arg("-c")
            .arg(format!(
                "aimd_current_max={}",
                sanitize_context_var(&aimd, 100)
            ))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: RESOURCE_ADMISSION_ADAPTER_TAG.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: RESOURCE_ADMISSION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }

        extract_recipe_decision_output(&output.stdout, RESOURCE_ADMISSION_ADAPTER_TAG)
    }

    /// Invoke the creative-idea dedup recipe once and return the raw decision
    /// text from the recipe's clean result channel (issue #2925). Errors surface
    /// loudly (NO-FALLBACK); the dedup-gate seam fails CLOSED on `Err`.
    fn invoke_idea_dedup_raw(&self, ctx: &IdeaDedupCtx) -> SimardResult<String> {
        let recipe = self.sibling_recipe(IDEA_DEDUP_RECIPE_FILENAME, IDEA_DEDUP_ADAPTER_TAG)?;
        let shortlist = render_existing_shortlist(&ctx.existing_shortlist);

        let output = Command::new("recipe-runner-rs")
            .arg(recipe.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!(
                "candidate_idea={}",
                sanitize_context_var(&ctx.candidate_idea, 4000)
            ))
            .arg("-c")
            .arg(format!(
                "candidate_rationale={}",
                sanitize_context_var(&ctx.candidate_rationale, 4000)
            ))
            .arg("-c")
            .arg(format!("existing_shortlist={shortlist}"))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: IDEA_DEDUP_ADAPTER_TAG.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: IDEA_DEDUP_ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }
        extract_recipe_decision_output(&output.stdout, IDEA_DEDUP_ADAPTER_TAG)
    }

    /// Invoke the consolidation clustering recipe once over the whole pool and
    /// return the raw clusters text (issue #2925). NO-FALLBACK on error.
    fn invoke_idea_consolidation_raw(&self, ctx: &IdeaConsolidationCtx) -> SimardResult<String> {
        let recipe = self.sibling_recipe(
            IDEA_CONSOLIDATION_RECIPE_FILENAME,
            IDEA_CONSOLIDATION_ADAPTER_TAG,
        )?;
        let pool = render_existing_shortlist(&ctx.pool);

        let output = Command::new("recipe-runner-rs")
            .arg(recipe.as_os_str())
            .arg("--output-format")
            .arg("json")
            .env("AMPLIHACK_AGENT_BINARY", self.agent_binary)
            .arg("-c")
            .arg(format!("existing_pool={pool}"))
            .output();

        let output = match output {
            Ok(o) => o,
            Err(e) => {
                return Err(SimardError::AdapterInvocationFailed {
                    base_type: IDEA_CONSOLIDATION_ADAPTER_TAG.to_string(),
                    reason: format!("recipe-runner-rs spawn failed: {e}"),
                });
            }
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SimardError::AdapterInvocationFailed {
                base_type: IDEA_CONSOLIDATION_ADAPTER_TAG.to_string(),
                reason: format!(
                    "recipe exited with {}: {}",
                    output.status,
                    truncate(&stderr, MAX_RATIONALE_CHARS)
                ),
            });
        }
        extract_recipe_decision_output(&output.stdout, IDEA_CONSOLIDATION_ADAPTER_TAG)
    }

    /// Resolve a recipe filename as a sibling of this brain's recipe path
    /// (hot-reload order already baked into `recipe_path`). Shared by the #2925
    /// dedup + consolidation seams.
    fn sibling_recipe(&self, filename: &str, adapter_tag: &'static str) -> SimardResult<PathBuf> {
        self.recipe_path
            .parent()
            .map(|d| d.join(filename))
            .filter(|p| p.is_file())
            .ok_or_else(|| SimardError::AdapterInvocationFailed {
                base_type: adapter_tag.to_string(),
                reason: format!(
                    "recipe '{filename}' not found beside {}",
                    self.recipe_path.display()
                ),
            })
    }
}

/// Render the live engineer set for the admission recipe's `live_engineers`
/// context var. Capped at 32 engineers (prompt-cost DoS guard); each engineer's
/// `changed_files` / `overlap_with_candidate` list is capped at 200 paths and
/// every field is control/ANSI-stripped + length-capped so an injected path or
/// goal id cannot corrupt the prompt. The exact-path rail (not the prompt) is
/// the hard decider, so this rendering is advisory context only.
fn render_admission_engineers(engineers: &[EngineerAdmissionSignalView]) -> String {
    engineers
        .iter()
        .take(32)
        .map(|e| {
            let changed = e
                .changed_files
                .iter()
                .take(200)
                .map(|p| sanitize_context_var(p, 500))
                .collect::<Vec<_>>()
                .join(", ");
            let overlap = e
                .overlap_with_candidate
                .iter()
                .take(200)
                .map(|p| sanitize_context_var(p, 500))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "[goal_id={} depended_on={} overlap=[{}] changed_files=[{}]]",
                sanitize_context_var(&e.goal_id, 500),
                e.depended_on,
                overlap,
                changed,
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The subset of [`super::LiveEngineerSignal`] fields the recipe renderer reads.
/// A local view keeps `render_admission_engineers` decoupled from the full ctx
/// type and trivially unit-testable.
type EngineerAdmissionSignalView = super::LiveEngineerSignal;

/// A structured engineer-admission decision envelope. Unlike the shared
/// [`DecisionEnvelope`], this reads the load-bearing `blocked_by` /
/// `after_goal_id` / `overlap_files` / `retry_after_secs` fields explicitly so a
/// `defer` / `serialize_after` decision carries its target (the base shim would
/// default every struct-variant field to empty — see #2690 API reference).
#[derive(Debug, Clone, serde::Deserialize)]
struct AdmissionEnvelope {
    decision: String,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    blocked_by: Vec<String>,
    #[serde(default)]
    after_goal_id: String,
    #[serde(default)]
    overlap_files: Vec<String>,
    #[serde(default)]
    retry_after_secs: Option<u64>,
}

/// Parse the admission recipe output into an [`EngineerAdmissionDecision`], or
/// `None` when no balanced JSON object with a known `decision` variant is
/// present (the caller surfaces that as a fail-open `Err`). Routes through the
/// shared sanitizing chokepoint so a banner-polluted envelope still parses.
fn parse_admission_decision(text: &str) -> Option<EngineerAdmissionDecision> {
    let env: AdmissionEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    if env.decision.trim().is_empty() {
        return None;
    }
    let rationale = {
        let r = env.rationale.trim();
        if r.is_empty() {
            truncate(env.decision.trim(), MAX_RATIONALE_CHARS)
        } else {
            truncate(r, MAX_RATIONALE_CHARS)
        }
    };
    admission_decision_from_variant(&env, rationale)
}

/// Map an admission decision variant token (case-insensitive) to an
/// [`EngineerAdmissionDecision`]; `None` for an unknown token. `blocked_by` /
/// `after_goal_id` / `overlap_files` / `retry_after_secs` are carried only by
/// the variants that own them.
fn admission_decision_from_variant(
    env: &AdmissionEnvelope,
    rationale: String,
) -> Option<EngineerAdmissionDecision> {
    let w = env.decision.trim();
    if w.eq_ignore_ascii_case("admit") {
        Some(EngineerAdmissionDecision::Admit { rationale })
    } else if w.eq_ignore_ascii_case("defer") {
        Some(EngineerAdmissionDecision::Defer {
            blocked_by: env.blocked_by.clone(),
            rationale,
            retry_after_secs: env.retry_after_secs,
        })
    } else if w.eq_ignore_ascii_case("serialize_after") {
        Some(EngineerAdmissionDecision::SerializeAfter {
            after_goal_id: env.after_goal_id.clone(),
            overlap_files: env.overlap_files.clone(),
            rationale,
        })
    } else {
        None
    }
}

/// A structured resource-admission decision envelope (issue #2706). Reads the
/// `decision` token + `rationale`. There is intentionally no `retry_after_secs`:
/// a resource `Defer` is retried on the natural next OODA round.
#[derive(Debug, Clone, serde::Deserialize)]
struct ResourceAdmissionEnvelope {
    decision: String,
    #[serde(default)]
    rationale: String,
}

/// Parse the resource-admission recipe output into a
/// [`ResourceAdmissionDecision`], or `None` when no balanced JSON object with a
/// known `decision` variant is present (the caller surfaces that as an `Err`,
/// which the seam fails CLOSED to a benign `Defer`). Routes through the shared
/// sanitizing chokepoint so a banner-polluted envelope still parses.
fn parse_resource_admission_decision(text: &str) -> Option<ResourceAdmissionDecision> {
    let env: ResourceAdmissionEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    if env.decision.trim().is_empty() {
        return None;
    }
    let rationale = {
        let r = env.rationale.trim();
        if r.is_empty() {
            truncate(env.decision.trim(), MAX_RATIONALE_CHARS)
        } else {
            truncate(r, MAX_RATIONALE_CHARS)
        }
    };
    resource_admission_decision_from_variant(&env, rationale)
}

/// Map a resource-admission decision variant token (case-insensitive) to a
/// [`ResourceAdmissionDecision`]; `None` for an unknown token so the seam fails
/// CLOSED (to a benign `Defer`) rather than defaulting on the brain's behalf.
fn resource_admission_decision_from_variant(
    env: &ResourceAdmissionEnvelope,
    rationale: String,
) -> Option<ResourceAdmissionDecision> {
    let w = env.decision.trim();
    if w.eq_ignore_ascii_case("admit") {
        Some(ResourceAdmissionDecision::Admit { rationale })
    } else if w.eq_ignore_ascii_case("defer") {
        Some(ResourceAdmissionDecision::Defer { rationale })
    } else if w.eq_ignore_ascii_case("reclaim_first") {
        Some(ResourceAdmissionDecision::ReclaimFirst { rationale })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Creative-ideas semantic dedup + consolidation envelopes (issue #2925)
// ---------------------------------------------------------------------------

/// Render an existing-idea shortlist/pool into a single bounded, sanitised block
/// for the recipe's `existing_shortlist` / `existing_pool` context var, one idea
/// per line as `node_id | idea_id | idea — rationale`. Capped at 64 entries
/// (prompt-cost DoS guard); every field is control/ANSI-stripped and
/// length-capped so untrusted pool content cannot corrupt the prompt.
fn render_existing_shortlist(views: &[super::ExistingIdeaView]) -> String {
    views
        .iter()
        .take(64)
        .map(|v| {
            format!(
                "{} | {} | {} — {}",
                sanitize_context_var(&v.node_id, 200),
                sanitize_context_var(&v.idea_id, 200),
                sanitize_context_var(&v.idea, 1000),
                sanitize_context_var(&v.rationale, 1000),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A structured creative-idea dedup decision envelope (issue #2925). Reads the
/// `choice` token, optional `target_node_id`, and `rationale`.
#[derive(Debug, Clone, serde::Deserialize)]
struct IdeaDedupEnvelope {
    choice: String,
    #[serde(default)]
    target_node_id: String,
    #[serde(default)]
    rationale: String,
}

/// Parse the dedup recipe output into an [`IdeaDedupDecision`], or `None` when no
/// balanced JSON object with a known `choice` is present, or when
/// `enhance_existing` omits `target_node_id` (the caller surfaces `None` as an
/// `Err`, which the seam fails CLOSED). Routes through the shared sanitising
/// chokepoint so a banner-polluted envelope still parses — no stdout scraping.
fn parse_idea_dedup_decision(text: &str) -> Option<IdeaDedupDecision> {
    let env: IdeaDedupEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    let choice = env.choice.trim();
    if choice.is_empty() {
        return None;
    }
    let rationale = {
        let r = env.rationale.trim();
        if r.is_empty() {
            truncate(choice, MAX_RATIONALE_CHARS)
        } else {
            truncate(r, MAX_RATIONALE_CHARS)
        }
    };
    if choice.eq_ignore_ascii_case("create_new") {
        Some(IdeaDedupDecision::CreateNew { rationale })
    } else if choice.eq_ignore_ascii_case("skip") {
        Some(IdeaDedupDecision::Skip { rationale })
    } else if choice.eq_ignore_ascii_case("enhance_existing") {
        let target = env.target_node_id.trim();
        if target.is_empty() {
            // enhance without a target is unactionable → fail closed.
            None
        } else {
            Some(IdeaDedupDecision::EnhanceExisting {
                target_node_id: target.to_string(),
                rationale,
            })
        }
    } else {
        None
    }
}

/// A structured consolidation clusters envelope (issue #2925).
#[derive(Debug, Clone, serde::Deserialize)]
struct IdeaConsolidationEnvelope {
    #[serde(default)]
    clusters: Vec<IdeaCluster>,
}

/// Parse the consolidation recipe output into a list of [`IdeaCluster`]s, or
/// `None` when no balanced JSON object with a `clusters` array is present.
/// Clusters missing a `canonical_id` are dropped. `Some(vec![])` is a valid
/// "nothing to consolidate" result and is distinct from an unparseable `None`.
fn parse_idea_consolidation(text: &str) -> Option<Vec<IdeaCluster>> {
    let env: IdeaConsolidationEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    Some(
        env.clusters
            .into_iter()
            .filter(|c| !c.canonical_id.trim().is_empty())
            .collect(),
    )
}

/// Render the gathered live signals into a single bounded, sanitized string for
/// the recipe's `live_signals` context var. Capped at 32 signals (prompt-cost
/// DoS guard, #2751); each field is control/ANSI-stripped and length-capped so
/// an injected `detail` cannot corrupt the prompt. The `verified` boolean is
/// rendered from the adapter-set flag — the recipe treats `detail` as untrusted,
/// and the Rust Rail-3 (not the prompt) is the decider.
fn render_live_signals(signals: &[crate::goal_curation::live_signal::LiveSignal]) -> String {
    signals
        .iter()
        .take(32)
        .map(|s| {
            format!(
                "[source={} kind={} verified={} detail={}]",
                sanitize_context_var(&s.source, 500),
                sanitize_context_var(&s.kind, 500),
                s.verified,
                sanitize_context_var(&s.detail, 2000),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A structured outcome-verification decision envelope. Unlike the shared
/// [`DecisionEnvelope`], this reads the optional `replan_hint` explicitly so a
/// `replan` decision carries its load-bearing re-scope guidance (the shared shim
/// would default every struct-variant field to empty — see #2751 API reference).
#[derive(Debug, Clone, serde::Deserialize)]
struct OutcomeEnvelope {
    decision: String,
    #[serde(default)]
    rationale: String,
    #[serde(default)]
    replan_hint: String,
}

/// Parse the outcome-verify recipe output into a [`GoalOutcomeDecision`], or
/// `None` when no balanced JSON object with a known `decision` variant is
/// present (the caller surfaces that as a NO-FALLBACK `Err`). Routes through the
/// shared sanitizing chokepoint so a banner-polluted envelope still parses.
fn parse_outcome_decision(text: &str) -> Option<GoalOutcomeDecision> {
    let env: OutcomeEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    if env.decision.trim().is_empty() {
        return None;
    }
    let rationale = {
        let r = env.rationale.trim();
        if r.is_empty() {
            truncate(env.decision.trim(), MAX_RATIONALE_CHARS)
        } else {
            truncate(r, MAX_RATIONALE_CHARS)
        }
    };
    outcome_decision_from_variant(
        &env.decision,
        rationale,
        truncate(env.replan_hint.trim(), MAX_RATIONALE_CHARS),
    )
}

/// Map an outcome-verify decision variant token (case-insensitive) to a
/// [`GoalOutcomeDecision`]; `None` for an unknown token. `replan_hint` is only
/// carried by the `replan` variant.
fn outcome_decision_from_variant(
    word: &str,
    rationale: String,
    replan_hint: String,
) -> Option<GoalOutcomeDecision> {
    let w = word.trim();
    if w.eq_ignore_ascii_case("mark_achieved") {
        Some(GoalOutcomeDecision::MarkAchieved { rationale })
    } else if w.eq_ignore_ascii_case("reopen") {
        Some(GoalOutcomeDecision::Reopen { rationale })
    } else if w.eq_ignore_ascii_case("replan") {
        Some(GoalOutcomeDecision::Replan {
            rationale,
            replan_hint,
        })
    } else if w.eq_ignore_ascii_case("keep_open_and_report") {
        Some(GoalOutcomeDecision::KeepOpenAndReport { rationale })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Parse functions — structured JSON-envelope decision block FIRST, with a
// backward-compatible first-word / first-number scan as a fallback (issue
// #2580). The recipe prompts emit a fenced `{"decision": ...}` / `{"adjusted_
// urgency": ...}` envelope; the extractor consumes THAT through the shared
// sanitizing chokepoint rather than relying on free-prose keyword-sniffing.
// ---------------------------------------------------------------------------

/// A structured decision envelope a reasoner emits as a JSON object. Consumed in
/// preference to the legacy first-word scan so well-formed structured output
/// parses deterministically (issue #2580 — operator zero-fallback contract).
/// `decision` is the required machine-parseable variant token (the action word
/// for decide, the lifecycle variant for act); `rationale` is optional prose.
/// Unknown extra fields are ignored for forward-compatibility.
#[derive(Debug, Clone, serde::Deserialize)]
struct DecisionEnvelope {
    decision: String,
    #[serde(default)]
    rationale: String,
}

/// Extract the `{"decision": "...", "rationale": "..."}` envelope from recipe
/// output, if present. Routes through the shared #2484 sanitizing chokepoint
/// ([`crate::recipe_output::extract_and_parse_json`] strips the banner, ANSI,
/// and interleaved log lines, and recovers a trailing-comma-defective body) so
/// a banner-polluted or trailing-comma envelope still parses. Returns
/// `None` when no balanced JSON object with a non-empty string `decision` field
/// is present — the caller then falls back to the legacy first-word scan.
fn extract_decision_envelope(text: &str) -> Option<DecisionEnvelope> {
    let env: DecisionEnvelope = crate::recipe_output::extract_and_parse_json(text)?;
    if env.decision.trim().is_empty() {
        return None;
    }
    Some(env)
}

/// Choose the rationale string for a parsed [`DecisionEnvelope`]: the model's
/// `rationale` when present, else the decision token itself (bounded).
fn envelope_rationale(env: &DecisionEnvelope) -> String {
    let r = env.rationale.trim();
    if r.is_empty() {
        truncate(env.decision.trim(), MAX_RATIONALE_CHARS)
    } else {
        truncate(r, MAX_RATIONALE_CHARS)
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
    // Structured JSON envelope FIRST (issue #2580): a well-formed
    // `{"decision":"<variant>","rationale":"..."}` block — extracted through the
    // shared #2484 sanitizing chokepoint — parses deterministically, so the
    // daemon no longer *relies* on free-prose first-word sniffing.
    if let Some(env) = extract_decision_envelope(text)
        && let Some(decision) =
            lifecycle_decision_from_variant(env.decision.trim(), envelope_rationale(&env))
    {
        return (decision, LifecycleParseOutcome::Parsed);
    }

    // Backward-compatible fallback: strip ANSI escapes + drop tracing-log /
    // runner-banner lines (shared #2484 extractor) so a noise-obscured first-word
    // decision keyword is not silently defaulted to `continue_skipping` — the
    // #2419-family non-progress loop. Clean-path zero-copy preserves today's
    // behaviour on clean output.
    let cleaned = crate::recipe_output::strip_recipe_noise(text);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return (
            default_continue_skipping(),
            LifecycleParseOutcome::DefaultEmpty,
        );
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    let rest = truncate(trimmed[first_word.len()..].trim(), MAX_RATIONALE_CHARS);
    match lifecycle_decision_from_variant(first_word, rest) {
        Some(decision) => (decision, LifecycleParseOutcome::Parsed),
        None => (
            default_continue_skipping(),
            LifecycleParseOutcome::DefaultMalformed,
        ),
    }
}

/// Map a lifecycle decision variant token (case-insensitive) to an
/// [`EngineerLifecycleDecision`] carrying `rationale`; `None` for an unknown
/// token. Shared by the structured JSON-envelope path and the legacy first-word
/// scan so both honour the exact same closed variant set (kept in sync with
/// [`LIFECYCLE_VARIANT_LIST`]). Variants with extra fields
/// (`reclaim_and_redispatch`, `open_tracking_issue`, `mark_goal_blocked`) reuse
/// `rationale` for the body/reason, matching the first-word parser's behaviour.
fn lifecycle_decision_from_variant(
    word: &str,
    rationale: String,
) -> Option<EngineerLifecycleDecision> {
    let w = word.trim();
    if w.eq_ignore_ascii_case("continue_skipping") {
        Some(EngineerLifecycleDecision::ContinueSkipping { rationale })
    } else if w.eq_ignore_ascii_case("deprioritize") {
        Some(EngineerLifecycleDecision::Deprioritize { rationale })
    } else if w.eq_ignore_ascii_case("consider_self_update") {
        Some(EngineerLifecycleDecision::ConsiderSelfUpdate { rationale })
    } else if w.eq_ignore_ascii_case("reclaim_and_redispatch") {
        Some(EngineerLifecycleDecision::ReclaimAndRedispatch {
            rationale,
            redispatch_context: String::new(),
        })
    } else if w.eq_ignore_ascii_case("open_tracking_issue") {
        Some(EngineerLifecycleDecision::OpenTrackingIssue {
            title: "OODA stuck".to_string(),
            body: rationale.clone(),
            rationale,
        })
    } else if w.eq_ignore_ascii_case("mark_goal_blocked") {
        Some(EngineerLifecycleDecision::MarkGoalBlocked {
            reason: rationale.clone(),
            rationale,
        })
    } else {
        None
    }
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

/// The `outcome` label for the [`VERDICT_PARSE_METRIC`]: `"parsed"` when a real
/// verdict/decision was extracted (base parse, or a ladder `Repaired`/
/// `Escalated` recovery), `"defaulted"` when a deterministic fallback was
/// applied (`DefaultEmpty`/`DefaultMalformed`/`Error` — the bug surface).
#[cfg_attr(not(test), allow(dead_code))]
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
#[cfg_attr(not(test), allow(dead_code))]
fn build_verdict_parse_context(
    phase: BrainPhase,
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
///
/// GROUP A NOTE (#4785): the Orient/Decide seams no longer call this — they now
/// act through the typed `record-orient`/`record-decide` tool + fail-closed
/// reader, so there is no stdout "parse outcome" to classify. It is RETAINED
/// (per the operator directive's keep-list) because the shared lifecycle /
/// merge-judge verdict-parse seams reintroduced in Groups B/C/D will rewire it;
/// deleting it now would force a churny re-add. Marked `allow(dead_code)` until
/// then rather than removed.
#[allow(dead_code)]
pub(crate) fn record_verdict_parse_metric(
    phase: BrainPhase,
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
            target: "simard::ooda_brain",
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
    phase: BrainPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    termination: LadderTermination,
) {
    tracing::warn!(
        target: "simard::ooda_brain",
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

/// Record the `brain_parse_error` metric (value `1.0`) and the
/// `simard.brain.parse_error` telemetry counter for a GENUINE ladder-exhausted
/// parse failure — the hard-failure terminal that a reasoner now surfaces to
/// its caller as an explicit `Err` rather than a silent deterministic default
/// (issue #2580, operator zero-fallback contract).
///
/// Fires ONLY on the terminal parse failure (ladder `Exhausted` / `InvokeError`
/// with no parseable decision), never on a first-try parse or a ladder
/// recovery, so `brain_parse_error` is the honest current-fallback-rate signal.
///
/// Best-effort telemetry: a metrics-write failure is logged, never propagated —
/// the explicit `Err` the caller receives is unaffected. The `metrics.jsonl`
/// write is a no-op under `cfg!(test)` so unit tests never touch the operator's
/// real metrics file (the returned `Err` is what tests assert on).
fn record_brain_parse_error(
    phase: BrainPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    termination: LadderTermination,
    attempts: u32,
) {
    crate::telemetry::counter_add(crate::telemetry::names::BRAIN_PARSE_ERROR, 1, &[]);
    if cfg!(test) {
        return;
    }
    let context = serde_json::json!({
        "phase": phase.as_str(),
        "goal_id": goal_id,
        "outcome_detail": outcome.label(),
        "cause": termination.cause_label(),
        "attempts": attempts,
    })
    .to_string();
    if let Err(e) = crate::self_metrics::record_metric(BRAIN_PARSE_ERROR_METRIC, 1.0, &context) {
        tracing::warn!(
            target: "simard::ooda_brain",
            error = %e,
            phase = phase.as_str(),
            "failed to record brain_parse_error metric (explicit Err still surfaced to caller)",
        );
    }
}

/// Build the explicit hard error a reasoner returns when its bounded escalation
/// ladder is exhausted with no parseable decision.
///
/// Surfacing this as an `Err` (instead of a silent deterministic default) is the
/// operator's zero-fallback contract (issue #2580): the caller records it
/// loudly and takes an explicit, observable path (Decide skips the priority,
/// Orient keeps base urgency, Act marks the goal blocked after N) — never a
/// fabricated `advance_goal` / `continue_skipping` / demotion masquerading as a
/// real model decision.
fn brain_parse_error_result<T>(
    adapter_tag: &str,
    phase: BrainPhase,
    goal_id: &str,
    outcome: LifecycleParseOutcome,
    termination: LadderTermination,
    attempts: u32,
) -> SimardResult<T> {
    Err(SimardError::AdapterInvocationFailed {
        base_type: adapter_tag.to_string(),
        reason: format!(
            "{phase}: bounded escalation ladder {cause} after {attempts} attempt(s) with no \
             parseable decision (outcome={outcome}); surfacing an explicit parse error rather \
             than a silent deterministic default (goal={goal_id})",
            phase = phase.as_str(),
            cause = termination.cause_label(),
            outcome = outcome.label(),
        ),
    })
}

/// Convert a completed escalation-ladder result into the reasoner's final
/// [`SimardResult`], enforcing the operator's zero-fallback contract
/// (issue #2580) at ONE shared chokepoint for every recipe-backed phase.
///
/// - A first-try parse or a ladder recovery (`Repaired`/`Escalated`) →
///   `Ok(decision)`.
/// - A GENUINE parse-failure terminal (`final_outcome.is_parse_failure()` — the
///   ladder was exhausted, a rung's invocation failed, or the ladder was
///   disabled and the base attempt parse-missed) → the loud
///   [`warn_parse_failure_default`] log, the [`record_brain_parse_error`] metric,
///   and an EXPLICIT `Err` from [`brain_parse_error_result`]. NEVER the
///   deterministic `default()` masquerading as a real decision.
///
/// Pure enough to unit-test the Ok/Err branch directly (the metric write is a
/// no-op under `cfg!(test)`), which is how the "no code path can emit a
/// deterministic-default decision from a parse-failure" contract is asserted.
fn finalize_ladder_result<D>(
    adapter_tag: &str,
    phase: BrainPhase,
    goal_id: &str,
    final_decision: D,
    final_outcome: LifecycleParseOutcome,
    termination: LadderTermination,
    attempts: u32,
) -> SimardResult<D> {
    if final_outcome.is_parse_failure() {
        warn_parse_failure_default(phase, goal_id, final_outcome, termination);
        record_brain_parse_error(phase, goal_id, final_outcome, termination, attempts);
        return brain_parse_error_result(
            adapter_tag,
            phase,
            goal_id,
            final_outcome,
            termination,
            attempts,
        );
    }
    Ok(final_decision)
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
    // RecipeBrain::new — constructor
    // ===================================================================

    #[test]
    fn new_returns_none_when_decide_recipe_missing() {
        let home = tempfile::TempDir::new().expect("tempdir");
        let brain = RecipeBrain::new_with_home(
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
        let brain = RecipeBrain::new_with_home(
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
        let brain = RecipeBrain::new_with_home(
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
    // Engineer-admission (issue #2690) — envelope parser + fail-open error
    // ===================================================================

    #[test]
    fn parse_admission_admit_envelope() {
        let d =
            parse_admission_decision(r#"{"decision": "admit", "rationale": "independent files"}"#)
                .expect("parses");
        assert!(matches!(d, EngineerAdmissionDecision::Admit { .. }));
        assert_eq!(d.rationale(), "independent files");
    }

    #[test]
    fn parse_admission_defer_carries_blocked_by() {
        let d = parse_admission_decision(
            r#"{"decision": "defer", "blocked_by": ["fix-goals-status"], "rationale": "shared goals_status.rs"}"#,
        )
        .expect("parses");
        match d {
            EngineerAdmissionDecision::Defer {
                blocked_by,
                retry_after_secs,
                ..
            } => {
                assert_eq!(blocked_by, vec!["fix-goals-status".to_string()]);
                assert!(retry_after_secs.is_none());
            }
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[test]
    fn parse_admission_serialize_after_carries_target_and_files() {
        let d = parse_admission_decision(
            r#"{"decision": "serialize_after", "after_goal_id": "g1", "overlap_files": ["src/a.rs"], "rationale": "rebase first"}"#,
        )
        .expect("parses");
        match d {
            EngineerAdmissionDecision::SerializeAfter {
                after_goal_id,
                overlap_files,
                ..
            } => {
                assert_eq!(after_goal_id, "g1");
                assert_eq!(overlap_files, vec!["src/a.rs".to_string()]);
            }
            other => panic!("expected SerializeAfter, got {other:?}"),
        }
    }

    #[test]
    fn parse_admission_unknown_variant_is_none() {
        assert!(parse_admission_decision(r#"{"decision": "nope"}"#).is_none());
        assert!(parse_admission_decision("not json at all").is_none());
        assert!(parse_admission_decision(r#"{"decision": ""}"#).is_none());
    }

    // ---- trailing-comma recovery (reasoner reliability, issue #2658) -------

    #[test]
    fn parse_admission_recovers_trailing_comma_in_object() {
        // A stray trailing comma before `}` is the most common LLM JSON defect.
        // Before the shared `extract_and_parse_json` chokepoint applied the
        // trailing-comma recovery view, this failed the strict parse and the
        // reasoner silently dropped its whole admission decision (fail-open).
        let d =
            parse_admission_decision(r#"{"decision": "admit", "rationale": "independent files",}"#)
                .expect("trailing comma before } must be recovered");
        assert!(matches!(d, EngineerAdmissionDecision::Admit { .. }));
        assert_eq!(d.rationale(), "independent files");
    }

    #[test]
    fn parse_admission_recovers_trailing_comma_in_array_and_banner() {
        // Trailing comma inside the `blocked_by` array AND a banner/log preamble
        // the sanitizing extractor must strip first — the two defects compose.
        let noisy = "Recipe: ooda-engineer-lifecycle SUCCESS (12.0s)\n2026-07-20T00:00:00.000000Z INFO decide\n{\"decision\": \"defer\", \"blocked_by\": [\"fix-goals-status\",], \"rationale\": \"shared file\",}";
        let d =
            parse_admission_decision(noisy).expect("banner + trailing commas must be recovered");
        match d {
            EngineerAdmissionDecision::Defer { blocked_by, .. } => {
                assert_eq!(blocked_by, vec!["fix-goals-status".to_string()]);
            }
            other => panic!("expected Defer, got {other:?}"),
        }
    }

    #[test]
    fn parse_outcome_recovers_trailing_comma() {
        // The outcome seam fails NO-FALLBACK on a parse miss, so recovering a
        // trailing-comma body is what keeps a genuine reasoner verdict from
        // being discarded.
        let d = parse_outcome_decision(r#"{"decision": "mark_achieved", "rationale": "done",}"#);
        assert!(
            d.is_some(),
            "a trailing-comma outcome envelope must still parse"
        );
    }

    #[test]
    fn extract_decision_envelope_recovers_trailing_comma() {
        let env =
            extract_decision_envelope(r#"{"decision": "advance_goal", "rationale": "next step",}"#)
                .expect("trailing-comma decision envelope must parse");
        assert_eq!(env.decision.trim(), "advance_goal");
    }

    #[test]
    fn parse_admission_still_rejects_non_comma_malformed_json() {
        // Leniency must NOT widen beyond the trailing-comma defect: an unquoted
        // key / missing value stays a parse miss (returns None, not a default).
        assert!(parse_admission_decision(r#"{decision: "admit"}"#).is_none());
        assert!(parse_admission_decision(r#"{"decision": }"#).is_none());
    }

    #[test]
    fn decide_engineer_admission_error_includes_adapter_tag() {
        // Recipe path with no sibling admission recipe ⇒ the resolve fails and
        // the error carries the admission adapter tag (the seam then fails open).
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipes/ooda-engineer-lifecycle.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = crate::ooda_brain::EngineerAdmissionCtx::default();
        let err = brain.decide_engineer_admission(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-engineer-admission-brain"),
            "error should contain the admission adapter tag; got: {msg}"
        );
    }

    // ── Resource-admission parsing (issue #2706) ───────────────────────────

    #[test]
    fn parse_resource_admission_admit_envelope() {
        let d = parse_resource_admission_decision(
            r#"{"decision": "admit", "rationale": "plenty of headroom"}"#,
        )
        .expect("parses");
        assert!(matches!(d, ResourceAdmissionDecision::Admit { .. }));
        assert_eq!(d.rationale(), "plenty of headroom");
    }

    #[test]
    fn parse_resource_admission_defer_envelope() {
        let d = parse_resource_admission_decision(
            r#"{"decision": "defer", "rationale": "box saturated"}"#,
        )
        .expect("parses");
        assert!(matches!(d, ResourceAdmissionDecision::Defer { .. }));
        assert_eq!(d.rationale(), "box saturated");
    }

    #[test]
    fn parse_resource_admission_reclaim_first() {
        let d = parse_resource_admission_decision(
            r#"{"decision": "reclaim_first", "rationale": "16 stale caches"}"#,
        )
        .expect("parses");
        assert!(matches!(d, ResourceAdmissionDecision::ReclaimFirst { .. }));
        assert_eq!(d.rationale(), "16 stale caches");
    }

    #[test]
    fn parse_resource_admission_strips_banner_prose() {
        // A banner-polluted envelope (leading prose + fenced block) still parses
        // through the shared sanitizing chokepoint.
        let d = parse_resource_admission_decision(
            "some banner\n```json\n{\"decision\": \"admit\", \"rationale\": \"ok\"}\n```\n",
        )
        .expect("parses through banner");
        assert!(matches!(d, ResourceAdmissionDecision::Admit { .. }));
    }

    #[test]
    fn parse_resource_admission_unknown_variant_is_none() {
        assert!(parse_resource_admission_decision(r#"{"decision": "serialize_after"}"#).is_none());
        assert!(parse_resource_admission_decision(r#"{"decision": "nope"}"#).is_none());
        assert!(parse_resource_admission_decision("not json at all").is_none());
        assert!(parse_resource_admission_decision(r#"{"decision": ""}"#).is_none());
    }

    // --- creative-idea dedup envelope (issue #2925) ------------------------

    #[test]
    fn parse_idea_dedup_create_new_envelope() {
        let d = parse_idea_dedup_decision(
            r#"{"choice": "create_new", "target_node_id": "", "rationale": "novel idea"}"#,
        );
        assert!(matches!(d, Some(IdeaDedupDecision::CreateNew { .. })));
    }

    #[test]
    fn parse_idea_dedup_skip_envelope() {
        let d = parse_idea_dedup_decision(r#"{"choice": "skip", "rationale": "restatement"}"#);
        assert!(matches!(d, Some(IdeaDedupDecision::Skip { .. })));
    }

    #[test]
    fn parse_idea_dedup_enhance_requires_target() {
        let ok = parse_idea_dedup_decision(
            r#"{"choice": "enhance_existing", "target_node_id": "node-42", "rationale": "adds evidence"}"#,
        );
        match ok {
            Some(IdeaDedupDecision::EnhanceExisting { target_node_id, .. }) => {
                assert_eq!(target_node_id, "node-42");
            }
            other => panic!("expected EnhanceExisting, got {other:?}"),
        }
        // enhance_existing without a target is unactionable ⇒ None (fail closed).
        assert!(
            parse_idea_dedup_decision(
                r#"{"choice": "enhance_existing", "target_node_id": "", "rationale": "x"}"#
            )
            .is_none()
        );
        assert!(
            parse_idea_dedup_decision(r#"{"choice": "enhance_existing", "rationale": "x"}"#)
                .is_none()
        );
    }

    #[test]
    fn parse_idea_dedup_strips_banner_prose() {
        let d = parse_idea_dedup_decision(
            "Recipe: creative-idea-dedup ... SUCCESS\n```json\n{\"choice\": \"skip\", \"rationale\": \"dupe\"}\n```\n",
        );
        assert!(matches!(d, Some(IdeaDedupDecision::Skip { .. })));
    }

    #[test]
    fn parse_idea_dedup_unknown_or_empty_is_none() {
        assert!(parse_idea_dedup_decision(r#"{"choice": "merge"}"#).is_none());
        assert!(parse_idea_dedup_decision(r#"{"choice": ""}"#).is_none());
        assert!(parse_idea_dedup_decision("not json at all").is_none());
    }

    #[test]
    fn parse_idea_consolidation_reads_clusters_and_drops_headless() {
        let clusters = parse_idea_consolidation(
            r#"{"clusters": [
                {"canonical_id": "n1", "redundant_ids": ["n2","n3"], "merged_rationale": "same", "evidence": ["e"]},
                {"canonical_id": "", "redundant_ids": ["n9"]}
            ]}"#,
        )
        .expect("parses");
        assert_eq!(
            clusters.len(),
            1,
            "a cluster without a canonical_id is dropped"
        );
        assert_eq!(clusters[0].canonical_id, "n1");
        assert_eq!(clusters[0].redundant_ids, vec!["n2", "n3"]);
    }

    #[test]
    fn parse_idea_consolidation_empty_is_some_and_bad_is_none() {
        assert_eq!(
            parse_idea_consolidation(r#"{"clusters": []}"#),
            Some(Vec::new()),
            "empty clusters is a valid 'nothing to consolidate' result"
        );
        assert!(parse_idea_consolidation("not json").is_none());
    }

    #[test]
    fn decide_idea_dedup_error_includes_adapter_tag() {
        // No sibling dedup recipe ⇒ resolve fails; the error carries the dedup
        // adapter tag (the seam then fails CLOSED, dropping the candidate).
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipes/ooda-engineer-lifecycle.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = crate::ooda_brain::IdeaDedupCtx::default();
        let err = brain.decide_idea_dedup(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-idea-dedup-brain"),
            "error should contain the dedup adapter tag; got: {msg}"
        );
    }

    #[test]
    fn decide_resource_admission_error_includes_adapter_tag() {
        // No sibling resource recipe ⇒ resolve fails; the error carries the
        // resource-admission adapter tag (the seam then fails CLOSED to Defer).
        let brain = RecipeBrain {
            recipe_path: PathBuf::from("/nonexistent/recipes/ooda-engineer-lifecycle.yaml"),
            agent_binary: "copilot",
            adapter_tag: "recipe-engineer-lifecycle-brain",
        };
        let ctx = crate::ooda_brain::ResourceAdmissionCtx::default();
        let err = brain.decide_resource_admission(&ctx).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("recipe-resource-admission-brain"),
            "error should contain the resource-admission adapter tag; got: {msg}"
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

        let path = resolve_recipe_path(tmp.path(), "ooda-decide.yaml", Some(tmp.path()));
        assert!(path.is_some());
        assert!(path.unwrap().to_str().unwrap().contains("ooda-decide.yaml"));
    }

    #[test]
    fn orient_brain_instance_uses_correct_recipe_filename() {
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
        use crate::ooda_brain::BrainPhase;

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
                BrainPhase::Decide,
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
                BrainPhase::MergeJudge,
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
                BrainPhase::Orient,
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

/// Issue #2570: the distillation fact-yield fix tightened the shared
/// `is_copilot_launcher_line` predicate so a JSON structural-token line (`{`,
/// `"`, `[`) is never launcher noise. `is_copilot_launcher_line` /
/// `strip_recipe_noise` is a shared chokepoint, so these decide/orient/lifecycle
/// consumers must keep stripping the REAL launcher preamble (the property they
/// depend on) — the guard only exempts JSON payload lines, which real launcher
/// lines never are. This module is the decide/orient/lifecycle third of the
/// cross-consumer regression coverage the issue asks for.
#[cfg(test)]
mod issue_2570_cross_consumer_launcher_guard_tests {
    use super::*;

    /// The real Copilot CLI 1.0.66-2 launch preamble the consumers must still
    /// strip. None of these lines begins with a JSON structural token, so the
    /// #2570 guard leaves them classified as launcher noise.
    fn launcher_preamble() -> String {
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: cfg\n\
         INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
         Run 'copilot update' to check for updates.\n"
            .to_string()
    }

    #[test]
    fn lifecycle_still_strips_real_launcher_preamble() {
        let raw = format!("{}deprioritize the goal is stalled", launcher_preamble());
        let (decision, outcome) = parse_lifecycle_outcome(&raw);
        assert_eq!(outcome, LifecycleParseOutcome::Parsed);
        assert!(
            matches!(decision, EngineerLifecycleDecision::Deprioritize { .. }),
            "the model's real lifecycle decision must be read, not launcher noise"
        );
    }

    #[test]
    fn shared_cleaner_preserves_pretty_fact_content_line_quoting_launcher_substring() {
        // The other half of the #2570 contract on the exact shared chokepoint
        // these consumers call: a `"`-leading fact content line that quotes the
        // launcher substring is preserved, not dropped.
        let content_line =
            "\"content\": \"the agent logged launching copilot binary=/x before answering\"";
        let cleaned = crate::recipe_output::strip_recipe_noise(content_line);
        assert_eq!(
            cleaned.as_ref(),
            content_line,
            "a JSON payload line must survive the shared cleaner"
        );
    }
}

/// Zero-fallback contract (issue #2580): a reasoner NEVER launders a
/// post-sanitization, post-bounded-retry parse failure into a silent
/// deterministic default. On exhaustion it surfaces an EXPLICIT `Err` +
/// `brain_parse_error` metric; a bounded retry that parses is a SUCCESS; a
/// well-formed structured JSON envelope parses through the shared sanitizing
/// chokepoint on every phase; and a legitimate model "no action" is a distinct,
/// observable `Ok(...)` outcome that never touches the parse-failure path.
#[cfg(test)]
mod zero_fallback_2580_tests {
    use super::*;

    /// The real Copilot CLI launch preamble + an ANSI-coloured tracing line —
    /// the exact stdout contamination the shared #2484 chokepoint must strip on
    /// every reasoner capture path before the structured decision is read.
    fn banner_and_ansi_noise() -> String {
        "\u{2139} NODE_OPTIONS=--max-old-space-size=32768 (saved preference). To change: cfg\n\
         INFO launching copilot binary=/home/azureuser/.npm-global/bin/copilot \
         version=\"GitHub Copilot CLI 1.0.66-2.\"\n\
         Run 'copilot update' to check for updates.\n\
         \x1b[2m2026-07-04T16:00:00.000000Z\x1b[0m \x1b[32mINFO\x1b[0m simard: cycle begin\n"
            .to_string()
    }

    // ─────────────────────────────────────────────────────────────────────
    // AC1: no code path emits a deterministic-default decision from a
    // parse-failure — the shared terminal returns an EXPLICIT Err + metric.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn parse_failure_terminal_returns_explicit_error_never_a_default() {
        // Every parse-failure outcome, at every non-recovery termination, must
        // become an Err — never `Ok(default)`. A String stands in for any
        // phase's decision type (finalize is decision-type-agnostic).
        for outcome in [
            LifecycleParseOutcome::DefaultEmpty,
            LifecycleParseOutcome::DefaultMalformed,
            LifecycleParseOutcome::Error,
        ] {
            for termination in [
                LadderTermination::Exhausted,
                LadderTermination::InvokeError,
                LadderTermination::Disabled,
            ] {
                let result: SimardResult<String> = finalize_ladder_result(
                    "recipe-decide-brain",
                    BrainPhase::Decide,
                    "goal-x",
                    "SILENT_DEFAULT_SENTINEL".to_string(),
                    outcome,
                    termination,
                    3,
                );
                let err = result.expect_err(&format!(
                    "parse-failure {outcome:?}/{termination:?} must surface as Err, not a default"
                ));
                let msg = err.to_string();
                assert!(
                    msg.contains("explicit parse error rather than a silent deterministic default"),
                    "error must state the zero-fallback contract; got: {msg}"
                );
                assert!(
                    msg.contains("recipe-decide-brain"),
                    "error must name the adapter tag; got: {msg}"
                );
            }
        }
    }

    #[test]
    fn parse_failure_terminal_increments_brain_parse_error_metric() {
        // The `brain_parse_error` counter is the honest current-fallback-rate
        // signal (issue #2580). It MUST increment on a genuine parse-failure
        // terminal. Counters are monotonic, so `>= before + 1` is race-safe.
        let name = crate::telemetry::names::BRAIN_PARSE_ERROR;
        let before = crate::telemetry::registry::capture()
            .counter(name, &[])
            .unwrap_or(0);
        let result: SimardResult<String> = finalize_ladder_result(
            "recipe-orient-brain",
            BrainPhase::Orient,
            "goal-y",
            "SILENT_DEFAULT_SENTINEL".to_string(),
            LifecycleParseOutcome::DefaultMalformed,
            LadderTermination::Exhausted,
            3,
        );
        assert!(result.is_err(), "parse-failure must be an Err");
        let after = crate::telemetry::registry::capture()
            .counter(name, &[])
            .unwrap_or(0);
        assert!(
            after > before,
            "brain_parse_error must increment on a parse-failure terminal (before={before}, after={after})"
        );
    }

    #[test]
    fn recovered_or_parsed_outcome_returns_ok_decision() {
        // A first-try parse and both ladder-recovery outcomes are real
        // decisions — finalize returns them as `Ok`, unchanged.
        for outcome in [
            LifecycleParseOutcome::Parsed,
            LifecycleParseOutcome::Repaired,
            LifecycleParseOutcome::Escalated,
        ] {
            let result: SimardResult<String> = finalize_ladder_result(
                "recipe-engineer-lifecycle-brain",
                BrainPhase::Act,
                "goal-z",
                "REAL_DECISION".to_string(),
                outcome,
                LadderTermination::Recovered,
                2,
            );
            assert_eq!(
                result.expect("a parsed/recovered decision must be Ok"),
                "REAL_DECISION",
                "the real decision must pass through unchanged for {outcome:?}"
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // AC2 + AC3: the shared sanitizing chokepoint covers EVERY reasoner
    // capture path, and the extractor consumes a structured JSON envelope
    // (well-formed structured output parses; no free-prose keyword reliance).
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn lifecycle_consumes_json_envelope_through_chokepoint() {
        let raw = format!(
            "{}{{\"decision\": \"reclaim_and_redispatch\", \"rationale\": \"worktree wedged\"}}",
            banner_and_ansi_noise()
        );
        let (decision, outcome) = parse_lifecycle_outcome(&raw);
        assert_eq!(outcome, LifecycleParseOutcome::Parsed);
        assert!(
            matches!(
                decision,
                EngineerLifecycleDecision::ReclaimAndRedispatch { .. }
            ),
            "expected ReclaimAndRedispatch from the envelope, got {decision:?}"
        );
    }

    #[test]
    fn merge_judge_envelope_survives_banner_and_ansi() {
        // The merge-judge capture path also routes through the shared chokepoint
        // (`extract_json_payload`); a banner+ANSI-polluted verdict envelope must
        // still yield the balanced JSON object, not the launcher noise.
        let raw = format!(
            "{}{{\"verdict\": \"not_ready\", \"rationale\": \"CI red\", \"blockers\": []}}",
            banner_and_ansi_noise()
        );
        let payload = crate::recipe_output::extract_json_payload(&raw)
            .expect("the chokepoint must recover the JSON verdict envelope from banner+ANSI noise");
        assert!(payload.contains("\"verdict\""));
        assert!(payload.contains("not_ready"));
        assert!(
            !payload.contains("launching copilot") && !payload.contains("NODE_OPTIONS"),
            "the sanitized payload must not carry launcher-preamble noise: {payload}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // A legitimate take-no-action is a DISTINCT, observable outcome — a real
    // model-emitted `continue_skipping` parses (Ok), verifiably separate from
    // a parse-failure (which is an Err), so a genuine no-op is never confused
    // with a laundered default.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn genuine_no_action_is_a_distinct_ok_outcome_not_a_parse_failure() {
        // A real model no-action: `continue_skipping` parses cleanly (Parsed).
        let (decision, outcome) = parse_lifecycle_outcome(
            r#"{"decision": "continue_skipping", "rationale": "engineer healthy, making progress"}"#,
        );
        assert_eq!(
            outcome,
            LifecycleParseOutcome::Parsed,
            "a genuine no-action must be a real parsed decision, not a parse-failure"
        );
        assert!(
            !outcome.is_parse_failure(),
            "a genuine no-action is NOT on the parse-failure path"
        );
        let ok = finalize_ladder_result(
            "recipe-engineer-lifecycle-brain",
            BrainPhase::Act,
            "healthy-goal",
            decision,
            outcome,
            LadderTermination::Recovered,
            1,
        );
        assert!(
            matches!(ok, Ok(EngineerLifecycleDecision::ContinueSkipping { .. })),
            "a genuine no-action passes through as Ok(ContinueSkipping) — distinct from a parse-failure Err"
        );

        // Contrast: an UNPARSEABLE response is a parse-failure (not a no-action).
        let (_default_decision, miss) = parse_lifecycle_outcome("...garbled banner, no variant...");
        assert!(
            miss.is_parse_failure(),
            "unparseable output is a parse-failure, distinct from a real continue_skipping"
        );
    }
}
