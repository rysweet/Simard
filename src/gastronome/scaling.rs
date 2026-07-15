//! Recipe scaling: take a base recipe to the servings an event needs, and
//! resolve each ingredient line into a concrete, costed quantity.

use super::cost::recipe_cost;
use super::library::Pantry;
use super::nutrition::recipe_nutrition;
use super::types::{
    GastronomeError, GastronomeResult, Recipe, ScaledIngredientLine, ScaledRecipe, round1, round2,
};

/// Scale `recipe` from its base yield up (or down) to `target_servings`,
/// resolving ingredient names/units and computing per-line and total cost and
/// nutrition for the scaled batch.
pub fn scale_recipe(
    pantry: &Pantry,
    recipe: &Recipe,
    target_servings: u32,
) -> GastronomeResult<ScaledRecipe> {
    if target_servings == 0 {
        return Err(GastronomeError::InvalidQuantity {
            field: "target_servings".into(),
        });
    }
    if recipe.servings == 0 {
        return Err(GastronomeError::InvalidQuantity {
            field: format!("recipe '{}' base servings", recipe.id),
        });
    }

    let scale_factor = target_servings as f64 / recipe.servings as f64;

    let mut lines = Vec::with_capacity(recipe.ingredients.len());
    for line in &recipe.ingredients {
        let ingredient = pantry.ingredient(&line.ingredient_id)?;
        let scaled_qty = line.quantity * scale_factor;
        let line_cost = ingredient.cost_per_unit * scaled_qty;
        lines.push(ScaledIngredientLine {
            ingredient_id: ingredient.id.clone(),
            name: ingredient.name.clone(),
            unit: ingredient.unit.clone(),
            quantity: round1(scaled_qty),
            line_cost: round2(line_cost),
        });
    }

    let nutrition_total = recipe_nutrition(pantry, recipe)?
        .scaled(scale_factor)
        .rounded();
    let cost_total = round2(recipe_cost(pantry, recipe)? * scale_factor);

    Ok(ScaledRecipe {
        recipe_id: recipe.id.clone(),
        name: recipe.name.clone(),
        course: recipe.course,
        base_servings: recipe.servings,
        target_servings,
        scale_factor,
        ingredients: lines,
        nutrition_total,
        cost_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::library::builtin_pantry;

    #[test]
    fn doubling_servings_doubles_quantities() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap(); // base 4
        let scaled = scale_recipe(&p, caprese, 8).unwrap();
        assert_eq!(scaled.scale_factor, 2.0);
        // tomato base 400 → 800
        let tomato = scaled
            .ingredients
            .iter()
            .find(|l| l.ingredient_id == "tomato")
            .unwrap();
        assert_eq!(tomato.quantity, 800.0);
    }

    #[test]
    fn fractional_scale_is_supported() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap(); // base 4
        let scaled = scale_recipe(&p, caprese, 6).unwrap();
        assert!((scaled.scale_factor - 1.5).abs() < 1e-9);
        let tomato = scaled
            .ingredients
            .iter()
            .find(|l| l.ingredient_id == "tomato")
            .unwrap();
        assert_eq!(tomato.quantity, 600.0);
    }

    #[test]
    fn zero_target_errors() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        assert!(scale_recipe(&p, caprese, 0).is_err());
    }

    #[test]
    fn scaled_cost_tracks_factor() {
        let p = builtin_pantry();
        let caprese = p.recipe("caprese").unwrap();
        let one = scale_recipe(&p, caprese, 4).unwrap();
        let two = scale_recipe(&p, caprese, 8).unwrap();
        assert!((two.cost_total - one.cost_total * 2.0).abs() < 0.01);
    }
}
