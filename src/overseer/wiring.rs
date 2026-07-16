//! Daemon wiring for the acting Overseer co-process (M2+).
//!
//! This module is the seam that turns the Overseer from an inert design sketch
//! into a live, in-process co-process the OODA daemon drives on a cadence. It
//! provides four things:
//!
//! 1. [`OverseerCadence`] — a tiny, injected-clock-testable interval scheduler
//!    (mirrors the daemon's sibling `should_run_*` periodic-task pattern).
//! 2. [`overseer_tick`] / [`run_overseer_tick_isolated`] — the drive helper that
//!    runs one meta-OODA turn (`run_cycle` → act on every admitted intervention)
//!    with structured per-tick tracing and **panic isolation**, so an error or
//!    panic in a tick is caught and logged and never crashes or stalls the
//!    daemon or the OODA loop.
//! 3. [`assemble_capabilities`] / [`build_overseer`] — construct the production
//!    [`Overseer`] from the already-shipped capability adapters, under a
//!    DISTINCT anti-recursion identity ([`overseer_identity`]) and with acting
//!    autonomy (verify-merge + HIGH-RISK) enabled.
//! 4. [`BoardGoalCurator`] — the production [`GoalCurator`] adapter that reads
//!    Simard's live goal board so Orient dedups against in-flight engineer work
//!    and the Overseer never fights the OODA loop.
//!
//! Reuse (never reimplementation): every capability is a thin adapter over an
//! existing Simard entry point — `SnapshotStatusReader` (status::assemble),
//! `SmartOrchestratorLauncher` (amplihack recipe run), `MergePrOps` (gated merge
//! authority: poll-until-green, NEVER `--admin`/`--no-verify`, notify-on-merge),
//! `MeetingGoalTransfer`, `StewardshipIssueFiler`, `SelfQualityAuditor`, and the
//! goal board via `goal_curation`.

use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::cognitive_memory::{CognitiveMemoryOps, RecallWeightSet};
use crate::goal_curation::{BacklogItem, GoalBoard, GoalProgress};
use crate::goal_curation::{
    DEFAULT_STEWARD_SCORE, add_backlog_item, load_goal_board, save_goal_board,
};

use crate::overseer::audit::SelfQualityAuditor;
use crate::overseer::capabilities::{
    BlockedGoal, DeployReport, Deployer, GoalBrief, GoalCurator, InFlightItem, IssueOutcome,
    MemoryRecall, ObservationEpisode, OverseerError, RecallKeys, RecalledEpisode, RecalledFact,
    RecalledProcedure, RecalledProspective, RecordOutcome,
};
use crate::overseer::config::{
    claim_reap_enabled, claim_reap_stale_secs, gap_scan_enabled, goal_health_enabled,
    memory_recall_enabled, overseer_author_login, overseer_interval_secs, whisper_enabled,
};
use crate::overseer::deploy::GuardedDeployer;
use crate::overseer::guardrails::RecursionGuard;
use crate::overseer::intervention::{Intervention, PlannedIntervention};
use crate::overseer::launch::SmartOrchestratorLauncher;
use crate::overseer::meeting_ops::MeetingGoalTransfer;
use crate::overseer::merge_ops::MergePrOps;
use crate::overseer::notify::DualChannelNotifier;
use crate::overseer::observer::StewardshipIssueFiler;
use crate::overseer::sensor::{
    SnapshotStatusReader, SurveyedIssue, blocked_goals_from_board, detect_workstream_gaps,
    in_flight_from_board,
};
use crate::overseer::signal::{DETAIL_CAP, GapItem, Signal, sanitize_detail};
use crate::overseer::{ActOutcome, Capabilities, CycleReport, Overseer};

// ─────────────────────────── cadence scheduler ─────────────────────────────

/// Interval scheduler for the periodic Overseer tick. Deliberately clock-free:
/// the caller feeds a monotonic `now_secs`, so production uses
/// `Instant::elapsed().as_secs()` while tests inject a virtual clock and assert
/// the tick fires exactly on/after the interval boundary. Mirrors the daemon's
/// sibling periodic tasks (backup / disk-health / brain-introspection).
#[derive(Clone, Copy, Debug)]
pub struct OverseerCadence {
    interval_secs: u64,
    last_tick_secs: u64,
}

impl OverseerCadence {
    /// Start the cadence at `now_secs` (the first tick fires one full interval
    /// later — nothing useful to do at t=0). The interval is floored at 1s so a
    /// pathological `0` can never busy-fire.
    pub fn new(interval_secs: u64, now_secs: u64) -> Self {
        Self {
            interval_secs: interval_secs.max(1),
            last_tick_secs: now_secs,
        }
    }

    /// The interval this cadence fires on (seconds).
    pub fn interval_secs(&self) -> u64 {
        self.interval_secs
    }

    /// Return `true` (and advance the last-tick marker to `now_secs`) exactly
    /// when at least one interval has elapsed since the previous tick; otherwise
    /// `false` and no state change. Monotonic: `now_secs` going backwards never
    /// fires.
    pub fn due(&mut self, now_secs: u64) -> bool {
        if now_secs.saturating_sub(self.last_tick_secs) >= self.interval_secs {
            self.last_tick_secs = now_secs;
            true
        } else {
            false
        }
    }
}

// ─────────────────────────── per-tick report ───────────────────────────────

/// Structured tally of one Overseer tick. Every field is emitted as a
/// `tracing` key so a tick is fully observable without `println!`/`eprintln!`.
///
/// Derives `Serialize`/`Deserialize` (additive; no logic change) so the acting
/// tick's outcome can be recorded verbatim into the durable
/// [activity feed](crate::overseer::activity). The `*_details` vectors carry the
/// human-readable, per-tick DETAIL lines (issue #21) — WHAT was observed and
/// WHAT was done, with concrete values — alongside the summary counts. Both are
/// `#[serde(default)]` so a feed written by an older build (which lacked them)
/// still deserializes, defaulting to empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverseerTickReport {
    /// Problems raised this cycle (post-dedup against in-flight work).
    pub problems: usize,
    /// Deduplicated issues filed via the stewardship path.
    pub issues_filed: usize,
    /// Fix workstreams launched (smart-orchestrator/default-workflow).
    pub recipes_launched: usize,
    /// Green, merge-ready PRs verified-and-merged (normal merge; never admin).
    pub prs_merged: usize,
    /// Guarded deploys performed (through the canary/self-deploy gates).
    pub deploys: usize,
    /// Interventions handed off to the operator (escalations).
    pub escalations: usize,
    /// Problems for which a structured root-cause WHY was produced this tick
    /// (issue #2635) — the MANDATORY analysis count (every problem gets one).
    pub root_cause_analyses: usize,
    /// Symptom-only mitigations taken this tick whose ROOT CAUSE was left
    /// unaddressed (issue #2635) — surfaced, never a silent patch.
    pub symptom_mitigations: usize,
    /// Interventions this tick that addressed the root cause — class `RootCause`
    /// or `Acknowledged` (`remediation.root_cause_addressed == true`).
    pub root_causes_addressed: usize,
    /// Interventions the gates held (autonomy/budget/conflict).
    pub held: usize,
    /// Advisory whispers delivered into Simard's OODA inbox this tick.
    pub whispers: usize,
    /// Whispers suppressed by the dedup window / per-hour cap this tick.
    pub whispers_suppressed: usize,
    /// False-parked standing/perpetual goals auto-unblocked + reactivated this
    /// tick (the self-heal path).
    pub goals_unblocked: usize,
    /// Genuinely-blocked "needs human review" goals escalated to the operator
    /// (email + Signal) this tick.
    pub goals_escalated: usize,
    /// Goal-board self-heal / escalation actions suppressed by the dedup gate
    /// this tick.
    pub goals_health_suppressed: usize,
    /// Backlog-coverage gaps FLAGGED this tick by the recurring gap-scan — each
    /// got the consolidated operator notification + one deduped issue. A
    /// DEDICATED counter, never folded into `issues_filed` / `escalations`.
    pub workstream_gaps_detected: usize,
    /// Backlog-coverage gaps SUPPRESSED this tick (a recurring gap within the
    /// dedup window — not re-notified/re-filed).
    pub workstream_gaps_suppressed: usize,
    /// Capability errors encountered while acting (isolated, never fatal).
    pub errors: usize,
    /// Completed whole cognitive-memory recall passes this tick (issue #2628):
    /// **1** when all four bounded sub-reads returned `Ok` (even on an empty
    /// graph), **0** otherwise. At most 1 per tick.
    pub memory_recalls: usize,
    /// Episodic observations actually persisted back into memory this tick
    /// (dedup suppressions excluded).
    pub memory_writes: usize,
    /// Surfaced memory failures this tick — a failed recall pass and/or a failed
    /// write-back (0, 1, or 2), never swallowed (no silent fallback).
    pub memory_errors: usize,
    /// Set when the tick itself panicked and was isolated by
    /// [`run_overseer_tick_isolated`].
    pub panicked: bool,
    /// Wall-clock duration of the tick in milliseconds.
    pub duration_ms: u64,
    /// Human-readable lines describing WHAT was observed this tick — the ranked
    /// problems and their concrete evidence signals, plus benign observations
    /// that raised no problem. Bounded to [`DETAIL_CAP`] with a `(+N more)`
    /// sentinel. Additive (issue #21); empty on ticks that observed nothing.
    pub observed_details: Vec<String>,
    /// Human-readable lines describing WHAT was done this tick — each admitted
    /// intervention and its outcome (`did: …`), each held intervention and why
    /// (`held: …`), and isolated failures. Bounded to [`DETAIL_CAP`] with a
    /// `(+N more)` sentinel. Additive (issue #21); empty on no-action ticks.
    pub action_details: Vec<String>,
}

