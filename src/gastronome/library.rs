//! The built-in pantry: a curated set of ingredients, recipes, and menus so
//! the Gastronome CLI produces a real costed + scheduled plan out of the box,
//! with no external data files required.
//!
//! [`Pantry`] is also the resolution surface the rest of the module builds on:
//! it owns the ingredient / recipe / menu tables and answers lookups by id.

use std::collections::BTreeMap;

use super::types::{
    Course, DietaryTag, GastronomeError, GastronomeResult, Ingredient, Menu, NutritionFacts,
    Recipe, RecipeIngredient, RecipeStep, Stage,
};

/// A pantry: the ingredient, recipe, and menu catalogue a plan resolves
/// against. Ids are unique within each table.
#[derive(Clone, Debug, Default)]
pub struct Pantry {
    ingredients: BTreeMap<String, Ingredient>,
    recipes: BTreeMap<String, Recipe>,
    menus: BTreeMap<String, Menu>,
}

impl Pantry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_ingredient(&mut self, ingredient: Ingredient) {
        self.ingredients.insert(ingredient.id.clone(), ingredient);
    }

    pub fn add_recipe(&mut self, recipe: Recipe) {
        self.recipes.insert(recipe.id.clone(), recipe);
    }

    pub fn add_menu(&mut self, menu: Menu) {
        self.menus.insert(menu.id.clone(), menu);
    }

    pub fn ingredient(&self, id: &str) -> GastronomeResult<&Ingredient> {
        self.ingredients
            .get(id)
            .ok_or_else(|| GastronomeError::UnknownIngredient(id.to_string()))
    }

    pub fn recipe(&self, id: &str) -> GastronomeResult<&Recipe> {
        self.recipes
            .get(id)
            .ok_or_else(|| GastronomeError::UnknownRecipe(id.to_string()))
    }

    pub fn menu(&self, id: &str) -> GastronomeResult<&Menu> {
        self.menus
            .get(id)
            .ok_or_else(|| GastronomeError::UnknownMenu(id.to_string()))
    }

    /// All recipes, ordered by id (deterministic).
    pub fn recipes(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes.values()
    }

    /// All menus, ordered by id (deterministic).
    pub fn menus(&self) -> impl Iterator<Item = &Menu> {
        self.menus.values()
    }

    /// The set of dietary tags a recipe satisfies — the intersection of the
    /// tags satisfied by every one of its ingredients. An empty recipe (no
    /// ingredients) satisfies nothing.
    pub fn recipe_dietary_tags(
        &self,
        recipe: &Recipe,
    ) -> GastronomeResult<std::collections::BTreeSet<DietaryTag>> {
        let mut iter = recipe.ingredients.iter();
        let Some(first) = iter.next() else {
            return Ok(std::collections::BTreeSet::new());
        };
        let mut acc: std::collections::BTreeSet<DietaryTag> =
            self.ingredient(&first.ingredient_id)?.tags.clone();
        for line in iter {
            let tags = &self.ingredient(&line.ingredient_id)?.tags;
            acc = acc.intersection(tags).copied().collect();
        }
        Ok(acc)
    }
}

