//! Fix 3 wiring: apply the no-progress breaker in the OODA curate phase.
//!
//! The pure policy lives in
//! [`crate::goal_curation::no_progress_breaker`]; this module is the thin,
//! side-effecting adapter that the OODA cycle calls each round. It turns the
//! breaker's [`NoProgressResolution`] into concrete board mutations and a
//! `gh`-filed tracking issue, mirroring the brain-*failure* safeguard in
//! [`crate::ooda_actions::advance_goal`] but for the *no-action* livelock.
//!
//! Kept in `ooda_loop` (the goal-selection / curate path) rather than the
//! reasoners, per the incident's coordination constraint (the
//! `ooda_brain`/reasoner/memory files are owned by the naming-cleanup rename).
//!
//! See `docs/concepts/steerable-ooda-daemon.md` ("The no-progress breaker
//! (Fix 3)").

use std::collections::HashSet;

use crate::error::SimardResult;
use crate::goal_curation::completion_gate::{
    CompletionEvidenceGate, CompletionVerdict, DependencyState, EvidenceSource,
};
use crate::goal_curation::no_progress_breaker::{
    NO_PROGRESS_BREAKER_THRESHOLD, NoProgressResolution, NoProgressTracker,
    SURFACED_INVESTIGATION_FAILURE_LIMIT, needs_reinvestigation,
    no_progress_blocked_reason_with_why, obsolescence_reason, resolution_for_why,
    surfaced_failure_escalation_issue, verify_stuck_goal,
};
use crate::goal_curation::no_progress_why::{
    Evidence, NoProgressClass, NoProgressWhy, NoProgressWhyReasoner,
};
use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress, WipRef};
use crate::ooda_actions::outcome_made_no_progress;
use crate::ooda_loop::{ActionOutcome, OodaState};

/// `WipRef.kind` marking a breaker-authored defer annotation (issue #16).
const DEPENDENCY_WIP_KIND: &str = "dependency";

/// Label prefix on the breaker-authored defer [`WipRef`] so the auto-clear pass
/// recognises *its own* defer and never resumes an operator-set hold.
const NO_PROGRESS_DEFER_LABEL_PREFIX: &str = "[no-progress-defer] ";

/// True when `wip` is the breaker-authored upstream-defer annotation (issue #16).
fn is_breaker_defer_ref(wip: &WipRef) -> bool {
    wip.kind.eq_ignore_ascii_case(DEPENDENCY_WIP_KIND)
        && wip.label.starts_with(NO_PROGRESS_DEFER_LABEL_PREFIX)
}

/// Label prefix on the breaker-authored tracking-issue [`WipRef`] the escalation
/// path links back to a stuck goal. Two jobs:
///
/// * **Makes the done-criteria measurable.** A stalled goal with *no* tracked
///   PR/issue has structurally unmeasurable done-criteria — `UNCLEAR-CRITERIA`
///   ("no tracked PR/issue the done-gate can verify"). Linking the filed
///   tracking issue gives the goal a derivable signal
///   ([`crate::goal_curation::completion_gate::has_derivable_signal`]) the
///   done-gate can finally observe (`CLOSED`), turning the WHY that stranded the
///   synthetic `simard-identity-*` goals into a checkable criterion.
/// * **Idempotence.** A goal already carrying its breaker tracking issue is
///   never re-filed, so a re-stall can never spam duplicate `ooda-stuck` issues.
const NO_PROGRESS_TRACKING_LABEL_PREFIX: &str = "[no-progress-tracking] ";

/// True when `wip` is a breaker-authored tracking-issue link (the escalation
/// artifact authored by [`link_tracking_issue`]).
fn is_breaker_tracking_ref(wip: &WipRef) -> bool {
    wip.kind.eq_ignore_ascii_case("issue")
        && wip.label.starts_with(NO_PROGRESS_TRACKING_LABEL_PREFIX)
}

/// A tracking issue the breaker successfully filed for an escalated goal.
///
/// Returned by [`NoProgressIssueFiler::file_issue`] so the caller can link the
/// issue back to the goal as a tracked artifact ([`link_tracking_issue`]) — the
/// step that converts an `UNCLEAR-CRITERIA` goal's *structurally unmeasurable*
/// done-criteria ("no tracked PR/issue the done-gate can verify") into a
/// machine-verifiable signal the done-gate can observe as `CLOSED`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FiledIssue {
    /// The issue number without a leading `#`, e.g. `"4231"`.
    pub number: String,
    /// The issue URL when the filer could resolve one.
    pub url: Option<String>,
}

/// Files a tracking issue for a goal the breaker escalated. Injected so tests
/// exercise the escalation path without shelling out to `gh`.
pub(crate) trait NoProgressIssueFiler {
    /// File (or attempt to file) a tracking issue. Returns the filed issue's
    /// reference on success so the caller can [`link_tracking_issue`] it back to
    /// the goal (making the done-criteria measurable); returns `None` when
    /// filing failed or the issue number could not be resolved.
    ///
    /// Failures must be logged, not propagated: the goal is already Blocked with
    /// the sentinel, and a missing issue must never abort the cycle. Returning
    /// `None` simply means the goal stays Blocked without a linked artifact
    /// (no worse than before this linkage existed).
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue>;
}

/// Production filer: `gh issue create --label ooda-stuck`, mirroring the
/// brain-failure safeguard in `ooda_actions::advance_goal::spawn`.
pub(crate) struct GhIssueFiler;

impl NoProgressIssueFiler for GhIssueFiler {
    fn file_issue(&self, title: &str, body: &str) -> Option<FiledIssue> {
        match std::process::Command::new("gh")
            .args([
                "issue",
                "create",
                "--title",
                title,
                "--body",
                body,
                "--label",
                "ooda-stuck",
            ])
            .output()
        {
            Ok(out) if out.status.success() => {
                // `gh issue create` prints the created issue's URL on success,
                // e.g. `https://github.com/rysweet/Simard/issues/4231`.
                let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let number = parse_issue_number(&url);
                tracing::warn!(
                    target: "simard::ooda",
                    title = %title,
                    issue = number.as_deref().unwrap_or("?"),
                    "no-progress breaker: tracking issue filed for stuck goal",
                );
                number.map(|number| FiledIssue {
                    url: (!url.is_empty()).then(|| url.clone()),
                    number,
                })
            }
            Ok(out) => {
                tracing::error!(
                    target: "simard::ooda",
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "no-progress breaker: gh issue create failed (goal still Blocked)",
                );
                None
            }
            Err(e) => {
                tracing::error!(
                    target: "simard::ooda",
                    error = %e,
                    "no-progress breaker: gh spawn failed (goal still Blocked)",
                );
                None
            }
        }
    }
}

/// Parse the issue number from a `gh issue create` success line, which prints
/// the created issue's URL (e.g. `https://github.com/owner/repo/issues/4231`).
/// Returns `None` when the trailing path segment is not a bare number, so a
/// malformed / unexpected output never fabricates a bogus link.
fn parse_issue_number(url: &str) -> Option<String> {
    let last = url.trim().trim_end_matches('/').rsplit('/').next()?;
    (!last.is_empty() && last.chars().all(|c| c.is_ascii_digit())).then(|| last.to_string())
}

/// Link a filed tracking `issue` to `goal` as a tracked artifact so the done-gate
/// gains a derivable signal
/// ([`crate::goal_curation::completion_gate::has_derivable_signal`]) and the
/// goal's done-criteria become machine-verifiable (the tracking issue observed
/// as `CLOSED`). Idempotent: a no-op when the goal already references this issue
/// number, so a re-escalation never appends a duplicate ref.
fn link_tracking_issue(goal: &mut ActiveGoal, filed: &FiledIssue) {
    let num = filed.number.trim_start_matches('#');
    let already = goal
        .wip_refs
        .iter()
        .any(|w| w.kind.eq_ignore_ascii_case("issue") && w.ref_id.trim_start_matches('#') == num);
    if already {
        return;
    }
    goal.wip_refs.push(WipRef {
        kind: "issue".to_string(),
        ref_id: num.to_string(),
        label: format!("{NO_PROGRESS_TRACKING_LABEL_PREFIX}#{num}"),
        url: filed.url.clone(),
    });
}

