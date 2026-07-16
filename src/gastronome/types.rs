//! Core, serde-friendly data model for the Gastronome culinary planner.
//!
//! Gastronome is a pluggable Simard identity that designs recipes, menus, and
//! catering/event plans. This module holds the deterministic, dependency-free
//! domain types the planner operates on: measurement units, priced/nutrition-
//! tagged [`Ingredient`]s, [`Recipe`]s, [`Menu`] courses, and the [`EventBrief`]
//! that drives an end-to-end costed + scheduled [`crate::gastronome::planner::MenuPlan`].
//!
//! Everything here is offline and pure: no LLM, no I/O, no clock. That keeps the
//! whole pipeline exercisable in unit tests and reproducible in CI.

use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

/// Errors produced by the Gastronome domain and CLI.
#[derive(Debug)]
pub enum GastronomeError {
    /// A recipe referenced an ingredient id that is not in the kitchen book.
    UnknownIngredient {
        /// The recipe that referenced the missing ingredient.
        recipe: String,
        /// The missing ingredient id.
        ingredient: String,
    },
    /// A brief/menu referenced a recipe id that is not in the kitchen book.
    UnknownRecipe {
        /// The missing recipe id.
        recipe: String,
    },
    /// A recipe line used a unit whose family does not match the ingredient's
    /// base unit family (e.g. millilitres for a gram-priced ingredient).
    UnitMismatch {
        /// The recipe containing the offending line.
        recipe: String,
        /// The ingredient id.
        ingredient: String,
        /// The unit family the line asked for.
        line_family: &'static str,
        /// The ingredient's base unit family.
        base_family: &'static str,
    },
    /// A recipe declared a non-positive yield (`servings`), which would make
    /// scaling and per-serving analysis undefined.
    InvalidServings {
        /// The offending recipe id.
        recipe: String,
        /// The offending servings value.
        servings: f64,
    },
    /// The prep-dependency graph contains a cycle, so no schedule exists.
    ScheduleCycle {
        /// A recipe id participating in the cycle.
        recipe: String,
    },
    /// A `service_time` string could not be parsed as `HH:MM` (24-hour).
    InvalidTime {
        /// The raw value that failed to parse.
        value: String,
    },
    /// A CLI usage error. The payload is a ready-to-print message.
    Usage(String),
    /// An underlying I/O failure (reading a book/brief file).
    Io(String),
    /// A TOML parse failure while loading a book/brief.
    Parse(String),
    /// A JSON serialization failure while rendering a plan.
    Serialize(String),
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownIngredient { recipe, ingredient } => write!(
                f,
                "recipe '{recipe}' references unknown ingredient '{ingredient}'"
            ),
            Self::UnknownRecipe { recipe } => {
                write!(f, "menu/brief references unknown recipe '{recipe}'")
            }
            Self::UnitMismatch {
                recipe,
                ingredient,
                line_family,
                base_family,
            } => write!(
                f,
                "recipe '{recipe}' measures ingredient '{ingredient}' in {line_family} \
                 units but it is priced in {base_family} units"
            ),
            Self::InvalidServings { recipe, servings } => write!(
                f,
                "recipe '{recipe}' has a non-positive yield of {servings} servings"
            ),
            Self::ScheduleCycle { recipe } => write!(
                f,
                "prep dependency cycle detected involving recipe '{recipe}'"
            ),
            Self::InvalidTime { value } => {
                write!(
                    f,
                    "invalid service_time '{value}', expected HH:MM (24-hour)"
                )
            }
            Self::Usage(msg) => write!(f, "{msg}"),
            Self::Io(msg) => write!(f, "i/o error: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Serialize(msg) => write!(f, "serialize error: {msg}"),
        }
    }
}

impl std::error::Error for GastronomeError {}

/// Convenient result alias for the Gastronome module.
pub type GastronomeResult<T> = Result<T, GastronomeError>;

/// A measurement unit. Units belong to one of three [`UnitFamily`]s and convert
/// to a canonical base amount (grams / millilitres / each).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    /// Mass in grams (the mass base unit).
    Gram,
    /// Mass in kilograms (1000 g).
    Kilogram,
    /// Volume in millilitres (the volume base unit).
    Milliliter,
    /// Volume in litres (1000 ml).
    Liter,
    /// A whole countable item (the count base unit).
    Each,
}

/// The dimensional family a [`Unit`] belongs to. Recipe lines may only be
/// converted within a single family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitFamily {
    /// Mass (base: gram).
    Mass,
    /// Volume (base: millilitre).
    Volume,
    /// Count (base: each).
    Count,
}

impl UnitFamily {
    /// A stable, human-readable label used in error messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mass => "mass",
            Self::Volume => "volume",
            Self::Count => "count",
        }
    }
}

