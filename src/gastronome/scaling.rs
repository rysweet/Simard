//! Recipe scaling: adjust ingredient quantities to hit a target serving count.
//!
//! Ingredient quantities scale linearly with the serving factor. Prep-step
//! durations are deliberately left unchanged: doubling a batch rarely doubles
//! the time to fold or bake it, and over-scaling prep time would inflate the
//! schedule's critical path unrealistically.

use super::error::{GastronomeError, GastronomeResult};
use super::types::Recipe;

/// Scale `recipe` so its yield covers at least `target_servings` portions.
///
/// The returned recipe keeps the same steps but has ingredient quantities
/// multiplied by `target_servings / recipe.servings`, and its `servings` field
/// set to `target_servings`.
///
/// # Errors
/// Returns [`GastronomeError::ZeroServings`] if the source recipe declares zero
/// servings, or [`GastronomeError::InvalidValue`] if `target_servings` is zero.
pub fn scale_recipe(recipe: &Recipe, target_servings: u32) -> GastronomeResult<Recipe> {
    if recipe.servings == 0 {
        return Err(GastronomeError::ZeroServings {
            recipe_id: recipe.id.clone(),
        });
    }
    if target_servings == 0 {
        return Err(GastronomeError::InvalidValue {
            field: format!("scale_recipe('{}') target_servings", recipe.id),
            reason: "must be greater than zero".to_string(),
        });
    }

    let factor = f64::from(target_servings) / f64::from(recipe.servings);
    let mut scaled = recipe.clone();
    for line in &mut scaled.ingredients {
        line.quantity *= factor;
    }
    scaled.servings = target_servings;
    Ok(scaled)
}

/// The multiplicative factor [`scale_recipe`] would apply. Useful for reporting.
#[must_use]
pub fn scale_factor(from_servings: u32, target_servings: u32) -> f64 {
    if from_servings == 0 {
        return 0.0;
    }
    f64::from(target_servings) / f64::from(from_servings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::types::{PrepStep, RecipeIngredient};

    fn recipe() -> Recipe {
        Recipe {
            id: "sauce".into(),
            name: "Sauce".into(),
            servings: 4,
            ingredients: vec![
                RecipeIngredient {
                    ingredient_id: "tomato".into(),
                    quantity: 400.0,
                },
                RecipeIngredient {
                    ingredient_id: "garlic".into(),
                    quantity: 2.0,
                },
            ],
            steps: vec![PrepStep {
                id: "simmer".into(),
                description: "simmer".into(),
                duration_minutes: 25,
                depends_on: vec![],
            }],
        }
    }

    #[test]
    fn scaling_up_multiplies_quantities() {
        let scaled = scale_recipe(&recipe(), 12).unwrap();
        assert_eq!(scaled.servings, 12);
        assert!((scaled.ingredients[0].quantity - 1200.0).abs() < 1e-9);
        assert!((scaled.ingredients[1].quantity - 6.0).abs() < 1e-9);
    }

    #[test]
    fn scaling_down_divides_quantities() {
        let scaled = scale_recipe(&recipe(), 2).unwrap();
        assert!((scaled.ingredients[0].quantity - 200.0).abs() < 1e-9);
    }

    #[test]
    fn scaling_preserves_step_durations() {
        let scaled = scale_recipe(&recipe(), 40).unwrap();
        assert_eq!(scaled.steps[0].duration_minutes, 25);
    }

    #[test]
    fn zero_target_is_rejected() {
        let err = scale_recipe(&recipe(), 0).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidValue { .. }));
    }

    #[test]
    fn scale_factor_matches() {
        assert!((scale_factor(4, 12) - 3.0).abs() < 1e-9);
        assert!(scale_factor(0, 12).abs() < 1e-9);
    }
}