/// The escalation side effect shared by every breaker path: set the goal
/// `Blocked` with `blocked_reason`, then file a tracking issue and **link it
/// back to the goal** as a tracked artifact — unless the goal already carries a
/// breaker-authored tracking issue, in which case no duplicate is filed
/// (idempotent).
///
/// Linking the issue is what makes an `UNCLEAR-CRITERIA` goal's done-criteria
/// measurable: without it the breaker filed a tracking issue but *orphaned* it,
/// so the goal's `wip_refs` stayed empty, `has_derivable_signal` stayed `false`,
/// and the done-gate could never verify completion — the exact WHY
/// ("no tracked PR/issue the done-gate can verify") that stranded the synthetic
/// `simard-identity-*` goals. With the link the done-gate can observe the
/// tracking issue as `CLOSED` and certify the goal, or a human can navigate
/// goal → issue to resolve/re-scope it.
fn escalate_with_tracking_issue(
    state: &mut OodaState,
    goal_id: &str,
    blocked_reason: String,
    issue_title: &str,
    issue_body: &str,
    filer: &dyn NoProgressIssueFiler,
) {
    // Idempotence: never file a second tracking issue for a goal already linked
    // to one (a re-stall must not spam duplicate `ooda-stuck` issues).
    let already_tracked = state
        .active_goals
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .is_some_and(|g| g.wip_refs.iter().any(is_breaker_tracking_ref));

    let filed = if already_tracked {
        None
    } else {
        filer.file_issue(issue_title, issue_body)
    };

    if let Some(g) = state
        .active_goals
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
    {
        g.status = GoalProgress::Blocked(blocked_reason);
        if let Some(issue) = &filed {
            link_tracking_issue(g, issue);
        }
    }
}

/// What the breaker did this cycle — returned for logging and asserted by tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NoProgressBreakerReport {
    /// Goals set `Completed` for the evidence-aware archive to pick up.
    pub marked_done: Vec<String>,
    /// Goals removed from the board as obsolete.
    pub dropped: Vec<String>,
    /// Goals set `Blocked` with the sentinel and escalated to a tracking issue.
    /// Under the root-cause upgrade (issue #16) the block reason always carries
    /// the concrete WHY + evidence — never a bare "needs human review".
    pub escalated: Vec<String>,
    /// `MISSING-PRECONDITION` goals whose precondition was healed (e.g. a missing
    /// governed repo cloned) and left active for a fresh retry (issue #16).
    pub healed: Vec<String>,
    /// `UPSTREAM-DEPENDENCY` goals deferred to `Paused` with the blocking ref
    /// recorded, awaiting auto-clear (issue #16).
    pub deferred: Vec<String>,
    /// Goals for which a single guided engineer was spawned with the WHY as
    /// guidance (issue #16). Bounded to one per goal.
    pub engineer_spawned: Vec<String>,
    /// Deferred goals auto-resumed to `NotStarted` because their upstream
    /// resolved (issue #16). Housekeeping, not a firing.
    pub auto_cleared: Vec<String>,
    /// Goals whose root-cause investigation itself errored — surfaced so the
    /// breaker fails **closed** (no terminal action, counter preserved) rather
    /// than silently blocking or completing on an unknown cause (issue #16).
    pub investigation_errors: Vec<String>,
    /// Goals whose **bare** `[OODA-SAFEGUARD] … needs human review` block was
    /// re-investigated this cycle by the already-blocked re-investigation pass
    /// (issue #17) and upgraded away from bare — completed / dropped / healed /
    /// deferred / handed to a fixer / blocked WITH the concrete why. Recorded per
    /// goal that the reasoner successfully classified (a fail-closed reasoner
    /// error is surfaced via [`investigation_errors`](Self::investigation_errors),
    /// not here). Housekeeping-style bookkeeping; does not itself constitute a
    /// [`fired`](Self::fired) — the terminal-action buckets do.
    pub reinvestigated: Vec<String>,
    /// Standing/perpetual goals (issue #2589) that produced a no-action ("idle")
    /// cycle. Such a goal is inherently bursty — it ships a durable improvement
    /// periodically and idles between — so it is **exempt** from the breaker:
    /// its consecutive-no-action counter is reset and it stays active rather than
    /// being blocked/escalated. Recorded for the cycle log because an idling
    /// standing goal is normal, not a fault. Never contributes to
    /// [`fired`](Self::fired).
    pub perpetual_idled: Vec<String>,
    /// Standing **research** goals (issue #4399) that produced a no-action
    /// ("idle") cycle. For a standing cognition-research goal an idle cycle is a
    /// **FAULT**, not the benign bursty idle of [`perpetual_idled`]: its charter
    /// is continuous exploration, so it must generate a NEW source/experiment
    /// every cycle. The never-idle rail therefore re-orients the goal
    /// ([`ActiveGoal::roll_to_new_cycle`]) so the next OODA cycle re-enters work
    /// generation, resets its no-action counter, and records the fault here so it
    /// is visible in the cycle log — the opposite of the old silent exemption.
    /// It stays **fail-closed**: the goal is never blocked/killed/parked, and
    /// this is deliberately NOT a [`fired`](Self::fired) firing (it is a
    /// re-orient, not a terminal breaker action).
    pub research_idle_faults: Vec<String>,
}

impl NoProgressBreakerReport {
    /// True when the breaker took a threshold action for at least one goal this
    /// cycle. A standing/perpetual idle, an auto-clear, and a fail-closed
    /// investigation error are deliberately **not** firings — they are the
    /// exemption / housekeeping / fail-closed paths working as intended.
    pub fn fired(&self) -> bool {
        !self.marked_done.is_empty()
            || !self.dropped.is_empty()
            || !self.escalated.is_empty()
            || !self.healed.is_empty()
            || !self.deferred.is_empty()
            || !self.engineer_spawned.is_empty()
    }

    /// Compact one-line summary for the cycle log.
    pub fn log_line(&self) -> String {
        format!(
            "done={} dropped={} escalated={} healed={} deferred={} engineer={} \
             auto_cleared={} reinvestigated={} errors={} perpetual_idled={} \
             research_faults={}",
            self.marked_done.len(),
            self.dropped.len(),
            self.escalated.len(),
            self.healed.len(),
            self.deferred.len(),
            self.engineer_spawned.len(),
            self.auto_cleared.len(),
            self.reinvestigated.len(),
            self.investigation_errors.len(),
            self.perpetual_idled.len(),
            self.research_idle_faults.len(),
        )
    }
}

/// Fixed vocabulary of research-goal idle-fault categories (issue #4399).
/// Rendered to a stable, constant `&str` for the cycle log so no untrusted text
/// is ever folded into a log line — this is the log-injection guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResearchIdleFault {
    /// The standing research goal advanced but produced no source-ingestion and
    /// no experiment — an idle cycle, which for a research charter is a fault.
    NoNovelActionProduced,
}

impl ResearchIdleFault {
    /// Stable, lowercase-kebab category token for the cycle log.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ResearchIdleFault::NoNovelActionProduced => "no-novel-action-produced",
        }
    }
}

/// How the breaker must treat a confirmed no-action ("idle") cycle for a
/// STANDING/perpetual goal. Non-standing goals are never classified here — they
/// fall through to the normal escalation ladder unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StandingIdle {
    /// Non-research standing goal (e.g. CI-stewardship). Idling is NORMAL for a
    /// bursty goal — take the benign perpetual-idle exemption (issue #2589):
    /// reset the counter, keep the goal active, record it in `perpetual_idled`.
    BenignExempt,
    /// Standing RESEARCH goal ([`ActiveGoal::is_standing_research_goal`]). Idling
    /// is a FAULT (issue #4399): record the goal in `research_idle_faults`, warn
    /// with the `fault` category, reset the counter, and re-orient via
    /// [`ActiveGoal::roll_to_new_cycle`]. Never block/kill/park.
    ResearchFault { fault: ResearchIdleFault },
}

