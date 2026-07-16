//! A small but genuinely runnable "kitchen" prototype: the operational app that
//! runs a menu.
//!
//! From a designed [`MenuConcept`](super::design::MenuConcept) it can:
//! - **scale** every recipe to any guest count (exact integer multiplication of
//!   per-serving quantities),
//! - produce a **costed shopping list** aggregated across the whole menu,
//! - run **nutrition analysis** (per-guest and total calories),
//! - run **cost analysis** (per-guest and total spend), and
//! - build a **prep schedule** that fires tasks per station, batched by guest
//!   count, and reports the wall-clock time to be service-ready.
//!
//! Everything is in-memory and deterministic, so it can be scaffolded from a
//! concept and exercised end-to-end in a test or example without any external
//! service.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::GastronomeError;
use super::design::{MenuConcept, RecipePlan, Station};

/// How many servings one prep batch covers. Larger guest counts need more
/// batches, which lengthens each task's effective duration.
pub const BATCH_SIZE: u32 = 20;

/// A recipe scaled to a concrete guest count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaledRecipe {
    pub code: String,
    pub name: String,
    pub course: String,
    pub servings: u32,
    pub total_cost_cents: u32,
    pub total_calories: u32,
    pub per_serving_cost_cents: u32,
    pub per_serving_calories: u32,
}

/// An aggregated shopping-list line across the whole menu.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShoppingLine {
    pub ingredient: String,
    pub unit: String,
    pub total_qty: u32,
    pub total_cost_cents: u32,
}

/// The result of costing a menu for a guest count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostAnalysis {
    pub guest_count: u32,
    pub per_guest_cents: u32,
    pub total_cents: u32,
}

/// The result of the menu's nutrition analysis for a guest count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NutritionAnalysis {
    pub guest_count: u32,
    pub per_guest_calories: u32,
    pub total_calories: u32,
}

/// A single prep task placed on the schedule with a concrete time window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepScheduledTask {
    pub recipe_code: String,
    pub task_name: String,
    pub station: Station,
    pub start_minute: u32,
    pub end_minute: u32,
}

/// A complete prep schedule for a guest count. Stations run in parallel, so
/// `total_minutes` is the busiest station's finishing time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepSchedule {
    pub tasks: Vec<PrepScheduledTask>,
    pub total_minutes: u32,
    pub batches: u32,
}

impl PrepSchedule {
    /// Whether any two tasks on the same station overlap in time. A correct
    /// schedule always returns `false`.
    #[must_use]
    pub fn has_station_overlap(&self) -> bool {
        for station in Station::all() {
            let mut windows: Vec<(u32, u32)> = self
                .tasks
                .iter()
                .filter(|t| t.station == station)
                .map(|t| (t.start_minute, t.end_minute))
                .collect();
            windows.sort_unstable();
            for pair in windows.windows(2) {
                if pair[0].1 > pair[1].0 {
                    return true;
                }
            }
        }
        false
    }
}

/// In-memory kitchen engine: the runnable app that operates a designed menu.
#[derive(Clone, Debug, Default)]
pub struct KitchenEngine {
    recipes: Vec<RecipePlan>,
    by_code: BTreeMap<String, usize>,
}

impl KitchenEngine {
    /// Create an empty engine.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scaffold a running engine from a designed menu concept: register every
    /// recipe in menu order.
    #[must_use]
    pub fn from_concept(concept: &MenuConcept) -> Self {
        let mut engine = Self::new();
        for course in &concept.menu.courses {
            for dish in &course.dishes {
                engine.add_recipe(dish.clone());
            }
        }
        engine
    }

    /// Add a recipe to the menu.
    pub fn add_recipe(&mut self, recipe: RecipePlan) {
        let index = self.recipes.len();
        self.by_code.insert(recipe.code.clone(), index);
        self.recipes.push(recipe);
    }

    /// All recipes, in menu order.
    #[must_use]
    pub fn recipes(&self) -> &[RecipePlan] {
        &self.recipes
    }

