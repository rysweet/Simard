//! The end-to-end planner: turn an [`EventBrief`] into a fully costed and
//! scheduled [`MenuPlan`]. This is the module's headline capability — a
//! Gastronome takes an event/menu brief to a costed, scheduled menu plan in one
//! call.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::book::KitchenBook;
use super::cost::{RecipeCost, cost_recipe, round_cents};
use super::nutrition::{RecipeNutrition, nutrition_recipe};
use super::scaling::scale_recipe;
use super::scheduling::{PrepSchedule, schedule};
use super::types::{ClockTime, EventBrief, GastronomeResult, Nutrition, Unit};

/// A planned course: the scaled cost and nutrition for one menu item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoursePlan {
    /// Recipe id.
    pub recipe: String,
    /// Recipe name.
    pub name: String,
    /// Portions served per guest.
    pub portions_per_guest: f64,
    /// Total servings prepared (`guest_count * portions_per_guest`).
    pub target_servings: f64,
    /// Cost breakdown at scale.
    pub cost: RecipeCost,
    /// Nutrition breakdown at scale.
    pub nutrition: RecipeNutrition,
}

/// One consolidated line of the shopping list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShoppingItem {
    /// Ingredient id.
    pub ingredient: String,
    /// Ingredient name.
    pub name: String,
    /// Total quantity needed across the whole menu, in base units.
    pub base_quantity: f64,
    /// The base unit.
    pub base_unit: Unit,
    /// Extended cost for the whole quantity.
    pub cost: f64,
}

/// A complete, costed, scheduled menu plan — the end-to-end deliverable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MenuPlan {
    /// Event name.
    pub event: String,
    /// Guest count.
    pub guest_count: u32,
    /// Service time (`HH:MM`).
    pub service_time: String,
    /// Per-course plans.
    pub courses: Vec<CoursePlan>,
    /// Consolidated shopping list across all courses.
    pub shopping_list: Vec<ShoppingItem>,
    /// Grand total ingredient cost.
    pub total_cost: f64,
    /// Cost per guest (`total_cost / guest_count`).
    pub cost_per_guest: f64,
    /// The declared per-guest budget, if any.
    pub budget_per_guest: Option<f64>,
    /// Whether `cost_per_guest <= budget_per_guest` (None if no budget given).
    pub within_budget: Option<bool>,
    /// Aggregate nutrition a single guest receives across all their portions.
    pub nutrition_per_guest: Nutrition,
    /// The backward-planned prep schedule.
    pub schedule: PrepSchedule,
}