/// Classify a confirmed no-action cycle for a STANDING goal (issue #4399). Pure
/// and total: reads only the in-memory goal and performs no IO. Returns `None`
/// when the goal is not standing (the caller then runs the normal escalation
/// ladder). This is the ONE place the research-vs-benign split is decided —
/// shared by both breaker sites via [`apply_standing_idle`] so their semantics
/// can never drift apart.
///
/// * standing AND research ([`ActiveGoal::is_standing_research_goal`]) →
///   [`StandingIdle::ResearchFault`]
/// * standing, non-research ([`ActiveGoal::is_perpetual`] only) →
///   [`StandingIdle::BenignExempt`]
///
/// Research is checked first because a research goal is *also* perpetual; the
/// conjunction predicate keeps the branches mutually exclusive. The decision is a
/// pure function of the structured charter predicates — no hardcoded goal id.
pub(crate) fn classify_standing_idle(goal: &ActiveGoal) -> Option<StandingIdle> {
    if goal.is_standing_research_goal() {
        Some(StandingIdle::ResearchFault {
            fault: ResearchIdleFault::NoNovelActionProduced,
        })
    } else if goal.is_perpetual() {
        Some(StandingIdle::BenignExempt)
    } else {
        None
    }
}

/// Re-orient a standing research goal that slipped into an idle so the NEXT cycle
/// re-enters Lever A work generation (design a NEW source/experiment). Uses the
/// SAME [`ActiveGoal::roll_to_new_cycle`] path the completion gate uses for a
/// non-completable standing goal: the goal is returned to the canonical
/// re-dispatchable state (`NotStarted`, stale WIP dropped), never Blocked or
/// removed. In-memory only; persisted by the next cycle commit. **Fail-closed**:
/// if the goal cannot be located it is left exactly as it was (active) — a
/// research-idle fault must never disable dispatch.
fn reorient_research_goal(board: &mut GoalBoard, goal_id: &str) {
    if let Some(goal) = board.active.iter_mut().find(|g| g.id == goal_id) {
        goal.roll_to_new_cycle();
    }
}

/// Apply the shared standing-idle policy at a breaker site (issue #4399):
/// [`classify_standing_idle`] decides, and this performs the matching side
/// effects. Returns `true` when `goal_id` names a standing goal that was fully
/// handled here — the caller must then `continue` — and `false` for an ordinary
/// goal the breaker should process through its normal threshold path. **Both**
/// breaker sites ([`apply_no_progress_breaker_with_threshold`] and
/// [`apply_no_progress_breaker_investigated`]) call THIS one function, so not just
/// the classification but the whole exemption/fault behaviour can never drift.
///
/// `tracker` is the counter the driver detached from `state.no_progress_tracker`
/// with `std::mem::take`, so a reset here mutates the same counter restored onto
/// `state` at the end of the pass. The `research_idle_faults` / `perpetual_idled`
/// entry is the **bare goal id** (a controlled goal-board slug); the fixed-
/// vocabulary fault category is surfaced only in the always-present `warn` log.
fn apply_standing_idle(
    board: &mut GoalBoard,
    tracker: &mut NoProgressTracker,
    report: &mut NoProgressBreakerReport,
    goal_id: &str,
) -> bool {
    let Some(classification) = board
        .active
        .iter()
        .find(|g| g.id == goal_id)
        .and_then(classify_standing_idle)
    else {
        return false;
    };

    // Every standing-idle path — benign OR research-fault — resets the no-action
    // counter and keeps the goal active for the next cycle; only the reporting and
    // re-orient differ. Hoisted so that "a standing idle never advances the breaker
    // toward a firing" is a single, unmissable invariant.
    tracker.record_progress(goal_id);

    match classification {
        StandingIdle::BenignExempt => {
            // Benign standing/perpetual exemption (issue #2589): a non-research
            // standing goal is inherently bursty — an idle no-action cycle is
            // NORMAL, not the livelock the breaker guards against. Keep it active
            // for the next cycle.
            report.perpetual_idled.push(goal_id.to_string());
            tracing::info!(
                target: "simard::ooda",
                goal = %goal_id,
                "no-progress breaker: standing/perpetual goal idled this cycle \
                 (normal, not a fault) — counter reset, goal stays active",
            );
        }
        StandingIdle::ResearchFault { fault } => {
            // Never-idle rail (issue #4399): a standing research goal that idles
            // is a FAULT. Record the fault and re-orient it (roll_to_new_cycle:
            // NotStarted + stale WIP dropped) so the NEXT cycle re-enters work
            // generation and yields a NEW source or a NEW experiment — while
            // staying fail-closed (never block/kill/park, never a firing).
            report.research_idle_faults.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                category = fault.as_str(),
                "no-progress breaker: research goal idled — FAULT: re-orienting to \
                 generate a novel source/experiment next cycle \
                 (counter reset, goal stays active, never blocked)",
            );
            reorient_research_goal(board, goal_id);
        }
    }
    true
}

/// Establishes a machine-fixable precondition for a `MISSING-PRECONDITION` stall
/// (issue #16) — e.g. cloning a governed repository that was never checked out.
/// Injected so the heal path is exercised hermetically. `Err(reason)` surfaces
/// the failure so the breaker can escalate WITH the clone error attached rather
/// than looping forever.
pub(crate) trait PreconditionHealer {
    /// Attempt to establish the goal's missing precondition.
    fn heal(&self, goal: &ActiveGoal, why: &NoProgressWhy) -> Result<(), String>;
}

/// Spawns a single guided engineer for an `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`
/// stall (issue #16), reusing the **same** engineer dispatch the OODA loop's Act
/// phase already uses. Injected so the guided-retry path is exercised without a
/// real subprocess. Returns `true` when the spawn was accepted.
pub(crate) trait NoProgressEngineerDispatcher {
    /// Spawn (or queue) one engineer for `goal_id` with `task` embedding the WHY.
    fn spawn_engineer(&self, goal_id: &str, task: &str) -> bool;
}

/// Whether the root-cause **investigated** breaker (issue #16) is enabled
/// (default). Set `SIMARD_NO_PROGRESS_INVESTIGATE=off` to fall back to the base
/// verify-once ladder ([`apply_no_progress_breaker`]) — a conservative operator
/// kill-switch, mirroring `SIMARD_COMPLETION_EVIDENCE`.
pub(crate) fn no_progress_investigation_enabled() -> bool {
    std::env::var("SIMARD_NO_PROGRESS_INVESTIGATE")
        .map(|v| v.trim() != "off")
        .unwrap_or(true)
}

/// Apply the Fix-3 no-progress breaker to this cycle's `outcomes` using the
/// default [`NO_PROGRESS_BREAKER_THRESHOLD`].
pub(crate) fn apply_no_progress_breaker(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
) -> NoProgressBreakerReport {
    apply_no_progress_breaker_with_threshold(
        state,
        outcomes,
        evidence,
        filer,
        NO_PROGRESS_BREAKER_THRESHOLD,
    )
}

