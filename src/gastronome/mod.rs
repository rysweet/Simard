//! Gastronome — a pluggable culinary, menu & event-design engine for Simard.
//!
//! The Gastronome identity designs recipes, menus, and catering/event plans. This
//! module is its deterministic core "brick": given a pantry, a recipe book, a
//! menu, and an event brief, it produces a fully **costed, nutrition-analysed,
//! dietary-screened, and prep-scheduled** [`MenuPlan`] — the end-to-end
//! "brief → plan" contract — with no I/O, clocks, or network in the core.
//!
//! ## Layout
//! - [`types`] — value objects (ingredients, recipes, prep steps, menus, briefs).
//! - [`pantry`] — indexed [`Pantry`] and [`RecipeBook`] with id-integrity checks.
//! - [`nutrition`] — macro-nutrition aggregation (total and per serving).
//! - [`cost`] — ingredient costing and budget screening.
//! - [`scaling`] — scale a recipe to a target serving/guest count.
//! - [`scheduling`] — backward critical-path prep scheduling from service time.
//! - [`plan`] — the [`build_menu_plan`] orchestrator that ties it all together.
//!
//! The `simard-gastronome` CLI binary is a thin "kitchen app" over
//! [`plan_from_bundle`] plus [`demo_bundle`] and [`render_plan_text`].

pub mod cost;
pub mod error;
pub mod nutrition;
pub mod pantry;
pub mod plan;
pub mod scaling;
pub mod scheduling;
pub mod types;

use serde::{Deserialize, Serialize};

pub use cost::{BudgetStatus, budget_status, recipe_cost, recipe_cost_per_serving};
pub use error::{GastronomeError, GastronomeResult};
pub use nutrition::{recipe_nutrition, recipe_nutrition_per_serving};
pub use pantry::{Pantry, RecipeBook};
pub use plan::{BudgetReport, DietaryViolation, MenuPlan, PlannedItem, build_menu_plan};
pub use scaling::{scale_factor, scale_recipe};
pub use scheduling::{PrepSchedule, ScheduledTask, schedule_prep};
pub use types::{
    Allergen, Course, DietaryRestriction, EventBrief, Ingredient, Menu, MenuItem, Nutrition,
    PrepStep, Recipe, RecipeIngredient, Unit, format_clock,
};

/// A self-contained "kitchen brief bundle": everything the planner needs in one
/// serialisable document. This is the on-disk input format the CLI reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KitchenBrief {
    /// The pantry of available ingredients.
    pub ingredients: Vec<Ingredient>,
    /// The recipe book.
    pub recipes: Vec<Recipe>,
    /// The menu to serve.
    pub menu: Menu,
    /// The event brief to plan against.
    pub brief: EventBrief,
}

/// Build a [`MenuPlan`] straight from a [`KitchenBrief`] bundle.
///
/// # Errors
/// Returns any [`GastronomeError`] from building the pantry/recipe book (duplicate
/// ids, invalid values) or from planning (dangling references, cyclic prep steps).
pub fn plan_from_bundle(bundle: &KitchenBrief) -> GastronomeResult<MenuPlan> {
    let pantry = Pantry::new(bundle.ingredients.iter().cloned())?;
    let book = RecipeBook::new(bundle.recipes.iter().cloned())?;
    build_menu_plan(&bundle.brief, &bundle.menu, &pantry, &book)
}

