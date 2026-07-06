//! The Overseer's `Signal` and `Problem` vocabulary — the Observe/Orient data
//! model. `Signal`s are cheap, additive indicators derived from one Observe pass;
//! Orient folds a set of `Signal`s into ranked, deduplicated `Problem`s.

use crate::overseer::capabilities::ObservedState;

/// A raw, low-level indicator derived from one Observe pass (StatusSnapshot +
/// logs + PR/CI/goal state). Non-authoritative on its own; Orient turns a set of
/// Signals into ranked `Problem`s. Each variant cites the durable field it comes
/// from (see `ObservedState`).
#[derive(Clone, Debug, PartialEq)]
pub enum Signal {
    /// Distillation parse-failure rate exceeds threshold (`distill_fail_pct`).
    DistillFailureRate { pct: f64 },
    /// Daemon self-relaunch/restart churn over the window (`restart_churn`).
    RestartChurn { restarts: u64 },
    /// Reasoner/brain decide-ladder exhaustion (`ladder_exhausted`).
    LadderExhausted { count: u64 },
    /// Daily LLM spend approaching/over budget (`spent_today_usd`/`daily_budget_usd`).
    BudgetPressure { spent_usd: f64, budget_usd: f64 },
    /// Engineer spawn/live count elevated (`live_engineers`).
    EngineerSpawnRate { live: u32 },
    /// Cognitive-memory growth beyond expectation (`memory_nodes`).
    MemoryGrowth { nodes_total: u64 },
    /// Gym self-eval skipped (`gym_skipped`).
    GymSkipped,
    /// A cluster of CI failures across recent runs (`ci_failures`).
    CiFailureCluster { repo: String, failing: u32 },
    /// A PR is green + merge-ready and awaiting a merge decision (`ready_prs`).
    PrReadyToMerge { repo: String, pr: u32 },
    /// A goal has been re-litigated / "stale-complete" repeatedly.
    StaleGoal { goal_id: String },
    /// A free-form anomaly surfaced by `TelemetrySignals.anomalies[]`.
    Anomaly { detail: String },
    /// A live goal has gone `consecutive_no_action` cycles without progress —
    /// the primary lightweight-whisper trigger. Fires strictly BELOW Simard's
    /// no-progress breaker so the Overseer can nudge before the hard breaker
    /// trips. From `ObservedState.{consecutive_no_action, active_goal_id}`.
    LoopDetected {
        goal_id: String,
        consecutive_no_action: u32,
    },
    /// Active work appears to be drifting from a goal's stated intent. From
    /// `ObservedState.{drift_detail, active_goal_id}`.
    DriftCorrection { goal_id: String, detail: String },
    /// A goal is `Blocked` on Simard's goal board — the goal-board *health*
    /// signal. From `ObservedState.blocked_goals`. `perpetual` reuses the
    /// standing/perpetual detection (#2589/#2609); `needs_review` is true when
    /// the block carries a "needs human review" safeguard marker. Routed to
    /// [`ProblemKind::GoalHygiene`]: a false-parked perpetual goal is
    /// self-healed, a genuine "needs human review" block is escalated.
    GoalBlocked {
        goal_id: String,
        reason: String,
        perpetual: bool,
        needs_review: bool,
        consecutive_no_action: u32,
    },
    /// ≥2 recalled episodes share a failure signature: this problem has happened
    /// before (issue #2628). Promotes problem detection from in-process counters
    /// to the cognitive-memory graph — raising priority and surfacing the prior
    /// procedure. Derived from [`ObservedState::recall`]'s episodes, keyed on
    /// their parsed `failure_signature`. Additive: it never removes or replaces
    /// any pre-recall signal.
    RecurringSignature { signature: String, occurrences: u32 },
    /// The recurring backlog-coverage gap-scan found important work that SHOULD
    /// have an active workstream but does not — the "WHAT WORKSTREAMS ARE WE
    /// MISSING?" question the Overseer asks each (or every Nth) tick. ONE
    /// consolidated signal carries EVERY genuine gap the Observe pass surfaced
    /// into [`ObservedState::workstream_gaps`] (unlike [`Signal::GoalBlocked`],
    /// which is one signal per goal). Each [`GapItem`] says what is uncovered and
    /// why it matters. Orient folds this into a single
    /// [`ProblemKind::WorkstreamCoverage`] problem.
    WorkstreamGap { gaps: Vec<GapItem> },
}

