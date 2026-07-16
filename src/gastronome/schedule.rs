//! Prep scheduling: back-scheduling each dish's steps from the serve time so the
//! kitchen knows when to start and what to do when.
//!
//! Each recipe's steps run sequentially, and the *last* step of every dish ends
//! exactly at the serve time. Walking the steps in reverse yields, for each
//! step, how many minutes before service it must start and finish. Dishes are
//! assumed to run in parallel (independent cooks/stations), so the whole menu's
//! "kitchen call" — when the first pot goes on — is the earliest start across
//! every dish. Times are reported on a 24-hour clock; a long prep that reaches
//! back past midnight simply wraps, which is the correct wall-clock reading.

use chrono::{Duration, NaiveTime};
use serde::{Deserialize, Serialize};

use super::error::{GastronomeError, GastronomeResult};
use super::scaling::MenuItem;

/// One scheduled prep step with its clock window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledStep {
    /// Course the step belongs to.
    pub course: String,
    /// Recipe id.
    pub recipe_id: String,
    /// Recipe name.
    pub recipe_name: String,
    /// What the cook does.
    pub description: String,
    /// Clock time to start, `HH:MM`.
    pub start: String,
    /// Clock time it finishes, `HH:MM`.
    pub end: String,
    /// Duration in minutes.
    pub minutes: u32,
    /// Whether the step needs hands-on attention.
    pub active: bool,
    /// Minutes before service this step starts (larger = earlier). Used for
    /// ordering and to derive the kitchen call time.
    pub minutes_before_serve: u32,
}

/// The whole-menu prep schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepSchedule {
    /// The serve time all dishes converge on, `HH:MM`.
    pub serve_time: String,
    /// When the earliest step starts — when the kitchen is called, `HH:MM`.
    pub kitchen_call_time: String,
    /// Total hands-on (active) minutes across the menu.
    pub total_active_minutes: u32,
    /// Steps ordered earliest-start first.
    pub steps: Vec<ScheduledStep>,
}

fn parse_time(value: &str) -> GastronomeResult<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").map_err(|_| GastronomeError::InvalidServeTime {
        value: value.to_string(),
    })
}

fn clock_before(serve: NaiveTime, minutes_before: u32) -> String {
    let (t, _wrapped) = serve.overflowing_sub_signed(Duration::minutes(i64::from(minutes_before)));
    t.format("%H:%M").to_string()
}