/// A small built-in sample brief so the CLI runs end-to-end with zero external
/// files (`simard-gastronome --demo`).
#[must_use]
pub fn demo_bundle() -> KitchenBrief {
    use std::collections::BTreeSet;

    let ingredients = vec![
        Ingredient {
            id: "flour".into(),
            name: "All-purpose flour".into(),
            unit: Unit::Gram,
            cost_per_unit: 0.0018,
            nutrition_per_unit: Nutrition {
                calories: 3.64,
                protein_g: 0.10,
                carbs_g: 0.76,
                fat_g: 0.01,
            },
            allergens: [Allergen::Gluten].into_iter().collect(),
            vegetarian: true,
            vegan: true,
        },
        Ingredient {
            id: "butter".into(),
            name: "Butter".into(),
            unit: Unit::Gram,
            cost_per_unit: 0.011,
            nutrition_per_unit: Nutrition {
                calories: 7.17,
                protein_g: 0.01,
                carbs_g: 0.0,
                fat_g: 0.81,
            },
            allergens: [Allergen::Dairy].into_iter().collect(),
            vegetarian: true,
            vegan: false,
        },
        Ingredient {
            id: "chicken".into(),
            name: "Chicken thigh".into(),
            unit: Unit::Gram,
            cost_per_unit: 0.008,
            nutrition_per_unit: Nutrition {
                calories: 2.09,
                protein_g: 0.18,
                carbs_g: 0.0,
                fat_g: 0.15,
            },
            allergens: BTreeSet::new(),
            vegetarian: false,
            vegan: false,
        },
        Ingredient {
            id: "potato".into(),
            name: "Potato".into(),
            unit: Unit::Gram,
            cost_per_unit: 0.002,
            nutrition_per_unit: Nutrition {
                calories: 0.77,
                protein_g: 0.02,
                carbs_g: 0.17,
                fat_g: 0.001,
            },
            allergens: BTreeSet::new(),
            vegetarian: true,
            vegan: true,
        },
        Ingredient {
            id: "olive-oil".into(),
            name: "Olive oil".into(),
            unit: Unit::Milliliter,
            cost_per_unit: 0.012,
            nutrition_per_unit: Nutrition {
                calories: 8.84,
                protein_g: 0.0,
                carbs_g: 0.0,
                fat_g: 1.0,
            },
            allergens: BTreeSet::new(),
            vegetarian: true,
            vegan: true,
        },
        Ingredient {
            id: "berries".into(),
            name: "Mixed berries".into(),
            unit: Unit::Gram,
            cost_per_unit: 0.015,
            nutrition_per_unit: Nutrition {
                calories: 0.57,
                protein_g: 0.007,
                carbs_g: 0.14,
                fat_g: 0.003,
            },
            allergens: BTreeSet::new(),
            vegetarian: true,
            vegan: true,
        },
    ];

    let recipes = vec![
        Recipe {
            id: "roast-chicken".into(),
            name: "Herb roast chicken".into(),
            servings: 4,
            ingredients: vec![
                RecipeIngredient {
                    ingredient_id: "chicken".into(),
                    quantity: 1200.0,
                },
                RecipeIngredient {
                    ingredient_id: "olive-oil".into(),
                    quantity: 30.0,
                },
            ],
            steps: vec![
                PrepStep {
                    id: "season".into(),
                    description: "Season and truss chicken".into(),
                    duration_minutes: 15,
                    depends_on: vec![],
                },
                PrepStep {
                    id: "roast".into(),
                    description: "Roast until cooked through".into(),
                    duration_minutes: 75,
                    depends_on: vec!["season".into()],
                },
                PrepStep {
                    id: "rest".into(),
                    description: "Rest before carving".into(),
                    duration_minutes: 15,
                    depends_on: vec!["roast".into()],
                },
            ],
        },
        Recipe {
            id: "roast-potatoes".into(),
            name: "Roast potatoes".into(),
            servings: 4,
            ingredients: vec![
                RecipeIngredient {
                    ingredient_id: "potato".into(),
                    quantity: 800.0,
                },
                RecipeIngredient {
                    ingredient_id: "olive-oil".into(),
                    quantity: 40.0,
                },
            ],
            steps: vec![
                PrepStep {
                    id: "parboil".into(),
                    description: "Parboil potatoes".into(),
                    duration_minutes: 15,
                    depends_on: vec![],
                },
                PrepStep {
                    id: "roast".into(),
                    description: "Roast until crisp".into(),
                    duration_minutes: 45,
                    depends_on: vec!["parboil".into()],
                },
            ],
        },
        Recipe {
            id: "berry-shortbread".into(),
            name: "Berry shortbread".into(),
            servings: 8,
            ingredients: vec![
                RecipeIngredient {
                    ingredient_id: "flour".into(),
                    quantity: 300.0,
                },
                RecipeIngredient {
                    ingredient_id: "butter".into(),
                    quantity: 200.0,
                },
                RecipeIngredient {
                    ingredient_id: "berries".into(),
                    quantity: 200.0,
                },
            ],
            steps: vec![
                PrepStep {
                    id: "mix".into(),
                    description: "Mix and chill dough".into(),
                    duration_minutes: 20,
                    depends_on: vec![],
                },
                PrepStep {
                    id: "bake".into(),
                    description: "Bake shortbread".into(),
                    duration_minutes: 25,
                    depends_on: vec!["mix".into()],
                },
                PrepStep {
                    id: "top".into(),
                    description: "Top with berries".into(),
                    duration_minutes: 10,
                    depends_on: vec!["bake".into()],
                },
            ],
        },
    ];

    let menu = Menu {
        name: "Autumn supper".into(),
        items: vec![
            MenuItem {
                recipe_id: "roast-chicken".into(),
                course: Course::Main,
            },
            MenuItem {
                recipe_id: "roast-potatoes".into(),
                course: Course::Side,
            },
            MenuItem {
                recipe_id: "berry-shortbread".into(),
                course: Course::Dessert,
            },
        ],
    };

    let brief = EventBrief {
        name: "Harvest dinner".into(),
        guests: 12,
        service_time_minutes: 19 * 60 + 30,
        budget_total: Some(120.0),
        dietary_restrictions: BTreeSet::new(),
        excluded_allergens: BTreeSet::new(),
    };

    KitchenBrief {
        ingredients,
        recipes,
        menu,
        brief,
    }
}

