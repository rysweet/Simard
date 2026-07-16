//! MANDATORY ROOT-CAUSE ("WHY") analysis for the Overseer (issue #2635).
//!
//! Whenever the Overseer detects a [`Problem`] it MUST first determine **WHY**
//! the problem occurred — a structured root-cause analysis — before/while
//! choosing an action, rather than blindly patching the symptom. The rejected
//! antipattern is repeatedly `UnblockGoal`-ing a perpetual goal every cycle
//! instead of asking *why it keeps getting blocked* and fixing/escalating that
//! cause.
//!
//! [`analyze`] is the analytic step: it synthesises ranked candidate causes from
//! the problem's evidence signals + observed telemetry and — where available —
//! recall of prior same-signature occurrences from cognitive memory
//! (amplihack-memory-lib, gathered by the caller). It is a **structured
//! deterministic** analyzer (guideline G3: structured reasoning over a brittle
//! single heuristic, no in-loop LLM call so the Overseer's tick stays hermetic
//! and cheap), mirroring the module's "reasoner-with-deterministic-fallback"
//! idiom. It ALWAYS returns a usable WHY — the Overseer never faces a problem
//! with no WHY.

use serde::{Deserialize, Serialize};

use crate::goal_curation::no_progress_breaker::is_no_progress_marker;
use crate::overseer::capabilities::ObservedState;
use crate::overseer::signal::{
    CauseCandidate, CauseSource, Confidence, Likelihood, Problem, ProblemKind, RootCause, Signal,
};

/// At (or above) this many recalled prior occurrences of the SAME root cause,
/// the Overseer stops re-applying the same symptom-level mitigation (e.g.
/// re-`UnblockGoal`-ing a perpetual goal) and escalates the ROOT CAUSE instead —
/// a deduplicated issue describing the systemic defect. This is the guard
/// against the operator's rejected "unblock it every cycle" antipattern.
pub const RECURRENCE_ESCALATION_THRESHOLD: u32 = 3;

/// Dead-band floor for a PERPETUAL, no-progress goal (issue #4124). A perpetual
/// goal that is *self-healed* every cycle (auto-`UnblockGoal`) can re-park on the
/// SAME root cause and re-emit the identical signature indefinitely while its
/// recurrence sits in the `[2, 3)` band — below
/// [`RECURRENCE_ESCALATION_THRESHOLD`] (3), so the general fast path never fires,
/// yet at the detection floor (2), so cognitive memory keeps reporting the same
/// recurring signature. Once a perpetual re-park's cause has recurred at this
/// floor the Overseer ESCALATES its root cause ONCE (operator-visible, naming the
/// missing dependency) instead of blindly re-unblocking it — terminating the loop
/// without collapsing into the rejected "unblock it every cycle" antipattern.
///
/// This is strictly below [`RECURRENCE_ESCALATION_THRESHOLD`]: a first-time or
/// single-recurrence false park (`recurrence < 2`) is still self-healed.
pub const PERPETUAL_RECURRENCE_ESCALATION_THRESHOLD: u32 = 2;

/// A prior occurrence of a problem's root cause, recalled from cognitive memory.
/// The Overseer records one of these each time it acts on a cause (via
/// amplihack-memory-lib); recall of them raises [`RootCause::recurrence`] so a
/// one-off false-park becomes a detected recurring root cause.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorOccurrence {
    /// The primary cause label recorded at that occurrence.
    pub cause_label: String,
    /// The action the Overseer took then (e.g. `"unblock_goal"`).
    pub action: String,
    /// The recorded outcome (e.g. `"re-blocked next cycle"`).
    pub outcome: String,
}

/// The deduplication signature for a root cause: the problem's dedup key joined
/// with the primary cause label. A filed escalation keyed on this describes the
/// ROOT CAUSE and deduplicates across symptom recurrences (so the systemic
/// defect is filed once, not once per re-occurrence of the symptom).
pub fn root_cause_signature(problem: &Problem, primary: &CauseCandidate) -> String {
    format!("{}::{}", problem.dedup_key, primary.label)
}

