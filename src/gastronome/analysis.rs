//! Shopping list, cost roll-up, and nutrition analysis.
//!
//! Derived deterministically from the scaled [`Menu`] — no external tools — so
//! this always runs. Aggregates ingredients across dishes into a shopping list,
//! rolls a total and per-guest cost against the brief budget, and summarises
//! per-guest and total nutrition. Emits CSV for the shopping list and the
//! nutrition breakdown.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::brief::Nutrition;
use super::menu::Menu;

/// One consolidated shopping-list line: an ingredient totalled across dishes.
#[derive(Debug, Clone, PartialEq)]
pub struct ShoppingRow {
    pub ingredient: String,
    pub unit: String,
    pub qty: f64,
    pub unit_cost: Option<f64>,
    pub total_cost: Option<f64>,
}

/// The consolidated shopping list plus a cost roll-up.
#[derive(Debug, Clone)]
pub struct ShoppingList {
    pub rows: Vec<ShoppingRow>,
    /// Total ingredient cost, if every ingredient carried a cost.
    pub total_cost: Option<f64>,
    /// Cost per guest, if `total_cost` is known and guests > 0.
    pub cost_per_guest: Option<f64>,
    /// Budget from the brief, if any.
    pub budget: Option<f64>,
    /// True when `total_cost` is known and exceeds `budget`.
    pub over_budget: bool,
}

/// Build the consolidated shopping list and cost roll-up for a menu.
///
/// Ingredients are aggregated by `(name, unit)` so the same item measured in
/// two different units stays on two lines (they cannot be summed safely).
pub fn build_shopping_list(menu: &Menu, budget: Option<f64>) -> ShoppingList {
    // BTreeMap keeps the output deterministic (sorted by name then unit).
    let mut agg: BTreeMap<(String, String), (f64, Option<f64>, bool)> = BTreeMap::new();
    for dish in &menu.dishes {
        for ing in &dish.ingredients {
            let key = (ing.name.clone(), ing.unit.clone());
            let entry = agg.entry(key).or_insert((0.0, ing.cost_per_unit, false));
            entry.0 += ing.total_qty;
            // Preserve a consistent unit cost; if lines disagree keep the first.
            if entry.1.is_none() {
                entry.1 = ing.cost_per_unit;
            }
            if ing.cost_per_unit.is_none() {
                entry.2 = true; // saw a line with no cost
            }
        }
    }

    let mut rows = Vec::with_capacity(agg.len());
    let mut total = 0.0;
    let mut costed_all = !agg.is_empty();
    for ((ingredient, unit), (qty, unit_cost, saw_uncosted)) in agg {
        let total_cost = match (unit_cost, saw_uncosted) {
            (Some(c), false) => Some(c * qty),
            _ => None,
        };
        match total_cost {
            Some(c) => total += c,
            None => costed_all = false,
        }
        rows.push(ShoppingRow {
            ingredient,
            unit,
            qty,
            unit_cost,
            total_cost,
        });
    }

    let total_cost = costed_all.then_some(total);
    let cost_per_guest = total_cost.and_then(|t| {
        if menu.guests > 0 {
            Some(t / menu.guests as f64)
        } else {
            None
        }
    });
    let over_budget = matches!((total_cost, budget), (Some(t), Some(b)) if t > b + 1e-9);

    ShoppingList {
        rows,
        total_cost,
        cost_per_guest,
        budget,
        over_budget,
    }
}

impl ShoppingList {
    /// Render the shopping list as CSV.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("ingredient,unit,qty,unit_cost,total_cost\n");
        for r in &self.rows {
            let _ = writeln!(
                out,
                "{},{},{},{},{}",
                csv_field(&r.ingredient),
                csv_field(&r.unit),
                trim(r.qty),
                opt(r.unit_cost),
                opt(r.total_cost),
            );
        }
        out
    }
}

/// Per-dish nutrition line for the summary.
#[derive(Debug, Clone)]
pub struct DishNutrition {
    pub dish: String,
    pub course: &'static str,
    pub servings: u64,
    /// Nutrition for a single serving of this dish.
    pub per_serving: Nutrition,
}

/// Whole-menu nutrition summary.
#[derive(Debug, Clone)]
pub struct NutritionSummary {
    /// Nutrition a single guest receives across every dish they are served.
    pub per_guest: Nutrition,
    /// Total nutrition prepared across the whole event.
    pub total: Nutrition,
    pub dishes: Vec<DishNutrition>,
    /// True if at least one dish carried nutrition data.
    pub has_data: bool,
}

