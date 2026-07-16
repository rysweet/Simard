//! The kitchen book: the pantry of [`Ingredient`]s and the [`Recipe`] repertoire
//! a Gastronome plans against, plus TOML (de)serialization and a self-contained
//! demo book so `simard-kitchen demo` runs end-to-end with no external files.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::types::{
    Course, EventBrief, GastronomeError, GastronomeResult, Ingredient, Nutrition, Recipe,
    RecipeLine, Unit,
};

/// A validated collection of ingredients and recipes, indexed by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KitchenBook {
    /// The pantry, keyed by ingredient id.
    #[serde(rename = "ingredient", default, with = "id_vec_ingredient")]
    pub ingredients: BTreeMap<String, Ingredient>,
    /// The recipe repertoire, keyed by recipe id.
    #[serde(rename = "recipe", default, with = "id_vec_recipe")]
    pub recipes: BTreeMap<String, Recipe>,
    /// An optional embedded brief so a single TOML file can be fully
    /// self-describing (`--file kitchen.toml`).
    #[serde(default)]
    pub brief: Option<EventBrief>,
}

impl KitchenBook {
    /// Build a book from explicit ingredient and recipe lists, validating that
    /// every recipe line references a known ingredient with a compatible unit
    /// family and that every recipe has a positive yield.
    ///
    /// # Errors
    /// Returns the first structural problem found (unknown ingredient, unit
    /// mismatch, or non-positive servings).
    pub fn new(
        ingredients: Vec<Ingredient>,
        recipes: Vec<Recipe>,
        brief: Option<EventBrief>,
    ) -> GastronomeResult<Self> {
        let ingredients: BTreeMap<String, Ingredient> =
            ingredients.into_iter().map(|i| (i.id.clone(), i)).collect();
        let recipes: BTreeMap<String, Recipe> =
            recipes.into_iter().map(|r| (r.id.clone(), r)).collect();
        let book = Self {
            ingredients,
            recipes,
            brief,
        };
        book.validate()?;
        Ok(book)
    }

    /// Validate cross-references and invariants across the whole book.
    ///
    /// # Errors
    /// Returns the first violation encountered.
    pub fn validate(&self) -> GastronomeResult<()> {
        for recipe in self.recipes.values() {
            if recipe.servings <= 0.0 {
                return Err(GastronomeError::InvalidServings {
                    recipe: recipe.id.clone(),
                    servings: recipe.servings,
                });
            }
            for line in &recipe.ingredients {
                let ingredient = self.ingredients.get(&line.ingredient).ok_or_else(|| {
                    GastronomeError::UnknownIngredient {
                        recipe: recipe.id.clone(),
                        ingredient: line.ingredient.clone(),
                    }
                })?;
                if line.unit.family() != ingredient.unit.family() {
                    return Err(GastronomeError::UnitMismatch {
                        recipe: recipe.id.clone(),
                        ingredient: ingredient.id.clone(),
                        line_family: line.unit.family().label(),
                        base_family: ingredient.unit.family().label(),
                    });
                }
            }
            for dep in &recipe.depends_on {
                if !self.recipes.contains_key(dep) {
                    return Err(GastronomeError::UnknownRecipe {
                        recipe: dep.clone(),
                    });
                }
            }
        }
        if let Some(brief) = &self.brief {
            self.validate_brief(brief)?;
        }
        Ok(())
    }

    /// Validate that every course in a brief references a known recipe.
    ///
    /// # Errors
    /// Returns [`GastronomeError::UnknownRecipe`] for the first missing recipe.
    pub fn validate_brief(&self, brief: &EventBrief) -> GastronomeResult<()> {
        for course in &brief.courses {
            if !self.recipes.contains_key(&course.recipe) {
                return Err(GastronomeError::UnknownRecipe {
                    recipe: course.recipe.clone(),
                });
            }
        }
        Ok(())
    }

    /// Look up a recipe by id.
    ///
    /// # Errors
    /// Returns [`GastronomeError::UnknownRecipe`] if absent.
    pub fn recipe(&self, id: &str) -> GastronomeResult<&Recipe> {
        self.recipes
            .get(id)
            .ok_or_else(|| GastronomeError::UnknownRecipe {
                recipe: id.to_string(),
            })
    }

