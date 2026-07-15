//! Core domain types for the Gastronome culinary / menu & event-design
//! identity.
//!
//! Everything here is pure, deterministic data: pantry ingredients with
//! per-unit cost and nutrition, recipes composed from those ingredients,
//! menus that select recipes, an event brief that drives planning, and the
//! resulting costed + scheduled [`MenuPlan`]. No I/O, no clocks, no
//! randomness — so the whole pipeline is trivially testable and reproducible.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// A dietary property. On an [`Ingredient`] the set lists the *positive*
/// properties the ingredient satisfies (e.g. an apple is `Vegan`,
/// `GlutenFree`, `DairyFree`, `NutFree`). On an [`EventBrief`] the set lists
/// the restrictions every served recipe MUST satisfy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DietaryTag {
    Vegetarian,
    Vegan,
    GlutenFree,
    DairyFree,
    NutFree,
    Pescatarian,
    Halal,
    Kosher,
}

impl DietaryTag {
    /// All tags, used when deriving an ingredient's satisfied set from its
    /// declared *violations*.
    pub const ALL: [DietaryTag; 8] = [
        DietaryTag::Vegetarian,
        DietaryTag::Vegan,
        DietaryTag::GlutenFree,
        DietaryTag::DairyFree,
        DietaryTag::NutFree,
        DietaryTag::Pescatarian,
        DietaryTag::Halal,
        DietaryTag::Kosher,
    ];
}

impl Display for DietaryTag {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            DietaryTag::Vegetarian => "vegetarian",
            DietaryTag::Vegan => "vegan",
            DietaryTag::GlutenFree => "gluten-free",
            DietaryTag::DairyFree => "dairy-free",
            DietaryTag::NutFree => "nut-free",
            DietaryTag::Pescatarian => "pescatarian",
            DietaryTag::Halal => "halal",
            DietaryTag::Kosher => "kosher",
        };
        f.write_str(s)
    }
}

/// The course an item occupies on a menu. Used to compose balanced menus and
/// to order plating in the prep schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Course {
    Appetizer,
    Main,
    Side,
    Dessert,
    Beverage,
}

impl Display for Course {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            Course::Appetizer => "appetizer",
            Course::Main => "main",
            Course::Side => "side",
            Course::Dessert => "dessert",
            Course::Beverage => "beverage",
        };
        f.write_str(s)
    }
}

/// The kitchen stage a prep step belongs to. Earlier stages are scheduled to
/// finish before later stages so the backward scheduler keeps a sane ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Mise en place — chopping, measuring, marinating.
    Prep,
    /// Active cooking — the oven/stove work.
    Cook,
    /// Final assembly and plating just before service.
    Plate,
}

impl Stage {
    /// Ordered stages, earliest first. Used by the scheduler.
    pub const ORDERED: [Stage; 3] = [Stage::Prep, Stage::Cook, Stage::Plate];
}

impl Display for Stage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            Stage::Prep => "prep",
            Stage::Cook => "cook",
            Stage::Plate => "plate",
        };
        f.write_str(s)
    }
}

/// Macro-nutrient facts. On an [`Ingredient`] these are expressed *per unit*
/// (per gram, per millilitre, per piece — whatever the ingredient's `unit`
/// is). Aggregated values reuse the same struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NutritionFacts {
    pub calories: f64,
    pub protein_g: f64,
    pub carbs_g: f64,
    pub fat_g: f64,
}

impl NutritionFacts {
    pub fn new(calories: f64, protein_g: f64, carbs_g: f64, fat_g: f64) -> Self {
        Self {
            calories,
            protein_g,
            carbs_g,
            fat_g,
        }
    }

    /// Multiply every field by `factor` (used to scale per-unit facts by a
    /// quantity, or to scale a recipe up to a headcount).
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        Self {
            calories: self.calories * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }

    /// Round each field to one decimal place for stable display / JSON.
    #[must_use]
    pub fn rounded(&self) -> Self {
        Self {
            calories: round1(self.calories),
            protein_g: round1(self.protein_g),
            carbs_g: round1(self.carbs_g),
            fat_g: round1(self.fat_g),
        }
    }
}

impl std::ops::Add for NutritionFacts {
    type Output = NutritionFacts;
    fn add(self, rhs: NutritionFacts) -> NutritionFacts {
        NutritionFacts {
            calories: self.calories + rhs.calories,
            protein_g: self.protein_g + rhs.protein_g,
            carbs_g: self.carbs_g + rhs.carbs_g,
            fat_g: self.fat_g + rhs.fat_g,
        }
    }
}

impl std::iter::Sum for NutritionFacts {
    fn sum<I: Iterator<Item = NutritionFacts>>(iter: I) -> Self {
        iter.fold(NutritionFacts::default(), |acc, x| acc + x)
    }
}

