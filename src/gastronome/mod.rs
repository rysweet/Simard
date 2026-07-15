//! Gastronome: a pluggable Simard identity for culinary, menu, and event design.
//!
//! This module is a self-contained "brick": pure-data domain types plus a
//! deterministic engine that turns an [`EventBrief`] into a costed, scheduled
//! [`MenuPlan`]. The LLM-facing persona (see
//! `prompt_assets/simard/gastronome_system.md`) delegates all numeric work —
//! nutrition, cost, scaling, and prep scheduling — to this engine so the
//! figures in a plan are reproducible rather than hallucinated.
//!
//! # Pipeline
//!
//! ```text
//! EventBrief ──▶ validate constraints ──▶ scale recipes to guests
//!            ──▶ cost per guest / event ──▶ nutrition per guest
//!            ──▶ backward prep schedule ──▶ MenuPlan
//! ```
//!
//! # Example
//!
//! ```
//! use simard::gastronome::{plan_event, sample_brief};
//!
//! let brief = sample_brief();
//! let plan = plan_event(&brief).expect("sample brief is valid");
//! assert!(plan.cost.per_guest_usd > 0.0);
//! assert!(!plan.schedule.tasks.is_empty());
//! ```

mod catalog;
mod cost;
mod nutrition;
mod planner;
mod sample;
mod scaling;
mod scheduling;
pub mod types;

#[cfg(test)]
mod tests;

use std::fmt::{self, Display, Formatter};

pub use catalog::Catalog;
pub use cost::{CostBreakdown, RecipeCost, recipe_cost};
pub use nutrition::{NutritionBreakdown, recipe_nutrition};
pub use planner::{BudgetStatus, MenuPlan, plan_event};
pub use sample::sample_brief;
pub use scaling::scale_recipe;
pub use scheduling::{PrepSchedule, PrepTask, build_schedule};
pub use types::{
    Course, DietaryTag, EventBrief, Ingredient, Menu, Nutrition, PrepStep, Recipe,
    RecipeIngredient, Unit,
};

/// Errors produced while planning a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GastronomeError {
    /// A recipe referenced an ingredient absent from the catalog.
    UnknownIngredient {
        /// The recipe that referenced the missing ingredient.
        recipe: String,
        /// The missing ingredient name.
        ingredient: String,
    },
    /// One or more recipes violate the brief's dietary constraints.
    DietaryViolation {
        /// Human-readable descriptions of each violation.
        violations: Vec<String>,
    },
    /// A recipe declared a non-positive base yield.
    InvalidYield {
        /// The offending recipe.
        recipe: String,
    },
    /// The menu contained no recipes.
    EmptyMenu,
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownIngredient { recipe, ingredient } => write!(
                f,
                "recipe '{recipe}' references unknown ingredient '{ingredient}'"
            ),
            Self::DietaryViolation { violations } => {
                write!(f, "dietary constraints violated: {}", violations.join("; "))
            }
            Self::InvalidYield { recipe } => {
                write!(f, "recipe '{recipe}' has a non-positive base yield")
            }
            Self::EmptyMenu => write!(f, "menu contains no recipes"),
        }
    }
}

impl std::error::Error for GastronomeError {}

/// Result alias for gastronome operations.
pub type GastronomeResult<T> = Result<T, GastronomeError>;
