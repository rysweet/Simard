//! Per-cycle goal coverage allocator (issue #2359, BUG 2).
//!
//! Makes goal **coverage** a first-class allocation rule: every OODA cycle,
//! every active goal that is not-started or in-progress is guaranteed exactly
//! one live engineer — subject to the AIMD safety cap. Coverage takes
//! precedence over adding parallelism to an already-covered goal.
//!
//! Contract reference: `docs/reference/goal-coverage-allocation.md`.
//!
//! # TDD red-phase placeholder (issue #2359)
//!
//! The function bodies below are intentional stubs. The inline `#[cfg(test)]`
//! tests are written against the public contract and **must fail** in the red
//! phase (the stubs panic via `unimplemented!()`). They **must pass** once the
//! real allocator lands in the implementation step — without further test edits.

use std::collections::HashMap;

use super::types::{ActionKind, OodaState, PlannedAction};
use crate::goal_curation::{ActiveGoal, GoalProgress};

/// Summary of one cycle's coverage pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoverageReport {
    /// Incomplete goals that ended this cycle with an engineer
    /// (already-covered + newly covered this cycle).
    pub covered: usize,
    /// Total incomplete goals this cycle (`NotStarted` or `InProgress`).
    pub incomplete: usize,
    /// Uncovered incomplete goals that could not be covered because the cap
    /// was reached; covered on a subsequent cycle.
    pub deferred: usize,
}

impl CoverageReport {
    /// Operator log line:
    /// `"covered N/M incomplete goals, deferred K due to cap"`.
    pub fn log_line(&self) -> String {
        format!(
            "covered {}/{} incomplete goals, deferred {} due to cap",
            self.covered, self.incomplete, self.deferred,
        )
    }
}

/// A goal is **incomplete** — and therefore needs a live engineer — when its
/// status is `NotStarted` or `InProgress`. `Proposed`, `Paused`, `Blocked`
/// (operator hold), and `Completed` are all excluded.
fn is_incomplete(status: &GoalProgress) -> bool {
    matches!(
        status,
        GoalProgress::NotStarted | GoalProgress::InProgress { .. }
    )
}

/// Guarantees coverage: every incomplete goal that lacks a live engineer ends
/// the cycle with exactly one `AdvanceGoal` action, ordered by goal priority and
/// subject to `cap` as a hard ceiling.
///
/// - **Incomplete** = status `NotStarted` or `InProgress` (`Proposed`,
///   `Paused`, `Blocked`, `Completed` excluded).
/// - A goal with a live engineer (`assigned_to` set) is already covered; any
///   Decide-emitted action for it is *extra parallelism* and yields a contested
///   slot to coverage.
/// - A goal **without** a live engineer needs one surviving action: its
///   Decide-emitted spawn is reused when present (never double-spawned),
///   otherwise a coverage action is synthesized. These are ordered by priority
///   so the highest-priority goals win contested slots and a goal's own spawn is
///   never evicted by coverage of a lower-priority goal.
/// - The `cap` is a hard ceiling: `planned.len() <= cap` on return.
#[allow(clippy::ptr_arg)] // Needs &mut Vec: reorders coverage actions ahead of extra parallelism and truncates to cap.
pub fn ensure_goal_coverage(
    state: &OodaState,
    planned: &mut Vec<PlannedAction>,
    cap: usize,
) -> CoverageReport {
    let incomplete_goals: Vec<&ActiveGoal> = state
        .active_goals
        .active
        .iter()
        .filter(|g| is_incomplete(&g.status))
        .collect();
    let incomplete = incomplete_goals.len();

    // A live engineer (`assigned_to`) means the goal is already covered no
    // matter what; any Decide-emitted action for it is extra parallelism.
    let with_engineer = incomplete_goals
        .iter()
        .filter(|g| g.assigned_to.is_some())
        .count();

    // Goals without a live engineer each need exactly one surviving action this
    // cycle. Order them by priority (lower number = higher priority); the stable
    // sort keeps board order for equal priorities.
    let mut needs_coverage: Vec<&ActiveGoal> = incomplete_goals
        .iter()
        .copied()
        .filter(|g| g.assigned_to.is_none())
        .collect();
    needs_coverage.sort_by_key(|g| g.priority);

    // Claim each needs-coverage goal's first Decide-emitted `AdvanceGoal` as its
    // coverage action — reusing the planned spawn rather than double-spawning.
    // Every other planned action (extra parallelism, non-goal actions) keeps its
    // relative order and sits behind coverage.
    let slot_of: HashMap<&str, usize> = needs_coverage
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();
    let mut claimed: Vec<Option<PlannedAction>> = (0..needs_coverage.len()).map(|_| None).collect();
    let mut other: Vec<PlannedAction> = Vec::new();
    for action in std::mem::take(planned) {
        let claim = if action.kind == ActionKind::AdvanceGoal {
            action
                .goal_id
                .as_deref()
                .and_then(|gid| slot_of.get(gid).copied())
                .filter(|&slot| claimed[slot].is_none())
        } else {
            None
        };
        match claim {
            Some(slot) => claimed[slot] = Some(action),
            None => other.push(action),
        }
    }

    // One coverage action per needs-coverage goal, in priority order: the reused
    // Decide spawn or a synthesized one.
    let coverage_actions: Vec<PlannedAction> = needs_coverage
        .iter()
        .enumerate()
        .map(|(i, g)| {
            claimed[i].take().unwrap_or_else(|| PlannedAction {
                kind: ActionKind::AdvanceGoal,
                goal_id: Some(g.id.clone()),
                description: format!(
                    "coverage: ensure a live engineer for incomplete goal '{}'",
                    g.id
                ),
            })
        })
        .collect();

    // Coverage actions sit at the FRONT (highest priority first), so the first
    // `cap` survive truncation — coverage always wins a contested slot over extra
    // parallelism, and a higher-priority goal's spawn is never evicted by a
    // lower-priority goal's coverage.
    let total_needs_coverage = coverage_actions.len();
    let newly_covered = total_needs_coverage.min(cap);
    let deferred = total_needs_coverage - newly_covered;

    let mut combined = coverage_actions;
    combined.append(&mut other);
    combined.truncate(cap);
    *planned = combined;

    CoverageReport {
        covered: with_engineer + newly_covered,
        incomplete,
        deferred,
    }
}