/// Threshold-parameterised core (tests inject a small threshold rather than
/// coupling to the shipped constant).
///
/// For each outcome carrying a goal id:
/// * a no-shippable-progress no-op ([`outcome_made_no_progress`]) bumps the
///   goal's consecutive-no-action counter; at `threshold` the done-gate runs
///   **once** and the goal is resolved via the ladder (mark done / drop /
///   escalate);
/// * any other successful goal outcome (engineer spawned, progress accepted)
///   resets the counter.
///
/// Marked-done goals are set [`GoalProgress::Completed`] so the subsequent
/// `archive_completed_evidence_aware` archives them with the same evidence;
/// dropped goals are removed from the board; escalated goals are set
/// [`GoalProgress::Blocked`] with the no-progress sentinel and a tracking issue
/// is filed. Stale counters are pruned to the live board.
pub(crate) fn apply_no_progress_breaker_with_threshold(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport {
    let mut report = NoProgressBreakerReport::default();

    // Detach the tracker from `state` so the disposition closure can borrow the
    // board immutably while the tracker mutates. They are disjoint, but the
    // borrow checker cannot prove it through a method call across the closure.
    let mut tracker = std::mem::take(&mut state.no_progress_tracker);

    for outcome in outcomes {
        let Some(goal_id) = outcome.action.goal_id.as_deref() else {
            continue;
        };

        if !outcome_made_no_progress(outcome) {
            // Real progress (engineer spawned, or reviewer-accepted advance)
            // resets the consecutive-no-action count. Failures (success=false)
            // are owned by `goal_failure_counts` and left untouched here.
            if outcome.success {
                tracker.record_progress(goal_id);
            }
            continue;
        }

        // Standing/perpetual exemption (issue #2589) + never-idle research rail
        // (issue #4399), unified in `apply_standing_idle` (which calls the shared
        // pure `classify_standing_idle`) so the two breaker sites can never drift.
        // A standing NON-research goal keeps the benign "idle = normal" exemption;
        // a standing RESEARCH goal's idle is a FAULT that re-orients it to generate
        // a novel source/experiment next cycle. Both stay fail-closed — never
        // hard-blocked / parked "needs human review": that is the production defect
        // this fixes. Detection reuses the *same* `is_perpetual()` flag (issue
        // #2580) the non-completability path keys on; the research split adds only
        // `is_standing_research_goal()` on top — never a second notion of standing.
        if apply_standing_idle(&mut state.active_goals, &mut tracker, &mut report, goal_id) {
            continue;
        }

        // Compute the resolution in an inner scope so the immutable borrow of
        // the board (via `goal`) ends before the match mutates the board.
        let resolution = {
            let Some(goal) = state.active_goals.active.iter().find(|g| g.id == goal_id) else {
                // The goal already left the board this cycle (e.g. archived);
                // clear any stale counter and skip.
                tracker.record_progress(goal_id);
                continue;
            };
            let gate = CompletionEvidenceGate::new(evidence);
            tracker.record_and_resolve(goal_id, threshold, || verify_stuck_goal(goal, &gate))
        };

        match resolution {
            NoProgressResolution::Continue => {}
            NoProgressResolution::MarkDone => {
                if let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = GoalProgress::Completed;
                }
                report.marked_done.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: evidence present — marking goal DONE for archival",
                );
            }
            NoProgressResolution::Drop { reason } => {
                state.active_goals.active.retain(|g| g.id != goal_id);
                state.active_goals.backlog.retain(|b| b.id != goal_id);
                report.dropped.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    reason = %reason,
                    "no-progress breaker: goal obsolete — DROPPING from the board",
                );
            }
            NoProgressResolution::Escalate {
                blocked_reason,
                issue_title,
                issue_body,
            } => {
                escalate_with_tracking_issue(
                    state,
                    goal_id,
                    blocked_reason,
                    &issue_title,
                    &issue_body,
                    filer,
                );
                report.escalated.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: unresolved after threshold — BLOCKED + tracking issue filed and linked",
                );
            }
            // The base breaker's ladder ([`resolve_no_progress`]) only yields the
            // four legacy resolutions above; the root-cause rungs (issue #16) are
            // produced by [`resolution_for_why`] and driven by
            // [`apply_no_progress_breaker_investigated`]. Handle them defensively
            // (fail-safe no-op + warn) so this match stays exhaustive without a
            // panic on the legacy path.
            NoProgressResolution::Heal { .. }
            | NoProgressResolution::Defer { .. }
            | NoProgressResolution::SpawnEngineer { .. }
            | NoProgressResolution::SurfaceInvestigationFailure { .. } => {
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: root-cause resolution reached the base adapter \
                     (unexpected) — taking no action",
                );
            }
        }
    }

    // Prune counters for goals no longer on the active board.
    let live: HashSet<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();
    tracker.retain_goals(&live);

    state.no_progress_tracker = tracker;
    report
}

// ===========================================================================
// Root-cause investigation adapter (issue #16)
// ===========================================================================

/// Apply the **investigated** no-progress breaker: before authoring any block,
/// run the injected root-cause `reasoner` at the threshold, then route the goal
/// down the self-resolving ladder — auto-complete, heal a missing precondition,
/// defer behind an upstream, or spawn one guided engineer — escalating to a
/// human (WITH the concrete WHY + evidence) only as a last resort.
///
/// Every dependency (evidence, reasoner, precondition healer, engineer
/// dispatcher, issue filer) is injected so the whole ladder is hermetically
/// testable. Fail-closed: a reasoner error takes **no** terminal action and
/// preserves the counter so the goal retries.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_no_progress_breaker_investigated(
    state: &mut OodaState,
    outcomes: &[ActionOutcome],
    evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport {
    let mut report = NoProgressBreakerReport::default();

    // Detach the tracker so the board can be borrowed while the tracker mutates.
    let mut tracker = std::mem::take(&mut state.no_progress_tracker);

    // --- Auto-clear pass: resume deferred goals whose upstream resolved. ---
    // Runs first and independently of `outcomes`, so a deferred goal auto-clears
    // even on a cycle it produced no fresh no-action outcome.
    let mut to_clear: Vec<usize> = Vec::new();
    for (i, goal) in state.active_goals.active.iter().enumerate() {
        if !matches!(goal.status, GoalProgress::Paused) {
            continue;
        }
        if !goal.wip_refs.iter().any(is_breaker_defer_ref) {
            continue;
        }
        match evidence.dependency_goal_state(goal) {
            Ok(DependencyState::Resolved { .. }) => to_clear.push(i),
            Ok(_) => {}
            Err(e) => tracing::error!(
                target: "simard::ooda",
                goal = %goal.id,
                error = %e,
                "no-progress breaker: dependency_goal_state errored during auto-clear \
                 (leaving goal Paused — fail closed)",
            ),
        }
    }
    for i in to_clear {
        let goal = &mut state.active_goals.active[i];
        goal.status = GoalProgress::NotStarted;
        goal.wip_refs.retain(|w| !is_breaker_defer_ref(w));
        tracker.reset_count(&goal.id);
        report.auto_cleared.push(goal.id.clone());
        tracing::info!(
            target: "simard::ooda",
            goal = %goal.id,
            "no-progress breaker: upstream resolved — auto-clearing deferred goal to active",
        );
    }

    for outcome in outcomes {
        let Some(goal_id) = outcome.action.goal_id.as_deref() else {
            continue;
        };

        if !outcome_made_no_progress(outcome) {
            // Real progress (engineer spawned, or reviewer-accepted advance)
            // resets the no-action count (and the spent guided-retry flag).
            if outcome.success {
                tracker.record_progress(goal_id);
            }
            continue;
        }

        // Standing/perpetual exemption (issue #2589) + never-idle research rail
        // (issue #4399) run BEFORE investigation, via the SAME `apply_standing_idle`
        // helper as the non-investigated site so the reasoner is never consulted for
        // a goal that is exempt OR that must be re-oriented. A standing NON-research
        // goal keeps the benign "idle = normal" exemption; a standing RESEARCH
        // goal's idle is a FAULT that re-orients it (never blocked, never
        // investigated).
        if apply_standing_idle(&mut state.active_goals, &mut tracker, &mut report, goal_id) {
            continue;
        }

        let consecutive = tracker.record_no_action(goal_id);
        if consecutive < threshold {
            continue;
        }

        // Threshold reached: investigate the root cause ONCE.
        let why = {
            let Some(goal) = state.active_goals.active.iter().find(|g| g.id == goal_id) else {
                // The goal already left the board this cycle; clear its counter.
                tracker.reset_count(goal_id);
                continue;
            };
            match reasoner.investigate(goal) {
                Ok(why) => why,
                Err(e) => {
                    // Fail closed: no terminal action, counter PRESERVED so the
                    // goal retries next cycle. Never a silent block or completion.
                    report.investigation_errors.push(goal_id.to_string());
                    tracing::error!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        error = %e,
                        "no-progress breaker: root-cause investigation errored — \
                         taking NO terminal action (fail closed), counter preserved",
                    );
                    continue;
                }
            }
        };

        let guided_retry_used = tracker.guided_retry_used(goal_id);
        let resolution = resolution_for_why(consecutive, why, guided_retry_used);

        // On-transition path: the goal is not in a Blocked state, so the
        // non-terminal `Heal` / `SpawnEngineer` rungs leave its status untouched
        // (`unblock_nonterminal = false`).
        apply_resolution_side_effects(
            state,
            goal_id,
            consecutive,
            resolution,
            healer,
            dispatcher,
            filer,
            &mut tracker,
            &mut report,
            false,
        );
    }

    // Prune counters/flags for goals no longer on the active board.
    let live: HashSet<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();
    tracker.retain_goals(&live);

    state.no_progress_tracker = tracker;
    report
}

