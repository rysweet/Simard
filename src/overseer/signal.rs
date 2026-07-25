//! The Overseer's `Signal` and `Problem` vocabulary — the Observe/Orient data
//! model. `Signal`s are cheap, additive indicators derived from one Observe pass;
//! Orient folds a set of `Signal`s into ranked, deduplicated `Problem`s.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::overseer::capabilities::{IssueReadiness, ObservedState, PrDisposition};
use crate::overseer::diagnosis::FailureCause;

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
    /// A caught decision-cycle / engineer / terminal-shell STEP FAILURE that has
    /// been DIAGNOSED (issue #2640, PART 2) — not merely logged. Carries the
    /// structured root [`FailureCause`], the exit code, and a bounded evidence
    /// excerpt so Orient/Decide can drive a CORRECTIVE workstream that targets the
    /// real WHY (e.g. arg-list-too-long → the E2BIG fix). Derived from
    /// [`ObservedState::recent_step_failures`], which the acting Overseer drains
    /// from the process-global failure sink each Observe pass. One signal per
    /// recorded diagnosis; Orient dedups same-cause failures into one problem.
    StepFailureDiagnosed {
        cause: FailureCause,
        exit_code: Option<i32>,
        evidence: String,
    },
    /// The agentic merge-queue reasoner judged an open PR STALE (#4097) — no
    /// activity for a long window. From an [`ObservedState::reasoned_prs`] entry
    /// with [`PrDisposition::Stale`](crate::overseer::capabilities::PrDisposition).
    /// Drives a gated `FlagStalePr` comment (never a merge or close). A REASONING
    /// proposal, never an authorization.
    StalePrDetected { repo: String, pr: u32 },
    /// The agentic merge-queue reasoner judged an open PR a DUPLICATE of another
    /// (#4097), carrying the ORIGINAL PR number. From an
    /// [`ObservedState::reasoned_prs`] entry with
    /// [`PrDisposition::Duplicate`](crate::overseer::capabilities::PrDisposition).
    /// Drives a gated `CloseDuplicatePr` that closes the dup referencing the
    /// original (never `--admin`/`--no-verify`).
    DuplicatePrDetected {
        repo: String,
        pr: u32,
        duplicate_of: u32,
    },
    /// The agentic reasoner triaged an open ISSUE as READY (actionable now) with
    /// no active workstream (#4097). From an [`ObservedState::triaged_issues`]
    /// entry with [`IssueReadiness::Ready`](crate::overseer::capabilities::IssueReadiness).
    /// Carries the plain-English next action so Decide can propose a workstream.
    /// A `Blocked`/`NeedsInfo` issue is NOT actionable-now and emits nothing.
    IssueNeedsWorkstream {
        repo: String,
        issue: u32,
        next_action: String,
    },
    /// The running daemon binary is behind merged `origin/main` — the
    /// authoritative "running daemon is stale" signal (issue #2590). Derived at
    /// the Observe/sensor stage from
    /// [`crate::self_deploy::ReconcileDetector::detect`] (production
    /// `GitDeploySource`) whenever `DeployDrift.needs_deploy` holds. Carries the
    /// merged-head `target_commit` a guarded self-deploy should converge on and
    /// the `behind_commits` count. Fail-safe: a git/source error reports "no
    /// drift", so this signal is simply absent rather than spuriously raised.
    /// Routed to [`ProblemKind::DeployDrift`]; Decide emits a guarded
    /// `Intervention::Deploy { commit: target_commit }` (the go/no-go SAFETY
    /// judgment stays in the guarded executor + the high-risk AutonomyGate).
    DeployDriftDetected {
        target_commit: String,
        behind_commits: usize,
    },
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

    /// The stable prefix every [`GapItem::signature`] of this category MUST begin
    /// with (`goal:` / `issue:` / `anomaly:`). Part of the bounded gap taxonomy:
    /// the sensor builds each signature as `<prefix><trusted-slug>`, so a
    /// recurring gap of the same kind always collapses onto one dedup key rather
    /// than a free-form, per-tick-unstable title. Kept in lock-step with
    /// [`GapCategory::label`] so provenance and signature never drift.
    pub fn signature_prefix(&self) -> &'static str {
        match self {
            Self::GoalUncovered => "goal:",
            Self::IssueUncovered => "issue:",
            Self::AnomalyUnaddressed => "anomaly:",
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
    /// recurring gap is deduped to at most one notification per signature.
    pub signature: String,
}