/// Analyse WHY a `problem` occurred: synthesise ranked candidate causes from the
/// evidence signals + observed telemetry, fold in `recall` of prior
/// same-signature occurrences, and return a structured, human-readable
/// [`RootCause`]. ALWAYS returns ≥1 candidate and a non-empty rationale.
///
/// Pure and deterministic (G3): no I/O, no LLM. The caller gathers `recall` from
/// cognitive memory (best-effort; an empty slice is the graceful-degrade shape
/// when memory is unavailable → a telemetry-only WHY with zero recurrence).
pub fn analyze(
    problem: &Problem,
    observed: &ObservedState,
    recall: &[PriorOccurrence],
) -> RootCause {
    let mut candidates = candidates_for(problem, observed);
    if candidates.is_empty() {
        candidates.push(unknown_candidate());
    }

    // The primary is the strongest candidate (built strongest-first).
    let primary_label = candidates[0].label.clone();

    // Recurrence = prior recalled occurrences of the SAME primary cause.
    let recurrence = recall
        .iter()
        .filter(|o| o.cause_label == primary_label)
        .count() as u32;

    // Source honestly reflects where the evidence came from: telemetry-only when
    // memory is unavailable or nothing prior matched; telemetry + memory recall
    // when a prior same-cause occurrence corroborated it.
    let source = if recall.is_empty() || recurrence == 0 {
        CauseSource::Telemetry
    } else {
        CauseSource::Both
    };

    // Promote a recall-corroborated primary and cite the recall as evidence.
    if recurrence > 0 {
        candidates[0].likelihood = Likelihood::High;
        let last = recall.iter().rev().find(|o| o.cause_label == primary_label);
        let tail = last
            .map(|o| format!(" (last: action={}, outcome={})", o.action, o.outcome))
            .unwrap_or_default();
        candidates[0].evidence.push(format!(
            "memory recall: {recurrence} prior occurrence(s) of this cause{tail}"
        ));
    }

    let confidence = confidence_for(&candidates[0], recurrence);
    let primary_rationale = rationale_for(problem, &candidates[0], recurrence);

    RootCause {
        candidates,
        primary_rationale,
        confidence,
        source,
        recurrence,
    }
}

// ─────────────────────────── candidate synthesis ───────────────────────────

fn candidates_for(problem: &Problem, observed: &ObservedState) -> Vec<CauseCandidate> {
    match problem.kind {
        ProblemKind::GoalHygiene => goal_hygiene_candidates(problem),
        ProblemKind::ProcessHealth => process_health_candidates(problem, observed),
        ProblemKind::ResourcePressure => resource_pressure_candidates(problem, observed),
        ProblemKind::QualityRegression => quality_regression_candidates(problem),
        ProblemKind::DeliveryReady => delivery_ready_candidates(),
        ProblemKind::LoopDetected => loop_candidates(problem),
        ProblemKind::DriftCorrection => drift_candidates(problem),
        ProblemKind::CrossCutting => cross_cutting_candidates(),
        ProblemKind::WorkstreamCoverage => workstream_coverage_candidates(problem),
        ProblemKind::StepFailure => step_failure_candidates(problem),
    }
}

fn goal_hygiene_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    for s in &problem.evidence {
        match s {
            Signal::GoalBlocked {
                reason,
                perpetual,
                needs_review,
                consecutive_no_action,
                ..
            } => {
                return blocked_goal_candidates(
                    reason,
                    *perpetual,
                    *needs_review,
                    *consecutive_no_action,
                );
            }
            Signal::StaleGoal { goal_id } => {
                return vec![cand(
                    "goal-re-litigated-or-stale",
                    Likelihood::Medium,
                    [format!("goal {goal_id} re-litigated / stale-complete")],
                )];
            }
            _ => {}
        }
    }
    vec![cand(
        "goal-hygiene-drift",
        Likelihood::Medium,
        ["a goal-board hygiene problem with no specific blocked-goal marker"],
    )]
}

