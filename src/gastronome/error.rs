//! Error type for the Gastronome engine.
//!
//! The engine is a self-contained "brick": it defines its own error rather than
//! rippling variants into the central [`crate::error::SimardError`], so it can be
//! regenerated or lifted out without touching the rest of the codebase.

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Anything that can go wrong while resolving or planning a menu.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GastronomeError {
    /// A recipe referenced a pantry ingredient id that does not exist.
    UnknownIngredient {
        recipe_id: String,
        ingredient_id: String,
    },
    /// A menu item referenced a recipe id that does not exist.
    UnknownRecipe { recipe_id: String },
    /// Two ingredients or recipes shared an id.
    DuplicateId { kind: &'static str, id: String },
    /// A recipe declared zero servings, which cannot be scaled.
    ZeroServings { recipe_id: String },
    /// A prep step depended on a step id that does not exist in its recipe.
    UnknownStepDependency {
        recipe_id: String,
        step_id: String,
        depends_on: String,
    },
    /// The prep steps of a recipe form a cycle and cannot be scheduled.
    CyclicPrepSteps { recipe_id: String },
    /// A quantity, cost, or duration was negative or non-finite.
    InvalidValue { field: String, reason: String },
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownIngredient {
                recipe_id,
                ingredient_id,
            } => write!(
                f,
                "recipe '{recipe_id}' references unknown ingredient '{ingredient_id}'"
            ),
            Self::UnknownRecipe { recipe_id } => {
                write!(f, "menu references unknown recipe '{recipe_id}'")
            }
            Self::DuplicateId { kind, id } => write!(f, "duplicate {kind} id '{id}'"),
            Self::ZeroServings { recipe_id } => {
                write!(f, "recipe '{recipe_id}' declares zero servings")
            }
            Self::UnknownStepDependency {
                recipe_id,
                step_id,
                depends_on,
            } => write!(
                f,
                "recipe '{recipe_id}' step '{step_id}' depends on unknown step '{depends_on}'"
            ),
            Self::CyclicPrepSteps { recipe_id } => {
                write!(f, "recipe '{recipe_id}' prep steps contain a cycle")
            }
            Self::InvalidValue { field, reason } => {
                write!(f, "invalid value for '{field}': {reason}")
            }
        }
    }
}

impl Error for GastronomeError {}

/// Convenience result alias for the engine.
pub type GastronomeResult<T> = Result<T, GastronomeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_descriptive() {
        let e = GastronomeError::UnknownIngredient {
            recipe_id: "soup".into(),
            ingredient_id: "unobtanium".into(),
        };
        assert!(e.to_string().contains("soup"));
        assert!(e.to_string().contains("unobtanium"));

        let e = GastronomeError::CyclicPrepSteps {
            recipe_id: "cake".into(),
        };
        assert!(e.to_string().contains("cycle"));

        let e = GastronomeError::DuplicateId {
            kind: "ingredient",
            id: "salt".into(),
        };
        assert!(e.to_string().contains("duplicate ingredient id 'salt'"));
    }

    #[test]
    fn error_is_std_error() {
        fn assert_error<E: Error>(_: &E) {}
        assert_error(&GastronomeError::UnknownRecipe {
            recipe_id: "x".into(),
        });
    }
}
