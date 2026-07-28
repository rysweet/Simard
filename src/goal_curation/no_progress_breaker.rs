//! No-progress breaker (Fix 3): bound consecutive no-action cycles per goal.
//!
//! # The livelock this closes
//!
//! A healthy OODA brain can *succeed* and still make zero progress: it emits a
//! well-formed decision whose content is "take no action, I'll verify later"
//! (`NO ACTION`). Classified by [`crate::ooda_actions::goal_session`] as
//! [`GoalAction::NoAction`](crate::ooda_actions), each such cycle was recorded
//! as a `success = true` no-op — so nothing counted them and nothing forced a
//! resolution. The daemon re-selected the same "done" supply-chain goals every
//! cycle and emitted "I'll break the loop by verifying concretely…" forever.
//!
//! # The breaker
//!
//! This module tracks, per goal, the number of *consecutive* no-action cycles
//! ([`NoProgressTracker`], mirroring `OodaState.goal_failure_counts` but for the
//! *no-action* livelock rather than the *brain-failure* one). After a small
//! threshold ([`NO_PROGRESS_BREAKER_THRESHOLD`]) it runs the concrete
//! verification **once** and commits to a definitive outcome via the ladder in
//! [`resolve_no_progress`]:
//!
//! ```text
//! consecutive no-action cycles on goal G reaches N
//!         │
//!         ▼
//! run the done-gate verification ONCE (not "I'll verify later")
//!         │
//!         ├─ evidence present  ──►  MarkDone   (Fix 2 done-gate)
//!         ├─ goal obsolete     ──►  Drop       (out-of-scope / tracked elsewhere)
//!         └─ neither           ──►  Escalate   (file an issue + Block the goal)
//! ```
//!
//! The verification reuses the Fix-2 [`CompletionEvidenceGate`] via
//! [`verify_stuck_goal`], so "verify concretely" means "ask the injected
//! [`EvidenceSource`] whether the referenced PR is merged / the issue is closed
//! / the self-change is deployed", then commit to the answer. There is no
//! fourth "I'll verify again" branch.
//!
//! The module is **pure**: side effects (marking the board, filing the GitHub
//! issue, logging) are performed by the caller from the returned
//! [`NoProgressResolution`], exactly as the completion-gate archive path leaves
//! its `(archived, blocked)` side effects to its caller. This keeps the breaker
//! hermetically testable and contained to `src/goal_curation/` — the incident's
//! coordination constraint (the `ooda_brain`/reasoner/memory files are owned by
//! the naming-cleanup rename, so they are left untouched).
//!
//! See `docs/concepts/steerable-ooda-daemon.md` ("The no-progress breaker
//! (Fix 3)").

use std::collections::{HashMap, HashSet};

use super::completion_gate::{CompletionEvidenceGate, CompletionVerdict, EvidenceSource};
use super::no_progress_why::{Evidence, NoProgressClass, NoProgressWhy};
use super::types::ActiveGoal;

/// Consecutive no-action cycles on one goal before the breaker fires. Kept
/// deliberately small (2–3) so a livelock is broken quickly, matching the
/// brain-failure safeguard's 3-cycle threshold.
pub const NO_PROGRESS_BREAKER_THRESHOLD: u32 = 3;

/// Sentinel prefix for a breaker-authored [`GoalProgress::Blocked`] reason.
///
/// Mirrors [`BRAIN_FAILURE_BLOCKED_PREFIX`](crate::ooda_actions) in shape (the
/// `U+1F512` lock + `[OODA-SAFEGUARD]` token) so the same auto-recovery and
/// `simard goal unblock-all` machinery can recognise safeguard-authored blocks
/// and distinguish them from operator-set, scope-blocked, or dependency-blocked
/// reasons.
///
/// [`GoalProgress::Blocked`]: super::types::GoalProgress::Blocked
pub const NO_PROGRESS_BLOCKED_PREFIX: &str =
    "\u{1F512} [OODA-SAFEGUARD] OODA goal made no shippable progress for ";

/// Sentinel suffix for a breaker-authored blocked reason. Rendered as
/// `{PREFIX}{count}{SUFFIX}`.
pub const NO_PROGRESS_BLOCKED_SUFFIX: &str = " consecutive no-action cycles; needs human review";

