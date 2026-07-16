//! The **Gastronome** capability: design an event menu (recipes, courses, and
//! catering/event plan) and stand up the runnable kitchen app that operates it,
//! then prove it by taking the brief to a costed, scheduled menu plan
//! end-to-end.
//!
//! This module is the runnable core behind the `simard-gastronome` identity. It
//! has two halves:
//!
//! - [`design`] turns a (possibly untrusted, free-text) brief into a structured
//!   [`MenuConcept`](design::MenuConcept) covering the menu (courses/recipes),
//!   an event service flow, and a menu identity.
//! - [`kitchen`] is a small in-memory kitchen app (scaling, costed shopping
//!   list, nutrition analysis, cost analysis, and prep scheduling) that can be
//!   scaffolded straight from a concept.
//!
//! [`run_gastronome`] wires the two together: design → scaffold → a costed,
//! scaled, scheduled plan → invariant verification, returning a
//! [`GastronomeOutcome`] that is both machine-readable (serde) and renderable as
//! an operator report via [`render_report`].

pub mod design;
pub mod kitchen;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

pub use design::{
    CoursePlan, Ingredient, MenuBrief, MenuConcept, MenuIdentity, MenuPlan, PrepTask, RecipePlan,
    ServiceFlow, ServiceStage, ServiceStyle, Station, design_menu,
};
pub use kitchen::{
    BATCH_SIZE, CostAnalysis, KitchenEngine, NutritionAnalysis, PrepSchedule, PrepScheduledTask,
    ScaledRecipe, ShoppingLine,
};

/// Errors produced while designing or operating a menu concept.
///
/// Self-contained (not folded into `SimardError`) so the gastronome stays a
/// modular brick with its own contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GastronomeError {
    /// The brief could not be turned into a serviceable menu.
    InvalidBrief { reason: String },
    /// A scaling request referenced a recipe that is not on the menu.
    UnknownRecipe { code: String },
    /// The requested guest count is not valid.
    InvalidGuestCount { reason: String },
    /// The end-to-end run failed its own verification invariants.
    VerificationFailed { reason: String },
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBrief { reason } => write!(f, "invalid menu brief: {reason}"),
            Self::UnknownRecipe { code } => write!(f, "unknown recipe: {code}"),
            Self::InvalidGuestCount { reason } => write!(f, "invalid guest count: {reason}"),
            Self::VerificationFailed { reason } => {
                write!(f, "gastronome verification failed: {reason}")
            }
        }
    }
}

impl Error for GastronomeError {}

/// A single demonstrated scaled dish, captured for the outcome report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuSummary {
    pub code: String,
    pub name: String,
    pub course: String,
    pub servings: u32,
    pub per_serving_cost_cents: u32,
    pub total_cost_cents: u32,
    pub per_serving_calories: u32,
}

/// The full result of an end-to-end gastronome run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GastronomeOutcome {
    pub concept: MenuConcept,
    pub guest_count: u32,
    pub course_count: usize,
    pub dish_count: usize,
    pub total_cost_cents: u32,
    pub per_guest_cost_cents: u32,
    pub per_guest_calories: u32,
    pub total_calories: u32,
    pub shopping_line_count: usize,
    pub prep_total_minutes: u32,
    pub prep_batches: u32,
    pub within_budget: bool,
    pub sample_dish: MenuSummary,
    /// Whether every post-run invariant held.
    pub verified: bool,
    pub verification_notes: Vec<String>,
}

