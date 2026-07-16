//! The recipe book: a searchable collection of [`Recipe`]s the planner draws
//! from, plus a small built-in sample book so the identity and CLI work out of
//! the box with no external data.
//!
//! Selection is deterministic: for a course under a set of required dietary
//! tags, the book returns the *cheapest* satisfying recipe, breaking ties by
//! recipe id. That keeps `plan_event` reproducible for the same inputs.

use serde::{Deserialize, Serialize};

use super::error::{GastronomeError, GastronomeResult};
use super::nutrition::Nutrition;
use super::types::{DietaryTag, Ingredient, PrepStep, Recipe, RecipeIngredient};

/// A collection of recipes. Wraps a `Vec<Recipe>` so it can be loaded from JSON
/// (the CLI's `--recipes` file) or constructed in code.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeBook {
    /// The recipes, in no particular order.
    pub recipes: Vec<Recipe>,
}

impl RecipeBook {
    /// Wrap a recipe list.
    #[must_use]
    pub fn new(recipes: Vec<Recipe>) -> Self {
        Self { recipes }
    }

    /// Parse a book from JSON. Accepts either a bare array of recipes or an
    /// object of the shape `{ "recipes": [ ... ] }`.
    ///
    /// # Errors
    /// Returns [`GastronomeError::NoRecipeForCourse`] with a parse reason when
    /// the JSON does not deserialize into a recipe book.
    pub fn from_json(json: &str) -> GastronomeResult<Self> {
        // Try the wrapper object first, then a bare array.
        if let Ok(book) = serde_json::from_str::<RecipeBook>(json) {
            return Ok(book);
        }
        match serde_json::from_str::<Vec<Recipe>>(json) {
            Ok(recipes) => Ok(Self::new(recipes)),
            Err(e) => Err(GastronomeError::NoRecipeForCourse {
                course: "<parse>".into(),
                reason: format!("recipe book JSON is invalid: {e}"),
            }),
        }
    }

    /// Look a recipe up by its stable id.
    #[must_use]
    pub fn find_by_id(&self, id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|r| r.id == id)
    }

    /// Select the cheapest recipe for `course` that carries every tag in
    /// `required`, breaking ties by recipe id for determinism.
    #[must_use]
    pub fn select(&self, course: &str, required: &[DietaryTag]) -> Option<&Recipe> {
        self.recipes
            .iter()
            .filter(|r| r.course == course && r.satisfies(required))
            .min_by(|a, b| {
                a.cost_per_serving()
                    .partial_cmp(&b.cost_per_serving())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.id.cmp(&b.id))
            })
    }

    /// A curated built-in book covering the common courses, so the identity has
    /// something to plan with before any external recipes are supplied.
    #[must_use]
    pub fn builtin() -> Self {
        Self::new(vec![
            garden_salad(),
            tomato_soup(),
            chickpea_curry(),
            roast_chicken(),
            crusty_bread(),
            fruit_crumble(),
        ])
    }
}

fn ing(name: &str, unit: &str, cost: f64, n: Nutrition) -> Ingredient {
    Ingredient {
        name: name.into(),
        unit: unit.into(),
        cost_per_unit: cost,
        nutrition_per_unit: n,
    }
}

fn step(desc: &str, minutes: u32, active: bool) -> PrepStep {
    PrepStep {
        description: desc.into(),
        minutes,
        active,
    }
}