/// Run ONE Overseer meta-OODA turn: `run_cycle` (Observe→Orient→Decide→plan)
/// then execute every ADMITTED intervention via `act`, tallying outcomes and
/// emitting one structured `tracing` event. Errors from `run_cycle` or any
/// `act` are caught and counted (never propagated) so a single bad capability
/// call cannot abort the tick — and the daemon never sees a `Result::Err`.
///
/// This does NOT catch panics; wrap it in [`run_overseer_tick_isolated`] for
/// the full fail-safe boundary the daemon uses.
pub fn overseer_tick(overseer: &mut Overseer) -> OverseerTickReport {
    overseer_tick_detailed(overseer).0
}

/// Like [`overseer_tick`] but also returns the per-problem [`ProblemEntry`] rows
/// (issue #2635) — problem + WHY + action + root-cause/symptom — so the daemon
/// can persist them into the durable activity feed. The scalar
/// [`OverseerTickReport`] stays `Copy`; the rich per-problem detail rides
/// alongside it here.
pub fn overseer_tick_detailed(
    overseer: &mut Overseer,
) -> (
    OverseerTickReport,
    Vec<crate::overseer::activity::ProblemEntry>,
) {
    let start = Instant::now();
    let mut report = OverseerTickReport::default();
    let mut problem_entries = Vec::new();

    match overseer.run_cycle() {
        Ok(cycle) => {
            report.problems = cycle.problems.len();

            // Cognitive-memory recall (#2628) counters, derived from the Observe
            // pass. A completed whole recall pass leaves `recall = Some(..)`
            // (even for an empty graph); a failed pass leaves `recall = None`
            // and `recall_error = Some(..)` — mutually exclusive, so a pass adds
            // to exactly one of `memory_recalls` / `memory_errors`.
            if cycle.observed.recall.is_some() {
                report.memory_recalls += 1;
            }
            if cycle.observed.recall_error.is_some() {
                report.memory_errors += 1;
            }

            // Every problem gets a WHY (the MANDATORY analysis, issue #2635),
            // regardless of whether the chosen action is later admitted or succeeds.
            report.root_cause_analyses = cycle.problems.iter().filter(|p| p.why.is_some()).count();

            report.observed_details = cap_details(observed_details_from(&cycle));
            let mut actions: Vec<String> = Vec::new();
            for (i, planned) in cycle.plan.iter().enumerate() {
                if !planned.admitted {
                    report.held += 1;
                    actions.push(describe_hold(planned));
                    continue;
                }
                match overseer.act(&planned.intervention) {
                    Ok(outcome) => {
                        tally_outcome(&mut report, &outcome);
                        actions.push(describe_action(&planned.intervention, &outcome));
                        // Root-cause honesty (issue #2635): tally the remediation
                        // ONLY for an action that actually took effect (not a
                        // dedup/rate-limit suppression), so the feed never claims
                        // a cause was addressed / a symptom mitigated when the act
                        // was a no-op. An errored act is counted under `errors`
                        // below and contributes to neither tally.
                        if outcome_takes_effect(&outcome) {
                            if planned.remediation.class
                                == crate::overseer::intervention::RemediationClass::SymptomMitigation
                            {
                                report.symptom_mitigations += 1;
                            }
                            if planned.remediation.root_cause_addressed {
                                report.root_causes_addressed += 1;
                            }
                        }
                        // Best-effort: record this occurrence's root-cause
                        // signature + cause + action + outcome into cognitive
                        // memory (amplihack-memory-lib, G2) so recall on a later
                        // cycle raises `recurrence` — turning a one-off into a
                        // detected recurring root cause. Only for effective
                        // actions (never suppressed no-ops); never fatal.
                        if outcome_records_occurrence(&outcome)
                            && let Some(entry) = cycle.entries.get(i)
                        {
                            overseer.record_occurrence(entry, &outcome);
                        }
                    }
                    Err(e) => {
                        report.errors += 1;
                        actions.push(describe_act_error(&planned.intervention, &e));
                        tracing::warn!(
                            target: "overseer::tick",
                            intervention = planned.intervention.label(),
                            error = %e,
                            "overseer intervention failed — isolated, continuing"
                        );
                    }
                }
            }
            report.action_details = cap_details(actions);

            // Deliberate, de-duplicated write-back of the Overseer's own
            // observation (#2628). A store increments `memory_writes`; a
            // de-duplicated / disabled / clean-tick write records nothing; a
            // backing-store error is SURFACED and counted (never swallowed) and
            // the tick still completes.
            match overseer.write_back_observation(&cycle.problems) {
                Ok(Some(RecordOutcome::Stored { .. })) => report.memory_writes += 1,
                Ok(_) => {}
                Err(e) => {
                    report.memory_errors += 1;
                    tracing::warn!(
                        target: "overseer::memory",
                        error = %e,
                        "overseer memory write-back failed — surfaced, continuing"
                    );
                }
            }

            // Hand the per-problem feed rows back for durable surfacing (#2635).
            problem_entries = cycle.entries;
        }
        Err(e) => {
            report.errors += 1;
            tracing::warn!(
                target: "overseer::tick",
                error = %e,
                "overseer run_cycle failed — isolated, no actions taken"
            );
        }
    }

    report.duration_ms = start.elapsed().as_millis() as u64;
    tracing::info!(
        target: "overseer::tick",
        enabled = true,
        problems = report.problems,
        issues_filed = report.issues_filed,
        recipes_launched = report.recipes_launched,
        prs_merged = report.prs_merged,
        deploys = report.deploys,
        escalations = report.escalations,
        root_cause_analyses = report.root_cause_analyses,
        symptom_mitigations = report.symptom_mitigations,
        root_causes_addressed = report.root_causes_addressed,
        held = report.held,
        whispers = report.whispers,
        whispers_suppressed = report.whispers_suppressed,
        goals_unblocked = report.goals_unblocked,
        goals_escalated = report.goals_escalated,
        goals_health_suppressed = report.goals_health_suppressed,
        memory_recalls = report.memory_recalls,
        memory_writes = report.memory_writes,
        memory_errors = report.memory_errors,
        workstream_gaps_detected = report.workstream_gaps_detected,
        workstream_gaps_suppressed = report.workstream_gaps_suppressed,
        errors = report.errors,
        duration_ms = report.duration_ms,
        "overseer tick complete"
    );
    (report, problem_entries)
}

/// Panic-isolated wrapper around [`overseer_tick`]. A panic inside a capability
/// adapter is caught, logged, and turned into a report with `panicked = true`;
/// the daemon loop continues unaffected. This is the boundary that guarantees
/// "a panicking overseer tick never crashes or stalls the daemon".
pub fn run_overseer_tick_isolated(overseer: &mut Overseer) -> OverseerTickReport {
    run_overseer_tick_isolated_detailed(overseer).0
}

/// Like [`run_overseer_tick_isolated`] but also returns the per-problem
/// [`ProblemEntry`] rows (issue #2635) for durable feed surfacing. A panic
/// yields a `panicked = true` report and an empty entry vector.
pub fn run_overseer_tick_isolated_detailed(
    overseer: &mut Overseer,
) -> (
    OverseerTickReport,
    Vec<crate::overseer::activity::ProblemEntry>,
) {
    let start = Instant::now();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        overseer_tick_detailed(overseer)
    })) {
        Ok(result) => result,
        Err(_) => {
            tracing::error!(
                target: "overseer::tick",
                panicked = true,
                "overseer tick panicked — isolated; daemon continues"
            );
            (
                OverseerTickReport {
                    panicked: true,
                    errors: 1,
                    duration_ms: start.elapsed().as_millis() as u64,
                    ..OverseerTickReport::default()
                },
                Vec::new(),
            )
        }
    }
}

fn tally_outcome(report: &mut OverseerTickReport, outcome: &ActOutcome) {
    match outcome {
        ActOutcome::Launched(_) => report.recipes_launched += 1,
        ActOutcome::Merged => report.prs_merged += 1,
        ActOutcome::Deployed(_) => report.deploys += 1,
        ActOutcome::IssueFiled(_) => report.issues_filed += 1,
        ActOutcome::Escalated => report.escalations += 1,
        ActOutcome::Whispered { .. } => report.whispers += 1,
        ActOutcome::WhisperSuppressed { .. } => report.whispers_suppressed += 1,
        ActOutcome::GoalUnblocked { .. } => report.goals_unblocked += 1,
        ActOutcome::GoalEscalated { .. } => report.goals_escalated += 1,
        ActOutcome::GoalHealthSuppressed { .. } => report.goals_health_suppressed += 1,
        ActOutcome::WorkstreamGapsFlagged {
            flagged,
            suppressed,
        } => {
            report.workstream_gaps_detected += flagged;
            report.workstream_gaps_suppressed += suppressed;
        }
        ActOutcome::ConflictResolved
        | ActOutcome::GoalTransferred
        | ActOutcome::Reported
        | ActOutcome::Audited => {}
    }
}

