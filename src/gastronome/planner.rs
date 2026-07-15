//! The end-to-end planner: take an [`EventBrief`] to a costed, scheduled
//! [`MenuPlan`]. This is the module's headline capability — "brief in, plan
//! out" — and it is pure and deterministic.

use super::cost::recipe_cost;
use super::library::Pantry;
use super::scaling::scale_recipe;
use super::scheduling::build_schedule;
use super::types::{
    CostBreakdown, EventBrief, GastronomeError, GastronomeResult, MenuPlan, NutritionFacts,
    NutritionSummary, Recipe, round2,
};

/// Plan an event end-to-end against `pantry`:
/// 1. resolve the named menu and its recipes,
/// 2. enforce dietary restrictions (fail closed on a conflict),
/// 3. scale every recipe to the guest count,
/// 4. roll up nutrition and cost (total + per-guest),
/// 5. back-schedule prep to the service time,
/// 6. attach budget/feasibility warnings.
pub fn plan_event(pantry: &Pantry, brief: &EventBrief) -> GastronomeResult<MenuPlan> {
    if brief.guest_count == 0 {
        return Err(GastronomeError::InvalidQuantity {
            field: "guest_count".into(),
        });
    }

    let menu = pantry.menu(&brief.menu_id)?;
    if menu.recipe_ids.is_empty() {
        return Err(GastronomeError::EmptyMenu(menu.id.clone()));
    }

    // Resolve recipes once; reused for dietary checks, scaling, scheduling.
    let recipes: Vec<&Recipe> = menu
        .recipe_ids
        .iter()
        .map(|id| pantry.recipe(id))
        .collect::<GastronomeResult<Vec<_>>>()?;

    // Dietary enforcement — fail closed.
    for recipe in &recipes {
        let satisfied = pantry.recipe_dietary_tags(recipe)?;
        for required in &brief.dietary_restrictions {
            if !satisfied.contains(required) {
                return Err(GastronomeError::DietaryConflict {
                    recipe: recipe.name.clone(),
                    tag: *required,
                });
            }
        }
    }

    // Scale + roll up.
    let mut scaled = Vec::with_capacity(recipes.len());
    let mut nutrition_total = NutritionFacts::default();
    let mut cost_total = 0.0;
    let mut per_recipe = Vec::with_capacity(recipes.len());
    for recipe in &recipes {
        let sr = scale_recipe(pantry, recipe, brief.guest_count)?;
        nutrition_total = nutrition_total + sr.nutrition_total;
        // Recompute the exact (unrounded) cost for the roll-up so per-recipe
        // rounding does not drift the total.
        let exact = recipe_cost(pantry, recipe)? * sr.scale_factor;
        cost_total += exact;
        per_recipe.push((recipe.name.clone(), round2(exact)));
        scaled.push(sr);
    }

    let guests = brief.guest_count as f64;
    let cost = CostBreakdown {
        total: round2(cost_total),
        per_guest: round2(cost_total / guests),
        per_recipe,
    };
    let nutrition = NutritionSummary {
        total: nutrition_total.rounded(),
        per_guest: nutrition_total.scaled(1.0 / guests).rounded(),
    };

    let schedule = build_schedule(&recipes, brief.service_time_min);

    // Warnings (non-fatal advisories).
    let mut warnings = Vec::new();
    if let Some(budget) = brief.budget_per_guest {
        let per_guest = cost_total / guests;
        if per_guest > budget {
            warnings.push(format!(
                "over budget: {:.2}/guest exceeds cap {:.2}/guest",
                per_guest, budget
            ));
        }
    }
    if let Some(last) = schedule.tasks.last() {
        // When prep fits, the run is laid out to end exactly at service time.
        // When it overflows, `kitchen_start` is clamped to 0 and the final
        // task ends *after* service time — that is the real "doesn't fit" case.
        if last.end_min > schedule.service_time_min {
            warnings
                .push("prep does not fit before service time; start the previous day".to_string());
        }
    }

    Ok(MenuPlan {
        event_name: brief.event_name.clone(),
        guest_count: brief.guest_count,
        menu_name: menu.name.clone(),
        recipes: scaled,
        nutrition,
        cost,
        schedule,
        warnings,
    })
}