fn garden_salad() -> Recipe {
    Recipe {
        id: "garden-salad".into(),
        name: "Garden Salad".into(),
        course: "starter".into(),
        base_servings: 4,
        dietary_tags: vec![
            DietaryTag::Vegetarian,
            DietaryTag::Vegan,
            DietaryTag::GlutenFree,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
        ],
        ingredients: vec![
            RecipeIngredient {
                ingredient: ing(
                    "mixed leaves",
                    "kg",
                    6.0,
                    Nutrition::new(150.0, 15.0, 20.0, 3.0),
                ),
                quantity: 0.05,
            },
            RecipeIngredient {
                ingredient: ing(
                    "cucumber",
                    "each",
                    0.6,
                    Nutrition::new(45.0, 2.0, 11.0, 0.3),
                ),
                quantity: 0.25,
            },
            RecipeIngredient {
                ingredient: ing(
                    "olive oil",
                    "litre",
                    8.0,
                    Nutrition::new(8840.0, 0.0, 0.0, 1000.0),
                ),
                quantity: 0.01,
            },
        ],
        steps: vec![
            step("wash and chop", 10, true),
            step("dress and toss", 5, true),
        ],
    }
}

fn tomato_soup() -> Recipe {
    Recipe {
        id: "tomato-soup".into(),
        name: "Roast Tomato Soup".into(),
        course: "starter".into(),
        base_servings: 4,
        dietary_tags: vec![
            DietaryTag::Vegetarian,
            DietaryTag::Vegan,
            DietaryTag::GlutenFree,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
        ],
        ingredients: vec![
            RecipeIngredient {
                ingredient: ing("tomatoes", "kg", 3.0, Nutrition::new(180.0, 9.0, 39.0, 2.0)),
                quantity: 0.25,
            },
            RecipeIngredient {
                ingredient: ing(
                    "vegetable stock",
                    "litre",
                    1.2,
                    Nutrition::new(40.0, 2.0, 6.0, 1.0),
                ),
                quantity: 0.3,
            },
        ],
        steps: vec![
            step("roast tomatoes", 30, false),
            step("blend and season", 10, true),
            step("reheat to serve", 8, false),
        ],
    }
}

fn chickpea_curry() -> Recipe {
    Recipe {
        id: "chickpea-curry".into(),
        name: "Chickpea & Spinach Curry".into(),
        course: "main".into(),
        base_servings: 4,
        dietary_tags: vec![
            DietaryTag::Vegetarian,
            DietaryTag::Vegan,
            DietaryTag::GlutenFree,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
            DietaryTag::Halal,
        ],
        ingredients: vec![
            RecipeIngredient {
                ingredient: ing(
                    "chickpeas",
                    "kg",
                    2.5,
                    Nutrition::new(1640.0, 90.0, 270.0, 26.0),
                ),
                quantity: 0.15,
            },
            RecipeIngredient {
                ingredient: ing("spinach", "kg", 4.0, Nutrition::new(230.0, 29.0, 36.0, 4.0)),
                quantity: 0.08,
            },
            RecipeIngredient {
                ingredient: ing(
                    "basmati rice",
                    "kg",
                    2.2,
                    Nutrition::new(3600.0, 74.0, 800.0, 8.0),
                ),
                quantity: 0.08,
            },
        ],
        steps: vec![
            step("soften aromatics", 12, true),
            step("simmer curry", 25, false),
            step("cook rice", 15, false),
        ],
    }
}

fn roast_chicken() -> Recipe {
    Recipe {
        id: "roast-chicken".into(),
        name: "Herb Roast Chicken".into(),
        course: "main".into(),
        base_servings: 4,
        dietary_tags: vec![
            DietaryTag::GlutenFree,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
        ],
        ingredients: vec![
            RecipeIngredient {
                ingredient: ing(
                    "chicken",
                    "kg",
                    5.5,
                    Nutrition::new(1900.0, 180.0, 0.0, 130.0),
                ),
                quantity: 0.3,
            },
            RecipeIngredient {
                ingredient: ing(
                    "potatoes",
                    "kg",
                    1.2,
                    Nutrition::new(770.0, 20.0, 170.0, 1.0),
                ),
                quantity: 0.2,
            },
        ],
        steps: vec![
            step("season and prep tray", 15, true),
            step("roast", 75, false),
            step("rest and carve", 15, true),
        ],
    }
}

