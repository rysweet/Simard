//! Cost and nutrition analysis over a resolved menu.
//!
//! Both summaries are pure reductions of the [`MenuItem`] list the planner
//! produces: cost sums ingredient spend per course and divides by guests;
//! nutrition sums one serving of each course into a per-guest plate and scales
//! that by the guest count for the catering total.

use serde::{Deserialize, Serialize};

use super::nutrition::Nutrition;
use super::scaling::MenuItem;

/// Cost of one course within the menu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CourseCost {
    /// Course name.
    pub course: String,
    /// Chosen recipe id.
    pub recipe_id: String,
    /// Chosen recipe name.
    pub recipe_name: String,
    /// Whole batches cooked for this course.
    pub batches: u32,
    /// Total ingredient cost for this course across all guests.
    pub total_cost: f64,
}

/// Whole-menu cost breakdown.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostSummary {
    /// Guests catered for.
    pub guests: u32,
    /// Per-course cost lines, in menu order.
    pub per_course: Vec<CourseCost>,
    /// Sum of every course's cost.
    pub total_cost: f64,
    /// `total_cost / guests`.
    pub cost_per_guest: f64,
}

impl CostSummary {
    /// Build the cost summary for `menu` catering `guests`.
    ///
    /// `guests` is taken as an explicit argument (rather than read from an
    /// item) so an empty menu still reports a meaningful per-guest figure of 0.
    #[must_use]
    pub fn compute(menu: &[MenuItem], guests: u32) -> Self {
        let per_course: Vec<CourseCost> = menu
            .iter()
            .map(|item| CourseCost {
                course: item.course.clone(),
                recipe_id: item.scaled.recipe.id.clone(),
                recipe_name: item.scaled.recipe.name.clone(),
                batches: item.scaled.batches(),
                total_cost: round2(item.scaled.total_cost()),
            })
            .collect();
        let total_cost = round2(per_course.iter().map(|c| c.total_cost).sum());
        let cost_per_guest = if guests == 0 {
            0.0
        } else {
            round2(total_cost / f64::from(guests))
        };
        Self {
            guests,
            per_course,
            total_cost,
            cost_per_guest,
        }
    }
}

/// Per-guest and whole-event nutrition rollup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NutritionSummary {
    /// Nutrition on a single guest's plate (one serving of every course).
    pub per_guest: Nutrition,
    /// Nutrition across the whole event (`per_guest * guests`).
    pub event_total: Nutrition,
}

impl NutritionSummary {
    /// Build the nutrition summary for `menu` catering `guests`.
    #[must_use]
    pub fn compute(menu: &[MenuItem], guests: u32) -> Self {
        let per_guest = menu
            .iter()
            .map(|item| item.scaled.recipe.nutrition_per_serving())
            .fold(Nutrition::default(), Nutrition::sum2);
        Self {
            per_guest: per_guest.rounded(),
            event_total: per_guest.scale(f64::from(guests)).rounded(),
        }
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::super::scaling::ScaledRecipe;
    use super::super::types::{Ingredient, Recipe, RecipeIngredient};
    use super::*;

    fn dish(id: &str, course: &str, cost_per_serving: f64, cal: f64) -> Recipe {
        Recipe {
            id: id.into(),
            name: id.to_uppercase(),
            course: course.into(),
            base_servings: 4,
            dietary_tags: vec![],
            ingredients: vec![RecipeIngredient {
                ingredient: Ingredient {
                    name: format!("{id}-stuff"),
                    unit: "each".into(),
                    cost_per_unit: cost_per_serving,
                    nutrition_per_unit: Nutrition::new(cal, 1.0, 2.0, 3.0),
                },
                quantity: 1.0,
            }],
            steps: vec![],
        }
    }

    fn menu() -> Vec<MenuItem> {
        vec![
            MenuItem {
                course: "starter".into(),
                scaled: ScaledRecipe::new(dish("soup", "starter", 1.0, 100.0), 10),
            },
            MenuItem {
                course: "main".into(),
                scaled: ScaledRecipe::new(dish("stew", "main", 3.5, 500.0), 10),
            },
        ]
    }

    #[test]
    fn cost_summary_totals_and_per_guest() {
        let s = CostSummary::compute(&menu(), 10);
        // soup: 1.0*10 = 10, stew: 3.5*10 = 35, total 45, per guest 4.5
        assert!((s.total_cost - 45.0).abs() < 1e-9);
        assert!((s.cost_per_guest - 4.5).abs() < 1e-9);
        assert_eq!(s.per_course.len(), 2);
        assert_eq!(s.per_course[0].batches, 3); // ceil(10/4)
    }

    #[test]
    fn cost_summary_empty_menu_is_zero() {
        let s = CostSummary::compute(&[], 10);
        assert_eq!(s.total_cost, 0.0);
        assert_eq!(s.cost_per_guest, 0.0);
    }

    #[test]
    fn nutrition_summary_per_guest_is_one_of_each_course() {
        let n = NutritionSummary::compute(&menu(), 10);
        // per guest: 100 + 500 = 600 cal
        assert!((n.per_guest.calories - 600.0).abs() < 1e-6);
        // event total: 600 * 10 = 6000
        assert!((n.event_total.calories - 6000.0).abs() < 1e-6);
    }
}