/// The single canonical prefix for a workstream-gap dedup key. The per-gap key
/// is `<DEDUP_KEY_PREFIX><signature>` and the combined per-tick key
/// (`overseer::mod::workstream_gap_key`) is `<DEDUP_KEY_PREFIX><sorted sigs>`,
/// so both filing seams agree on one stable, content-addressed token.
pub const GAP_DEDUP_KEY_PREFIX: &str = "workstream-gap:";

/// Upper bound on a gap signature's length, matching the sensor's field bound
/// (`sensor::MAX_GAP_FIELD_LEN` plus the short category prefix). A signature is
/// only ever built from trusted identifiers; this bound guarantees it stays a
/// small, safe token even for exotic ids (IV-1: dedup-search-injection defense).
pub const MAX_GAP_SIGNATURE_LEN: usize = 200;

impl GapItem {
    /// This gap's stable, restart-durable dedup key: `workstream-gap:<signature>`.
    ///
    /// The key is content-addressed on trusted identifiers only, so the SAME
    /// uncovered work always yields the SAME key across ticks — and across daemon
    /// restarts. The gap-scan notifier collapses a recurring gap onto this key
    /// (one operator notification per window) and a downstream filer can search an
    /// existing open issue by embedding it, instead of emitting a fresh,
    /// free-form-titled issue every detection tick. Centralising the key here (vs.
    /// inlining `format!("workstream-gap:{}", sig)` at each seam) stops the filing
    /// paths from silently drifting apart.
    pub fn dedup_key(&self) -> String {
        format!("{}{}", GAP_DEDUP_KEY_PREFIX, self.signature)
    }

    /// True when `signature` upholds the construction contract the sensor
    /// guarantees: it begins with this gap's [`GapCategory::signature_prefix`] and
    /// is a bounded slug in the restricted alphabet (see
    /// [`is_bounded_signature_slug`]). The notifier guards on this so a malformed
    /// signature can never inflate a notification, an issue body, or a dedup
    /// search query — the bounded taxonomy is enforced at the filing seam, not
    /// merely assumed.
    pub fn has_valid_dedup_signature(&self) -> bool {
        self.signature.starts_with(self.category.signature_prefix())
            && is_bounded_signature_slug(&self.signature)
    }
}

/// Validate that a gap signature is a bounded slug: non-empty, at most
/// [`MAX_GAP_SIGNATURE_LEN`] bytes (which equals characters for the ASCII-only
/// alphabet enforced here), beginning with an ASCII alphanumeric, and
/// composed solely of the restricted alphabet `[A-Za-z0-9:_#./-]`. This is the
/// IV-1 dedup-search-injection defense: no whitespace, quoting, or shell/search
/// metacharacter can appear in a key that is later embedded in a `gh` search
/// query or an issue body, regardless of how exotic the source identifier is.
pub fn is_bounded_signature_slug(sig: &str) -> bool {
    // `sig.len()` is the UTF-8 byte length. Because the character check below
    // admits only ASCII bytes (alphanumerics + the restricted separators), a
    // slug that passes this function is pure ASCII, so its byte length equals
    // its `char` count — the `MAX_GAP_SIGNATURE_LEN` bound is therefore an exact
    // character bound for every *valid* slug. Checking bytes up front also
    // cheaply rejects an over-long multi-byte input before the per-char scan.
    if sig.is_empty() || sig.len() > MAX_GAP_SIGNATURE_LEN {
        return false;
    }
    // Single pass: an ASCII alphanumeric is allowed anywhere; the restricted
    // separators are allowed only after the first char (so the slug must open
    // with an alphanumeric, never a separator).
    sig.char_indices().all(|(i, c)| {
        c.is_ascii_alphanumeric() || (i > 0 && matches!(c, ':' | '_' | '#' | '.' | '/' | '-'))
    })
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
    /// problem family; driven by the deduped notification act path.
    WorkstreamCoverage,
    /// A diagnosed decision-cycle / engineer / terminal-shell step failure
    /// (issue #2640, PART 2). Routed to a CORRECTIVE workstream that diagnoses
    /// the WHY and applies the remedy — never a silent log.
    StepFailure,
    /// The running daemon is behind merged `origin/main` and must self-deploy
    /// (issue #2590). Decide emits a guarded `Intervention::Deploy` to the merged
    /// head; the deploy gate + high-risk AutonomyGate own the go/no-go decision.
    DeployDrift,
}

