//! End-to-end planning: turn an [`EventBrief`] plus a [`RecipeBook`] into a
//! fully costed, nutritionally analysed, and prep-scheduled [`EventPlan`].
//!
//! This is the "done when" surface of the Gastronome identity: a brief in, a
//! costed and scheduled menu plan out, deterministically.

use serde::Serialize;

use super::analysis::{CostSummary, NutritionSummary};
use super::error::GastronomeError;
use super::error::GastronomeResult;
use super::recipe_book::RecipeBook;
use super::scaling::{MenuItem, ScaledRecipe, ShoppingLine, consolidate_shopping};
use super::schedule::PrepSchedule;
use super::types::{DietaryTag, EventBrief};

/// A complete, self-contained plan for an event: what to serve, what it costs,
/// what it delivers nutritionally, what to buy, and when to cook it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventPlan {
    /// Event name (from the brief).
    pub event: String,
    /// Guests catered for.
    pub guests: u32,
    /// Serve time, `HH:MM`.
    pub serve_time: String,
    /// The resolved menu, one item per requested course.
    pub menu: Vec<MenuItem>,
    /// Cost breakdown and per-guest cost.
    pub cost: CostSummary,
    /// Per-guest and whole-event nutrition.
    pub nutrition: NutritionSummary,
    /// Consolidated shopping list across the whole menu.
    pub shopping_list: Vec<ShoppingLine>,
    /// Back-scheduled prep timeline.
    pub schedule: PrepSchedule,
}

/// The union of the brief-wide constraints and a course's own extra
/// constraints, de-duplicated and order-preserving.
fn required_tags(brief: &EventBrief, course_dietary: &[DietaryTag]) -> Vec<DietaryTag> {
    let mut required = brief.dietary_constraints.clone();
    for tag in course_dietary {
        if !required.contains(tag) {
            required.push(*tag);
        }
    }
    required
}