#[cfg(test)]
mod tests {
    use super::{CoverageReport, ensure_goal_coverage};
    use crate::goal_curation::{ActiveGoal, GoalBoard, GoalProgress};
    use crate::ooda_loop::{ActionKind, OodaState, PlannedAction};

    // ── Fixtures ───────────────────────────────────────────────────────────

    fn goal(id: &str, priority: u32, status: GoalProgress, assigned: Option<&str>) -> ActiveGoal {
        ActiveGoal {
            parent_goal_id: None,
            priority_explicit: false,
            repo: None,
            id: id.to_string(),
            description: format!("desc for {id}"),
            priority,
            status,
            assigned_to: assigned.map(|s| s.to_string()),
            current_activity: None,
            wip_refs: vec![],
            last_progress_update_at: None,
        }
    }

    fn state_with(goals: Vec<ActiveGoal>) -> OodaState {
        let board = GoalBoard {
            active: goals,
            backlog: vec![],
        };
        OodaState::new(board)
    }

    fn advance(goal_id: &str) -> PlannedAction {
        PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some(goal_id.to_string()),
            description: format!("advance {goal_id}"),
        }
    }

    /// goal_ids of the `AdvanceGoal` actions in `planned`, in order.
    fn advance_goal_ids(planned: &[PlannedAction]) -> Vec<String> {
        planned
            .iter()
            .filter(|a| a.kind == ActionKind::AdvanceGoal)
            .filter_map(|a| a.goal_id.clone())
            .collect()
    }

    // ── Covering uncovered incomplete goals ────────────────────────────────

    #[test]
    fn covers_each_uncovered_incomplete_goal() {
        let state = state_with(vec![
            goal("g-b", 2, GoalProgress::NotStarted, None),
            goal("g-c", 3, GoalProgress::InProgress { percent: 40 }, None),
        ]);
        let mut planned: Vec<PlannedAction> = vec![];

        let report = ensure_goal_coverage(&state, &mut planned, 5);

        let ids = advance_goal_ids(&planned);
        assert!(
            ids.contains(&"g-b".to_string()) && ids.contains(&"g-c".to_string()),
            "both uncovered incomplete goals must get an AdvanceGoal action, got {ids:?}"
        );
        assert_eq!(report.incomplete, 2);
        assert_eq!(report.covered, 2);
        assert_eq!(report.deferred, 0);
    }

    #[test]
    fn skips_goals_with_a_live_assigned_engineer() {
        let state = state_with(vec![goal(
            "g-a",
            1,
            GoalProgress::InProgress { percent: 10 },
            Some("engineer-g-a-123"),
        )]);
        let mut planned: Vec<PlannedAction> = vec![];

        let report = ensure_goal_coverage(&state, &mut planned, 5);

        assert!(
            advance_goal_ids(&planned).is_empty(),
            "an already-covered (assigned) goal must NOT get a new action"
        );
        assert_eq!(report.incomplete, 1);
        assert_eq!(report.covered, 1, "the assigned goal is already covered");
        assert_eq!(report.deferred, 0);
    }

    #[test]
    fn skips_completed_blocked_paused_proposed_goals() {
        let state = state_with(vec![
            goal("g-done", 1, GoalProgress::Completed, None),
            goal(
                "g-blocked",
                2,
                GoalProgress::Blocked("operator hold".into()),
                None,
            ),
            goal("g-paused", 3, GoalProgress::Paused, None),
            goal("g-proposed", 4, GoalProgress::Proposed, None),
            goal("g-active", 5, GoalProgress::NotStarted, None),
        ]);
        let mut planned: Vec<PlannedAction> = vec![];

        let report = ensure_goal_coverage(&state, &mut planned, 5);

        assert_eq!(
            advance_goal_ids(&planned),
            vec!["g-active".to_string()],
            "only NotStarted/InProgress goals are covered; \
             Completed/Blocked/Paused/Proposed are excluded"
        );
        assert_eq!(report.incomplete, 1);
        assert_eq!(report.covered, 1);
        assert_eq!(report.deferred, 0);
    }

    // ── Ordering + cap ─────────────────────────────────────────────────────

    #[test]
    fn covers_in_ascending_priority_order() {
        let state = state_with(vec![
            goal("g-hi", 30, GoalProgress::NotStarted, None),
            goal("g-lo", 10, GoalProgress::NotStarted, None),
            goal("g-mid", 20, GoalProgress::NotStarted, None),
        ]);
        let mut planned: Vec<PlannedAction> = vec![];

        ensure_goal_coverage(&state, &mut planned, 5);

        assert_eq!(
            advance_goal_ids(&planned),
            vec!["g-lo".to_string(), "g-mid".to_string(), "g-hi".to_string()],
            "uncovered goals must be covered strictly in ascending priority order \
             (lower number = higher priority)"
        );
    }

    #[test]
    fn respects_cap_and_defers_lowest_priority() {
        let state = state_with(vec![
            goal("g1", 1, GoalProgress::NotStarted, None),
            goal("g2", 2, GoalProgress::NotStarted, None),
            goal("g3", 3, GoalProgress::NotStarted, None),
            goal("g4", 4, GoalProgress::NotStarted, None),
        ]);
        let mut planned: Vec<PlannedAction> = vec![];

        let report = ensure_goal_coverage(&state, &mut planned, 2);

        assert!(
            planned.len() <= 2,
            "the cap is a hard ceiling: planned.len() ({}) must be <= cap (2)",
            planned.len()
        );
        assert_eq!(
            advance_goal_ids(&planned),
            vec!["g1".to_string(), "g2".to_string()],
            "with cap=2 the two highest-priority uncovered goals are covered"
        );
        assert_eq!(report.incomplete, 4);
        assert_eq!(report.covered, 2);
        assert_eq!(
            report.deferred, 2,
            "the two lowest-priority goals are deferred"
        );
    }

    #[test]
    fn never_exceeds_cap() {
        let goals: Vec<ActiveGoal> = (0..6)
            .map(|i| goal(&format!("g{i}"), i, GoalProgress::NotStarted, None))
            .collect();
        let state = state_with(goals);
        let mut planned: Vec<PlannedAction> = vec![];

        ensure_goal_coverage(&state, &mut planned, 3);

        assert!(
            planned.len() <= 3,
            "planned.len() ({}) must never exceed the cap (3)",
            planned.len()
        );
    }

    // ── No double-spawn / coverage ≥ parallelism ───────────────────────────

    #[test]
    fn does_not_double_spawn_when_action_already_planned() {
        // The Decide phase already emitted an AdvanceGoal for g-b.
        let state = state_with(vec![goal("g-b", 2, GoalProgress::NotStarted, None)]);
        let mut planned: Vec<PlannedAction> = vec![advance("g-b")];

        let report = ensure_goal_coverage(&state, &mut planned, 5);

        let g_b_actions = advance_goal_ids(&planned)
            .into_iter()
            .filter(|id| id == "g-b")
            .count();
        assert_eq!(
            g_b_actions, 1,
            "coverage must not add a second action for a goal already planned by Decide"
        );
        assert_eq!(report.incomplete, 1);
        assert_eq!(report.covered, 1);
        assert_eq!(report.deferred, 0);
    }

    #[test]
    fn coverage_wins_a_contested_slot_over_extra_parallelism() {
        // g-a is already covered (live assigned engineer) but Decide also
        // emitted an extra-parallelism action for it. g-b is uncovered. With
        // cap=1, coverage of g-b must win the slot; g-a's extra action drops.
        let state = state_with(vec![
            goal(
                "g-a",
                1,
                GoalProgress::InProgress { percent: 50 },
                Some("engineer-g-a-1"),
            ),
            goal("g-b", 2, GoalProgress::NotStarted, None),
        ]);
        let mut planned: Vec<PlannedAction> = vec![advance("g-a")];

        let report = ensure_goal_coverage(&state, &mut planned, 1);

        assert_eq!(
            advance_goal_ids(&planned),
            vec!["g-b".to_string()],
            "coverage of the uncovered goal must win the single slot; the \
             extra-parallelism action for the already-covered goal is dropped"
        );
        assert_eq!(report.incomplete, 2);
        assert_eq!(
            report.covered, 2,
            "g-a stays covered via its live engineer and g-b is newly covered"
        );
        assert_eq!(report.deferred, 0);
    }

    #[test]
    fn unassigned_goal_decide_spawn_is_not_evicted_by_lower_priority_coverage() {
        // Regression: a Decide-emitted AdvanceGoal for an UNASSIGNED incomplete
        // goal is that goal's primary spawn, not extra parallelism. It must be
        // ordered by the goal's priority alongside synthesized coverage actions
        // — never unconditionally evicted by coverage of a lower-priority goal —
        // and the report must not over-count it as covered after eviction.
        let state = state_with(vec![
            goal("g-hi", 1, GoalProgress::NotStarted, None), // Decide surfaced this
            goal("g-mid", 2, GoalProgress::NotStarted, None), // unsurfaced, needs coverage
            goal("g-lo", 3, GoalProgress::NotStarted, None), // unsurfaced, needs coverage
        ]);
        // Decide planned only the highest-priority unassigned goal's spawn.
        let mut planned: Vec<PlannedAction> = vec![advance("g-hi")];

        let report = ensure_goal_coverage(&state, &mut planned, 2);

        assert_eq!(
            advance_goal_ids(&planned),
            vec!["g-hi".to_string(), "g-mid".to_string()],
            "the unassigned goal Decide already surfaced (highest priority) must \
             survive the cap; only the lowest-priority goal is deferred"
        );
        assert!(planned.len() <= 2, "cap is a hard ceiling");
        assert_eq!(report.incomplete, 3);
        assert_eq!(
            report.covered, 2,
            "exactly the two highest-priority goals are covered this cycle"
        );
        assert_eq!(
            report.deferred, 1,
            "the lowest-priority goal is deferred and re-covered next cycle"
        );
    }

    #[test]
    fn idempotent_when_all_incomplete_goals_already_covered() {
        let state = state_with(vec![
            goal(
                "g-a",
                1,
                GoalProgress::InProgress { percent: 20 },
                Some("eng-a"),
            ),
            goal("g-b", 2, GoalProgress::NotStarted, Some("eng-b")),
        ]);
        let mut planned: Vec<PlannedAction> = vec![];

        let report = ensure_goal_coverage(&state, &mut planned, 5);

        assert!(
            advance_goal_ids(&planned).is_empty(),
            "no new actions when every incomplete goal already has an engineer"
        );
        assert_eq!(report.incomplete, 2);
        assert_eq!(report.covered, 2);
        assert_eq!(report.deferred, 0);
    }

    // ── CoverageReport::log_line ───────────────────────────────────────────

    #[test]
    fn log_line_reports_deferred_due_to_cap() {
        let report = CoverageReport {
            covered: 4,
            incomplete: 5,
            deferred: 1,
        };
        assert_eq!(
            report.log_line(),
            "covered 4/5 incomplete goals, deferred 1 due to cap"
        );
    }

    #[test]
    fn log_line_reports_zero_deferred_when_all_fit() {
        let report = CoverageReport {
            covered: 3,
            incomplete: 3,
            deferred: 0,
        };
        assert_eq!(
            report.log_line(),
            "covered 3/3 incomplete goals, deferred 0 due to cap"
        );
    }
}