/// Build a [`MenuPlan`] from a brief against a kitchen book.
///
/// # Errors
/// Returns an error if the brief references an unknown recipe, the service time
/// is malformed, or the prep-dependency graph has a cycle.
pub fn plan_event(book: &KitchenBook, brief: &EventBrief) -> GastronomeResult<MenuPlan> {
    book.validate_brief(brief)?;
    let service_time = ClockTime::parse(&brief.service_time)?;
    let guest_count = f64::from(brief.guest_count);

    let mut courses = Vec::with_capacity(brief.courses.len());
    let mut shopping: BTreeMap<String, ShoppingItem> = BTreeMap::new();
    let mut total_cost = 0.0;
    let mut nutrition_per_guest = Nutrition::default();

    for course in &brief.courses {
        let recipe = book.recipe(&course.recipe)?;
        let target_servings = guest_count * course.portions_per_guest;
        let scaled = scale_recipe(book, recipe, target_servings)?;
        let cost = cost_recipe(book, &scaled)?;
        let nutrition = nutrition_recipe(book, &scaled)?;

        total_cost += cost.total;
        // Each guest gets `portions_per_guest` servings of this course.
        nutrition_per_guest =
            nutrition_per_guest + nutrition.per_serving.scaled(course.portions_per_guest);

        for line in &scaled.lines {
            let unit_cost = book.ingredient(&line.ingredient)?.price_per_base;
            let entry = shopping
                .entry(line.ingredient.clone())
                .or_insert_with(|| ShoppingItem {
                    ingredient: line.ingredient.clone(),
                    name: line.name.clone(),
                    base_quantity: 0.0,
                    base_unit: line.base_unit,
                    cost: 0.0,
                });
            entry.base_quantity += line.base_quantity;
            entry.cost += line.base_quantity * unit_cost;
        }

        courses.push(CoursePlan {
            recipe: recipe.id.clone(),
            name: recipe.name.clone(),
            portions_per_guest: course.portions_per_guest,
            target_servings,
            cost,
            nutrition,
        });
    }

    let cost_per_guest = if brief.guest_count == 0 {
        0.0
    } else {
        total_cost / guest_count
    };
    let within_budget = brief
        .budget_per_guest
        .map(|budget| cost_per_guest <= budget);

    let recipe_ids: Vec<String> = brief.courses.iter().map(|c| c.recipe.clone()).collect();
    let schedule = schedule(book, &recipe_ids, service_time)?;

    let shopping_list: Vec<ShoppingItem> = shopping.into_values().collect();

    Ok(MenuPlan {
        event: brief.name.clone(),
        guest_count: brief.guest_count,
        service_time: brief.service_time.clone(),
        courses,
        shopping_list,
        total_cost,
        cost_per_guest,
        budget_per_guest: brief.budget_per_guest,
        within_budget,
        nutrition_per_guest,
        schedule,
    })
}

