//! The Gastronome identity: a self-contained culinary planner that turns an
//! event/menu brief into a costed, nutritionally analysed, and prep-scheduled
//! menu plan.
//!
//! Gastronome is a *pluggable Simard identity* (see
//! `prompt_assets/simard/gastronome_system.md` and the builtin
//! `simard-gastronome` entry in [`crate::identity`]). Where the engineering
//! identities reason about code, Gastronome reasons about kitchens: it designs
//! recipes and menus, then runs the deterministic planning math a kitchen needs
//! to execute them.
//!
//! # Layers
//!
//! - [`nutrition`] — an additive macronutrient vector.
//! - [`types`] — ingredients, recipes, the dietary vocabulary, and the brief.
//! - [`scaling`] — scale a recipe to a guest count; consolidate shopping lists.
//! - [`analysis`] — cost and nutrition summaries over a resolved menu.
//! - [`schedule`] — back-schedule prep so every dish lands at serve time.
//! - [`recipe_book`] — a searchable recipe collection with a built-in sample.
//! - [`plan`] — the end-to-end [`plan::plan_event`] entry point.
//!
//! # Example
//!
//! ```
//! use simard::gastronome::{plan_event, EventBrief, CourseRequest, RecipeBook};
//!
//! let brief = EventBrief {
//!     name: "Team Dinner".into(),
//!     guests: 10,
//!     serve_time: "19:30".into(),
//!     courses: vec![CourseRequest::new("starter"), CourseRequest::new("main")],
//!     dietary_constraints: vec![],
//! };
//! let plan = plan_event(&brief, &RecipeBook::builtin()).unwrap();
//! assert_eq!(plan.menu.len(), 2);
//! assert!(plan.cost.cost_per_guest > 0.0);
//! ```

pub mod analysis;
pub mod error;
pub mod nutrition;
pub mod plan;
pub mod recipe_book;
pub mod scaling;
pub mod schedule;
pub mod types;

pub use analysis::{CostSummary, CourseCost, NutritionSummary};
pub use error::{GastronomeError, GastronomeResult};
pub use nutrition::Nutrition;
pub use plan::{EventPlan, plan_event};
pub use recipe_book::RecipeBook;
pub use scaling::{MenuItem, ScaledRecipe, ShoppingLine, consolidate_shopping};
pub use schedule::{PrepSchedule, ScheduledStep};
pub use types::{
    CourseRequest, DietaryTag, EventBrief, Ingredient, PrepStep, Recipe, RecipeIngredient,
};