/// Drive one classified [`NoProgressResolution`] to its board mutation, tracker
/// update, injected side effect (heal / spawn / file issue), and report entry.
///
/// Shared by BOTH the on-transition breaker
/// ([`apply_no_progress_breaker_investigated`]) and the already-blocked
/// re-investigation pass ([`reinvestigate_bare_blocked_goals`], issue #17) so the
/// class → action mapping can never drift between the two populations.
/// `consecutive` renders into any authored block reason / clone-error escalation.
///
/// `unblock_nonterminal` distinguishes the callers' starting state. The
/// on-transition path acts on a goal that is *not* Blocked, so the non-terminal
/// `Heal` / `SpawnEngineer` rungs leave its status untouched (`false`). The
/// re-investigation path acts on a goal already parked in a BARE `Blocked` state,
/// so those same rungs must additionally UN-BLOCK it to
/// [`GoalProgress::NotStarted`] (`true`) — otherwise the brain would never
/// re-select it and the heal / spawned fixer could never advance it. The terminal
/// rungs (MarkDone / Drop / Defer / Escalate) set a definitive non-bare status
/// regardless of the flag.
#[allow(clippy::too_many_arguments)]
fn apply_resolution_side_effects(
    state: &mut OodaState,
    goal_id: &str,
    consecutive: u32,
    resolution: NoProgressResolution,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    tracker: &mut NoProgressTracker,
    report: &mut NoProgressBreakerReport,
    unblock_nonterminal: bool,
) {
    match resolution {
        NoProgressResolution::Continue => {}
        NoProgressResolution::MarkDone => {
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::Completed;
            }
            tracker.reset_count(goal_id);
            report.marked_done.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                "no-progress breaker: ALREADY-COMPLETE — marking goal DONE (no block)",
            );
        }
        NoProgressResolution::Drop { reason } => {
            state.active_goals.active.retain(|g| g.id != goal_id);
            state.active_goals.backlog.retain(|b| b.id != goal_id);
            tracker.reset_count(goal_id);
            report.dropped.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                reason = %reason,
                "no-progress breaker: OBSOLETE — DROPPING from the board (no block)",
            );
        }
        NoProgressResolution::Heal { why } => {
            let goal = match state.active_goals.active.iter().find(|g| g.id == goal_id) {
                Some(g) => g.clone(),
                None => {
                    tracker.reset_count(goal_id);
                    return;
                }
            };
            match healer.heal(&goal, &why) {
                Ok(()) => {
                    // Precondition established; give a genuine fresh window.
                    tracker.reset_count(goal_id);
                    if unblock_nonterminal
                        && let Some(g) = state
                            .active_goals
                            .active
                            .iter_mut()
                            .find(|g| g.id == goal_id)
                    {
                        g.status = GoalProgress::NotStarted;
                    }
                    report.healed.push(goal_id.to_string());
                    tracing::warn!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        "no-progress breaker: MISSING-PRECONDITION healed — retrying (no block)",
                    );
                }
                Err(err) => {
                    // A failed heal must not loop forever: escalate WITH the
                    // clone error attached as evidence (fail closed, with WHY).
                    let mut why_err = why;
                    why_err
                        .evidence
                        .push(Evidence::new("clone-error", goal_id, err.clone()));
                    let blocked_reason = no_progress_blocked_reason_with_why(consecutive, &why_err);
                    // Route through the shared helper so the filed tracking issue
                    // is LINKED back to the goal (measurable done-criteria) and a
                    // re-stall never spams a duplicate `ooda-stuck` issue.
                    escalate_with_tracking_issue(
                        state,
                        goal_id,
                        blocked_reason,
                        &format!(
                            "OODA no-progress breaker: precondition heal failed for '{goal_id}'"
                        ),
                        &format!(
                            "Healing the MISSING-PRECONDITION for goal `{goal_id}` failed: \
                             {err}\n\nThe goal has been Blocked with the WHY attached."
                        ),
                        filer,
                    );
                    tracker.reset_count(goal_id);
                    report.escalated.push(goal_id.to_string());
                    tracing::error!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        error = %err,
                        "no-progress breaker: precondition heal FAILED — escalating WITH why",
                    );
                }
            }
        }
        NoProgressResolution::Defer {
            blocking_ref,
            evidence: _ev,
        } => {
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::Paused;
                // Record the specific blocking upstream as the WHY so the
                // auto-clear pass can resume the goal when it resolves.
                if !g.wip_refs.iter().any(is_breaker_defer_ref) {
                    g.wip_refs.push(WipRef {
                        kind: DEPENDENCY_WIP_KIND.to_string(),
                        ref_id: blocking_ref.clone(),
                        label: format!("{NO_PROGRESS_DEFER_LABEL_PREFIX}{blocking_ref}"),
                        url: None,
                    });
                }
            }
            tracker.reset_count(goal_id);
            report.deferred.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                blocking_ref = %blocking_ref,
                "no-progress breaker: UPSTREAM-DEPENDENCY — deferring (Paused), \
                 will auto-clear (no block)",
            );
        }
        NoProgressResolution::SpawnEngineer { task, why } => {
            let spawned = dispatcher.spawn_engineer(goal_id, &task);
            // Bound the guided retry to one regardless of accept/reject: a
            // rejected spawn escalates (WITH why) on the next threshold rather
            // than spawning forever.
            tracker.mark_guided_retry(goal_id);
            tracker.reset_count(goal_id);
            // Re-investigation path: the goal was BARE-blocked — un-block it so the
            // brain can re-select it and the spawned fixer can advance it.
            if unblock_nonterminal
                && let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::NotStarted;
            }
            if spawned {
                report.engineer_spawned.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    why = %why.class.token(),
                    "no-progress breaker: {} — spawned ONE guided engineer (no block yet)",
                    why.class.token(),
                );
            } else {
                tracing::error!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    "no-progress breaker: guided engineer spawn was rejected — \
                     will escalate WITH why on next stall",
                );
            }
        }
        NoProgressResolution::Escalate {
            blocked_reason,
            issue_title,
            issue_body,
        } => {
            escalate_with_tracking_issue(
                state,
                goal_id,
                blocked_reason,
                &issue_title,
                &issue_body,
                filer,
            );
            tracker.reset_count(goal_id);
            report.escalated.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                "no-progress breaker: stuck after guided retry — BLOCKED WITH why + issue filed and linked",
            );
        }
        NoProgressResolution::SurfaceInvestigationFailure { class, reason } => {
            // Bound the evidence-less re-investigation (issue #16 follow-up). The
            // first fix (#4096) made this rung non-terminal so a goal is never
            // parked with a bare `evidence=[(none)]` block — but an *unbounded*
            // re-investigation is its own livelock: a goal whose done-criteria are
            // permanently unclear surfaces → resets → forever, making no shippable
            // progress and never reaching a human. After
            // `SURFACED_INVESTIGATION_FAILURE_LIMIT` consecutive surfaced failures,
            // stop spinning and escalate to a human WITH the re-investigation count
            // as concrete evidence (so the never-`evidence=[(none)]` invariant
            // holds — the count is real evidence, not `(none)`) and a measurable
            // "make the done-criteria machine-checkable" ask.
            let surfaced = tracker.record_surfaced_failure(goal_id);
            if surfaced >= SURFACED_INVESTIGATION_FAILURE_LIMIT {
                let why = NoProgressWhy::new(
                    class,
                    vec![Evidence::new(
                        "re-investigation",
                        goal_id,
                        format!("{surfaced} consecutive evidence-less investigations"),
                    )],
                );
                let blocked_reason = no_progress_blocked_reason_with_why(consecutive, &why);
                let (issue_title, issue_body) =
                    surfaced_failure_escalation_issue(goal_id, class, surfaced);
                escalate_with_tracking_issue(
                    state,
                    goal_id,
                    blocked_reason,
                    &issue_title,
                    &issue_body,
                    filer,
                );
                tracker.clear_surfaced_failures(goal_id);
                tracker.reset_count(goal_id);
                report.escalated.push(goal_id.to_string());
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    why = %class.token(),
                    surfaced_failures = surfaced,
                    "no-progress breaker: evidence-less re-investigation bounded out after \
                     {surfaced} surfaced failures — BLOCKED WITH re-investigation count as \
                     evidence + human triage issue filed and linked to make the done-criteria measurable",
                );
                return;
            }

            // Below the bound: the independent investigation reached the terminal
            // rung with NO evidence. A goal must NEVER be parked with
            // `evidence=[(none)]`, so this is a SURFACED failure — not a bare
            // block. Take no terminal action: record it in `investigation_errors`
            // (fail visible) and leave the goal retriable so the next investigation
            // can recover real evidence (fail closed). The guided-retry flag is
            // preserved, so a future terminal rung goes straight here again rather
            // than spawning a second engineer. On the re-investigation path the
            // goal starts in a bare / `(none)` Blocked state, so un-block it to
            // `NotStarted` so the brain can re-select it and a later cycle can
            // re-investigate.
            if unblock_nonterminal
                && let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::NotStarted;
            }
            tracker.reset_count(goal_id);
            report.investigation_errors.push(goal_id.to_string());
            tracing::error!(
                target: "simard::ooda",
                goal = %goal_id,
                reason = %reason,
                surfaced_failures = surfaced,
                "no-progress breaker: evidence-less terminal outcome SURFACED as an \
                 investigation failure (never parked with evidence=[(none)]) — retriable",
            );
        }
    }
}