fn crusty_bread() -> Recipe {
    Recipe {
        id: "crusty-bread".into(),
        name: "Crusty Loaf".into(),
        course: "side".into(),
        base_servings: 8,
        dietary_tags: vec![
            DietaryTag::Vegetarian,
            DietaryTag::Vegan,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
        ],
        ingredients: vec![RecipeIngredient {
            ingredient: ing(
                "bread flour",
                "kg",
                2.0,
                Nutrition::new(3640.0, 100.0, 760.0, 10.0),
            ),
            quantity: 0.06,
        }],
        steps: vec![
            step("mix and knead", 15, true),
            step("prove", 90, false),
            step("bake", 35, false),
        ],
    }
}

fn fruit_crumble() -> Recipe {
    Recipe {
        id: "fruit-crumble".into(),
        name: "Apple & Berry Crumble".into(),
        course: "dessert".into(),
        base_servings: 6,
        dietary_tags: vec![DietaryTag::Vegetarian, DietaryTag::NutFree],
        ingredients: vec![
            RecipeIngredient {
                ingredient: ing("apples", "kg", 2.0, Nutrition::new(520.0, 3.0, 140.0, 2.0)),
                quantity: 0.12,
            },
            RecipeIngredient {
                ingredient: ing(
                    "mixed berries",
                    "kg",
                    7.0,
                    Nutrition::new(430.0, 5.0, 100.0, 3.0),
                ),
                quantity: 0.05,
            },
            RecipeIngredient {
                ingredient: ing(
                    "crumble mix",
                    "kg",
                    3.5,
                    Nutrition::new(4500.0, 60.0, 620.0, 200.0),
                ),
                quantity: 0.05,
            },
        ],
        steps: vec![
            step("prepare fruit", 15, true),
            step("top and bake", 40, false),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_book_has_a_main_and_a_starter() {
        let book = RecipeBook::builtin();
        assert!(book.select("main", &[]).is_some());
        assert!(book.select("starter", &[]).is_some());
        assert!(book.select("dessert", &[]).is_some());
    }

    #[test]
    fn select_honours_dietary_constraints() {
        let book = RecipeBook::builtin();
        // roast-chicken is not vegan; the vegan main must be the curry.
        let vegan_main = book.select("main", &[DietaryTag::Vegan]).unwrap();
        assert_eq!(vegan_main.id, "chickpea-curry");
    }

    #[test]
    fn select_returns_none_when_no_dish_qualifies() {
        let book = RecipeBook::builtin();
        // No vegan dessert in the built-in book.
        assert!(book.select("dessert", &[DietaryTag::Vegan]).is_none());
    }

    #[test]
    fn select_picks_cheapest_then_id() {
        let book = RecipeBook::builtin();
        // Two vegan starters (salad, soup); cheapest per serving wins.
        let chosen = book.select("starter", &[DietaryTag::Vegan]).unwrap();
        let salad_cost = garden_salad().cost_per_serving();
        let soup_cost = tomato_soup().cost_per_serving();
        let expected = if salad_cost <= soup_cost {
            "garden-salad"
        } else {
            "tomato-soup"
        };
        assert_eq!(chosen.id, expected);
    }

    #[test]
    fn find_by_id_round_trips() {
        let book = RecipeBook::builtin();
        assert_eq!(
            book.find_by_id("roast-chicken").unwrap().name,
            "Herb Roast Chicken"
        );
        assert!(book.find_by_id("nope").is_none());
    }

    #[test]
    fn from_json_accepts_bare_array_and_wrapper() {
        let bare = r#"[{"id":"x","name":"X","course":"main","base_servings":2,
            "ingredients":[{"ingredient":{"name":"a","unit":"each","cost_per_unit":1.0},
            "quantity":1.0}]}]"#;
        let book = RecipeBook::from_json(bare).unwrap();
        assert_eq!(book.recipes.len(), 1);

        let wrapped = format!("{{\"recipes\": {bare}}}");
        let book2 = RecipeBook::from_json(&wrapped).unwrap();
        assert_eq!(book2.recipes.len(), 1);
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(RecipeBook::from_json("not json").is_err());
    }
}