/// How likely a single candidate cause is, relative to the others in the same
/// analysis. Ordered so `High` is the strongest; used to rank
/// [`RootCause::candidates`] and pick the primary cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Likelihood {
    Low,
    Medium,
    High,
}

impl Likelihood {
    /// Short, stable label for logs/feeds.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Overall confidence the analysis has in its primary cause. Distinct from a
/// single candidate's [`Likelihood`]: it summarises the WHOLE `RootCause`
/// (telemetry strength + memory corroboration).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    /// Short, stable label for logs/feeds.
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// Where the evidence behind a [`RootCause`] came from. A telemetry-only WHY is
/// the graceful-degrade shape when cognitive memory is unavailable; `MemoryRecall`
/// / `Both` mark a WHY corroborated by recall of prior same-signature occurrences.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CauseSource {
    /// Derived purely from the current Observe pass (evidence signals + telemetry).
    Telemetry,
    /// Derived purely from recall of prior occurrences (rare; telemetry silent).
    MemoryRecall,
    /// Telemetry-derived AND corroborated by recall of a prior same-cause occurrence.
    Both,
}

impl CauseSource {
    /// Short, stable label for logs/feeds.
    pub fn label(self) -> &'static str {
        match self {
            Self::Telemetry => "telemetry",
            Self::MemoryRecall => "memory-recall",
            Self::Both => "telemetry+memory-recall",
        }
    }
}

/// One candidate cause of a [`Problem`], with its relative [`Likelihood`] and the
/// human-readable evidence lines that support it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CauseCandidate {
    /// A short, stable, kebab-case cause label (also used as the recall/dedup key
    /// component in [`crate::overseer::root_cause::root_cause_signature`]).
    pub label: String,
    /// How likely this candidate is relative to the others in the analysis.
    pub likelihood: Likelihood,
    /// Human-readable evidence lines supporting this candidate (telemetry fields,
    /// marker strings, recalled prior outcomes).
    pub evidence: Vec<String>,
}

/// The structured, human-readable **WHY** behind a detected [`Problem`] — the
/// output of [`crate::overseer::root_cause::analyze`] and the heart of the
/// MANDATORY ROOT-CAUSE principle (issue #2635). Every problem the Overseer
/// acts on carries one, so the Overseer models *why* a problem occurred before
/// choosing an action rather than blindly patching the symptom.
///
/// `Display` renders the canonical one-line WHY (the `primary_rationale` plus
/// its confidence/source/recurrence context) for feeds, logs, and operator
/// notifications.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootCause {
    /// Ranked candidate causes, strongest first. Always non-empty (the analyzer
    /// falls back to a single low-confidence "unknown" candidate).
    pub candidates: Vec<CauseCandidate>,
    /// The human-readable primary rationale — the one-line WHY.
    pub primary_rationale: String,
    /// Overall confidence in the primary cause.
    pub confidence: Confidence,
    /// Where the analysis drew its evidence from.
    pub source: CauseSource,
    /// How many prior same-signature occurrences memory recall found for the
    /// primary cause (0 when memory is unavailable or this is a first sighting).
    /// Drives the escalate-the-root-cause-instead-of-re-patching decision.
    pub recurrence: u32,
}

impl RootCause {
    /// The primary (highest-likelihood) candidate cause, if any.
    pub fn primary(&self) -> Option<&CauseCandidate> {
        self.candidates.first()
    }

    /// A last-resort WHY used only when no analyzer branch and no evidence apply,
    /// so a `RootCause` value always exists (the Overseer never faces a problem
    /// with no WHY). Telemetry-sourced, low confidence, zero recurrence.
    pub fn unknown() -> Self {
        Self {
            candidates: vec![CauseCandidate {
                label: "unknown-cause".to_string(),
                likelihood: Likelihood::Low,
                evidence: vec!["no distinguishing telemetry or recall available".to_string()],
            }],
            primary_rationale: "cause not yet determined from available telemetry".to_string(),
            confidence: Confidence::Low,
            source: CauseSource::Telemetry,
            recurrence: 0,
        }
    }
}

