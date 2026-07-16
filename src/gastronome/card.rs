//! Menu-card rendering.
//!
//! Produces a human-readable Markdown menu card from the scaled menu, grouped
//! by course, annotated with dietary tags, and footed with the per-guest
//! nutrition and cost summary. This is the presentation artifact of the
//! pipeline — deterministic and tool-free.

use std::fmt::Write as _;

use super::analysis::{NutritionSummary, ShoppingList};
use super::brief::Course;
use super::menu::Menu;

/// Render the menu card as Markdown.
pub fn render_menu_card(
    menu: &Menu,
    nutrition: &NutritionSummary,
    shopping: &ShoppingList,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", menu.event);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "_Menu for {} guest{}._",
        menu.guests,
        if menu.guests == 1 { "" } else { "s" }
    );
    let _ = writeln!(out);

    for course in Course::all_in_menu_order() {
        let dishes: Vec<_> = menu.dishes.iter().filter(|d| d.course == course).collect();
        if dishes.is_empty() {
            continue;
        }
        let _ = writeln!(out, "## {}", title_case(course.label()));
        let _ = writeln!(out);
        for dish in dishes {
            let tags = if dish.tags.is_empty() {
                String::new()
            } else {
                format!("  _({})_", dish.tags.join(", "))
            };
            let _ = writeln!(
                out,
                "- **{}** — {} serving{}{}",
                dish.name,
                dish.total_servings,
                if dish.total_servings == 1 { "" } else { "s" },
                tags
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Per-guest nutrition");
    let _ = writeln!(out);
    let n = &nutrition.per_guest;
    if nutrition.has_data {
        let _ = writeln!(
            out,
            "- Energy: {} kcal\n- Protein: {} g\n- Carbohydrate: {} g\n- Fat: {} g",
            round(n.kcal),
            round(n.protein_g),
            round(n.carbs_g),
            round(n.fat_g),
        );
    } else {
        let _ = writeln!(out, "- No nutrition data supplied in the brief.");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## Cost");
    let _ = writeln!(out);
    match (shopping.total_cost, shopping.cost_per_guest) {
        (Some(total), Some(per)) => {
            let _ = writeln!(
                out,
                "- Total: {} {}\n- Per guest: {} {}{}",
                round(total),
                menu.currency,
                round(per),
                menu.currency,
                if shopping.over_budget {
                    " — **over budget**"
                } else {
                    ""
                },
            );
        }
        _ => {
            let _ = writeln!(out, "- Cost not fully specified in the brief.");
        }
    }

    out
}

fn round(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r:.2}")
    }
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::analysis::{build_nutrition, build_shopping_list};
    use crate::gastronome::brief::MenuBrief;
    use crate::gastronome::menu::scale;

    const BRIEF: &str = r#"{
        "event":"Autumn dinner","guests":8,"currency":"USD","budget":100.0,
        "dishes":[
            {"name":"Squash soup","course":"starter","tags":["vegetarian"],
             "ingredients":[{"name":"Squash","qty_per_serving":100,"unit":"g","cost_per_unit":0.004,
                 "nutrition":{"kcal":0.45,"protein_g":0.01,"carbs_g":0.12,"fat_g":0.001}}]},
            {"name":"Beef roast","course":"main",
             "ingredients":[{"name":"Beef","qty_per_serving":180,"unit":"g","cost_per_unit":0.02}]}
        ]}"#;

    fn parts() -> (Menu, NutritionSummary, ShoppingList) {
        let brief = MenuBrief::from_json_bytes(BRIEF.as_bytes()).unwrap();
        let m = scale(&brief);
        let n = build_nutrition(&m);
        let s = build_shopping_list(&m, brief.budget);
        (m, n, s)
    }

    #[test]
    fn card_lists_event_courses_and_dishes() {
        let (m, n, s) = parts();
        let card = render_menu_card(&m, &n, &s);
        assert!(card.contains("# Autumn dinner"));
        assert!(card.contains("## Starter"));
        assert!(card.contains("## Main"));
        assert!(card.contains("Squash soup"));
        assert!(card.contains("Beef roast"));
        assert!(card.contains("vegetarian"));
    }

    #[test]
    fn card_includes_nutrition_and_cost_sections() {
        let (m, n, s) = parts();
        let card = render_menu_card(&m, &n, &s);
        assert!(card.contains("## Per-guest nutrition"));
        assert!(card.contains("kcal"));
        assert!(card.contains("## Cost"));
        assert!(card.contains("USD"));
    }

    #[test]
    fn card_notes_missing_data() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#,
        )
        .unwrap();
        let m = scale(&brief);
        let n = build_nutrition(&m);
        let s = build_shopping_list(&m, None);
        let card = render_menu_card(&m, &n, &s);
        assert!(card.contains("No nutrition data"));
        assert!(card.contains("Cost not fully specified"));
    }
}
