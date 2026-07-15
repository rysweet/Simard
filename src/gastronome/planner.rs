//! End-to-end planner: turn an [`EventBrief`] into a costed, scheduled
//! [`MenuPlan`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::catalog::Catalog;
use super::cost::{CostBreakdown, aggregate_cost, recipe_cost};
use super::nutrition::{NutritionBreakdown, per_guest_nutrition, recipe_nutrition};
use super::scaling::scale_recipe;
use super::scheduling::{PrepSchedule, build_schedule};
use super::types::{DietaryTag, EventBrief, Nutrition, Recipe};
use super::{GastronomeError, GastronomeResult};

/// Whether the plan's per-guest cost fits the brief's budget.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BudgetStatus {
    /// No budget was specified in the brief.
    Unconstrained,
    /// Per-guest cost is at or under budget.
    WithinBudget {
        /// The brief's budget per guest, USD.
        budget_per_guest_usd: f64,
        /// Actual per-guest cost, USD.
        per_guest_usd: f64,
        /// Remaining room under budget, USD.
        headroom_usd: f64,
    },
    /// Per-guest cost exceeds budget.
    OverBudget {
        /// The brief's budget per guest, USD.
        budget_per_guest_usd: f64,
        /// Actual per-guest cost, USD.
        per_guest_usd: f64,
        /// Amount over budget, USD.
        overage_usd: f64,
    },
}

impl BudgetStatus {
    /// Whether the plan is affordable (unconstrained or within budget).
    #[must_use]
    pub fn is_affordable(&self) -> bool {
        !matches!(self, Self::OverBudget { .. })
    }
}

/// A complete, costed, scheduled plan for catering an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuPlan {
    /// Event name from the brief.
    pub event_name: String,
    /// Guest count the plan is scaled to.
    pub guest_count: u32,
    /// When service begins.
    pub event_start: DateTime<Utc>,
    /// Each recipe scaled to the guest count.
    pub scaled_recipes: Vec<Recipe>,
    /// Cost breakdown at event scale.
    pub cost: CostBreakdown,
    /// Nutrition a single guest receives (one serving of each recipe).
    pub nutrition_per_guest: Nutrition,
    /// Per-recipe nutrition at event scale.
    pub nutrition_per_recipe: Vec<NutritionBreakdown>,
    /// Budget assessment.
    pub budget: BudgetStatus,
    /// Backward prep schedule.
    pub schedule: PrepSchedule,
}

/// Plan an event from a brief.
///
/// # Errors
///
/// - [`GastronomeError::EmptyMenu`] if the menu has no recipes.
/// - [`GastronomeError::UnknownIngredient`] if a recipe references an
///   ingredient absent from the catalog.
/// - [`GastronomeError::InvalidYield`] if a recipe has a non-positive yield.
/// - [`GastronomeError::DietaryViolation`] if any recipe fails a required
///   dietary constraint.
pub fn plan_event(brief: &EventBrief) -> GastronomeResult<MenuPlan> {
    if brief.menu.recipes.is_empty() {
        return Err(GastronomeError::EmptyMenu);
    }

    let catalog = Catalog::new(&brief.catalog);

    for recipe in &brief.menu.recipes {
        catalog.validate_recipe(recipe)?;
        if recipe.base_servings <= 0.0 {
            return Err(GastronomeError::InvalidYield {
                recipe: recipe.name.clone(),
            });
        }
    }

    let violations = dietary_violations(brief, &catalog);
    if !violations.is_empty() {
        return Err(GastronomeError::DietaryViolation { violations });
    }

    let guests = brief.effective_guests();

    let scaled: Vec<Recipe> = brief
        .menu
        .recipes
        .iter()
        .map(|r| scale_recipe(r, guests))
        .collect();

    let mut per_recipe_cost = Vec::with_capacity(scaled.len());
    let mut per_recipe_nutrition = Vec::with_capacity(scaled.len());
    for recipe in &scaled {
        per_recipe_cost.push(recipe_cost(recipe, &catalog)?);
        per_recipe_nutrition.push(recipe_nutrition(recipe, &catalog)?);
    }

    let cost = aggregate_cost(per_recipe_cost, guests);
    let nutrition_per_guest = per_guest_nutrition(&per_recipe_nutrition);
    let budget = assess_budget(brief.budget_per_guest_usd, cost.per_guest_usd);
    let schedule = build_schedule(&scaled, brief.event_start, brief.effective_cooks());

    Ok(MenuPlan {
        event_name: brief.event_name.clone(),
        guest_count: brief.guest_count,
        event_start: brief.event_start,
        scaled_recipes: scaled,
        cost,
        nutrition_per_guest,
        nutrition_per_recipe: per_recipe_nutrition,
        budget,
        schedule,
    })
}

/// Collect dietary-constraint violations across every recipe in the brief.
///
/// A recipe satisfies a required tag only when *every* ingredient it uses
/// carries that tag.
fn dietary_violations(brief: &EventBrief, catalog: &Catalog) -> Vec<String> {
    let mut violations = Vec::new();
    for recipe in &brief.menu.recipes {
        for &required in &brief.dietary_constraints {
            if let Some(bad) = offending_ingredient(recipe, required, catalog) {
                violations.push(format!(
                    "recipe '{}' is not {} (ingredient '{}' lacks the tag)",
                    recipe.name,
                    required.label(),
                    bad
                ));
            }
        }
    }
    violations
}

fn offending_ingredient(
    recipe: &Recipe,
    required: DietaryTag,
    catalog: &Catalog,
) -> Option<String> {
    for ri in &recipe.ingredients {
        match catalog.get(&ri.ingredient) {
            Some(ing) if ing.tags.contains(&required) => {}
            _ => return Some(ri.ingredient.clone()),
        }
    }
    None
}

fn assess_budget(budget: Option<f64>, per_guest: f64) -> BudgetStatus {
    match budget {
        None => BudgetStatus::Unconstrained,
        Some(b) if per_guest <= b => BudgetStatus::WithinBudget {
            budget_per_guest_usd: b,
            per_guest_usd: per_guest,
            headroom_usd: b - per_guest,
        },
        Some(b) => BudgetStatus::OverBudget {
            budget_per_guest_usd: b,
            per_guest_usd: per_guest,
            overage_usd: per_guest - b,
        },
    }
}
