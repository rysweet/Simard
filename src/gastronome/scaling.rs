//! Scaling a recipe from its authored `base_servings` up (or down) to the guest
//! count a brief demands.
//!
//! Ingredient quantities and nutrition are authored *per serving*, so scaling to
//! `target_servings` is a linear multiply. Prep, however, happens in *batches*:
//! a recipe written for 4 can only be cooked in whole multiples of 4, so the
//! batch count is `ceil(target / base)`. Both views matter — the shopping list
//! wants exact quantities, the schedule wants whole batches.

use serde::{Deserialize, Serialize};

use super::nutrition::Nutrition;
use super::types::Recipe;

/// One aggregated shopping-list line: how much of an ingredient to buy and what
/// it costs at the scaled quantity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShoppingLine {
    /// Ingredient name.
    pub name: String,
    /// Purchase unit.
    pub unit: String,
    /// Total quantity required across the scaled recipe(s).
    pub quantity: f64,
    /// Cost of one unit.
    pub unit_cost: f64,
    /// `quantity * unit_cost`.
    pub line_cost: f64,
}

/// A recipe resolved to a concrete guest count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledRecipe {
    /// The underlying recipe.
    pub recipe: Recipe,
    /// The number of servings this scaling targets.
    pub target_servings: u32,
}

impl ScaledRecipe {
    /// Scale `recipe` to `target_servings`.
    #[must_use]
    pub fn new(recipe: Recipe, target_servings: u32) -> Self {
        Self {
            recipe,
            target_servings,
        }
    }

    /// Number of whole batches needed to yield at least `target_servings`.
    ///
    /// A recipe authored for `base_servings` cooks in whole batches, so a
    /// 4-serving recipe catering 10 guests needs 3 batches (12 servings).
    /// Returns 0 only when `base_servings` is 0 (an invalid recipe the planner
    /// rejects earlier); callers should treat 0 as "cannot batch".
    #[must_use]
    pub fn batches(&self) -> u32 {
        let base = self.recipe.base_servings;
        if base == 0 {
            return 0;
        }
        self.target_servings.div_ceil(base)
    }

    /// Total ingredient cost at the target serving count.
    #[must_use]
    pub fn total_cost(&self) -> f64 {
        self.recipe.cost_per_serving() * f64::from(self.target_servings)
    }

    /// Total nutrition served across all guests (per-serving nutrition times the
    /// serving count).
    #[must_use]
    pub fn total_nutrition(&self) -> Nutrition {
        self.recipe
            .nutrition_per_serving()
            .scale(f64::from(self.target_servings))
    }

    /// Per-ingredient scaled shopping lines for exactly this recipe.
    #[must_use]
    pub fn shopping_lines(&self) -> Vec<ShoppingLine> {
        let servings = f64::from(self.target_servings);
        self.recipe
            .ingredients
            .iter()
            .map(|line| {
                let quantity = line.quantity * servings;
                ShoppingLine {
                    name: line.ingredient.name.clone(),
                    unit: line.ingredient.unit.clone(),
                    quantity,
                    unit_cost: line.ingredient.cost_per_unit,
                    line_cost: quantity * line.ingredient.cost_per_unit,
                }
            })
            .collect()
    }
}

/// A resolved menu line: the course it fills and the recipe scaled to the
/// event's guest count. This is the unit cost, nutrition, and prep scheduling
/// all operate over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    /// The course this dish was selected for.
    pub course: String,
    /// The chosen recipe, scaled to the guest count.
    pub scaled: ScaledRecipe,
}

/// Aggregate shopping lines from several scaled recipes into one consolidated
/// list, merging identical `(name, unit)` pairs and summing their quantities and
/// costs. The result is sorted by ingredient name for a stable, human-friendly
/// order.
#[must_use]
pub fn consolidate_shopping(scaled: &[ScaledRecipe]) -> Vec<ShoppingLine> {
    let mut merged: Vec<ShoppingLine> = Vec::new();
    for recipe in scaled {
        for line in recipe.shopping_lines() {
            if let Some(existing) = merged
                .iter_mut()
                .find(|l| l.name == line.name && l.unit == line.unit)
            {
                existing.quantity += line.quantity;
                existing.line_cost += line.line_cost;
            } else {
                merged.push(line);
            }
        }
    }
    merged.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.unit.cmp(&b.unit)));
    merged
}

#[cfg(test)]
mod tests {
    use super::super::types::{Ingredient, RecipeIngredient};
    use super::*;

    fn ingredient(name: &str, unit: &str, cost: f64) -> Ingredient {
        Ingredient {
            name: name.into(),
            unit: unit.into(),
            cost_per_unit: cost,
            nutrition_per_unit: Nutrition::new(100.0, 5.0, 10.0, 2.0),
        }
    }

    fn soup() -> Recipe {
        Recipe {
            id: "soup".into(),
            name: "Tomato Soup".into(),
            course: "starter".into(),
            base_servings: 4,
            dietary_tags: vec![],
            ingredients: vec![
                RecipeIngredient {
                    ingredient: ingredient("tomato", "kg", 3.0),
                    quantity: 0.2,
                },
                RecipeIngredient {
                    ingredient: ingredient("stock", "litre", 1.0),
                    quantity: 0.25,
                },
            ],
            steps: vec![],
        }
    }

    #[test]
    fn batches_round_up() {
        assert_eq!(ScaledRecipe::new(soup(), 4).batches(), 1);
        assert_eq!(ScaledRecipe::new(soup(), 5).batches(), 2);
        assert_eq!(ScaledRecipe::new(soup(), 8).batches(), 2);
        assert_eq!(ScaledRecipe::new(soup(), 10).batches(), 3);
    }

    #[test]
    fn total_cost_scales_linearly_with_servings() {
        // per serving: 0.2*3 + 0.25*1 = 0.85
        let s = ScaledRecipe::new(soup(), 10);
        assert!((s.total_cost() - 8.5).abs() < 1e-9);
    }

    #[test]
    fn shopping_lines_scale_quantities() {
        let s = ScaledRecipe::new(soup(), 10);
        let lines = s.shopping_lines();
        let tomato = lines.iter().find(|l| l.name == "tomato").unwrap();
        assert!((tomato.quantity - 2.0).abs() < 1e-9);
        assert!((tomato.line_cost - 6.0).abs() < 1e-9);
    }

    #[test]
    fn consolidate_merges_shared_ingredients() {
        // Two recipes both using "tomato/kg" should merge into one line.
        let a = ScaledRecipe::new(soup(), 4); // 0.8 kg tomato
        let b = ScaledRecipe::new(soup(), 4); // 0.8 kg tomato
        let merged = consolidate_shopping(&[a, b]);
        let tomato = merged.iter().find(|l| l.name == "tomato").unwrap();
        assert!((tomato.quantity - 1.6).abs() < 1e-9);
        // Sorted by name: stock comes after tomato? s < t, so stock first.
        assert_eq!(merged[0].name, "stock");
        assert_eq!(merged[1].name, "tomato");
    }
}