// ===========================================================================
// Already-blocked re-investigation pass (issue #17)
// ===========================================================================

/// Re-investigate goals already parked in a **bare** `[OODA-SAFEGUARD] … needs
/// human review` block and drive each away from bare (issue #17).
///
/// The on-transition breaker ([`apply_no_progress_breaker_investigated`])
/// investigates WHY a goal is stuck **only at the cycle it crosses the
/// threshold**. That leaves goals parked bare by a pre-#16 daemon build — or on a
/// cycle the reasoner erred — stranded forever with an unexplained "needs human
/// review" marker, never re-examined. This population-driven pass closes that
/// gap: every cycle it scans the ACTIVE board (independent of this cycle's
/// `outcomes`, mirroring the auto-clear scan) for goals in a **bare** blocked
/// state ([`is_bare_no_progress_block`]) and runs the SAME injected WHY reasoner +
/// [`resolution_for_why`] ladder over them via the shared
/// [`apply_resolution_side_effects`], so no goal is ever left bare — each is
/// upgraded to a concrete WHY and, when the WHY is actionable, completed /
/// dropped / healed / deferred / handed to a spawned fixer.
///
/// Invariants:
/// * **Perpetual exemption** (I5): standing/perpetual goals are excluded before
///   investigation, mirroring the on-transition path.
/// * **Fail closed** (I2): a reasoner error takes NO terminal action, leaves the
///   bare marker exactly as-is (retried next cycle), and records nothing in the
///   dedupe set.
/// * **Un-block on non-terminal resolution**: a re-investigated goal handed to a
///   fixer or healed is set [`GoalProgress::NotStarted`] so the brain can
///   re-select it (`unblock_nonterminal = true`).
/// * **Idempotency** (I3/I4): the WHY-rewrite removes the goal from the bare
///   population next cycle (primary), and the persisted `(goal, class)` dedupe set
///   in [`NoProgressTracker`] prevents a duplicate terminal action if a restart
///   re-parks the goal bare (belt-and-suspenders) — at most ONE fixer per
///   `(goal, class)`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reinvestigate_bare_blocked_goals(
    state: &mut OodaState,
    _evidence: &dyn EvidenceSource,
    reasoner: &dyn NoProgressWhyReasoner,
    healer: &dyn PreconditionHealer,
    dispatcher: &dyn NoProgressEngineerDispatcher,
    filer: &dyn NoProgressIssueFiler,
    threshold: u32,
) -> NoProgressBreakerReport {
    let mut report = NoProgressBreakerReport::default();

    // Detach the tracker so the board can be borrowed while the tracker mutates.
    let mut tracker = std::mem::take(&mut state.no_progress_tracker);

    // The bare-blocked, non-perpetual population, captured up front. The
    // perpetual exemption (I5) runs BEFORE investigation, so a standing goal's
    // reasoner is never consulted. Collecting ids first keeps the board free to
    // mutate as each goal is resolved.
    let bare_ids: Vec<String> = state
        .active_goals
        .active
        .iter()
        .filter(|g| !g.is_perpetual())
        .filter_map(|g| match &g.status {
            GoalProgress::Blocked(reason) if needs_reinvestigation(reason) => Some(g.id.clone()),
            _ => None,
        })
        .collect();

    for goal_id in bare_ids {
        // Investigate the bare goal ONCE. Fail closed on error: no terminal
        // action, marker left exactly as-is, nothing recorded in the dedupe set.
        let why = {
            let Some(goal) = state.active_goals.active.iter().find(|g| g.id == goal_id) else {
                continue;
            };
            match reasoner.investigate(goal) {
                Ok(why) => why,
                Err(e) => {
                    report.investigation_errors.push(goal_id.clone());
                    tracing::error!(
                        target: "simard::ooda",
                        goal = %goal_id,
                        error = %e,
                        "no-progress re-investigation: root-cause investigation errored — \
                         leaving the bare block untouched (fail closed), will retry next cycle",
                    );
                    continue;
                }
            }
        };

        let class = why.class;
        report.reinvestigated.push(goal_id.clone());

        // Belt-and-suspenders dedupe (I3): a terminal action was already taken for
        // this (goal, class) — possibly before a restart that re-parked the goal
        // bare. Do NOT repeat the side effect (e.g. spawn a second fixer), but
        // never leave the goal bare: rewrite it to the WHY-bearing block so the
        // rail excludes it next cycle — UNLESS the re-investigation produced no
        // evidence, in which case rewriting would re-author the very
        // `evidence=[(none)]` block this change forbids. For that case, surface an
        // investigation failure and un-block the goal so it stays retriable.
        if tracker.reinvestigated(&goal_id, class) {
            if why.evidence.is_empty() {
                if let Some(g) = state
                    .active_goals
                    .active
                    .iter_mut()
                    .find(|g| g.id == goal_id)
                {
                    g.status = GoalProgress::NotStarted;
                }
                report.investigation_errors.push(goal_id.clone());
                tracing::error!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    why = %class.token(),
                    "no-progress re-investigation: (goal, class) already resolved but the \
                     re-investigation produced NO evidence — refusing to re-author an \
                     evidence=[(none)] block; surfaced as an investigation failure, un-blocked",
                );
                continue;
            }
            let blocked_reason = no_progress_blocked_reason_with_why(threshold, &why);
            if let Some(g) = state
                .active_goals
                .active
                .iter_mut()
                .find(|g| g.id == goal_id)
            {
                g.status = GoalProgress::Blocked(blocked_reason);
            }
            tracing::info!(
                target: "simard::ooda",
                goal = %goal_id,
                why = %class.token(),
                "no-progress re-investigation: (goal, class) already resolved — \
                 rewriting to the WHY-bearing block, taking NO new terminal action (dedupe)",
            );
            continue;
        }

        let guided_retry_used = tracker.guided_retry_used(&goal_id);
        let resolution = resolution_for_why(threshold, why, guided_retry_used);

        // An evidence-less terminal outcome takes NO terminal action (it is
        // surfaced + retried), so it must NOT be recorded in the (goal, class)
        // dedupe set — otherwise a later cycle that DOES recover evidence would be
        // wrongly deduped instead of spawning a fixer / escalating with the WHY.
        let took_terminal_action = resolution.is_terminal();

        // Re-investigation path: the goal starts BARE-blocked, so a non-terminal
        // `Heal` / `SpawnEngineer` rung must UN-BLOCK it to NotStarted.
        apply_resolution_side_effects(
            state,
            &goal_id,
            threshold,
            resolution,
            healer,
            dispatcher,
            filer,
            &mut tracker,
            &mut report,
            true,
        );

        // A terminal action was taken for this (goal, class); record it so a
        // re-park after a restart cannot trigger a second one. Only recorded on a
        // successful classification (never on a fail-closed error above) that took
        // a real terminal action (never a surfaced evidence-less failure).
        if took_terminal_action {
            tracker.mark_reinvestigated(&goal_id, class);
        }
    }

    // Prune counters/flags for goals no longer on the active board.
    let live: HashSet<String> = state
        .active_goals
        .active
        .iter()
        .map(|g| g.id.clone())
        .collect();
    tracker.retain_goals(&live);

    state.no_progress_tracker = tracker;
    report
}

