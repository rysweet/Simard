//! Prep scheduling: turn a set of scaled recipes and a service time into a
//! concrete, per-task prep timetable using backward critical-path scheduling.
//!
//! Each recipe's prep steps form a dependency DAG (a step cannot start until the
//! steps it `depends_on` have finished). We schedule *backwards* from the event's
//! service time so that every recipe's terminal steps finish exactly at service,
//! and each step starts as late as possible while still meeting its dependents.
//! The earliest start across all tasks is when the kitchen must fire up.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::error::{GastronomeError, GastronomeResult};
use super::types::{Recipe, format_clock};

/// One scheduled prep task: a single step of a single recipe placed on the clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// The recipe this step belongs to.
    pub recipe_id: String,
    /// The recipe's display name.
    pub recipe_name: String,
    /// The step id within the recipe.
    pub step_id: String,
    /// What the cook does.
    pub description: String,
    /// Absolute start time, in minutes since midnight (may be negative = prior day).
    pub start_minutes: i64,
    /// Absolute end time, in minutes since midnight.
    pub end_minutes: i64,
}

impl ScheduledTask {
    /// The start time formatted as `HH:MM`.
    #[must_use]
    pub fn start_clock(&self) -> String {
        format_clock_signed(self.start_minutes)
    }

    /// The end time formatted as `HH:MM`.
    #[must_use]
    pub fn end_clock(&self) -> String {
        format_clock_signed(self.end_minutes)
    }
}

/// A complete prep timetable for an event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepSchedule {
    /// The service time all recipes are scheduled to finish by.
    pub service_time_minutes: u32,
    /// When the first task must start (earliest start across all tasks).
    pub kitchen_start_minutes: i64,
    /// Total prep lead time in minutes (`service_time - kitchen_start`).
    pub total_lead_minutes: i64,
    /// Every scheduled task, ordered by start time then recipe/step id.
    pub tasks: Vec<ScheduledTask>,
}

/// Format signed minutes-since-midnight as `HH:MM`, wrapping into `[00:00,24:00)`.
#[must_use]
pub fn format_clock_signed(minutes: i64) -> String {
    let day = 24 * 60;
    let wrapped = minutes.rem_euclid(day);
    // rem_euclid guarantees 0..day, so the cast is lossless.
    format_clock(u32::try_from(wrapped).unwrap_or(0))
}

/// Build a prep schedule for a set of already-scaled recipes finishing at
/// `service_time_minutes`.
///
/// # Errors
/// Returns [`GastronomeError::UnknownStepDependency`] or
/// [`GastronomeError::CyclicPrepSteps`] if a recipe's steps are ill-formed.
pub fn schedule_prep(
    recipes: &[Recipe],
    service_time_minutes: u32,
) -> GastronomeResult<PrepSchedule> {
    let service = i64::from(service_time_minutes);
    let mut tasks = Vec::new();

    for recipe in recipes {
        schedule_recipe_into(recipe, service, &mut tasks)?;
    }

    tasks.sort_by(|a, b| {
        a.start_minutes
            .cmp(&b.start_minutes)
            .then_with(|| a.recipe_id.cmp(&b.recipe_id))
            .then_with(|| a.step_id.cmp(&b.step_id))
    });

    let kitchen_start = tasks
        .iter()
        .map(|t| t.start_minutes)
        .min()
        .unwrap_or(service);

    Ok(PrepSchedule {
        service_time_minutes,
        kitchen_start_minutes: kitchen_start,
        total_lead_minutes: service - kitchen_start,
        tasks,
    })
}

