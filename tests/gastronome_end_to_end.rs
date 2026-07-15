//! End-to-end integration coverage for the Gastronome culinary / menu &
//! event-design identity: the public "brief in → costed, scheduled plan out"
//! contract, exercised through the library API exactly as the
//! `simard gastronome` CLI drives it.

use std::collections::BTreeSet;

use simard::gastronome::types::DietaryTag;
use simard::gastronome::{
    EventBrief, builtin_pantry, demo_brief, parse_brief, plan_event, render_plan,
};

#[test]
fn brief_to_costed_scheduled_plan_end_to_end() {
    let pantry = builtin_pantry();
    let brief = demo_brief();
    let plan = plan_event(&pantry, &brief).expect("demo brief plans");

    // Menu chosen and scaled to the headcount.
    assert_eq!(plan.guest_count, 24);
    assert_eq!(plan.menu_name, "Italian dinner");
    assert_eq!(plan.recipes.len(), 3);
    for r in &plan.recipes {
        assert_eq!(r.target_servings, 24);
        assert!(r.cost_total > 0.0);
        assert!(r.nutrition_total.calories > 0.0);
    }

    // Costed: total and per-guest are consistent.
    assert!(plan.cost.total > 0.0);
    assert!((plan.cost.per_guest * 24.0 - plan.cost.total).abs() < 0.5);

    // Nutrition rolled up per guest.
    assert!(plan.nutrition.per_guest.calories > 0.0);
    assert!(plan.nutrition.total.protein_g > 0.0);

    // Scheduled: finishes exactly at service time (18:00 = 1080).
    let last = plan.schedule.tasks.last().expect("has tasks");
    assert_eq!(last.end_min, 18 * 60);
    assert!(plan.schedule.kitchen_start_min < plan.schedule.service_time_min);

    // Rendered report carries all sections.
    let report = render_plan(&plan);
    for section in ["Event:", "Cost", "Nutrition", "Prep schedule"] {
        assert!(report.contains(section), "report missing {section}");
    }
}

#[test]
fn text_brief_json_parses_and_plans() {
    let json = r#"{
        "event_name": "Board dinner",
        "guest_count": 12,
        "menu_id": "italian-dinner",
        "service_time_min": 1140
    }"#;
    let brief = parse_brief(json).expect("json brief parses");
    let plan = plan_event(&builtin_pantry(), &brief).expect("plans");
    assert_eq!(plan.guest_count, 12);
    assert_eq!(plan.schedule.tasks.last().unwrap().end_min, 1140);
}

#[test]
fn text_brief_toml_parses_and_plans() {
    let toml_text = r#"
        event_name = "Board dinner"
        guest_count = 12
        menu_id = "italian-dinner"
        service_time_min = 1140
    "#;
    let brief = parse_brief(toml_text).expect("toml brief parses");
    let plan = plan_event(&builtin_pantry(), &brief).expect("plans");
    assert_eq!(plan.guest_count, 12);
}

#[test]
fn dietary_restrictions_are_enforced_fail_closed() {
    let pantry = builtin_pantry();

    // A vegan restriction on the (dairy+gluten) Italian menu must fail.
    let mut restrictions = BTreeSet::new();
    restrictions.insert(DietaryTag::Vegan);
    let bad = EventBrief {
        event_name: "Vegan gala".into(),
        guest_count: 10,
        menu_id: "italian-dinner".into(),
        dietary_restrictions: restrictions.clone(),
        budget_per_guest: None,
        service_time_min: 1080,
    };
    assert!(
        plan_event(&pantry, &bad).is_err(),
        "vegan italian must fail closed"
    );

    // The vegan+GF lunch satisfies the same restrictions.
    restrictions.insert(DietaryTag::GlutenFree);
    let good = EventBrief {
        event_name: "Vegan lunch".into(),
        guest_count: 10,
        menu_id: "vegan-gf-lunch".into(),
        dietary_restrictions: restrictions,
        budget_per_guest: None,
        service_time_min: 780,
    };
    assert!(
        plan_event(&pantry, &good).is_ok(),
        "vegan-gf lunch must satisfy"
    );
}

#[test]
fn budget_overage_is_surfaced_not_hidden() {
    let pantry = builtin_pantry();
    let mut brief = demo_brief();
    brief.budget_per_guest = Some(0.01);
    let plan = plan_event(&pantry, &brief).unwrap();
    assert!(
        plan.warnings.iter().any(|w| w.contains("over budget")),
        "over-budget plan must warn"
    );
}

#[test]
fn plan_scales_linearly_with_guest_count() {
    let pantry = builtin_pantry();
    let mut small = demo_brief();
    small.guest_count = 10;
    small.budget_per_guest = None;
    let mut large = demo_brief();
    large.guest_count = 30;
    large.budget_per_guest = None;

    let p_small = plan_event(&pantry, &small).unwrap();
    let p_large = plan_event(&pantry, &large).unwrap();
    assert!((p_large.cost.total - p_small.cost.total * 3.0).abs() < 0.5);
}
