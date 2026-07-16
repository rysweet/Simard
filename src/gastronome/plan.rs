//! The menu planner: turn an [`EventBrief`] + [`Menu`] into a fully costed,
//! nutrition-analysed, dietary-screened and prep-scheduled [`MenuPlan`].
//!
//! This is the "brief in, plan out" entry point the Gastronome identity and the
//! `simard-gastronome` CLI both drive.

use serde::{Deserialize, Serialize};

use super::cost::{BudgetStatus, budget_status, recipe_cost, recipe_cost_per_serving};
use super::error::{GastronomeError, GastronomeResult};
use super::nutrition::recipe_nutrition_per_serving;
use super::pantry::{Pantry, RecipeBook};
use super::scaling::scale_recipe;
use super::scheduling::{PrepSchedule, schedule_prep};
use super::types::{
    Allergen, Course, DietaryRestriction, EventBrief, Menu, Nutrition, format_clock,
};

/// A serialisable snapshot of [`BudgetStatus`] for the plan output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum BudgetReport {
    /// No budget was set in the brief.
    NoBudget,
    /// Total cost is within budget.
    WithinBudget { under_by: f64 },
    /// Total cost exceeds budget.
    OverBudget { over_by: f64 },
}

impl From<BudgetStatus> for BudgetReport {
    fn from(status: BudgetStatus) -> Self {
        match status {
            BudgetStatus::NoBudget => Self::NoBudget,
            BudgetStatus::WithinBudget { under_by } => Self::WithinBudget { under_by },
            BudgetStatus::OverBudget { over_by } => Self::OverBudget { over_by },
        }
    }
}

/// A single planned menu line, costed and analysed for the whole guest count.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlannedItem {
    /// The recipe served.
    pub recipe_id: String,
    /// The recipe's display name.
    pub recipe_name: String,
    /// The course it is served as.
    pub course: Course,
    /// Servings prepared (= guest count).
    pub servings: u32,
    /// Total ingredient cost for all servings of this item.
    pub total_cost: f64,
    /// Ingredient cost per serving.
    pub cost_per_serving: f64,
    /// Nutrition of one serving.
    pub nutrition_per_serving: Nutrition,
}

/// A dietary screening failure: an ingredient that violates a brief constraint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DietaryViolation {
    /// The recipe containing the offending ingredient.
    pub recipe_id: String,
    /// The offending ingredient id.
    pub ingredient_id: String,
    /// A human-readable description of what was violated.
    pub issue: String,
}

/// The complete plan a Gastronome produces from a brief.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuPlan {
    /// Event name from the brief.
    pub event_name: String,
    /// Menu name.
    pub menu_name: String,
    /// Guest count planned for.
    pub guests: u32,
    /// Service time formatted `HH:MM`.
    pub service_time: String,
    /// Per-item costed breakdown.
    pub items: Vec<PlannedItem>,
    /// Total ingredient cost for the whole event.
    pub total_cost: f64,
    /// Ingredient cost per guest.
    pub cost_per_guest: f64,
    /// Budget screening result.
    pub budget: BudgetReport,
    /// Aggregate nutrition a single guest receives across the whole menu.
    pub nutrition_per_guest: Nutrition,
    /// Any dietary/allergen screening failures (empty = the menu is compliant).
    pub dietary_violations: Vec<DietaryViolation>,
    /// The prep timetable.
    pub schedule: PrepSchedule,
    /// Non-fatal advisories (e.g. over budget, empty menu).
    pub warnings: Vec<String>,
}

impl MenuPlan {
    /// Whether the plan satisfies every dietary constraint in the brief.
    #[must_use]
    pub fn is_dietary_compliant(&self) -> bool {
        self.dietary_violations.is_empty()
    }
}