/// A pantry item: a purchasable ingredient with a canonical `unit`, a
/// `cost_per_unit` (in the plan's currency), per-unit nutrition, and the set
/// of dietary tags it *satisfies*.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    pub id: String,
    pub name: String,
    /// Human-facing unit label the quantities are expressed in ("g", "ml",
    /// "piece", …). Purely descriptive — the math is unit-agnostic.
    pub unit: String,
    pub cost_per_unit: f64,
    pub nutrition_per_unit: NutritionFacts,
    #[serde(default)]
    pub tags: BTreeSet<DietaryTag>,
}

impl Ingredient {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        unit: impl Into<String>,
        cost_per_unit: f64,
        nutrition_per_unit: NutritionFacts,
        tags: impl IntoIterator<Item = DietaryTag>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            unit: unit.into(),
            cost_per_unit,
            nutrition_per_unit,
            tags: tags.into_iter().collect(),
        }
    }
}

/// One line of a recipe: how much of a given ingredient (by id) it uses, in
/// that ingredient's unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeIngredient {
    pub ingredient_id: String,
    pub quantity: f64,
}

impl RecipeIngredient {
    pub fn new(ingredient_id: impl Into<String>, quantity: f64) -> Self {
        Self {
            ingredient_id: ingredient_id.into(),
            quantity,
        }
    }
}

/// A single prep step with its kitchen `stage` and active-work `minutes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeStep {
    pub description: String,
    pub stage: Stage,
    pub minutes: u32,
}

impl RecipeStep {
    pub fn new(description: impl Into<String>, stage: Stage, minutes: u32) -> Self {
        Self {
            description: description.into(),
            stage,
            minutes,
        }
    }
}

/// A recipe: a base yield (`servings`) plus the ingredient lines and prep
/// steps needed to produce it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub course: Course,
    /// Base number of servings the ingredient quantities produce.
    pub servings: u32,
    pub ingredients: Vec<RecipeIngredient>,
    #[serde(default)]
    pub steps: Vec<RecipeStep>,
}

/// A named menu: an ordered selection of recipe ids.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Menu {
    pub id: String,
    pub name: String,
    pub recipe_ids: Vec<String>,
}

impl Menu {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        recipe_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            recipe_ids: recipe_ids.into_iter().map(Into::into).collect(),
        }
    }
}

/// The operator's request: the event, its size, dietary constraints, budget,
/// and the service time the food must be ready by.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBrief {
    pub event_name: String,
    pub guest_count: u32,
    /// The named menu to cost & schedule. (A future extension may auto-compose
    /// a menu; today the brief names one.)
    pub menu_id: String,
    #[serde(default)]
    pub dietary_restrictions: BTreeSet<DietaryTag>,
    /// Optional per-guest budget cap in the plan currency. Exceeding it emits
    /// a warning rather than failing the plan.
    #[serde(default)]
    pub budget_per_guest: Option<f64>,
    /// Service time, expressed as minutes-from-midnight (e.g. 18*60 = 1080 for
    /// 18:00). The prep schedule is computed backwards from here.
    pub service_time_min: u32,
}

/// A recipe scaled to the number of servings the event needs, with its own
/// nutrition and cost already computed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledRecipe {
    pub recipe_id: String,
    pub name: String,
    pub course: Course,
    pub base_servings: u32,
    pub target_servings: u32,
    /// Multiplier applied to base quantities (`target / base`).
    pub scale_factor: f64,
    /// Ingredient lines with quantities already multiplied by `scale_factor`.
    pub ingredients: Vec<ScaledIngredientLine>,
    /// Total nutrition for the whole scaled batch.
    pub nutrition_total: NutritionFacts,
    /// Total cost for the whole scaled batch.
    pub cost_total: f64,
}

/// A scaled, resolved ingredient line (name + rounded quantity + line cost).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScaledIngredientLine {
    pub ingredient_id: String,
    pub name: String,
    pub unit: String,
    pub quantity: f64,
    pub line_cost: f64,
}

/// Cost roll-up for the whole plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub total: f64,
    pub per_guest: f64,
    /// `(recipe name, total cost)` for each recipe on the plan.
    pub per_recipe: Vec<(String, f64)>,
}

/// Nutrition roll-up for the whole plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NutritionSummary {
    pub total: NutritionFacts,
    pub per_guest: NutritionFacts,
}

/// One concrete, time-boxed task in the prep schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepTask {
    pub recipe_name: String,
    pub stage: Stage,
    pub description: String,
    /// Start time, minutes-from-midnight.
    pub start_min: u32,
    /// End time, minutes-from-midnight.
    pub end_min: u32,
}

/// The ordered prep schedule plus the derived kitchen-start time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepSchedule {
    /// Earliest task start (minutes-from-midnight): when the kitchen must open.
    pub kitchen_start_min: u32,
    /// Service time the schedule targets (minutes-from-midnight).
    pub service_time_min: u32,
    pub tasks: Vec<PrepTask>,
}