    /// Number of recipes on the menu.
    #[must_use]
    pub fn recipe_count(&self) -> usize {
        self.recipes.len()
    }

    /// Look up a recipe by code.
    #[must_use]
    pub fn recipe(&self, code: &str) -> Option<&RecipePlan> {
        self.by_code.get(code).map(|&i| &self.recipes[i])
    }

    /// Scale a recipe to `guests` servings.
    ///
    /// # Errors
    /// [`GastronomeError::UnknownRecipe`] if `code` is not on the menu, or
    /// [`GastronomeError::InvalidGuestCount`] if `guests` is zero.
    pub fn scale_recipe(&self, code: &str, guests: u32) -> Result<ScaledRecipe, GastronomeError> {
        if guests == 0 {
            return Err(GastronomeError::InvalidGuestCount {
                reason: "a plan must serve at least one guest".to_string(),
            });
        }
        let recipe = self
            .recipe(code)
            .ok_or_else(|| GastronomeError::UnknownRecipe {
                code: code.to_string(),
            })?;
        let per_serving_cost_cents = recipe.cost_cents_per_serving();
        let per_serving_calories = recipe.calories_per_serving();
        Ok(ScaledRecipe {
            code: recipe.code.clone(),
            name: recipe.name.clone(),
            course: recipe.course.clone(),
            servings: guests,
            total_cost_cents: per_serving_cost_cents * guests,
            total_calories: per_serving_calories * guests,
            per_serving_cost_cents,
            per_serving_calories,
        })
    }

    /// Build a costed shopping list for `guests`, aggregating identical
    /// ingredients (matched by name + unit) across the whole menu.
    #[must_use]
    pub fn shopping_list(&self, guests: u32) -> Vec<ShoppingLine> {
        let mut lines: BTreeMap<(String, String), (u32, u32)> = BTreeMap::new();
        for recipe in &self.recipes {
            for ing in &recipe.ingredients {
                let entry = lines
                    .entry((ing.name.clone(), ing.unit.clone()))
                    .or_insert((0, 0));
                entry.0 += ing.qty_per_serving * guests;
                entry.1 += ing.cost_cents_per_serving * guests;
            }
        }
        lines
            .into_iter()
            .map(
                |((ingredient, unit), (total_qty, total_cost_cents))| ShoppingLine {
                    ingredient,
                    unit,
                    total_qty,
                    total_cost_cents,
                },
            )
            .collect()
    }

    /// Cost the whole menu for `guests`.
    #[must_use]
    pub fn cost_analysis(&self, guests: u32) -> CostAnalysis {
        let per_guest_cents: u32 = self
            .recipes
            .iter()
            .map(RecipePlan::cost_cents_per_serving)
            .sum();
        CostAnalysis {
            guest_count: guests,
            per_guest_cents,
            total_cents: per_guest_cents * guests,
        }
    }

    /// Nutrition analysis for the whole menu at `guests`.
    #[must_use]
    pub fn nutrition_analysis(&self, guests: u32) -> NutritionAnalysis {
        let per_guest_calories: u32 = self
            .recipes
            .iter()
            .map(RecipePlan::calories_per_serving)
            .sum();
        NutritionAnalysis {
            guest_count: guests,
            per_guest_calories,
            total_calories: per_guest_calories * guests,
        }
    }

    /// Number of prep batches required to serve `guests`.
    #[must_use]
    pub fn batches_for(guests: u32) -> u32 {
        guests.div_ceil(BATCH_SIZE).max(1)
    }

    /// Build a prep schedule for `guests`. Each task's effective duration is its
    /// base minutes times the number of batches; stations run in parallel and
    /// each station's tasks are laid out back-to-back in menu order.
    #[must_use]
    pub fn prep_schedule(&self, guests: u32) -> PrepSchedule {
        let batches = Self::batches_for(guests);
        let mut station_cursor: BTreeMap<Station, u32> = BTreeMap::new();
        let mut tasks = Vec::new();
        for recipe in &self.recipes {
            for prep in &recipe.prep_tasks {
                let cursor = station_cursor.entry(prep.station).or_insert(0);
                let start = *cursor;
                let end = start + prep.minutes * batches;
                *cursor = end;
                tasks.push(PrepScheduledTask {
                    recipe_code: recipe.code.clone(),
                    task_name: prep.name.clone(),
                    station: prep.station,
                    start_minute: start,
                    end_minute: end,
                });
            }
        }
        let total_minutes = station_cursor.values().copied().max().unwrap_or(0);
        PrepSchedule {
            tasks,
            total_minutes,
            batches,
        }
    }