/// Fold a churny [`ActiveGoal.id`](super::types::ActiveGoal) into a stable,
/// injection-safe identity token: the first 16 lowercase hex characters of
/// `sha256(goal_id)` (the first 8 digest bytes).
///
/// Pure and total. Mirrors the folding shape of
/// [`crate::stewardship::failure_signature`] so a volatile id collapses to one
/// deterministic `[0-9a-f]{16}` key that two OODA cycles on the same goal share.
///
/// Because the output is a fixed-charset hex literal it is safe to interpolate
/// into a `gh --search` query argument (SR1): it can never carry whitespace,
/// quotes, or GitHub search qualifiers (`is:`, `label:`, `in:`) that a raw,
/// attacker-influenced goal id could smuggle into the open-issue backstop's
/// dedup check. It is a one-way hash, not an encoding: no fragment of the raw id
/// survives into the folded key.
pub(crate) fn fold_goal_identity(goal_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(goal_id.as_bytes());
    let mut out = String::with_capacity(16);
    for b in &digest[..8] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// True when `reason` was authored by the no-progress breaker.
///
/// Keys on the globally-unique [`NO_PROGRESS_BLOCKED_PREFIX`] sentinel **alone**
/// (issue #16): the base breaker rendered `{PREFIX}{count}{SUFFIX}`, but the
/// root-cause upgrade appends a WHY segment in place of the bare
/// `needs human review` suffix, so recognition must not depend on that suffix.
/// The prefix (the `🔒 [OODA-SAFEGUARD]` lock token) still uniquely identifies a
/// safeguard-authored block and distinguishes it from operator-set, scope-, or
/// dependency-blocked reasons, so `simard goal unblock-all`, the load-time
/// self-heal, and the overseer count-parser keep working unchanged on both the
/// legacy and the WHY-bearing strings.
pub fn is_no_progress_marker(reason: &str) -> bool {
    reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX)
}

/// Translate a stored block `reason` into a PLAIN-ENGLISH sentence for operator
/// surfaces — the dashboard and any human-facing feed (issue #4276). A
/// safeguard-authored marker (`🔒 [OODA-SAFEGUARD] … why=… evidence=[…]`) is
/// opaque jargon to a person; this renders it as a plain sentence, free of every
/// machine token (`OODA-SAFEGUARD`, `UNCLEAR-CRITERIA`, `evidence=[`, `why=`, the
/// 🔒 lock). A NON-marker reason (an operator-set or dependency block) is already
/// human-readable and returned unchanged.
pub fn humanize_block_reason(reason: &str) -> String {
    if !is_no_progress_marker(reason) {
        return reason.to_string();
    }
    // Marker shape: `{PREFIX}{count}{SUFFIX}` (legacy) or
    // `{PREFIX}{count} consecutive no-action cycles; why={TOKEN} evidence=[…]`.
    // Extract the leading consecutive-cycle count without surfacing the marker.
    let tail = reason
        .strip_prefix(NO_PROGRESS_BLOCKED_PREFIX)
        .unwrap_or(reason);
    let count: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    let cycles = if count.is_empty() {
        String::new()
    } else {
        format!(" for {count} cycles")
    };
    // Detect the unclear/unmeasurable-criteria class for a more specific sentence,
    // WITHOUT ever echoing the class token to the operator.
    if reason.contains(crate::goal_curation::NoProgressClass::UnclearCriteria.token()) {
        format!(
            "Simard couldn't tell when this goal is finished{cycles}, so it made no \
             shippable progress. It needs a checkable finish condition (a specific \
             issue CLOSED, PR MERGED, or file/command the done-gate can verify)."
        )
    } else {
        format!(
            "Simard couldn't make shippable progress on this goal{cycles} and needs \
             human review to re-scope or unblock it."
        )
    }
}

/// True when `reason` is a **bare** no-progress safeguard block (issue #17): it
/// carries the [`NO_PROGRESS_BLOCKED_PREFIX`] marker (so
/// [`is_no_progress_marker`] holds) but has **no** [`NoProgressClass`] WHY token
/// attached — i.e. the legacy `{PREFIX}{count}{SUFFIX}` "needs human review"
/// shape a pre-#17 daemon parked stalled goals with, or a block a reasoner error
/// left un-classified.
///
/// This is the thin deterministic rail that gates the agentic re-investigation
/// pass ([`crate::ooda_loop::no_progress::reinvestigate_bare_blocked_goals`]): it
/// selects exactly the bare-blocked population and never mistakes a WHY-bearing
/// block (authored by [`no_progress_blocked_reason_with_why`], which always
/// embeds a [`NoProgressClass::token`]) or any other kind of block
/// (operator-set, scope, dependency, brain-failure) for one. Keying on "marker
/// present AND no class token" makes the WHY-rewrite its own idempotency
/// guarantee: once re-investigation attaches a WHY the reason is no longer bare,
/// so the pass never re-processes it.
pub fn is_bare_no_progress_block(reason: &str) -> bool {
    is_no_progress_marker(reason)
        && !NoProgressClass::ALL
            .iter()
            .any(|class| reason.contains(class.token()))
}