/// Which backlog source a [`GapItem`] came from — so the renderer can label the
/// gap's provenance from structured data rather than parsing the summary (G3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapCategory {
    /// A high-priority goal on Simard's board with no active engineer / PR.
    GoalUncovered,
    /// A high-signal open GitHub issue with no open PR and no active workstream.
    IssueUncovered,
    /// A live telemetry anomaly with no fix in flight.
    AnomalyUnaddressed,
}

impl GapCategory {
    /// A short, stable provenance label used in the rendered notification / log.
    pub fn label(&self) -> &'static str {
        match self {
            Self::GoalUncovered => "goal",
            Self::IssueUncovered => "issue",
            Self::AnomalyUnaddressed => "anomaly",
        }
    }
}

/// One genuine backlog-coverage gap: a specific piece of important work that is
/// uncovered (no active workstream, no open PR, no fix in flight), carrying the
/// structured specifics the Act path renders verbatim. All string fields are
/// bounded at the detector (`sensor::MAX_GAP_FIELD_LEN`) and the `signature` is a
/// restricted slug built from trusted identifiers only (never hostile free text),
/// so a gap can never inflate a notification, an issue body, or a log line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GapItem {
    /// The backlog source this gap came from.
    pub category: GapCategory,
    /// The specific, human-readable reference (goal id, `repo#number`, or the
    /// anomaly detail) — what is uncovered.
    pub ref_id: String,
    /// A short human-readable title for the uncovered work.
    pub title: String,
    /// Why this uncovered work matters (the reason it deserves a workstream).
    pub why_it_matters: String,
    /// Stable per-gap dedup signature (`goal:<id>` / `issue:<repo>#<n>` /
    /// `anomaly:<slug>`). Identical inputs yield identical signatures so a
    /// recurring gap is deduped to at most one notification + issue per signature.
    pub signature: String,
}

/// Coarse relative importance. `Ord` sorts ascending so `Critical` comes first,
/// mirroring `crate::cognitive_threads::Priority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Critical,
    High,
    Normal,
    Low,
}

/// Problem family used by Decide to pick an intervention shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// Parse failures, restart churn, ladder exhaustion.
    ProcessHealth,
    /// Budget pressure, engineer-spawn spikes, memory growth.
    ResourcePressure,
    /// A PR ready to merge, or a conflict to resolve.
    DeliveryReady,
    /// CI-failure clusters, gym skipped.
    QualityRegression,
    /// Stale/re-litigated goals.
    GoalHygiene,
    /// Naming/architecture sweeps, terminology cleanups, cross-repo initiatives.
    CrossCutting,
    /// A live goal looping without progress — steered by a lightweight whisper.
    LoopDetected,
    /// Active work drifting from a goal's intent — nudged by an advisory whisper.
    DriftCorrection,
    /// Important backlog work is uncovered — a high-priority goal, high-signal
    /// issue, or live anomaly with no active workstream. The recurring gap-scan's
    /// problem family; driven by the deduped notify + file-issue act path.
    WorkstreamCoverage,
}

/// A classified, deduplicated, prioritised problem — the output of Orient and the
/// input to Decide. Carries the evidence `Signal`s plus a `dedup_key` used to
/// avoid fighting Simard's in-flight work and to avoid duplicate interventions.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub priority: Priority,
    /// Stable dedup key. An adapter should mirror
    /// `crate::stewardship::failure_signature` semantics so the same problem does
    /// not spawn a duplicate workstream or a duplicate issue.
    pub dedup_key: String,
    pub summary: String,
    pub evidence: Vec<Signal>,
}

// Thresholds are illustrative defaults for the sketch; real values would be
// `SIMARD_OVERSEER_*` env knobs clamped to floors (see the design doc).
const DISTILL_FAIL_PCT_THRESHOLD: f64 = 20.0;
const RESTART_CHURN_THRESHOLD: u64 = 3;
const BUDGET_PRESSURE_FRACTION: f64 = 0.8;
const ENGINEER_SPAWN_THRESHOLD: u32 = 8;