fn describe_tags(tags: &[DietaryTag]) -> String {
    if tags.is_empty() {
        "no constraints".to_string()
    } else {
        tags.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Plan an event end-to-end.
///
/// For each requested course the planner resolves a recipe — a pinned
/// `recipe_id` if given, otherwise the cheapest recipe for that course
/// satisfying the combined dietary constraints — scales it to the guest count,
/// then computes cost, nutrition, a consolidated shopping list, and a prep
/// schedule that lands every dish at the serve time.
///
/// # Errors
/// - [`GastronomeError::InvalidGuestCount`] when the brief has zero guests.
/// - [`GastronomeError::NoRecipeForCourse`] when a course cannot be filled
///   (no match, or a pinned recipe is missing or fails constraints).
/// - [`GastronomeError::InvalidBaseServings`] when a chosen recipe cannot scale.
/// - [`GastronomeError::InvalidServeTime`] when `serve_time` is not `HH:MM`.
pub fn plan_event(brief: &EventBrief, book: &RecipeBook) -> GastronomeResult<EventPlan> {
    brief.validate()?;

    let mut menu: Vec<MenuItem> = Vec::with_capacity(brief.courses.len());
    for request in &brief.courses {
        let required = required_tags(brief, &request.dietary);

        let recipe = if let Some(id) = &request.recipe_id {
            let recipe = book
                .find_by_id(id)
                .ok_or_else(|| GastronomeError::NoRecipeForCourse {
                    course: request.course.clone(),
                    reason: format!("pinned recipe '{id}' is not in the book"),
                })?;
            if !recipe.satisfies(&required) {
                return Err(GastronomeError::NoRecipeForCourse {
                    course: request.course.clone(),
                    reason: format!(
                        "pinned recipe '{id}' does not satisfy required tags: {}",
                        describe_tags(&required)
                    ),
                });
            }
            recipe
        } else {
            book.select(&request.course, &required).ok_or_else(|| {
                GastronomeError::NoRecipeForCourse {
                    course: request.course.clone(),
                    reason: format!(
                        "no '{}' recipe satisfies required tags: {}",
                        request.course,
                        describe_tags(&required)
                    ),
                }
            })?
        };

        recipe.validate()?;
        menu.push(MenuItem {
            course: request.course.clone(),
            scaled: ScaledRecipe::new(recipe.clone(), brief.guests),
        });
    }

    let cost = CostSummary::compute(&menu, brief.guests);
    let nutrition = NutritionSummary::compute(&menu, brief.guests);
    let scaled: Vec<ScaledRecipe> = menu.iter().map(|item| item.scaled.clone()).collect();
    let shopping_list = consolidate_shopping(&scaled);
    let schedule = PrepSchedule::compute(&menu, &brief.serve_time)?;

    Ok(EventPlan {
        event: brief.name.clone(),
        guests: brief.guests,
        serve_time: schedule.serve_time.clone(),
        menu,
        cost,
        nutrition,
        shopping_list,
        schedule,
    })
}

impl EventPlan {
    /// Render the plan as a human-readable, multi-section report for the CLI.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "MENU PLAN — {} ({} guests, serve {})\n",
            self.event, self.guests, self.serve_time
        ));
        out.push_str("========================================================\n\n");

        out.push_str("Menu\n----\n");
        for item in &self.menu {
            out.push_str(&format!(
                "  {:<9} {} [{} batch(es), {} min prep]\n",
                format!("{}:", item.course),
                item.scaled.recipe.name,
                item.scaled.batches(),
                item.scaled.recipe.total_prep_minutes(),
            ));
        }

        out.push_str("\nCost\n----\n");
        for line in &self.cost.per_course {
            out.push_str(&format!(
                "  {:<9} {:<28} {:>8.2}\n",
                format!("{}:", line.course),
                line.recipe_name,
                line.total_cost,
            ));
        }
        out.push_str(&format!(
            "  {:<38} {:>8.2}\n",
            "TOTAL", self.cost.total_cost
        ));
        out.push_str(&format!(
            "  {:<38} {:>8.2}\n",
            "PER GUEST", self.cost.cost_per_guest
        ));

        out.push_str("\nNutrition (per guest)\n---------------------\n");
        let g = self.nutrition.per_guest;
        out.push_str(&format!(
            "  {:.0} kcal | {:.1} g protein | {:.1} g carbs | {:.1} g fat\n",
            g.calories, g.protein_g, g.carbs_g, g.fat_g
        ));

        out.push_str("\nShopping list\n-------------\n");
        for line in &self.shopping_list {
            out.push_str(&format!(
                "  {:<20} {:>8.3} {:<7} {:>8.2}\n",
                line.name, line.quantity, line.unit, line.line_cost
            ));
        }

        out.push_str(&format!(
            "\nPrep schedule (kitchen call {}, {} active min)\n",
            self.schedule.kitchen_call_time, self.schedule.total_active_minutes
        ));
        out.push_str("---------------------------------------------\n");
        for step in &self.schedule.steps {
            out.push_str(&format!(
                "  {}-{}  {:<9} {:<20} {}\n",
                step.start,
                step.end,
                format!("[{}]", if step.active { "hands-on" } else { "passive" }),
                step.recipe_name,
                step.description,
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::CourseRequest;
    use super::*;

    fn brief() -> EventBrief {
        EventBrief {
            name: "Summer Dinner".into(),
            guests: 12,
            serve_time: "19:00".into(),
            courses: vec![
                CourseRequest::new("starter"),
                CourseRequest::new("main"),
                CourseRequest::new("dessert"),
            ],
            dietary_constraints: vec![],
        }
    }

    #[test]
    fn plans_a_full_three_course_event() {
        let plan = plan_event(&brief(), &RecipeBook::builtin()).unwrap();
        assert_eq!(plan.guests, 12);
        assert_eq!(plan.menu.len(), 3);
        assert!(plan.cost.total_cost > 0.0);
        assert!(plan.cost.cost_per_guest > 0.0);
        assert!(plan.nutrition.per_guest.calories > 0.0);
        assert!(!plan.shopping_list.is_empty());
        assert!(!plan.schedule.steps.is_empty());
        // Every dish is scaled to the full guest count.
        for item in &plan.menu {
            assert_eq!(item.scaled.target_servings, 12);
        }
    }

    #[test]
    fn end_to_end_totals_are_internally_consistent() {
        let plan = plan_event(&brief(), &RecipeBook::builtin()).unwrap();
        // Cost total equals the sum of shopping-line costs (both derive from the
        // same scaled ingredient quantities).
        let shopping_total: f64 = plan.shopping_list.iter().map(|l| l.line_cost).sum();
        assert!((shopping_total - plan.cost.total_cost).abs() < 0.05);
        // Per-guest cost * guests ≈ total.
        assert!((plan.cost.cost_per_guest * 12.0 - plan.cost.total_cost).abs() < 0.5);
    }

    #[test]
    fn vegan_constraint_selects_vegan_dishes() {
        let mut b = brief();
        b.dietary_constraints = vec![DietaryTag::Vegan];
        // dessert has no vegan option in the built-in book → drop it.
        b.courses = vec![CourseRequest::new("starter"), CourseRequest::new("main")];
        let plan = plan_event(&b, &RecipeBook::builtin()).unwrap();
        let main = plan.menu.iter().find(|i| i.course == "main").unwrap();
        assert_eq!(main.scaled.recipe.id, "chickpea-curry");
    }

    #[test]
    fn unfillable_course_reports_which_and_why() {
        let mut b = brief();
        b.dietary_constraints = vec![DietaryTag::Vegan];
        b.courses = vec![CourseRequest::new("dessert")];
        let err = plan_event(&b, &RecipeBook::builtin()).unwrap_err();
        match err {
            GastronomeError::NoRecipeForCourse { course, reason } => {
                assert_eq!(course, "dessert");
                assert!(reason.contains("vegan"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn pinned_recipe_is_honoured() {
        let mut b = brief();
        b.courses = vec![CourseRequest {
            course: "main".into(),
            dietary: vec![],
            recipe_id: Some("roast-chicken".into()),
        }];
        let plan = plan_event(&b, &RecipeBook::builtin()).unwrap();
        assert_eq!(plan.menu[0].scaled.recipe.id, "roast-chicken");
    }

    #[test]
    fn pinned_recipe_failing_constraints_errors() {
        let mut b = brief();
        b.dietary_constraints = vec![DietaryTag::Vegan];
        b.courses = vec![CourseRequest {
            course: "main".into(),
            dietary: vec![],
            recipe_id: Some("roast-chicken".into()),
        }];
        let err = plan_event(&b, &RecipeBook::builtin()).unwrap_err();
        assert!(matches!(err, GastronomeError::NoRecipeForCourse { .. }));
    }

    #[test]
    fn zero_guests_is_rejected() {
        let mut b = brief();
        b.guests = 0;
        assert!(matches!(
            plan_event(&b, &RecipeBook::builtin()),
            Err(GastronomeError::InvalidGuestCount { .. })
        ));
    }

    #[test]
    fn render_contains_all_sections() {
        let plan = plan_event(&brief(), &RecipeBook::builtin()).unwrap();
        let text = plan.render();
        for needle in [
            "MENU PLAN",
            "Cost",
            "Nutrition",
            "Shopping list",
            "Prep schedule",
        ] {
            assert!(text.contains(needle), "render missing section: {needle}");
        }
    }

    #[test]
    fn plan_serializes_to_json() {
        let plan = plan_event(&brief(), &RecipeBook::builtin()).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains("\"cost_per_guest\""));
        assert!(json.contains("\"kitchen_call_time\""));
    }
}
