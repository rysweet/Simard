//! Graceful OODA completion + bounded reflection safeguard (issue #1025).
//!
//! A **pure**, side-effect-free decision layer that turns the deploy-aware
//! done-gate's already-computed [`CompletionVerdict`] into a terminal loop
//! decision. It introduces no new evidence source, never lets a model self-
//! report "done", and never touches the network, `gh`, or the goal store.
//!
//! Before this layer the reflection loop kept re-invoking reflect/verify even
//! after a goal's deliverable PR was merged and green, because the completion
//! predicate never treated "gate-verified complete" as a *terminal* state. This
//! module supplies that terminal predicate plus a bounded no-progress safeguard,
//! while preserving Simard's perpetual-by-default posture.
//!
//! See `docs/concepts/graceful-ooda-completion.md` and
//! `docs/reference/ooda-graceful-completion-api.md`.

use std::collections::BTreeMap;

use crate::goal_curation::{ActiveGoal, CompletionVerdict, GoalBoard};

/// Environment variable overriding [`ReflectionBounds::max_reflection_cycles`].
pub const MAX_REFLECTION_CYCLES_ENV: &str = "SIMARD_OODA_MAX_REFLECTION_CYCLES";
/// Environment variable overriding [`ReflectionBounds::stop_when_idle`].
pub const STOP_WHEN_ACHIEVED_ENV: &str = "SIMARD_OODA_STOP_WHEN_ACHIEVED";

/// The single decision [`evaluate`] returns for one reflection tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopDecision {
    /// Goal not yet achieved and reflection budget not exhausted — reflect again.
    Continue,
    /// Terminal predicate holds (gate-verified achieved) — break the loop cleanly.
    GracefulComplete,
    /// Non-perpetual goal still not achieved after `max_reflection_cycles`
    /// consecutive no-progress cycles — yield with a recorded blocker.
    BoundExceeded,
}

/// Policy for the bounded no-progress safeguard. Perpetual by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflectionBounds {
    /// Consecutive no-progress reflection cycles a non-perpetual goal may burn
    /// before [`evaluate`] yields [`LoopDecision::BoundExceeded`]. `0` disables
    /// the bound (prior, uncapped behavior).
    pub max_reflection_cycles: u32,

    /// When `true`, an all-ACHIEVED board lets the daemon loop idle. Sourced
    /// from [`STOP_WHEN_ACHIEVED_ENV`]. Defaults to `false` (perpetual).
    pub stop_when_idle: bool,
}

impl Default for ReflectionBounds {
    /// Perpetual-safe defaults: bound disabled (`0`) and `stop_when_idle = false`,
    /// so the daemon stays perpetual and no goal is ever spin-capped unless an
    /// operator opts in via the environment.
    fn default() -> Self {
        Self {
            max_reflection_cycles: 0,
            stop_when_idle: false,
        }
    }
}

impl ReflectionBounds {
    /// Build from the environment. Malformed values fall back to the safe
    /// default and emit a `tracing::warn!` — never a panic.
    ///
    /// * [`MAX_REFLECTION_CYCLES_ENV`] -> `max_reflection_cycles`
    /// * [`STOP_WHEN_ACHIEVED_ENV`]    -> `stop_when_idle`
    pub fn from_env() -> Self {
        Self::from_env_values(
            std::env::var(MAX_REFLECTION_CYCLES_ENV).ok().as_deref(),
            std::env::var(STOP_WHEN_ACHIEVED_ENV).ok().as_deref(),
        )
    }

    /// Pure core of [`from_env`](Self::from_env), split out so it is testable
    /// without mutating process-global environment state.
    ///
    /// `None` (unset) yields the corresponding [`Default`] field. A present but
    /// malformed value also degrades to the default and warns.
    pub fn from_env_values(max_cycles: Option<&str>, stop_when_idle: Option<&str>) -> Self {
        let default = Self::default();

        let max_reflection_cycles = match max_cycles.map(str::trim) {
            None | Some("") => default.max_reflection_cycles,
            Some(raw) => match raw.parse::<u32>() {
                Ok(n) => n,
                Err(_) => {
                    tracing::warn!(
                        env = MAX_REFLECTION_CYCLES_ENV,
                        value = raw,
                        "malformed reflection-cycle bound; using default"
                    );
                    default.max_reflection_cycles
                }
            },
        };

        let stop_when_idle = match stop_when_idle.map(str::trim) {
            None | Some("") => default.stop_when_idle,
            Some(raw) => match parse_bool(raw) {
                Some(b) => b,
                None => {
                    tracing::warn!(
                        env = STOP_WHEN_ACHIEVED_ENV,
                        value = raw,
                        "malformed stop-when-achieved flag; using default"
                    );
                    default.stop_when_idle
                }
            },
        };

        Self {
            max_reflection_cycles,
            stop_when_idle,
        }
    }

