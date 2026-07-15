//! Gastronome — a pluggable Simard culinary / menu & event-design identity.
//!
//! This module is the deterministic engine behind the Gastronome identity: a
//! small "kitchen app" that takes an [`EventBrief`] and produces a costed,
//! scheduled [`MenuPlan`] end-to-end. It is pure Rust — no I/O, clocks, or
//! network — so every capability (nutrition, cost, scaling, prep scheduling)
//! is reproducible and unit-tested.
//!
//! Capability map:
//! - [`types`] — the domain vocabulary (ingredients, recipes, menus, briefs,
//!   plans) plus [`GastronomeError`].
//! - [`library`] — the built-in [`Pantry`] so plans work with zero config.
//! - [`nutrition`] — per-recipe / per-serving nutrition roll-ups.
//! - [`cost`] — per-recipe / per-serving costing.
//! - [`scaling`] — scale a recipe to an event's headcount.
//! - [`scheduling`] — back-schedule prep to the service time.
//! - [`planner`] — the end-to-end brief → plan pipeline.

pub mod cost;
pub mod library;
pub mod nutrition;
pub mod planner;
pub mod scaling;
pub mod scheduling;
pub mod types;

pub use library::{Pantry, builtin_pantry};
pub use planner::{demo_brief, plan_event};
pub use types::{
    CostBreakdown, Course, DietaryTag, EventBrief, GastronomeError, GastronomeResult, Ingredient,
    Menu, MenuPlan, NutritionFacts, NutritionSummary, PrepSchedule, PrepTask, Recipe,
    RecipeIngredient, RecipeStep, ScaledIngredientLine, ScaledRecipe, Stage, fmt_hhmm,
};

/// Parse an [`EventBrief`] from a document. JSON is tried first, then TOML, so
/// operators can hand the CLI whichever they prefer.
pub fn parse_brief(text: &str) -> GastronomeResult<EventBrief> {
    let json_err = match serde_json::from_str::<EventBrief>(text) {
        Ok(brief) => return Ok(brief),
        Err(e) => e.to_string(),
    };
    match toml::from_str::<EventBrief>(text) {
        Ok(brief) => Ok(brief),
        Err(toml_err) => Err(GastronomeError::Parse(format!(
            "not valid JSON ({json_err}) or TOML ({toml_err})"
        ))),
    }
}

/// Render a [`MenuPlan`] as an operator-friendly text report.
pub fn render_plan(plan: &MenuPlan) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let _ = writeln!(out, "Event:  {}", plan.event_name);
    let _ = writeln!(
        out,
        "Menu:   {} ({} guests)",
        plan.menu_name, plan.guest_count
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "Menu & scaling");
    for r in &plan.recipes {
        let _ = writeln!(
            out,
            "  - {} [{}] — base {} → {} servings (x{:.2}), cost {:.2}",
            r.name, r.course, r.base_servings, r.target_servings, r.scale_factor, r.cost_total
        );
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "Cost");
    let _ = writeln!(out, "  total:     {:.2}", plan.cost.total);
    let _ = writeln!(out, "  per guest: {:.2}", plan.cost.per_guest);
    let _ = writeln!(out);

    let _ = writeln!(out, "Nutrition (per guest)");
    let n = &plan.nutrition.per_guest;
    let _ = writeln!(
        out,
        "  {:.0} kcal | protein {:.1} g | carbs {:.1} g | fat {:.1} g",
        n.calories, n.protein_g, n.carbs_g, n.fat_g
    );
    let _ = writeln!(out);

    let _ = writeln!(
        out,
        "Prep schedule (kitchen opens {}, service {})",
        fmt_hhmm(plan.schedule.kitchen_start_min),
        fmt_hhmm(plan.schedule.service_time_min)
    );
    for t in &plan.schedule.tasks {
        let _ = writeln!(
            out,
            "  {}–{}  [{}] {} — {}",
            fmt_hhmm(t.start_min),
            fmt_hhmm(t.end_min),
            t.stage,
            t.recipe_name,
            t.description
        );
    }

    if !plan.warnings.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Warnings");
        for w in &plan.warnings {
            let _ = writeln!(out, "  ! {w}");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_brief_accepts_json() {
        let json = r#"{
            "event_name": "Gala",
            "guest_count": 10,
            "menu_id": "italian-dinner",
            "service_time_min": 1140
        }"#;
        let brief = parse_brief(json).unwrap();
        assert_eq!(brief.guest_count, 10);
        assert_eq!(brief.menu_id, "italian-dinner");
    }

    #[test]
    fn parse_brief_accepts_toml() {
        let toml_text = r#"
            event_name = "Gala"
            guest_count = 10
            menu_id = "italian-dinner"
            service_time_min = 1140
        "#;
        let brief = parse_brief(toml_text).unwrap();
        assert_eq!(brief.guest_count, 10);
    }

    #[test]
    fn parse_brief_rejects_garbage() {
        assert!(matches!(
            parse_brief("not a brief at all ::: {{"),
            Err(GastronomeError::Parse(_))
        ));
    }

    #[test]
    fn render_plan_contains_key_sections() {
        let p = builtin_pantry();
        let plan = plan_event(&p, &demo_brief()).unwrap();
        let text = render_plan(&plan);
        assert!(text.contains("Cost"));
        assert!(text.contains("Nutrition"));
        assert!(text.contains("Prep schedule"));
        assert!(text.contains("service 18:00"));
    }

    #[test]
    fn end_to_end_brief_text_to_plan() {
        // Full "brief in → plan out" path through the public API.
        let json = r#"{
            "event_name": "Client lunch",
            "guest_count": 16,
            "menu_id": "vegan-gf-lunch",
            "dietary_restrictions": ["vegan", "gluten-free"],
            "service_time_min": 750
        }"#;
        let brief = parse_brief(json).unwrap();
        let plan = plan_event(&builtin_pantry(), &brief).unwrap();
        assert_eq!(plan.guest_count, 16);
        assert!(plan.cost.total > 0.0);
        assert_eq!(plan.schedule.tasks.last().unwrap().end_min, 750);
    }
}
