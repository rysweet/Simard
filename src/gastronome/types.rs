//! Core domain types for the Gastronome identity: ingredients, recipes, the
//! dietary vocabulary, and the event brief that drives planning.
//!
//! Everything here is plain, serde-friendly data so a recipe book and an event
//! brief can round-trip through JSON (the CLI's on-disk format) while the
//! planning code ([`crate::gastronome::plan`]) works against typed values.

use serde::{Deserialize, Serialize};

use super::error::{GastronomeError, GastronomeResult};
use super::nutrition::Nutrition;

/// A dietary attribute a dish can carry or a brief can require.
///
/// A recipe *carries* the tags that describe it (a chickpea curry carries
/// [`DietaryTag::Vegan`], [`DietaryTag::GlutenFree`], ...). A brief *requires*
/// tags as constraints; a recipe satisfies a constraint iff it carries the
/// required tag ([`Recipe::satisfies`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DietaryTag {
    /// Contains no meat, poultry, or fish.
    Vegetarian,
    /// Contains no animal products at all.
    Vegan,
    /// Contains no gluten.
    GlutenFree,
    /// Contains no dairy.
    DairyFree,
    /// Contains no nuts.
    NutFree,
    /// Fish/seafood permitted but no other meat.
    Pescatarian,
    /// Prepared to halal requirements.
    Halal,
    /// Prepared to kosher requirements.
    Kosher,
}

impl std::fmt::Display for DietaryTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Vegetarian => "vegetarian",
            Self::Vegan => "vegan",
            Self::GlutenFree => "gluten-free",
            Self::DairyFree => "dairy-free",
            Self::NutFree => "nut-free",
            Self::Pescatarian => "pescatarian",
            Self::Halal => "halal",
            Self::Kosher => "kosher",
        };
        f.write_str(label)
    }
}

/// A purchasable ingredient priced per unit, with the nutrition delivered by
/// one unit. `unit` is free-form (e.g. `"kg"`, `"litre"`, `"each"`); costs and
/// nutrition are always expressed *per one of that unit*.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ingredient {
    /// Display name, also the shopping-list aggregation key together with `unit`.
    pub name: String,
    /// The unit `cost_per_unit` and `nutrition_per_unit` are quoted in.
    pub unit: String,
    /// Purchase cost of one `unit`, in the brief's currency (minor units are
    /// the caller's choice; the planner is currency-agnostic).
    pub cost_per_unit: f64,
    /// Nutrition delivered by one `unit`.
    #[serde(default)]
    pub nutrition_per_unit: Nutrition,
}

/// One ingredient line inside a recipe: how much of an [`Ingredient`] a single
/// base serving needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeIngredient {
    /// The ingredient used.
    pub ingredient: Ingredient,
    /// Quantity of `ingredient.unit` required per base serving.
    pub quantity: f64,
}

impl RecipeIngredient {
    /// Cost of this line for a single base serving.
    #[must_use]
    pub fn cost(&self) -> f64 {
        self.quantity * self.ingredient.cost_per_unit
    }

    /// Nutrition contributed by this line for a single base serving.
    #[must_use]
    pub fn nutrition(&self) -> Nutrition {
        self.ingredient.nutrition_per_unit.scale(self.quantity)
    }
}

/// A single prep step. `minutes` is wall-clock duration; `active` distinguishes
/// hands-on work (chopping) from passive/oven/rest time (marinating, baking),
/// which the scheduler reports but treats identically for back-scheduling.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrepStep {
    /// What the cook does.
    pub description: String,
    /// How long the step takes, in minutes.
    pub minutes: u32,
    /// Whether the step needs a cook's hands (`true`) or is passive (`false`).
    #[serde(default = "default_active")]
    pub active: bool,
}

fn default_active() -> bool {
    true
}

/// A recipe: a named dish for a course, priced and portioned per
/// `base_servings`, with an ordered prep procedure and the dietary tags it
/// carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Stable identifier used to pin a recipe from a brief.
    pub id: String,
    /// Human-readable dish name.
    pub name: String,
    /// The course this dish is served as (e.g. `"starter"`, `"main"`).
    pub course: String,
    /// How many servings the ingredient quantities and steps are written for.
    pub base_servings: u32,
    /// Dietary attributes this dish carries.
    #[serde(default)]
    pub dietary_tags: Vec<DietaryTag>,
    /// Ingredient lines, each quoted per base serving.
    pub ingredients: Vec<RecipeIngredient>,
    /// Ordered prep steps (index 0 runs first).
    #[serde(default)]
    pub steps: Vec<PrepStep>,
}

impl Recipe {
    /// Validate the invariants scaling and scheduling rely on.
    ///
    /// # Errors
    /// Returns [`GastronomeError::InvalidBaseServings`] when `base_servings`
    /// is zero.
    pub fn validate(&self) -> GastronomeResult<()> {
        if self.base_servings == 0 {
            return Err(GastronomeError::InvalidBaseServings {
                recipe: self.id.clone(),
                base_servings: 0,
            });
        }
        Ok(())
    }

    /// Cost of one serving of this dish.
    #[must_use]
    pub fn cost_per_serving(&self) -> f64 {
        self.ingredients.iter().map(RecipeIngredient::cost).sum()
    }