// ===========================================================================
// Production seams (issue #16)
// ===========================================================================

/// Production [`NoProgressWhyReasoner`]: a **deterministic**, evidence-driven
/// classifier. The breaker fires precisely when the agentic loop is *failing* on
/// a goal, so routing must not delegate to the brain (it would reason about its
/// own failure). Signals, in ladder order: the done-gate certifies
/// `ALREADY-COMPLETE`; an obsolescence marker means `OBSOLETE`; an absent
/// governed repo means `MISSING-PRECONDITION`; a pending upstream means
/// `UPSTREAM-DEPENDENCY`. At the terminal rung a goal that still references open
/// work is `GENUINELY-STUCK` (evidence = those open artifacts); a goal with **no**
/// tracked artifact the done-gate can ever check is `UNCLEAR-CRITERIA` (evidence
/// = the named unmeasurable criterion) — the synthetic `simard-identity-*` goals.
/// An evidence-source error on the auxiliary signals downgrades to
/// `GENUINELY-STUCK` (fail closed — never self-heal / self-defer on an unknown
/// state), tagging the errored probe so the WHY is still concrete.
///
/// **Invariant (issue #16 follow-up):** this reasoner never returns an
/// empty-evidence WHY for either **block-authoring terminal class**
/// (`GENUINELY-STUCK` / `UNCLEAR-CRITERIA`) — the only classes whose evidence
/// renders into a human-facing `GoalProgress::Blocked` reason. Every such branch
/// attaches at least one concrete [`Evidence`], so the breaker can never author a
/// bare `evidence=[(none)]` block — the exact live-daemon defect that stranded
/// the `simard-identity-*` / coverage / parity goals with a generic,
/// evidence-free stamp. (`ALREADY-COMPLETE` routes to auto-complete and
/// `OBSOLETE` to drop, so neither renders into a block; their evidence is
/// narrative only.)
pub(crate) struct DeterministicNoProgressReasoner<'a> {
    evidence: &'a dyn EvidenceSource,
}

impl<'a> DeterministicNoProgressReasoner<'a> {
    pub(crate) fn new(evidence: &'a dyn EvidenceSource) -> Self {
        Self { evidence }
    }
}

/// Narrative evidence for an `ALREADY-COMPLETE` goal, from its tracked artifacts.
fn artifact_evidence(goal: &ActiveGoal) -> Vec<Evidence> {
    goal.wip_refs
        .iter()
        .filter_map(|w| match w.kind.to_ascii_lowercase().as_str() {
            "issue" => Some(Evidence::new(
                "issue",
                format!("#{}", w.ref_id.trim_start_matches('#')),
                "CLOSED",
            )),
            "pr" => Some(Evidence::new(
                "pr",
                format!("#{}", w.ref_id.trim_start_matches('#')),
                "MERGED",
            )),
            _ => None,
        })
        .collect()
}

/// Narrative evidence for a `GENUINELY-STUCK` goal: its still-open artifacts.
fn stuck_evidence(goal: &ActiveGoal) -> Vec<Evidence> {
    goal.wip_refs
        .iter()
        .filter_map(|w| match w.kind.to_ascii_lowercase().as_str() {
            "pr" => Some(Evidence::new(
                "pr",
                format!("#{}", w.ref_id.trim_start_matches('#')),
                "OPEN",
            )),
            "issue" => Some(Evidence::new(
                "issue",
                format!("#{}", w.ref_id.trim_start_matches('#')),
                "OPEN",
            )),
            _ => None,
        })
        .collect()
}

/// Evidence for an `UNCLEAR-CRITERIA` goal (issue #16 follow-up): a stalled goal
/// that reached the terminal rung with **no** tracked artifact the done-gate can
/// ever check — no open/closed PR or issue, no absent precondition, no upstream.
/// Its done-criteria are therefore structurally unmeasurable (the synthetic
/// `simard-identity-*` goals). This names that missing, measurable criterion so
/// the WHY is concrete and evidence-backed rather than a bare `(none)` stamp —
/// the exact live-daemon `evidence=[(none)]` defect this closes.
fn unclear_criteria_evidence(goal: &ActiveGoal) -> Vec<Evidence> {
    vec![Evidence::new(
        "done-criteria",
        goal.id.clone(),
        "unmeasurable: no tracked PR/issue the done-gate can verify",
    )]
}

/// Evidence for a `GENUINELY-STUCK` goal reached by a **fail-closed downgrade**
/// (issue #16 follow-up): an auxiliary signal (`repo-presence` /
/// `dependency-state`) errored, so the reasoner cannot self-heal / self-defer and
/// commits to `GENUINELY-STUCK`. The errored probe is named as evidence (and any
/// open artifacts appended) so the downgrade never renders `evidence=[(none)]`;
/// the full error is already surfaced via `tracing::error!`.
fn downgrade_evidence(goal: &ActiveGoal, signal: &str) -> Vec<Evidence> {
    let mut evidence = vec![Evidence::new(
        "signal",
        signal.to_string(),
        "unknown: evidence probe errored",
    )];
    evidence.extend(stuck_evidence(goal));
    evidence
}