/// Build the curated built-in pantry.
///
/// Costs are illustrative unit costs in a generic currency; nutrition is
/// per-unit (per gram for dry/wet goods, per piece for whole items).
pub fn builtin_pantry() -> Pantry {
    let mut p = Pantry::new();

    // ---- Ingredients (per-unit cost + nutrition) ----
    // Dry / pantry staples: per gram.
    p.add_ingredient(Ingredient::new(
        "flour",
        "All-purpose flour",
        "g",
        0.0015,
        NutritionFacts::new(3.64, 0.10, 0.76, 0.01),
        [
            DietaryTag::Vegan,
            DietaryTag::Vegetarian,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
            DietaryTag::Pescatarian,
            DietaryTag::Halal,
            DietaryTag::Kosher,
        ],
    ));
    p.add_ingredient(Ingredient::new(
        "sugar",
        "Granulated sugar",
        "g",
        0.0012,
        NutritionFacts::new(3.87, 0.0, 1.0, 0.0),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "olive_oil",
        "Olive oil",
        "ml",
        0.012,
        NutritionFacts::new(8.84, 0.0, 0.0, 1.0),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "salt",
        "Sea salt",
        "g",
        0.0008,
        NutritionFacts::new(0.0, 0.0, 0.0, 0.0),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "tomato",
        "Roma tomato",
        "g",
        0.004,
        NutritionFacts::new(0.18, 0.009, 0.039, 0.002),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "basil",
        "Fresh basil",
        "g",
        0.03,
        NutritionFacts::new(0.23, 0.032, 0.027, 0.006),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "mozzarella",
        "Fresh mozzarella",
        "g",
        0.011,
        NutritionFacts::new(2.80, 0.22, 0.022, 0.17),
        // Dairy: vegetarian + gluten-free + nut-free, not vegan / dairy-free.
        [
            DietaryTag::Vegetarian,
            DietaryTag::GlutenFree,
            DietaryTag::NutFree,
            DietaryTag::Halal,
            DietaryTag::Kosher,
        ],
    ));
    p.add_ingredient(Ingredient::new(
        "pasta",
        "Durum wheat pasta",
        "g",
        0.003,
        NutritionFacts::new(3.71, 0.13, 0.75, 0.015),
        // Contains gluten: vegan otherwise.
        [
            DietaryTag::Vegan,
            DietaryTag::Vegetarian,
            DietaryTag::DairyFree,
            DietaryTag::NutFree,
            DietaryTag::Pescatarian,
            DietaryTag::Halal,
            DietaryTag::Kosher,
        ],
    ));
    p.add_ingredient(Ingredient::new(
        "chickpeas",
        "Cooked chickpeas",
        "g",
        0.0035,
        NutritionFacts::new(1.64, 0.089, 0.27, 0.026),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "lemon",
        "Lemon",
        "piece",
        0.45,
        NutritionFacts::new(17.0, 0.6, 5.4, 0.2),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "cucumber",
        "Cucumber",
        "g",
        0.003,
        NutritionFacts::new(0.15, 0.007, 0.036, 0.001),
        vegan_gf(),
    ));
    p.add_ingredient(Ingredient::new(
        "dark_chocolate",
        "Dark chocolate (70%)",
        "g",
        0.02,
        NutritionFacts::new(5.98, 0.078, 0.46, 0.43),
        // Dairy-free dark chocolate, but not nut-free (shared lines).
        [
            DietaryTag::Vegan,
            DietaryTag::Vegetarian,
            DietaryTag::DairyFree,
            DietaryTag::GlutenFree,
            DietaryTag::Halal,
            DietaryTag::Kosher,
        ],
    ));
    p.add_ingredient(Ingredient::new(
        "strawberry",
        "Strawberry",
        "g",
        0.008,
        NutritionFacts::new(0.32, 0.007, 0.077, 0.003),
        vegan_gf(),
    ));

    // ---- Recipes ----
    p.add_recipe(Recipe {
        id: "caprese".into(),
        name: "Caprese salad".into(),
        description: "Tomato, fresh mozzarella, and basil with olive oil.".into(),
        course: Course::Appetizer,
        servings: 4,
        ingredients: vec![
            RecipeIngredient::new("tomato", 400.0),
            RecipeIngredient::new("mozzarella", 250.0),
            RecipeIngredient::new("basil", 20.0),
            RecipeIngredient::new("olive_oil", 40.0),
            RecipeIngredient::new("salt", 4.0),
        ],
        steps: vec![
            RecipeStep::new("Slice tomatoes and mozzarella", Stage::Prep, 15),
            RecipeStep::new(
                "Arrange, tear basil, dress with oil and salt",
                Stage::Plate,
                10,
            ),
        ],
    });
    p.add_recipe(Recipe {
        id: "hummus".into(),
        name: "Lemon hummus with crudités".into(),
        description: "Chickpea dip with lemon, olive oil, and cucumber batons.".into(),
        course: Course::Appetizer,
        servings: 6,
        ingredients: vec![
            RecipeIngredient::new("chickpeas", 600.0),
            RecipeIngredient::new("olive_oil", 60.0),
            RecipeIngredient::new("lemon", 2.0),
            RecipeIngredient::new("salt", 6.0),
            RecipeIngredient::new("cucumber", 300.0),
        ],
        steps: vec![
            RecipeStep::new("Juice lemons, cut cucumber batons", Stage::Prep, 10),
            RecipeStep::new("Blend chickpeas, oil, lemon, salt", Stage::Cook, 10),
            RecipeStep::new("Plate dip with crudités", Stage::Plate, 5),
        ],
    });
    p.add_recipe(Recipe {
        id: "pasta_pomodoro".into(),
        name: "Pasta pomodoro".into(),
        description: "Durum pasta in a fresh tomato-basil sauce.".into(),
        course: Course::Main,
        servings: 4,
        ingredients: vec![
            RecipeIngredient::new("pasta", 480.0),
            RecipeIngredient::new("tomato", 600.0),
            RecipeIngredient::new("basil", 15.0),
            RecipeIngredient::new("olive_oil", 45.0),
            RecipeIngredient::new("salt", 8.0),
        ],
        steps: vec![
            RecipeStep::new("Chop tomatoes and basil", Stage::Prep, 15),
            RecipeStep::new("Simmer sauce, boil pasta", Stage::Cook, 25),
            RecipeStep::new("Toss and plate", Stage::Plate, 10),
        ],
    });
    p.add_recipe(Recipe {
        id: "chickpea_bowl".into(),
        name: "Warm chickpea & cucumber bowl".into(),
        description: "Vegan, gluten-free chickpea main with lemon dressing.".into(),
        course: Course::Main,
        servings: 4,
        ingredients: vec![
            RecipeIngredient::new("chickpeas", 700.0),
            RecipeIngredient::new("cucumber", 250.0),
            RecipeIngredient::new("olive_oil", 40.0),
            RecipeIngredient::new("lemon", 2.0),
            RecipeIngredient::new("salt", 6.0),
        ],
        steps: vec![
            RecipeStep::new("Dice cucumber, juice lemon", Stage::Prep, 12),
            RecipeStep::new("Warm chickpeas with oil and salt", Stage::Cook, 12),
            RecipeStep::new("Assemble bowls", Stage::Plate, 8),
        ],
    });
    p.add_recipe(Recipe {
        id: "chocolate_mousse".into(),
        name: "Dark chocolate mousse".into(),
        description: "Dairy-free dark chocolate mousse.".into(),
        course: Course::Dessert,
        servings: 6,
        ingredients: vec![
            RecipeIngredient::new("dark_chocolate", 300.0),
            RecipeIngredient::new("sugar", 60.0),
            RecipeIngredient::new("strawberry", 180.0),
        ],
        steps: vec![
            RecipeStep::new("Melt chocolate, hull strawberries", Stage::Prep, 12),
            RecipeStep::new("Whip and fold, chill", Stage::Cook, 20),
            RecipeStep::new("Portion and garnish", Stage::Plate, 10),
        ],
    });

    // ---- Menus ----
    p.add_menu(Menu::new(
        "italian-dinner",
        "Italian dinner",
        ["caprese", "pasta_pomodoro", "chocolate_mousse"],
    ));
    p.add_menu(Menu::new(
        "vegan-gf-lunch",
        "Vegan gluten-free lunch",
        ["hummus", "chickpea_bowl", "chocolate_mousse"],
    ));

    p
}

