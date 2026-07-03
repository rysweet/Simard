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
//! coordination constraint (the `ooda_brain`/reasoner/bridge files are owned by
//! the naming-cleanup rename, so they are left untouched).
//!
//! See `docs/concepts/steerable-ooda-daemon.md` ("The no-progress breaker
//! (Fix 3)").

use std::collections::{HashMap, HashSet};

use super::completion_gate::{CompletionEvidenceGate, CompletionVerdict, EvidenceSource};
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

/// True when `reason` was authored by the no-progress breaker (both sentinel
/// halves present). Distinct from the brain-failure marker.
pub fn is_no_progress_marker(reason: &str) -> bool {
    reason.starts_with(NO_PROGRESS_BLOCKED_PREFIX) && reason.contains(NO_PROGRESS_BLOCKED_SUFFIX)
}

/// Render the sentinel [`GoalProgress::Blocked`] reason for a goal escalated
/// after `consecutive` no-action cycles.
///
/// [`GoalProgress::Blocked`]: super::types::GoalProgress::Blocked
pub fn no_progress_blocked_reason(consecutive: u32) -> String {
    format!("{NO_PROGRESS_BLOCKED_PREFIX}{consecutive}{NO_PROGRESS_BLOCKED_SUFFIX}")
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
    /// Threshold reached and unresolved — the caller files `issue_title` /
    /// `issue_body` as a tracking issue and sets the goal
    /// [`GoalProgress::Blocked`](super::types::GoalProgress::Blocked) to
    /// `blocked_reason` (the [`is_no_progress_marker`] sentinel).
    Escalate {
        blocked_reason: String,
        issue_title: String,
        issue_body: String,
    },
}

impl NoProgressResolution {
    /// `true` for every resolution except [`NoProgressResolution::Continue`] —
    /// i.e. the breaker fired and the goal is leaving the no-action loop.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Continue)
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

/// Per-goal consecutive no-action counter that drives the breaker.
///
/// Mirrors `OodaState.goal_failure_counts` but tracks the *no-action* livelock:
/// [`record_no_action`](Self::record_no_action) bumps a goal's count,
/// [`record_progress`](Self::record_progress) resets it after concrete progress,
/// and [`record_and_resolve`](Self::record_and_resolve) folds "bump then decide"
/// into one call, clearing the counter once the breaker fires.
#[derive(Debug, Default, Clone)]
pub struct NoProgressTracker {
    counts: HashMap<String, u32>,
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
    /// commit, a PR, an accepted progress bump).
    pub fn record_progress(&mut self, goal_id: &str) {
        self.counts.remove(goal_id);
    }

    /// Current consecutive no-action count for `goal_id` (`0` when untracked).
    pub fn consecutive(&self, goal_id: &str) -> u32 {
        self.counts.get(goal_id).copied().unwrap_or(0)
    }

    /// Drop counters for goals no longer on the board (mirrors the
    /// `OodaState.goal_failure_counts` pruning), so stale ids cannot leak.
    pub fn retain_goals(&mut self, live: &HashSet<String>) {
        self.counts.retain(|id, _| live.contains(id));
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