/// Consecutive no-action cycles at (or above) which the Overseer whispers a
/// loop-correction. Deliberately BELOW
/// [`crate::goal_curation::no_progress_breaker::NO_PROGRESS_BREAKER_THRESHOLD`]
/// so the lightweight whisper nudges Simard before the hard breaker escalates.
pub const WHISPER_LOOP_THRESHOLD: u32 = 2;

/// Minimum number of recalled episodes that must share a `failure_signature`
/// before the Overseer raises a [`Signal::RecurringSignature`] (issue #2628). A
/// single prior occurrence is not "recurring"; two or more is the floor.
pub const RECURRING_SIGNATURE_THRESHOLD: u32 = 2;

/// Pure Observe→Signal derivation. No I/O; unit-testable with a hand-built
/// `ObservedState`. Real thresholds would be env-tunable.
pub fn signals_from(state: &ObservedState) -> Vec<Signal> {
    let mut out = Vec::new();

    if let Some(pct) = state.distill_fail_pct
        && pct >= DISTILL_FAIL_PCT_THRESHOLD
    {
        out.push(Signal::DistillFailureRate { pct });
    }
    if let Some(restarts) = state.restart_churn
        && restarts >= RESTART_CHURN_THRESHOLD
    {
        out.push(Signal::RestartChurn { restarts });
    }
    if let Some(count) = state.ladder_exhausted
        && count > 0
    {
        out.push(Signal::LadderExhausted { count });
    }
    if let (Some(spent), Some(budget)) = (state.spent_today_usd, state.daily_budget_usd)
        && budget > 0.0
        && spent >= budget * BUDGET_PRESSURE_FRACTION
    {
        out.push(Signal::BudgetPressure {
            spent_usd: spent,
            budget_usd: budget,
        });
    }
    if let Some(live) = state.live_engineers
        && live >= ENGINEER_SPAWN_THRESHOLD
    {
        out.push(Signal::EngineerSpawnRate { live });
    }
    if state.gym_skipped {
        out.push(Signal::GymSkipped);
    }
    for cf in &state.ci_failures {
        out.push(Signal::CiFailureCluster {
            repo: cf.repo.clone(),
            failing: cf.failing,
        });
    }
    for pr in &state.ready_prs {
        out.push(Signal::PrReadyToMerge {
            repo: pr.repo.clone(),
            pr: pr.pr,
        });
    }
    for detail in &state.anomalies {
        out.push(Signal::Anomaly {
            detail: detail.clone(),
        });
    }

    // A live goal looping without progress: whisper trigger. Requires an active
    // goal (idle churn with no goal is not a goal loop to steer).
    if let (Some(n), Some(goal_id)) = (state.consecutive_no_action, state.active_goal_id.as_ref())
        && n >= WHISPER_LOOP_THRESHOLD
    {
        out.push(Signal::LoopDetected {
            goal_id: goal_id.clone(),
            consecutive_no_action: n,
        });
    }
    // Active work drifting from a goal's intent: advisory whisper trigger.
    if let (Some(detail), Some(goal_id)) =
        (state.drift_detail.as_ref(), state.active_goal_id.as_ref())
    {
        out.push(Signal::DriftCorrection {
            goal_id: goal_id.clone(),
            detail: detail.clone(),
        });
    }

    // Goal-board health: one signal per blocked goal observed on the board.
    for bg in &state.blocked_goals {
        out.push(Signal::GoalBlocked {
            goal_id: bg.id.clone(),
            reason: bg.reason.clone(),
            perpetual: bg.perpetual,
            needs_review: bg.needs_review,
            consecutive_no_action: bg.consecutive_no_action,
        });
    }

    // Cognitive-memory recall (#2628): when ≥2 recalled episodes share a failure
    // signature, this problem has recurred — raise a structural signal so Orient
    // can promote it (and surface the prior procedure) rather than relying only
    // on in-process counters. Additive: only appends; a `None`/empty recall or a
    // recall error leaves the signal set exactly as it was before recall.
    if let Some(snapshot) = &state.recall {
        let mut counts: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
        for ep in &snapshot.episodes {
            if let Some(sig) = &ep.failure_signature {
                *counts.entry(sig.as_str()).or_insert(0) += 1;
            }
        }
        for (signature, occurrences) in counts {
            if occurrences >= RECURRING_SIGNATURE_THRESHOLD {
                out.push(Signal::RecurringSignature {
                    signature: signature.to_string(),
                    occurrences,
                });
            }
        }
    }

    // Backlog-coverage gaps: ONE consolidated signal carrying every genuine gap
    // the Observe pass surfaced (never one-per-gap, so Act notifies once with the
    // full list). A clean picture emits no signal — no gap, no noise.
    if !state.workstream_gaps.is_empty() {
        out.push(Signal::WorkstreamGap {
            gaps: state.workstream_gaps.clone(),
        });
    }

    out
}