/// The evidence-less `(none)` variant of a safeguard block — the exact
/// live-daemon defect (verified 2026-07-15): a WHY-bearing block whose evidence
/// rendered `(none)` because the goal never produced a tracked issue/PR (the six
/// `simard-identity-*` goals, the coverage/coin/parity goals). Shape:
/// `🔒 [OODA-SAFEGUARD] … why=GENUINELY-STUCK evidence=[(none)]`.
///
/// Crucially this is NOT [`is_bare_no_progress_block`] — it carries a class
/// token, so the legacy "bare" predicate skips it, which is precisely why ~12–13
/// goals stayed stranded with a generic, evidence-free stamp. The
/// re-investigation pass must treat it as a first-class member of its population
/// so the goal is driven away from `(none)` (to a concrete WHY, a fixer, or a
/// surfaced investigation failure) rather than parked forever.
///
/// Keys on the marker plus the literal `evidence=[(none)]` segment authored by
/// [`no_progress_blocked_reason_with_why`] via [`NoProgressWhy::render_evidence`]
/// — so once re-investigation attaches real evidence (or unblocks the goal) the
/// reason no longer matches and the pass never re-processes it (idempotent).
pub fn is_evidenceless_no_progress_block(reason: &str) -> bool {
    is_no_progress_marker(reason) && reason.contains("evidence=[(none)]")
}

/// True when `reason` is a safeguard block the re-investigation pass must
/// re-examine (issue #16/#17): either a legacy **bare** block
/// ([`is_bare_no_progress_block`]) or an evidence-less `(none)` block
/// ([`is_evidenceless_no_progress_block`]). Both denote a goal parked WITHOUT a
/// concrete, evidence-backed WHY — the population that must never be left
/// stranded.
pub fn needs_reinvestigation(reason: &str) -> bool {
    is_bare_no_progress_block(reason) || is_evidenceless_no_progress_block(reason)
}

/// Render the sentinel [`GoalProgress::Blocked`] reason for a goal escalated
/// after `consecutive` no-action cycles.
///
/// Retained for existing callers/tests and as the legacy shape recognised by the
/// self-heal path. New escalations author the richer
/// [`no_progress_blocked_reason_with_why`] instead.
///
/// [`GoalProgress::Blocked`]: super::types::GoalProgress::Blocked
pub fn no_progress_blocked_reason(consecutive: u32) -> String {
    format!("{NO_PROGRESS_BLOCKED_PREFIX}{consecutive}{NO_PROGRESS_BLOCKED_SUFFIX}")
}

/// Render a **WHY-bearing** escalation reason (issue #16): the safeguard sentinel
/// with the classified root cause and its evidence attached, so a human block is
/// never bare.
///
/// Shape: `{PREFIX}{consecutive} consecutive no-action cycles; why={TOKEN} evidence=[…]`.
///
/// Invariants (asserted by tests):
/// - **starts with** [`NO_PROGRESS_BLOCKED_PREFIX`] → [`is_no_progress_marker`]
///   stays `true` and the load-time self-heal still recognises it;
/// - the leading digits after the prefix still parse as `consecutive`, so the
///   overseer's `{prefix}{count}` count-parser is undisturbed;
/// - it **contains** the class token and the evidence, and is strictly richer
///   than the bare [`no_progress_blocked_reason`] — never a "needs human review"
///   with no diagnosis.
pub fn no_progress_blocked_reason_with_why(consecutive: u32, why: &NoProgressWhy) -> String {
    format!(
        "{NO_PROGRESS_BLOCKED_PREFIX}{consecutive} consecutive no-action cycles; \
         why={} evidence=[{}]",
        why.class.token(),
        why.render_evidence(),
    )
}

/// The verified disposition of a stuck goal at the breaker threshold, computed
/// by running the done-gate **once** (see [`verify_stuck_goal`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StuckGoalDisposition {
    /// The done-gate certified the goal complete — hard evidence is present.
    Done,
    /// The goal is obsolete: its work is tracked elsewhere / out of scope, so it
    /// should leave the active board without a completion claim.
    Obsolete { reason: String },
    /// Neither done nor obsolete — a derivable signal refutes completion (or the
    /// state is unverifiable), and a human must resolve it.
    Unresolved,
}