impl Unit {
    /// The dimensional family this unit belongs to.
    #[must_use]
    pub fn family(self) -> UnitFamily {
        match self {
            Self::Gram | Self::Kilogram => UnitFamily::Mass,
            Self::Milliliter | Self::Liter => UnitFamily::Volume,
            Self::Each => UnitFamily::Count,
        }
    }

    /// The multiplier that converts one of this unit into the family's base
    /// amount (grams, millilitres, or each).
    #[must_use]
    pub fn to_base_factor(self) -> f64 {
        match self {
            Self::Gram | Self::Milliliter | Self::Each => 1.0,
            Self::Kilogram | Self::Liter => 1000.0,
        }
    }

    /// The canonical base unit for this unit's family.
    #[must_use]
    pub fn base_unit(self) -> Unit {
        match self.family() {
            UnitFamily::Mass => Unit::Gram,
            UnitFamily::Volume => Unit::Milliliter,
            UnitFamily::Count => Unit::Each,
        }
    }

    /// Short label for display (`g`, `kg`, `ml`, `l`, `ea`).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Gram => "g",
            Self::Kilogram => "kg",
            Self::Milliliter => "ml",
            Self::Liter => "l",
            Self::Each => "ea",
        }
    }
}

impl Display for Unit {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Macronutrient facts expressed *per 100 base units* of an ingredient
/// (per 100 g, per 100 ml, or per 100 each). Aggregation scales linearly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Scale every macro by `factor` (used when converting per-100 facts to an
    /// arbitrary amount).
    #[must_use]
    pub fn scaled(self, factor: f64) -> Self {
        Self {
            calories: self.calories * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }
}

impl std::ops::Add for Nutrition {
    type Output = Self;

    /// Add two nutrition records component-wise.
    fn add(self, other: Self) -> Self {
        Self {
            calories: self.calories + other.calories,
            protein_g: self.protein_g + other.protein_g,
            carbs_g: self.carbs_g + other.carbs_g,
            fat_g: self.fat_g + other.fat_g,
        }
    }
}

/// A priced, nutrition-tagged pantry ingredient.
///
/// `price_per_base` and `nutrition` are both expressed against the ingredient's
/// base unit (grams for a `unit = "gram"` ingredient, etc.): `price_per_base`
/// is the cost of one base unit and `nutrition` is per 100 base units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    /// Stable identifier referenced by recipe lines.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The base unit this ingredient is priced and measured in.
    pub unit: Unit,
    /// Cost of a single base unit, in the plan's currency (e.g. dollars/gram).
    pub price_per_base: f64,
    /// Macronutrients per 100 base units. Optional; defaults to all-zero.
    #[serde(default)]
    pub nutrition: Nutrition,
    /// Optional dietary tags (e.g. `vegan`, `gluten-free`, `contains-nuts`).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One line of a recipe: an amount of an ingredient in some [`Unit`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeLine {
    /// The ingredient id (must exist in the kitchen book).
    pub ingredient: String,
    /// The quantity in `unit`.
    pub quantity: f64,
    /// The unit the quantity is expressed in.
    pub unit: Unit,
}

/// A recipe: a yield (`servings`), a set of ingredient lines, and prep/cook
/// durations plus optional prerequisites used by the prep scheduler.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Stable identifier referenced by menus and briefs.
    pub id: String,
    /// Human-readable dish name.
    pub name: String,
    /// Number of servings the ingredient list as written yields. Must be > 0.
    pub servings: f64,
    /// Hands-on prep minutes (mise en place, assembly).
    #[serde(default)]
    pub prep_minutes: u32,
    /// Unattended cook/rest minutes (oven, proof, chill).
    #[serde(default)]
    pub cook_minutes: u32,
    /// Ingredient lines.
    #[serde(default)]
    pub ingredients: Vec<RecipeLine>,
    /// Recipe ids that must be finished before this recipe *starts* (e.g. a
    /// poolish before the dough). Used only for scheduling.
    #[serde(default)]
    pub depends_on: Vec<String>,
}

impl Recipe {
    /// Total wall-clock minutes for one pass of this recipe (prep + cook).
    #[must_use]
    pub fn duration_minutes(&self) -> u32 {
        self.prep_minutes + self.cook_minutes
    }
}

/// A single course in an [`EventBrief`]: which recipe, and how many portions
/// each guest receives (e.g. `0.5` for a shared/small plate).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Course {
    /// The recipe id.
    pub recipe: String,
    /// Portions served per guest. Defaults to `1.0`.
    #[serde(default = "one")]
    pub portions_per_guest: f64,
}

fn one() -> f64 {
    1.0
}

/// The catering brief that drives an end-to-end plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBrief {
    /// Event name (for report headers).
    pub name: String,
    /// Number of guests to cater for. Must be >= 1 for a meaningful plan.
    pub guest_count: u32,
    /// Service time as `HH:MM` (24-hour). The prep schedule is anchored so all
    /// dishes are ready by this time.
    pub service_time: String,
    /// Optional per-guest budget; the plan reports whether it is met.
    #[serde(default)]
    pub budget_per_guest: Option<f64>,
    /// The courses on the menu.
    #[serde(default)]
    pub courses: Vec<Course>,
}