/// Plan `menu` for `brief`, resolving recipes and ingredients from the books.
///
/// Each of `brief.guests` guests is assumed to eat one serving of every menu
/// item. Recipes are scaled to the guest count, costed and analysed, screened
/// against the brief's dietary restrictions and excluded allergens, and their
/// prep steps are scheduled to finish at the service time.
///
/// # Errors
/// Returns [`GastronomeError::UnknownRecipe`] / [`GastronomeError::UnknownIngredient`]
/// for dangling references, or a scheduling error for ill-formed prep steps.
pub fn build_menu_plan(
    brief: &EventBrief,
    menu: &Menu,
    pantry: &Pantry,
    book: &RecipeBook,
) -> GastronomeResult<MenuPlan> {
    let guests = brief.guests.max(1);
    let mut items = Vec::new();
    let mut scaled_recipes = Vec::new();
    let mut total_cost = 0.0;
    let mut nutrition_per_guest = Nutrition::default();
    let mut violations = Vec::new();
    let mut warnings = Vec::new();

    if menu.items.is_empty() {
        warnings.push("menu has no items".to_string());
    }

    for item in &menu.items {
        let recipe = book
            .get(&item.recipe_id)
            .ok_or_else(|| GastronomeError::UnknownRecipe {
                recipe_id: item.recipe_id.clone(),
            })?;

        screen_recipe_dietary(recipe, brief, pantry, &mut violations)?;

        let per_serving_nutrition = recipe_nutrition_per_serving(recipe, pantry)?;
        let cost_per_serving = recipe_cost_per_serving(recipe, pantry)?;

        let scaled = scale_recipe(recipe, guests)?;
        let item_cost = recipe_cost(&scaled, pantry)?;
        total_cost += item_cost;
        nutrition_per_guest = nutrition_per_guest.add(&per_serving_nutrition);

        items.push(PlannedItem {
            recipe_id: recipe.id.clone(),
            recipe_name: recipe.name.clone(),
            course: item.course,
            servings: guests,
            total_cost: item_cost,
            cost_per_serving,
            nutrition_per_serving: per_serving_nutrition,
        });
        scaled_recipes.push(scaled);
    }

    let cost_per_guest = total_cost / f64::from(guests);
    let status = budget_status(total_cost, brief.budget_total);
    if let BudgetStatus::OverBudget { over_by } = status {
        warnings.push(format!("over budget by {over_by:.2}"));
    }
    for violation in &violations {
        warnings.push(format!(
            "dietary: {} in recipe '{}' — {}",
            violation.ingredient_id, violation.recipe_id, violation.issue
        ));
    }

    let schedule = schedule_prep(&scaled_recipes, brief.service_time_minutes)?;

    let event_name = if brief.name.is_empty() {
        "Untitled event".to_string()
    } else {
        brief.name.clone()
    };
    let menu_name = if menu.name.is_empty() {
        "Untitled menu".to_string()
    } else {
        menu.name.clone()
    };

    Ok(MenuPlan {
        event_name,
        menu_name,
        guests,
        service_time: format_clock(brief.service_time_minutes),
        items,
        total_cost,
        cost_per_guest,
        budget: status.into(),
        nutrition_per_guest,
        dietary_violations: violations,
        schedule,
        warnings,
    })
}

/// Screen every ingredient of a recipe against the brief's restrictions.
fn screen_recipe_dietary(
    recipe: &super::types::Recipe,
    brief: &EventBrief,
    pantry: &Pantry,
    out: &mut Vec<DietaryViolation>,
) -> GastronomeResult<()> {
    for line in &recipe.ingredients {
        let ingredient =
            pantry
                .get(&line.ingredient_id)
                .ok_or_else(|| GastronomeError::UnknownIngredient {
                    recipe_id: recipe.id.clone(),
                    ingredient_id: line.ingredient_id.clone(),
                })?;

        for restriction in &brief.dietary_restrictions {
            let violates = match restriction {
                DietaryRestriction::Vegetarian => !ingredient.vegetarian,
                DietaryRestriction::Vegan => !ingredient.vegan,
            };
            if violates {
                out.push(DietaryViolation {
                    recipe_id: recipe.id.clone(),
                    ingredient_id: ingredient.id.clone(),
                    issue: format!("not {restriction}"),
                });
            }
        }

        for excluded in &brief.excluded_allergens {
            if ingredient.allergens.contains(excluded) {
                out.push(DietaryViolation {
                    recipe_id: recipe.id.clone(),
                    ingredient_id: ingredient.id.clone(),
                    issue: format!("contains excluded allergen {}", allergen_label(*excluded)),
                });
            }
        }
    }
    Ok(())
}