impl PrepSchedule {
    /// Back-schedule the whole `menu` from `serve_time` (`HH:MM`).
    ///
    /// # Errors
    /// Returns [`GastronomeError::InvalidServeTime`] if `serve_time` is not a
    /// valid `HH:MM`, or [`GastronomeError::InvalidStepDuration`] never (step
    /// durations are `u32`), kept for forward-compatibility of the signature.
    pub fn compute(menu: &[MenuItem], serve_time: &str) -> GastronomeResult<Self> {
        let serve = parse_time(serve_time)?;
        let mut steps: Vec<ScheduledStep> = Vec::new();

        for item in menu {
            let recipe = &item.scaled.recipe;
            // Walk steps in reverse: the final step ends at serve time.
            let mut offset_at_end: u32 = 0;
            for step in recipe.steps.iter().rev() {
                let start_offset = offset_at_end + step.minutes;
                steps.push(ScheduledStep {
                    course: item.course.clone(),
                    recipe_id: recipe.id.clone(),
                    recipe_name: recipe.name.clone(),
                    description: step.description.clone(),
                    start: clock_before(serve, start_offset),
                    end: clock_before(serve, offset_at_end),
                    minutes: step.minutes,
                    active: step.active,
                    minutes_before_serve: start_offset,
                });
                offset_at_end = start_offset;
            }
        }

        // Earliest start first; ties broken by course then recipe for stability.
        steps.sort_by(|a, b| {
            b.minutes_before_serve
                .cmp(&a.minutes_before_serve)
                .then_with(|| a.course.cmp(&b.course))
                .then_with(|| a.recipe_id.cmp(&b.recipe_id))
        });

        let max_offset = steps
            .iter()
            .map(|s| s.minutes_before_serve)
            .max()
            .unwrap_or(0);
        let total_active_minutes = steps.iter().filter(|s| s.active).map(|s| s.minutes).sum();

        Ok(Self {
            serve_time: serve.format("%H:%M").to_string(),
            kitchen_call_time: clock_before(serve, max_offset),
            total_active_minutes,
            steps,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::nutrition::Nutrition;
    use super::super::scaling::ScaledRecipe;
    use super::super::types::{Ingredient, PrepStep, Recipe, RecipeIngredient};
    use super::*;

    fn recipe_with_steps(id: &str, course: &str, steps: Vec<PrepStep>) -> Recipe {
        Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            course: course.into(),
            base_servings: 4,
            dietary_tags: vec![],
            ingredients: vec![RecipeIngredient {
                ingredient: Ingredient {
                    name: "x".into(),
                    unit: "each".into(),
                    cost_per_unit: 1.0,
                    nutrition_per_unit: Nutrition::default(),
                },
                quantity: 1.0,
            }],
            steps,
        }
    }

    fn item(recipe: Recipe) -> MenuItem {
        MenuItem {
            course: recipe.course.clone(),
            scaled: ScaledRecipe::new(recipe, 8),
        }
    }

    #[test]
    fn last_step_ends_at_serve_time() {
        let r = recipe_with_steps(
            "stew",
            "main",
            vec![
                PrepStep {
                    description: "prep".into(),
                    minutes: 20,
                    active: true,
                },
                PrepStep {
                    description: "simmer".into(),
                    minutes: 40,
                    active: false,
                },
            ],
        );
        let sched = PrepSchedule::compute(&[item(r)], "19:00").unwrap();
        let simmer = sched
            .steps
            .iter()
            .find(|s| s.description == "simmer")
            .unwrap();
        assert_eq!(simmer.end, "19:00");
        assert_eq!(simmer.start, "18:20");
        let prep = sched
            .steps
            .iter()
            .find(|s| s.description == "prep")
            .unwrap();
        assert_eq!(prep.end, "18:20");
        assert_eq!(prep.start, "18:00");
    }

    #[test]
    fn kitchen_call_is_earliest_start() {
        let quick = recipe_with_steps(
            "salad",
            "starter",
            vec![PrepStep {
                description: "toss".into(),
                minutes: 10,
                active: true,
            }],
        );
        let slow = recipe_with_steps(
            "roast",
            "main",
            vec![PrepStep {
                description: "roast".into(),
                minutes: 120,
                active: false,
            }],
        );
        let sched = PrepSchedule::compute(&[item(quick), item(slow)], "18:00").unwrap();
        assert_eq!(sched.serve_time, "18:00");
        assert_eq!(sched.kitchen_call_time, "16:00");
        // Earliest-start-first ordering: roast (120 before) precedes toss (10).
        assert_eq!(sched.steps[0].description, "roast");
    }

    #[test]
    fn total_active_minutes_counts_only_active() {
        let r = recipe_with_steps(
            "bread",
            "side",
            vec![
                PrepStep {
                    description: "knead".into(),
                    minutes: 15,
                    active: true,
                },
                PrepStep {
                    description: "prove".into(),
                    minutes: 60,
                    active: false,
                },
                PrepStep {
                    description: "bake".into(),
                    minutes: 30,
                    active: false,
                },
            ],
        );
        let sched = PrepSchedule::compute(&[item(r)], "12:00").unwrap();
        assert_eq!(sched.total_active_minutes, 15);
    }

    #[test]
    fn prep_before_midnight_wraps_clock() {
        let r = recipe_with_steps(
            "brisket",
            "main",
            vec![PrepStep {
                description: "smoke".into(),
                minutes: 120,
                active: false,
            }],
        );
        // Serve at 00:30, 120 min prep => start 22:30 previous day.
        let sched = PrepSchedule::compute(&[item(r)], "00:30").unwrap();
        assert_eq!(sched.kitchen_call_time, "22:30");
    }

    #[test]
    fn invalid_serve_time_is_rejected() {
        let r = recipe_with_steps("x", "main", vec![]);
        let err = PrepSchedule::compute(&[item(r)], "25:99").unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidServeTime { .. }));
    }
}