impl NoProgressWhyReasoner for DeterministicNoProgressReasoner<'_> {
    fn investigate(&self, goal: &ActiveGoal) -> SimardResult<NoProgressWhy> {
        // 1. Done-gate positively certifies completion (the kgpacks-rs incident).
        let gate = CompletionEvidenceGate::new(self.evidence);
        if let CompletionVerdict::Complete(_) = gate.evaluate(goal) {
            return Ok(NoProgressWhy::new(
                NoProgressClass::AlreadyComplete,
                artifact_evidence(goal),
            ));
        }
        // 2. Explicit obsolescence / handoff marker.
        if let Some(reason) = obsolescence_reason(goal) {
            return Ok(NoProgressWhy::new(
                NoProgressClass::Obsolete,
                vec![Evidence::new("obsolete", goal.id.clone(), reason)],
            ));
        }
        // 3. A governed target repo was never cloned.
        match self.evidence.repo_present(goal) {
            Ok(false) => {
                let repo = goal.repo.clone().unwrap_or_else(|| goal.id.clone());
                return Ok(NoProgressWhy::new(
                    NoProgressClass::MissingPrecondition,
                    vec![Evidence::new("repo", repo, "absent")],
                ));
            }
            Ok(true) => {}
            Err(e) => {
                tracing::error!(
                    target: "simard::ooda",
                    goal = %goal.id,
                    error = %e,
                    "no-progress reasoner: repo_present errored — downgrading to GENUINELY-STUCK",
                );
                return Ok(NoProgressWhy::new(
                    NoProgressClass::GenuinelyStuck,
                    downgrade_evidence(goal, "repo-presence"),
                ));
            }
        }
        // 4. Gated on a specific upstream that has not landed.
        match self.evidence.dependency_goal_state(goal) {
            Ok(DependencyState::Pending { blocking_ref }) => {
                return Ok(NoProgressWhy::new(
                    NoProgressClass::UpstreamDependency,
                    vec![Evidence::new("dependency", blocking_ref, "OPEN")],
                ));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(
                    target: "simard::ooda",
                    goal = %goal.id,
                    error = %e,
                    "no-progress reasoner: dependency_goal_state errored — \
                     downgrading to GENUINELY-STUCK",
                );
                return Ok(NoProgressWhy::new(
                    NoProgressClass::GenuinelyStuck,
                    downgrade_evidence(goal, "dependency-state"),
                ));
            }
        }
        // 5. No machine-resolvable cause found. Split the terminal rung by whether
        //    the goal still references open work the done-gate could ever track:
        //      - open artifacts present  -> GENUINELY-STUCK (evidence = them);
        //      - no tracked artifact     -> UNCLEAR-CRITERIA (evidence = the named
        //        unmeasurable criterion) — the synthetic simard-identity-* goals.
        //    Never emit an empty-evidence GENUINELY-STUCK block: that is the exact
        //    live-daemon `evidence=[(none)]` defect (issue #16 follow-up).
        let open_artifacts = stuck_evidence(goal);
        if open_artifacts.is_empty() {
            Ok(NoProgressWhy::new(
                NoProgressClass::UnclearCriteria,
                unclear_criteria_evidence(goal),
            ))
        } else {
            Ok(NoProgressWhy::new(
                NoProgressClass::GenuinelyStuck,
                open_artifacts,
            ))
        }
    }
}

/// Production [`PreconditionHealer`]: clones a governed repository that was never
/// checked out into `$HOME/src/<repo>` via `gh repo clone`, so the next cycle can
/// make progress.
pub(crate) struct CloneRepoHealer {
    default_owner: String,
}

impl CloneRepoHealer {
    pub(crate) fn new(default_owner: impl Into<String>) -> Self {
        Self {
            default_owner: default_owner.into(),
        }
    }
}

impl PreconditionHealer for CloneRepoHealer {
    fn heal(&self, goal: &ActiveGoal, _why: &NoProgressWhy) -> Result<(), String> {
        let repo = goal
            .repo
            .as_deref()
            .ok_or_else(|| "goal has no target repo to clone".to_string())?;
        let name = repo.rsplit('/').next().unwrap_or(repo);
        let owner_repo = if repo.contains('/') {
            repo.to_string()
        } else {
            format!("{}/{repo}", self.default_owner)
        };
        let home = dirs::home_dir().ok_or_else(|| "cannot resolve home dir".to_string())?;
        let dest = home.join("src").join(name);
        if dest.is_dir() {
            return Ok(());
        }
        let dest_str = dest
            .to_str()
            .ok_or_else(|| "clone destination path is not valid UTF-8".to_string())?;
        let out = std::process::Command::new("gh")
            .args(["repo", "clone", &owner_repo, dest_str])
            .output()
            .map_err(|e| format!("failed to spawn `gh repo clone {owner_repo}`: {e}"))?;
        if out.status.success() {
            tracing::warn!(
                target: "simard::ooda",
                repo = %owner_repo,
                "no-progress breaker: cloned missing governed repo for precondition heal",
            );
            Ok(())
        } else {
            Err(format!(
                "`gh repo clone {owner_repo}` exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }
}

/// Production [`NoProgressEngineerDispatcher`]: collects the guided-engineer
/// spawn requests during the breaker pass (which holds `&mut OodaState`), so the
/// caller can drain them and dispatch through the **same**
/// `dispatch_spawn_engineer` the Act phase uses — once the state borrow is free.
/// This reuses the existing capability rather than building a parallel spawner.
#[derive(Default)]
pub(crate) struct QueueingEngineerDispatcher {
    requests: std::cell::RefCell<Vec<(String, String)>>,
}

impl QueueingEngineerDispatcher {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Consume the collected `(goal_id, task)` spawn requests.
    pub(crate) fn into_requests(self) -> Vec<(String, String)> {
        self.requests.into_inner()
    }
}

impl NoProgressEngineerDispatcher for QueueingEngineerDispatcher {
    fn spawn_engineer(&self, goal_id: &str, task: &str) -> bool {
        self.requests
            .borrow_mut()
            .push((goal_id.to_string(), task.to_string()));
        true
    }
}

/// Default production threshold re-export for the cycle wiring.
pub(crate) const INVESTIGATED_BREAKER_THRESHOLD: u32 = NO_PROGRESS_BREAKER_THRESHOLD;

#[cfg(test)]
mod tests_tracking_issue_link {
    use super::{FiledIssue, link_tracking_issue, parse_issue_number};
    use crate::goal_curation::{ActiveGoal, WipRef};

    #[test]
    fn parse_issue_number_extracts_trailing_number_from_gh_url() {
        assert_eq!(
            parse_issue_number("https://github.com/rysweet/Simard/issues/4231").as_deref(),
            Some("4231"),
        );
        // Trailing slash tolerated.
        assert_eq!(
            parse_issue_number("https://github.com/o/r/issues/12/").as_deref(),
            Some("12"),
        );
    }

    #[test]
    fn parse_issue_number_rejects_non_numeric_or_empty() {
        assert_eq!(parse_issue_number(""), None);
        assert_eq!(parse_issue_number("not a url"), None);
        assert_eq!(
            parse_issue_number("https://github.com/o/r/pull/abc"),
            None,
            "a non-numeric tail must never fabricate a bogus link",
        );
    }

    #[test]
    fn link_tracking_issue_appends_a_recognisable_issue_ref() {
        let mut goal = ActiveGoal::new("g", "d", 1);
        link_tracking_issue(
            &mut goal,
            &FiledIssue {
                number: "4231".to_string(),
                url: Some("https://example/issues/4231".to_string()),
            },
        );
        assert_eq!(goal.wip_refs.len(), 1);
        let w = &goal.wip_refs[0];
        assert_eq!(w.kind, "issue");
        assert_eq!(w.ref_id, "4231");
        assert!(super::is_breaker_tracking_ref(w));
        assert_eq!(w.url.as_deref(), Some("https://example/issues/4231"));
    }

    #[test]
    fn link_tracking_issue_is_idempotent_for_the_same_issue_number() {
        let mut goal = ActiveGoal::new("g", "d", 1);
        // Pre-existing reference to the same issue (leading '#' tolerated).
        goal.wip_refs.push(WipRef {
            kind: "issue".to_string(),
            ref_id: "#4231".to_string(),
            label: "some earlier ref".to_string(),
            url: None,
        });
        link_tracking_issue(
            &mut goal,
            &FiledIssue {
                number: "4231".to_string(),
                url: None,
            },
        );
        assert_eq!(
            goal.wip_refs.len(),
            1,
            "no duplicate ref for an issue number the goal already carries",
        );
    }
}
