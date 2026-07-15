//! Cost analysis for recipes and menus.

use serde::{Deserialize, Serialize};

use super::catalog::Catalog;
use super::types::Recipe;
use super::{GastronomeError, GastronomeResult};

/// Cost of a single recipe at its current yield.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeCost {
    /// Recipe name.
    pub recipe: String,
    /// Total ingredient cost in USD across the whole recipe.
    pub total_usd: f64,
    /// Cost in USD per serving (`total_usd / servings`).
    pub per_serving_usd: f64,
}

/// Compute the cost of a recipe using the ingredient catalog.
///
/// The recipe's `base_servings` is used as the divisor for the per-serving
/// figure, so cost a scaled recipe *after* scaling to get event-level numbers.
///
/// # Errors
///
/// Returns [`GastronomeError::UnknownIngredient`] if a referenced ingredient
/// is missing, or [`GastronomeError::InvalidYield`] if `base_servings <= 0`.
pub fn recipe_cost(recipe: &Recipe, catalog: &Catalog) -> GastronomeResult<RecipeCost> {
    if recipe.base_servings <= 0.0 {
        return Err(GastronomeError::InvalidYield {
            recipe: recipe.name.clone(),
        });
    }
    let mut total = 0.0;
    for ri in &recipe.ingredients {
        let ing =
            catalog
                .get(&ri.ingredient)
                .ok_or_else(|| GastronomeError::UnknownIngredient {
                    recipe: recipe.name.clone(),
                    ingredient: ri.ingredient.clone(),
                })?;
        total += ri.quantity * ing.cost_per_unit_usd;
    }
    Ok(RecipeCost {
        recipe: recipe.name.clone(),
        total_usd: total,
        per_serving_usd: total / recipe.base_servings,
    })
}

/// A costed breakdown of a whole menu at event scale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Per-recipe costs (each already scaled to the event yield).
    pub per_recipe: Vec<RecipeCost>,
    /// Total cost to cater the whole event, USD.
    pub event_total_usd: f64,
    /// Cost per guest, USD.
    pub per_guest_usd: f64,
}

/// Aggregate per-recipe costs into a menu-level breakdown.
///
/// `guests` is the guest count the recipes were scaled to and must be
/// positive; it is used only as the divisor for `per_guest_usd`.
#[must_use]
pub fn aggregate_cost(per_recipe: Vec<RecipeCost>, guests: f64) -> CostBreakdown {
    let event_total: f64 = per_recipe.iter().map(|c| c.total_usd).sum();
    let divisor = if guests > 0.0 { guests } else { 1.0 };
    CostBreakdown {
        per_recipe,
        event_total_usd: event_total,
        per_guest_usd: event_total / divisor,
    }
}