// ─────────────────── informative detail rendering (issue #21) ───────────────
//
// These pure renderers turn the typed cycle output into the human-readable
// `observed_details` / `action_details` lines the activity log carries. They
// enumerate the ACTUAL problems and actions with concrete values, so an operator
// can tell WHAT the Overseer saw and WHAT it did — never just "saw N problems".

/// Bound a detail list to [`DETAIL_CAP`], replacing the overflow with a single
/// `(+N more)` sentinel so the persisted feed stays deterministic and small.
fn cap_details(mut v: Vec<String>) -> Vec<String> {
    if v.len() > DETAIL_CAP {
        let extra = v.len() - DETAIL_CAP;
        v.truncate(DETAIL_CAP);
        v.push(format!("(+{extra} more)"));
    }
    v
}

/// Build the WHAT-was-observed lines: each ranked problem (kind + concrete
/// summary), enumerating its individual evidence signals only when more than one
/// merged into the problem (a single signal's `describe` just restates the
/// summary), followed by any benign observed signals that raised no problem
/// (e.g. a signal already covered by Simard's in-flight work). Ordered
/// deterministically (problems first, in rank order) for hermetic tests.
fn observed_details_from(cycle: &CycleReport) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut used: Vec<&Signal> = Vec::new();
    for p in &cycle.problems {
        out.push(sanitize_detail(&format!("{:?} — {}", p.kind, p.summary)));
        // Enumerate the merged evidence only when it adds detail beyond the
        // summary (multiple signals folded into one problem).
        if p.evidence.len() > 1 {
            for ev in &p.evidence {
                out.push(format!("  {}", ev.describe()));
            }
        }
        for ev in &p.evidence {
            used.push(ev);
        }
    }
    // Signals that never became a problem are still worth surfacing (benign, but
    // "state why nothing happened" beats silence).
    for sig in &cycle.signals {
        if !used.contains(&sig) {
            out.push(sig.describe());
        }
    }
    out
}

/// `owner/name#pr` for a PR-shaped intervention, else the generic target.
fn pr_target(iv: &Intervention) -> String {
    match iv {
        Intervention::VerifyAndMergePr { repo, pr }
        | Intervention::ResolveConflict { repo, pr } => {
            format!("{repo}#{pr}")
        }
        _ => intervention_target(iv),
    }
}

/// A short, human-readable descriptor of an intervention's TARGET (the subject it
/// acts on), used to name held/failed actions concretely.
fn intervention_target(iv: &Intervention) -> String {
    match iv {
        Intervention::LaunchRecipe { brief } => format!("recipe for {}", brief.target_repo),
        Intervention::VerifyAndMergePr { repo, pr } => format!("verify-and-merge {repo}#{pr}"),
        Intervention::ResolveConflict { repo, pr } => format!("resolve-conflict {repo}#{pr}"),
        Intervention::Deploy { commit } => format!("deploy {}", short_commit(commit)),
        Intervention::FileIssue { run } => {
            format!("issue for {} ({})", run.source_module, run.failure_kind)
        }
        Intervention::TransferGoal { goal } => format!("transfer goal '{}'", goal.title),
        Intervention::Report => "status report".to_string(),
        Intervention::RunAudit { .. } => "quality audit".to_string(),
        Intervention::Escalate { reason } => format!("escalation: {reason}"),
        Intervention::Whisper { .. } => "advisory whisper".to_string(),
        Intervention::UnblockGoal { goal_id, .. } => format!("unblock goal {goal_id}"),
        Intervention::EscalateBlockedGoal { goal_id, .. } => {
            format!("escalate blocked goal {goal_id}")
        }
        Intervention::FlagWorkstreamGaps { gaps } => {
            format!("flag {} uncovered workstream(s)", gaps.len())
        }
    }
}

/// The operator-facing reason an escalation was raised.
fn escalate_reason(iv: &Intervention) -> String {
    match iv {
        Intervention::Escalate { reason } | Intervention::EscalateBlockedGoal { reason, .. } => {
            reason.clone()
        }
        _ => intervention_target(iv),
    }
}

/// First 12 chars of a commit SHA (enough to identify, short enough that the
/// full 40-char blob never trips the high-entropy secret redactor).
fn short_commit(c: &str) -> String {
    c.chars().take(12).collect()
}

/// Render one ADMITTED intervention + its outcome as a `did: …` action line
/// carrying the concrete identifiers (PR numbers, issue URLs, workstream ids,
/// goal ids) an operator needs. Sanitised, so a capability that returned a
/// secret-bearing url/id can never leak it into the persisted feed.
fn describe_action(iv: &Intervention, outcome: &ActOutcome) -> String {
    let body = match outcome {
        ActOutcome::Launched(h) => {
            format!("launched workstream {} ({})", h.id, intervention_target(iv))
        }
        ActOutcome::Merged => format!("merged PR {}", pr_target(iv)),
        ActOutcome::ConflictResolved => format!("resolved conflicts on {}", pr_target(iv)),
        ActOutcome::Deployed(r) => format!("deployed {}", short_commit(&r.deployed_commit)),
        ActOutcome::IssueFiled(o) => match o {
            IssueOutcome::FiledNew { url } => format!("filed issue {url}"),
            IssueOutcome::MatchedExisting { url } => format!("matched existing issue {url}"),
        },
        ActOutcome::GoalTransferred => format!("transferred {}", intervention_target(iv)),
        ActOutcome::Reported => "emitted a status report".to_string(),
        ActOutcome::Audited => "ran a quality audit".to_string(),
        ActOutcome::Escalated => format!("escalated to operator: {}", escalate_reason(iv)),
        ActOutcome::Whispered { signature, .. } => {
            format!("whispered steering note ({signature})")
        }
        ActOutcome::WhisperSuppressed { reason } => format!("whisper suppressed — {reason}"),
        ActOutcome::GoalUnblocked { goal_id } => {
            format!("self-healed blocked goal {goal_id} (unblocked + reactivated)")
        }
        ActOutcome::GoalEscalated { goal_id } => {
            format!("escalated blocked goal {goal_id} for human review")
        }
        ActOutcome::GoalHealthSuppressed { reason } => {
            format!("goal-board action suppressed — {reason}")
        }
        ActOutcome::WorkstreamGapsFlagged {
            flagged,
            suppressed,
        } => {
            if *flagged > 0 {
                format!(
                    "flagged {flagged} uncovered workstream(s) — notified operator + filed deduped issue(s) ({suppressed} suppressed)"
                )
            } else {
                format!("workstream gaps suppressed — {suppressed} within the dedup window")
            }
        }
    };
    sanitize_detail(&format!("did: {body}"))
}

/// Render a HELD intervention as a `held: …` line that names the concrete target
/// it declined to act on AND the gate reason — so "no action" is always
/// explained, never a silent gap.
fn describe_hold(planned: &PlannedIntervention) -> String {
    sanitize_detail(&format!(
        "held: {} — {}",
        intervention_target(&planned.intervention),
        planned.note
    ))
}

/// Render an isolated act failure as a `did: … failed` line, classified by the
/// failing capability/gate. Sanitised so a raw error body (which may echo a
/// token) never leaks into the persisted, operator-visible feed.
fn describe_act_error(iv: &Intervention, err: &OverseerError) -> String {
    let (kind, detail): (&str, String) = match err {
        OverseerError::Capability { what, detail } => (what, detail.clone()),
        OverseerError::Gated { risk, .. } => ("gated", format!("risk={risk}")),
        OverseerError::Budget {
            spent_usd,
            budget_usd,
        } => ("budget", format!("${spent_usd:.2} of ${budget_usd:.2}")),
        OverseerError::Recursion { subject } => ("recursion", format!("own {subject}")),
        OverseerError::Conflict { with } => ("conflict", format!("overlaps {with}")),
        OverseerError::NotMergeReady { pr, reason } => {
            ("merge readiness", format!("PR #{pr}: {reason}"))
        }
    };
    sanitize_detail(&format!(
        "did: {} failed — {kind}: {detail} (isolated)",
        intervention_target(iv)
    ))
}

/// True when an act outcome represents a REAL intervention on a root cause (so
/// its occurrence is worth recording for recurrence tracking) rather than a
/// suppressed no-op (a dedup-window/rate-limit suppression records nothing —
/// avoiding double-counting a cause the Overseer did not actually act on again).
fn outcome_records_occurrence(outcome: &ActOutcome) -> bool {
    matches!(
        outcome,
        ActOutcome::Launched(_)
            | ActOutcome::Merged
            | ActOutcome::Deployed(_)
            | ActOutcome::IssueFiled(_)
            | ActOutcome::Escalated
            | ActOutcome::Whispered { .. }
            | ActOutcome::GoalUnblocked { .. }
            | ActOutcome::GoalEscalated { .. }
            | ActOutcome::ConflictResolved
            | ActOutcome::GoalTransferred
            | ActOutcome::Audited
    )
}