/// Render a [`MenuPlan`] as a human-readable kitchen brief / prep sheet.
#[must_use]
pub fn render_plan_text(plan: &MenuPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} — {}\n", plan.event_name, plan.menu_name));
    out.push_str(&format!(
        "Guests: {}   Service: {}\n\n",
        plan.guests, plan.service_time
    ));

    out.push_str("## Menu & cost\n");
    for item in &plan.items {
        out.push_str(&format!(
            "- [{}] {} — {} servings, {:.2}/serving, {:.2} total\n",
            item.course, item.recipe_name, item.servings, item.cost_per_serving, item.total_cost
        ));
    }
    out.push_str(&format!(
        "\nTotal cost: {:.2}   Per guest: {:.2}\n",
        plan.total_cost, plan.cost_per_guest
    ));
    match plan.budget {
        BudgetReport::NoBudget => out.push_str("Budget: (none set)\n"),
        BudgetReport::WithinBudget { under_by } => {
            out.push_str(&format!("Budget: within budget ({under_by:.2} to spare)\n"));
        }
        BudgetReport::OverBudget { over_by } => {
            out.push_str(&format!("Budget: OVER by {over_by:.2}\n"));
        }
    }

    out.push_str(&format!(
        "\n## Nutrition per guest\n{:.0} kcal, {:.1} g protein, {:.1} g carbs, {:.1} g fat\n",
        plan.nutrition_per_guest.calories,
        plan.nutrition_per_guest.protein_g,
        plan.nutrition_per_guest.carbs_g,
        plan.nutrition_per_guest.fat_g,
    ));

    out.push_str(&format!(
        "\n## Prep schedule (kitchen starts {}, {} min lead)\n",
        format_clock(
            u32::try_from(plan.schedule.kitchen_start_minutes.rem_euclid(24 * 60)).unwrap_or(0)
        ),
        plan.schedule.total_lead_minutes,
    ));
    for task in &plan.schedule.tasks {
        out.push_str(&format!(
            "- {}–{}  {} · {}\n",
            task.start_clock(),
            task.end_clock(),
            task.recipe_name,
            task.description,
        ));
    }

    if !plan.dietary_violations.is_empty() {
        out.push_str("\n## Dietary violations\n");
        for v in &plan.dietary_violations {
            out.push_str(&format!(
                "- {} in {} — {}\n",
                v.ingredient_id, v.recipe_id, v.issue
            ));
        }
    }

    if !plan.warnings.is_empty() {
        out.push_str("\n## Warnings\n");
        for w in &plan.warnings {
            out.push_str(&format!("- {w}\n"));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_bundle_plans_end_to_end() {
        let plan = plan_from_bundle(&demo_bundle()).unwrap();
        assert_eq!(plan.guests, 12);
        assert_eq!(plan.items.len(), 3);
        assert!(plan.total_cost > 0.0);
        // roast-chicken critical path (15+75+15 = 105) dominates.
        assert_eq!(plan.schedule.total_lead_minutes, 105);
        assert!(plan.is_dietary_compliant());
    }

    #[test]
    fn demo_bundle_roundtrips_through_json() {
        let bundle = demo_bundle();
        let json = serde_json::to_string(&bundle).unwrap();
        let back: KitchenBrief = serde_json::from_str(&json).unwrap();
        assert_eq!(bundle, back);
    }

    #[test]
    fn render_text_contains_key_sections() {
        let plan = plan_from_bundle(&demo_bundle()).unwrap();
        let text = render_plan_text(&plan);
        assert!(text.contains("Menu & cost"));
        assert!(text.contains("Prep schedule"));
        assert!(text.contains("Nutrition per guest"));
        assert!(text.contains("Total cost"));
    }

    #[test]
    fn plan_from_bundle_rejects_duplicate_ingredient() {
        let mut bundle = demo_bundle();
        let dup = bundle.ingredients[0].clone();
        bundle.ingredients.push(dup);
        let err = plan_from_bundle(&bundle).unwrap_err();
        assert!(matches!(err, GastronomeError::DuplicateId { .. }));
    }
}