fn allergen_label(allergen: Allergen) -> String {
    allergen.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::types::{
        Ingredient, MenuItem, PrepStep, Recipe, RecipeIngredient, Unit,
    };
    use std::collections::BTreeSet;

    fn ingredient(
        id: &str,
        cost: f64,
        veg: bool,
        vegan: bool,
        allergens: &[Allergen],
    ) -> Ingredient {
        Ingredient {
            id: id.into(),
            name: id.into(),
            unit: Unit::Gram,
            cost_per_unit: cost,
            nutrition_per_unit: Nutrition {
                calories: 1.0,
                protein_g: 0.1,
                carbs_g: 0.2,
                fat_g: 0.05,
            },
            allergens: allergens.iter().copied().collect(),
            vegetarian: veg,
            vegan,
        }
    }

    fn pantry() -> Pantry {
        Pantry::new([
            ingredient("flour", 0.002, true, true, &[Allergen::Gluten]),
            ingredient("butter", 0.01, true, false, &[Allergen::Dairy]),
            ingredient("chicken", 0.02, false, false, &[]),
        ])
        .unwrap()
    }

    fn book() -> RecipeBook {
        RecipeBook::new([
            Recipe {
                id: "biscuit".into(),
                name: "Biscuit".into(),
                servings: 4,
                ingredients: vec![
                    RecipeIngredient {
                        ingredient_id: "flour".into(),
                        quantity: 200.0,
                    },
                    RecipeIngredient {
                        ingredient_id: "butter".into(),
                        quantity: 80.0,
                    },
                ],
                steps: vec![
                    PrepStep {
                        id: "mix".into(),
                        description: "mix".into(),
                        duration_minutes: 10,
                        depends_on: vec![],
                    },
                    PrepStep {
                        id: "bake".into(),
                        description: "bake".into(),
                        duration_minutes: 20,
                        depends_on: vec!["mix".into()],
                    },
                ],
            },
            Recipe {
                id: "roast".into(),
                name: "Roast Chicken".into(),
                servings: 4,
                ingredients: vec![RecipeIngredient {
                    ingredient_id: "chicken".into(),
                    quantity: 1200.0,
                }],
                steps: vec![PrepStep {
                    id: "cook".into(),
                    description: "roast".into(),
                    duration_minutes: 90,
                    depends_on: vec![],
                }],
            },
        ])
        .unwrap()
    }

    fn brief(guests: u32, budget: Option<f64>, restrictions: &[DietaryRestriction]) -> EventBrief {
        EventBrief {
            name: "Dinner".into(),
            guests,
            service_time_minutes: 19 * 60,
            budget_total: budget,
            dietary_restrictions: restrictions.iter().copied().collect(),
            excluded_allergens: BTreeSet::new(),
        }
    }

    fn menu() -> Menu {
        Menu {
            name: "Set Menu".into(),
            items: vec![
                MenuItem {
                    recipe_id: "biscuit".into(),
                    course: Course::Side,
                },
                MenuItem {
                    recipe_id: "roast".into(),
                    course: Course::Main,
                },
            ],
        }
    }

    #[test]
    fn end_to_end_plan_is_costed_and_scheduled() {
        let plan =
            build_menu_plan(&brief(8, Some(60.0), &[]), &menu(), &pantry(), &book()).unwrap();
        assert_eq!(plan.guests, 8);
        assert_eq!(plan.service_time, "19:00");
        assert_eq!(plan.items.len(), 2);
        // biscuit per-serving cost: (0.002*200 + 0.01*80)/4 = (0.4+0.8)/4 = 0.3; x8 = 2.4
        // roast per-serving: (0.02*1200)/4 = 6; x8 = 48; total = 50.4
        assert!((plan.total_cost - 50.4).abs() < 1e-6);
        assert!((plan.cost_per_guest - 6.3).abs() < 1e-6);
        assert!(matches!(plan.budget, BudgetReport::WithinBudget { .. }));
        // roast critical path 90 min dominates the schedule lead time.
        assert_eq!(plan.schedule.total_lead_minutes, 90);
        assert!(plan.is_dietary_compliant());
    }

    #[test]
    fn over_budget_is_flagged() {
        let plan =
            build_menu_plan(&brief(8, Some(10.0), &[]), &menu(), &pantry(), &book()).unwrap();
        assert!(matches!(plan.budget, BudgetReport::OverBudget { .. }));
        assert!(plan.warnings.iter().any(|w| w.contains("over budget")));
    }

    #[test]
    fn vegetarian_restriction_flags_chicken() {
        let plan = build_menu_plan(
            &brief(4, None, &[DietaryRestriction::Vegetarian]),
            &menu(),
            &pantry(),
            &book(),
        )
        .unwrap();
        assert!(!plan.is_dietary_compliant());
        assert!(
            plan.dietary_violations
                .iter()
                .any(|v| v.ingredient_id == "chicken")
        );
    }

    #[test]
    fn vegan_restriction_flags_butter_and_chicken() {
        let plan = build_menu_plan(
            &brief(4, None, &[DietaryRestriction::Vegan]),
            &menu(),
            &pantry(),
            &book(),
        )
        .unwrap();
        let flagged: BTreeSet<&str> = plan
            .dietary_violations
            .iter()
            .map(|v| v.ingredient_id.as_str())
            .collect();
        assert!(flagged.contains("butter"));
        assert!(flagged.contains("chicken"));
        assert!(!flagged.contains("flour"));
    }

    #[test]
    fn excluded_allergen_is_flagged() {
        let mut b = brief(4, None, &[]);
        b.excluded_allergens.insert(Allergen::Gluten);
        let plan = build_menu_plan(&b, &menu(), &pantry(), &book()).unwrap();
        assert!(
            plan.dietary_violations
                .iter()
                .any(|v| v.issue.contains("gluten"))
        );
    }

    #[test]
    fn unknown_recipe_reference_errors() {
        let bad_menu = Menu {
            name: "Bad".into(),
            items: vec![MenuItem {
                recipe_id: "ghost".into(),
                course: Course::Main,
            }],
        };
        let err = build_menu_plan(&brief(4, None, &[]), &bad_menu, &pantry(), &book()).unwrap_err();
        assert!(matches!(err, GastronomeError::UnknownRecipe { .. }));
    }

    #[test]
    fn empty_menu_warns_but_succeeds() {
        let empty = Menu {
            name: "Empty".into(),
            items: vec![],
        };
        let plan = build_menu_plan(&brief(4, None, &[]), &empty, &pantry(), &book()).unwrap();
        assert!(plan.warnings.iter().any(|w| w.contains("no items")));
        assert!((plan.total_cost).abs() < 1e-9);
    }

    #[test]
    fn plan_roundtrips_through_json() {
        let plan =
            build_menu_plan(&brief(6, Some(100.0), &[]), &menu(), &pantry(), &book()).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        let back: MenuPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }
}
