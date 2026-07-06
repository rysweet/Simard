//! The goal **prioritization pass**: a pure, deterministic function that
//! differentiates *undifferentiated* goal priorities while leaving the
//! operator's *explicitly-set* ones untouched (issue #2695 follow-up).
//!
//! The operator's complaint is that almost every goal sits at the same priority
//! (e.g. many at p3), which is effectively no prioritization. The DISPLAY half
//! of the fix (ordering + visible tiers) lives in the dashboard; this is the
//! SUBSTANCE half: given a set of goals plus structured goal-graph signals, spread
//! the non-explicit priorities into a bounded `1..=5` band driven by real signals
//! (bottleneck/blocking-relationships, lifecycle urgency, standing-vs-one-off,
//! staleness) rather than a flat default.
//!
//! Design invariants (pinned by `tests_prioritize.rs`):
//!   * **Pure & deterministic** — same inputs + injected `now` ⇒ same output.
//!   * **Order-preserving** — the pass rewrites `priority` only; it never drops,
//!     adds, or reorders goals (display ordering is the renderer's job).
//!   * **Explicit-preserving** — a goal with `priority_explicit == true` keeps its
//!     exact priority and flag, even under the strongest differentiating signals.
//!   * **Bounded** — every re-scored priority lands in `1..=5` (never `< 1`).
//!   * **Neutral default** — a goal with no differentiating signal stays at the
//!     neutral middle tier (p3), so the pass differentiates rather than clobbers.
//!   * **Structured signals only** — differentiation reads structured goal-graph
//!     data (`depends_on`) and the goal's own lifecycle fields, never brittle
//!     description parsing (G3).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use super::types::{ActiveGoal, GoalProgress};

/// The neutral middle priority a goal keeps when it carries no differentiating
/// signal. Chosen so the pass *spreads* undifferentiated goals rather than
/// yanking every no-signal goal to the top.
const NEUTRAL_PRIORITY: u32 = 3;

/// A goal a full week without a progress update earns one staleness point, up to
/// [`MAX_STALENESS_BONUS`]. Keeps a long-idle goal from silently sinking forever.
const STALENESS_WEEK_DAYS: i64 = 7;
/// Cap on the staleness contribution so an ancient goal cannot swamp the
/// structural (bottleneck) signal.
const MAX_STALENESS_BONUS: i64 = 2;

/// Structured goal-graph signals fed into the pass. Kept separate from the goals
/// themselves so the pass stays pure — the caller gathers the graph edges (from
/// the decomposition graph or the in-hand board linkage) and hands them in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrioritizationSignals {
    /// `dependent goal id -> ids of the goals it depends on (its blockers)`.
    ///
    /// This is the `depends_on` edge set from the goal graph. A goal that appears
    /// as a *blocker* for many dependents is a bottleneck and is prioritized up;
    /// a leaf that nothing depends on gets no bottleneck boost.
    pub depends_on: HashMap<String, Vec<String>>,
}

/// Re-score the priorities of the `priority_explicit == false` goals in `goals`
/// using `signals` and the injected clock `now`, returning a new `Vec` in the
/// SAME order with only `priority` rewritten. Explicit goals pass through
/// unchanged. See the module docs for the pinned invariants.
#[must_use]
pub fn prioritize(
    goals: &[ActiveGoal],
    signals: &PrioritizationSignals,
    now: DateTime<Utc>,
) -> Vec<ActiveGoal> {
    // How many goals depend on each goal id (its outbound blocker count). Built
    // once; counting is commutative so this is deterministic regardless of the
    // HashMap iteration order.
    let mut dependents: HashMap<&str, u32> = HashMap::new();
    for blockers in signals.depends_on.values() {
        for blocker in blockers {
            *dependents.entry(blocker.as_str()).or_insert(0) += 1;
        }
    }

    goals
        .iter()
        .map(|goal| {
            let mut out = goal.clone();
            if goal.priority_explicit {
                // Never reshuffle an operator's explicit choice.
                return out;
            }
            let count = dependents.get(goal.id.as_str()).copied().unwrap_or(0);
            let has_unmet_deps = signals.depends_on.contains_key(goal.id.as_str());
            out.priority = score_to_priority(signal_score(goal, count, has_unmet_deps, now));
            out
        })
        .collect()
}

/// The raw urgency score for a goal: higher ⇒ more urgent ⇒ a *lower* (more
/// important) priority number after banding. Neutral (no signal) is `0`, which
/// bands to the neutral middle tier.
fn signal_score(
    goal: &ActiveGoal,
    dependents: u32,
    has_unmet_deps: bool,
    now: DateTime<Utc>,
) -> i64 {
    let mut score: i64 = 0;

    // Bottleneck: a goal others depend on gates downstream work — the strongest
    // differentiating signal.
    score += 3 * i64::from(dependents);

    // Participation in a blocking chain: a goal with unmet `depends_on` of its
    // own is part of a critical path, so it edges up over a fully-isolated goal.
    if has_unmet_deps {
        score += 1;
    }

    // In-flight work (an open PR / branch / engineer session) means the goal is
    // actively moving and worth keeping near the top.
    if !goal.wip_refs.is_empty() {
        score += 1;
    }

    // Lifecycle urgency.
    match &goal.status {
        // Blocked work needs attention to unblock whatever waits on it; in-flight
        // work should keep its momentum. Both out-rank idle/not-started goals.
        GoalProgress::Blocked(_) | GoalProgress::InProgress { .. } => score += 2,
        // Deliberately-paused work is less urgent than active work.
        GoalProgress::Paused => score -= 1,
        // A completed goal has the least remaining urgency.
        GoalProgress::Completed => score -= 3,
        GoalProgress::NotStarted | GoalProgress::Proposed => {}
    }

    // Standing/perpetual goals are durable maintenance work with no terminal
    // done-state — lower urgency than a one-off deliverable.
    if goal.is_perpetual() {
        score -= 2;
    }

    // Staleness: a goal that has gone a long time without progress drifts up so it
    // is not forgotten. Bounded so it never swamps the structural signal.
    if let Some(last) = goal.last_progress_update_at {
        let days = (now - last).num_days().max(0);
        score += (days / STALENESS_WEEK_DAYS).min(MAX_STALENESS_BONUS);
    }

    score
}

/// Band a raw [`signal_score`] into the `1..=5` priority range. Absolute
/// thresholds (not relative ranks) so a no-signal goal always lands on the
/// neutral middle tier and a strong bottleneck always reaches the top, whatever
/// the rest of the board looks like.
fn score_to_priority(score: i64) -> u32 {
    match score {
        s if s >= 6 => 1,
        s if s >= 3 => 2,
        s if s >= 0 => NEUTRAL_PRIORITY, // 0..=2 — neutral / weak signal
        s if s >= -2 => 4,
        _ => 5,
    }
}
