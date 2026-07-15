//! Unit tests for the gastronome engine. All deterministic — no LLM provider.

use chrono::{TimeZone, Utc};

use super::*;
use crate::gastronome::cost::aggregate_cost;
use crate::gastronome::nutrition::per_guest_nutrition;
use crate::gastronome::types::{
    Course, DietaryTag, EventBrief, Ingredient, Nutrition, PrepStep, Recipe, RecipeIngredient, Unit,
};

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "expected {b}, got {a}");
}

fn simple_ingredient(name: &str, cost: f64, cals: f64, tags: &[DietaryTag]) -> Ingredient {
    Ingredient {
        name: name.to_string(),
        unit: Unit::Gram,
        cost_per_unit_usd: cost,
        nutrition: Nutrition {
            calories: cals,
            protein_g: 1.0,
            carbs_g: 2.0,
            fat_g: 0.5,
        },
        tags: tags.iter().copied().collect(),
    }
}

fn recipe_two_ingredients() -> Recipe {
    Recipe {
        name: "Test Dish".to_string(),
        course: Course::Main,
        base_servings: 2.0,
        ingredients: vec![
            RecipeIngredient {
                ingredient: "flour".to_string(),
                quantity: 100.0,
            },
            RecipeIngredient {
                ingredient: "sugar".to_string(),
                quantity: 50.0,
            },
        ],
        steps: vec![
            PrepStep {
                description: "mix".to_string(),
                minutes: 10.0,
                make_ahead: true,
                scales_with_servings: true,
            },
            PrepStep {
                description: "bake".to_string(),
                minutes: 30.0,
                make_ahead: false,
                scales_with_servings: false,
            },
        ],
    }
}

fn two_ingredient_catalog() -> Vec<Ingredient> {
    vec![
        simple_ingredient("flour", 0.01, 3.0, &[DietaryTag::Vegan]),
        simple_ingredient("sugar", 0.02, 4.0, &[DietaryTag::Vegan]),
    ]
}

// ---------------------------------------------------------------------------
// Unit
// ---------------------------------------------------------------------------

#[test]
fn unit_abbrev() {
    assert_eq!(Unit::Gram.abbrev(), "g");
    assert_eq!(Unit::Milliliter.abbrev(), "ml");
    assert_eq!(Unit::Piece.abbrev(), "pc");
}

#[test]
fn dietary_tag_label() {
    assert_eq!(DietaryTag::Vegetarian.label(), "vegetarian");
    assert_eq!(DietaryTag::GlutenFree.label(), "gluten_free");
}

#[test]
fn course_label() {
    assert_eq!(Course::Main.label(), "main");
    assert_eq!(Course::Dessert.label(), "dessert");
}

// ---------------------------------------------------------------------------
// Nutrition arithmetic
// ---------------------------------------------------------------------------

#[test]
fn nutrition_scaled_and_plus() {
    let base = Nutrition {
        calories: 10.0,
        protein_g: 1.0,
        carbs_g: 2.0,
        fat_g: 0.5,
    };
    let doubled = base.scaled(2.0);
    approx(doubled.calories, 20.0);
    approx(doubled.fat_g, 1.0);
    let summed = base.plus(doubled);
    approx(summed.calories, 30.0);
    approx(summed.protein_g, 3.0);
}

// ---------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------

#[test]
fn recipe_cost_computes_total_and_per_serving() {
    let catalog_vec = two_ingredient_catalog();
    let catalog = Catalog::new(&catalog_vec);
    let recipe = recipe_two_ingredients();
    let cost = recipe_cost(&recipe, &catalog).unwrap();
    // 100*0.01 + 50*0.02 = 1.0 + 1.0 = 2.0 total, / 2 servings = 1.0
    approx(cost.total_usd, 2.0);
    approx(cost.per_serving_usd, 1.0);
}

