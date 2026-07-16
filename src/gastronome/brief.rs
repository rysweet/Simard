//! Menu / event-brief data model.
//!
//! A [`MenuBrief`] is the untrusted input to the Gastronome pipeline: a
//! description of an event and the dishes to serve. It is parsed from JSON,
//! validated for culinary sanity, and then drives recipe scaling, nutrition &
//! cost analysis, and prep scheduling.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{GastronomeError, GastronomeResult};

/// Default servings of a dish per guest when the brief does not specify one.
pub const DEFAULT_SERVINGS_PER_GUEST: f64 = 1.0;

/// Upper bound on guest count accepted from an untrusted brief. Guards against
/// resource exhaustion and overflow when scaling recipes. No single event
/// sensibly serves more than this.
pub const MAX_GUESTS: u32 = 100_000;

/// Upper bound on the number of dishes in a single brief.
pub const MAX_DISHES: usize = 512;

/// Upper bound on ingredients per dish.
pub const MAX_INGREDIENTS_PER_DISH: usize = 512;

/// Upper bound on prep steps per dish.
pub const MAX_PREP_STEPS_PER_DISH: usize = 512;

/// Upper bound on a single prep step duration (minutes). A step longer than a
/// week of continuous work is certainly a malformed brief.
pub const MAX_PREP_MINUTES: f64 = 10_080.0;

/// Upper bound on servings-per-guest (a hostile brief could otherwise drive
/// unbounded ingredient totals).
pub const MAX_SERVINGS_PER_GUEST: f64 = 1_000.0;

/// Upper bound on an ingredient's per-serving quantity. Bounds the scaled
/// totals so a hostile brief cannot drive them to non-finite values (the
/// `MAX_GUESTS`/`MAX_SERVINGS_PER_GUEST` caps only bound the multiplier).
pub const MAX_QTY_PER_SERVING: f64 = 1_000_000.0;

/// Upper bound on an ingredient's cost per unit, for the same reason.
pub const MAX_COST_PER_UNIT: f64 = 1_000_000.0;

/// Upper bound on any single per-unit nutrition value, for the same reason.
pub const MAX_NUTRITION_VALUE: f64 = 1_000_000.0;

/// An event menu to be planned, parsed from a brief JSON document.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MenuBrief {
    /// Human-readable event name.
    pub event: String,
    /// Number of guests to cater for.
    pub guests: u32,
    /// Currency label for cost roll-ups (defaults to "USD").
    #[serde(default)]
    pub currency: Option<String>,
    /// Clock time the meal is served, `"HH:MM"` (24h). Enables clock times on
    /// the prep schedule; when absent the schedule uses relative offsets only.
    #[serde(default)]
    pub service_time: Option<String>,
    /// Event-wide dietary constraints (e.g. `"vegetarian"`, `"nut-free"`).
    #[serde(default)]
    pub dietary: Vec<String>,
    /// The dishes that make up the menu.
    pub dishes: Vec<Dish>,
    /// Optional total budget in the brief's currency.
    #[serde(default)]
    pub budget: Option<f64>,
}

/// A single dish on the menu.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Dish {
    /// Dish name as it appears on the menu card.
    pub name: String,
    /// Course family. Unknown values fall back to [`Course::Other`].
    pub course: String,
    /// Servings of this dish per guest (defaults to
    /// [`DEFAULT_SERVINGS_PER_GUEST`]).
    #[serde(default)]
    pub servings_per_guest: Option<f64>,
    /// Recipe ingredients, quantified per single serving.
    pub ingredients: Vec<Ingredient>,
    /// Prep tasks required to produce the dish.
    #[serde(default)]
    pub prep: Vec<PrepStep>,
    /// Dietary tags this dish satisfies (e.g. `"vegetarian"`, `"gluten-free"`).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A recipe ingredient, quantified per single serving of its dish.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ingredient {
    pub name: String,
    /// Quantity of this ingredient for a single serving, in `unit`.
    pub qty_per_serving: f64,
    /// Unit of measure (e.g. `"g"`, `"ml"`, `"each"`).
    pub unit: String,
    /// Cost of one `unit` of this ingredient, in the brief currency.
    #[serde(default)]
    pub cost_per_unit: Option<f64>,
    /// Nutrition contributed by one `unit` of this ingredient.
    #[serde(default)]
    pub nutrition: Option<Nutrition>,
}

/// Nutrition facts for one unit of an ingredient (or a rolled-up total).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
pub struct Nutrition {
    #[serde(default)]
    pub kcal: f64,
    #[serde(default)]
    pub protein_g: f64,
    #[serde(default)]
    pub carbs_g: f64,
    #[serde(default)]
    pub fat_g: f64,
}

