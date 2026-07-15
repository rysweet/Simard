//! Scaling recipes from their base yield to a target number of servings.

use super::types::Recipe;

/// Scale a recipe to `target_servings`, returning a new recipe.
///
/// Ingredient quantities scale linearly with the serving factor. Prep-step
/// durations scale only when [`PrepStep::scales_with_servings`] is set
/// (chopping scales; preheating an oven does not). The returned recipe's
/// `base_servings` is set to `target_servings` so downstream cost/nutrition
/// per-serving figures stay correct.
///
/// A non-positive `base_servings` or `target_servings` is treated as a no-op
/// scale factor of `1.0` so callers never divide by zero; the planner rejects
/// invalid yields up front via the cost/nutrition validators.
///
/// [`PrepStep::scales_with_servings`]: super::types::PrepStep::scales_with_servings
#[must_use]
pub fn scale_recipe(recipe: &Recipe, target_servings: f64) -> Recipe {
    let factor = if recipe.base_servings > 0.0 && target_servings > 0.0 {
        target_servings / recipe.base_servings
    } else {
        1.0
    };

    let ingredients = recipe
        .ingredients
        .iter()
        .map(|ri| {
            let mut scaled = ri.clone();
            scaled.quantity *= factor;
            scaled
        })
        .collect();

    let steps = recipe
        .steps
        .iter()
        .map(|step| {
            let mut scaled = step.clone();
            if step.scales_with_servings {
                scaled.minutes *= factor;
            }
            scaled
        })
        .collect();

    Recipe {
        name: recipe.name.clone(),
        course: recipe.course,
        base_servings: if target_servings > 0.0 {
            target_servings
        } else {
            recipe.base_servings
        },
        ingredients,
        steps,
    }
}