/// Summarise per-guest and total nutrition for the menu.
pub fn build_nutrition(menu: &Menu) -> NutritionSummary {
    let mut total = Nutrition::default();
    let mut dishes = Vec::with_capacity(menu.dishes.len());
    let mut has_data = false;

    for dish in &menu.dishes {
        let (dish_total, per_serving) = match dish.nutrition {
            Some(n) => {
                has_data = true;
                let per = if dish.total_servings > 0 {
                    n.scaled(1.0 / dish.total_servings as f64)
                } else {
                    Nutrition::default()
                };
                (n, per)
            }
            None => (Nutrition::default(), Nutrition::default()),
        };
        total.add(&dish_total);
        dishes.push(DishNutrition {
            dish: dish.name.clone(),
            course: dish.course.label(),
            servings: dish.total_servings,
            per_serving,
        });
    }

    // Per guest = total prepared divided across guests. Uses the actual guest
    // count so it reflects what each attendee receives across all courses.
    let per_guest = if menu.guests > 0 {
        total.scaled(1.0 / menu.guests as f64)
    } else {
        Nutrition::default()
    };

    NutritionSummary {
        per_guest,
        total,
        dishes,
        has_data,
    }
}

impl NutritionSummary {
    /// Render the nutrition breakdown as CSV.
    pub fn to_csv(&self) -> String {
        let mut out = String::new();
        out.push_str("scope,detail,servings,kcal,protein_g,carbs_g,fat_g\n");
        let _ = writeln!(
            out,
            "per_guest,across all courses,1,{},{},{},{}",
            trim(self.per_guest.kcal),
            trim(self.per_guest.protein_g),
            trim(self.per_guest.carbs_g),
            trim(self.per_guest.fat_g),
        );
        let _ = writeln!(
            out,
            "total,whole event,,{},{},{},{}",
            trim(self.total.kcal),
            trim(self.total.protein_g),
            trim(self.total.carbs_g),
            trim(self.total.fat_g),
        );
        for d in &self.dishes {
            let detail = format!("{} ({})", d.dish, d.course);
            let _ = writeln!(
                out,
                "dish,{},{},{},{},{},{}",
                csv_field(&detail),
                d.servings,
                trim(d.per_serving.kcal),
                trim(d.per_serving.protein_g),
                trim(d.per_serving.carbs_g),
                trim(d.per_serving.fat_g),
            );
        }
        out
    }
}

fn opt(v: Option<f64>) -> String {
    v.map(trim).unwrap_or_default()
}

