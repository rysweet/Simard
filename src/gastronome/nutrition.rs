//! Nutrition analysis: aggregate a [`ScaledRecipe`]'s ingredient macros
//! (stored per 100 base units) into a whole-recipe total and a per-serving
//! breakdown.

use serde::{Deserialize, Serialize};

use super::book::KitchenBook;
use super::scaling::ScaledRecipe;
use super::types::{GastronomeResult, Nutrition};

/// Nutrition for a scaled recipe, both in total and per serving.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeNutrition {
    /// Recipe id.
    pub recipe: String,
    /// Target servings the totals were computed for.
    pub target_servings: f64,
    /// Macros for the whole scaled batch.
    pub total: Nutrition,
    /// Macros for a single serving.
    pub per_serving: Nutrition,
}

/// Compute nutrition for a scaled recipe.
///
/// # Errors
/// Returns an error if a scaled line references an unknown ingredient.
pub fn nutrition_recipe(
    book: &KitchenBook,
    scaled: &ScaledRecipe,
) -> GastronomeResult<RecipeNutrition> {
    let mut total = Nutrition::default();
    for line in &scaled.lines {
        let ingredient = book.ingredient(&line.ingredient)?;
        // nutrition is per 100 base units, so scale by base_quantity / 100.
        let factor = line.base_quantity / 100.0;
        total = total + ingredient.nutrition.scaled(factor);
    }
    let per_serving = if scaled.target_servings > 0.0 {
        total.scaled(1.0 / scaled.target_servings)
    } else {
        Nutrition::default()
    };
    Ok(RecipeNutrition {
        recipe: scaled.recipe.clone(),
        target_servings: scaled.target_servings,
        total,
        per_serving,
    })
}

#[cfg(test)]
mod tests {
    use super::super::scaling::scale_recipe;
    use super::*;

    #[test]
    fn per_serving_is_total_over_servings() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("focaccia").unwrap(); // yields 8
        let scaled = scale_recipe(&book, recipe, 8.0).unwrap();
        let n = nutrition_recipe(&book, &scaled).unwrap();
        assert!((n.per_serving.calories - n.total.calories / 8.0).abs() < 1e-9);
    }

    #[test]
    fn scaling_servings_keeps_per_serving_constant() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("focaccia").unwrap();
        let small = nutrition_recipe(&book, &scale_recipe(&book, recipe, 8.0).unwrap()).unwrap();
        let big = nutrition_recipe(&book, &scale_recipe(&book, recipe, 80.0).unwrap()).unwrap();
        // Per-serving macros are invariant to batch scaling.
        assert!((small.per_serving.calories - big.per_serving.calories).abs() < 1e-6);
        assert!((big.total.calories - small.total.calories * 10.0).abs() < 1e-6);
    }

    #[test]
    fn known_macro_math_for_flour_only() {
        use super::super::types::{Ingredient, Nutrition as N, Recipe, RecipeLine, Unit};
        let ings = vec![Ingredient {
            id: "flour".into(),
            name: "Flour".into(),
            unit: Unit::Gram,
            price_per_base: 0.0,
            nutrition: N {
                calories: 364.0,
                protein_g: 12.0,
                carbs_g: 76.0,
                fat_g: 1.2,
            },
            tags: vec![],
        }];
        let recipe = Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 2.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec![],
            ingredients: vec![RecipeLine {
                ingredient: "flour".into(),
                quantity: 200.0,
                unit: Unit::Gram,
            }],
        };
        let book = KitchenBook::new(ings, vec![recipe.clone()], None).unwrap();
        let n = nutrition_recipe(&book, &scale_recipe(&book, &recipe, 2.0).unwrap()).unwrap();
        // 200 g of a 364 kcal/100g flour = 728 kcal total, 364 per serving (2 servings).
        assert!((n.total.calories - 728.0).abs() < 1e-9);
        assert!((n.per_serving.calories - 364.0).abs() < 1e-9);
    }
}
