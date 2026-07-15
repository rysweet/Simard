//! Ingredient catalog: name -> [`Ingredient`] lookup with validation.

use std::collections::BTreeMap;

use super::types::{Ingredient, Recipe};
use super::{GastronomeError, GastronomeResult};

/// An indexed view over a brief's ingredient list for O(log n) lookup.
#[derive(Clone, Debug)]
pub struct Catalog<'a> {
    by_name: BTreeMap<&'a str, &'a Ingredient>,
}

impl<'a> Catalog<'a> {
    /// Build a catalog index from a slice of ingredients.
    ///
    /// Later entries with a duplicate name override earlier ones.
    #[must_use]
    pub fn new(ingredients: &'a [Ingredient]) -> Self {
        let by_name = ingredients
            .iter()
            .map(|ing| (ing.name.as_str(), ing))
            .collect();
        Self { by_name }
    }

    /// Look up an ingredient by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'a Ingredient> {
        self.by_name.get(name).copied()
    }

    /// Verify every ingredient referenced by `recipe` exists in the catalog.
    ///
    /// # Errors
    ///
    /// Returns [`GastronomeError::UnknownIngredient`] for the first missing
    /// reference encountered.
    pub fn validate_recipe(&self, recipe: &Recipe) -> GastronomeResult<()> {
        for ri in &recipe.ingredients {
            if self.get(&ri.ingredient).is_none() {
                return Err(GastronomeError::UnknownIngredient {
                    recipe: recipe.name.clone(),
                    ingredient: ri.ingredient.clone(),
                });
            }
        }
        Ok(())
    }
}