/// Design a menu from a free-text brief, scaffold its kitchen app, and take the
/// brief to a costed, scaled, scheduled plan end-to-end, verifying invariants
/// along the way.
///
/// # Errors
/// Propagates [`GastronomeError`] from design or scaling steps, and returns
/// [`GastronomeError::VerificationFailed`] if a post-run invariant is violated.
pub fn run_gastronome(brief: &MenuBrief) -> Result<GastronomeOutcome, GastronomeError> {
    let concept = design_menu(brief)?;
    let engine = KitchenEngine::from_concept(&concept);
    let guests = brief.guest_count;

    let course_count = concept.menu.course_count();
    let dish_count = concept.menu.dish_count();

    let cost = engine.cost_analysis(guests);
    let nutrition = engine.nutrition_analysis(guests);
    let schedule = engine.prep_schedule(guests);
    let shopping = engine.shopping_list(guests);
    let shopping_total: u32 = shopping.iter().map(|l| l.total_cost_cents).sum();

    // Demonstrate scaling on the first (menu-opening) dish.
    let first_code = concept
        .menu
        .recipes()
        .first()
        .map(|r| r.code.clone())
        .ok_or_else(|| GastronomeError::InvalidBrief {
            reason: "menu produced no dishes".to_string(),
        })?;
    let scaled = engine.scale_recipe(&first_code, guests)?;

    let within_budget = cost.per_guest_cents <= brief.style.budget_per_guest_cents();

    // --- Verify invariants ---
    let mut notes = Vec::new();
    let mut verified = true;
    let check = |condition: bool, ok: &str, fail: &str, notes: &mut Vec<String>| {
        if condition {
            notes.push(format!("ok: {ok}"));
        } else {
            notes.push(format!("FAIL: {fail}"));
        }
        condition
    };

    verified &= check(
        dish_count >= course_count && course_count > 0,
        "every course has at least one dish",
        "a course produced no dish",
        &mut notes,
    );
    verified &= check(
        cost.per_guest_cents * guests == cost.total_cents,
        "menu cost scales exactly with guest count",
        "menu cost did not scale exactly with guest count",
        &mut notes,
    );
    verified &= check(
        nutrition.per_guest_calories * guests == nutrition.total_calories,
        "nutrition scales exactly with guest count",
        "nutrition did not scale exactly with guest count",
        &mut notes,
    );
    verified &= check(
        shopping_total == cost.total_cents,
        "shopping list reconciles with total menu cost",
        "shopping list did not reconcile with total menu cost",
        &mut notes,
    );
    verified &= check(
        schedule.total_minutes > 0
            && schedule.tasks.len() == engine.prep_task_count()
            && !schedule.has_station_overlap(),
        "prep schedule covers all tasks with no station overlap",
        "prep schedule was incomplete or double-booked a station",
        &mut notes,
    );
    verified &= check(
        scaled.total_cost_cents == scaled.per_serving_cost_cents * guests,
        "sample dish scales exactly to guest count",
        "sample dish did not scale exactly to guest count",
        &mut notes,
    );

    if !verified {
        return Err(GastronomeError::VerificationFailed {
            reason: notes.join("; "),
        });
    }

    let sample_dish = MenuSummary {
        code: scaled.code,
        name: scaled.name,
        course: scaled.course,
        servings: scaled.servings,
        per_serving_cost_cents: scaled.per_serving_cost_cents,
        total_cost_cents: scaled.total_cost_cents,
        per_serving_calories: scaled.per_serving_calories,
    };

    Ok(GastronomeOutcome {
        concept,
        guest_count: guests,
        course_count,
        dish_count,
        total_cost_cents: cost.total_cents,
        per_guest_cost_cents: cost.per_guest_cents,
        per_guest_calories: nutrition.per_guest_calories,
        total_calories: nutrition.total_calories,
        shopping_line_count: shopping.len(),
        prep_total_minutes: schedule.total_minutes,
        prep_batches: schedule.batches,
        within_budget,
        sample_dish,
        verified,
        verification_notes: notes,
    })
}