/// A ready-made demo brief that plans successfully against the built-in
/// pantry — used by `simard gastronome demo` and the end-to-end tests.
pub fn demo_brief() -> EventBrief {
    EventBrief {
        event_name: "Team celebration dinner".to_string(),
        guest_count: 24,
        menu_id: "italian-dinner".to_string(),
        dietary_restrictions: std::collections::BTreeSet::new(),
        budget_per_guest: Some(12.0),
        service_time_min: 18 * 60, // 18:00
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::library::builtin_pantry;
    use crate::gastronome::types::DietaryTag;

    #[test]
    fn demo_plans_end_to_end() {
        let p = builtin_pantry();
        let plan = plan_event(&p, &demo_brief()).unwrap();
        assert_eq!(plan.guest_count, 24);
        assert_eq!(plan.recipes.len(), 3);
        assert!(plan.cost.total > 0.0);
        assert!(plan.nutrition.per_guest.calories > 0.0);
        assert_eq!(plan.schedule.tasks.last().unwrap().end_min, 18 * 60);
    }

    #[test]
    fn per_guest_cost_is_total_over_guests() {
        let p = builtin_pantry();
        let plan = plan_event(&p, &demo_brief()).unwrap();
        assert!((plan.cost.per_guest * 24.0 - plan.cost.total).abs() < 0.5);
    }

    #[test]
    fn zero_guests_errors() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        brief.guest_count = 0;
        assert!(matches!(
            plan_event(&p, &brief),
            Err(GastronomeError::InvalidQuantity { .. })
        ));
    }

    #[test]
    fn dietary_conflict_fails_closed() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        // Italian dinner has caprese (dairy) + pasta (gluten) → cannot be vegan.
        brief.dietary_restrictions.insert(DietaryTag::Vegan);
        assert!(matches!(
            plan_event(&p, &brief),
            Err(GastronomeError::DietaryConflict { .. })
        ));
    }

    #[test]
    fn vegan_gf_menu_satisfies_restrictions() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        brief.menu_id = "vegan-gf-lunch".into();
        brief.dietary_restrictions.insert(DietaryTag::Vegan);
        brief.dietary_restrictions.insert(DietaryTag::GlutenFree);
        let plan = plan_event(&p, &brief).unwrap();
        assert_eq!(plan.recipes.len(), 3);
    }

    #[test]
    fn budget_cap_emits_warning() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        brief.budget_per_guest = Some(0.01); // impossibly low
        let plan = plan_event(&p, &brief).unwrap();
        assert!(plan.warnings.iter().any(|w| w.contains("over budget")));
    }

    #[test]
    fn generous_budget_no_warning() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        brief.budget_per_guest = Some(1000.0);
        let plan = plan_event(&p, &brief).unwrap();
        assert!(!plan.warnings.iter().any(|w| w.contains("over budget")));
    }

    #[test]
    fn prep_overflow_emits_fit_warning() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        // Service at 00:30 — far less than the menu's total prep time, so the
        // schedule cannot fit before service and must warn.
        brief.service_time_min = 30;
        let plan = plan_event(&p, &brief).unwrap();
        assert!(
            plan.warnings.iter().any(|w| w.contains("does not fit")),
            "expected a prep-fit warning, got: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn feasible_service_time_has_no_fit_warning() {
        let p = builtin_pantry();
        let brief = demo_brief(); // 18:00, plenty of runway
        let plan = plan_event(&p, &brief).unwrap();
        assert!(
            !plan.warnings.iter().any(|w| w.contains("does not fit")),
            "did not expect a prep-fit warning, got: {:?}",
            plan.warnings
        );
    }

    #[test]
    fn unknown_menu_errors() {
        let p = builtin_pantry();
        let mut brief = demo_brief();
        brief.menu_id = "does-not-exist".into();
        assert!(matches!(
            plan_event(&p, &brief),
            Err(GastronomeError::UnknownMenu(_))
        ));
    }

    #[test]
    fn scaling_is_linear_in_guest_count() {
        let p = builtin_pantry();
        let mut b12 = demo_brief();
        b12.guest_count = 12;
        b12.budget_per_guest = None;
        let mut b24 = demo_brief();
        b24.guest_count = 24;
        b24.budget_per_guest = None;
        let p12 = plan_event(&p, &b12).unwrap();
        let p24 = plan_event(&p, &b24).unwrap();
        assert!((p24.cost.total - p12.cost.total * 2.0).abs() < 0.5);
    }
}
