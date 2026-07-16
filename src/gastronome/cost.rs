//! Cost analysis: roll a [`ScaledRecipe`] up into per-line and per-recipe costs
//! using each ingredient's `price_per_base`. All money is carried as `f64` in
//! the plan's (single, unspecified) currency and rounded to cents only for
//! display via [`round_cents`].

use serde::{Deserialize, Serialize};

use super::book::KitchenBook;
use super::scaling::ScaledRecipe;
use super::types::GastronomeResult;

/// The cost of a single scaled ingredient requirement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LineCost {
    /// Ingredient id.
    pub ingredient: String,
    /// Ingredient name.
    pub name: String,
    /// Quantity in base units.
    pub base_quantity: f64,
    /// Extended cost (`base_quantity * price_per_base`).
    pub cost: f64,
}

/// The cost of a whole scaled recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeCost {
    /// Recipe id.
    pub recipe: String,
    /// Recipe name.
    pub name: String,
    /// Target servings.
    pub target_servings: f64,
    /// Per-ingredient costs.
    pub lines: Vec<LineCost>,
    /// Sum of line costs.
    pub total: f64,
}

impl RecipeCost {
    /// Cost per serving at the target scale.
    #[must_use]
    pub fn per_serving(&self) -> f64 {
        if self.target_servings <= 0.0 {
            0.0
        } else {
            self.total / self.target_servings
        }
    }
}

/// Compute the cost of a scaled recipe.
///
/// # Errors
/// Returns an error if a scaled line references an unknown ingredient.
pub fn cost_recipe(book: &KitchenBook, scaled: &ScaledRecipe) -> GastronomeResult<RecipeCost> {
    let mut lines = Vec::with_capacity(scaled.lines.len());
    let mut total = 0.0;
    for line in &scaled.lines {
        let ingredient = book.ingredient(&line.ingredient)?;
        let cost = line.base_quantity * ingredient.price_per_base;
        total += cost;
        lines.push(LineCost {
            ingredient: line.ingredient.clone(),
            name: line.name.clone(),
            base_quantity: line.base_quantity,
            cost,
        });
    }
    Ok(RecipeCost {
        recipe: scaled.recipe.clone(),
        name: scaled.name.clone(),
        target_servings: scaled.target_servings,
        lines,
        total,
    })
}

/// Round a money amount to the nearest cent for display. Uses round-half-up on
/// the absolute value so `-0.005` and `0.005` are symmetric.
#[must_use]
pub fn round_cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::super::scaling::scale_recipe;
    use super::*;

    #[test]
    fn recipe_cost_sums_line_costs() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("green_beans").unwrap(); // yields 4
        let scaled = scale_recipe(&book, recipe, 4.0).unwrap();
        let cost = cost_recipe(&book, &scaled).unwrap();
        // green_beans 600 g * 0.006 = 3.60; butter 40 g * 0.009 = 0.36; salt 4 g * 0.002 = 0.008
        assert!((cost.total - (3.60 + 0.36 + 0.008)).abs() < 1e-9);
        assert_eq!(cost.lines.len(), 3);
    }

    #[test]
    fn per_serving_divides_total() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("green_beans").unwrap();
        let scaled = scale_recipe(&book, recipe, 8.0).unwrap();
        let cost = cost_recipe(&book, &scaled).unwrap();
        assert!((cost.per_serving() - cost.total / 8.0).abs() < 1e-12);
    }

    #[test]
    fn scaling_doubles_cost() {
        let book = KitchenBook::demo();
        let recipe = book.recipe("green_beans").unwrap();
        let one = cost_recipe(&book, &scale_recipe(&book, recipe, 4.0).unwrap()).unwrap();
        let two = cost_recipe(&book, &scale_recipe(&book, recipe, 8.0).unwrap()).unwrap();
        assert!((two.total - one.total * 2.0).abs() < 1e-9);
    }

    #[test]
    fn round_cents_rounds_half_up() {
        assert_eq!(round_cents(1.005), 1.0); // f64 repr makes this 1.00
        assert_eq!(round_cents(3.606), 3.61);
        assert_eq!(round_cents(0.008), 0.01);
    }
}