/// True when an act outcome actually TOOK EFFECT this tick — anything except a
/// dedup/rate-limit suppression no-op. Used to tally the root-cause/symptom
/// remediation honestly: an acknowledged `Reported` deliberate block took effect
/// (it was surfaced), a suppressed self-heal/whisper did NOT (it was a no-op), so
/// the feed never claims a cause was addressed / a symptom mitigated when nothing
/// happened.
fn outcome_takes_effect(outcome: &ActOutcome) -> bool {
    !matches!(
        outcome,
        ActOutcome::WhisperSuppressed { .. } | ActOutcome::GoalHealthSuppressed { .. }
    )
}

// ─────────────────────────── identity ──────────────────────────────────────

/// The Overseer's DISTINCT anti-recursion identity: a stable, well-known bot
/// login plus the branch/goal namespaces its launched work uses. Sourced from
/// [`overseer_author_login`] so the daemon, the merge path, and goal dedup all
/// agree on ONE identity that is never the human operator's login. A fully
/// populated guard fails CLOSED and refuses the Overseer's own PRs/commits,
/// branches, and goals.
pub fn overseer_identity() -> RecursionGuard {
    RecursionGuard {
        author_login: overseer_author_login(),
        branch_prefix: "overseer/".to_string(),
        goal_source_tag: "overseer:".to_string(),
    }
}

// ─────────────────────────── goal-board curator ────────────────────────────

/// Production [`GoalCurator`]: reads Simard's live goal board so Orient dedups
/// the Overseer's problems against in-flight engineer goals, and enqueues
/// proposed goals onto the backlog under the Overseer's own source tag.
///
/// Reuse: `goal_curation::{load_goal_board, add_backlog_item, save_goal_board}`
/// and [`in_flight_from_board`].
pub struct BoardGoalCurator {
    mem: Arc<dyn CognitiveMemoryOps>,
}

impl BoardGoalCurator {
    pub fn new(mem: Arc<dyn CognitiveMemoryOps>) -> Self {
        Self { mem }
    }

    fn load(&self) -> Result<GoalBoard, OverseerError> {
        load_goal_board(self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.load",
            detail: e.to_string(),
        })
    }
}

impl GoalCurator for BoardGoalCurator {
    fn propose(&self, goal: &GoalBrief) -> Result<(), OverseerError> {
        let mut board = self.load()?;
        let id = format!("overseer-{}", slugify(&goal.title));
        // Idempotent: never double-enqueue nor collide with an active goal.
        if board.backlog.iter().any(|b| b.id == id) || board.active.iter().any(|g| g.id == id) {
            return Ok(());
        }
        let item = BacklogItem {
            id,
            description: format!("{} — {}", goal.title, goal.rationale),
            // Stamp the Overseer's DISTINCT source tag so anti-recursion never
            // re-opens a goal the Overseer itself filed.
            source: format!("overseer:{}", goal.target_repo),
            score: DEFAULT_STEWARD_SCORE,
        };
        add_backlog_item(&mut board, item).map_err(|e| OverseerError::Capability {
            what: "goal_board.enqueue",
            detail: e.to_string(),
        })?;
        save_goal_board(&board, self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.save",
            detail: e.to_string(),
        })
    }

    fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
        Ok(in_flight_from_board(&self.load()?))
    }

    fn blocked_goals(&self) -> Result<Vec<BlockedGoal>, OverseerError> {
        Ok(blocked_goals_from_board(&self.load()?))
    }

    fn observe_board(&self) -> Result<(Vec<BlockedGoal>, Vec<InFlightItem>), OverseerError> {
        // Single board read projected two ways. The split
        // `blocked_goals()` + `in_flight()` path the Observe pass would
        // otherwise take loads (and JSON-deserializes) the same snapshot
        // twice per tick; project both from one `load()` instead.
        let board = self.load()?;
        Ok((
            blocked_goals_from_board(&board),
            in_flight_from_board(&board),
        ))
    }

    fn unblock(&self, goal_id: &str) -> Result<(), OverseerError> {
        // The exact `simard goal unblock` mutation: restore the blocked goal to
        // `NotStarted` so the next OODA cycle re-enters the spawn path. Reuses
        // the shipped `load_goal_board` / `save_goal_board` under the flock
        // write-lock (`save_goal_board` acquires `BoardWriteLock`).
        let mut board = self.load()?;
        let goal = board
            .active
            .iter_mut()
            .find(|g| g.id == goal_id)
            .ok_or_else(|| OverseerError::Capability {
                what: "goal_board.unblock",
                detail: format!("goal '{goal_id}' not found on the active board"),
            })?;
        goal.status = GoalProgress::NotStarted;
        save_goal_board(&board, self.mem.as_ref()).map_err(|e| OverseerError::Capability {
            what: "goal_board.save",
            detail: e.to_string(),
        })
    }

    fn workstream_gaps(&self, anomalies: &[String]) -> Result<Vec<GapItem>, OverseerError> {
        // Board survey is the durable, primary source. A board-read failure
        // degrades to no gaps (logged) — never a panic, never a fabricated gap.
        let board = match self.load() {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    target: "overseer::gap_scan",
                    error = %e,
                    "gap-scan: goal-board read failed; degrading to no gaps"
                );
                return Ok(Vec::new());
            }
        };
        // High-signal open issues + open-PR issue coverage are best-effort
        // external `gh` reads; each degrades to empty (logged) so a network hiccup
        // never aborts the scan nor invents a gap. Anomalies flow through from the
        // Observe pass; correlating a specific anomaly to an in-flight fix is left
        // to the durable per-signature dedup (M1 issue + gap gate) rather than a
        // brittle text match.
        let issues = survey_high_signal_open_issues(OVERSEER_SURVEY_REPO);
        let coverage = issue_coverage_from_open_prs(OVERSEER_SURVEY_REPO);
        Ok(detect_workstream_gaps(
            &board, &issues, anomalies, &coverage,
        ))
    }
}

/// The repo the Overseer surveys for high-signal open issues + open PRs — its own
/// stewarded repo (`rysweet/Simard`), per the gap-scan directive.
const OVERSEER_SURVEY_REPO: &str = "rysweet/Simard";

/// Upper bound on issues / PRs pulled in one survey, so the `gh` reads stay cheap
/// and the candidate set bounded regardless of backlog size.
const OVERSEER_SURVEY_LIMIT: u32 = 100;

/// Best-effort label-aware survey of OPEN issues via `gh issue list --json
/// number,title,labels`. Any failure (spawn, non-zero exit, JSON parse) degrades
/// to an empty list (logged via tracing) — no gap is ever fabricated from a failed
/// read, and there is no stray `print`. The detector filters these to the
/// high-signal, uncovered ones.
fn survey_high_signal_open_issues(repo: &str) -> Vec<SurveyedIssue> {
    let output = match std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "-R",
            repo,
            "--state",
            "open",
            "--json",
            "number,title,labels",
            "--limit",
            &OVERSEER_SURVEY_LIMIT.to_string(),
        ])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                target: "overseer::gap_scan", repo, error = %e,
                "gap-scan: `gh issue list` spawn failed; degrading to no issue gaps"
            );
            return Vec::new();
        }
    };
    if !output.status.success() {
        tracing::warn!(
            target: "overseer::gap_scan", repo,
            status = %output.status,
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "gap-scan: `gh issue list` failed; degrading to no issue gaps"
        );
        return Vec::new();
    }
    #[derive(serde::Deserialize)]
    struct RawLabel {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct RawIssue {
        number: u64,
        title: String,
        labels: Vec<RawLabel>,
    }
    match serde_json::from_slice::<Vec<RawIssue>>(&output.stdout) {
        Ok(raws) => raws
            .into_iter()
            .map(|r| SurveyedIssue {
                repo: repo.to_string(),
                number: r.number,
                title: r.title,
                labels: r.labels.into_iter().map(|l| l.name).collect(),
            })
            .collect(),
        Err(e) => {
            tracing::warn!(
                target: "overseer::gap_scan", repo, error = %e,
                "gap-scan: `gh issue list` JSON parse failed; degrading to no issue gaps"
            );
            Vec::new()
        }
    }
}

/// Build the ISSUE coverage set from the OPEN PRs: an issue with an open PR
/// referencing it is not a gap. Best-effort — a `gh pr list` failure degrades to
/// empty coverage (logged), so the worst case is flagging an already-covered issue
/// (bounded by the per-signature dedup), never a panic. Issue references are read
/// structurally: `#<n>` tokens in the PR title, and the `issue-<n>` / `issue/<n>`
/// branch-naming convention.
fn issue_coverage_from_open_prs(repo: &str) -> Vec<String> {
    use crate::stewardship::merge_authority::{PrGhClient, RealPrGhClient};
    let prs = match RealPrGhClient::new().list_open_prs(repo, OVERSEER_SURVEY_LIMIT) {
        Ok(prs) => prs,
        Err(e) => {
            tracing::warn!(
                target: "overseer::gap_scan", repo, error = %e,
                "gap-scan: `gh pr list` failed; degrading to no open-PR issue coverage"
            );
            return Vec::new();
        }
    };
    let mut coverage = Vec::new();
    for pr in &prs {
        for n in issue_refs_from_pr(&pr.title, &pr.head_ref_name) {
            coverage.push(format!("issue:{repo}#{n}"));
        }
    }
    coverage
}