impl Nutrition {
    /// Scale every field by `factor`.
    pub fn scaled(&self, factor: f64) -> Nutrition {
        Nutrition {
            kcal: self.kcal * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }

    /// Accumulate another nutrition record into this one.
    pub fn add(&mut self, other: &Nutrition) {
        self.kcal += other.kcal;
        self.protein_g += other.protein_g;
        self.carbs_g += other.carbs_g;
        self.fat_g += other.fat_g;
    }

    /// True when any field is negative, non-finite, or exceeds
    /// [`MAX_NUTRITION_VALUE`] — i.e. not a usable per-unit nutrition value.
    fn is_invalid(&self) -> bool {
        [self.kcal, self.protein_g, self.carbs_g, self.fat_g]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > MAX_NUTRITION_VALUE)
    }
}

/// A single prep task for a dish.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PrepStep {
    /// What to do (e.g. `"Braise short ribs"`).
    pub task: String,
    /// Estimated duration in minutes.
    pub minutes: f64,
    /// Kitchen station the task occupies (e.g. `"oven"`, `"stove"`, `"cold"`).
    #[serde(default)]
    pub station: Option<String>,
}

impl Dish {
    /// Effective servings-per-guest, applying the default.
    pub fn servings_per_guest(&self) -> f64 {
        self.servings_per_guest
            .unwrap_or(DEFAULT_SERVINGS_PER_GUEST)
    }

    /// Normalised course used to group the dish on the menu.
    pub fn normalized_course(&self) -> Course {
        Course::classify(&self.course)
    }
}

impl MenuBrief {
    /// Currency label, applying the default.
    pub fn currency(&self) -> &str {
        self.currency.as_deref().unwrap_or("USD")
    }

    /// Read and validate a brief from a JSON file.
    pub fn from_path(path: &Path) -> GastronomeResult<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| GastronomeError::io(format!("reading brief {}", path.display()), e))?;
        Self::from_json_bytes(&bytes)
    }

    /// Parse and validate a brief from JSON bytes.
    pub fn from_json_bytes(bytes: &[u8]) -> GastronomeResult<Self> {
        let brief: MenuBrief = serde_json::from_slice(bytes)
            .map_err(|e| GastronomeError::parse("brief json", e.to_string()))?;
        brief.validate()?;
        Ok(brief)
    }

    /// Reject semantically impossible or malformed briefs.
    pub fn validate(&self) -> GastronomeResult<()> {
        if self.event.trim().is_empty() {
            return Err(GastronomeError::invalid_brief("event must not be empty"));
        }
        if self.guests == 0 {
            return Err(GastronomeError::invalid_brief("guests must be at least 1"));
        }
        if self.guests > MAX_GUESTS {
            return Err(GastronomeError::invalid_brief(format!(
                "guests must not exceed {MAX_GUESTS} (got {})",
                self.guests
            )));
        }
        if let Some(t) = &self.service_time
            && parse_hhmm(t).is_none()
        {
            return Err(GastronomeError::invalid_brief(format!(
                "service_time must be 'HH:MM' 24h (got {t:?})"
            )));
        }
        if let Some(budget) = self.budget
            && (!budget.is_finite() || budget < 0.0)
        {
            return Err(GastronomeError::invalid_brief(
                "budget must be non-negative",
            ));
        }
        if self.dishes.is_empty() {
            return Err(GastronomeError::invalid_brief(
                "menu must contain at least one dish",
            ));
        }
        if self.dishes.len() > MAX_DISHES {
            return Err(GastronomeError::invalid_brief(format!(
                "menu must not exceed {MAX_DISHES} dishes (got {})",
                self.dishes.len()
            )));
        }
        for dish in &self.dishes {
            dish.validate()?;
        }
        Ok(())
    }
}

impl Dish {
    fn validate(&self) -> GastronomeResult<()> {
        if self.name.trim().is_empty() {
            return Err(GastronomeError::invalid_brief(
                "dish name must not be empty",
            ));
        }
        let spg = self.servings_per_guest();
        if !spg.is_finite() || spg <= 0.0 {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{}' servings_per_guest must be positive and finite (got {spg})",
                self.name
            )));
        }
        if spg > MAX_SERVINGS_PER_GUEST {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{}' servings_per_guest must not exceed {MAX_SERVINGS_PER_GUEST} (got {spg})",
                self.name
            )));
        }
        if self.ingredients.is_empty() {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{}' must have at least one ingredient",
                self.name
            )));
        }
        if self.ingredients.len() > MAX_INGREDIENTS_PER_DISH {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{}' must not exceed {MAX_INGREDIENTS_PER_DISH} ingredients",
                self.name
            )));
        }
        if self.prep.len() > MAX_PREP_STEPS_PER_DISH {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{}' must not exceed {MAX_PREP_STEPS_PER_DISH} prep steps",
                self.name
            )));
        }
        for ing in &self.ingredients {
            ing.validate(&self.name)?;
        }
        for step in &self.prep {
            step.validate(&self.name)?;
        }
        Ok(())
    }
}