    /// Nutrition of one serving of this dish.
    #[must_use]
    pub fn nutrition_per_serving(&self) -> Nutrition {
        self.ingredients
            .iter()
            .map(RecipeIngredient::nutrition)
            .fold(Nutrition::default(), Nutrition::sum2)
    }

    /// Total hands-off + hands-on prep time for one batch, in minutes.
    #[must_use]
    pub fn total_prep_minutes(&self) -> u32 {
        self.steps.iter().map(|s| s.minutes).sum()
    }

    /// Whether this dish carries every required tag.
    #[must_use]
    pub fn satisfies(&self, required: &[DietaryTag]) -> bool {
        required.iter().all(|tag| self.dietary_tags.contains(tag))
    }
}

/// A request for one course of the menu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CourseRequest {
    /// The course to fill (matched against [`Recipe::course`]).
    pub course: String,
    /// Extra dietary constraints for this course only, on top of the brief-wide
    /// [`EventBrief::dietary_constraints`].
    #[serde(default)]
    pub dietary: Vec<DietaryTag>,
    /// Pin a specific recipe by id instead of auto-selecting.
    #[serde(default)]
    pub recipe_id: Option<String>,
}

impl CourseRequest {
    /// Construct a course request for `course` with no extra constraints.
    #[must_use]
    pub fn new(course: impl Into<String>) -> Self {
        Self {
            course: course.into(),
            dietary: Vec::new(),
            recipe_id: None,
        }
    }
}

/// The full brief a client hands the Gastronome: the event, its size, when food
/// hits the table, the courses wanted, and any diet-wide constraints.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventBrief {
    /// Event name (for the plan header).
    pub name: String,
    /// Number of guests to cater for.
    pub guests: u32,
    /// When the food is served, `HH:MM` on a 24-hour clock.
    pub serve_time: String,
    /// Courses requested, in serving order.
    pub courses: Vec<CourseRequest>,
    /// Dietary constraints that apply to every course.
    #[serde(default)]
    pub dietary_constraints: Vec<DietaryTag>,
}

impl EventBrief {
    /// Validate the guest count the planner divides and scales by.
    ///
    /// # Errors
    /// Returns [`GastronomeError::InvalidGuestCount`] when `guests` is zero.
    pub fn validate(&self) -> GastronomeResult<()> {
        if self.guests == 0 {
            return Err(GastronomeError::InvalidGuestCount { guests: 0 });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flour() -> Ingredient {
        Ingredient {
            name: "flour".into(),
            unit: "kg".into(),
            cost_per_unit: 2.0,
            nutrition_per_unit: Nutrition::new(3640.0, 100.0, 760.0, 10.0),
        }
    }

    fn bread() -> Recipe {
        Recipe {
            id: "bread".into(),
            name: "Crusty Loaf".into(),
            course: "side".into(),
            base_servings: 4,
            dietary_tags: vec![DietaryTag::Vegan, DietaryTag::NutFree],
            ingredients: vec![RecipeIngredient {
                ingredient: flour(),
                quantity: 0.125, // 125 g per serving
            }],
            steps: vec![
                PrepStep {
                    description: "mix and knead".into(),
                    minutes: 15,
                    active: true,
                },
                PrepStep {
                    description: "prove".into(),
                    minutes: 60,
                    active: false,
                },
                PrepStep {
                    description: "bake".into(),
                    minutes: 30,
                    active: false,
                },
            ],
        }
    }

    #[test]
    fn recipe_cost_per_serving_sums_lines() {
        assert!((bread().cost_per_serving() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn recipe_nutrition_per_serving_scales_by_quantity() {
        let n = bread().nutrition_per_serving();
        assert!((n.calories - 455.0).abs() < 1e-6);
        assert!((n.carbs_g - 95.0).abs() < 1e-6);
    }

    #[test]
    fn total_prep_minutes_sums_steps() {
        assert_eq!(bread().total_prep_minutes(), 105);
    }

    #[test]
    fn satisfies_requires_all_tags() {
        let r = bread();
        assert!(r.satisfies(&[DietaryTag::Vegan]));
        assert!(r.satisfies(&[DietaryTag::Vegan, DietaryTag::NutFree]));
        assert!(!r.satisfies(&[DietaryTag::GlutenFree]));
    }

    #[test]
    fn satisfies_empty_constraint_is_true() {
        assert!(bread().satisfies(&[]));
    }

    #[test]
    fn recipe_validate_rejects_zero_servings() {
        let mut r = bread();
        r.base_servings = 0;
        assert_eq!(
            r.validate(),
            Err(GastronomeError::InvalidBaseServings {
                recipe: "bread".into(),
                base_servings: 0
            })
        );
    }

    #[test]
    fn brief_validate_rejects_zero_guests() {
        let brief = EventBrief {
            name: "party".into(),
            guests: 0,
            serve_time: "18:00".into(),
            courses: vec![CourseRequest::new("main")],
            dietary_constraints: vec![],
        };
        assert_eq!(
            brief.validate(),
            Err(GastronomeError::InvalidGuestCount { guests: 0 })
        );
    }

    #[test]
    fn dietary_tag_roundtrips_kebab_case() {
        let json = serde_json::to_string(&DietaryTag::GlutenFree).unwrap();
        assert_eq!(json, "\"gluten-free\"");
        let back: DietaryTag = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DietaryTag::GlutenFree);
    }
}