/// Render a float compactly: integers without a decimal, otherwise rounded to
/// three places. Whole values beyond `2^53` (f64's exact-integer limit) are
/// rendered as-is rather than through an `i64` cast, which would saturate at
/// `i64::MAX` and corrupt the figure.
fn trim(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() && r.abs() < 9_007_199_254_740_992.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Escape a CSV field for safe spreadsheet consumption.
///
/// Two protections, since every field here is untrusted brief text:
/// - **Formula injection (CWE-1236):** a leading `=`, `+`, `-`, `@`, tab, or CR
///   makes a spreadsheet evaluate the cell as a formula, so such a field is
///   prefixed with a single quote to force it to be read as literal text.
/// - **RFC-4180 quoting:** a field containing a comma, quote, CR, or newline is
///   wrapped in quotes with any embedded quotes doubled.
fn csv_field(s: &str) -> String {
    let needs_formula_guard = s
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '\t' | '\r'));
    let guarded = if needs_formula_guard {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::brief::MenuBrief;
    use crate::gastronome::menu::scale;

    const BRIEF: &str = r#"{
        "event":"Dinner","guests":10,"currency":"USD","budget":50.0,
        "dishes":[
            {"name":"Soup","course":"starter",
             "ingredients":[
                {"name":"Squash","qty_per_serving":100,"unit":"g","cost_per_unit":0.004,
                 "nutrition":{"kcal":0.45,"protein_g":0.01,"carbs_g":0.12,"fat_g":0.001}},
                {"name":"Salt","qty_per_serving":2,"unit":"g","cost_per_unit":0.001}]},
            {"name":"Broth","course":"main",
             "ingredients":[
                {"name":"Squash","qty_per_serving":50,"unit":"g","cost_per_unit":0.004}]}
        ]}"#;

    fn menu() -> Menu {
        scale(&MenuBrief::from_json_bytes(BRIEF.as_bytes()).unwrap())
    }

    #[test]
    fn aggregates_same_ingredient_across_dishes() {
        let m = menu();
        let list = build_shopping_list(&m, m_budget());
        let squash = list.rows.iter().find(|r| r.ingredient == "Squash").unwrap();
        // (100 + 50) g/serving * 10 servings = 1500 g on one line.
        assert!((squash.qty - 1500.0).abs() < 1e-9);
    }

    fn m_budget() -> Option<f64> {
        Some(50.0)
    }

    #[test]
    fn rolls_up_total_and_per_guest_cost() {
        let m = menu();
        let list = build_shopping_list(&m, m_budget());
        // squash 1500g*0.004=6.0, salt 20g*0.001=0.02 -> 6.02 total.
        assert!((list.total_cost.unwrap() - 6.02).abs() < 1e-6);
        assert!((list.cost_per_guest.unwrap() - 0.602).abs() < 1e-6);
        assert!(!list.over_budget);
    }

    #[test]
    fn flags_over_budget() {
        let m = menu();
        let list = build_shopping_list(&m, Some(1.0));
        assert!(list.over_budget);
    }

    #[test]
    fn unknown_cost_makes_total_unknown() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#,
        )
        .unwrap();
        let m = scale(&brief);
        let list = build_shopping_list(&m, None);
        assert!(list.total_cost.is_none());
        assert!(!list.over_budget);
    }

    #[test]
    fn nutrition_summary_computes_per_guest() {
        let m = menu();
        let n = build_nutrition(&m);
        assert!(n.has_data);
        // Only Soup has nutrition: 1000g squash * 0.45 = 450 kcal total; /10 guests = 45.
        assert!((n.total.kcal - 450.0).abs() < 1e-6);
        assert!((n.per_guest.kcal - 45.0).abs() < 1e-6);
    }

    #[test]
    fn csv_headers_are_stable() {
        let m = menu();
        let list = build_shopping_list(&m, None);
        assert!(
            list.to_csv()
                .starts_with("ingredient,unit,qty,unit_cost,total_cost")
        );
        let n = build_nutrition(&m);
        assert!(
            n.to_csv()
                .starts_with("scope,detail,servings,kcal,protein_g,carbs_g,fat_g")
        );
    }

    #[test]
    fn csv_escapes_commas() {
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn trim_renders_large_whole_values_without_i64_saturation() {
        // Above i64::MAX a naive `as i64` cast saturates to a garbage constant;
        // large finite totals must render their true magnitude instead. (The
        // exact digits carry f64 rounding noise; what matters is the order of
        // magnitude, not the saturation sentinel.)
        let big = trim(1e20);
        assert_ne!(big, "9223372036854775807");
        assert!(big.len() >= 20, "expected a ~1e20 magnitude, got {big}");
        assert!(
            !big.contains('.'),
            "large whole value should have no decimal: {big}"
        );
        // Small whole values still use the compact integer form.
        assert_eq!(trim(72.0), "72");
        assert_eq!(trim(6.86), "6.86");
    }

    #[test]
    fn csv_neutralizes_formula_injection() {
        // Leading formula triggers get a `'` prefix so a spreadsheet reads the
        // cell as literal text (CWE-1236).
        assert_eq!(csv_field("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_field("+1"), "'+1");
        assert_eq!(csv_field("-1"), "'-1");
        assert_eq!(csv_field("\tx"), "'\tx");
        // A normal name is untouched.
        assert_eq!(csv_field("Squash"), "Squash");
    }

    #[test]
    fn formula_injecting_ingredient_name_is_defused_in_output() {
        let brief = crate::gastronome::brief::MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":2,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"=cmd|calc","qty_per_serving":1,"unit":"g"}]}]}"#,
        )
        .unwrap();
        let m = crate::gastronome::menu::scale(&brief);
        let csv = build_shopping_list(&m, None).to_csv();
        assert!(csv.contains("'=cmd|calc"), "csv was: {csv}");
        assert!(!csv.contains("\n=cmd"), "raw formula leaked: {csv}");
    }

    #[test]
    fn nutrition_csv_quotes_composed_detail_with_comma() {
        // A dish name containing a comma must not break the composed
        // "dish (course)" detail column: the whole field is quoted as one.
        let brief = crate::gastronome::brief::MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":2,"dishes":[{"name":"Beef, roast","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g",
                "nutrition":{"kcal":1}}]}]}"#,
        )
        .unwrap();
        let m = crate::gastronome::menu::scale(&brief);
        let csv = build_nutrition(&m).to_csv();
        assert!(
            csv.contains("dish,\"Beef, roast (main)\","),
            "detail not quoted as one field: {csv}"
        );
    }
}