/// The resolution the breaker selects for a goal that produced a no-action
/// cycle. Everything except [`NoProgressResolution::Continue`] is *terminal*:
/// the goal leaves the no-action loop and cannot accumulate another cycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoProgressResolution {
    /// Below the threshold — record the no-op and let the goal retry next cycle.
    Continue,
    /// Threshold reached with evidence present — mark the goal DONE via the
    /// done-gate (the caller archives it).
    MarkDone,
    /// Threshold reached and the goal is obsolete — DROP it from the active
    /// board (the caller removes it), carrying the human-readable reason.
    Drop { reason: String },
    /// `MISSING-PRECONDITION` (issue #16) — a machine-establishable precondition
    /// is absent (e.g. a governed repo was never cloned). The caller establishes
    /// it (clones the repo, or spawns an engineer to), resets the no-action
    /// counter, and lets the goal retry. **No block.**
    Heal { why: NoProgressWhy },
    /// `UPSTREAM-DEPENDENCY` (issue #16) — the goal is gated on a specific
    /// upstream that has not landed. The caller sets the goal
    /// [`GoalProgress::Paused`](super::types::GoalProgress::Paused), records
    /// `blocking_ref`, and lets the auto-clear pass resume it when the upstream
    /// resolves. **No block.**
    Defer {
        blocking_ref: String,
        evidence: Vec<Evidence>,
    },
    /// `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`, first occurrence (issue #16) — the
    /// caller spawns **one** guided engineer (via the shared dispatch) with
    /// `task` embedding the WHY, and records that the goal has spent its guided
    /// retry. **No block yet.**
    SpawnEngineer { task: String, why: NoProgressWhy },
    /// Threshold reached and unresolved — the caller files `issue_title` /
    /// `issue_body` as a tracking issue and sets the goal
    /// [`GoalProgress::Blocked`](super::types::GoalProgress::Blocked) to
    /// `blocked_reason`. Under the root-cause upgrade (issue #16) this is reached
    /// only after a spent guided retry, and `blocked_reason` carries the concrete
    /// WHY + evidence (see [`no_progress_blocked_reason_with_why`]) — never a
    /// bare "needs human review".
    Escalate {
        blocked_reason: String,
        issue_title: String,
        issue_body: String,
    },
    /// `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`, terminal rung reached but the
    /// independent investigation produced **no evidence** (issue #16). The old
    /// code stamped a bare `evidence=[(none)]` block here — the exact live-daemon
    /// defect (2026-07-15): the six `simard-identity-*` goals and the
    /// coverage/coin/parity goals never produced a tracked issue/PR, so their
    /// evidence rendered `(none)`. A goal must NEVER be parked with
    /// `evidence=[(none)]`: an evidence-less terminal outcome is itself a
    /// **surfaced investigation failure**, not a silent generic block. The caller
    /// records the goal in
    /// [`investigation_errors`](crate::ooda_loop::no_progress::NoProgressBreakerReport::investigation_errors),
    /// takes **no** terminal action, and leaves the goal retriable (fail
    /// visible + fail closed) so the next investigation can recover real evidence.
    ///
    /// `class` is carried so that once the surfaced-failure retries are bounded
    /// out (see
    /// [`SURFACED_INVESTIGATION_FAILURE_LIMIT`]) the human escalation names the
    /// accurate root cause (`UNCLEAR-CRITERIA` vs `GENUINELY-STUCK`) and tailors
    /// the "make the done-criteria measurable" ask to it.
    SurfaceInvestigationFailure {
        class: NoProgressClass,
        reason: String,
    },
}

/// How many **consecutive** evidence-less
/// [`SurfaceInvestigationFailure`](NoProgressResolution::SurfaceInvestigationFailure)
/// outcomes a single goal may accrue before the breaker stops re-investigating
/// and escalates it to a human (issue #16 follow-up).
///
/// `SurfaceInvestigationFailure` (issue #16) fixed the live defect of parking a
/// goal with a bare `evidence=[(none)]` block by making the evidence-less
/// terminal rung *non-terminal*: it resets the counter and lets the goal
/// re-investigate next cycle. But an *unbounded* re-investigation is its own
/// livelock — a goal whose done-criteria are **permanently** unclear (the six
/// `simard-identity-*` codename goals) re-investigates → produces no evidence →
/// surfaces → resets → forever, making **no shippable progress** and **never
/// reaching a human**. This bound closes that livelock: after this many
/// consecutive surfaced failures the goal is escalated to a human WITH the
/// re-investigation count as concrete evidence — so the never-`evidence=[(none)]`
/// invariant is preserved (the count is real evidence, not `(none)`) while the
/// spin is broken and a human is finally asked to make the done-criteria
/// measurable.
pub const SURFACED_INVESTIGATION_FAILURE_LIMIT: u32 = 3;

impl NoProgressResolution {
    /// `true` for every resolution that removes the goal from the no-action loop
    /// with a definitive action. [`Continue`](Self::Continue) (below threshold)
    /// and [`SurfaceInvestigationFailure`](Self::SurfaceInvestigationFailure) (an
    /// evidence-less terminal outcome that is surfaced and retried, taking NO
    /// terminal action) are the two non-terminal resolutions.
    pub fn is_terminal(&self) -> bool {
        !matches!(
            self,
            Self::Continue | Self::SurfaceInvestigationFailure { .. }
        )
    }
}

/// Case-insensitive substrings that mark a goal as obsolete / handed off. When
/// any appears in the goal's `current_activity` or an `issue` `wip_ref` label,
/// the breaker drops the goal instead of escalating it.
const OBSOLESCENCE_MARKERS: &[&str] = &[
    "out of scope",
    "out-of-scope",
    "superseded",
    "tracked elsewhere",
    "obsolete",
    "wontfix",
    "won't fix",
    "no longer needed",
];