/// Extract the issue numbers an open PR references, structurally: every `#<n>`
/// token in the title, plus the digits after an `issue-` / `issue/` marker in the
/// branch name (Simard's branch convention).
fn issue_refs_from_pr(title: &str, branch: &str) -> Vec<u64> {
    let mut nums = hash_issue_numbers(title);
    let lower = branch.to_ascii_lowercase();
    for marker in ["issue-", "issue/"] {
        if let Some(pos) = lower.find(marker) {
            let digits: String = lower[pos + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u64>() {
                nums.push(n);
            }
        }
    }
    nums
}

/// Collect every `#<digits>` issue reference in `text`.
fn hash_issue_numbers(text: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(pos) = rest.find('#') {
        let after = &rest[pos + 1..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u64>() {
            out.push(n);
        }
        // Advance past this `#` (and any digits) to find the next reference.
        rest = &after[digits.len()..];
    }
    out
}

/// Lowercase, hyphenate, and bound a title into a stable backlog id fragment.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(48));
    let mut last_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

// ─────────────────────────── memory recall (#2628) ─────────────────────────

/// Production [`MemoryRecall`]: bounded read access to Simard's cognitive-memory
/// graph plus one deliberate, de-duplicated episodic write-back, over the
/// **same** shared [`CognitiveMemoryOps`] handle the daemon already holds
/// (single-source — never a second store). Each method is a thin adapter onto an
/// already-shipped memory query (guideline G2: no new memory-library API), maps
/// every underlying `Err` to `OverseerError::Capability { what: "memory-recall" }`
/// (fail-closed, never an empty `Ok`), and enforces the per-kind size budgets.
pub struct MemoryRecallOps {
    mem: Arc<dyn CognitiveMemoryOps>,
}

/// The fixed provenance every Overseer write-back carries. Never caller-chosen,
/// so a hostile payload can never spoof a different author into the graph.
const OVERSEER_SOURCE_LABEL: &str = "overseer";

/// Minimum confidence for semantic recall — `0.0` keeps recall inclusive; the
/// ranked order (not a hard floor) decides relevance.
const RECALL_MIN_CONFIDENCE: f64 = 0.0;

impl MemoryRecallOps {
    pub fn new(mem: Arc<dyn CognitiveMemoryOps>) -> Self {
        Self { mem }
    }

    /// Map any backing-store error to the recall capability error. The `what`
    /// tag is fixed so telemetry and tests can key on the recall seam.
    fn cap_err(e: impl std::fmt::Display) -> OverseerError {
        OverseerError::Capability {
            what: "memory-recall",
            detail: e.to_string(),
        }
    }
}

/// Parse the `[sig:…]` marker the Overseer's own write-back embeds so a later
/// recall can recover an episode's failure signature (episodes carry no typed
/// signature field on the read path). `None` when the episode carried none.
fn parse_failure_signature(content: &str) -> Option<String> {
    let start = content.find("[sig:")? + "[sig:".len();
    let rest = &content[start..];
    let end = rest.find(']')?;
    let sig = rest[..end].trim();
    if sig.is_empty() {
        None
    } else {
        Some(sig.to_string())
    }
}

impl MemoryRecall for MemoryRecallOps {
    fn recall_semantic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledFact>, OverseerError> {
        let facts = self
            .mem
            .recall_facts_ranked(
                &keys.query(),
                limit,
                RECALL_MIN_CONFIDENCE,
                RecallWeightSet::default(),
            )
            .map_err(Self::cap_err)?;
        Ok(facts
            .into_iter()
            .map(|f| RecalledFact {
                id: f.node_id,
                content: f.content,
                score: f.confidence as f32,
            })
            .collect())
    }

    fn recall_episodic(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledEpisode>, OverseerError> {
        let episodes = self
            .mem
            .recall_episodes_ranked(&keys.query(), limit, RecallWeightSet::default())
            .map_err(Self::cap_err)?;
        Ok(episodes
            .into_iter()
            .map(|e| RecalledEpisode {
                failure_signature: parse_failure_signature(&e.content),
                id: e.node_id,
                summary: e.content,
                score: 0.0,
            })
            .collect())
    }

    fn recall_procedural(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProcedure>, OverseerError> {
        let procs = self
            .mem
            .recall_procedure(&keys.query(), limit)
            .map_err(Self::cap_err)?;
        Ok(procs
            .into_iter()
            .map(|p| RecalledProcedure {
                content: if p.steps.is_empty() {
                    p.name.clone()
                } else {
                    format!("{}: {}", p.name, p.steps.join(" → "))
                },
                id: p.node_id,
            })
            .collect())
    }

    fn recall_prospective(
        &self,
        keys: &RecallKeys,
        limit: u32,
    ) -> Result<Vec<RecalledProspective>, OverseerError> {
        // `check_triggers` takes a single `&str`, so join the keys into one
        // deterministic probe rather than fanning out per key.
        let hits = self
            .mem
            .check_triggers(&keys.query())
            .map_err(Self::cap_err)?;
        Ok(hits
            .into_iter()
            .take(limit as usize)
            .map(|p| RecalledProspective {
                id: p.node_id,
                content: p.description,
            })
            .collect())
    }

    fn record_observation(
        &self,
        episode: &ObservationEpisode,
    ) -> Result<RecordOutcome, OverseerError> {
        // Embed the signature marker so a later recall can recover it, and carry
        // a typed metadata copy. Provenance is FIXED (`source_label` is never
        // caller-chosen), and the metadata is a validated JSON object carrying
        // only the signature — no secrets, tokens, or env.
        let content = format!("{} [sig:{}]", episode.content, episode.signature);
        let metadata = serde_json::json!({ "signature": episode.signature });
        let node_id = self
            .mem
            .store_episode(&content, OVERSEER_SOURCE_LABEL, Some(&metadata))
            .map_err(Self::cap_err)?;
        Ok(RecordOutcome::Stored { node_id })
    }
}

// ─────────────────────────── deployer (safe stub) ──────────────────────────

/// A [`Deployer`] that REFUSES autonomous deploys. The deterministic Decide
/// mapping the wired daemon uses never emits `Intervention::Deploy`, so a live
/// deploy is never planned through this path; if one ever were, it escalates
/// rather than blindly swapping the binary. Guarded self-deploy stays the
/// operator's canary/self-deploy gates (unit-tested via `GuardedDeployer` and
/// `evaluate_deploy_gate`), never a blind deploy from the acting loop.
#[derive(Clone, Copy, Debug, Default)]
pub struct RefuseDeployer;

impl Deployer for RefuseDeployer {
    fn deploy(&self, _commit: &str) -> Result<DeployReport, OverseerError> {
        Err(OverseerError::Capability {
            what: "deploy",
            detail: "autonomous deploy is not wired into the acting loop — escalate to the \
                     operator's canary/self-deploy gates (never a blind deploy)"
                .to_string(),
        })
    }

    fn deployed_commit(&self) -> Result<String, OverseerError> {
        Ok(GuardedDeployer::running_commit_marker().to_string())
    }
}

// ─────────────────────────── assembly ──────────────────────────────────────

/// Assemble the production [`Capabilities`] from the already-shipped adapters.
/// Every handle is a thin reuse of an existing Simard subsystem; the merge path
/// carries the Overseer's DISTINCT anti-recursion identity so it can never merge
/// its OWN PR.
pub fn assemble_capabilities(
    mem: Arc<dyn CognitiveMemoryOps>,
    repo_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
) -> Capabilities {
    Capabilities {
        status: Box::new(SnapshotStatusReader::from_env()),
        recipes: Box::new(SmartOrchestratorLauncher::from_env()),
        prs: Box::new(MergePrOps::from_env().with_recursion_guard(overseer_identity())),
        deployer: Box::new(RefuseDeployer),
        meetings: Box::new(MeetingGoalTransfer::from_env()),
        issues: Box::new(StewardshipIssueFiler::new(Arc::new(
            crate::stewardship::RealGhClient,
        ))),
        // The goal curator and the memory-recall seam SHARE the one
        // `Arc<dyn CognitiveMemoryOps>` handle (single-source; no second store).
        goals: Box::new(BoardGoalCurator::new(Arc::clone(&mem))),
        auditor: Box::new(SelfQualityAuditor::from_env(repo_root, state_root)),
        memory: Box::new(MemoryRecallOps::new(mem)),
    }
}

/// Build the production acting [`Overseer`]: the assembled capabilities, the
/// DISTINCT anti-recursion identity, and acting autonomy ON (verify-merge +
/// HIGH-RISK). The merge path still only merges GREEN, merge-ready PRs through
/// the gated authority (never `--admin`/`--no-verify`) and notifies the operator
/// on every merge; the review gate is fail-closed until an operator wires a
/// reviewer, so nothing is merged unreviewed.
pub fn build_overseer(
    mem: Arc<dyn CognitiveMemoryOps>,
    repo_root: std::path::PathBuf,
    state_root: std::path::PathBuf,
) -> Overseer {
    let overseer = Overseer::new(assemble_capabilities(
        Arc::clone(&mem),
        repo_root,
        state_root.clone(),
    ))
    .with_verify_merge_autonomy(true)
    .with_high_risk_autonomy(true)
    .with_identity(overseer_identity())
    // The Simard Whisperer: advisory steering notes onto the SAME
    // meeting-handoff inbox the OODA observe step scans. Enabled by default
    // (opt-out via SIMARD_OVERSEER_WHISPER), consistent with the acting
    // Overseer's opt-out gate.
    .with_whisper_enabled(whisper_enabled())
    .with_whisper_sink(Box::new(
        crate::overseer::whisper_ops::MeetingHandoffWhisperSink::new(
            crate::meeting_facilitator::default_handoff_dir(),
        ),
    ))
    // Goal-board health: self-heal false-parked perpetual goals and escalate
    // genuine "needs human review" blocks to the operator on BOTH channels
    // (email + Signal) via the SAME mandatory notifier the merge path uses.
    // Enabled by default (opt-out via SIMARD_OVERSEER_GOAL_HEALTH).
    .with_goal_health_enabled(goal_health_enabled())
    .with_operator_notifier(Box::new(DualChannelNotifier::from_env()))
    // Cognitive-memory recall (#2628): the Overseer reads Simard's memory
    // graph in Observe/Orient and writes its observation back — over the
    // SAME shared handle assembled above. Enabled by default (opt-out via
    // SIMARD_OVERSEER_MEMORY_RECALL); a disabled Overseer forces it off.
    .with_memory_recall_enabled(memory_recall_enabled())
    // The recurring backlog-coverage gap-scan: each tick, survey the whole
    // work picture and flag important work with no active workstream, then
    // notify the operator (email + Signal). Enabled by default
    // (opt-out via SIMARD_OVERSEER_GAP_SCAN); its every-N cadence is applied
    // by the daemon tick loop.
    .with_gap_scan_enabled(gap_scan_enabled())
    // Root-cause recall/store (issue #2635, G2): the SAME cognitive-memory
    // handle the goal board + recall seam read through, so the Overseer recalls
    // prior occurrences of a problem's root cause and records new ones — turning
    // a one-off false-park into a detected recurring root cause it escalates
    // instead of re-patching.
    .with_memory(mem);

    // Periodic stale-engineer-claim reaper (issue #4099): sweep + reclaim the
    // `engineer_claims` leak independent of per-goal polling. Wire the shared
    // ledger chokepoint, the worktree liveness probe, and the orphan cleanup —
    // all rooted at the SAME `state_root` the engineers spawn under. Opening the
    // ledger is fail-visible: if it fails the reaper is simply not wired this
    // tick (the sweep is skipped), never a panic that would abort the tick.
    match build_claim_reaper_seams(&state_root) {
        Some((ledger, probe, cleanup)) => overseer.with_claim_reaper(
            ledger,
            probe,
            cleanup,
            claim_reap_enabled(),
            claim_reap_stale_secs(),
        ),
        None => overseer,
    }
}

/// Open the reaper's ledger + build its worktree-rooted probe and cleanup seams.
/// Returns `None` (fail-visible log) if the ledger cannot be opened so the tick
/// runs without the reaper rather than panicking.
fn build_claim_reaper_seams(
    state_root: &std::path::Path,
) -> Option<crate::overseer::claim_reaper::ClaimReaperSeamSet> {
    let ledger_path = crate::typed_ooda::ledger_path(state_root);
    let policy = crate::typed_ooda::CapabilityPolicy::new("engineer-claim-reaper");
    let handler = match crate::typed_ooda::CapabilityHandler::open(&ledger_path, policy) {
        Ok(handler) => handler,
        Err(error) => {
            tracing::error!(
                target: "simard::claim_reaper",
                error = %error,
                ledger_path = %ledger_path.display(),
                "[simard] claim-reaper NOT wired this tick: failed to open ledger",
            );
            return None;
        }
    };
    Some((
        Box::new(handler),
        Box::new(
            crate::overseer::claim_reaper::WorktreeClaimLivenessProbe::new(
                state_root.to_path_buf(),
            ),
        ),
        Box::new(crate::overseer::claim_reaper::WorktreeDirCleanup::new(
            state_root.to_path_buf(),
        )),
    ))
}

/// Resolve the acting Overseer's tick cadence (seconds), clamped to the config
/// floor so self-tuning can never drive a hot loop.
pub fn overseer_tick_interval_secs() -> u64 {
    overseer_interval_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::capabilities::{
        AuditReport, AuditScope, Auditor, CheckItem, IssueOutcome, ObservedState,
        OrchestratorRunBrief, PrOps, RecipeBrief, RecipeLauncher, StatusReader, VerifyReport,
        WorkstreamHandle, WorkstreamStatus,
    };
    use crate::overseer::intervention::Intervention;
    use std::sync::Mutex;

    // ── cadence ───────────────────────────────────────────────────────────

    #[test]
    fn cadence_fires_only_after_the_interval_elapses() {
        // Injected virtual clock (seconds). Interval 900s, started at t=0.
        let mut cadence = OverseerCadence::new(900, 0);
        assert!(!cadence.due(1), "1s in — far below interval, no tick");
        assert!(!cadence.due(899), "just under the interval — no tick");
        assert!(cadence.due(900), "exactly at the interval — tick fires");
        assert!(!cadence.due(901), "immediately after a tick — no re-fire");
        assert!(!cadence.due(1799), "still within the next window — no tick");
        assert!(cadence.due(1800), "one more interval elapsed — tick fires");
    }

    #[test]
    fn cadence_interval_is_floored_to_avoid_a_hot_loop() {
        let mut cadence = OverseerCadence::new(0, 0);
        assert_eq!(cadence.interval_secs(), 1);
        assert!(!cadence.due(0));
        assert!(cadence.due(1));
    }

    #[test]
    fn cadence_never_fires_when_the_clock_goes_backwards() {
        let mut cadence = OverseerCadence::new(60, 100);
        assert!(!cadence.due(50), "clock moved backwards — never fires");
        assert!(cadence.due(160), "forward past the interval — fires");
    }

    // ── fakes for the tick driver ─────────────────────────────────────────

    struct FakeStatus(ObservedState);
    impl StatusReader for FakeStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            Ok(self.0.clone())
        }
    }

    /// A status reader that panics — used to prove panic isolation.
    struct PanicStatus;
    impl StatusReader for PanicStatus {
        fn snapshot(&self) -> Result<ObservedState, OverseerError> {
            panic!("boom: capability adapter blew up mid-observe");
        }
    }

    struct FakeRecipes;
    impl RecipeLauncher for FakeRecipes {
        fn launch(&self, _b: &RecipeBrief) -> Result<WorkstreamHandle, OverseerError> {
            Ok(WorkstreamHandle {
                id: "ws-1".to_string(),
            })
        }
        fn poll(&self, _h: &WorkstreamHandle) -> Result<WorkstreamStatus, OverseerError> {
            Ok(WorkstreamStatus::Running)
        }
    }

    type MergeLog = Arc<Mutex<Vec<(String, u32)>>>;

    /// Records every `merge` call and returns `ready` from `verify`, so a tick
    /// that drives a green PR to merge is observable without any network. The
    /// merge log is a shared handle so the test can inspect it after the
    /// recorder is boxed into the Overseer — no `unsafe`.
    struct RecordingPrs {
        ready: bool,
        merges: MergeLog,
        /// Candidates the survey rail reports (#4097). In production `ready_prs`
        /// is populated by `PrOps::survey_ready_prs`, not the status snapshot, so
        /// tests seed the merge path HERE rather than via `FakeStatus`.
        ready_prs: Vec<crate::overseer::capabilities::PrRef>,
    }
    impl RecordingPrs {
        fn new(ready: bool) -> (Self, MergeLog) {
            let merges: MergeLog = Arc::new(Mutex::new(vec![]));
            (
                Self {
                    ready,
                    merges: Arc::clone(&merges),
                    ready_prs: Vec::new(),
                },
                merges,
            )
        }

        /// Seed the candidates the survey rail will report this tick.
        fn with_ready_prs(mut self, ready_prs: Vec<crate::overseer::capabilities::PrRef>) -> Self {
            self.ready_prs = ready_prs;
            self
        }
    }
    impl PrOps for RecordingPrs {
        fn verify(&self, _repo: &str, _pr: u32) -> Result<VerifyReport, OverseerError> {
            Ok(VerifyReport {
                ready: self.ready,
                checks: vec![CheckItem {
                    name: "objective gates".to_string(),
                    passed: self.ready,
                    note: "test".to_string(),
                }],
            })
        }
        fn merge(&self, repo: &str, pr: u32) -> Result<(), OverseerError> {
            self.merges.lock().unwrap().push((repo.to_string(), pr));
            Ok(())
        }
        fn resolve_conflict(&self, _repo: &str, _pr: u32) -> Result<(), OverseerError> {
            Ok(())
        }
        fn survey_ready_prs(&self, _repos: &[String]) -> Vec<crate::overseer::capabilities::PrRef> {
            self.ready_prs.clone()
        }
    }

    struct FakeDeployer;
    impl Deployer for FakeDeployer {
        fn deploy(&self, commit: &str) -> Result<DeployReport, OverseerError> {
            Ok(DeployReport {
                deployed_commit: commit.to_string(),
                gates_passed: true,
            })
        }
        fn deployed_commit(&self) -> Result<String, OverseerError> {
            Ok("deadbeef".to_string())
        }
    }

    struct FakeMeetings;
    impl crate::overseer::capabilities::MeetingHost for FakeMeetings {
        fn transfer_goal(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
            Ok(())
        }
    }

    struct FakeIssues;
    impl crate::overseer::capabilities::IssueFiler for FakeIssues {
        fn file(&self, _run: &OrchestratorRunBrief) -> Result<IssueOutcome, OverseerError> {
            Ok(IssueOutcome::FiledNew {
                url: "https://example/issues/1".to_string(),
            })
        }
    }

    struct FakeGoals(Vec<InFlightItem>);
    impl GoalCurator for FakeGoals {
        fn propose(&self, _g: &GoalBrief) -> Result<(), OverseerError> {
            Ok(())
        }
        fn in_flight(&self) -> Result<Vec<InFlightItem>, OverseerError> {
            Ok(self.0.clone())
        }
    }

    struct FakeAuditor;
    impl Auditor for FakeAuditor {
        fn run_audit(&self, scope: &AuditScope) -> Result<AuditReport, OverseerError> {
            Ok(AuditReport {
                scope: scope.clone(),
                passed: true,
                findings: vec![],
            })
        }
    }

    fn caps_with(status: Box<dyn StatusReader>, prs: Box<dyn PrOps>) -> Capabilities {
        Capabilities {
            status,
            recipes: Box::new(FakeRecipes),
            prs,
            deployer: Box::new(FakeDeployer),
            meetings: Box::new(FakeMeetings),
            issues: Box::new(FakeIssues),
            goals: Box::new(FakeGoals(vec![])),
            auditor: Box::new(FakeAuditor),
            memory: Box::new(crate::overseer::capabilities::InertMemoryRecall),
        }
    }

    // ── tick driver ───────────────────────────────────────────────────────

    #[test]
    fn tick_drives_run_cycle_then_launches_a_fix_for_a_process_health_signal() {
        // A high distill-failure rate raises one ProcessHealth problem → Decide
        // maps it to a LaunchRecipe → the tick executes it via `act`.
        let observed = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.problems, 1);
        assert_eq!(
            report.recipes_launched, 1,
            "the tick must ACT, not just plan"
        );
        assert_eq!(report.errors, 0);
        assert!(!report.panicked);
    }

    #[test]
    fn tick_verifies_and_merges_a_green_ready_pr_and_records_the_merge() {
        // Seed a merge-ready PR signal → Decide maps to VerifyAndMergePr →
        // with verify-merge autonomy ON, the tick merges it (normal merge).
        // `ready_prs` is sourced from the survey rail (#4097), not the snapshot.
        let (prs, merges) = RecordingPrs::new(true);
        let prs = prs.with_ready_prs(vec![crate::overseer::capabilities::PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 42,
        }]);
        let mut overseer = Overseer::new(caps_with(
            Box::new(FakeStatus(ObservedState::default())),
            Box::new(prs),
        ))
        .with_verify_merge_autonomy(true);
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.prs_merged, 1, "green ready PR must be merged");
        assert_eq!(
            *merges.lock().unwrap(),
            vec![("rysweet/Simard".to_string(), 42)]
        );
    }

    #[test]
    fn tick_holds_the_merge_when_autonomy_is_not_opted_in() {
        let (prs, merges) = RecordingPrs::new(true);
        let prs = prs.with_ready_prs(vec![crate::overseer::capabilities::PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 7,
        }]);
        // Default autonomy (verify-merge OFF) → the merge is HELD (escalated).
        let mut overseer = Overseer::new(caps_with(
            Box::new(FakeStatus(ObservedState::default())),
            Box::new(prs),
        ));
        let report = overseer_tick(&mut overseer);
        assert_eq!(report.prs_merged, 0);
        assert_eq!(report.held, 1, "verify-merge is opt-in; held by default");
        assert!(
            merges.lock().unwrap().is_empty(),
            "nothing merged when held"
        );
    }

    // ── panic isolation ───────────────────────────────────────────────────

    #[test]
    fn a_panicking_tick_is_isolated_and_the_overseer_survives() {
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(PanicStatus), Box::new(prs)));
        // The panic inside `snapshot` must be caught, not propagated.
        let report = run_overseer_tick_isolated(&mut overseer);
        assert!(report.panicked, "the tick panicked and was isolated");
        assert_eq!(report.prs_merged, 0);
        // The overseer is still usable — a second isolated tick also survives.
        let report2 = run_overseer_tick_isolated(&mut overseer);
        assert!(report2.panicked);
    }

    #[test]
    fn a_run_cycle_error_is_isolated_without_panicking() {
        struct ErrStatus;
        impl StatusReader for ErrStatus {
            fn snapshot(&self) -> Result<ObservedState, OverseerError> {
                Err(OverseerError::Capability {
                    what: "status",
                    detail: "degraded".to_string(),
                })
            }
        }
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(ErrStatus), Box::new(prs)));
        let report = overseer_tick(&mut overseer);
        assert!(!report.panicked, "an error is not a panic");
        assert_eq!(report.errors, 1);
        assert_eq!(report.problems, 0);
    }

    // ── identity ──────────────────────────────────────────────────────────

    #[test]
    fn overseer_identity_is_configured_and_refuses_its_own_pr() {
        let guard = overseer_identity();
        assert!(guard.is_configured(), "identity must be fully populated");
        let own = crate::overseer::guardrails::Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 1,
            author: guard.author_login.clone(),
        };
        assert!(guard.admit(&own).is_err(), "must refuse its OWN PR");
        let foreign = crate::overseer::guardrails::Subject::Pr {
            repo: "rysweet/Simard".to_string(),
            pr: 2,
            author: "a-human-operator".to_string(),
        };
        assert!(guard.admit(&foreign).is_ok(), "must admit a human's PR");
    }

    #[test]
    fn slugify_is_stable_and_bounded() {
        assert_eq!(slugify("Fix the OODA loop!"), "fix-the-ooda-loop");
        assert_eq!(slugify("  spaced  "), "spaced");
        assert!(slugify(&"x".repeat(200)).len() <= 48);
    }

    #[test]
    fn refuse_deployer_never_deploys_but_reports_running_commit() {
        let d = RefuseDeployer;
        assert!(d.deploy("abc123").is_err(), "never a blind deploy");
        assert!(d.deployed_commit().is_ok());
    }

    #[test]
    fn intervention_labels_are_stable_for_tracing() {
        // Guard the labels the tracing/tally path depends on.
        assert_eq!(
            Intervention::Report.label(),
            "report",
            "label churn would break tracing dashboards"
        );
    }

    // ── issue #21: informative detail strings ─────────────────────────────
    //
    // The activity log must say WHAT was observed (concrete signal values) and
    // WHAT was done (concrete actions + outcomes), not bare counts. These tests
    // pin `sanitize_detail`, the `describe_*` renderers, and the detail-vec
    // population inside `overseer_tick` (bounded, with a `(+N more)` sentinel).

    use crate::overseer::capabilities::OverseerError;
    use crate::overseer::intervention::PlannedIntervention;
    use crate::overseer::signal::DETAIL_STR_CAP;

    #[test]
    fn sanitize_detail_strips_ansi_and_c0_controls() {
        let dirty = "\u{1b}[1;31mred\u{1b}[0m\tline\nbreak";
        let clean = sanitize_detail(dirty);
        assert!(
            !clean.contains('\u{1b}'),
            "ANSI escape bytes must be stripped: {clean:?}"
        );
        assert!(
            !clean.contains('\n') && !clean.contains('\t'),
            "C0 controls (newline/tab) must be collapsed to spaces: {clean:?}"
        );
        assert!(clean.contains("red"), "benign text must survive: {clean:?}");
        assert!(
            clean.contains("line"),
            "benign text must survive: {clean:?}"
        );
    }

    #[test]
    fn sanitize_detail_redacts_token_shaped_secrets() {
        let ghp = sanitize_detail("pushed with ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00");
        assert!(
            !ghp.contains("ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00"),
            "a GitHub token must be redacted before it is persisted/rendered: {ghp:?}"
        );
        let bearer = sanitize_detail("leaked credential blob EXAMPLEfakebearertokenDONOTUSE00");
        assert!(
            !bearer.contains("EXAMPLEfakebearertokenDONOTUSE00"),
            "a Bearer token must be redacted: {bearer:?}"
        );
    }

    #[test]
    fn sanitize_detail_truncates_to_the_cap_with_an_ellipsis() {
        let long = "x".repeat(DETAIL_STR_CAP * 3);
        let out = sanitize_detail(&long);
        // Bounded well within a small multiple of the cap and marked truncated.
        assert!(
            out.chars().count() <= DETAIL_STR_CAP + 1,
            "a detail line must be truncated to DETAIL_STR_CAP: got {} chars",
            out.chars().count()
        );
        assert!(
            out.ends_with('…'),
            "a truncated detail must end with an ellipsis marker: {out:?}"
        );
    }

    #[test]
    fn describe_action_merge_names_repo_and_pr() {
        let s = describe_action(
            &Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 123,
            },
            &ActOutcome::Merged,
        );
        assert!(
            s.starts_with("did:"),
            "action lines self-prefix with 'did:': {s:?}"
        );
        assert!(s.contains("rysweet/Simard"), "must name the repo: {s:?}");
        assert!(s.contains("123"), "must name the PR number: {s:?}");
        assert!(
            s.to_lowercase().contains("merg"),
            "must say it merged: {s:?}"
        );
    }

    #[test]
    fn describe_action_issue_carries_the_url() {
        let s = describe_action(
            &Intervention::FileIssue {
                run: crate::overseer::capabilities::OrchestratorRunBrief {
                    recipe_name: "smart-orchestrator".to_string(),
                    failed_step: "build".to_string(),
                    source_module: "overseer".to_string(),
                    failure_kind: "compile".to_string(),
                    error_text: "boom".to_string(),
                },
            },
            &ActOutcome::IssueFiled(IssueOutcome::FiledNew {
                url: "https://github.com/rysweet/Simard/issues/321".to_string(),
            }),
        );
        assert!(
            s.contains("https://github.com/rysweet/Simard/issues/321"),
            "a filed issue must surface its URL so operators can click through: {s:?}"
        );
    }

    #[test]
    fn describe_action_launch_names_the_workstream() {
        let s = describe_action(
            &Intervention::LaunchRecipe {
                brief: RecipeBrief {
                    task_description: "fix distill".to_string(),
                    target_repo: "rysweet/Simard".to_string(),
                    sequence_group: None,
                },
            },
            &ActOutcome::Launched(WorkstreamHandle {
                id: "ws-77".to_string(),
            }),
        );
        assert!(
            s.to_lowercase().contains("launch"),
            "must say launched: {s:?}"
        );
        assert!(
            s.contains("ws-77"),
            "must name the workstream handle: {s:?}"
        );
    }

    #[test]
    fn describe_action_escalation_carries_the_reason() {
        let s = describe_action(
            &Intervention::Escalate {
                reason: "high-risk deploy needs a human".to_string(),
            },
            &ActOutcome::Escalated,
        );
        assert!(
            s.to_lowercase().contains("escalat"),
            "must say escalated: {s:?}"
        );
        assert!(
            s.contains("high-risk deploy needs a human"),
            "must carry WHY it escalated: {s:?}"
        );
    }

    #[test]
    fn describe_action_goal_unblock_names_the_goal() {
        let s = describe_action(
            &Intervention::UnblockGoal {
                goal_id: "g-9".to_string(),
                reason: "false-parked perpetual goal".to_string(),
            },
            &ActOutcome::GoalUnblocked {
                goal_id: "g-9".to_string(),
            },
        );
        assert!(
            s.contains("g-9"),
            "must name the reactivated goal id: {s:?}"
        );
    }

    #[test]
    fn describe_hold_states_what_was_held_and_why() {
        let held = describe_hold(&PlannedIntervention {
            intervention: Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 7,
            },
            admitted: false,
            note: "verify-merge is opt-in; escalated to operator".to_string(),
            remediation: crate::overseer::intervention::Remediation::root_cause(),
        });
        assert!(
            held.starts_with("held:"),
            "held lines self-prefix with 'held:': {held:?}"
        );
        assert!(
            held.contains("rysweet/Simard"),
            "must name the target repo: {held:?}"
        );
        assert!(held.contains('7'), "must name the target PR: {held:?}");
        assert!(
            held.contains("verify-merge is opt-in; escalated to operator"),
            "must carry the gate reason so 'no action' is explained: {held:?}"
        );
    }

    #[test]
    fn describe_act_error_handles_not_merge_ready_in_plain_english() {
        // #4097: NotMergeReady normally maps to an escalation (handled in act()),
        // but describe_act_error's classifier must exhaustively cover it and
        // render a plain-English, jargon-free, target-named line.
        let err = OverseerError::NotMergeReady {
            pr: 4097,
            reason: "the merge-readiness review did not approve this change yet".to_string(),
        };
        let s = describe_act_error(
            &Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 4097,
            },
            &err,
        );
        assert!(
            s.contains("4097"),
            "an act line must name the target PR: {s:?}"
        );
        assert!(
            !s.contains("check #7") && !s.contains("DiffReviewer"),
            "no internal gate jargon may leak into the operator feed: {s:?}"
        );
    }

    #[test]
    fn describe_act_error_is_classified_and_never_leaks_the_raw_body() {
        let err = OverseerError::Capability {
            what: "merge",
            detail: "remote said: token ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00 invalid".to_string(),
        };
        let s = describe_act_error(
            &Intervention::VerifyAndMergePr {
                repo: "rysweet/Simard".to_string(),
                pr: 5,
            },
            &err,
        );
        assert!(
            s.to_lowercase().contains("merge"),
            "an act error must be classified by capability/kind: {s:?}"
        );
        assert!(
            !s.contains("ghp_EXAMPLE_FAKE_TOKEN_do_not_use_00"),
            "a raw error body may carry secrets and must be redacted: {s:?}"
        );
        assert!(
            s.to_lowercase().contains("fail"),
            "an act error line must read as a failure: {s:?}"
        );
    }

    #[test]
    fn tick_records_observed_details_with_concrete_values() {
        // A high distill-failure rate must surface as a concrete observed line.
        let observed = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);

        assert_eq!(report.problems, 1, "the distill signal raises one problem");
        assert!(
            !report.observed_details.is_empty(),
            "an observed problem must produce a human-readable detail line, not just a count"
        );
        let joined = report.observed_details.join(" | ");
        assert!(
            joined.contains("62"),
            "the observed detail must carry the concrete distill_fail_pct: {joined:?}"
        );
    }

    #[test]
    fn tick_records_action_details_for_a_launched_workstream() {
        let observed = ObservedState {
            distill_fail_pct: Some(62.0),
            ..ObservedState::default()
        };
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);

        assert_eq!(report.recipes_launched, 1, "the tick launches a fix");
        let joined = report.action_details.join(" | ");
        assert!(
            joined.to_lowercase().contains("launch"),
            "a launched workstream must be described as an action taken: {joined:?}"
        );
        assert!(
            joined.contains("ws-1"),
            "the action detail must name the concrete workstream handle: {joined:?}"
        );
    }

    #[test]
    fn tick_records_action_details_for_a_merged_pr() {
        let (prs, _merges) = RecordingPrs::new(true);
        let prs = prs.with_ready_prs(vec![crate::overseer::capabilities::PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 42,
        }]);
        let mut overseer = Overseer::new(caps_with(
            Box::new(FakeStatus(ObservedState::default())),
            Box::new(prs),
        ))
        .with_verify_merge_autonomy(true);
        let report = overseer_tick(&mut overseer);

        assert_eq!(report.prs_merged, 1);
        let joined = report.action_details.join(" | ");
        assert!(
            joined.contains("rysweet/Simard") && joined.contains("42"),
            "a merged PR must be named concretely in the action details: {joined:?}"
        );
        assert!(
            joined.to_lowercase().contains("merg"),
            "the action detail must state the merge outcome: {joined:?}"
        );
    }

    #[test]
    fn tick_records_why_a_held_intervention_took_no_action() {
        // Default autonomy → the merge is HELD; the log must SAY WHY, not go silent.
        let (prs, _merges) = RecordingPrs::new(true);
        let prs = prs.with_ready_prs(vec![crate::overseer::capabilities::PrRef {
            repo: "rysweet/Simard".to_string(),
            pr: 7,
        }]);
        let mut overseer = Overseer::new(caps_with(
            Box::new(FakeStatus(ObservedState::default())),
            Box::new(prs),
        ));
        let report = overseer_tick(&mut overseer);

        assert_eq!(report.held, 1);
        let joined = report.action_details.join(" | ");
        assert!(
            joined.to_lowercase().contains("held"),
            "a held intervention must appear in the details as held, not vanish: {joined:?}"
        );
        assert!(
            joined.contains("rysweet/Simard") && joined.contains('7'),
            "the held line must name the concrete PR it declined to act on: {joined:?}"
        );
    }

    #[test]
    fn observed_details_are_bounded_with_a_plus_n_more_sentinel() {
        // Far more distinct problems than the cap → the vec is bounded and the
        // overflow is summarised, never rendered unbounded.
        let anomalies: Vec<String> = (0..(DETAIL_CAP + 12))
            .map(|i| format!("anomaly-{i}"))
            .collect();
        let observed = ObservedState {
            anomalies,
            ..ObservedState::default()
        };
        let (prs, _merges) = RecordingPrs::new(true);
        let mut overseer = Overseer::new(caps_with(Box::new(FakeStatus(observed)), Box::new(prs)));
        let report = overseer_tick(&mut overseer);

        assert!(
            report.observed_details.len() <= DETAIL_CAP + 1,
            "observed_details must be capped at DETAIL_CAP (+1 for the sentinel): got {}",
            report.observed_details.len()
        );
        let last = report
            .observed_details
            .last()
            .expect("capped detail list is non-empty");
        assert!(
            last.to_lowercase().contains("more"),
            "the final capped entry must summarise the overflow (e.g. '(+N more)'): {last:?}"
        );
    }
}