fn blocked_goal_candidates(
    reason: &str,
    perpetual: bool,
    needs_review: bool,
    cycles: u32,
) -> Vec<CauseCandidate> {
    let no_progress = is_no_progress_marker(reason);
    if perpetual && no_progress {
        // The canonical false park: a standing/perpetual goal hard-parked by the
        // no-progress safeguard. Secondary hypothesis: starvation by higher-
        // priority work keeping it from making progress in the first place.
        vec![
            cand(
                "parked-by-no-progress-safeguard",
                Likelihood::High,
                [
                    format!("blocked_goal.reason: {reason}"),
                    format!(
                        "perpetual=true, needs_review={needs_review}, no-action cycles={cycles}"
                    ),
                ],
            ),
            cand(
                "higher-priority-work-starvation",
                Likelihood::Low,
                ["a standing goal can be starved of progress by higher-priority in-flight work"],
            ),
        ]
    } else if no_progress {
        vec![cand(
            "standing-goal-not-tagged-perpetual",
            Likelihood::High,
            [
                format!("blocked_goal.reason: {reason}"),
                "perpetual=false but hard-parked by the no-progress safeguard — likely a standing \
                 goal missing its perpetual tag"
                    .to_string(),
            ],
        )]
    } else if needs_review {
        vec![cand(
            "brain-failure-or-safeguard-regression",
            Likelihood::High,
            [
                format!("blocked_goal.reason: {reason}"),
                "carries a needs-human-review safeguard marker — the reasoner/brain repeatedly \
                 failed to advance the goal"
                    .to_string(),
            ],
        )]
    } else {
        vec![cand(
            "deliberate-operator-or-dependency-block",
            Likelihood::High,
            [
                format!("blocked_goal.reason: {reason}"),
                "no safeguard marker — an intentional operator action or external dependency block"
                    .to_string(),
            ],
        )]
    }
}

fn process_health_candidates(problem: &Problem, observed: &ObservedState) -> Vec<CauseCandidate> {
    for s in &problem.evidence {
        match s {
            Signal::DistillFailureRate { pct } => {
                let ctx = observed
                    .distill_fail_pct
                    .map(|p| format!("distill_fail_pct={p:.0}%"))
                    .unwrap_or_else(|| format!("distill parse-failure rate {pct:.0}%"));
                return vec![
                    cand(
                        "distillation-schema-or-format-drift",
                        Likelihood::High,
                        [
                            ctx.clone(),
                            "distiller output no longer matches the expected schema/format"
                                .to_string(),
                        ],
                    ),
                    cand(
                        "model-output-regression",
                        Likelihood::Medium,
                        [
                            "a model/prompt change degraded structured-output adherence"
                                .to_string(),
                        ],
                    ),
                    cand(
                        "upstream-source-change",
                        Likelihood::Low,
                        ["an upstream source shape changed, breaking distillation".to_string()],
                    ),
                ];
            }
            Signal::RestartChurn { restarts } => {
                return vec![cand(
                    "daemon-crash-loop-or-oom",
                    Likelihood::High,
                    [
                        format!("restart_churn={restarts}"),
                        "the daemon is self-relaunching repeatedly — a crash loop or OOM"
                            .to_string(),
                    ],
                )];
            }
            Signal::LadderExhausted { count } => {
                return vec![cand(
                    "reasoner-decide-ladder-exhaustion",
                    Likelihood::High,
                    [format!("ladder_exhausted={count}"), "the reasoner exhausted its decide ladder — provider/timeout/degraded reasoning".to_string()],
                )];
            }
            Signal::Anomaly { detail } => {
                return vec![cand(
                    "telemetry-anomaly",
                    Likelihood::Medium,
                    [format!("anomaly: {detail}")],
                )];
            }
            _ => {}
        }
    }
    vec![cand(
        "process-health-degradation",
        Likelihood::Medium,
        ["a process-health degradation with no single dominant signal"],
    )]
}