/// Detect an explicit obsolescence / handoff signal on a stuck goal.
///
/// Returns a human reason when the goal's work has been determined out of scope
/// and tracked elsewhere (e.g. an out-of-scope issue was filed) — the "DROP"
/// branch of the ladder. Checks the goal's `current_activity` and its `issue`
/// `wip_ref` labels for any [`OBSOLESCENCE_MARKERS`] token.
pub fn obsolescence_reason(goal: &ActiveGoal) -> Option<String> {
    fn marker_in(text: &str) -> Option<&'static str> {
        let low = text.to_ascii_lowercase();
        OBSOLESCENCE_MARKERS
            .iter()
            .copied()
            .find(|m| low.contains(m))
    }

    if let Some(m) = goal.current_activity.as_deref().and_then(marker_in) {
        return Some(format!("goal marked '{m}' (tracked elsewhere)"));
    }
    for wip in &goal.wip_refs {
        if !wip.kind.eq_ignore_ascii_case("issue") {
            continue;
        }
        if let Some(m) = marker_in(&wip.label) {
            return Some(format!("out-of-scope issue #{} filed ('{m}')", wip.ref_id));
        }
    }
    None
}

/// Verify a stuck goal **once** against the Fix-2 done-gate and map the verdict
/// to a [`StuckGoalDisposition`]:
///
/// - gate says `Complete`                    → [`StuckGoalDisposition::Done`]
/// - gate `Blocked` and the goal is obsolete → [`StuckGoalDisposition::Obsolete`]
/// - gate `Blocked` otherwise                → [`StuckGoalDisposition::Unresolved`]
///
/// This is the concrete "verify, don't just say you'll verify" step at the
/// heart of the breaker.
pub fn verify_stuck_goal<E: EvidenceSource>(
    goal: &ActiveGoal,
    gate: &CompletionEvidenceGate<E>,
) -> StuckGoalDisposition {
    match gate.evaluate(goal) {
        CompletionVerdict::Complete(_) => StuckGoalDisposition::Done,
        CompletionVerdict::Blocked { .. } => match obsolescence_reason(goal) {
            Some(reason) => StuckGoalDisposition::Obsolete { reason },
            None => StuckGoalDisposition::Unresolved,
        },
    }
}

/// Build the escalation tracking-issue `(title, body)` for a goal blocked by the
/// breaker after `consecutive` no-action cycles.
fn escalation_issue(goal_id: &str, consecutive: u32) -> (String, String) {
    let title = format!(
        "OODA no-progress breaker: goal '{goal_id}' stuck ({consecutive} no-action cycles)"
    );
    let body = format!(
        "The OODA daemon produced **no shippable action** on goal `{goal_id}` for \
         {consecutive} consecutive cycles (repeated `NO ACTION` / \"I'll verify \
         concretely…\" responses).\n\n\
         The no-progress breaker ran the done-gate once: the goal is neither \
         verifiably complete (no merged PR + closed issue + deploy) nor obsolete \
         (no out-of-scope / tracked-elsewhere signal), so it has been marked \
         Blocked pending human review.\n\n\
         Inspect the goal's `wip_refs` and the relevant PR/issue, then either \
         supply the missing completion evidence, mark the goal out of scope, or \
         re-scope it.\n\n\
         Triggered by the deterministic safeguard in \
         `src/goal_curation/no_progress_breaker.rs` (Fix 3).",
    );
    (title, body)
}

/// The core policy: decide the resolution for a goal that produced a no-action
/// cycle.
///
/// `consecutive_no_progress` is the count **including** the current cycle. Below
/// `threshold` this returns [`NoProgressResolution::Continue`] and does **not**
/// consult `disposition`. At or above `threshold` it forces exactly one
/// definitive outcome by evaluating `disposition` (a closure so the concrete
/// verification runs **once**, only when the breaker actually fires — never on
/// every no-action cycle).
pub fn resolve_no_progress(
    goal_id: &str,
    consecutive_no_progress: u32,
    threshold: u32,
    disposition: impl FnOnce() -> StuckGoalDisposition,
) -> NoProgressResolution {
    if consecutive_no_progress < threshold {
        return NoProgressResolution::Continue;
    }
    match disposition() {
        StuckGoalDisposition::Done => NoProgressResolution::MarkDone,
        StuckGoalDisposition::Obsolete { reason } => NoProgressResolution::Drop { reason },
        StuckGoalDisposition::Unresolved => {
            let (issue_title, issue_body) = escalation_issue(goal_id, consecutive_no_progress);
            NoProgressResolution::Escalate {
                blocked_reason: no_progress_blocked_reason(consecutive_no_progress),
                issue_title,
                issue_body,
            }
        }
    }
}

/// Build the guided-engineer task for an `UNCLEAR-CRITERIA` / `GENUINELY-STUCK`
/// stall, embedding the classified WHY + evidence so the engineer starts from the
/// diagnosis rather than a cold read.
fn engineer_task_for_why(why: &NoProgressWhy) -> String {
    format!(
        "Prior OODA cycles stalled with no shippable progress. Diagnosed root \
         cause: why={} evidence=[{}]. Investigate and fix this specific WHY \
         (clarify and make the done-criteria measurable if they are unclear), \
         then advance the goal.",
        why.class.token(),
        why.render_evidence(),
    )
}