/// Render an operator-facing text report for a gastronome outcome.
#[must_use]
pub fn render_report(outcome: &GastronomeOutcome) -> String {
    let concept = &outcome.concept;
    let brief = &concept.brief;
    let mut out = String::new();

    out.push_str("Probe mode: gastronome-run\n");
    out.push_str(&format!("Menu: {}\n", brief.name));
    out.push_str(&format!("Occasion: {}\n", brief.occasion));
    out.push_str(&format!("Style: {}\n", brief.style.label()));
    out.push_str(&format!("Guests: {}\n", outcome.guest_count));
    out.push_str(&format!("Tagline: {}\n", concept.identity.tagline));
    out.push_str(&format!("Voice: {}\n", concept.identity.voice));
    out.push_str(&format!(
        "Courses: {} | Dishes: {}\n",
        outcome.course_count, outcome.dish_count
    ));
    for course in &concept.menu.courses {
        for dish in &course.dishes {
            out.push_str(&format!(
                "  {} [{}] {} — {} cents/serving, {} cal/serving\n",
                dish.code,
                course.name,
                dish.name,
                dish.cost_cents_per_serving(),
                dish.calories_per_serving(),
            ));
        }
    }
    out.push_str(&format!(
        "Service notes: {}\n",
        concept.menu.service_notes.join(", ")
    ));
    out.push_str("Service flow:\n");
    for stage in &concept.service_flow.stages {
        out.push_str(&format!(
            "  {} -> {}\n",
            stage.name,
            stage.touchpoints.join(", ")
        ));
    }
    out.push_str(&format!(
        "Cost: {} cents/guest, {} cents total ({} guests)\n",
        outcome.per_guest_cost_cents, outcome.total_cost_cents, outcome.guest_count
    ));
    out.push_str(&format!(
        "Nutrition: {} cal/guest, {} cal total\n",
        outcome.per_guest_calories, outcome.total_calories
    ));
    out.push_str(&format!(
        "Budget ({} cents/guest anchor): {}\n",
        brief.style.budget_per_guest_cents(),
        if outcome.within_budget {
            "within budget"
        } else {
            "over anchor"
        }
    ));
    out.push_str(&format!(
        "Shopping lines: {}\n",
        outcome.shopping_line_count
    ));
    out.push_str(&format!(
        "Prep schedule: {} minutes across {} batch(es)\n",
        outcome.prep_total_minutes, outcome.prep_batches
    ));
    let dish = &outcome.sample_dish;
    out.push_str(&format!(
        "Sample scaled dish: {} ({}) in {}, {} servings, {} cents/serving, {} cents total, {} cal/serving\n",
        dish.code,
        dish.name,
        dish.course,
        dish.servings,
        dish.per_serving_cost_cents,
        dish.total_cost_cents,
        dish.per_serving_calories,
    ));
    out.push_str(&format!(
        "Plan verified: {}\n",
        if outcome.verified { "yes" } else { "no" }
    ));
    for note in &outcome.verification_notes {
        out.push_str(&format!("  - {note}\n"));
    }
    out.push_str("Session phase: complete\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_run_verifies() {
        let brief = MenuBrief::from_prompt(
            "Harvest Feast menu for a wedding of 120 guests, elegant plated",
        );
        let outcome = run_gastronome(&brief).unwrap();
        assert!(outcome.verified);
        assert_eq!(outcome.guest_count, 120);
        assert_eq!(outcome.course_count, 4);
        assert!(outcome.total_cost_cents > 0);
        assert_eq!(
            outcome.per_guest_cost_cents * outcome.guest_count,
            outcome.total_cost_cents
        );
        assert!(outcome.prep_total_minutes > 0);
    }

    #[test]
    fn end_to_end_run_is_deterministic() {
        let brief = MenuBrief::new("Determinism", "a gala", ServiceStyle::Bistro, 50, "t");
        let a = run_gastronome(&brief).unwrap();
        let b = run_gastronome(&brief).unwrap();
        assert_eq!(a.sample_dish, b.sample_dish);
        assert_eq!(a.total_cost_cents, b.total_cost_cents);
        assert_eq!(a.concept, b.concept);
    }

    #[test]
    fn report_contains_key_sections() {
        let brief = MenuBrief::new("Reportel", "a gala", ServiceStyle::FineDining, 200, "grand");
        let outcome = run_gastronome(&brief).unwrap();
        let report = render_report(&outcome);
        assert!(report.contains("Probe mode: gastronome-run"));
        assert!(report.contains("Menu: Reportel"));
        assert!(report.contains("Guests: 200"));
        assert!(report.contains("Sample scaled dish: C1"));
        assert!(report.contains("Plan verified: yes"));
        assert!(report.contains("Session phase: complete"));
    }

    #[test]
    fn outcome_serializes_to_json() {
        let brief = MenuBrief::new("JSON Table", "a dinner", ServiceStyle::Casual, 24, "t");
        let outcome = run_gastronome(&brief).unwrap();
        let json = serde_json::to_string(&outcome).unwrap();
        assert!(json.contains("\"guest_count\":24"));
        let round: GastronomeOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(round.guest_count, 24);
    }

    #[test]
    fn error_display_is_readable() {
        let err = GastronomeError::UnknownRecipe {
            code: "C9".to_string(),
        };
        assert_eq!(err.to_string(), "unknown recipe: C9");
    }

    #[test]
    fn tiny_event_still_runs_end_to_end() {
        let brief = MenuBrief::new("Tiny", "a dinner", ServiceStyle::Casual, 4, "cozy");
        let outcome = run_gastronome(&brief).unwrap();
        assert!(outcome.verified);
        assert_eq!(outcome.guest_count, 4);
        assert_eq!(outcome.course_count, 2);
    }
}
