//! Lookup tables for ingredients and recipes, with id-integrity validation.

use std::collections::BTreeMap;

use super::error::{GastronomeError, GastronomeResult};
use super::types::{Ingredient, Recipe};

/// An indexed set of pantry ingredients keyed by [`Ingredient::id`].
#[derive(Clone, Debug, Default)]
pub struct Pantry {
    by_id: BTreeMap<String, Ingredient>,
}

impl Pantry {
    /// Build a pantry from a list of ingredients, rejecting duplicate ids and
    /// non-finite / negative costs.
    ///
    /// # Errors
    /// Returns [`GastronomeError::DuplicateId`] or
    /// [`GastronomeError::InvalidValue`].
    pub fn new(ingredients: impl IntoIterator<Item = Ingredient>) -> GastronomeResult<Self> {
        let mut by_id = BTreeMap::new();
        for ingredient in ingredients {
            validate_cost(&ingredient)?;
            if by_id.contains_key(&ingredient.id) {
                return Err(GastronomeError::DuplicateId {
                    kind: "ingredient",
                    id: ingredient.id,
                });
            }
            by_id.insert(ingredient.id.clone(), ingredient);
        }
        Ok(Self { by_id })
    }

    /// Look up an ingredient by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Ingredient> {
        self.by_id.get(id)
    }

    /// Number of ingredients in the pantry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the pantry has no ingredients.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

fn validate_cost(ingredient: &Ingredient) -> GastronomeResult<()> {
    if !ingredient.cost_per_unit.is_finite() || ingredient.cost_per_unit < 0.0 {
        return Err(GastronomeError::InvalidValue {
            field: format!("ingredient '{}' cost_per_unit", ingredient.id),
            reason: "must be finite and non-negative".to_string(),
        });
    }
    Ok(())
}

/// An indexed set of recipes keyed by [`Recipe::id`].
#[derive(Clone, Debug, Default)]
pub struct RecipeBook {
    by_id: BTreeMap<String, Recipe>,
}

impl RecipeBook {
    /// Build a recipe book, rejecting duplicate ids, zero servings, negative
    /// quantities, and unknown step dependencies.
    ///
    /// # Errors
    /// Returns the relevant [`GastronomeError`] variant on the first problem.
    pub fn new(recipes: impl IntoIterator<Item = Recipe>) -> GastronomeResult<Self> {
        let mut by_id = BTreeMap::new();
        for recipe in recipes {
            validate_recipe(&recipe)?;
            if by_id.contains_key(&recipe.id) {
                return Err(GastronomeError::DuplicateId {
                    kind: "recipe",
                    id: recipe.id,
                });
            }
            by_id.insert(recipe.id.clone(), recipe);
        }
        Ok(Self { by_id })
    }

    /// Look up a recipe by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.by_id.get(id)
    }

    /// Number of recipes in the book.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the book has no recipes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

fn validate_recipe(recipe: &Recipe) -> GastronomeResult<()> {
    if recipe.servings == 0 {
        return Err(GastronomeError::ZeroServings {
            recipe_id: recipe.id.clone(),
        });
    }
    for line in &recipe.ingredients {
        if !line.quantity.is_finite() || line.quantity < 0.0 {
            return Err(GastronomeError::InvalidValue {
                field: format!(
                    "recipe '{}' ingredient '{}' quantity",
                    recipe.id, line.ingredient_id
                ),
                reason: "must be finite and non-negative".to_string(),
            });
        }
    }
    let step_ids: std::collections::BTreeSet<&str> =
        recipe.steps.iter().map(|s| s.id.as_str()).collect();
    for step in &recipe.steps {
        for dep in &step.depends_on {
            if !step_ids.contains(dep.as_str()) {
                return Err(GastronomeError::UnknownStepDependency {
                    recipe_id: recipe.id.clone(),
                    step_id: step.id.clone(),
                    depends_on: dep.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::types::{PrepStep, RecipeIngredient, Unit};

    fn ingredient(id: &str, cost: f64) -> Ingredient {
        Ingredient {
            id: id.to_string(),
            name: id.to_string(),
            unit: Unit::Gram,
            cost_per_unit: cost,
            nutrition_per_unit: Default::default(),
            allergens: Default::default(),
            vegetarian: true,
            vegan: true,
        }
    }

    #[test]
    fn pantry_rejects_duplicate_ingredient_ids() {
        let err = Pantry::new([ingredient("salt", 0.1), ingredient("salt", 0.2)]).unwrap_err();
        assert!(matches!(
            err,
            GastronomeError::DuplicateId {
                kind: "ingredient",
                ..
            }
        ));
    }

    #[test]
    fn pantry_rejects_negative_cost() {
        let err = Pantry::new([ingredient("salt", -1.0)]).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidValue { .. }));
    }

    #[test]
    fn pantry_lookup_and_len() {
        let pantry = Pantry::new([ingredient("salt", 0.1), ingredient("pepper", 0.3)]).unwrap();
        assert_eq!(pantry.len(), 2);
        assert!(!pantry.is_empty());
        assert_eq!(pantry.get("salt").unwrap().name, "salt");
        assert!(pantry.get("missing").is_none());
    }

    #[test]
    fn recipe_book_rejects_zero_servings() {
        let recipe = Recipe {
            id: "empty".into(),
            name: "Empty".into(),
            servings: 0,
            ingredients: vec![],
            steps: vec![],
        };
        let err = RecipeBook::new([recipe]).unwrap_err();
        assert!(matches!(err, GastronomeError::ZeroServings { .. }));
    }

    #[test]
    fn recipe_book_rejects_unknown_step_dependency() {
        let recipe = Recipe {
            id: "cake".into(),
            name: "Cake".into(),
            servings: 8,
            ingredients: vec![RecipeIngredient {
                ingredient_id: "flour".into(),
                quantity: 200.0,
            }],
            steps: vec![PrepStep {
                id: "bake".into(),
                description: "bake".into(),
                duration_minutes: 30,
                depends_on: vec!["mix".into()],
            }],
        };
        let err = RecipeBook::new([recipe]).unwrap_err();
        assert!(matches!(err, GastronomeError::UnknownStepDependency { .. }));
    }

    #[test]
    fn recipe_book_accepts_valid_recipe() {
        let recipe = Recipe {
            id: "cake".into(),
            name: "Cake".into(),
            servings: 8,
            ingredients: vec![RecipeIngredient {
                ingredient_id: "flour".into(),
                quantity: 200.0,
            }],
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
                    duration_minutes: 30,
                    depends_on: vec!["mix".into()],
                },
            ],
        };
        let book = RecipeBook::new([recipe]).unwrap();
        assert_eq!(book.len(), 1);
        assert!(!book.is_empty());
        assert_eq!(book.get("cake").unwrap().servings, 8);
    }
}
