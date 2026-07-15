//! Backward prep scheduling: lay a menu's prep steps onto parallel cooks so
//! that every task finishes by the event start.
//!
//! The scheduler models each recipe as one job (its steps run sequentially),
//! balances jobs across `cook_count` cooks with a longest-processing-time
//! greedy heuristic to minimise the makespan, then lays each cook's jobs out
//! backward from the event start. Recipes whose work is mostly at-service
//! (not make-ahead) are placed closest to service; make-ahead-heavy recipes
//! are pulled earlier.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::types::Recipe;

/// A single scheduled prep task (one recipe step assigned to one cook).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepTask {
    /// Name of the recipe this step belongs to.
    pub recipe: String,
    /// Step description.
    pub step: String,
    /// 1-based cook index this task is assigned to.
    pub cook: u32,
    /// When the task starts.
    pub start: DateTime<Utc>,
    /// When the task ends.
    pub end: DateTime<Utc>,
    /// Active minutes for the task.
    pub minutes: f64,
    /// Whether this step is make-ahead.
    pub make_ahead: bool,
}

/// A complete prep schedule for an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepSchedule {
    /// When service begins; every task finishes by this time.
    pub event_start: DateTime<Utc>,
    /// The earliest task start — when the kitchen must begin work.
    pub kitchen_start: DateTime<Utc>,
    /// Number of cooks the work was distributed across.
    pub cook_count: u32,
    /// Total active prep minutes across all tasks.
    pub total_active_minutes: f64,
    /// Wall-clock minutes from `kitchen_start` to `event_start`.
    pub makespan_minutes: f64,
    /// Active minutes assigned to each cook (index 0 = cook 1).
    pub per_cook_minutes: Vec<f64>,
    /// All tasks, sorted by start time ascending.
    pub tasks: Vec<PrepTask>,
}

struct Job<'a> {
    recipe: &'a Recipe,
    total_minutes: f64,
    at_service_minutes: f64,
}

/// Upper bound on a single step's duration (100 years, in minutes). Guards
/// against an untrusted brief supplying an absurd `minutes` value that would
/// overflow the timeline arithmetic.
const MAX_STEP_MINUTES: f64 = 100.0 * 365.0 * 24.0 * 60.0;

fn minutes_to_duration(minutes: f64) -> Duration {
    // Clamp defensively: a negative, non-finite, or absurdly large step
    // duration must never invert a task, panic, or overflow the timeline.
    let safe = if minutes.is_finite() {
        minutes.clamp(0.0, MAX_STEP_MINUTES)
    } else {
        0.0
    };
    let millis = (safe * 60_000.0).round() as i64;
    Duration::milliseconds(millis)
}

/// Build a backward prep schedule for a set of (already scaled) recipes.
///
/// `cook_count` is clamped to at least one. Recipes with no steps contribute
/// no tasks. When there are no tasks at all, `kitchen_start == event_start`.
#[must_use]
pub fn build_schedule(
    recipes: &[Recipe],
    event_start: DateTime<Utc>,
    cook_count: u32,
) -> PrepSchedule {
    let cooks = cook_count.max(1);

    // Build jobs, skipping recipes with no prep work.
    let mut jobs: Vec<Job> = recipes
        .iter()
        .filter(|r| !r.steps.is_empty())
        .map(|r| {
            let total: f64 = r.steps.iter().map(|s| s.minutes).sum();
            let at_service: f64 = r
                .steps
                .iter()
                .filter(|s| !s.make_ahead)
                .map(|s| s.minutes)
                .sum();
            Job {
                recipe: r,
                total_minutes: total,
                at_service_minutes: at_service,
            }
        })
        .collect();

    // Longest-processing-time assignment: heaviest jobs first, each to the
    // least-loaded cook. Deterministic tie-breaks keep output stable.
    jobs.sort_by(|a, b| {
        b.total_minutes
            .partial_cmp(&a.total_minutes)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.recipe.name.cmp(&b.recipe.name))
    });

    let mut load = vec![0.0_f64; cooks as usize];
    let mut assigned: Vec<Vec<&Job>> = vec![Vec::new(); cooks as usize];
    for job in &jobs {
        let cook = least_loaded_cook(&load);
        load[cook] += job.total_minutes;
        assigned[cook].push(job);
    }

    let mut tasks: Vec<PrepTask> = Vec::new();
    for (cook_idx, cook_jobs) in assigned.iter_mut().enumerate() {
        // Within a cook, place make-ahead-heavy jobs earlier and at-service
        // jobs last (closest to service). Sort ascending by at-service load.
        cook_jobs.sort_by(|a, b| {
            a.at_service_minutes
                .partial_cmp(&b.at_service_minutes)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.recipe.name.cmp(&b.recipe.name))
        });

        // Lay out backward from event_start: the last job ends at service.
        let mut cursor = event_start;
        for job in cook_jobs.iter().rev() {
            for step in job.recipe.steps.iter().rev() {
                let end = cursor;
                // `checked_sub_signed` never panics: on the (clamped-but-still-
                // possible) overflow near the calendar bounds it yields a
                // zero-length task rather than aborting the process.
                let start = end
                    .checked_sub_signed(minutes_to_duration(step.minutes))
                    .unwrap_or(end);
                tasks.push(PrepTask {
                    recipe: job.recipe.name.clone(),
                    step: step.description.clone(),
                    cook: cook_idx as u32 + 1,
                    start,
                    end,
                    minutes: step.minutes,
                    make_ahead: step.make_ahead,
                });
                cursor = start;
            }
        }
    }

    tasks.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.cook.cmp(&b.cook))
            .then_with(|| a.recipe.cmp(&b.recipe))
    });

    let kitchen_start = tasks.iter().map(|t| t.start).min().unwrap_or(event_start);
    let total_active_minutes: f64 = tasks.iter().map(|t| t.minutes).sum();
    let makespan_minutes = (event_start - kitchen_start).num_milliseconds() as f64 / 60_000.0;

    PrepSchedule {
        event_start,
        kitchen_start,
        cook_count: cooks,
        total_active_minutes,
        makespan_minutes,
        per_cook_minutes: load,
        tasks,
    }
}

fn least_loaded_cook(load: &[f64]) -> usize {
    let mut best = 0;
    for (i, &l) in load.iter().enumerate() {
        if l < load[best] {
            best = i;
        }
    }
    best
}