/// A parsed clock time (minutes since midnight), used by the scheduler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClockTime {
    minutes_since_midnight: i32,
}

impl ClockTime {
    /// Parse an `HH:MM` 24-hour string.
    ///
    /// # Errors
    /// Returns [`GastronomeError::InvalidTime`] if the value is not `HH:MM`
    /// with `HH` in `0..=23` and `MM` in `0..=59`.
    pub fn parse(value: &str) -> GastronomeResult<Self> {
        let invalid = || GastronomeError::InvalidTime {
            value: value.to_string(),
        };
        let (h, m) = value.split_once(':').ok_or_else(invalid)?;
        let hours: i32 = h.parse().map_err(|_| invalid())?;
        let mins: i32 = m.parse().map_err(|_| invalid())?;
        if !(0..=23).contains(&hours) || !(0..=59).contains(&mins) {
            return Err(invalid());
        }
        Ok(Self {
            minutes_since_midnight: hours * 60 + mins,
        })
    }

    /// Construct from raw minutes since midnight (may be negative for
    /// "the day before" when a schedule reaches back past 00:00).
    #[must_use]
    pub fn from_minutes(minutes_since_midnight: i32) -> Self {
        Self {
            minutes_since_midnight,
        }
    }

    /// Minutes since midnight (may be negative).
    #[must_use]
    pub fn minutes(self) -> i32 {
        self.minutes_since_midnight
    }

    /// Subtract `minutes`, returning a new [`ClockTime`].
    #[must_use]
    pub fn minus(self, minutes: i32) -> Self {
        Self {
            minutes_since_midnight: self.minutes_since_midnight - minutes,
        }
    }
}

impl Display for ClockTime {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Normalise into a 24-hour clock, prefixing "-1d " for times that fall
        // on the previous day (common when a long bake reaches before midnight).
        let mut day_offset = 0;
        let mut mins = self.minutes_since_midnight;
        while mins < 0 {
            mins += 24 * 60;
            day_offset -= 1;
        }
        let h = (mins / 60) % 24;
        let m = mins % 60;
        if day_offset == 0 {
            write!(f, "{h:02}:{m:02}")
        } else {
            write!(f, "{h:02}:{m:02} (-{}d)", -day_offset)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_families_and_base_factors() {
        assert_eq!(Unit::Kilogram.family(), UnitFamily::Mass);
        assert_eq!(Unit::Liter.family(), UnitFamily::Volume);
        assert_eq!(Unit::Each.family(), UnitFamily::Count);
        assert_eq!(Unit::Kilogram.to_base_factor(), 1000.0);
        assert_eq!(Unit::Gram.to_base_factor(), 1.0);
        assert_eq!(Unit::Liter.base_unit(), Unit::Milliliter);
    }

    #[test]
    fn nutrition_scaled_and_added() {
        let per100 = Nutrition {
            calories: 100.0,
            protein_g: 10.0,
            carbs_g: 5.0,
            fat_g: 2.0,
        };
        let half = per100.scaled(0.5);
        assert_eq!(half.calories, 50.0);
        let sum = half + per100;
        assert_eq!(sum.calories, 150.0);
        assert_eq!(sum.protein_g, 15.0);
    }

    #[test]
    fn recipe_duration_is_prep_plus_cook() {
        let r = Recipe {
            id: "r".into(),
            name: "R".into(),
            servings: 4.0,
            prep_minutes: 20,
            cook_minutes: 40,
            ingredients: vec![],
            depends_on: vec![],
        };
        assert_eq!(r.duration_minutes(), 60);
    }

    #[test]
    fn clock_time_parse_roundtrip() {
        let t = ClockTime::parse("18:30").unwrap();
        assert_eq!(t.minutes(), 18 * 60 + 30);
        assert_eq!(t.to_string(), "18:30");
    }

    #[test]
    fn clock_time_rejects_bad_values() {
        assert!(ClockTime::parse("24:00").is_err());
        assert!(ClockTime::parse("12:60").is_err());
        assert!(ClockTime::parse("noon").is_err());
        assert!(ClockTime::parse("1230").is_err());
    }

    #[test]
    fn clock_time_minus_wraps_to_previous_day() {
        let t = ClockTime::parse("00:30").unwrap().minus(60);
        assert_eq!(t.minutes(), -30);
        assert_eq!(t.to_string(), "23:30 (-1d)");
    }

    #[test]
    fn error_display_is_human_readable() {
        let e = GastronomeError::UnknownIngredient {
            recipe: "focaccia".into(),
            ingredient: "flour".into(),
        };
        assert!(e.to_string().contains("focaccia"));
        assert!(e.to_string().contains("flour"));
    }
}
