//! Cost analysis: turn per-unit ingredient costs into recipe-, batch-, and
//! per-serving costs.

use super::library::Pantry;
use super::types::{GastronomeResult, Recipe};

/// Total ingredient cost for a recipe at its **base** yield.
pub fn recipe_cost(pantry: &Pantry, recipe: &Recipe) -> GastronomeResult<f64> {
    let mut total = 0.0;
    for line in &recipe.ingredients {
        let ingredient = pantry.ingredient(&line.ingredient_id)?;
        total += ingredient.cost_per_unit * line.quantity;
    }
    Ok(total)
}

/// Cost per single serving of a recipe at its base yield.
pub fn recipe_cost_per_serving(pantry: &Pantry, recipe: &Recipe) -> GastronomeResult<f64> {
    let total = recipe_cost(pantry, recipe)?;
    let servings = recipe.servings.max(1) as f64;
    Ok(total / servings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::library::builtin_pantry;

    #[test]
    fn recipe_cost_sums_lines() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        let cost = recipe_cost(&p, caprese).unwrap();
        // tomato 400*0.004=1.6; mozz 250*0.011=2.75; basil 20*0.03=0.6;
        // oil 40*0.012=0.48; salt 4*0.0008=0.0032. ~5.4332.
        assert!((cost - 5.4332).abs() < 1e-4, "got {cost}");
    }

    #[test]
    fn per_serving_divides_by_yield() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        let total = recipe_cost(&p, caprese).unwrap();
        let per = recipe_cost_per_serving(&p, caprese).unwrap();
        assert!((per * 4.0 - total).abs() < 1e-9);
    }

    #[test]
    fn unknown_ingredient_errors() {
        let p = builtin_pantry();
        let bad = Recipe {
            id: "b".into(),
            name: "bad".into(),
            description: String::new(),
            course: crate::gastronome::types::Course::Side,
            servings: 2,
            ingredients: vec![crate::gastronome::types::RecipeIngredient::new(
                "ghost", 10.0,
            )],
            steps: vec![],
        };
        assert!(recipe_cost(&p, &bad).is_err());
    }
}