// ─────────────────────── informative detail rendering (issue #21) ───────────
//
// The Overseer activity log must say WHAT was observed and WHAT was done with
// CONCRETE values, not bare counts ("saw 3 problems"). These primitives turn the
// typed `Signal`/`Problem`/action vocabulary into short, human-readable strings
// (guideline G3: structured data → templated rendering, never string parsing).
// Every rendered string is persisted to the durable feed and shown to operators,
// so `sanitize_detail` neutralises terminal-control bytes and token-shaped
// secrets and bounds each line before it ever reaches disk or a screen.

/// Per-line character cap for one rendered detail string. Long capability
/// output (error bodies, goal reasons) is truncated with an ellipsis so a single
/// pathological line can never blow up the feed, the TUI wrap, or the SPA row.
pub(crate) const DETAIL_STR_CAP: usize = 512;

/// Maximum number of detail lines retained per list (observed / actions) on one
/// tick. Overflow is summarised with a `(+N more)` sentinel so the feed stays
/// bounded and deterministic (the render surfaces cap again for their viewport).
pub(crate) const DETAIL_CAP: usize = 24;

/// Make one free-form string safe to persist and render as an Overseer detail:
/// strip ANSI escapes, collapse control/whitespace runs to single spaces, redact
/// token-shaped secrets, and bound the length. Idempotent for already-clean
/// input.
///
/// Reuse: ANSI stripping delegates to the single shared hardened stripper
/// [`crate::recipe_output::strip_ansi`] (issue #2484) rather than a private copy.
pub(crate) fn sanitize_detail(s: &str) -> String {
    // 1. Strip ANSI/CSI/OSC escape sequences (shared, hardened implementation).
    let stripped = crate::recipe_output::strip_ansi(s);
    // 2. Map every control byte (newline, tab, ESC remnants, …) to a space so a
    //    detail is always a single clean line, then tokenise + redact secrets.
    let spaced: String = stripped
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let redacted = spaced
        .split_whitespace()
        .map(redact_secret_token)
        .collect::<Vec<_>>()
        .join(" ");
    // 3. Bound the line length with a visible truncation marker.
    if redacted.chars().count() > DETAIL_STR_CAP {
        let head: String = redacted.chars().take(DETAIL_STR_CAP).collect();
        format!("{head}…")
    } else {
        redacted
    }
}

/// Placeholder written in place of a redacted secret.
const REDACTED: &str = "<redacted-secret>";

/// Redact a single whitespace-delimited token if it is shaped like a secret:
/// a GitHub token (`ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_`/`github_pat_` prefix) or a
/// long, high-entropy alphanumeric blob (a bearer token / opaque credential).
/// Ordinary words, repo slugs (`owner/name#42`), ids (`g-9`, `ws-77`), and URLs
/// (whose alnum runs are broken by `/` and `.`) are left untouched.
fn redact_secret_token(word: &str) -> String {
    const GH_PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    for p in GH_PREFIXES {
        if let Some(rest) = word.strip_prefix(p)
            && rest.len() >= 8
            && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return REDACTED.to_string();
        }
    }
    // Generic high-entropy blob: a long contiguous alphanumeric run carrying
    // BOTH letters and digits (never a plain word, path, or number).
    if word.len() >= 32
        && word.chars().all(|c| c.is_ascii_alphanumeric())
        && word.chars().any(|c| c.is_ascii_digit())
        && word.chars().any(|c| c.is_ascii_alphabetic())
    {
        return REDACTED.to_string();
    }
    word.to_string()
}