/// Build the WHY-bearing escalation `(title, body)` for a goal that stalled again
/// after its guided engineer retry.
fn why_escalation_issue(consecutive: u32, why: &NoProgressWhy) -> (String, String) {
    let token = why.class.token();
    let title = format!("OODA no-progress breaker: goal stuck after guided retry ({token})");
    let body = format!(
        "The OODA daemon produced **no shippable action** for {consecutive} \
         consecutive cycles and a guided engineer retry did not resolve it.\n\n\
         Root cause: **{token}**\n\
         Evidence: {}\n\n\
         The goal has been marked Blocked with the WHY attached (never a bare \
         \"needs human review\"). Inspect the evidence above and either supply the \
         missing completion evidence, re-scope the goal with measurable \
         done-criteria, or mark it out of scope.\n\n\
         Triggered by the root-cause safeguard in \
         `src/goal_curation/no_progress_breaker.rs` (issue #16).",
        why.render_evidence(),
    );
    (title, body)
}

/// Build the escalation `(title, body)` for a goal whose evidence-less
/// re-investigation was bounded out (issue #16 follow-up): after
/// [`SURFACED_INVESTIGATION_FAILURE_LIMIT`] consecutive surfaced failures the
/// breaker stops spinning and asks a **human** to make the done-criteria
/// *measurable* so the done-gate can eventually certify (or the operator can
/// re-scope / drop). The body is deliberately concrete and actionable — it names
/// the exact, machine-checkable shapes the daemon can verify.
pub(crate) fn surfaced_failure_escalation_issue(
    goal_id: &str,
    class: NoProgressClass,
    surfaced_failures: u32,
) -> (String, String) {
    let token = class.token();
    let title = format!(
        "OODA no-progress breaker: goal '{goal_id}' has unmeasurable done-criteria ({token})"
    );
    let clarify = match class {
        NoProgressClass::UnclearCriteria => {
            "The goal's done-criteria are **not expressed as anything the done-gate \
             can machine-check**, so the daemon can never certify it complete and \
             its re-investigation produced no evidence every cycle."
        }
        _ => {
            "The daemon found **no machine-resolvable cause** across repeated \
             independent investigations, and each produced no evidence."
        }
    };
    let body = format!(
        "The OODA no-progress breaker re-investigated goal `{goal_id}` \
         **{surfaced_failures} consecutive times** and each investigation classified \
         it **{token}** while producing **no supporting evidence**. To avoid an \
         unbounded evidence-less re-investigation livelock (which makes no shippable \
         progress and never reaches a human), the goal has now been Blocked and \
         surfaced for human triage.\n\n\
         {clarify}\n\n\
         **Action — make the done-criteria measurable.** Re-scope the goal so \
         completion is machine-verifiable, e.g. one of:\n\
         - a specific issue the daemon can observe as `CLOSED`,\n\
         - a specific PR the daemon can observe as `MERGED`,\n\
         - a specific file/command whose presence or output the done-gate can check.\n\n\
         Alternatively, drop the goal if it is out of scope. The block reason \
         carries the re-investigation count as evidence (never a bare \
         `evidence=[(none)]`).\n\n\
         Triggered by the surfaced-failure bound in \
         `src/goal_curation/no_progress_breaker.rs` \
         (`SURFACED_INVESTIGATION_FAILURE_LIMIT`)."
    );
    (title, body)
}

/// Map a classified [`NoProgressWhy`] to the resolution the breaker takes at the
/// threshold (issue #16). This is the **pure** heart of the root-cause ladder;
/// the side effects (transition, clone, defer, spawn, escalate) are performed by
/// the caller in `crate::ooda_loop::no_progress`.
///
/// `consecutive` is the count that renders into the escalation reason/issue.
/// `guided_retry_used` is the goal's persisted one-shot flag: the first time an
/// `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` stall reaches here it spawns a guided
/// engineer ([`NoProgressResolution::SpawnEngineer`]); only once that retry is
/// spent and the goal is *still* stuck does it [`Escalate`](NoProgressResolution::Escalate)
/// — always WITH the concrete WHY + evidence attached.
pub fn resolution_for_why(
    consecutive: u32,
    why: NoProgressWhy,
    guided_retry_used: bool,
) -> NoProgressResolution {
    match why.class {
        NoProgressClass::AlreadyComplete => NoProgressResolution::MarkDone,
        NoProgressClass::Obsolete => NoProgressResolution::Drop {
            reason: format!("obsolete: {}", why.render_evidence()),
        },
        NoProgressClass::MissingPrecondition => NoProgressResolution::Heal { why },
        NoProgressClass::UpstreamDependency => {
            let blocking_ref = why.blocking_ref();
            NoProgressResolution::Defer {
                blocking_ref,
                evidence: why.evidence,
            }
        }
        NoProgressClass::UnclearCriteria | NoProgressClass::GenuinelyStuck => {
            if guided_retry_used {
                // Terminal rung: the guided (independent) investigation is spent
                // and the goal is still stuck. NEVER author a bare
                // `evidence=[(none)]` block — the exact live-daemon defect. If the
                // investigation produced concrete evidence, escalate WITH it;
                // otherwise surface the evidence-less outcome as an investigation
                // failure (fail visible + retriable), taking no bare terminal
                // action.
                if why.evidence.is_empty() {
                    return NoProgressResolution::SurfaceInvestigationFailure {
                        class: why.class,
                        reason: format!(
                            "independent investigation of a stalled goal classified {} but \
                             produced no supporting evidence at the terminal rung — refusing to \
                             park it with an empty-evidence block; surfaced for retry",
                            why.class.token(),
                        ),
                    };
                }
                let blocked_reason = no_progress_blocked_reason_with_why(consecutive, &why);
                let (issue_title, issue_body) = why_escalation_issue(consecutive, &why);
                NoProgressResolution::Escalate {
                    blocked_reason,
                    issue_title,
                    issue_body,
                }
            } else {
                let task = engineer_task_for_why(&why);
                NoProgressResolution::SpawnEngineer { task, why }
            }
        }
    }
}

