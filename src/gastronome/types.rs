//! Domain types for the Gastronome culinary planning engine.
//!
//! Everything here is a plain, `serde`-serialisable value object. The engine is
//! deterministic and dependency-light on purpose: a menu brief in, a costed and
//! scheduled plan out, with no I/O or clocks in the core.

use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// The unit a quantity of an ingredient is expressed in. Costs and nutrition on
/// an [`Ingredient`] are always *per one of this unit* so aggregation is a plain
/// multiply-and-sum.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    /// A gram of a solid ingredient.
    Gram,
    /// A millilitre of a liquid ingredient.
    Milliliter,
    /// A countable whole item (one egg, one lemon).
    Piece,
}

impl Display for Unit {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Gram => "g",
            Self::Milliliter => "ml",
            Self::Piece => "pc",
        };
        f.write_str(label)
    }
}

/// A common food allergen an ingredient may contain. Used for dietary
/// screening of a menu against an event brief.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Allergen {
    Gluten,
    Dairy,
    Egg,
    Fish,
    Shellfish,
    TreeNut,
    Peanut,
    Soy,
    Sesame,
}

impl Display for Allergen {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Gluten => "gluten",
            Self::Dairy => "dairy",
            Self::Egg => "egg",
            Self::Fish => "fish",
            Self::Shellfish => "shellfish",
            Self::TreeNut => "tree-nut",
            Self::Peanut => "peanut",
            Self::Soy => "soy",
            Self::Sesame => "sesame",
        };
        f.write_str(label)
    }
}

/// A dietary restriction a guest population may impose on the whole menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DietaryRestriction {
    /// No meat or fish.
    Vegetarian,
    /// No animal products at all.
    Vegan,
}

impl Display for DietaryRestriction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Vegetarian => "vegetarian",
            Self::Vegan => "vegan",
        };
        f.write_str(label)
    }
}

/// Macro-nutrition for a single unit of an ingredient (see [`Unit`]). Values are
/// absolute amounts, never percentages, so they sum linearly.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Nutrition {
    /// Energy in kilocalories.
    #[serde(default)]
    pub calories: f64,
    /// Protein in grams.
    #[serde(default)]
    pub protein_g: f64,
    /// Carbohydrate in grams.
    #[serde(default)]
    pub carbs_g: f64,
    /// Fat in grams.
    #[serde(default)]
    pub fat_g: f64,
}

impl Nutrition {
    /// Scale every macro by `factor` (used when a quantity is more than one unit).
    #[must_use]
    pub fn scaled(&self, factor: f64) -> Self {
        Self {
            calories: self.calories * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }

    /// Component-wise sum of two nutrition records.
    #[must_use]
    pub fn add(&self, other: &Self) -> Self {
        Self {
            calories: self.calories + other.calories,
            protein_g: self.protein_g + other.protein_g,
            carbs_g: self.carbs_g + other.carbs_g,
            fat_g: self.fat_g + other.fat_g,
        }
    }
}

/// A pantry ingredient: its unit basis, unit cost, per-unit nutrition, allergen
/// set, and whether it is vegetarian / vegan friendly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    /// Stable identifier referenced by recipes.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The unit that `cost_per_unit` and `nutrition_per_unit` are keyed to.
    pub unit: Unit,
    /// Cost of a single [`Unit`] of this ingredient, in the plan's currency.
    pub cost_per_unit: f64,
    /// Nutrition of a single [`Unit`] of this ingredient.
    #[serde(default)]
    pub nutrition_per_unit: Nutrition,
    /// Allergens this ingredient contains.
    #[serde(default)]
    pub allergens: BTreeSet<Allergen>,
    /// Whether the ingredient is acceptable in a vegetarian menu.
    #[serde(default = "default_true")]
    pub vegetarian: bool,
    /// Whether the ingredient is acceptable in a vegan menu.
    #[serde(default)]
    pub vegan: bool,
}

fn default_true() -> bool {
    true
}

/// A single prep step within a recipe. Steps form a per-recipe dependency DAG:
/// a step cannot start until every step in `depends_on` has finished.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepStep {
    /// Identifier unique within its recipe.
    pub id: String,
    /// What the cook does in this step.
    pub description: String,
    /// How long the step takes, in whole minutes.
    pub duration_minutes: u32,
    /// Ids of steps (in the same recipe) that must finish before this one starts.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

/// A recipe: the ingredients it needs to yield `servings` portions and the prep
/// steps to make it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Stable identifier referenced by menu items.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// How many portions the ingredient quantities below yield. Must be > 0.
    pub servings: u32,
    /// The ingredients and their quantities for this base yield.
    pub ingredients: Vec<RecipeIngredient>,
    /// The prep steps to make this recipe.
    #[serde(default)]
    pub steps: Vec<PrepStep>,
}

