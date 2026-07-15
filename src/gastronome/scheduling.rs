//! Prep scheduling: turn each recipe's stage-tagged steps into a concrete,
//! back-scheduled timeline that finishes exactly at service time.
//!
//! Model (a deliberate, documented v1 simplification): a single kitchen line
//! works tasks **sequentially** in stage order — all `Prep`, then all `Cook`,
//! then all `Plate` — with recipes ordered by name within a stage for a
//! stable plan. Step durations are per-recipe active-work estimates and do not
//! grow with batch size. The whole run is laid out so the final task ends at
//! `service_time_min`; the derived kitchen-open time is the first task's start.

use super::types::{PrepSchedule, PrepTask, Recipe, Stage};

/// Build a back-scheduled prep timeline for `recipes`, finishing at
/// `service_time_min` (minutes-from-midnight).
pub fn build_schedule(recipes: &[&Recipe], service_time_min: u32) -> PrepSchedule {
    // Collect tasks in kitchen order: stage-major, then recipe name, then the
    // recipe's own step order.
    let mut ordered: Vec<(&str, &Stage, &str, u32)> = Vec::new();
    let mut by_name: Vec<&&Recipe> = recipes.iter().collect();
    by_name.sort_by(|a, b| a.name.cmp(&b.name));

    for stage in Stage::ORDERED.iter() {
        for recipe in &by_name {
            for step in &recipe.steps {
                if step.stage == *stage {
                    ordered.push((&recipe.name, &step.stage, &step.description, step.minutes));
                }
            }
        }
    }

    let total: u32 = ordered.iter().map(|(_, _, _, m)| *m).sum();
    let kitchen_start_min = service_time_min.saturating_sub(total);

    let mut cursor = kitchen_start_min;
    let mut tasks = Vec::with_capacity(ordered.len());
    for (recipe_name, stage, description, minutes) in ordered {
        let start_min = cursor;
        let end_min = cursor + minutes;
        cursor = end_min;
        tasks.push(PrepTask {
            recipe_name: recipe_name.to_string(),
            stage: *stage,
            description: description.to_string(),
            start_min,
            end_min,
        });
    }

    PrepSchedule {
        kitchen_start_min,
        service_time_min,
        tasks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::library::builtin_pantry;

    #[test]
    fn schedule_finishes_at_service_time() {
        let p = builtin_pantry();
        let menu = p.menu("italian-dinner").unwrap();
        let recipes: Vec<&Recipe> = menu
            .recipe_ids
            .iter()
            .map(|id| p.recipe(id).unwrap())
            .collect();
        let sched = build_schedule(&recipes, 18 * 60);
        let last = sched.tasks.last().unwrap();
        assert_eq!(last.end_min, 18 * 60);
        assert_eq!(last.stage, Stage::Plate);
    }

    #[test]
    fn tasks_are_contiguous_and_stage_ordered() {
        let p = builtin_pantry();
        let menu = p.menu("italian-dinner").unwrap();
        let recipes: Vec<&Recipe> = menu
            .recipe_ids
            .iter()
            .map(|id| p.recipe(id).unwrap())
            .collect();
        let sched = build_schedule(&recipes, 18 * 60);
        // Contiguous, non-overlapping.
        for w in sched.tasks.windows(2) {
            assert_eq!(w[0].end_min, w[1].start_min);
        }
        // Stage order never regresses.
        let mut prev = Stage::Prep;
        for t in &sched.tasks {
            assert!(
                Stage::ORDERED.iter().position(|s| *s == t.stage).unwrap()
                    >= Stage::ORDERED.iter().position(|s| *s == prev).unwrap()
            );
            prev = t.stage;
        }
    }

    #[test]
    fn kitchen_start_equals_service_minus_total() {
        let p = builtin_pantry();
        let menu = p.menu("italian-dinner").unwrap();
        let recipes: Vec<&Recipe> = menu
            .recipe_ids
            .iter()
            .map(|id| p.recipe(id).unwrap())
            .collect();
        let sched = build_schedule(&recipes, 18 * 60);
        let total: u32 = sched.tasks.iter().map(|t| t.end_min - t.start_min).sum();
        assert_eq!(sched.kitchen_start_min, 18 * 60 - total);
    }

    #[test]
    fn empty_recipes_yield_empty_schedule() {
        let sched = build_schedule(&[], 12 * 60);
        assert!(sched.tasks.is_empty());
        assert_eq!(sched.kitchen_start_min, 12 * 60);
    }
}