/// Per-goal consecutive no-action counter that drives the breaker.
///
/// Mirrors `OodaState.goal_failure_counts` but tracks the *no-action* livelock:
/// [`record_no_action`](Self::record_no_action) bumps a goal's count,
/// [`record_progress`](Self::record_progress) resets it after concrete progress,
/// and [`record_and_resolve`](Self::record_and_resolve) folds "bump then decide"
/// into one call, clearing the counter once the breaker fires.
///
/// `Serialize`/`Deserialize` so the counter survives daemon restarts alongside
/// `OodaState.goal_failure_counts` (a livelock spanning a restart must still be
/// bounded). `#[serde(default)]` on the field keeps snapshots written before
/// this counter existed deserializable.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NoProgressTracker {
    #[serde(default)]
    counts: HashMap<String, u32>,
    /// Goals that have already spent their **one** guided-engineer retry (issue
    /// #16). An `UNCLEAR-CRITERIA` / `GENUINELY-STUCK` stall spawns a guided
    /// engineer the first time it reaches the threshold; if it stalls again with
    /// this flag set it escalates (WITH the WHY) instead of spawning a second
    /// engineer. `#[serde(default)]` keeps pre-#16 snapshots deserializable.
    #[serde(default)]
    guided_retries: HashSet<String>,
    /// Goals whose **bare** no-progress block has already been re-investigated to
    /// a terminal resolution for a given [`NoProgressClass`] (issue #17). Keyed on
    /// `(goal_id, class_token)`; the class is stored as its stable
    /// [`NoProgressClass::token`] **string** (never an enum-tagged form) so an
    /// older / rolled-back binary can still parse the snapshot and the
    /// fail-to-empty goal-board store never turns a parse miss into a full board
    /// wipe. This is the belt-and-suspenders dedupe: the WHY-rewrite already
    /// removes a re-investigated goal from the bare population next cycle, but this
    /// persisted set additionally bounds re-investigation to **one** terminal
    /// action per `(goal, class)` even if a crash/restart re-parks the goal bare
    /// between the board rewrite and the tracker persist. `#[serde(default)]`
    /// keeps pre-#17 snapshots deserializable (loads as an empty set).
    #[serde(default)]
    reinvestigated: HashSet<(String, String)>,
    /// Per-goal count of **consecutive** evidence-less
    /// [`SurfaceInvestigationFailure`](NoProgressResolution::SurfaceInvestigationFailure)
    /// outcomes (issue #16 follow-up). Bumped every time the breaker surfaces an
    /// evidence-less terminal failure for a goal and reset the instant the goal
    /// makes real progress. Once it reaches
    /// [`SURFACED_INVESTIGATION_FAILURE_LIMIT`] the breaker stops re-investigating
    /// and escalates the goal to a human, breaking the unbounded re-investigation
    /// livelock. `#[serde(default)]` keeps snapshots written before this counter
    /// existed deserializable (loads as an empty map).
    #[serde(default)]
    surfaced_failures: HashMap<String, u32>,
}

