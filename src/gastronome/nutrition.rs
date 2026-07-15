//! Nutrition analysis: aggregate per-unit ingredient facts into recipe-,
//! batch-, and per-serving totals.

use super::library::Pantry;
use super::types::{GastronomeResult, NutritionFacts, Recipe};

/// Total nutrition for a recipe at its **base** yield (all ingredient lines
/// summed at their stated quantities).
pub fn recipe_nutrition(pantry: &Pantry, recipe: &Recipe) -> GastronomeResult<NutritionFacts> {
    let mut total = NutritionFacts::default();
    for line in &recipe.ingredients {
        let ingredient = pantry.ingredient(&line.ingredient_id)?;
        total = total + ingredient.nutrition_per_unit.scaled(line.quantity);
    }
    Ok(total)
}

/// Nutrition per single serving of a recipe at its base yield.
pub fn recipe_nutrition_per_serving(
    pantry: &Pantry,
    recipe: &Recipe,
) -> GastronomeResult<NutritionFacts> {
    let total = recipe_nutrition(pantry, recipe)?;
    let servings = recipe.servings.max(1) as f64;
    Ok(total.scaled(1.0 / servings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::library::builtin_pantry;

    #[test]
    fn recipe_nutrition_sums_lines() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        let total = recipe_nutrition(&p, caprese).unwrap();
        // 400g tomato + 250g mozzarella + 20g basil + 40ml oil + 4g salt.
        // Tomato: 0.18*400 = 72 cal; mozzarella 2.80*250 = 700; basil 0.23*20 = 4.6;
        // oil 8.84*40 = 353.6; salt 0. Total ~1130.2.
        assert!(
            (total.calories - 1130.2).abs() < 1.0,
            "got {}",
            total.calories
        );
        assert!(total.protein_g > 0.0);
    }

    #[test]
    fn per_serving_divides_by_yield() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        let total = recipe_nutrition(&p, caprese).unwrap();
        let per = recipe_nutrition_per_serving(&p, caprese).unwrap();
        assert!((per.calories * 4.0 - total.calories).abs() < 1e-6);
    }

    #[test]
    fn empty_recipe_is_zero() {
        let p = builtin_pantry();
        let empty = Recipe {
            id: "e".into(),
            name: "empty".into(),
            description: String::new(),
            course: crate::gastronome::types::Course::Side,
            servings: 2,
            ingredients: vec![],
            steps: vec![],
        };
        let total = recipe_nutrition(&p, &empty).unwrap();
        assert_eq!(total, NutritionFacts::default());
    }
}
