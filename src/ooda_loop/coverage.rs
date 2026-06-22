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

use std::collections::HashSet;

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

/// Ensures coverage by **prepending** one `AdvanceGoal` action per uncovered
/// incomplete goal to the cycle's planned-action list (highest priority first),
/// then truncating the whole list to `cap`.
///
/// - **Incomplete** = status `NotStarted` or `InProgress` (`Proposed`,
///   `Paused`, `Blocked`, `Completed` excluded).
/// - **Covered** = a live engineer already exists for the goal (reuse the
///   existing in-flight detection; `assigned_to` set, or an action already
///   planned for the goal) — never double-spawn.
/// - The `cap` is a hard ceiling: `planned.len() <= cap` on return.
#[allow(clippy::ptr_arg)] // Needs &mut Vec: prepends coverage actions and truncates to cap.
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

    // Goals already covered by a Decide-emitted action this cycle. Reusing
    // these (alongside `assigned_to`) is the in-flight de-dup that prevents a
    // second engineer for the same goal.
    let planned_goal_ids: HashSet<&str> = planned
        .iter()
        .filter(|a| a.kind == ActionKind::AdvanceGoal)
        .filter_map(|a| a.goal_id.as_deref())
        .collect();

    let is_covered = |g: &ActiveGoal| -> bool {
        g.assigned_to.is_some() || planned_goal_ids.contains(g.id.as_str())
    };

    let already_covered = incomplete_goals.iter().filter(|g| is_covered(g)).count();

    // Uncovered incomplete goals, highest priority first (lower number = higher
    // priority). `sort_by_key` is stable, so equal-priority goals keep board
    // order.
    let mut uncovered: Vec<&ActiveGoal> = incomplete_goals
        .iter()
        .copied()
        .filter(|g| !is_covered(g))
        .collect();
    uncovered.sort_by_key(|g| g.priority);

    let total_uncovered = uncovered.len();
    // Coverage actions sit at the FRONT of the list, so the first `cap` of them
    // survive the truncation — coverage always wins a contested slot over extra
    // parallelism for an already-covered goal.
    let newly_covered = total_uncovered.min(cap);
    let deferred = total_uncovered - newly_covered;

    let coverage_actions: Vec<PlannedAction> = uncovered
        .iter()
        .map(|g| PlannedAction {
            kind: ActionKind::AdvanceGoal,
            goal_id: Some(g.id.clone()),
            description: format!(
                "coverage: ensure a live engineer for incomplete goal '{}'",
                g.id
            ),
        })
        .collect();

    // Prepend coverage actions ahead of the Decide-emitted actions, then apply
    // the AIMD cap as a hard ceiling.
    let mut combined = coverage_actions;
    combined.append(planned);
    combined.truncate(cap);
    *planned = combined;

    CoverageReport {
        covered: already_covered + newly_covered,
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
