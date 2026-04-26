//! Orient phase: rank goals by urgency, informed by environment context.

use std::collections::HashMap;

use crate::error::SimardResult;
use crate::goal_curation::{GoalBoard, GoalProgress};

use super::{Observation, Priority};

/// Urgency penalty per consecutive failure on a goal. Five failures in a
/// row drives any goal's urgency to 0 (deprioritised below everything else).
const FAILURE_PENALTY_PER_CONSECUTIVE: f64 = 0.2;

/// Orient: rank goals by urgency, informed by environment context.
///
/// Base urgency: Blocked > not-started > in-progress > completed.
/// Environment signals (dirty working tree, open issues mentioning a goal)
/// can boost a goal's urgency so the OODA loop prioritises actionable work.
/// Goals with consecutive failures are demoted by
/// `FAILURE_PENALTY_PER_CONSECUTIVE * count` (clamped to ≥0) so the daemon
/// stops burning budget retrying the same broken target.
pub fn orient(
    observation: &Observation,
    goals: &GoalBoard,
    failure_counts: &HashMap<String, u32>,
) -> SimardResult<Vec<Priority>> {
    let env = &observation.environment;
    let has_dirty_tree = !env.git_status.is_empty();

    let mut priorities: Vec<Priority> = goals
        .active
        .iter()
        .map(|g| {
            let (mut urgency, mut reason) = match &g.status {
                GoalProgress::Blocked(r) => (1.0, format!("blocked: {r}")),
                GoalProgress::NotStarted => (0.8, "not yet started".to_string()),
                GoalProgress::InProgress { percent } => (
                    0.6 * (1.0 - (*percent as f64 / 100.0)),
                    format!("{percent}% complete"),
                ),
                GoalProgress::Completed => (0.0, "completed".to_string()),
            };

            // Boost urgency if an open issue mentions this goal.
            let mentioned_in_issues = env
                .open_issues
                .iter()
                .any(|title| title.to_lowercase().contains(&g.id.to_lowercase()));
            if mentioned_in_issues {
                urgency = (urgency + 0.1).min(1.0);
                reason = format!("{reason}; mentioned in open issue");
            }

            // Slight boost for in-progress goals when the tree is dirty
            // (indicates active development that may relate to this goal).
            if has_dirty_tree && matches!(g.status, GoalProgress::InProgress { .. }) {
                urgency = (urgency + 0.05).min(1.0);
                reason = format!("{reason}; dirty working tree");
            }

            // Demote chronically failing goals.
            if let Some(&count) = failure_counts.get(&g.id)
                && count > 0
            {
                let penalty = FAILURE_PENALTY_PER_CONSECUTIVE * count as f64;
                let demoted = (urgency - penalty).max(0.0);
                reason = format!(
                    "{reason}; {count} consecutive failure(s) → urgency {urgency:.2} − {penalty:.2}"
                );
                urgency = demoted;
            }

            Priority {
                goal_id: g.id.clone(),
                urgency,
                reason,
            }
        })
        .collect();

    if observation.memory_stats.episodic_count > 100 {
        priorities.push(Priority {
            goal_id: "__memory__".to_string(),
            urgency: 0.5,
            reason: format!(
                "episodic memory has {} entries, consolidation needed",
                observation.memory_stats.episodic_count
            ),
        });
    }

    if let Some(ref score) = observation.gym_health
        && score.overall < 0.7
    {
        priorities.push(Priority {
            goal_id: "__improvement__".to_string(),
            urgency: 0.7,
            reason: format!("gym overall {:.1}% below 70% target", score.overall * 100.0),
        });
    }

    // ── Eval watchdog override ──────────────────────────────────────
    // If the watchdog tripped in observe(), nothing else matters this
    // cycle. Push a synthetic priority with urgency 1.0 (above any
    // real goal) so decide() routes to it. This is the loop's "stop
    // and ring the alarm" path — kept alongside other priorities so
    // the existing ranking/filing/dashboard infrastructure picks it up
    // for free, but with enough urgency that it preempts ordinary work.
    if let Some(ref reason) = observation.eval_watchdog {
        priorities.push(Priority {
            goal_id: "__eval_watchdog__".to_string(),
            urgency: 1.0,
            reason: format!("EVAL WATCHDOG: {reason}"),
        });
    }

    priorities.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(priorities)
}
