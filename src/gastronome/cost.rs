//! Cost aggregation: roll a recipe's ingredient costs up to the whole recipe and
//! down to a single serving, and screen a total against a budget.

use super::error::{GastronomeError, GastronomeResult};
use super::pantry::Pantry;
use super::types::Recipe;

/// Total ingredient cost for a recipe's full base yield, in the plan currency.
///
/// # Errors
/// Returns [`GastronomeError::UnknownIngredient`] if a line references an
/// ingredient absent from `pantry`.
pub fn recipe_cost(recipe: &Recipe, pantry: &Pantry) -> GastronomeResult<f64> {
    let mut total = 0.0;
    for line in &recipe.ingredients {
        let ingredient =
            pantry
                .get(&line.ingredient_id)
                .ok_or_else(|| GastronomeError::UnknownIngredient {
                    recipe_id: recipe.id.clone(),
                    ingredient_id: line.ingredient_id.clone(),
                })?;
        total += ingredient.cost_per_unit * line.quantity;
    }
    Ok(total)
}

/// Ingredient cost of a single serving of a recipe.
///
/// # Errors
/// See [`recipe_cost`].
pub fn recipe_cost_per_serving(recipe: &Recipe, pantry: &Pantry) -> GastronomeResult<f64> {
    let total = recipe_cost(recipe, pantry)?;
    Ok(total / f64::from(recipe.servings.max(1)))
}

/// The outcome of comparing a plan's total cost against a budget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BudgetStatus {
    /// No budget was provided in the brief.
    NoBudget,
    /// Total cost is within (or equal to) the budget; carries the slack.
    WithinBudget { under_by: f64 },
    /// Total cost exceeds the budget; carries the overrun.
    OverBudget { over_by: f64 },
}

/// Screen a total cost against an optional budget.
#[must_use]
pub fn budget_status(total_cost: f64, budget: Option<f64>) -> BudgetStatus {
    match budget {
        None => BudgetStatus::NoBudget,
        Some(budget) if total_cost <= budget => BudgetStatus::WithinBudget {
            under_by: budget - total_cost,
        },
        Some(budget) => BudgetStatus::OverBudget {
            over_by: total_cost - budget,
        },
    }
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
                nutrition_per_unit: Default::default(),
                allergens: Default::default(),
                vegetarian: true,
                vegan: true,
            },
            Ingredient {
                id: "butter".into(),
                name: "Butter".into(),
                unit: Unit::Gram,
                cost_per_unit: 0.01,
                nutrition_per_unit: Default::default(),
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
    fn total_cost_sums_ingredient_lines() {
        // flour: 0.002*100 = 0.2 ; butter: 0.01*50 = 0.5 => 0.7
        let c = recipe_cost(&recipe(), &pantry()).unwrap();
        assert!((c - 0.7).abs() < 1e-9);
    }

    #[test]
    fn per_serving_divides_total() {
        let c = recipe_cost_per_serving(&recipe(), &pantry()).unwrap();
        assert!((c - 0.175).abs() < 1e-9);
    }

    #[test]
    fn budget_status_covers_all_branches() {
        assert_eq!(budget_status(5.0, None), BudgetStatus::NoBudget);
        match budget_status(5.0, Some(8.0)) {
            BudgetStatus::WithinBudget { under_by } => assert!((under_by - 3.0).abs() < 1e-9),
            other => panic!("expected within budget, got {other:?}"),
        }
        match budget_status(10.0, Some(8.0)) {
            BudgetStatus::OverBudget { over_by } => assert!((over_by - 2.0).abs() < 1e-9),
            other => panic!("expected over budget, got {other:?}"),
        }
        // exactly on budget counts as within
        assert!(matches!(
            budget_status(8.0, Some(8.0)),
            BudgetStatus::WithinBudget { .. }
        ));
    }
}
