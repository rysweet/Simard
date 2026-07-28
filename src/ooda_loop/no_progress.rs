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

use std::collections::HashMap;
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

/// `WipRef.kind` for the breaker's durable *suppression marker* — the
/// restart-surviving record that this goal has already been escalated by the
/// no-progress breaker, written BEFORE and INDEPENDENT of any `gh` issue link.
///
/// Its sole job is idempotence: a goal carrying this marker is never re-filed,
/// even if [`NoProgressIssueFiler::file_issue`] returned `None` (gh failed, or
/// its URL did not parse to a bare issue number). It is distinct from the linked
/// tracking ref (`kind = "issue"`, label-prefixed `[no-progress-tracking] `)
/// that a *successful* filing upgrades it into. The kind is novel, so every
/// other `wip_refs` consumer (`has_derivable_signal`, `stuck_evidence`,
/// `artifact_evidence`, the stale-assignment sweep) ignores it via their
/// `_ => None` fall-through — the marker is inert to completion/liveness logic
/// and only the breaker's idempotence guard reads it.
const NO_PROGRESS_SUPPRESSION_MARKER_KIND: &str = "ooda-breaker-marker";

/// Fixed sentinel `WipRef.ref_id` for the suppression marker. A constant — NEVER
/// derived from goal text — so goal descriptions can never smuggle content into
/// the marker (no argv/flag injection, no path traversal).
const NO_PROGRESS_SUPPRESSION_MARKER_REF_ID: &str = "ooda-breaker";

/// True when `wip` is a breaker-authored escalation artifact — EITHER the durable
/// suppression marker ([`NO_PROGRESS_SUPPRESSION_MARKER_KIND`], written
/// before/independent of linking) OR the upgraded linked tracking issue
/// (`kind = "issue"`, label-prefixed `[no-progress-tracking] `). Either one means
/// "this goal has already been escalated by the breaker", so the idempotence
/// guard in [`escalate_with_tracking_issue`] suppresses re-filing whether or not
/// the `gh` link ever landed. Recognizing the bare marker is what makes storm
/// suppression durable and restart-surviving (issue-storm suppression fix).
fn is_breaker_tracking_ref(wip: &WipRef) -> bool {
    wip.kind
        .eq_ignore_ascii_case(NO_PROGRESS_SUPPRESSION_MARKER_KIND)
        || (wip.kind.eq_ignore_ascii_case("issue")
            && wip.label.starts_with(NO_PROGRESS_TRACKING_LABEL_PREFIX))
}

/// True when `wip` is specifically the *bare* durable suppression marker (not a
/// linked tracking issue). Used by [`upgrade_suppression_marker_to_link`] to drop
/// the marker when a later filing succeeds, so the `<= 1 breaker ref per goal`
/// invariant holds (the marker is replaced by the linked ref, never supplemented).
fn is_suppression_marker(wip: &WipRef) -> bool {
    wip.kind
        .eq_ignore_ascii_case(NO_PROGRESS_SUPPRESSION_MARKER_KIND)
}

/// The durable, link-independent suppression [`WipRef`] the breaker writes to a
/// stuck goal's board record BEFORE attempting the `gh` filing. Persisted through
/// the existing atomic goal-board save path as an ordinary `WipRef` — no schema
/// change. A fixed sentinel identity (`ref_id` is a constant, never goal-derived).
fn suppression_marker() -> WipRef {
    WipRef {
        kind: NO_PROGRESS_SUPPRESSION_MARKER_KIND.to_string(),
        ref_id: NO_PROGRESS_SUPPRESSION_MARKER_REF_ID.to_string(),
        label: format!("{NO_PROGRESS_TRACKING_LABEL_PREFIX}ooda-breaker (unlinked)"),
        url: None,
    }
}