/// Render a plan as a human-readable text report for the CLI.
#[must_use]
pub fn render_plan_text(plan: &MenuPlan) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Menu plan — {}", plan.event);
    let _ = writeln!(
        out,
        "Guests: {}   Service: {}",
        plan.guest_count, plan.service_time
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "Courses");
    for c in &plan.courses {
        let _ = writeln!(
            out,
            "  {:<28} {:>5.0} servings  cost ${:>8.2}  ({:.0} kcal/serving)",
            c.name,
            c.target_servings,
            round_cents(c.cost.total),
            c.nutrition.per_serving.calories
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Shopping list");
    for item in &plan.shopping_list {
        let _ = writeln!(
            out,
            "  {:<28} {:>10.1} {:<3} ${:>8.2}",
            item.name,
            item.base_quantity,
            item.base_unit.label(),
            round_cents(item.cost)
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Cost");
    let _ = writeln!(out, "  Total          ${:.2}", round_cents(plan.total_cost));
    let _ = writeln!(
        out,
        "  Per guest      ${:.2}",
        round_cents(plan.cost_per_guest)
    );
    if let Some(budget) = plan.budget_per_guest {
        let verdict = match plan.within_budget {
            Some(true) => "within budget",
            Some(false) => "OVER BUDGET",
            None => "n/a",
        };
        let _ = writeln!(
            out,
            "  Budget/guest   ${:.2}  ({verdict})",
            round_cents(budget)
        );
    }
    let _ = writeln!(out);

    let n = &plan.nutrition_per_guest;
    let _ = writeln!(out, "Nutrition per guest");
    let _ = writeln!(
        out,
        "  {:.0} kcal   protein {:.1} g   carbs {:.1} g   fat {:.1} g",
        n.calories, n.protein_g, n.carbs_g, n.fat_g
    );
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "Prep schedule (kitchen call {} → service {})",
        plan.schedule.kitchen_start, plan.schedule.service_time
    );
    for t in &plan.schedule.tasks {
        let _ = writeln!(
            out,
            "  {} – {}  {:<28} ({} min)",
            t.start, t.end, t.name, t.duration_minutes
        );
    }
    out
}

/// Render a plan as pretty JSON.
///
/// # Errors
/// Returns [`super::types::GastronomeError::Serialize`] if serialization fails.
pub fn render_plan_json(plan: &MenuPlan) -> GastronomeResult<String> {
    serde_json::to_string_pretty(plan)
        .map_err(|e| super::types::GastronomeError::Serialize(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_plan() -> MenuPlan {
        let book = KitchenBook::demo();
        let brief = book.brief.clone().unwrap();
        plan_event(&book, &brief).unwrap()
    }

    #[test]
    fn end_to_end_demo_produces_costed_scheduled_plan() {
        let plan = demo_plan();
        // Costed: positive total and per-guest.
        assert!(plan.total_cost > 0.0);
        assert!(plan.cost_per_guest > 0.0);
        // Scheduled: every course appears, plus the poolish prerequisite.
        assert!(plan.schedule.tasks.iter().any(|t| t.recipe == "poolish"));
        assert!(plan.schedule.tasks.iter().any(|t| t.recipe == "focaccia"));
        // Nutrition per guest is aggregated.
        assert!(plan.nutrition_per_guest.calories > 0.0);
        // Budget verdict present.
        assert!(plan.within_budget.is_some());
    }

    #[test]
    fn shopping_list_consolidates_shared_ingredients() {
        let plan = demo_plan();
        // flour appears in poolish? no — flour is used by focaccia and tart.
        let flour = plan
            .shopping_list
            .iter()
            .find(|i| i.ingredient == "flour")
            .unwrap();
        // focaccia: 300 g/8 servings * 40 = 1500 g; tart: 60 g/8 * 40 = 300 g → 1800 g.
        assert!((flour.base_quantity - 1800.0).abs() < 1e-6);
    }

    #[test]
    fn per_guest_cost_is_total_over_guests() {
        let plan = demo_plan();
        assert!((plan.cost_per_guest - plan.total_cost / 40.0).abs() < 1e-9);
    }

    #[test]
    fn total_cost_equals_sum_of_shopping_list() {
        let plan = demo_plan();
        let sum: f64 = plan.shopping_list.iter().map(|i| i.cost).sum();
        assert!((sum - plan.total_cost).abs() < 1e-6);
    }

    #[test]
    fn total_cost_equals_sum_of_course_costs() {
        let plan = demo_plan();
        let sum: f64 = plan.courses.iter().map(|c| c.cost.total).sum();
        assert!((sum - plan.total_cost).abs() < 1e-6);
    }

    #[test]
    fn budget_flag_reflects_threshold() {
        let book = KitchenBook::demo();
        let mut brief = book.brief.clone().unwrap();
        brief.budget_per_guest = Some(0.01); // impossible
        let plan = plan_event(&book, &brief).unwrap();
        assert_eq!(plan.within_budget, Some(false));
        brief.budget_per_guest = Some(1_000.0); // trivially met
        let plan = plan_event(&book, &brief).unwrap();
        assert_eq!(plan.within_budget, Some(true));
    }

    #[test]
    fn text_report_mentions_key_sections() {
        let plan = demo_plan();
        let text = render_plan_text(&plan);
        assert!(text.contains("Menu plan"));
        assert!(text.contains("Shopping list"));
        assert!(text.contains("Prep schedule"));
        assert!(text.contains("Per guest"));
    }

    #[test]
    fn json_report_roundtrips() {
        let plan = demo_plan();
        let json = render_plan_json(&plan).unwrap();
        let parsed: MenuPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.event, plan.event);
        assert!((parsed.total_cost - plan.total_cost).abs() < 1e-9);
    }

    #[test]
    fn zero_portions_course_costs_nothing() {
        let book = KitchenBook::demo();
        let mut brief = book.brief.clone().unwrap();
        for c in &mut brief.courses {
            c.portions_per_guest = 0.0;
        }
        let plan = plan_event(&book, &brief).unwrap();
        assert!(plan.total_cost.abs() < 1e-9);
        assert!(plan.nutrition_per_guest.calories.abs() < 1e-9);
    }
}