fn resource_pressure_candidates(
    problem: &Problem,
    observed: &ObservedState,
) -> Vec<CauseCandidate> {
    for s in &problem.evidence {
        match s {
            Signal::BudgetPressure {
                spent_usd,
                budget_usd,
            } => {
                let _ = observed;
                return vec![
                    cand(
                        "spend-spike-or-runaway-retries",
                        Likelihood::High,
                        [
                            format!("spent ${spent_usd:.2} of ${budget_usd:.2} daily budget"),
                            "daily LLM spend is climbing — likely a spend spike or a runaway retry loop".to_string(),
                        ],
                    ),
                    cand(
                        "misconfigured-daily-budget",
                        Likelihood::Medium,
                        ["the daily budget may be set too low for the current workload".to_string()],
                    ),
                ];
            }
            Signal::EngineerSpawnRate { live } => {
                return vec![cand(
                    "engineer-spawn-storm",
                    Likelihood::High,
                    [
                        format!("live_engineers={live}"),
                        "elevated engineer spawn — a fan-out storm or stuck workstreams"
                            .to_string(),
                    ],
                )];
            }
            Signal::MemoryGrowth { nodes_total } => {
                return vec![cand(
                    "unbounded-memory-growth",
                    Likelihood::High,
                    [format!("memory_nodes={nodes_total}"), "cognitive-memory growth beyond expectation — consolidation/forgetting is lagging".to_string()],
                )];
            }
            _ => {}
        }
    }
    vec![cand(
        "resource-pressure",
        Likelihood::Medium,
        ["resource pressure with no single dominant signal"],
    )]
}

fn quality_regression_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    for s in &problem.evidence {
        match s {
            Signal::CiFailureCluster { repo, failing } => {
                return vec![cand(
                    "broken-or-flaky-tests-or-regression",
                    Likelihood::High,
                    [
                        format!("{failing} failing checks in {repo}"),
                        "a cluster of CI failures — a real regression or flaky/broken tests"
                            .to_string(),
                    ],
                )];
            }
            Signal::GymSkipped => {
                return vec![cand(
                    "gym-self-eval-skipped",
                    Likelihood::High,
                    [
                        "the gym self-eval was skipped — quality signal is going unmeasured"
                            .to_string(),
                    ],
                )];
            }
            _ => {}
        }
    }
    vec![cand(
        "quality-regression",
        Likelihood::Medium,
        ["a quality regression with no single dominant signal"],
    )]
}

fn delivery_ready_candidates() -> Vec<CauseCandidate> {
    vec![cand(
        "pr-green-awaiting-merge-decision",
        Likelihood::High,
        ["a green, merge-ready PR is awaiting a merge decision"],
    )]
}

fn loop_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    let cycles = problem.evidence.iter().find_map(|s| match s {
        Signal::LoopDetected {
            consecutive_no_action,
            ..
        } => Some(*consecutive_no_action),
        _ => None,
    });
    vec![cand(
        "goal-loop-no-progress",
        Likelihood::High,
        [format!(
            "the active goal is looping with no progress ({} cycle(s))",
            cycles
                .map(|n| n.to_string())
                .unwrap_or_else(|| "several".to_string())
        )],
    )]
}

fn drift_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    let detail = problem.evidence.iter().find_map(|s| match s {
        Signal::DriftCorrection { detail, .. } => Some(detail.clone()),
        _ => None,
    });
    vec![cand(
        "work-drifting-from-goal-intent",
        Likelihood::High,
        [detail.unwrap_or_else(|| {
            "active work is drifting from the goal's stated intent".to_string()
        })],
    )]
}

fn cross_cutting_candidates() -> Vec<CauseCandidate> {
    vec![cand(
        "cross-cutting-initiative",
        Likelihood::Medium,
        ["a cross-cutting / mechanical sweep initiative"],
    )]
}