/// Fold the bare suppression marker into a linked tracking `WipRef` when a filing
/// succeeds: drop the bare marker, then append the linked tracking ref via
/// [`link_tracking_issue`]. This keeps at most **one** breaker artifact per goal
/// (marker upgraded in place, never duplicated) so the storm-suppression
/// invariant `<= 1 breaker ref per goal` holds even after a failed-then-successful
/// filing sequence.
fn upgrade_suppression_marker_to_link(goal: &mut ActiveGoal, filed: &FiledIssue) {
    goal.wip_refs.retain(|w| !is_suppression_marker(w));
    link_tracking_issue(goal, filed);
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
/// `Blocked` with `blocked_reason`, DURABLY mark it suppressed so it is never
/// re-filed, then best-effort file + link a `gh` tracking issue. Storm-safe and
/// restart-surviving.
///
/// Ordering (the storm fix): the durable suppression marker and the `Blocked`
/// status are written FIRST, through the existing atomic goal-board save path, so
/// the goal is idempotently suppressed BEFORE [`NoProgressIssueFiler::file_issue`]
/// is attempted. A `None` from `file_issue` (a `gh` outage, or a URL that did not
/// parse to a bare issue number) therefore leaves the goal `Blocked` + suppressed
/// (no re-file next cycle) instead of `Blocked` + untracked (re-file forever) —
/// the exact loop that produced the ~15-duplicate `UNCLEAR-CRITERIA` issue storm.
/// On a `Some`, the bare marker is UPGRADED IN PLACE to the linked tracking ref
/// via [`upgrade_suppression_marker_to_link`] — never appended as a duplicate.
///
/// Linking the issue is what makes an `UNCLEAR-CRITERIA` goal's done-criteria
/// measurable: with the link the done-gate can observe the tracking issue as
/// `CLOSED` and certify the goal, or a human can navigate goal → issue to
/// resolve/re-scope it.
///
/// Deliberate trade-off: a goal that received a *bare* marker from a failed first
/// filing is never re-linked on a later cycle (the idempotence guard short-circuits
/// before `file_issue`). Storm suppression is prioritized over eventual linking;
/// the stall is still durably surfaced via the `Blocked` status and its WHY. The
/// escape hatch is manual (remove the bare marker; the next cycle re-escalates).
fn escalate_with_tracking_issue(
    state: &mut OodaState,
    goal_id: &str,
    blocked_reason: String,
    issue_title: &str,
    issue_body: &str,
    filer: &dyn NoProgressIssueFiler,
) {
    // Idempotence: a goal already carrying any breaker artifact (a bare
    // suppression marker OR a linked tracking ref) is never re-filed — a re-stall
    // must not spam duplicate `ooda-stuck` issues, even across a daemon restart.
    let Some(g) = state
        .active_goals
        .active
        .iter_mut()
        .find(|g| g.id == goal_id)
    else {
        return;
    };
    let already_tracked = g.wip_refs.iter().any(is_breaker_tracking_ref);

    // 1. Durable, link-independent suppression FIRST. Always block the goal; an
    //    already-suppressed goal stops here so it is never re-filed (idempotence
    //    across a `gh` failure and a daemon restart, since the marker lives on the
    //    goal board, not the in-memory tracker).
    g.status = GoalProgress::Blocked(blocked_reason);
    if already_tracked {
        return;
    }
    g.wip_refs.push(suppression_marker());

    // 2. Best-effort link SECOND, holding the same borrow (`file_issue` does not
    //    touch `state`). On success upgrade the bare marker in place to the linked
    //    ref; on `None` the goal stays Blocked + suppressed and is not re-filed.
    if let Some(filed) = filer.file_issue(issue_title, issue_body) {
        upgrade_suppression_marker_to_link(g, &filed);
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
    /// every cycle. The breaker records the fault here (so it is visible in the
    /// cycle log — the opposite of the old silent exemption) and resets its
    /// no-action counter. Issue #4453: the breaker no longer re-orients the goal
    /// itself — the destructive [`ActiveGoal::roll_to_new_cycle`] is owned solely
    /// by the agentic per-goal reasoner's `reorient` action, so this is purely a
    /// SIGNAL. It stays **fail-closed**: the goal is never blocked/killed/parked,
    /// and this is deliberately NOT a [`fired`](Self::fired) firing.
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

    /// True when this pass produced something worth surfacing in the aggregate
    /// cycle log: a threshold [`fired`](Self::fired), an auto-clear, a
    /// fail-closed investigation error, OR a research-idle fault (issue #4399,
    /// crusty finding 2). A *pure* research-idle cycle is none of the first three
    /// — the never-idle rail re-oriented a goal without firing/clearing/erroring
    /// — so without the last term the `research_faults=N` metric would never
    /// reach the cycle log even though a fault occurred. This is the single
    /// source of truth for the root-cause breaker's log gate so the count is
    /// surfaced consistently. (The per-goal `warn!` inside `apply_standing_idle`
    /// still fires regardless; this makes the *aggregate count* observable too.)
    pub fn is_noteworthy(&self) -> bool {
        self.fired()
            || !self.auto_cleared.is_empty()
            || !self.investigation_errors.is_empty()
            || !self.research_idle_faults.is_empty()
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
    /// with the `fault` category, and reset the counter. Issue #4453 demoted this
    /// to a pure SIGNAL — the breaker no longer re-orients here; the destructive
    /// [`ActiveGoal::roll_to_new_cycle`] is owned solely by the agentic per-goal
    /// reasoner's `reorient` action. Never block/kill/park.
    ResearchFault { fault: ResearchIdleFault },
    /// Standing RESEARCH goal that still holds a LIVE in-flight artifact — an
    /// open PR / working branch / engineer session
    /// ([`ActiveGoal::has_live_in_flight_ref`]). It is NOT idle: a durable,
    /// unmerged PR in review is genuine novel progress (issue #4399, crusty
    /// finding 1). This is PROGRESS, not a fault — reset the no-action counter
    /// and keep the goal active, but record NO fault and do NOT re-orient.
    /// Re-orienting would call [`ActiveGoal::roll_to_new_cycle`], wiping the
    /// load-bearing `wip_refs` the Overseer dedup set, engineer-admission
    /// control, and completion gate depend on — which would lose merge tracking
    /// and let the next cycle spawn an overlapping engineer on the same seam.
    ResearchInFlight,
}

/// Classify a confirmed no-action cycle for a STANDING goal (issue #4399). Pure
/// and total: reads only the in-memory goal and performs no IO. Returns `None`
/// when the goal is not standing (the caller then runs the normal escalation
/// ladder). This is the ONE place the research-vs-benign split is decided —
/// shared by both breaker sites via [`apply_standing_idle`] so their semantics
/// can never drift apart.
///
/// * standing AND research ([`ActiveGoal::is_standing_research_goal`]) with a
///   LIVE in-flight ref ([`ActiveGoal::has_live_in_flight_ref`]) →
///   [`StandingIdle::ResearchInFlight`] (progress — preserve refs, no fault)
/// * standing AND research, NO live ref →
///   [`StandingIdle::ResearchFault`]
/// * standing, non-research ([`ActiveGoal::is_perpetual`] only) →
///   [`StandingIdle::BenignExempt`]
///
/// Research is checked first because a research goal is *also* perpetual; the
/// conjunction predicate keeps the branches mutually exclusive. Within the
/// research branch the live-in-flight guard is checked first so a goal holding an
/// open, unmerged PR is treated as progress and NEVER faulted or re-oriented
/// (issue #4399, crusty finding 1) — wiping its load-bearing `wip_refs` would
/// drop dedup / admission / merge-tracking state. The decision is a pure function
/// of the structured charter predicates — no hardcoded goal id.
pub(crate) fn classify_standing_idle(goal: &ActiveGoal) -> Option<StandingIdle> {
    if goal.is_standing_research_goal() {
        if goal.has_live_in_flight_ref() {
            Some(StandingIdle::ResearchInFlight)
        } else {
            Some(StandingIdle::ResearchFault {
                fault: ResearchIdleFault::NoNovelActionProduced,
            })
        }
    } else if goal.is_perpetual() {
        Some(StandingIdle::BenignExempt)
    } else {
        None
    }
}

/// Per-cycle PR-liveness reconcile (NEW-1 Prong 2, PR #4428). Prunes every `pr`
/// [`WipRef`] on each ACTIVE goal whose PR number is NOT in `open_prs`, so a
/// merged/closed PR can no longer read as a LIVE in-flight ref through
/// [`ActiveGoal::has_live_in_flight_ref`] and suppress the never-idle fault
/// forever.
///
/// This is the reconcile step [`has_live_in_flight_ref`](ActiveGoal::has_live_in_flight_ref)
/// assumes has already run this cycle. It is **pure and IO-free**: the caller
/// fetches the open-PR set once per cycle (via the existing
/// [`crate::stewardship::merge_authority::PrGhClient::list_open_prs`] path — NO
/// new shell/gh parse) and passes the numbers in. Tests drive it directly with
/// an in-memory set.
///
/// Decisions (see PR #4428 brief):
/// * **ref_id parse** — `ref_id.trim_start_matches('#').parse::<u32>()`, matching
///   the repo's existing normalization. A ref whose id does NOT parse is LEFT in
///   place (with a warning) rather than pruned: a malformed id may still be a
///   live PR, and dropping it could reintroduce the round-1 finding-#1 regression.
/// * **fail-open** — the caller SKIPS this reconcile entirely when the open-PR
///   fetch errors (prunes nothing that cycle), so a fetch blip can never wipe a
///   live PR ref. On a genuine empty open set (`Ok([])`) all `pr` refs prune.
/// * **scope** — ACTIVE (non-terminal) goals only; backlog/archived untouched.
///
/// Only `pr` refs are considered; every other kind (branch/session/engineer are
/// handled by the stale-assignment sweep, issue is durable) passes through.
///
/// Returns the pruned `(goal_id, ref_id)` pairs so the caller can log exactly
/// what was dropped (used by the prod wrapper's per-ref `tracing::info!`).
/// Single-repo pure prune contract exercised directly by the NEW-1 unit tests
/// (`tests_no_progress`). Production reconciles per-repo through
/// [`prune_merged_pr_refs_scoped`]; this single-set form is retained as the
/// IO-free contract those tests drive, so it is compiled only under `cfg(test)`.
#[cfg(test)]
pub(crate) fn prune_merged_pr_refs(
    board: &mut GoalBoard,
    open_prs: &HashSet<u32>,
) -> Vec<(String, String)> {
    let mut pruned = Vec::new();
    for goal in board.active.iter_mut() {
        // Disjoint field borrows: `id` (read) and `wip_refs` (mutated) so the
        // common no-prune case allocates nothing — this runs every cycle over
        // every active goal (cf. the per-ref alloc trimmed in #4399).
        retain_open_pr_refs(&goal.id, &mut goal.wip_refs, open_prs, &mut pruned);
    }
    pruned
}

/// Repo-scoped variant of [`prune_merged_pr_refs`] (FIX-2, OBSERVATION 2). Each
/// active goal's `pr` refs are reconciled against the open-PR set of ITS OWN
/// repo — not one shared set — so a goal tracking a PR in a repo other than
/// Simard can never have its (possibly still-OPEN) PR pruned against the wrong
/// repo's open set (a latent F1-style false prune).
///
/// * `repo_of` maps a goal to its canonical `owner/repo` slug (the caller owns
///   the `goal.repo` → slug policy).
/// * `open_by_repo` holds the open-PR set for each slug that was successfully
///   fetched this cycle. **Fail-open per repo**: a goal whose slug is ABSENT
///   from the map (its fetch errored, or was skipped) is left entirely
///   untouched — never pruned — so a `gh` blip on one repo can never wipe a
///   possibly-live ref. A slug PRESENT with an empty set means "genuinely
///   nothing open" and all its `pr` refs prune (correct).
///
/// Shares the exact per-goal `pr`-retain logic with [`prune_merged_pr_refs`]
/// via [`retain_open_pr_refs`], so the parse / fail-open-on-unparseable-id
/// behaviour can never drift between the two.
pub(crate) fn prune_merged_pr_refs_scoped(
    board: &mut GoalBoard,
    repo_of: impl Fn(&ActiveGoal) -> String,
    open_by_repo: &HashMap<String, HashSet<u32>>,
) -> Vec<(String, String)> {
    let mut pruned = Vec::new();
    for goal in board.active.iter_mut() {
        let slug = repo_of(goal);
        // Fail-open: no open set for this goal's repo → prune nothing for it.
        let Some(open_prs) = open_by_repo.get(&slug) else {
            continue;
        };
        retain_open_pr_refs(&goal.id, &mut goal.wip_refs, open_prs, &mut pruned);
    }
    pruned
}

/// Shared per-goal `pr`-ref retain used by both [`prune_merged_pr_refs`] and
/// [`prune_merged_pr_refs_scoped`]. Keeps every non-`pr` ref untouched; keeps a
/// `pr` ref whose number is in `open_prs`; prunes a `pr` ref whose number is
/// absent; and KEEPS (fail-open, with a warning) a `pr` ref whose id does not
/// parse as a `u32` — a malformed id may still be a live PR, and dropping it
/// could reintroduce the round-1 finding-#1 regression. Each pruned
/// `(goal_id, ref_id)` pair is pushed onto `pruned`; the `goal_id` clone happens
/// only on the (rare) prune path so the common no-prune case allocates nothing.
fn retain_open_pr_refs(
    goal_id: &str,
    wip_refs: &mut Vec<WipRef>,
    open_prs: &HashSet<u32>,
    pruned: &mut Vec<(String, String)>,
) {
    wip_refs.retain(|wip| {
        if !wip.kind.trim().eq_ignore_ascii_case("pr") {
            return true;
        }
        match wip.ref_id.trim_start_matches('#').parse::<u32>() {
            Ok(num) => {
                let live = open_prs.contains(&num);
                if !live {
                    pruned.push((goal_id.to_string(), wip.ref_id.clone()));
                }
                live
            }
            Err(_) => {
                tracing::warn!(
                    target: "simard::ooda",
                    goal = %goal_id,
                    ref_id = %wip.ref_id,
                    "pr wip_ref id did not parse as u32 — keeping ref (not pruning, fail-open)"
                );
                true
            }
        }
    });
}

/// effects. Returns `true` when `goal_id` names a standing goal that was fully
/// handled here — the caller must then `continue` — and `false` for an ordinary
/// goal the breaker should process through its normal threshold path. **Both**
/// breaker sites ([`apply_no_progress_breaker_with_threshold`] and
/// [`apply_no_progress_breaker_investigated`]) call THIS one function, so not just
/// the classification but the whole exemption/fault behaviour can never drift.
///
/// The goal is located ONCE (mutably) so the fault branch can re-orient the very
/// same goal it just classified — no second board scan. **Fail-closed**: if the
/// goal is not on the board it is left exactly as it was (the early `return
/// false` hands it back to the caller's normal path); a research-idle fault
/// must never disable dispatch.
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
    let Some(goal) = board.active.iter().find(|g| g.id == goal_id) else {
        return false;
    };
    let Some(classification) = classify_standing_idle(goal) else {
        return false;
    };

    // Every standing-idle path — benign OR research-fault — resets the no-action
    // counter and keeps the goal active for the next cycle; only the reporting
    // differs (re-orient is no longer done here — issue #4453). Hoisted so that
    // "a standing idle never advances the breaker toward a firing" is a single,
    // unmissable invariant.
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
            // Never-idle rail (issue #4399), demoted to a pure SIGNAL by the
            // agentic per-goal-per-cycle rail (issue #4453): a standing research
            // goal that idles is recorded as a FAULT signal and its no-action
            // counter is reset (done above) so the breaker never fires on it — but
            // this imperative path NO LONGER re-orients it. The re-orient decision
            // (and the destructive `roll_to_new_cycle`) is owned exclusively by the
            // reasoner's `reorient` action in `drive_per_goal_cycle`, which runs
            // once per active goal every cycle and reads this same idle condition
            // via `classify_standing_idle`. Rolling here as well would double-drive
            // the goal — resetting it to `NotStarted` and dropping WIP even when the
            // reasoner decided to `wait`/`continue`/`investigate` — which was the
            // 70ab8541 idle→reset fault-loop. Stays fail-closed: never
            // block/kill/park, never a firing; the goal stays active for the
            // reasoner to decide on next.
            report.research_idle_faults.push(goal_id.to_string());
            tracing::warn!(
                target: "simard::ooda",
                goal = %goal_id,
                category = fault.as_str(),
                "no-progress breaker: research goal idled — FAULT signal recorded \
                 (counter reset, goal stays active, never blocked); re-orient is \
                 owned by the agentic per-goal reasoner, not this imperative path",
            );
        }
        StandingIdle::ResearchInFlight => {
            // Live in-flight progress (issue #4399, crusty finding 1): the
            // research goal still holds an open, unmerged PR (or a working branch
            // / engineer session) — genuine novel progress, NOT an idle. The
            // hoisted counter reset above already keeps it active for the next
            // cycle; we deliberately record NO fault and do NOT re-orient, because
            // roll_to_new_cycle would wipe the load-bearing wip_refs the Overseer
            // dedup set, engineer-admission control, and completion gate depend on
            // — losing merge tracking and letting the next cycle spawn an
            // overlapping engineer on the same seam. wip_refs / assigned_to /
            // status are left untouched. Neither `research_idle_faults` nor
            // `perpetual_idled`: it was not idle.
            tracing::info!(
                target: "simard::ooda",
                goal = %goal_id,
                "no-progress breaker: research goal holds a live in-flight artifact \
                 (open PR/branch/session) — progress, not idle: counter reset, refs \
                 preserved, goal stays active (not faulted, not re-oriented)",
            );
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

/// Attempt to derive checkable done-criteria for a stalled goal from its OWN
/// `description` — no external clarification, no brain call. Consulted at the
/// terminal rung of [`DeterministicNoProgressReasoner::investigate`] *before* an
/// empty-artifact stall defaults to `UNCLEAR-CRITERIA`.
///
/// Returns:
/// * `Some(evidence)` — non-empty, bounded — when the description carries a
///   machine-checkable finish condition: **either** an explicit, self-contained
///   criteria section (a recognized [`crate::done_criteria::CRITERIA_HEADINGS`]
///   heading with at least one concrete [`crate::done_criteria::has_checkable_item`]
///   item) **or** an operator done-gate finish line
///   ([`crate::goal_board_store::DONE_WHEN_MARKER`]) written by a
///   [`crate::goal_board_store::DoneGatePin`] repair. The caller proceeds as
///   `GENUINELY-STUCK` with this evidence, so a goal that already spelled out
///   concrete done-criteria — or was *repaired* to have them — is not
///   misclassified `UNCLEAR-CRITERIA` and swept into the storm-feeding
///   population, and is not re-blocked cycle after cycle (issue #4930).
/// * `None` — when nothing checkable can be derived. The caller falls to the
///   legacy `UNCLEAR-CRITERIA` classification (byte-identical to before).
///
/// Both signals come from the single shared, hardened
/// [`crate::done_criteria::detect_measurable_criteria`] detector so admission,
/// classification and the done-gate repair path share exactly one definition
/// (issue #4930): one length cap, one heading set, one checkable-item scan, and
/// one finish-line marker — no drifting second copy, and the repair mechanism can
/// never disagree with the classifier that consumes it.
///
/// Totality/safety contract: never panics, never returns `Some(vec![])`, and
/// bounds its work by [`crate::done_criteria::DERIVE_CRITERIA_MAX_SCAN`] so
/// adversarial goal text cannot cause a panic or pathological scanning. The
/// emitted evidence carries only the goal id and a constant token — never raw
/// goal text — so nothing is smuggled into the WHY / log line.
fn derive_criteria(goal: &ActiveGoal) -> Option<Vec<Evidence>> {
    use crate::done_criteria::{CriteriaSignal, detect_measurable_criteria};

    // One length-capped, lower-cased pass over the untrusted description, shared
    // by admission and classification via the hardened `done_criteria` detector.
    let why = match detect_measurable_criteria(&goal.description)? {
        CriteriaSignal::Heading(heading) => {
            format!("derivable: goal description states explicit {heading}")
        }
        // A `goal set-done-gate` repair (issue #4930): the finish line is a
        // machine-checkable anchor an operator pinned, so the goal is genuinely
        // stuck-with-criteria rather than UNCLEAR-CRITERIA — and must not be
        // re-blocked on the next cycle even when it carries no markdown bullet.
        CriteriaSignal::DoneGateFinishLine => {
            "derivable: goal carries an operator-pinned done-gate finish line".to_string()
        }
    };

    Some(vec![Evidence::new("done-criteria", goal.id.clone(), why)])
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
        // 5. No machine-resolvable cause found. Split the terminal rung:
        //      - open artifacts present  -> GENUINELY-STUCK (evidence = them);
        //      - else, criteria DERIVABLE from the goal's own description
        //        -> GENUINELY-STUCK (evidence = the derived criterion), so a goal
        //        that already spelled out concrete done-criteria is not
        //        misclassified UNCLEAR-CRITERIA and swept into the issue-storm
        //        population;
        //      - else no tracked artifact and nothing derivable -> UNCLEAR-CRITERIA
        //        (evidence = the named unmeasurable criterion) — the synthetic
        //        simard-identity-* goals.
        //    Never emit an empty-evidence GENUINELY-STUCK block: that is the exact
        //    live-daemon `evidence=[(none)]` defect (issue #16 follow-up).
        let open_artifacts = stuck_evidence(goal);
        if !open_artifacts.is_empty() {
            Ok(NoProgressWhy::new(
                NoProgressClass::GenuinelyStuck,
                open_artifacts,
            ))
        } else if let Some(derived) = derive_criteria(goal) {
            Ok(NoProgressWhy::new(NoProgressClass::GenuinelyStuck, derived))
        } else {
            Ok(NoProgressWhy::new(
                NoProgressClass::UnclearCriteria,
                unclear_criteria_evidence(goal),
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

/// TDD (Step 7) — FAILING tests pinning the storm-suppression contract for the
/// UNCLEAR-CRITERIA / no-progress-breaker duplicate-issue storm defect.
///
/// # The defect these tests lock down
///
/// `escalate_with_tracking_issue` only writes a durable breaker-tracking
/// [`WipRef`] when [`NoProgressIssueFiler::file_issue`] returns `Some`. When
/// `gh issue create` fails (`file_issue` → `None`) the goal is Blocked but
/// carries **no** breaker ref, so the next firing sees `already_tracked == false`
/// and re-files. Over ~2 days of cycles that produced ~15 duplicate
/// "no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)"
/// issues off a handful of stuck goals — the observed storm.
///
/// # The contract (what the fix must make true)
///
/// Suppression must be **durable and independent of `gh` URL-parse success**:
///   * a firing whose `file_issue` returns `None` STILL writes a durable
///     per-goal suppression marker to the goal's `wip_refs`;
///   * that marker is recognised by [`is_breaker_tracking_ref`], so the existing
///     `already_tracked` guard suppresses every subsequent re-file;
///   * because the marker lives in `wip_refs` (serialized on the goal board) the
///     suppression survives a daemon restart (the in-memory tracker resets, the
///     marker does not);
///   * a later successful filing UPGRADES the bare marker to a real linked
///     tracking ref WITHOUT appending a second breaker ref (≤ 1 breaker marker
///     per goal);
///   * a truly-unclear goal therefore yields exactly ONE deduplicated breaker
///     outcome (a single filing/annotation) even when link-parsing fails.
///
/// These tests compile against the CURRENT symbols and fail on BEHAVIOUR:
/// today `file_issue` → `None` leaves no marker, so re-filing happens and the
/// filing counter climbs past 1.
#[cfg(test)]
mod tests_storm_suppression {
    use std::cell::Cell;

    use super::{NoProgressIssueFiler, escalate_with_tracking_issue, is_breaker_tracking_ref};
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::ooda_loop::OodaState;

    /// A filer that always FAILS to file (models a `gh` outage / unresolved
    /// issue number) and counts how many times the escalation path invoked it.
    #[derive(Default)]
    struct FailingCountingFiler {
        calls: Cell<usize>,
    }

    impl NoProgressIssueFiler for FailingCountingFiler {
        fn file_issue(&self, _title: &str, _body: &str) -> Option<super::FiledIssue> {
            self.calls.set(self.calls.get() + 1);
            None
        }
    }

    /// A filer that SUCCEEDS, returning a fixed issue number, and counts calls.
    struct SucceedingCountingFiler {
        number: String,
        calls: Cell<usize>,
    }

    impl SucceedingCountingFiler {
        fn new(number: &str) -> Self {
            Self {
                number: number.to_string(),
                calls: Cell::new(0),
            }
        }
    }

    impl NoProgressIssueFiler for SucceedingCountingFiler {
        fn file_issue(&self, _title: &str, _body: &str) -> Option<super::FiledIssue> {
            self.calls.set(self.calls.get() + 1);
            Some(super::FiledIssue {
                number: self.number.clone(),
                url: Some(format!("https://github.com/o/r/issues/{}", self.number)),
            })
        }
    }

    fn state_with_active(goal: ActiveGoal) -> OodaState {
        let mut board = GoalBoard::new();
        board.active.push(goal);
        OodaState::new(board)
    }

    fn only_goal(state: &OodaState) -> &ActiveGoal {
        &state.active_goals.active[0]
    }

    fn breaker_ref_count(goal: &ActiveGoal) -> usize {
        goal.wip_refs
            .iter()
            .filter(|w| is_breaker_tracking_ref(w))
            .count()
    }

    /// T1 — the core storm-stopper. When `file_issue` returns `None`, escalation
    /// must STILL leave a durable suppression marker so a second escalation of
    /// the same goal does NOT re-file. Today it re-files → `calls == 2`.
    #[test]
    fn failed_filing_still_suppresses_a_second_escalation() {
        let filer = FailingCountingFiler::default();
        let mut state = state_with_active(ActiveGoal::new(
            "simard-identity-atelier-industrial-furniture-de",
            "a synthetic identity goal with no tracked artifact",
            1,
        ));

        // First firing: gh fails, but suppression must be recorded durably.
        escalate_with_tracking_issue(
            &mut state,
            "simard-identity-atelier-industrial-furniture-de",
            "[OODA-SAFEGUARD] no-progress breaker: UNCLEAR-CRITERIA".to_string(),
            "no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)",
            "body",
            &filer,
        );
        assert_eq!(
            filer.calls.get(),
            1,
            "first firing attempts exactly one filing"
        );

        // Second firing of the SAME goal: the durable suppression marker written
        // on the first (failed) firing must short-circuit re-filing.
        escalate_with_tracking_issue(
            &mut state,
            "simard-identity-atelier-industrial-furniture-de",
            "[OODA-SAFEGUARD] no-progress breaker: UNCLEAR-CRITERIA".to_string(),
            "no-progress breaker: goal stuck after guided retry (UNCLEAR-CRITERIA)",
            "body",
            &filer,
        );
        assert_eq!(
            filer.calls.get(),
            1,
            "a failed first filing must not cause the breaker to re-file every \
             cycle — this is the duplicate-issue storm (15 dup issues in ~2 days)",
        );
    }

    /// T1b — the marker written on a failed filing must be a durable
    /// breaker-tracking ref (recognised by `is_breaker_tracking_ref`), because
    /// the `already_tracked` guard keys on exactly that predicate.
    #[test]
    fn failed_filing_writes_a_durable_breaker_marker() {
        let filer = FailingCountingFiler::default();
        let mut state = state_with_active(ActiveGoal::new("g", "vague goal", 1));

        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &filer,
        );

        let goal = only_goal(&state);
        assert!(
            matches!(goal.status, GoalProgress::Blocked(_)),
            "the goal is still Blocked even when filing failed",
        );
        assert_eq!(
            breaker_ref_count(goal),
            1,
            "a failed filing must persist exactly one durable breaker suppression \
             marker so dedup no longer depends on gh URL-parse success",
        );
    }

    /// T2 — restart durability. The suppression marker lives on the goal board
    /// (`wip_refs`), so it survives a daemon restart that resets the in-memory
    /// tracker. Modelled by a serde round-trip of the goal into a fresh state.
    #[test]
    fn suppression_survives_a_restart_and_still_blocks_re_filing() {
        let first = FailingCountingFiler::default();
        let mut state = state_with_active(ActiveGoal::new("g", "vague goal", 1));
        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &first,
        );
        assert_eq!(first.calls.get(), 1);

        // Simulate a restart: serialize the goal (as the board would be persisted)
        // and rebuild a brand-new OodaState from the deserialized goal. The
        // in-memory NoProgressTracker is fresh; only wip_refs carry over.
        let json = serde_json::to_string(only_goal(&state)).expect("goal serializes");
        let restored: ActiveGoal = serde_json::from_str(&json).expect("goal deserializes");
        let mut restarted = state_with_active(restored);

        let second = FailingCountingFiler::default();
        escalate_with_tracking_issue(
            &mut restarted,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &second,
        );
        assert_eq!(
            second.calls.get(),
            0,
            "after a restart the durable marker must still suppress re-filing — \
             the tracker resets but the goal-board marker does not",
        );
    }

    /// T3 — a later successful filing must UPGRADE the bare marker to a linked
    /// tracking ref WITHOUT appending a second breaker ref (≤ 1 marker per goal).
    #[test]
    fn successful_filing_upgrades_the_bare_marker_without_duplicating() {
        // First: a failed filing writes the bare durable marker.
        let failing = FailingCountingFiler::default();
        let mut state = state_with_active(ActiveGoal::new("g", "vague goal", 1));
        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &failing,
        );
        assert_eq!(
            breaker_ref_count(only_goal(&state)),
            1,
            "bare marker present"
        );

        // Then: `gh` recovers and a filing succeeds. The escalation must not
        // append a SECOND breaker ref — it upgrades the existing bare marker.
        let ok = SucceedingCountingFiler::new("4231");
        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &ok,
        );
        assert_eq!(
            breaker_ref_count(only_goal(&state)),
            1,
            "the goal must carry at most one breaker marker — the successful \
             filing upgrades the bare marker in place, never appends a duplicate",
        );
    }

    /// T3-happy — the pre-existing happy path stays intact: a first successful
    /// filing writes exactly one linked breaker ref and a re-escalation of an
    /// already-tracked goal never files again.
    #[test]
    fn happy_path_files_once_and_never_re_files_when_already_tracked() {
        let ok = SucceedingCountingFiler::new("5000");
        let mut state = state_with_active(ActiveGoal::new("g", "vague goal", 1));

        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &ok,
        );
        assert_eq!(ok.calls.get(), 1);
        assert_eq!(breaker_ref_count(only_goal(&state)), 1);

        // Already tracked → no second filing.
        escalate_with_tracking_issue(
            &mut state,
            "g",
            "[OODA-SAFEGUARD] UNCLEAR-CRITERIA".to_string(),
            "title",
            "body",
            &ok,
        );
        assert_eq!(
            ok.calls.get(),
            1,
            "an already-tracked goal is never re-filed"
        );
        assert_eq!(
            breaker_ref_count(only_goal(&state)),
            1,
            "still exactly one marker"
        );
    }
}

/// Direct unit tests for the pure `derive_criteria` terminal-rung helper — the
/// secondary (misclassification) half of the storm fix. These pin its totality
/// and security contract at the seam the reasoner tests can only exercise
/// end-to-end: conservatism, bounded scan over adversarial text, and the
/// bare-heading rejection.
#[cfg(test)]
mod tests_derive_criteria {
    use super::derive_criteria;
    use crate::done_criteria::DERIVE_CRITERIA_MAX_SCAN;
    use crate::goal_curation::ActiveGoal;

    fn goal_with_desc(desc: &str) -> ActiveGoal {
        let mut g = ActiveGoal::new("g", "", 1);
        g.description = desc.to_string();
        g
    }

    #[test]
    fn derives_from_an_explicit_criteria_section_with_items() {
        let g = goal_with_desc(
            "Harden supply-chain provenance.\n\n\
             Acceptance criteria:\n\
             - `cargo deny check` passes in CI\n\
             - every crate has a verified provenance attestation\n",
        );
        let derived = derive_criteria(&g).expect("an explicit criteria section is derivable");
        assert!(!derived.is_empty(), "never returns Some(empty)");
        assert_eq!(derived[0].kind, "done-criteria");
        assert!(
            derived[0].reference == "g",
            "evidence references the goal id, never raw goal text (no log injection)",
        );
    }

    #[test]
    fn recognizes_ordered_and_checkbox_items() {
        let ordered = goal_with_desc("Definition of done:\n1. build is green\n2. docs updated\n");
        assert!(derive_criteria(&ordered).is_some());
        let checkbox = goal_with_desc("Success criteria:\n[ ] tests pass\n[x] reviewed\n");
        assert!(derive_criteria(&checkbox).is_some());
    }

    #[test]
    fn vague_goal_with_no_criteria_section_is_not_derivable() {
        assert!(
            derive_criteria(&goal_with_desc(
                "a synthetic identity goal with no tracked artifact"
            ))
            .is_none(),
            "conservative: nothing derivable -> None (falls to UNCLEAR-CRITERIA)",
        );
    }

    #[test]
    fn bare_heading_without_any_checkable_item_is_not_derivable() {
        assert!(
            derive_criteria(&goal_with_desc(
                "Acceptance criteria: TBD, to be defined later"
            ))
            .is_none(),
            "a heading with no concrete item must not over-trigger derivation",
        );
    }

    #[test]
    fn is_total_and_bounded_over_adversarial_text() {
        // A criteria heading beyond the scan cap must NOT be matched (bounded work),
        // and pathological text (very long, control chars, `--`-prefixed, multibyte)
        // must never panic.
        let mut desc = "x".repeat(DERIVE_CRITERIA_MAX_SCAN + 500);
        desc.push_str("\nAcceptance criteria:\n- item beyond the scan cap\n");
        assert!(
            derive_criteria(&goal_with_desc(&desc)).is_none(),
            "a heading past the scan cap is out of bounds and not derived",
        );

        for pathological in [
            "\u{0}\u{1}\u{2}--\u{7}",
            "🦀🔥\n- 日本語\nacceptance criteria",
            "---\n* \n- ",
            "",
        ] {
            let _ = derive_criteria(&goal_with_desc(pathological));
        }
    }

    #[test]
    fn done_gate_pin_repair_is_derivable_even_without_a_markdown_bullet() {
        // Issue #4930 core case: an operator repairs an UNCLEAR-CRITERIA goal with
        // `goal set-done-gate`, which appends a prose finish line (no heading, no
        // markdown bullet). Before the fix `derive_criteria` returned None here, so
        // the reasoner re-classified the goal UNCLEAR-CRITERIA and re-blocked it
        // every cycle. The finish line must now be recognised as derivable so the
        // repair actually sticks.
        let mut g = goal_with_desc("Move the governed repo roster out of the framework.");
        assert!(
            derive_criteria(&g).is_none(),
            "unrepaired prose goal is not derivable (would fall to UNCLEAR-CRITERIA)"
        );
        crate::goal_board_store::DoneGatePin {
            pr: Some("4440".into()),
            issue: None,
            criteria: Some("roster is identity-owned".into()),
        }
        .apply_to(&mut g);
        let derived =
            derive_criteria(&g).expect("a done-gate pin repair must be derivable (issue #4930)");
        assert!(!derived.is_empty(), "never returns Some(empty)");
        assert_eq!(derived[0].kind, "done-criteria");
        assert_eq!(
            derived[0].reference, "g",
            "evidence references only the goal id, never raw goal text",
        );
    }

    #[test]
    fn criteria_only_pin_without_any_wip_ref_is_still_derivable() {
        // The "unrepairable/flag" case the reviews flagged (B1/B4): a pin that
        // binds NO measurable wip-ref (no pr/issue) still writes the finish line.
        // stuck_evidence() would be empty for such a goal, so derive_criteria is
        // the ONLY thing standing between it and an UNCLEAR-CRITERIA re-block — it
        // must recognise the finish line marker.
        let mut g = goal_with_desc("Improve the daemon's cognition somehow.");
        crate::goal_board_store::DoneGatePin {
            pr: None,
            issue: None,
            criteria: Some("the overseer signs off on the cognition rubric".into()),
        }
        .apply_to(&mut g);
        assert!(
            g.wip_refs.is_empty(),
            "precondition: a criteria-only pin binds no wip-ref"
        );
        assert!(
            derive_criteria(&g).is_some(),
            "a criteria-only done-gate finish line must still un-stick the goal (B4)"
        );
    }
}