impl Ingredient {
    fn validate(&self, dish: &str) -> GastronomeResult<()> {
        if self.name.trim().is_empty() {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{dish}' has an ingredient with an empty name"
            )));
        }
        if self.unit.trim().is_empty() {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' must have a unit",
                self.name
            )));
        }
        if !self.qty_per_serving.is_finite() || self.qty_per_serving <= 0.0 {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' qty_per_serving must be positive and finite (got {})",
                self.name, self.qty_per_serving
            )));
        }
        if self.qty_per_serving > MAX_QTY_PER_SERVING {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' qty_per_serving must not exceed {MAX_QTY_PER_SERVING} (got {})",
                self.name, self.qty_per_serving
            )));
        }
        if let Some(cost) = self.cost_per_unit
            && (!cost.is_finite() || cost < 0.0)
        {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' cost_per_unit must be non-negative",
                self.name
            )));
        }
        if let Some(cost) = self.cost_per_unit
            && cost > MAX_COST_PER_UNIT
        {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' cost_per_unit must not exceed {MAX_COST_PER_UNIT} (got {cost})",
                self.name
            )));
        }
        if let Some(n) = &self.nutrition
            && n.is_invalid()
        {
            return Err(GastronomeError::invalid_brief(format!(
                "ingredient '{}' in dish '{dish}' has negative, non-finite, or out-of-range nutrition (each value must be within [0, {MAX_NUTRITION_VALUE}])",
                self.name
            )));
        }
        Ok(())
    }
}

impl PrepStep {
    fn validate(&self, dish: &str) -> GastronomeResult<()> {
        if self.task.trim().is_empty() {
            return Err(GastronomeError::invalid_brief(format!(
                "dish '{dish}' has a prep step with an empty task"
            )));
        }
        if !self.minutes.is_finite() || self.minutes <= 0.0 {
            return Err(GastronomeError::invalid_brief(format!(
                "prep step '{}' in dish '{dish}' minutes must be positive and finite (got {})",
                self.task, self.minutes
            )));
        }
        if self.minutes > MAX_PREP_MINUTES {
            return Err(GastronomeError::invalid_brief(format!(
                "prep step '{}' in dish '{dish}' minutes must not exceed {MAX_PREP_MINUTES}",
                self.task
            )));
        }
        Ok(())
    }
}

/// Supported course families. Unknown courses map to [`Course::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Course {
    Starter,
    Main,
    Side,
    Dessert,
    Drink,
    Other,
}

impl Course {
    /// Map a free-form course string to a course family.
    pub fn classify(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "starter" | "appetizer" | "appetiser" | "entree" | "hors-doeuvre" | "amuse-bouche"
            | "soup" | "salad" | "first" => Self::Starter,
            "main" | "entrée" | "mains" | "main-course" | "plat" => Self::Main,
            "side" | "sides" | "accompaniment" | "vegetable" => Self::Side,
            "dessert" | "pudding" | "sweet" => Self::Dessert,
            "drink" | "beverage" | "cocktail" | "wine" => Self::Drink,
            _ => Self::Other,
        }
    }

    /// Stable label for manifests and menu cards.
    pub fn label(self) -> &'static str {
        match self {
            Self::Starter => "starter",
            Self::Main => "main",
            Self::Side => "side",
            Self::Dessert => "dessert",
            Self::Drink => "drink",
            Self::Other => "other",
        }
    }

    /// Course order for menu presentation (drinks last).
    pub fn all_in_menu_order() -> [Course; 6] {
        [
            Self::Starter,
            Self::Main,
            Self::Side,
            Self::Dessert,
            Self::Drink,
            Self::Other,
        ]
    }
}

/// Parse a `"HH:MM"` 24-hour clock string into minutes-since-midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = m.parse().ok()?;
    if h >= 24 || m >= 60 {
        return None;
    }
    Some(h * 60 + m)
}