impl Signal {
    /// Render one observed `Signal` as a short, human-readable evidence line that
    /// carries the SPECIFIC observed value(s) — the operator-facing answer to
    /// "what did the Overseer actually see?". Routed through [`sanitize_detail`]
    /// because free-form fields (anomaly text, goal reasons) are attacker-
    /// influenceable and end up persisted and rendered.
    pub fn describe(&self) -> String {
        let raw = match self {
            Signal::DistillFailureRate { pct } => {
                format!("distillation parse-failure rate {pct:.0}%")
            }
            Signal::RestartChurn { restarts } => {
                format!("daemon restart churn: {restarts} restarts in window")
            }
            Signal::LadderExhausted { count } => {
                format!("reasoner decide-ladder exhausted ×{count}")
            }
            Signal::BudgetPressure {
                spent_usd,
                budget_usd,
            } => format!("LLM budget pressure: ${spent_usd:.2} of ${budget_usd:.2}"),
            Signal::EngineerSpawnRate { live } => {
                format!("elevated engineer spawn: {live} live")
            }
            Signal::MemoryGrowth { nodes_total } => {
                format!("cognitive-memory growth: {nodes_total} nodes")
            }
            Signal::GymSkipped => "gym self-eval skipped".to_string(),
            Signal::CiFailureCluster { repo, failing } => {
                format!("CI-failure cluster in {repo}: {failing} failing")
            }
            Signal::PrReadyToMerge { repo, pr } => {
                format!("PR {repo}#{pr} green and merge-ready")
            }
            Signal::StaleGoal { goal_id } => {
                format!("goal {goal_id} re-litigated / stale-complete")
            }
            Signal::Anomaly { detail } => format!("telemetry anomaly: {detail}"),
            Signal::LoopDetected {
                goal_id,
                consecutive_no_action,
            } => format!("goal {goal_id} looping — no progress for {consecutive_no_action} cycles"),
            Signal::DriftCorrection { goal_id, detail } => {
                format!("goal {goal_id} drifting from intent: {detail}")
            }
            Signal::GoalBlocked {
                goal_id,
                reason,
                perpetual,
                needs_review,
                consecutive_no_action,
            } => {
                let mut tags: Vec<&str> = Vec::new();
                if *perpetual {
                    tags.push("perpetual");
                }
                if *needs_review {
                    tags.push("needs human review");
                }
                let tag = if tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", tags.join(", "))
                };
                format!(
                    "blocked goal {goal_id}: {reason}{tag} ({consecutive_no_action} no-action cycle(s))"
                )
            }
            Signal::RecurringSignature {
                signature,
                occurrences,
            } => format!("recurring failure signature '{signature}' seen {occurrences} time(s)"),
            Signal::WorkstreamGap { gaps } => {
                let refs: Vec<&str> = gaps.iter().take(3).map(|g| g.ref_id.as_str()).collect();
                let more = if gaps.len() > 3 {
                    format!(" (+{} more)", gaps.len() - 3)
                } else {
                    String::new()
                };
                format!(
                    "{} uncovered workstream(s): {}{more}",
                    gaps.len(),
                    refs.join(", ")
                )
            }
        };
        sanitize_detail(&raw)
    }
}

#[cfg(test)]
mod describe_tests {
    //! Contract for `Signal::describe` (issue #21): each variant must render a
    //! human-readable evidence line that carries the SPECIFIC observed values —
    //! never a bare "saw N problems". These tests fail until `Signal::describe`
    //! exists and enumerates the concrete fields.
    use super::*;

    /// Case-insensitive substring assertion with a helpful failure message.
    fn has(hay: &str, needle: &str) {
        assert!(
            hay.to_lowercase().contains(&needle.to_lowercase()),
            "describe() output {hay:?} must mention {needle:?} — operators need \
             the concrete observed value, not a bare count"
        );
    }

    #[test]
    fn distill_failure_rate_names_the_percentage() {
        let d = Signal::DistillFailureRate { pct: 34.0 }.describe();
        has(&d, "distill");
        has(&d, "34");
    }

