//! Nutrition aggregation: roll a recipe's ingredient nutrition up to the whole
//! recipe and down to a single serving.

use super::error::{GastronomeError, GastronomeResult};
use super::pantry::Pantry;
use super::types::{Nutrition, Recipe};

/// Total nutrition for a recipe's full base yield (all `servings`).
///
/// # Errors
/// Returns [`GastronomeError::UnknownIngredient`] if a line references an
/// ingredient absent from `pantry`.
pub fn recipe_nutrition(recipe: &Recipe, pantry: &Pantry) -> GastronomeResult<Nutrition> {
    let mut total = Nutrition::default();
    for line in &recipe.ingredients {
        let ingredient =
            pantry
                .get(&line.ingredient_id)
                .ok_or_else(|| GastronomeError::UnknownIngredient {
                    recipe_id: recipe.id.clone(),
                    ingredient_id: line.ingredient_id.clone(),
                })?;
        total = total.add(&ingredient.nutrition_per_unit.scaled(line.quantity));
    }
    Ok(total)
}

/// Nutrition for a single serving of a recipe.
///
/// # Errors
/// See [`recipe_nutrition`].
pub fn recipe_nutrition_per_serving(
    recipe: &Recipe,
    pantry: &Pantry,
) -> GastronomeResult<Nutrition> {
    let total = recipe_nutrition(recipe, pantry)?;
    let servings = f64::from(recipe.servings.max(1));
    Ok(total.scaled(1.0 / servings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::types::{Ingredient, RecipeIngredient, Unit};

    fn pantry() -> Pantry {
        Pantry::new([
            Ingredient {
                id: "flour".into(),
                name: "Flour".into(),
                unit: Unit::Gram,
                cost_per_unit: 0.002,
                nutrition_per_unit: Nutrition {
                    calories: 3.64,
                    protein_g: 0.10,
                    carbs_g: 0.76,
                    fat_g: 0.01,
                },
                allergens: Default::default(),
                vegetarian: true,
                vegan: true,
            },
            Ingredient {
                id: "butter".into(),
                name: "Butter".into(),
                unit: Unit::Gram,
                cost_per_unit: 0.01,
                nutrition_per_unit: Nutrition {
                    calories: 7.17,
                    protein_g: 0.01,
                    carbs_g: 0.0,
                    fat_g: 0.81,
                },
                allergens: Default::default(),
                vegetarian: true,
                vegan: false,
            },
        ])
        .unwrap()
    }

    fn recipe() -> Recipe {
        Recipe {
            id: "shortbread".into(),
            name: "Shortbread".into(),
            servings: 4,
            ingredients: vec![
                RecipeIngredient {
                    ingredient_id: "flour".into(),
                    quantity: 100.0,
                },
                RecipeIngredient {
                    ingredient_id: "butter".into(),
                    quantity: 50.0,
                },
            ],
            steps: vec![],
        }
    }

    #[test]
    fn total_nutrition_sums_scaled_ingredients() {
        let n = recipe_nutrition(&recipe(), &pantry()).unwrap();
        // flour: 3.64*100 = 364 cal; butter: 7.17*50 = 358.5 cal
        assert!((n.calories - 722.5).abs() < 1e-6);
        assert!((n.fat_g - (0.01 * 100.0 + 0.81 * 50.0)).abs() < 1e-6);
    }

    #[test]
    fn per_serving_divides_by_servings() {
        let total = recipe_nutrition(&recipe(), &pantry()).unwrap();
        let per = recipe_nutrition_per_serving(&recipe(), &pantry()).unwrap();
        assert!((per.calories - total.calories / 4.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_ingredient_errors() {
        let mut r = recipe();
        r.ingredients.push(RecipeIngredient {
            ingredient_id: "gold".into(),
            quantity: 1.0,
        });
        let err = recipe_nutrition(&r, &pantry()).unwrap_err();
        assert!(matches!(err, GastronomeError::UnknownIngredient { .. }));
    }
}
