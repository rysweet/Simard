//! Nutrition analysis for recipes and menus.

use serde::{Deserialize, Serialize};

use super::catalog::Catalog;
use super::types::{Nutrition, Recipe};
use super::{GastronomeError, GastronomeResult};

/// Nutrition totals for a recipe, both whole and per serving.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NutritionBreakdown {
    /// Recipe name.
    pub recipe: String,
    /// Nutrition summed across the whole recipe at its current yield.
    pub total: Nutrition,
    /// Nutrition per serving (`total / base_servings`).
    pub per_serving: Nutrition,
}

/// Compute nutrition for a recipe using the ingredient catalog.
///
/// # Errors
///
/// Returns [`GastronomeError::UnknownIngredient`] if a referenced ingredient
/// is missing, or [`GastronomeError::InvalidYield`] if `base_servings <= 0`.
pub fn recipe_nutrition(
    recipe: &Recipe,
    catalog: &Catalog,
) -> GastronomeResult<NutritionBreakdown> {
    if recipe.base_servings <= 0.0 {
        return Err(GastronomeError::InvalidYield {
            recipe: recipe.name.clone(),
        });
    }
    let mut total = Nutrition::default();
    for ri in &recipe.ingredients {
        let ing =
            catalog
                .get(&ri.ingredient)
                .ok_or_else(|| GastronomeError::UnknownIngredient {
                    recipe: recipe.name.clone(),
                    ingredient: ri.ingredient.clone(),
                })?;
        total = total.plus(ing.nutrition.scaled(ri.quantity));
    }
    Ok(NutritionBreakdown {
        recipe: recipe.name.clone(),
        total,
        per_serving: total.scaled(1.0 / recipe.base_servings),
    })
}

/// Sum the per-serving nutrition of several recipes into a per-guest total.
///
/// A guest is assumed to receive one serving of each recipe on the menu.
#[must_use]
pub fn per_guest_nutrition(breakdowns: &[NutritionBreakdown]) -> Nutrition {
    breakdowns
        .iter()
        .fold(Nutrition::default(), |acc, b| acc.plus(b.per_serving))
}