    #[test]
    fn restart_churn_names_the_restart_count() {
        let d = Signal::RestartChurn { restarts: 5 }.describe();
        has(&d, "restart");
        has(&d, "5");
    }

    #[test]
    fn ladder_exhausted_names_the_count() {
        let d = Signal::LadderExhausted { count: 3 }.describe();
        has(&d, "ladder");
        has(&d, "3");
    }

    #[test]
    fn budget_pressure_names_spend_and_ceiling() {
        let d = Signal::BudgetPressure {
            spent_usd: 8.0,
            budget_usd: 10.0,
        }
        .describe();
        has(&d, "budget");
        has(&d, "8");
        has(&d, "10");
    }

    #[test]
    fn engineer_spawn_rate_names_the_live_count() {
        let d = Signal::EngineerSpawnRate { live: 12 }.describe();
        has(&d, "engineer");
        has(&d, "12");
    }

    #[test]
    fn memory_growth_names_the_node_total() {
        let d = Signal::MemoryGrowth { nodes_total: 99 }.describe();
        has(&d, "memory");
        has(&d, "99");
    }

    #[test]
    fn gym_skipped_says_gym() {
        let d = Signal::GymSkipped.describe();
        has(&d, "gym");
    }

    #[test]
    fn ci_failure_cluster_names_repo_and_failing_count() {
        let d = Signal::CiFailureCluster {
            repo: "rysweet/Simard".to_string(),
            failing: 3,
        }
        .describe();
        has(&d, "rysweet/Simard");
        has(&d, "3");
    }

    #[test]
    fn pr_ready_names_repo_and_number() {
        let d = Signal::PrReadyToMerge {
            repo: "rysweet/Simard".to_string(),
            pr: 42,
        }
        .describe();
        has(&d, "rysweet/Simard");
        has(&d, "42");
    }

    #[test]
    fn stale_goal_names_the_goal_id() {
        let d = Signal::StaleGoal {
            goal_id: "g-100".to_string(),
        }
        .describe();
        has(&d, "g-100");
    }

    #[test]
    fn anomaly_carries_its_detail() {
        let d = Signal::Anomaly {
            detail: "disk 92% full".to_string(),
        }
        .describe();
        has(&d, "disk 92% full");
    }

    #[test]
    fn loop_detected_names_goal_and_no_action_count() {
        let d = Signal::LoopDetected {
            goal_id: "g-7".to_string(),
            consecutive_no_action: 4,
        }
        .describe();
        has(&d, "g-7");
        has(&d, "4");
    }

    #[test]
    fn drift_correction_names_goal_and_detail() {
        let d = Signal::DriftCorrection {
            goal_id: "g-8".to_string(),
            detail: "scope creep into unrelated repo".to_string(),
        }
        .describe();
        has(&d, "g-8");
        has(&d, "scope creep");
    }

    #[test]
    fn goal_blocked_names_goal_id_and_reason() {
        let d = Signal::GoalBlocked {
            goal_id: "g-42".to_string(),
            reason: "needs human review".to_string(),
            perpetual: false,
            needs_review: true,
            consecutive_no_action: 6,
        }
        .describe();
        has(&d, "g-42");
        has(&d, "needs human review");
    }

    /// A hostile signal payload (terminal escape + secret-shaped token) must be
    /// neutralised by `describe()` routing through `sanitize_detail`: no raw
    /// ESC byte survives and the token is redacted, since these strings are
    /// persisted and rendered to operators.
    #[test]
    fn describe_neutralises_control_sequences_and_secrets() {
        let d = Signal::Anomaly {
            detail: "\u{1b}[31mALERT\u{1b}[0m token ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00"
                .to_string(),
        }
        .describe();
        assert!(
            !d.contains('\u{1b}'),
            "a raw ESC byte must never survive into a persisted/rendered detail: {d:?}"
        );
        assert!(
            !d.contains("ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00"),
            "a token-shaped secret must be redacted from the detail line: {d:?}"
        );
        // The benign words survive.
        assert!(
            d.to_lowercase().contains("alert"),
            "lost benign text: {d:?}"
        );
    }
}