impl NoProgressTracker {
    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a no-action cycle for `goal_id`; returns the new consecutive count.
    pub fn record_no_action(&mut self, goal_id: &str) -> u32 {
        let entry = self.counts.entry(goal_id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Reset `goal_id`'s counter after concrete progress (an engineer spawn, a
    /// commit, a PR, an accepted progress bump). Also clears the goal's spent
    /// guided-retry flag: genuine progress means a *future* stall earns a fresh
    /// guided retry.
    pub fn record_progress(&mut self, goal_id: &str) {
        self.counts.remove(goal_id);
        self.guided_retries.remove(goal_id);
        self.reinvestigated.retain(|(id, _)| id != goal_id);
        self.surfaced_failures.remove(goal_id);
    }

    /// Record one evidence-less surfaced investigation failure for `goal_id` and
    /// return the new **consecutive** count (issue #16 follow-up). The breaker
    /// compares this against [`SURFACED_INVESTIGATION_FAILURE_LIMIT`] to decide
    /// whether to keep re-investigating (below the bound) or escalate to a human
    /// (at/above it), so an evidence-less goal can never re-investigate forever.
    pub fn record_surfaced_failure(&mut self, goal_id: &str) -> u32 {
        let entry = self
            .surfaced_failures
            .entry(goal_id.to_string())
            .or_insert(0);
        *entry += 1;
        *entry
    }

    /// Clear `goal_id`'s consecutive surfaced-failure count (issue #16 follow-up).
    /// Called once the goal is escalated out of the re-investigation loop so a
    /// later re-entry starts a fresh window rather than escalating immediately.
    pub fn clear_surfaced_failures(&mut self, goal_id: &str) {
        self.surfaced_failures.remove(goal_id);
    }

    /// Current consecutive surfaced-failure count for `goal_id` (`0` when
    /// untracked) — read by tests and the breaker's bound check.
    pub fn surfaced_failures(&self, goal_id: &str) -> u32 {
        self.surfaced_failures.get(goal_id).copied().unwrap_or(0)
    }

    /// Reset only `goal_id`'s no-action counter, **preserving** any spent
    /// guided-retry flag (issue #16). Used when the breaker gives the goal a
    /// fresh retry window that must NOT reset the one-shot guided-retry bound —
    /// e.g. after healing a precondition or spawning the guided engineer.
    pub fn reset_count(&mut self, goal_id: &str) {
        self.counts.remove(goal_id);
    }

    /// Mark that `goal_id` has spent its one guided-engineer retry (issue #16).
    pub fn mark_guided_retry(&mut self, goal_id: &str) {
        self.guided_retries.insert(goal_id.to_string());
    }

    /// Whether `goal_id` has already spent its one guided-engineer retry.
    pub fn guided_retry_used(&self, goal_id: &str) -> bool {
        self.guided_retries.contains(goal_id)
    }

    /// Record that `goal_id`'s bare no-progress block has been re-investigated to
    /// a terminal resolution for `class` (issue #17). Idempotent. Called **only**
    /// after a terminal action succeeds (never on a fail-closed reasoner error),
    /// so a re-park after a restart cannot trigger a second terminal action for
    /// the same `(goal, class)`.
    pub fn mark_reinvestigated(&mut self, goal_id: &str, class: NoProgressClass) {
        self.reinvestigated
            .insert((goal_id.to_string(), class.token().to_string()));
    }

    /// Whether `goal_id`'s bare block has already been re-investigated to a
    /// terminal resolution for `class` (issue #17) — the belt-and-suspenders
    /// dedupe guard the re-investigation pass consults before taking any terminal
    /// action, bounding it to one per `(goal, class)` across daemon restarts.
    pub fn reinvestigated(&self, goal_id: &str, class: NoProgressClass) -> bool {
        self.reinvestigated
            .contains(&(goal_id.to_string(), class.token().to_string()))
    }

    /// Current consecutive no-action count for `goal_id` (`0` when untracked).
    pub fn consecutive(&self, goal_id: &str) -> u32 {
        self.counts.get(goal_id).copied().unwrap_or(0)
    }

    /// Drop counters for goals no longer on the board (mirrors the
    /// `OodaState.goal_failure_counts` pruning), so stale ids cannot leak.
    pub fn retain_goals(&mut self, live: &HashSet<String>) {
        self.counts.retain(|id, _| live.contains(id));
        self.guided_retries.retain(|id| live.contains(id));
        self.reinvestigated.retain(|(id, _)| live.contains(id));
        self.surfaced_failures.retain(|id, _| live.contains(id));
    }

    /// Record a no-action cycle for `goal_id` and return the breaker's
    /// resolution.
    ///
    /// `disposition` is evaluated lazily — only when the count reaches
    /// `threshold` — so the concrete done-gate verification runs exactly once
    /// per breaker firing, not on every no-action cycle. When the breaker fires
    /// (any terminal resolution) the counter is cleared: the goal has left the
    /// no-action loop (done / dropped / blocked) and cannot accumulate an
    /// `(N+1)`th consecutive no-action cycle.
    pub fn record_and_resolve(
        &mut self,
        goal_id: &str,
        threshold: u32,
        disposition: impl FnOnce() -> StuckGoalDisposition,
    ) -> NoProgressResolution {
        let consecutive = self.record_no_action(goal_id);
        let resolution = resolve_no_progress(goal_id, consecutive, threshold, disposition);
        if resolution.is_terminal() {
            self.counts.remove(goal_id);
        }
        resolution
    }
}
