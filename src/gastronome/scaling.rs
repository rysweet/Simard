//! Recipe scaling: convert a recipe written for its native yield into the
//! quantities needed for a target number of servings, normalised to each
//! ingredient's base unit (grams / millilitres / each) so downstream cost,
//! nutrition, and shopping-list math is unit-consistent.

use serde::{Deserialize, Serialize};

use super::book::KitchenBook;
use super::types::{GastronomeResult, Recipe, Unit};

/// One scaled ingredient requirement, expressed in the ingredient's base unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledLine {
    /// Ingredient id.
    pub ingredient: String,
    /// Human-readable ingredient name (denormalised for reporting).
    pub name: String,
    /// Quantity in the base unit (grams / millilitres / each).
    pub base_quantity: f64,
    /// The base unit the quantity is expressed in.
    pub base_unit: Unit,
}

/// A recipe scaled to a target number of servings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledRecipe {
    /// Recipe id.
    pub recipe: String,
    /// Recipe name.
    pub name: String,
    /// Target servings this scaling produces.
    pub target_servings: f64,
    /// `target_servings / recipe.servings`.
    pub scale_factor: f64,
    /// Scaled ingredient requirements in base units.
    pub lines: Vec<ScaledLine>,
}

/// Scale `recipe` to `target_servings`, resolving each line's ingredient in
/// `book` and converting to that ingredient's base unit.
///
/// # Errors
/// Returns an error if a line references an unknown ingredient. (Unit-family
/// compatibility is guaranteed by [`KitchenBook::validate`], but is re-checked
/// implicitly via the ingredient's base unit.)
pub fn scale_recipe(
    book: &KitchenBook,
    recipe: &Recipe,
    target_servings: f64,
) -> GastronomeResult<ScaledRecipe> {
    // recipe.servings is guaranteed > 0 by book validation.
    let scale_factor = target_servings / recipe.servings;
    let mut lines = Vec::with_capacity(recipe.ingredients.len());
    for line in &recipe.ingredients {
        let ingredient = book.ingredient(&line.ingredient)?;
        let base_quantity = line.quantity * line.unit.to_base_factor() * scale_factor;
        lines.push(ScaledLine {
            ingredient: ingredient.id.clone(),
            name: ingredient.name.clone(),
            base_quantity,
            base_unit: ingredient.unit.base_unit(),
        });
    }
    Ok(ScaledRecipe {
        recipe: recipe.id.clone(),
        name: recipe.name.clone(),
        target_servings,
        scale_factor,
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_quantities_linearly_and_normalises_units() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("focaccia").unwrap(); // yields 8
        let scaled = scale_recipe(&book, recipe, 40.0).unwrap();
        assert_eq!(scaled.scale_factor, 5.0);
        // flour line was 300 g → 1500 g
        let flour = scaled
            .lines
            .iter()
            .find(|l| l.ingredient == "flour")
            .unwrap();
        assert_eq!(flour.base_quantity, 1500.0);
        assert_eq!(flour.base_unit, Unit::Gram);
    }

    #[test]
    fn converts_kilograms_and_litres_to_base_units() {
        use super::super::types::Ingredient;
        use super::super::types::{Nutrition, RecipeLine};
        let ings = vec![
            Ingredient {
                id: "flour".into(),
                name: "Flour".into(),
                unit: Unit::Gram,
                price_per_base: 0.001,
                nutrition: Nutrition::default(),
                tags: vec![],
            },
            Ingredient {
                id: "stock".into(),
                name: "Stock".into(),
                unit: Unit::Milliliter,
                price_per_base: 0.001,
                nutrition: Nutrition::default(),
                tags: vec![],
            },
        ];
        let recipe = Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 2.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec![],
            ingredients: vec![
                RecipeLine {
                    ingredient: "flour".into(),
                    quantity: 1.0,
                    unit: Unit::Kilogram,
                },
                RecipeLine {
                    ingredient: "stock".into(),
                    quantity: 2.0,
                    unit: Unit::Liter,
                },
            ],
        };
        let book = KitchenBook::new(ings, vec![recipe.clone()], None).unwrap();
        let scaled = scale_recipe(&book, &recipe, 2.0).unwrap();
        let flour = scaled
            .lines
            .iter()
            .find(|l| l.ingredient == "flour")
            .unwrap();
        let stock = scaled
            .lines
            .iter()
            .find(|l| l.ingredient == "stock")
            .unwrap();
        assert_eq!(flour.base_quantity, 1000.0); // 1 kg → 1000 g
        assert_eq!(stock.base_quantity, 2000.0); // 2 l → 2000 ml
    }

    #[test]
    fn scaling_down_below_native_yield_works() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("focaccia").unwrap();
        let scaled = scale_recipe(&book, recipe, 4.0).unwrap();
        assert_eq!(scaled.scale_factor, 0.5);
        let flour = scaled
            .lines
            .iter()
            .find(|l| l.ingredient == "flour")
            .unwrap();
        assert_eq!(flour.base_quantity, 150.0);
    }
}