impl fmt::Display for RootCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} (confidence: {}, source: {}",
            self.primary_rationale,
            self.confidence.label(),
            self.source.label()
        )?;
        if self.recurrence > 0 {
            write!(f, ", seen {}× before", self.recurrence)?;
        }
        write!(f, ")")
    }
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
    /// The MANDATORY root-cause analysis (issue #2635). `None` immediately after
    /// the pure Orient fold; populated by the Overseer's `run_cycle` enrichment
    /// step (recall + [`crate::overseer::root_cause::analyze`]) before Decide, so
    /// every problem the Overseer acts on carries a structured WHY.
    pub why: Option<RootCause>,
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

    // Diagnosed step failures (issue #2640, PART 2): each recorded diagnosis
    // becomes a corrective signal so a caught decision-cycle / engineer /
    // terminal-shell failure drives a fix instead of a silent log line. Orient
    // dedups same-cause failures into one problem.
    for diagnosis in &state.recent_step_failures {
        out.push(Signal::StepFailureDiagnosed {
            cause: diagnosis.cause,
            exit_code: diagnosis.exit_code,
            evidence: diagnosis.evidence.clone(),
        });
    }

    // Autonomous self-deploy drift (issue #2590): the running binary is behind
    // merged main. The effectful, fail-safe git probe already ran in the observe
    // rail (which leaves `deploy_drift = None` on any error / current daemon), so
    // this lift is pure — no drift observed ⇒ no signal ⇒ no deploy.
    if let Some(drift) = &state.deploy_drift {
        out.push(Signal::DeployDriftDetected {
            target_commit: drift.target_commit.clone(),
            behind_commits: drift.behind_commits,
        });
    }

    // Agentic merge-queue reasoning (#4097): the reviewer's per-PR PROPOSALS.
    // CRITICAL invariant — a `ReadyForMerge` REASONING never itself authorizes a
    // merge: it emits NO `PrReadyToMerge` here. Merge authorization comes ONLY
    // from the re-narrowed `ready_prs` projection above. Only the non-merge
    // dispositions (`Stale`/`Duplicate`) turn into their own gated interventions.
    for rp in &state.reasoned_prs {
        match rp.disposition {
            PrDisposition::Stale => out.push(Signal::StalePrDetected {
                repo: rp.repo.clone(),
                pr: rp.pr,
            }),
            PrDisposition::Duplicate => {
                // Parse already guarantees `duplicate_of` is `Some` for a coherent
                // `Duplicate`; guard anyway so a stray one is dropped, not merged
                // with a fabricated original.
                if let Some(original) = rp.duplicate_of {
                    out.push(Signal::DuplicatePrDetected {
                        repo: rp.repo.clone(),
                        pr: rp.pr,
                        duplicate_of: original,
                    });
                }
            }
            PrDisposition::ReadyForMerge | PrDisposition::NeedsWork => {}
        }
    }

    // Agentic issue triage (#4097): a Ready (actionable-now) issue surfaces a
    // workstream proposal. A Blocked/NeedsInfo issue is deliberately silent — it
    // is not actionable this pass and must not spawn a workstream.
    for issue in &state.triaged_issues {
        if issue.readiness == IssueReadiness::Ready {
            out.push(Signal::IssueNeedsWorkstream {
                repo: issue.repo.clone(),
                issue: issue.issue,
                next_action: issue.next_action.clone(),
            });
        }
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
            Signal::StepFailureDiagnosed {
                cause,
                exit_code,
                evidence,
            } => {
                let code = exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string());
                format!(
                    "diagnosed step failure: {} (exit {code}) — {evidence}",
                    cause.as_str()
                )
            }
            Signal::StalePrDetected { repo, pr } => {
                format!("PR {repo}#{pr} judged stale (no recent activity)")
            }
            Signal::DuplicatePrDetected {
                repo,
                pr,
                duplicate_of,
            } => format!("PR {repo}#{pr} judged a duplicate of #{duplicate_of}"),
            Signal::IssueNeedsWorkstream {
                repo,
                issue,
                next_action,
            } => format!("issue {repo}#{issue} ready with no workstream — next: {next_action}"),
            Signal::DeployDriftDetected {
                target_commit,
                behind_commits,
            } => format!(
                "running binary {behind_commits} commit(s) behind merged main \
                 (deploy target {target_commit})"
            ),
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