#[test]
fn recipe_cost_unknown_ingredient_errors() {
    let catalog_vec = vec![simple_ingredient("flour", 0.01, 3.0, &[])];
    let catalog = Catalog::new(&catalog_vec);
    let recipe = recipe_two_ingredients();
    let err = recipe_cost(&recipe, &catalog).unwrap_err();
    assert_eq!(
        err,
        GastronomeError::UnknownIngredient {
            recipe: "Test Dish".to_string(),
            ingredient: "sugar".to_string(),
        }
    );
}

#[test]
fn recipe_cost_invalid_yield_errors() {
    let catalog_vec = two_ingredient_catalog();
    let catalog = Catalog::new(&catalog_vec);
    let mut recipe = recipe_two_ingredients();
    recipe.base_servings = 0.0;
    let err = recipe_cost(&recipe, &catalog).unwrap_err();
    assert!(matches!(err, GastronomeError::InvalidYield { .. }));
}

#[test]
fn aggregate_cost_sums_and_divides() {
    let per_recipe = vec![
        RecipeCost {
            recipe: "a".to_string(),
            total_usd: 12.0,
            per_serving_usd: 1.0,
        },
        RecipeCost {
            recipe: "b".to_string(),
            total_usd: 24.0,
            per_serving_usd: 2.0,
        },
    ];
    let agg = aggregate_cost(per_recipe, 12.0);
    approx(agg.event_total_usd, 36.0);
    approx(agg.per_guest_usd, 3.0);
}

// ---------------------------------------------------------------------------
// Nutrition breakdown
// ---------------------------------------------------------------------------

#[test]
fn recipe_nutrition_sums_ingredients() {
    let catalog_vec = two_ingredient_catalog();
    let catalog = Catalog::new(&catalog_vec);
    let recipe = recipe_two_ingredients();
    let n = recipe_nutrition(&recipe, &catalog).unwrap();
    // flour: 100*3=300 cal; sugar: 50*4=200 cal; total 500; per serving 250
    approx(n.total.calories, 500.0);
    approx(n.per_serving.calories, 250.0);
    // protein: (100+50)*1 = 150 total, 75 per serving
    approx(n.total.protein_g, 150.0);
    approx(n.per_serving.protein_g, 75.0);
}

#[test]
fn per_guest_nutrition_adds_recipes() {
    let catalog_vec = two_ingredient_catalog();
    let catalog = Catalog::new(&catalog_vec);
    let recipe = recipe_two_ingredients();
    let n = recipe_nutrition(&recipe, &catalog).unwrap();
    let total = per_guest_nutrition(&[n.clone(), n]);
    approx(total.calories, 500.0);
}

// ---------------------------------------------------------------------------
// Scaling
// ---------------------------------------------------------------------------

#[test]
fn scale_recipe_scales_ingredients_and_marked_steps() {
    let recipe = recipe_two_ingredients();
    let scaled = scale_recipe(&recipe, 6.0); // factor 3
    approx(scaled.base_servings, 6.0);
    approx(scaled.ingredients[0].quantity, 300.0);
    approx(scaled.ingredients[1].quantity, 150.0);
    // "mix" scales_with_servings=true -> 30; "bake" false -> stays 30
    approx(scaled.steps[0].minutes, 30.0);
    approx(scaled.steps[1].minutes, 30.0);
}

#[test]
fn scale_recipe_preserves_cost_per_serving() {
    let catalog_vec = two_ingredient_catalog();
    let catalog = Catalog::new(&catalog_vec);
    let recipe = recipe_two_ingredients();
    let base = recipe_cost(&recipe, &catalog).unwrap();
    let scaled = scale_recipe(&recipe, 20.0);
    let scaled_cost = recipe_cost(&scaled, &catalog).unwrap();
    approx(base.per_serving_usd, scaled_cost.per_serving_usd);
}

