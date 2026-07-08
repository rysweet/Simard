//! OBSERVE-ONLY Act phase for read-only identities (Simard #3125).
//!
//! When the active identity's posture is [`WriteAuthority::ReadOnly`], the Act
//! phase runs this observe-only branch instead of the engineer-dispatching
//! one. The branch:
//!   * consults an agentic [`ObserveOnlyBrain`] over the identity's TARGET repo
//!     set (what to observe / what goals to propose is a reasoner decision —
//!     kept agentic, with NO caller-imposed wall-clock timeout),
//!   * records the returned observations as `[simard]` operator diagnostics, and
//!   * appends the proposed goals to the board scoped to the identity's targets
//!     (`repo = Some(target)`), UNASSIGNED — it NEVER calls
//!     `dispatch_spawn_engineer` and NEVER writes to a target repo.
//!
//! Fail-closed contract (no fallbacks / no silent degradation):
//!   * a failing observe brain surfaces its error — it must NOT fall back to
//!     the engineer-dispatching Act phase, and
//!   * a proposal whose repo is absent or outside `targets` is a hard error
//!     (never silently re-scoped to `rysweet/Simard`); the whole pass is
//!     validated BEFORE any board mutation, so a rejected proposal leaves the
//!     board untouched.

use crate::error::{SimardError, SimardResult};
use crate::goal_curation::{ActiveGoal, GoalProgress};
use crate::identity::SeedGoal;
use crate::ooda_loop::{ActionOutcome, OodaState, PlannedAction};

/// The agentic reasoner for the observe-only Act phase.
///
/// `observe` decides — over the identity's target repo set — what to observe
/// and what goals to propose. It is deliberately a trait so the intelligence
/// can live in a prompt/reasoner; the deterministic rail around it (scope
/// validation + no engineer dispatch) lives in [`act_observe_only`]. No
/// caller-imposed wall-clock timeout is applied to `observe`.
pub trait ObserveOnlyBrain: Send + Sync {
    fn observe(&self, targets: &[String]) -> SimardResult<ObserveOutcome>;
}

/// The result of one observe-only reasoning pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ObserveOutcome {
    /// Human-readable observations about the target repos' health.
    pub observations: Vec<String>,
    /// Target-scoped goals proposed for the identity's own board.
    pub proposals: Vec<SeedGoal>,
}

/// Run one observe-only Act pass and return the number of goals proposed.
///
/// See the module docs for the full fail-closed contract. On any error the
/// board is left untouched.
pub(crate) fn act_observe_only(
    brain: &dyn ObserveOnlyBrain,
    targets: &[String],
    state: &mut OodaState,
) -> SimardResult<usize> {
    // Agentic step (NO fallback): a broken observe brain surfaces as an error
    // — never a silent fall-through to engineer dispatch.
    let outcome = brain.observe(targets)?;

    // Validate EVERY proposal before mutating the board so a fail-closed
    // rejection never leaks a partially-scoped goal (no implicit Simard scope).
    for proposal in &outcome.proposals {
        let scoped = proposal
            .repo
            .as_deref()
            .is_some_and(|repo| targets.iter().any(|t| t == repo));
        if !scoped {
            return Err(SimardError::InvalidGoalRecord {
                field: "repo".to_string(),
                reason: format!(
                    "observe-only proposal '{}' escapes the identity's target scope {:?} (repo {:?}); refusing to seed (issue #3125)",
                    proposal.title, targets, proposal.repo
                ),
            });
        }
    }

    // Record observations as operator diagnostics.
    for observation in &outcome.observations {
        eprintln!("[simard] OODA observe-only: {observation}");
    }

    // Append each proposal UNASSIGNED and target-scoped — no engineer dispatch.
    let proposed = outcome.proposals.len();
    for proposal in outcome.proposals {
        let id = crate::goals::goal_slug(&proposal.title);
        state.active_goals.active.push(ActiveGoal {
            parent_goal_id: None,
            id,
            description: proposal.description,
            priority: proposal.priority,
            status: GoalProgress::NotStarted,
            assigned_to: None,
            repo: proposal.repo,
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        });
    }

    if proposed > 0 {
        eprintln!("[simard] OODA observe-only: proposed {proposed} target-scoped goal(s)");
    }
    Ok(proposed)
}

/// Deterministic observe-only floor (Simard #3125).
///
/// The non-agentic baseline consulted when no prompt-backed observe brain is
/// wired: it records that it inspected the identity's targets and proposes no
/// new goals (the identity's declared seed goals were placed on the board at
/// boot by `seed_identity_board`). Mirrors the deterministic-floor pattern
/// used by the lifecycle / admission / decide brains. A prompt-backed
/// `ObserveOnlyBrain` can replace it without touching the rail.
pub struct DeterministicObserveBrain;

impl ObserveOnlyBrain for DeterministicObserveBrain {
    fn observe(&self, targets: &[String]) -> SimardResult<ObserveOutcome> {
        let observations = if targets.is_empty() {
            vec!["read-only identity with no target repos configured".to_string()]
        } else {
            vec![format!(
                "observed {} target repo(s) in read-only posture: {}",
                targets.len(),
                targets.join(", ")
            )]
        };
        Ok(ObserveOutcome {
            observations,
            proposals: Vec::new(),
        })
    }
}

/// Run the OBSERVE-ONLY Act phase for a read-only identity (Simard #3125).
///
/// Called by [`crate::ooda_loop::act`] when `state.write_authority` is
/// read-only. Runs the deterministic observe floor over the identity's targets
/// (recording observations + proposing target-scoped goals) and returns one
/// benign observe-only [`ActionOutcome`] per planned action. It NEVER calls
/// `dispatch_spawn_engineer` — the whole point is that a read-only observer
/// spends zero AI credits on write-bearing engineer dispatch the guardrail
/// would block. Fail-closed: an observe-brain error propagates (the cycle
/// fails loudly rather than silently reverting to engineer dispatch).
pub(crate) fn run_observe_only_act(
    actions: &[PlannedAction],
    state: &mut OodaState,
) -> SimardResult<Vec<ActionOutcome>> {
    let targets = state.observer_targets.clone();
    let brain = DeterministicObserveBrain;
    let proposed = act_observe_only(&brain, &targets, state)?;

    let outcomes = actions
        .iter()
        .map(|action| ActionOutcome {
            action: action.clone(),
            success: true,
            detail: format!(
                "observe-only: identity posture is read-only; recorded observations, proposed {proposed} target-scoped goal(s), dispatched 0 engineers (issue #3125)"
            ),
        })
        .collect();
    Ok(outcomes)
}