    /// Total number of prep tasks across all recipes.
    #[must_use]
    pub fn prep_task_count(&self) -> usize {
        self.recipes.iter().map(|r| r.prep_tasks.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::design::{MenuBrief, ServiceStyle, design_menu};

    fn sample_engine() -> KitchenEngine {
        let brief = MenuBrief::new("Test Feast", "a gala", ServiceStyle::Upscale, 40, "t");
        let concept = design_menu(&brief).unwrap();
        KitchenEngine::from_concept(&concept)
    }

    #[test]
    fn from_concept_registers_all_recipes() {
        let engine = sample_engine();
        assert_eq!(engine.recipe_count(), 4);
        assert!(engine.recipe("C1").is_some());
        assert!(engine.recipe("C4").is_some());
        assert!(engine.recipe("C9").is_none());
    }

    #[test]
    fn scaling_is_exact_integer_multiplication() {
        let engine = sample_engine();
        let one = engine.scale_recipe("C1", 1).unwrap();
        let hundred = engine.scale_recipe("C1", 100).unwrap();
        assert_eq!(one.total_cost_cents * 100, hundred.total_cost_cents);
        assert_eq!(one.total_calories * 100, hundred.total_calories);
        assert_eq!(hundred.per_serving_cost_cents, one.per_serving_cost_cents);
    }

    #[test]
    fn scale_recipe_rejects_bad_input() {
        let engine = sample_engine();
        assert!(matches!(
            engine.scale_recipe("C1", 0),
            Err(GastronomeError::InvalidGuestCount { .. })
        ));
        assert!(matches!(
            engine.scale_recipe("nope", 10),
            Err(GastronomeError::UnknownRecipe { .. })
        ));
    }

    #[test]
    fn shopping_list_reconciles_with_cost_analysis() {
        let engine = sample_engine();
        let guests = 37;
        let cost = engine.cost_analysis(guests);
        let list_total: u32 = engine
            .shopping_list(guests)
            .iter()
            .map(|line| line.total_cost_cents)
            .sum();
        assert_eq!(list_total, cost.total_cents);
        assert_eq!(cost.per_guest_cents * guests, cost.total_cents);
    }

    #[test]
    fn nutrition_scales_with_guests() {
        let engine = sample_engine();
        let n = engine.nutrition_analysis(120);
        assert_eq!(n.per_guest_calories * 120, n.total_calories);
        assert!(n.per_guest_calories > 0);
    }

    #[test]
    fn prep_schedule_batches_with_guests_and_never_overlaps() {
        let engine = sample_engine();
        let small = engine.prep_schedule(10);
        let large = engine.prep_schedule(200);
        assert_eq!(small.batches, 1);
        assert_eq!(large.batches, 10);
        assert!(large.total_minutes > small.total_minutes);
        assert_eq!(small.tasks.len(), engine.prep_task_count());
        assert!(!small.has_station_overlap());
        assert!(!large.has_station_overlap());
        assert!(small.total_minutes > 0);
    }

    #[test]
    fn shopping_list_aggregates_repeated_ingredients() {
        // "Plating" tasks and shared ingredients (e.g. none repeat here) — verify
        // aggregation keeps one line per (name, unit) pair.
        let engine = sample_engine();
        let list = engine.shopping_list(10);
        let mut keys: Vec<(String, String)> = list
            .iter()
            .map(|l| (l.ingredient.clone(), l.unit.clone()))
            .collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "each (ingredient, unit) appears once");
    }
}