    /// Look up an ingredient by id.
    ///
    /// # Errors
    /// Returns [`GastronomeError::UnknownIngredient`] (with `recipe = "-"`) if
    /// absent; callers with a recipe context should prefer explicit lookups.
    pub fn ingredient(&self, id: &str) -> GastronomeResult<&Ingredient> {
        self.ingredients
            .get(id)
            .ok_or_else(|| GastronomeError::UnknownIngredient {
                recipe: "-".to_string(),
                ingredient: id.to_string(),
            })
    }

    /// Parse a book from a TOML string, then validate it.
    ///
    /// # Errors
    /// Returns [`GastronomeError::Parse`] on malformed TOML or a validation
    /// error on structural problems.
    pub fn from_toml(text: &str) -> GastronomeResult<Self> {
        let book: Self = toml::from_str(text).map_err(|e| GastronomeError::Parse(e.to_string()))?;
        book.validate()?;
        Ok(book)
    }

    /// The built-in demo book: a small garden-wedding repertoire that exercises
    /// mass/volume/count units, an ingredient dependency (`poolish → focaccia`),
    /// costing, nutrition, and scheduling. Used by `simard-kitchen demo`.
    #[must_use]
    pub fn demo() -> Self {
        let ingredients = vec![
            ingredient(
                "flour",
                "Bread flour",
                Unit::Gram,
                0.0018,
                364.0,
                12.0,
                76.0,
                1.2,
                &["vegan", "contains-gluten"],
            ),
            ingredient(
                "water",
                "Water",
                Unit::Milliliter,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                &["vegan"],
            ),
            ingredient(
                "olive_oil",
                "Extra-virgin olive oil",
                Unit::Milliliter,
                0.012,
                884.0,
                0.0,
                0.0,
                100.0,
                &["vegan"],
            ),
            ingredient(
                "salt",
                "Sea salt",
                Unit::Gram,
                0.002,
                0.0,
                0.0,
                0.0,
                0.0,
                &["vegan"],
            ),
            ingredient(
                "yeast",
                "Instant yeast",
                Unit::Gram,
                0.02,
                325.0,
                40.0,
                41.0,
                7.0,
                &["vegan"],
            ),
            ingredient(
                "rosemary",
                "Fresh rosemary",
                Unit::Gram,
                0.03,
                131.0,
                3.3,
                20.7,
                5.9,
                &["vegan"],
            ),
            ingredient(
                "chicken",
                "Free-range chicken thigh",
                Unit::Gram,
                0.011,
                209.0,
                26.0,
                0.0,
                11.0,
                &["contains-none"],
            ),
            ingredient(
                "lemon",
                "Lemon",
                Unit::Each,
                0.55,
                17.0,
                0.6,
                5.4,
                0.2,
                &["vegan"],
            ),
            ingredient(
                "garlic",
                "Garlic clove",
                Unit::Each,
                0.09,
                4.0,
                0.2,
                1.0,
                0.0,
                &["vegan"],
            ),
            ingredient(
                "green_beans",
                "Green beans",
                Unit::Gram,
                0.006,
                31.0,
                1.8,
                7.0,
                0.2,
                &["vegan"],
            ),
            ingredient(
                "butter",
                "Butter",
                Unit::Gram,
                0.009,
                717.0,
                0.9,
                0.1,
                81.0,
                &["vegetarian", "contains-dairy"],
            ),
            ingredient(
                "dark_chocolate",
                "Dark chocolate 70%",
                Unit::Gram,
                0.022,
                598.0,
                7.8,
                46.0,
                43.0,
                &["vegetarian"],
            ),
            ingredient(
                "sugar",
                "Caster sugar",
                Unit::Gram,
                0.0016,
                387.0,
                0.0,
                100.0,
                0.0,
                &["vegan"],
            ),
            ingredient(
                "egg",
                "Egg",
                Unit::Each,
                0.35,
                78.0,
                6.3,
                0.6,
                5.3,
                &["vegetarian", "contains-egg"],
            ),
        ];

        let recipes = vec![
            Recipe {
                id: "poolish".into(),
                name: "Overnight poolish".into(),
                servings: 8.0,
                prep_minutes: 10,
                cook_minutes: 720,
                depends_on: vec![],
                ingredients: vec![
                    line("flour", 200.0, Unit::Gram),
                    line("water", 200.0, Unit::Milliliter),
                    line("yeast", 1.0, Unit::Gram),
                ],
            },
            Recipe {
                id: "focaccia".into(),
                name: "Rosemary focaccia".into(),
                servings: 8.0,
                prep_minutes: 30,
                cook_minutes: 150,
                depends_on: vec!["poolish".into()],
                ingredients: vec![
                    line("flour", 300.0, Unit::Gram),
                    line("water", 180.0, Unit::Milliliter),
                    line("olive_oil", 40.0, Unit::Milliliter),
                    line("salt", 10.0, Unit::Gram),
                    line("yeast", 4.0, Unit::Gram),
                    line("rosemary", 8.0, Unit::Gram),
                ],
            },
            Recipe {
                id: "roast_chicken".into(),
                name: "Lemon-garlic roast chicken".into(),
                servings: 4.0,
                prep_minutes: 25,
                cook_minutes: 45,
                depends_on: vec![],
                ingredients: vec![
                    line("chicken", 1200.0, Unit::Gram),
                    line("lemon", 2.0, Unit::Each),
                    line("garlic", 6.0, Unit::Each),
                    line("olive_oil", 30.0, Unit::Milliliter),
                    line("salt", 12.0, Unit::Gram),
                ],
            },
            Recipe {
                id: "green_beans".into(),
                name: "Buttered green beans".into(),
                servings: 4.0,
                prep_minutes: 15,
                cook_minutes: 12,
                depends_on: vec![],
                ingredients: vec![
                    line("green_beans", 600.0, Unit::Gram),
                    line("butter", 40.0, Unit::Gram),
                    line("salt", 4.0, Unit::Gram),
                ],
            },
            Recipe {
                id: "chocolate_tart".into(),
                name: "Dark chocolate tart".into(),
                servings: 8.0,
                prep_minutes: 40,
                cook_minutes: 120,
                depends_on: vec![],
                ingredients: vec![
                    line("dark_chocolate", 300.0, Unit::Gram),
                    line("butter", 150.0, Unit::Gram),
                    line("sugar", 120.0, Unit::Gram),
                    line("egg", 4.0, Unit::Each),
                    line("flour", 60.0, Unit::Gram),
                ],
            },
        ];

        let brief = EventBrief {
            name: "Garden Wedding Dinner".into(),
            guest_count: 40,
            service_time: "18:30".into(),
            budget_per_guest: Some(18.0),
            courses: vec![
                course("focaccia", 1.0),
                course("roast_chicken", 1.0),
                course("green_beans", 1.0),
                course("chocolate_tart", 1.0),
            ],
        };

        // demo() is hand-built and always valid; new() would only error on a
        // programming mistake, which the module tests guard against.
        Self::new(ingredients, recipes, Some(brief)).expect("demo book is valid")
    }
}