/// Schedule one recipe's steps backwards from `service`, appending to `out`.
fn schedule_recipe_into(
    recipe: &Recipe,
    service: i64,
    out: &mut Vec<ScheduledTask>,
) -> GastronomeResult<()> {
    if recipe.steps.is_empty() {
        return Ok(());
    }

    // Index steps and validate dependency references.
    let index: BTreeMap<&str, &super::types::PrepStep> =
        recipe.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    for step in &recipe.steps {
        for dep in &step.depends_on {
            if !index.contains_key(dep.as_str()) {
                return Err(GastronomeError::UnknownStepDependency {
                    recipe_id: recipe.id.clone(),
                    step_id: step.id.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
    }

    // Dependents: for step S, which steps list S in depends_on.
    let mut dependents: BTreeMap<&str, Vec<&str>> = recipe
        .steps
        .iter()
        .map(|s| (s.id.as_str(), Vec::new()))
        .collect();
    for step in &recipe.steps {
        for dep in &step.depends_on {
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(step.id.as_str());
        }
    }

    // Memoised backward pass computing latest_start for each step.
    let mut latest_start: BTreeMap<&str, i64> = BTreeMap::new();
    for step in &recipe.steps {
        latest_start_of(
            step.id.as_str(),
            recipe,
            &index,
            &dependents,
            service,
            &mut latest_start,
            &mut Vec::new(),
        )?;
    }

    for step in &recipe.steps {
        let start = latest_start[step.id.as_str()];
        out.push(ScheduledTask {
            recipe_id: recipe.id.clone(),
            recipe_name: recipe.name.clone(),
            step_id: step.id.clone(),
            description: step.description.clone(),
            start_minutes: start,
            end_minutes: start + i64::from(step.duration_minutes),
        });
    }

    Ok(())
}

/// Recursive, cycle-detecting computation of a step's latest start time.
///
/// `latest_finish(S) = min over dependents T of latest_start(T)`, defaulting to
/// `service` when S has no dependents; `latest_start(S) = latest_finish(S) - dur`.
fn latest_start_of<'a>(
    step_id: &'a str,
    recipe: &Recipe,
    index: &BTreeMap<&'a str, &'a super::types::PrepStep>,
    dependents: &BTreeMap<&'a str, Vec<&'a str>>,
    service: i64,
    memo: &mut BTreeMap<&'a str, i64>,
    stack: &mut Vec<&'a str>,
) -> GastronomeResult<i64> {
    if let Some(&cached) = memo.get(step_id) {
        return Ok(cached);
    }
    if stack.contains(&step_id) {
        return Err(GastronomeError::CyclicPrepSteps {
            recipe_id: recipe.id.clone(),
        });
    }
    stack.push(step_id);

    let mut latest_finish = service;
    for &dependent in &dependents[step_id] {
        let dep_start =
            latest_start_of(dependent, recipe, index, dependents, service, memo, stack)?;
        latest_finish = latest_finish.min(dep_start);
    }

    let duration = i64::from(index[step_id].duration_minutes);
    let start = latest_finish - duration;

    stack.pop();
    memo.insert(step_id, start);
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::types::PrepStep;

    fn step(id: &str, dur: u32, deps: &[&str]) -> PrepStep {
        PrepStep {
            id: id.to_string(),
            description: format!("do {id}"),
            duration_minutes: dur,
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn recipe(id: &str, steps: Vec<PrepStep>) -> Recipe {
        Recipe {
            id: id.to_string(),
            name: id.to_uppercase(),
            servings: 4,
            ingredients: vec![],
            steps,
        }
    }

    #[test]
    fn linear_chain_schedules_backwards_from_service() {
        // mix(10) -> rest(20) -> bake(30), service at 12:00 (720)
        let r = recipe(
            "cake",
            vec![
                step("mix", 10, &[]),
                step("rest", 20, &["mix"]),
                step("bake", 30, &["rest"]),
            ],
        );
        let sched = schedule_prep(&[r], 720).unwrap();
        // bake ends 720 starts 690; rest ends 690 starts 670; mix ends 670 starts 660
        let by_id: BTreeMap<_, _> = sched
            .tasks
            .iter()
            .map(|t| (t.step_id.as_str(), t))
            .collect();
        assert_eq!(by_id["bake"].end_minutes, 720);
        assert_eq!(by_id["bake"].start_minutes, 690);
        assert_eq!(by_id["rest"].start_minutes, 670);
        assert_eq!(by_id["mix"].start_minutes, 660);
        assert_eq!(sched.kitchen_start_minutes, 660);
        assert_eq!(sched.total_lead_minutes, 60);
    }

    #[test]
    fn diamond_dependency_takes_longest_path() {
        // start(10) -> a(30) -> join(5); start -> b(15) -> join
        // critical path: start(10)+a(30)+join(5) = 45
        let r = recipe(
            "dish",
            vec![
                step("start", 10, &[]),
                step("a", 30, &["start"]),
                step("b", 15, &["start"]),
                step("join", 5, &["a", "b"]),
            ],
        );
        let sched = schedule_prep(&[r], 600).unwrap();
        assert_eq!(sched.total_lead_minutes, 45);
        let by_id: BTreeMap<_, _> = sched
            .tasks
            .iter()
            .map(|t| (t.step_id.as_str(), t))
            .collect();
        // join ends at 600, starts 595; a ends 595 starts 565; start ends 565 starts 555
        assert_eq!(by_id["join"].end_minutes, 600);
        assert_eq!(by_id["a"].start_minutes, 565);
        assert_eq!(by_id["start"].start_minutes, 555);
        // b can start as late as 580 (join_start 595 - 15) but path is not critical
        assert_eq!(by_id["b"].start_minutes, 580);
    }

    #[test]
    fn multiple_recipes_share_service_and_report_earliest_start() {
        let quick = recipe("salad", vec![step("toss", 10, &[])]);
        let slow = recipe(
            "roast",
            vec![step("season", 15, &[]), step("cook", 90, &["season"])],
        );
        let sched = schedule_prep(&[quick, slow], 1000).unwrap();
        // roast lead = 105, salad lead = 10 => kitchen start = 1000-105 = 895
        assert_eq!(sched.kitchen_start_minutes, 895);
        assert_eq!(sched.total_lead_minutes, 105);
        assert_eq!(sched.tasks.len(), 3);
    }

    #[test]
    fn tasks_are_sorted_by_start_time() {
        let r = recipe(
            "cake",
            vec![step("mix", 10, &[]), step("bake", 30, &["mix"])],
        );
        let sched = schedule_prep(&[r], 720).unwrap();
        assert!(sched.tasks[0].start_minutes <= sched.tasks[1].start_minutes);
        assert_eq!(sched.tasks[0].step_id, "mix");
    }

    #[test]
    fn cycle_is_detected() {
        let r = recipe("loop", vec![step("a", 5, &["b"]), step("b", 5, &["a"])]);
        let err = schedule_prep(&[r], 600).unwrap_err();
        assert!(matches!(err, GastronomeError::CyclicPrepSteps { .. }));
    }

    #[test]
    fn empty_steps_produce_empty_schedule() {
        let r = recipe("nothing", vec![]);
        let sched = schedule_prep(&[r], 600).unwrap();
        assert!(sched.tasks.is_empty());
        assert_eq!(sched.kitchen_start_minutes, 600);
        assert_eq!(sched.total_lead_minutes, 0);
    }

    #[test]
    fn negative_start_wraps_to_previous_day_clock() {
        // A 90-minute task finishing at 00:30 (30) starts at -60 => 23:00.
        let r = recipe("overnight", vec![step("proof", 90, &[])]);
        let sched = schedule_prep(&[r], 30).unwrap();
        assert_eq!(sched.tasks[0].start_minutes, -60);
        assert_eq!(sched.tasks[0].start_clock(), "23:00");
        assert_eq!(sched.tasks[0].end_clock(), "00:30");
    }
}
