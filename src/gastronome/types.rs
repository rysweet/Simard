//! Core domain types for the Gastronome culinary/menu/event identity.
//!
//! These types are pure data (`serde`-serializable) so a brief and its
//! ingredient catalog can round-trip through JSON, be reasoned about by the
//! LLM-facing Gastronome persona, and be costed/scheduled deterministically by
//! the engine in the sibling modules.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The physical basis an ingredient is measured and priced in.
///
/// Kept intentionally small: mass (grams), volume (millilitres), and discrete
/// count (pieces). Recipe quantities are expressed in the ingredient's own
/// unit, so no unit conversion is required by the engine.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Mass, measured in grams.
    Gram,
    /// Volume, measured in millilitres.
    Milliliter,
    /// Discrete count (e.g. eggs, lemons).
    Piece,
}

impl Unit {
    /// A short human-readable abbreviation for the unit.
    #[must_use]
    pub fn abbrev(self) -> &'static str {
        match self {
            Self::Gram => "g",
            Self::Milliliter => "ml",
            Self::Piece => "pc",
        }
    }
}

/// A dietary tag describing a property an ingredient or recipe satisfies.
///
/// Constraints on a brief are expressed as a set of required tags; a recipe is
/// eligible only if every one of its ingredients carries the required tag.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DietaryTag {
    /// Contains no meat, poultry, or fish.
    Vegetarian,
    /// Contains no animal products whatsoever.
    Vegan,
    /// Contains no gluten.
    GlutenFree,
    /// Contains no dairy.
    DairyFree,
    /// Contains no tree nuts or peanuts.
    NutFree,
    /// Contains no pork and is otherwise halal-compatible.
    Halal,
}

impl DietaryTag {
    /// The lower-case label used in prompts and reports.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Vegetarian => "vegetarian",
            Self::Vegan => "vegan",
            Self::GlutenFree => "gluten_free",
            Self::DairyFree => "dairy_free",
            Self::NutFree => "nut_free",
            Self::Halal => "halal",
        }
    }
}

/// Macro-nutrient facts for one base unit of an ingredient.
///
/// Values are per single base unit (per gram, per millilitre, or per piece)
/// so the engine can multiply linearly by the quantity used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Nutrition {
    /// Energy in kilocalories.
    pub calories: f64,
    /// Protein in grams.
    pub protein_g: f64,
    /// Carbohydrate in grams.
    pub carbs_g: f64,
    /// Fat in grams.
    pub fat_g: f64,
}

impl Nutrition {
    /// Scale every nutrient by `factor`.
    #[must_use]
    pub fn scaled(self, factor: f64) -> Self {
        Self {
            calories: self.calories * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }

    /// Add two nutrition records component-wise.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            calories: self.calories + other.calories,
            protein_g: self.protein_g + other.protein_g,
            carbs_g: self.carbs_g + other.carbs_g,
            fat_g: self.fat_g + other.fat_g,
        }
    }
}

/// A pantry item with per-unit cost, nutrition, and dietary tags.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    /// Unique ingredient name (referenced by recipes).
    pub name: String,
    /// The unit this ingredient is measured and priced in.
    pub unit: Unit,
    /// Cost in USD for one base unit.
    pub cost_per_unit_usd: f64,
    /// Nutrition for one base unit.
    pub nutrition: Nutrition,
    /// Dietary tags this ingredient satisfies.
    #[serde(default)]
    pub tags: BTreeSet<DietaryTag>,
}

/// A quantity of a named ingredient used within a recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeIngredient {
    /// The ingredient name; must resolve against the catalog.
    pub ingredient: String,
    /// Amount, expressed in the ingredient's own unit.
    pub quantity: f64,
}

/// A single preparation step in a recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepStep {
    /// Human-readable description of the task.
    pub description: String,
    /// Active minutes the step takes at the base yield.
    pub minutes: f64,
    /// Whether the step can be completed ahead of service (make-ahead).
    ///
    /// Steps that must be done at service time (`false`) are scheduled to
    /// finish exactly at the event start; make-ahead steps may be pulled
    /// earlier.
    #[serde(default)]
    pub make_ahead: bool,
    /// Whether the step's duration scales with the number of servings.
    ///
    /// Chopping scales with quantity; preheating an oven does not.
    #[serde(default = "default_true")]
    pub scales_with_servings: bool,
}

fn default_true() -> bool {
    true
}

/// The course a menu item belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Course {
    /// An opening small plate.
    Appetizer,
    /// The central dish.
    Main,
    /// An accompaniment.
    Side,
    /// A sweet closing course.
    Dessert,
    /// A drink.
    Beverage,
}

impl Course {
    /// The lower-case label used in reports.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Appetizer => "appetizer",
            Self::Main => "main",
            Self::Side => "side",
            Self::Dessert => "dessert",
            Self::Beverage => "beverage",
        }
    }
}

/// A recipe: a base yield plus its ingredients and prep steps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Recipe name.
    pub name: String,
    /// Course this recipe serves.
    pub course: Course,
    /// Number of servings the base ingredient list yields.
    pub base_servings: f64,
    /// Ingredients used at the base yield.
    pub ingredients: Vec<RecipeIngredient>,
    /// Preparation steps.
    #[serde(default)]
    pub steps: Vec<PrepStep>,
}

/// A menu: an ordered collection of recipes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    /// Menu name.
    pub name: String,
    /// The recipes making up the menu.
    pub recipes: Vec<Recipe>,
}

/// A brief describing an event to be catered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBrief {
    /// Human-readable event name.
    pub event_name: String,
    /// Number of guests to serve.
    pub guest_count: u32,
    /// When service begins (all prep is scheduled to finish by this time).
    pub event_start: DateTime<Utc>,
    /// Target budget per guest in USD (`None` means unconstrained).
    #[serde(default)]
    pub budget_per_guest_usd: Option<f64>,
    /// Dietary constraints every recipe must satisfy.
    #[serde(default)]
    pub dietary_constraints: BTreeSet<DietaryTag>,
    /// Number of cooks available to prep in parallel (minimum 1).
    #[serde(default = "default_cooks")]
    pub cook_count: u32,
    /// The ingredient catalog referenced by the menu's recipes.
    pub catalog: Vec<Ingredient>,
    /// The proposed menu.
    pub menu: Menu,
}

fn default_cooks() -> u32 {
    1
}

impl EventBrief {
    /// The effective number of cooks (never below one).
    #[must_use]
    pub fn effective_cooks(&self) -> u32 {
        self.cook_count.max(1)
    }

    /// The effective guest count (never below one), as `f64`.
    #[must_use]
    pub fn effective_guests(&self) -> f64 {
        f64::from(self.guest_count.max(1))
    }
}