fn workstream_coverage_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    let n = problem
        .evidence
        .iter()
        .filter_map(|s| match s {
            Signal::WorkstreamGap { gaps } => Some(gaps.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    vec![cand(
        "important-work-with-no-active-workstream",
        Likelihood::High,
        [format!(
            "{n} high-value item(s) (goal/issue/anomaly) have no active workstream, PR, or fix in \
             flight — a backlog-coverage gap"
        )],
    )]
}

/// A diagnosed step failure (#2640) already carries its structured root cause in
/// the evidence signal — the classifier answered WHY at the catch site — so the
/// candidate is that cause verbatim, at high likelihood.
fn step_failure_candidates(problem: &Problem) -> Vec<CauseCandidate> {
    for s in &problem.evidence {
        if let Signal::StepFailureDiagnosed {
            cause,
            exit_code,
            evidence,
        } = s
        {
            let code = exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            return vec![cand(
                cause.as_str(),
                Likelihood::High,
                [format!(
                    "a decision-cycle / engineer / terminal-shell step failed with diagnosed root \
                     cause {} (exit {code}): {evidence}",
                    cause.as_str()
                )],
            )];
        }
    }
    vec![cand(
        "step-failure-undiagnosed",
        Likelihood::Medium,
        ["a step failed but no structured diagnosis was attached to the problem"],
    )]
}

fn unknown_candidate() -> CauseCandidate {
    cand(
        "unknown-cause",
        Likelihood::Low,
        ["no distinguishing telemetry or recall available"],
    )
}

// ─────────────────────────── rationale + confidence ────────────────────────

fn rationale_for(problem: &Problem, primary: &CauseCandidate, recurrence: u32) -> String {
    let base = match primary.label.as_str() {
        "parked-by-no-progress-safeguard" => {
            "perpetual goal parked by the no-progress safeguard (false park)".to_string()
        }
        "standing-goal-not-tagged-perpetual" => {
            "a standing goal is hard-parked by the no-progress safeguard because it is not tagged \
             perpetual"
                .to_string()
        }
        "brain-failure-or-safeguard-regression" => {
            "the goal repeatedly failed to advance and tripped a needs-human-review safeguard"
                .to_string()
        }
        "deliberate-operator-or-dependency-block" => {
            "the goal is intentionally blocked on an operator action or external dependency"
                .to_string()
        }
        "distillation-schema-or-format-drift" => {
            "distiller output has drifted from the expected schema/format, so parsing fails"
                .to_string()
        }
        "daemon-crash-loop-or-oom" => {
            "the daemon is self-relaunching repeatedly — a crash loop or out-of-memory".to_string()
        }
        "reasoner-decide-ladder-exhaustion" => {
            "the reasoner exhausted its decide ladder — degraded reasoning or provider trouble"
                .to_string()
        }
        "spend-spike-or-runaway-retries" => {
            "daily LLM spend is approaching/over budget — likely a spend spike or runaway retries"
                .to_string()
        }
        "engineer-spawn-storm" => {
            "engineer spawn is elevated — a fan-out storm or stuck workstreams".to_string()
        }
        "unbounded-memory-growth" => {
            "cognitive-memory is growing beyond expectation — consolidation/forgetting is lagging"
                .to_string()
        }
        "broken-or-flaky-tests-or-regression" => {
            "a cluster of CI failures — a real regression or flaky/broken tests".to_string()
        }
        "gym-self-eval-skipped" => {
            "the gym self-eval was skipped, so the quality signal is going unmeasured".to_string()
        }
        "pr-green-awaiting-merge-decision" => {
            "a green, merge-ready PR is awaiting a merge decision".to_string()
        }
        "goal-loop-no-progress" => "the active goal is looping without making progress".to_string(),
        "work-drifting-from-goal-intent" => {
            "active work is drifting from the goal's stated intent".to_string()
        }
        "important-work-with-no-active-workstream" => {
            "important backlog work has no active workstream, PR, or fix in flight".to_string()
        }
        _ => format!("{} — {}", problem.summary, primary.label.replace('-', " ")),
    };
    if recurrence > 0 {
        format!("{base} — RECURRING: this cause has been seen {recurrence} time(s) before")
    } else {
        base
    }
}

fn confidence_for(primary: &CauseCandidate, recurrence: u32) -> Confidence {
    if recurrence > 0 {
        Confidence::High
    } else {
        match primary.likelihood {
            Likelihood::High => Confidence::High,
            Likelihood::Medium => Confidence::Medium,
            Likelihood::Low => Confidence::Low,
        }
    }
}

/// Small builder: a `CauseCandidate` from a label, likelihood, and evidence
/// lines (accepting any iterable of `Into<String>`).
fn cand<I, S>(label: &str, likelihood: Likelihood, evidence: I) -> CauseCandidate
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    CauseCandidate {
        label: label.to_string(),
        likelihood,
        evidence: evidence.into_iter().map(Into::into).collect(),
    }
}