    /// True when a **non-perpetual** goal has burned at least
    /// `max_reflection_cycles` consecutive no-progress cycles, so the loop
    /// should yield it with a recorded blocker rather than reflect forever.
    ///
    /// Returns `false` when the bound is disabled (`max_reflection_cycles == 0`)
    /// or the goal is perpetual/standing. This is the exact predicate
    /// [`evaluate`] uses for its [`LoopDecision::BoundExceeded`] arm, exposed so
    /// a caller iterating goals that are already known **not** gate-complete
    /// (e.g. the daemon's post-cycle board) can consult it without synthesizing
    /// a [`CompletionVerdict`](crate::goal_curation::CompletionVerdict).
    pub fn bound_exhausted(&self, is_perpetual: bool, no_progress_streak: u32) -> bool {
        self.max_reflection_cycles > 0
            && !is_perpetual
            && no_progress_streak >= self.max_reflection_cycles
    }
}

/// Lenient truthy/falsey parse for the `stop_when_idle` flag. Returns `None`
/// for anything unrecognised so the caller can warn and fall back.
fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// A single goal is achieved when its done-gate verdict `is_complete()`.
///
/// The verdict already encapsulates the goal's success-criteria (its
/// `description`) evaluation, so this predicate adds **no** second evidence
/// source and never treats a model-reported "done" as complete.
pub fn goal_achieved(verdict: &CompletionVerdict) -> bool {
    verdict.is_complete()
}

/// Returns `true` only when every active goal on the board is complete by
/// gate-verified evidence (`verdict.is_complete()`).
///
/// A goal with no verdict this cycle is treated as **not** achieved (conservative
/// — absence of gate-verified evidence is never completion). An empty board is
/// not "all achieved": there is nothing to have achieved, so the perpetual
/// daemon has no reason to idle.
pub fn goals_all_achieved(
    board: &GoalBoard,
    verdicts: &BTreeMap<String, CompletionVerdict>,
) -> bool {
    !board.active.is_empty()
        && board
            .active
            .iter()
            .all(|goal| verdicts.get(&goal.id).is_some_and(goal_achieved))
}