#[allow(clippy::too_many_arguments)]
fn ingredient(
    id: &str,
    name: &str,
    unit: Unit,
    price_per_base: f64,
    calories: f64,
    protein_g: f64,
    carbs_g: f64,
    fat_g: f64,
    tags: &[&str],
) -> Ingredient {
    Ingredient {
        id: id.into(),
        name: name.into(),
        unit,
        price_per_base,
        nutrition: Nutrition {
            calories,
            protein_g,
            carbs_g,
            fat_g,
        },
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
    }
}

fn line(ingredient: &str, quantity: f64, unit: Unit) -> RecipeLine {
    RecipeLine {
        ingredient: ingredient.into(),
        quantity,
        unit,
    }
}

fn course(recipe: &str, portions_per_guest: f64) -> Course {
    Course {
        recipe: recipe.into(),
        portions_per_guest,
    }
}

// A TOML file lists `[[ingredient]]` / `[[recipe]]` as arrays, but the in-memory
// model indexes them by id for O(log n) lookups. These adapters bridge the two
// representations so the public struct stays ergonomic.
mod id_vec_ingredient {
    use super::{BTreeMap, Ingredient};
    use serde::{Deserializer, Serialize, Serializer, de::Deserialize};

    pub(super) fn serialize<S: Serializer>(
        map: &BTreeMap<String, Ingredient>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.values().collect::<Vec<_>>().serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, Ingredient>, D::Error> {
        let items = Vec::<Ingredient>::deserialize(d)?;
        Ok(items.into_iter().map(|i| (i.id.clone(), i)).collect())
    }
}

mod id_vec_recipe {
    use super::{BTreeMap, Recipe};
    use serde::{Deserializer, Serialize, Serializer, de::Deserialize};