/// The end-to-end output: a costed, scheduled menu plan for the event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuPlan {
    pub event_name: String,
    pub guest_count: u32,
    pub menu_name: String,
    pub recipes: Vec<ScaledRecipe>,
    pub nutrition: NutritionSummary,
    pub cost: CostBreakdown,
    pub schedule: PrepSchedule,
    /// Non-fatal advisories (over budget, etc.).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Errors the Gastronome domain can return. Self-contained so the module does
/// not widen the crate-wide `SimardError` surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GastronomeError {
    /// A referenced recipe id was not found in the pantry/library.
    UnknownRecipe(String),
    /// A referenced ingredient id was not found in the pantry.
    UnknownIngredient(String),
    /// A referenced menu id was not found.
    UnknownMenu(String),
    /// Guest count or a recipe's base servings was zero.
    InvalidQuantity { field: String },
    /// The menu had no recipes.
    EmptyMenu(String),
    /// A recipe on the menu violates a required dietary restriction.
    DietaryConflict { recipe: String, tag: DietaryTag },
    /// Failed to parse an [`EventBrief`] from an input document.
    Parse(String),
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            GastronomeError::UnknownRecipe(id) => write!(f, "unknown recipe '{id}'"),
            GastronomeError::UnknownIngredient(id) => write!(f, "unknown ingredient '{id}'"),
            GastronomeError::UnknownMenu(id) => write!(f, "unknown menu '{id}'"),
            GastronomeError::InvalidQuantity { field } => {
                write!(f, "invalid quantity: '{field}' must be greater than zero")
            }
            GastronomeError::EmptyMenu(id) => write!(f, "menu '{id}' has no recipes"),
            GastronomeError::DietaryConflict { recipe, tag } => {
                write!(f, "recipe '{recipe}' violates dietary restriction '{tag}'")
            }
            GastronomeError::Parse(reason) => write!(f, "failed to parse brief: {reason}"),
        }
    }
}

impl Error for GastronomeError {}

/// Result alias for the Gastronome domain.
pub type GastronomeResult<T> = Result<T, GastronomeError>;

/// Round a value to two decimal places (currency).
pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Round a value to one decimal place (nutrition grams / calories).
pub fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Format minutes-from-midnight as `HH:MM` (24-hour). Values >= 24h wrap by
/// day but keep the raw hour so an early-morning prep start reads naturally.
pub fn fmt_hhmm(minutes: u32) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    format!("{h:02}:{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nutrition_scaled_and_add() {
        let n = NutritionFacts::new(100.0, 10.0, 20.0, 5.0);
        let doubled = n.scaled(2.0);
        assert_eq!(doubled.calories, 200.0);
        let sum = n + doubled;
        assert_eq!(sum.calories, 300.0);
        assert_eq!(sum.protein_g, 30.0);
    }

    #[test]
    fn nutrition_sum_iterator() {
        let facts = vec![
            NutritionFacts::new(10.0, 1.0, 2.0, 0.5),
            NutritionFacts::new(20.0, 2.0, 4.0, 1.0),
        ];
        let total: NutritionFacts = facts.into_iter().sum();
        assert_eq!(total.calories, 30.0);
        assert_eq!(total.fat_g, 1.5);
    }

    #[test]
    fn rounding_helpers() {
        assert_eq!(round2(1.239), 1.24);
        assert_eq!(round1(2.34), 2.3);
    }

    #[test]
    fn fmt_hhmm_pads() {
        assert_eq!(fmt_hhmm(9 * 60 + 5), "09:05");
        assert_eq!(fmt_hhmm(18 * 60), "18:00");
    }

    #[test]
    fn dietary_tag_display_all_variants() {
        for tag in DietaryTag::ALL {
            assert!(!tag.to_string().is_empty());
        }
    }

    #[test]
    fn course_and_stage_display() {
        assert_eq!(Course::Main.to_string(), "main");
        assert_eq!(Stage::Prep.to_string(), "prep");
        assert_eq!(Stage::ORDERED.len(), 3);
    }

    #[test]
    fn error_display_is_descriptive() {
        let e = GastronomeError::DietaryConflict {
            recipe: "beef wellington".into(),
            tag: DietaryTag::Vegan,
        };
        assert!(e.to_string().contains("vegan"));
        assert!(
            GastronomeError::UnknownRecipe("x".into())
                .to_string()
                .contains("unknown recipe")
        );
    }

    #[test]
    fn ingredient_serde_roundtrip() {
        let ing = Ingredient::new(
            "flour",
            "All-purpose flour",
            "g",
            0.002,
            NutritionFacts::new(3.64, 0.1, 0.76, 0.01),
            [DietaryTag::Vegan, DietaryTag::DairyFree],
        );
        let json = serde_json::to_string(&ing).unwrap();
        let back: Ingredient = serde_json::from_str(&json).unwrap();
        assert_eq!(ing, back);
    }
}