/// Tags shared by simple plant ingredients: everything except the
/// dairy/gluten violations they don't have.
fn vegan_gf() -> Vec<DietaryTag> {
    vec![
        DietaryTag::Vegan,
        DietaryTag::Vegetarian,
        DietaryTag::GlutenFree,
        DietaryTag::DairyFree,
        DietaryTag::NutFree,
        DietaryTag::Pescatarian,
        DietaryTag::Halal,
        DietaryTag::Kosher,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_pantry_resolves_all_menu_recipes_and_ingredients() {
        let p = builtin_pantry();
        for menu in p.menus() {
            assert!(!menu.recipe_ids.is_empty(), "menu {} empty", menu.id);
            for rid in &menu.recipe_ids {
                let recipe = p.recipe(rid).expect("recipe resolves");
                for line in &recipe.ingredients {
                    p.ingredient(&line.ingredient_id)
                        .expect("ingredient resolves");
                }
            }
        }
    }

    #[test]
    fn unknown_lookups_error() {
        let p = builtin_pantry();
        assert!(p.recipe("nope").is_err());
        assert!(p.ingredient("nope").is_err());
        assert!(p.menu("nope").is_err());
    }

    #[test]
    fn recipe_dietary_tags_are_ingredient_intersection() {
        let p = builtin_pantry();
        // Caprese contains mozzarella (dairy) → NOT vegan / dairy-free, but
        // it is vegetarian + gluten-free.
        let caprese = p.recipe("caprese").unwrap();
        let tags = p.recipe_dietary_tags(caprese).unwrap();
        assert!(tags.contains(&DietaryTag::Vegetarian));
        assert!(tags.contains(&DietaryTag::GlutenFree));
        assert!(!tags.contains(&DietaryTag::Vegan));
        assert!(!tags.contains(&DietaryTag::DairyFree));
    }

    #[test]
    fn vegan_gf_bowl_satisfies_vegan_and_gluten_free() {
        let p = builtin_pantry();
        let bowl = p.recipe("chickpea_bowl").unwrap();
        let tags = p.recipe_dietary_tags(bowl).unwrap();
        assert!(tags.contains(&DietaryTag::Vegan));
        assert!(tags.contains(&DietaryTag::GlutenFree));
    }

    #[test]
    fn pasta_is_not_gluten_free() {
        let p = builtin_pantry();
        let pasta = p.recipe("pasta_pomodoro").unwrap();
        let tags = p.recipe_dietary_tags(pasta).unwrap();
        assert!(!tags.contains(&DietaryTag::GlutenFree));
        assert!(tags.contains(&DietaryTag::Vegan));
    }
}