/// Format minutes-since-midnight as `"HH:MM"`, wrapping within a 24h day.
pub fn format_hhmm(total_minutes: i64) -> String {
    let day = 24 * 60;
    let mut t = total_minutes % day;
    if t < 0 {
        t += day;
    }
    format!("{:02}:{:02}", t / 60, t % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "event": "Autumn tasting dinner",
            "guests": 20,
            "currency": "USD",
            "service_time": "19:00",
            "dietary": ["vegetarian"],
            "dishes": [
                {
                    "name": "Roasted squash soup",
                    "course": "starter",
                    "ingredients": [
                        { "name": "Butternut squash", "qty_per_serving": 200, "unit": "g",
                          "cost_per_unit": 0.004,
                          "nutrition": { "kcal": 0.45, "protein_g": 0.01, "carbs_g": 0.12, "fat_g": 0.001 } },
                        { "name": "Cream", "qty_per_serving": 30, "unit": "ml", "cost_per_unit": 0.006 }
                    ],
                    "prep": [ { "task": "Roast squash", "minutes": 40, "station": "oven" } ],
                    "tags": ["vegetarian", "gluten-free"]
                }
            ],
            "budget": 300.0
        }"#
    }

    #[test]
    fn parses_and_validates_sample() {
        let brief = MenuBrief::from_json_bytes(sample_json().as_bytes()).unwrap();
        assert_eq!(brief.event, "Autumn tasting dinner");
        assert_eq!(brief.guests, 20);
        assert_eq!(brief.currency(), "USD");
        assert_eq!(brief.dishes.len(), 1);
        assert_eq!(brief.dishes[0].normalized_course(), Course::Starter);
        assert_eq!(brief.dishes[0].servings_per_guest(), 1.0);
    }

    #[test]
    fn rejects_zero_guests() {
        let json = r#"{"event":"x","guests":0,"dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_guest_count() {
        let json = r#"{"event":"x","guests":99999999999,"dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#;
        // guests is u32; 1e11 exceeds u32::MAX so serde rejects it as a parse error.
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::Parse { .. }));
    }

    #[test]
    fn rejects_guests_over_domain_cap() {
        let json = format!(
            r#"{{"event":"x","guests":{},"dishes":[{{"name":"d","course":"main",
            "ingredients":[{{"name":"i","qty_per_serving":1,"unit":"g"}}]}}]}}"#,
            MAX_GUESTS + 1
        );
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_empty_menu() {
        let json = r#"{"event":"x","guests":4,"dishes":[]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_dish_without_ingredients() {
        let json =
            r#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main","ingredients":[]}]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_negative_ingredient_qty() {
        let json = r#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":-1,"unit":"g"}]}]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_ingredient_qty() {
        // A huge but finite quantity would scale to a non-finite total without
        // this cap, silently corrupting the shopping list and manifest.
        let json = format!(
            r#"{{"event":"x","guests":4,"dishes":[{{"name":"d","course":"main",
            "ingredients":[{{"name":"i","qty_per_serving":{},"unit":"g"}}]}}]}}"#,
            MAX_QTY_PER_SERVING * 10.0
        );
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_ingredient_cost() {
        let json = format!(
            r#"{{"event":"x","guests":4,"dishes":[{{"name":"d","course":"main",
            "ingredients":[{{"name":"i","qty_per_serving":1,"unit":"g","cost_per_unit":{}}}]}}]}}"#,
            MAX_COST_PER_UNIT * 10.0
        );
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_absurd_nutrition_value() {
        let json = format!(
            r#"{{"event":"x","guests":4,"dishes":[{{"name":"d","course":"main",
            "ingredients":[{{"name":"i","qty_per_serving":1,"unit":"g",
            "nutrition":{{"kcal":{}}}}}]}}]}}"#,
            MAX_NUTRITION_VALUE * 10.0
        );
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_bad_service_time() {
        let json = r#"{"event":"x","guests":4,"service_time":"25:00","dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn rejects_negative_prep_minutes() {
        let json = r#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
            "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}],
            "prep":[{"task":"chop","minutes":-5}]}]}"#;
        let err = MenuBrief::from_json_bytes(json.as_bytes()).unwrap_err();
        assert!(matches!(err, GastronomeError::InvalidBrief { .. }));
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = MenuBrief::from_json_bytes(b"{not json").unwrap_err();
        assert!(matches!(err, GastronomeError::Parse { .. }));
    }

    #[test]
    fn course_mapping_is_stable() {
        assert_eq!(Course::classify("Appetizer"), Course::Starter);
        assert_eq!(Course::classify("MAIN"), Course::Main);
        assert_eq!(Course::classify("pudding"), Course::Dessert);
        assert_eq!(Course::classify("cocktail"), Course::Drink);
        assert_eq!(Course::classify("mystery"), Course::Other);
        assert_eq!(Course::Main.label(), "main");
    }

    #[test]
    fn hhmm_roundtrips() {
        assert_eq!(parse_hhmm("19:30"), Some(19 * 60 + 30));
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("nope"), None);
        assert_eq!(format_hhmm(19 * 60 + 30), "19:30");
        assert_eq!(format_hhmm(-30), "23:30");
    }

    #[test]
    fn nutrition_scales_and_accumulates() {
        let n = Nutrition {
            kcal: 100.0,
            protein_g: 10.0,
            carbs_g: 20.0,
            fat_g: 5.0,
        };
        let s = n.scaled(2.0);
        assert_eq!(s.kcal, 200.0);
        let mut acc = Nutrition::default();
        acc.add(&n);
        acc.add(&n);
        assert_eq!(acc.protein_g, 20.0);
    }
}