#[test]
fn scale_recipe_zero_target_is_noop_factor() {
    let recipe = recipe_two_ingredients();
    let scaled = scale_recipe(&recipe, 0.0);
    approx(scaled.ingredients[0].quantity, 100.0);
    approx(scaled.base_servings, 2.0);
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

#[test]
fn schedule_all_tasks_finish_by_event_start() {
    let recipe = recipe_two_ingredients();
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let sched = build_schedule(&[recipe], event, 1);
    assert_eq!(sched.tasks.len(), 2);
    for task in &sched.tasks {
        assert!(task.end <= event, "task ends after service: {task:?}");
        assert!(task.start < task.end);
    }
    assert_eq!(sched.kitchen_start, sched.tasks[0].start);
}

#[test]
fn schedule_at_service_step_ends_at_event_start() {
    let recipe = recipe_two_ingredients();
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let sched = build_schedule(&[recipe], event, 1);
    // The at-service "bake" step should finish exactly at service.
    let bake = sched.tasks.iter().find(|t| t.step == "bake").unwrap();
    assert_eq!(bake.end, event);
    assert!(!bake.make_ahead);
}

#[test]
fn schedule_parallel_cooks_reduce_makespan() {
    let mut r1 = recipe_two_ingredients();
    r1.name = "R1".to_string();
    let mut r2 = recipe_two_ingredients();
    r2.name = "R2".to_string();
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let one = build_schedule(&[r1.clone(), r2.clone()], event, 1);
    let two = build_schedule(&[r1, r2], event, 2);
    assert!(
        two.makespan_minutes < one.makespan_minutes,
        "two cooks ({}) should beat one ({})",
        two.makespan_minutes,
        one.makespan_minutes
    );
    assert_eq!(two.cook_count, 2);
}

#[test]
fn schedule_empty_recipes_yield_no_tasks() {
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let sched = build_schedule(&[], event, 3);
    assert!(sched.tasks.is_empty());
    assert_eq!(sched.kitchen_start, event);
    approx(sched.total_active_minutes, 0.0);
}

#[test]
fn schedule_recipe_without_steps_is_skipped() {
    let mut recipe = recipe_two_ingredients();
    recipe.steps.clear();
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let sched = build_schedule(&[recipe], event, 1);
    assert!(sched.tasks.is_empty());
}

#[test]
fn schedule_clamps_negative_step_minutes() {
    let mut recipe = recipe_two_ingredients();
    recipe.steps[0].minutes = -30.0;
    let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
    let sched = build_schedule(&[recipe], event, 1);
    for task in &sched.tasks {
        assert!(
            task.start <= task.end,
            "negative minutes must not invert task"
        );
        assert!(task.end <= event);
    }
}

#[test]
fn schedule_survives_absurd_step_minutes_without_panicking() {
    // An untrusted brief could set a huge or non-finite duration; the
    // scheduler must clamp it rather than overflow the timeline and panic.
    for bad in [1e30_f64, f64::INFINITY, f64::NAN] {
        let mut recipe = recipe_two_ingredients();
        recipe.steps[0].minutes = bad;
        let event = Utc.with_ymd_and_hms(2026, 1, 1, 18, 0, 0).unwrap();
        let sched = build_schedule(&[recipe], event, 1);
        for task in &sched.tasks {
            assert!(task.start <= task.end);
            assert!(task.end <= event);
        }
    }
}

// ---------------------------------------------------------------------------
// Planner end-to-end
// ---------------------------------------------------------------------------

#[test]
fn plan_event_on_sample_brief_succeeds() {
    let brief = sample_brief();
    let plan = plan_event(&brief).unwrap();
    assert_eq!(plan.guest_count, 24);
    assert_eq!(plan.scaled_recipes.len(), 3);
    assert!(plan.cost.per_guest_usd > 0.0);
    assert!(plan.cost.event_total_usd > plan.cost.per_guest_usd);
    assert!(plan.nutrition_per_guest.calories > 0.0);
    assert!(!plan.schedule.tasks.is_empty());
    assert!(plan.budget.is_affordable());
    // Every task finishes by service.
    for task in &plan.schedule.tasks {
        assert!(task.end <= plan.event_start);
    }
}

#[test]
fn plan_event_scales_cost_with_guest_count() {
    let mut brief = sample_brief();
    brief.budget_per_guest_usd = None;
    let plan24 = plan_event(&brief).unwrap();
    brief.guest_count = 48;
    let plan48 = plan_event(&brief).unwrap();
    // Per-guest cost is stable; event total roughly doubles.
    approx(plan24.cost.per_guest_usd, plan48.cost.per_guest_usd);
    approx(
        plan48.cost.event_total_usd,
        plan24.cost.event_total_usd * 2.0,
    );
}

#[test]
fn plan_event_empty_menu_errors() {
    let mut brief = sample_brief();
    brief.menu.recipes.clear();
    assert_eq!(plan_event(&brief).unwrap_err(), GastronomeError::EmptyMenu);
}

#[test]
fn plan_event_dietary_violation_errors() {
    let mut brief = sample_brief();
    // Require vegan; feta cheese in the main is not vegan.
    brief.dietary_constraints = [DietaryTag::Vegan].into_iter().collect();
    let err = plan_event(&brief).unwrap_err();
    match err {
        GastronomeError::DietaryViolation { violations } => {
            assert!(violations.iter().any(|v| v.contains("feta cheese")));
        }
        other => panic!("expected dietary violation, got {other:?}"),
    }
}

#[test]
fn plan_event_over_budget_is_flagged() {
    let mut brief = sample_brief();
    brief.budget_per_guest_usd = Some(0.01);
    let plan = plan_event(&brief).unwrap();
    assert!(!plan.budget.is_affordable());
    assert!(matches!(plan.budget, BudgetStatus::OverBudget { .. }));
}

#[test]
fn plan_event_unknown_ingredient_errors() {
    let mut brief = sample_brief();
    brief.menu.recipes[0].ingredients.push(RecipeIngredient {
        ingredient: "unobtainium".to_string(),
        quantity: 1.0,
    });
    let err = plan_event(&brief).unwrap_err();
    assert!(matches!(err, GastronomeError::UnknownIngredient { .. }));
}

#[test]
fn plan_event_serializes_to_json() {
    let brief = sample_brief();
    let plan = plan_event(&brief).unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    assert!(json.contains("scaled_recipes"));
    let roundtrip: MenuPlan = serde_json::from_str(&json).unwrap();
    // JSON f64 parsing is only accurate to ~1 ULP without the serde_json
    // `float_roundtrip` feature, so assert structural equality and compare
    // numeric fields approximately rather than with exact `PartialEq`.
    assert_eq!(roundtrip.event_name, plan.event_name);
    assert_eq!(roundtrip.guest_count, plan.guest_count);
    assert_eq!(roundtrip.scaled_recipes.len(), plan.scaled_recipes.len());
    assert_eq!(roundtrip.schedule.tasks.len(), plan.schedule.tasks.len());
    approx(roundtrip.cost.per_guest_usd, plan.cost.per_guest_usd);
    approx(
        roundtrip.nutrition_per_guest.calories,
        plan.nutrition_per_guest.calories,
    );
}

#[test]
fn event_brief_roundtrips_through_json() {
    let brief = sample_brief();
    let json = serde_json::to_string(&brief).unwrap();
    let roundtrip: EventBrief = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip.event_name, brief.event_name);
    assert_eq!(roundtrip.guest_count, brief.guest_count);
    assert_eq!(roundtrip.catalog.len(), brief.catalog.len());
    assert_eq!(roundtrip.menu.recipes.len(), brief.menu.recipes.len());
    assert_eq!(roundtrip.dietary_constraints, brief.dietary_constraints);
    // A replan from the round-tripped brief yields the same headline figures.
    let a = plan_event(&brief).unwrap();
    let b = plan_event(&roundtrip).unwrap();
    approx(a.cost.per_guest_usd, b.cost.per_guest_usd);
}

#[test]
fn gastronome_error_display_is_human_readable() {
    let err = GastronomeError::EmptyMenu;
    assert_eq!(err.to_string(), "menu contains no recipes");
    let err = GastronomeError::UnknownIngredient {
        recipe: "R".to_string(),
        ingredient: "X".to_string(),
    };
    assert!(err.to_string().contains("unknown ingredient 'X'"));
}