#[cfg(test)]
mod gap_dedup_key_tests {
    //! Contract for the bounded workstream-gap taxonomy (Problem 2, issue #4687):
    //! every gap carries a stable, restart-durable, injection-safe dedup key so a
    //! recurring gap collapses onto one key instead of flooding the tracker with
    //! near-duplicate, free-form-titled issues on every daemon restart.
    use super::*;

    fn gap(category: GapCategory, signature: &str) -> GapItem {
        GapItem {
            category,
            ref_id: "ref".to_string(),
            title: "title".to_string(),
            why_it_matters: "why".to_string(),
            signature: signature.to_string(),
        }
    }

    #[test]
    fn dedup_key_is_the_stable_prefixed_signature() {
        let g = gap(GapCategory::GoalUncovered, "goal:g-hot");
        assert_eq!(g.dedup_key(), "workstream-gap:goal:g-hot");
        // The centralised prefix is the single source of truth for both seams.
        assert!(g.dedup_key().starts_with(GAP_DEDUP_KEY_PREFIX));
    }

    #[test]
    fn identical_uncovered_work_yields_an_identical_key_across_ticks() {
        // Content-addressed on trusted identifiers only: a re-detection of the
        // same gap (a fresh GapItem, as after a daemon restart) produces the same
        // key, so the notifier/filer deduplicates it instead of re-filing.
        let first = gap(GapCategory::IssueUncovered, "issue:rysweet/simard#4687");
        let after_restart = gap(GapCategory::IssueUncovered, "issue:rysweet/simard#4687");
        assert_eq!(first.dedup_key(), after_restart.dedup_key());
    }

    #[test]
    fn category_prefix_tracks_the_label() {
        for (cat, prefix, label) in [
            (GapCategory::GoalUncovered, "goal:", "goal"),
            (GapCategory::IssueUncovered, "issue:", "issue"),
            (GapCategory::AnomalyUnaddressed, "anomaly:", "anomaly"),
        ] {
            assert_eq!(cat.signature_prefix(), prefix);
            assert_eq!(cat.signature_prefix().trim_end_matches(':'), cat.label());
            assert_eq!(cat.label(), label);
        }
    }

    #[test]
    fn well_formed_gaps_from_every_category_are_valid() {
        assert!(gap(GapCategory::GoalUncovered, "goal:g-hot").has_valid_dedup_signature());
        assert!(
            gap(GapCategory::IssueUncovered, "issue:rysweet/simard#4687")
                .has_valid_dedup_signature()
        );
        assert!(
            gap(
                GapCategory::AnomalyUnaddressed,
                "anomaly:distill_parse_fail_rate_high",
            )
            .has_valid_dedup_signature()
        );
    }

    #[test]
    fn a_signature_missing_its_category_prefix_is_rejected() {
        // A goal gap whose signature does not start with `goal:` breaks the
        // bounded-taxonomy contract and must not be treated as dedupable.
        assert!(
            !gap(GapCategory::GoalUncovered, "issue:rysweet/simard#1").has_valid_dedup_signature()
        );
    }

    #[test]
    fn injection_and_unbounded_signatures_are_rejected() {
        // IV-1: a signature carrying whitespace / search or shell metacharacters
        // (as free-form text would) is not a bounded slug and is rejected before
        // it can reach a `gh` search query or an issue body.
        assert!(!is_bounded_signature_slug(""));
        assert!(!is_bounded_signature_slug("goal:has space"));
        assert!(!is_bounded_signature_slug("goal:\"quoted\""));
        assert!(!is_bounded_signature_slug("goal:a`b"));
        assert!(!is_bounded_signature_slug(":leading-colon"));
        assert!(!is_bounded_signature_slug(&format!(
            "goal:{}",
            "x".repeat(MAX_GAP_SIGNATURE_LEN)
        )));
        // The legitimate trusted-slug shapes stay valid.
        assert!(is_bounded_signature_slug("goal:g-hot"));
        assert!(is_bounded_signature_slug("issue:rysweet/simard#4687"));
        assert!(is_bounded_signature_slug(
            "anomaly:distill_parse_fail_rate_high"
        ));
    }
}