/// One ingredient line in a recipe: how much of a pantry ingredient it uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeIngredient {
    /// [`Ingredient::id`] this line refers to.
    pub ingredient_id: String,
    /// Quantity, in the ingredient's own [`Unit`], for the recipe's base yield.
    pub quantity: f64,
}

/// The course a menu item belongs to. Purely descriptive; it does not change
/// costing or scheduling.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
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
        let label = match self {
            Self::Appetizer => "appetizer",
            Self::Main => "main",
            Self::Side => "side",
            Self::Dessert => "dessert",
            Self::Beverage => "beverage",
        };
        f.write_str(label)
    }
}

/// A menu item: a recipe served as a particular course.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuItem {
    /// [`Recipe::id`] this item serves.
    pub recipe_id: String,
    /// Which course the item is served as.
    pub course: Course,
}

/// A menu: an ordered list of items.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Menu {
    /// Human-readable name of the menu.
    #[serde(default)]
    pub name: String,
    /// The items on the menu.
    pub items: Vec<MenuItem>,
}

/// The event/menu brief a Gastronome plans against.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBrief {
    /// Human-readable event name.
    #[serde(default)]
    pub name: String,
    /// Number of guests. Every guest is assumed to eat one serving of each item.
    pub guests: u32,
    /// Service time as whole minutes since midnight (e.g. 18:30 -> 1110).
    pub service_time_minutes: u32,
    /// Optional total budget for the whole event, in the plan's currency.
    #[serde(default)]
    pub budget_total: Option<f64>,
    /// Dietary restrictions the whole menu must satisfy.
    #[serde(default)]
    pub dietary_restrictions: BTreeSet<DietaryRestriction>,
    /// Allergens that must be absent from the menu.
    #[serde(default)]
    pub excluded_allergens: BTreeSet<Allergen>,
}

/// Format whole minutes-since-midnight as `HH:MM` (24-hour, wrapping at 1440).
#[must_use]
pub fn format_clock(minutes: u32) -> String {
    let wrapped = minutes % (24 * 60);
    format!("{:02}:{:02}", wrapped / 60, wrapped % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nutrition_scaled_and_add_are_linear() {
        let base = Nutrition {
            calories: 10.0,
            protein_g: 1.0,
            carbs_g: 2.0,
            fat_g: 0.5,
        };
        let doubled = base.scaled(2.0);
        assert!((doubled.calories - 20.0).abs() < 1e-9);
        assert!((doubled.protein_g - 2.0).abs() < 1e-9);
        let summed = base.add(&doubled);
        assert!((summed.calories - 30.0).abs() < 1e-9);
        assert!((summed.fat_g - 1.5).abs() < 1e-9);
    }

    #[test]
    fn nutrition_default_is_zero() {
        let n = Nutrition::default();
        assert!(n.calories.abs() < 1e-9);
        assert!(n.protein_g.abs() < 1e-9);
    }

    #[test]
    fn format_clock_pads_and_wraps() {
        assert_eq!(format_clock(0), "00:00");
        assert_eq!(format_clock(9 * 60 + 5), "09:05");
        assert_eq!(format_clock(18 * 60 + 30), "18:30");
        assert_eq!(format_clock(24 * 60 + 15), "00:15");
    }

    #[test]
    fn unit_and_allergen_display() {
        assert_eq!(Unit::Gram.to_string(), "g");
        assert_eq!(Unit::Milliliter.to_string(), "ml");
        assert_eq!(Unit::Piece.to_string(), "pc");
        assert_eq!(Allergen::TreeNut.to_string(), "tree-nut");
        assert_eq!(Course::Dessert.to_string(), "dessert");
        assert_eq!(DietaryRestriction::Vegan.to_string(), "vegan");
    }

    #[test]
    fn ingredient_roundtrips_through_serde_with_defaults() {
        let json = r#"{
            "id": "flour",
            "name": "All-purpose flour",
            "unit": "gram",
            "cost_per_unit": 0.002
        }"#;
        let ing: Ingredient = serde_json::from_str(json).unwrap();
        assert_eq!(ing.id, "flour");
        assert_eq!(ing.unit, Unit::Gram);
        assert!(ing.vegetarian, "vegetarian defaults to true");
        assert!(!ing.vegan, "vegan defaults to false");
        assert!(ing.allergens.is_empty());
        let back = serde_json::to_string(&ing).unwrap();
        let reparsed: Ingredient = serde_json::from_str(&back).unwrap();
        assert_eq!(ing, reparsed);
    }

    #[test]
    fn event_brief_parses_with_optional_fields_absent() {
        let json = r#"{"guests": 20, "service_time_minutes": 1110}"#;
        let brief: EventBrief = serde_json::from_str(json).unwrap();
        assert_eq!(brief.guests, 20);
        assert_eq!(brief.service_time_minutes, 1110);
        assert!(brief.budget_total.is_none());
        assert!(brief.dietary_restrictions.is_empty());
    }
}