    pub(super) fn serialize<S: Serializer>(
        map: &BTreeMap<String, Recipe>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        map.values().collect::<Vec<_>>().serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<String, Recipe>, D::Error> {
        let items = Vec::<Recipe>::deserialize(d)?;
        Ok(items.into_iter().map(|r| (r.id.clone(), r)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_book_is_valid_and_indexed() {
        let book = KitchenBook::demo();
        book.validate().unwrap();
        assert!(book.recipes.contains_key("focaccia"));
        assert!(book.ingredients.contains_key("flour"));
        assert!(book.brief.is_some());
    }

    #[test]
    fn validate_rejects_unknown_ingredient() {
        let recipes = vec![Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 4.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec![],
            ingredients: vec![line("ghost", 1.0, Unit::Gram)],
        }];
        let err = KitchenBook::new(vec![], recipes, None).unwrap_err();
        assert!(matches!(err, GastronomeError::UnknownIngredient { .. }));
    }

    #[test]
    fn validate_rejects_unit_family_mismatch() {
        let ings = vec![ingredient(
            "flour",
            "Flour",
            Unit::Gram,
            0.001,
            0.0,
            0.0,
            0.0,
            0.0,
            &[],
        )];
        let recipes = vec![Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 4.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec![],
            ingredients: vec![line("flour", 1.0, Unit::Milliliter)],
        }];
        let err = KitchenBook::new(ings, recipes, None).unwrap_err();
        assert!(matches!(err, GastronomeError::UnitMismatch { .. }));
    }

    #[test]
    fn validate_rejects_non_positive_servings() {
        let recipes = vec![Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 0.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec![],
            ingredients: vec![],
        }];
        let err = KitchenBook::new(vec![], recipes, None).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidServings { .. }));
    }

    #[test]
    fn validate_rejects_unknown_dependency() {
        let recipes = vec![Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 4.0,
            prep_minutes: 0,
            cook_minutes: 0,
            depends_on: vec!["missing".into()],
            ingredients: vec![],
        }];
        let err = KitchenBook::new(vec![], recipes, None).unwrap_err();
        assert!(matches!(err, GastronomeError::UnknownRecipe { .. }));
    }

    #[test]
    fn toml_roundtrip_preserves_book() {
        let book = KitchenBook::demo();
        let text = toml::to_string(&book).unwrap();
        let parsed = KitchenBook::from_toml(&text).unwrap();
        assert_eq!(parsed.recipes.len(), book.recipes.len());
        assert_eq!(parsed.ingredients.len(), book.ingredients.len());
        assert_eq!(parsed.recipe("focaccia").unwrap().name, "Rosemary focaccia");
    }

    #[test]
    fn from_toml_rejects_garbage() {
        assert!(matches!(
            KitchenBook::from_toml("not = [valid").unwrap_err(),
            GastronomeError::Parse(_)
        ));
    }

    #[test]
    fn recipe_and_ingredient_lookup_errors() {
        let book = KitchenBook::demo();
        assert!(matches!(
            book.recipe("nope").unwrap_err(),
            GastronomeError::UnknownRecipe { .. }
        ));
        assert!(matches!(
            book.ingredient("nope").unwrap_err(),
            GastronomeError::UnknownIngredient { .. }
        ));
    }
}
