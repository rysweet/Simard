//! A ready-made sample [`EventBrief`] used by docs, tests, and the CLI's
//! `sample-brief` subcommand.

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};

use super::types::{
    Course, DietaryTag, EventBrief, Ingredient, Menu, Nutrition, PrepStep, Recipe,
    RecipeIngredient, Unit,
};

fn tags(items: &[DietaryTag]) -> BTreeSet<DietaryTag> {
    items.iter().copied().collect()
}

fn ingredient(
    name: &str,
    unit: Unit,
    cost: f64,
    nutrition: Nutrition,
    t: &[DietaryTag],
) -> Ingredient {
    Ingredient {
        name: name.to_string(),
        unit,
        cost_per_unit_usd: cost,
        nutrition,
        tags: tags(t),
    }
}

/// Build a self-contained, valid sample brief: a vegetarian garden luncheon
/// for 24 guests with a $12/guest budget and two cooks.
#[must_use]
pub fn sample_brief() -> EventBrief {
    use DietaryTag::{DairyFree, GlutenFree, NutFree, Vegan, Vegetarian};

    // Per-base-unit nutrition (per gram, per ml, or per piece).
    let veg_all = &[Vegetarian, Vegan, GlutenFree, DairyFree, NutFree][..];
    let veg_dairy = &[Vegetarian, GlutenFree, NutFree][..];

    let catalog = vec![
        ingredient(
            "tomato",
            Unit::Gram,
            0.004,
            Nutrition {
                calories: 0.18,
                protein_g: 0.009,
                carbs_g: 0.039,
                fat_g: 0.002,
            },
            veg_all,
        ),
        ingredient(
            "cucumber",
            Unit::Gram,
            0.003,
            Nutrition {
                calories: 0.15,
                protein_g: 0.007,
                carbs_g: 0.036,
                fat_g: 0.001,
            },
            veg_all,
        ),
        ingredient(
            "olive oil",
            Unit::Milliliter,
            0.012,
            Nutrition {
                calories: 8.84,
                protein_g: 0.0,
                carbs_g: 0.0,
                fat_g: 1.0,
            },
            veg_all,
        ),
        ingredient(
            "chickpeas",
            Unit::Gram,
            0.005,
            Nutrition {
                calories: 1.64,
                protein_g: 0.089,
                carbs_g: 0.27,
                fat_g: 0.026,
            },
            veg_all,
        ),
        ingredient(
            "quinoa",
            Unit::Gram,
            0.008,
            Nutrition {
                calories: 3.68,
                protein_g: 0.14,
                carbs_g: 0.64,
                fat_g: 0.061,
            },
            veg_all,
        ),
        ingredient(
            "feta cheese",
            Unit::Gram,
            0.016,
            Nutrition {
                calories: 2.64,
                protein_g: 0.14,
                carbs_g: 0.041,
                fat_g: 0.21,
            },
            veg_dairy,
        ),
        ingredient(
            "lemon",
            Unit::Piece,
            0.5,
            Nutrition {
                calories: 17.0,
                protein_g: 0.6,
                carbs_g: 5.4,
                fat_g: 0.2,
            },
            veg_all,
        ),
        ingredient(
            "strawberry",
            Unit::Gram,
            0.009,
            Nutrition {
                calories: 0.32,
                protein_g: 0.007,
                carbs_g: 0.077,
                fat_g: 0.003,
            },
            veg_all,
        ),
        ingredient(
            "honey",
            Unit::Gram,
            0.011,
            Nutrition {
                calories: 3.04,
                protein_g: 0.003,
                carbs_g: 0.82,
                fat_g: 0.0,
            },
            veg_dairy,
        ),
    ];

    let garden_salad = Recipe {
        name: "Garden Salad".to_string(),
        course: Course::Appetizer,
        base_servings: 4.0,
        ingredients: vec![
            RecipeIngredient {
                ingredient: "tomato".to_string(),
                quantity: 300.0,
            },
            RecipeIngredient {
                ingredient: "cucumber".to_string(),
                quantity: 250.0,
            },
            RecipeIngredient {
                ingredient: "olive oil".to_string(),
                quantity: 30.0,
            },
            RecipeIngredient {
                ingredient: "lemon".to_string(),
                quantity: 1.0,
            },
        ],
        steps: vec![
            PrepStep {
                description: "Wash and dice vegetables".to_string(),
                minutes: 12.0,
                make_ahead: true,
                scales_with_servings: true,
            },
            PrepStep {
                description: "Whisk lemon-oil dressing".to_string(),
                minutes: 5.0,
                make_ahead: true,
                scales_with_servings: false,
            },
            PrepStep {
                description: "Toss and plate".to_string(),
                minutes: 6.0,
                make_ahead: false,
                scales_with_servings: true,
            },
        ],
    };

    let quinoa_bowl = Recipe {
        name: "Quinoa Chickpea Bowl".to_string(),
        course: Course::Main,
        base_servings: 4.0,
        ingredients: vec![
            RecipeIngredient {
                ingredient: "quinoa".to_string(),
                quantity: 320.0,
            },
            RecipeIngredient {
                ingredient: "chickpeas".to_string(),
                quantity: 400.0,
            },
            RecipeIngredient {
                ingredient: "feta cheese".to_string(),
                quantity: 120.0,
            },
            RecipeIngredient {
                ingredient: "olive oil".to_string(),
                quantity: 40.0,
            },
        ],
        steps: vec![
            PrepStep {
                description: "Rinse and simmer quinoa".to_string(),
                minutes: 20.0,
                make_ahead: true,
                scales_with_servings: false,
            },
            PrepStep {
                description: "Roast chickpeas".to_string(),
                minutes: 25.0,
                make_ahead: true,
                scales_with_servings: true,
            },
            PrepStep {
                description: "Assemble bowls with feta".to_string(),
                minutes: 10.0,
                make_ahead: false,
                scales_with_servings: true,
            },
        ],
    };

    let berry_cup = Recipe {
        name: "Honey Berry Cup".to_string(),
        course: Course::Dessert,
        base_servings: 4.0,
        ingredients: vec![
            RecipeIngredient {
                ingredient: "strawberry".to_string(),
                quantity: 400.0,
            },
            RecipeIngredient {
                ingredient: "honey".to_string(),
                quantity: 60.0,
            },
        ],
        steps: vec![
            PrepStep {
                description: "Hull and slice strawberries".to_string(),
                minutes: 10.0,
                make_ahead: true,
                scales_with_servings: true,
            },
            PrepStep {
                description: "Drizzle honey and chill".to_string(),
                minutes: 5.0,
                make_ahead: false,
                scales_with_servings: false,
            },
        ],
    };

    let menu = Menu {
        name: "Garden Luncheon".to_string(),
        recipes: vec![garden_salad, quinoa_bowl, berry_cup],
    };

    EventBrief {
        event_name: "Summer Garden Luncheon".to_string(),
        guest_count: 24,
        event_start: Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap(),
        budget_per_guest_usd: Some(12.0),
        dietary_constraints: tags(&[Vegetarian]),
        cook_count: 2,
        catalog,
        menu,
    }
}