/// Map one reflection tick to a [`LoopDecision`].
///
/// Precedence:
///   1. [`LoopDecision::GracefulComplete`] if `goal_achieved(verdict)`.
///   2. [`LoopDecision::BoundExceeded`] if the goal is **not** perpetual,
///      `bounds.max_reflection_cycles > 0`, and
///      `no_progress_streak >= bounds.max_reflection_cycles`.
///   3. [`LoopDecision::Continue`] otherwise.
///
/// `goal` is used only to determine perpetual/standing status (for the
/// exemption); achievement is decided from `verdict` alone. Achievement is
/// checked **before** the bound, so a goal that becomes achieved on the same
/// cycle it would have tripped the bound completes gracefully rather than
/// yielding.
pub fn evaluate(
    goal: &ActiveGoal,
    verdict: &CompletionVerdict,
    no_progress_streak: u32,
    bounds: &ReflectionBounds,
) -> LoopDecision {
    if goal_achieved(verdict) {
        return LoopDecision::GracefulComplete;
    }

    if bounds.bound_exhausted(goal.is_perpetual(), no_progress_streak) {
        return LoopDecision::BoundExceeded;
    }

    LoopDecision::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_curation::CompletionEvidence;

    fn complete_verdict() -> CompletionVerdict {
        CompletionVerdict::Complete(CompletionEvidence {
            pr_merged: true,
            issue_closed: true,
            self_affecting: false,
            deployed: true,
        })
    }

    fn blocked_verdict() -> CompletionVerdict {
        CompletionVerdict::Blocked {
            evidence: CompletionEvidence {
                pr_merged: false,
                issue_closed: false,
                self_affecting: false,
                deployed: true,
            },
            missing: vec![crate::goal_curation::MissingEvidence::PrNotMerged],
        }
    }

    fn normal_goal(id: &str) -> ActiveGoal {
        ActiveGoal::new(id, format!("deliver {id}"), 100)
    }

    fn perpetual_goal(id: &str) -> ActiveGoal {
        ActiveGoal::new(id, format!("standing {id}"), 100).mark_standing()
    }

    // --- goal_achieved / goals_all_achieved truth table -------------------

    #[test]
    fn goal_achieved_only_for_complete() {
        assert!(goal_achieved(&complete_verdict()));
        assert!(!goal_achieved(&blocked_verdict()));
    }

    #[test]
    fn goals_all_achieved_true_when_every_goal_complete() {
        let mut board = GoalBoard::new();
        board.active.push(normal_goal("a"));
        board.active.push(normal_goal("b"));
        let mut verdicts = BTreeMap::new();
        verdicts.insert("a".to_string(), complete_verdict());
        verdicts.insert("b".to_string(), complete_verdict());
        assert!(goals_all_achieved(&board, &verdicts));
    }

    #[test]
    fn goals_all_achieved_false_when_one_blocked() {
        let mut board = GoalBoard::new();
        board.active.push(normal_goal("a"));
        board.active.push(normal_goal("b"));
        let mut verdicts = BTreeMap::new();
        verdicts.insert("a".to_string(), complete_verdict());
        verdicts.insert("b".to_string(), blocked_verdict());
        assert!(!goals_all_achieved(&board, &verdicts));
    }

    #[test]
    fn goals_all_achieved_false_when_verdict_missing() {
        // A goal with no gate verdict this cycle is never "achieved".
        let mut board = GoalBoard::new();
        board.active.push(normal_goal("a"));
        board.active.push(normal_goal("b"));
        let mut verdicts = BTreeMap::new();
        verdicts.insert("a".to_string(), complete_verdict());
        assert!(!goals_all_achieved(&board, &verdicts));
    }

    #[test]
    fn goals_all_achieved_false_for_empty_board() {
        // Nothing achieved => no reason for the perpetual daemon to idle.
        let board = GoalBoard::new();
        let verdicts = BTreeMap::new();
        assert!(!goals_all_achieved(&board, &verdicts));
    }

    // --- evaluate decision matrix -----------------------------------------

    #[test]
    fn evaluate_graceful_complete_on_verified_completion() {
        let goal = normal_goal("g");
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        // Even with a huge streak, a gate-verified completion terminates gracefully.
        assert_eq!(
            evaluate(&goal, &complete_verdict(), 99, &bounds),
            LoopDecision::GracefulComplete
        );
    }

    #[test]
    fn evaluate_continue_when_not_achieved_and_under_bound() {
        let goal = normal_goal("g");
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        assert_eq!(
            evaluate(&goal, &blocked_verdict(), 2, &bounds),
            LoopDecision::Continue
        );
    }

    #[test]
    fn evaluate_bound_exceeded_at_threshold() {
        let goal = normal_goal("g");
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        // streak == max => bound exceeded (>= semantics).
        assert_eq!(
            evaluate(&goal, &blocked_verdict(), 3, &bounds),
            LoopDecision::BoundExceeded
        );
        // and above the threshold too.
        assert_eq!(
            evaluate(&goal, &blocked_verdict(), 7, &bounds),
            LoopDecision::BoundExceeded
        );
    }

    #[test]
    fn evaluate_achievement_takes_precedence_over_bound() {
        // Achieved on the very cycle the bound would trip => GracefulComplete.
        let goal = normal_goal("g");
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        assert_eq!(
            evaluate(&goal, &complete_verdict(), 3, &bounds),
            LoopDecision::GracefulComplete
        );
    }

    #[test]
    fn evaluate_bound_disabled_when_zero() {
        let goal = normal_goal("g");
        let bounds = ReflectionBounds {
            max_reflection_cycles: 0,
            stop_when_idle: false,
        };
        // 0 disables the bound: never yields, always continues while not achieved.
        assert_eq!(
            evaluate(&goal, &blocked_verdict(), 1_000, &bounds),
            LoopDecision::Continue
        );
    }

    #[test]
    fn evaluate_perpetual_goal_is_exempt_from_bound() {
        let goal = perpetual_goal("standing");
        assert!(goal.is_perpetual());
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        // A standing goal that never completes falls through to Continue,
        // preserving the perpetual-goal no-progress exemption.
        assert_eq!(
            evaluate(&goal, &blocked_verdict(), 10_000, &bounds),
            LoopDecision::Continue
        );
    }

    #[test]
    fn evaluate_perpetual_goal_can_still_complete_gracefully() {
        // Exemption is only from BoundExceeded; a gate-verified perpetual goal
        // still reports GracefulComplete (the daemon rolls it to a fresh cycle).
        let goal = perpetual_goal("standing");
        let bounds = ReflectionBounds::default();
        assert_eq!(
            evaluate(&goal, &complete_verdict(), 0, &bounds),
            LoopDecision::GracefulComplete
        );
    }

    // --- ReflectionBounds config ------------------------------------------

    #[test]
    fn reflection_bounds_default_is_perpetual_safe() {
        let d = ReflectionBounds::default();
        assert_eq!(d.max_reflection_cycles, 0, "bound disabled by default");
        assert!(!d.stop_when_idle, "daemon perpetual by default");
    }

    #[test]
    fn from_env_values_unset_yields_default() {
        assert_eq!(
            ReflectionBounds::from_env_values(None, None),
            ReflectionBounds::default()
        );
    }

    #[test]
    fn from_env_values_parses_valid() {
        let b = ReflectionBounds::from_env_values(Some("5"), Some("true"));
        assert_eq!(b.max_reflection_cycles, 5);
        assert!(b.stop_when_idle);
    }

    #[test]
    fn from_env_values_accepts_bool_aliases() {
        for truthy in ["1", "true", "YES", "On"] {
            assert!(
                ReflectionBounds::from_env_values(None, Some(truthy)).stop_when_idle,
                "{truthy} should parse truthy"
            );
        }
        for falsey in ["0", "false", "NO", "Off"] {
            assert!(
                !ReflectionBounds::from_env_values(None, Some(falsey)).stop_when_idle,
                "{falsey} should parse falsey"
            );
        }
    }

    #[test]
    fn from_env_values_malformed_degrades_to_default_without_panic() {
        let b = ReflectionBounds::from_env_values(Some("not-a-number"), Some("maybe"));
        assert_eq!(b, ReflectionBounds::default());
    }

    #[test]
    fn from_env_values_blank_is_treated_as_unset() {
        let b = ReflectionBounds::from_env_values(Some("   "), Some("  "));
        assert_eq!(b, ReflectionBounds::default());
    }

    // --- bound_exhausted (shared BoundExceeded predicate) -----------------

    #[test]
    fn bound_exhausted_disabled_when_cap_zero() {
        let bounds = ReflectionBounds::default(); // max_reflection_cycles == 0
        assert!(!bounds.bound_exhausted(false, 100_000));
    }

    #[test]
    fn bound_exhausted_exempts_perpetual_goals() {
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        assert!(!bounds.bound_exhausted(true, 100_000));
    }

    #[test]
    fn bound_exhausted_fires_at_or_above_cap_for_non_perpetual() {
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        assert!(!bounds.bound_exhausted(false, 2), "under cap keeps going");
        assert!(bounds.bound_exhausted(false, 3), "at cap yields");
        assert!(bounds.bound_exhausted(false, 4), "over cap yields");
    }

    #[test]
    fn bound_exhausted_matches_evaluate_bound_arm() {
        // Single-source-of-truth: `evaluate`'s BoundExceeded arm must agree with
        // the standalone predicate the daemon consults for post-cycle goals.
        let bounds = ReflectionBounds {
            max_reflection_cycles: 3,
            stop_when_idle: false,
        };
        let g = normal_goal("stuck");
        assert_eq!(
            evaluate(&g, &blocked_verdict(), 3, &bounds),
            LoopDecision::BoundExceeded
        );
        assert!(bounds.bound_exhausted(g.is_perpetual(), 3));
    }
}
