//! Recipe scaling: turn a per-serving brief into concrete totals.
//!
//! [`scale`] is the deterministic core of the pipeline (no external tools): it
//! multiplies every recipe up to the guest count, rounding servings to whole
//! portions, and carries per-ingredient cost and nutrition through the scale so
//! the downstream analysis, scheduling, and card stages work from a single
//! source of truth.

use super::brief::{Course, MenuBrief, Nutrition};

/// One ingredient scaled to the whole event.
#[derive(Debug, Clone)]
pub struct ScaledIngredient {
    pub name: String,
    pub unit: String,
    /// Total quantity needed across the whole event, in `unit`.
    pub total_qty: f64,
    /// Cost of one `unit`, if the brief supplied it.
    pub cost_per_unit: Option<f64>,
    /// Total cost for this ingredient (`total_qty * cost_per_unit`), if known.
    pub total_cost: Option<f64>,
    /// Total nutrition contributed by this ingredient, if the brief supplied it.
    pub nutrition: Option<Nutrition>,
}

/// One dish scaled to the whole event.
#[derive(Debug, Clone)]
pub struct ScaledDish {
    pub name: String,
    pub course: Course,
    /// Whole servings to produce (guests × servings-per-guest, rounded up).
    pub total_servings: u64,
    pub ingredients: Vec<ScaledIngredient>,
    pub tags: Vec<String>,
    /// Total nutrition for the whole dish across all servings.
    pub nutrition: Option<Nutrition>,
    /// Total cost for the whole dish, if every ingredient carried a cost.
    pub total_cost: Option<f64>,
}

/// A fully scaled menu, ready for analysis, scheduling, and presentation.
#[derive(Debug, Clone)]
pub struct Menu {
    pub event: String,
    pub guests: u32,
    pub currency: String,
    pub dishes: Vec<ScaledDish>,
}

impl Menu {
    /// Total whole servings produced across every dish.
    pub fn total_servings(&self) -> u64 {
        self.dishes.iter().map(|d| d.total_servings).sum()
    }

    /// Number of distinct courses present on the menu.
    pub fn course_count(&self) -> usize {
        let mut seen: Vec<Course> = Vec::new();
        for d in &self.dishes {
            if !seen.contains(&d.course) {
                seen.push(d.course);
            }
        }
        seen.len()
    }
}

/// Whole servings to produce for a dish: guests × servings-per-guest, rounded
/// up to a whole portion (you cannot plate a fraction of a serving).
fn whole_servings(guests: u32, servings_per_guest: f64) -> u64 {
    let raw = guests as f64 * servings_per_guest;
    raw.ceil().max(1.0) as u64
}

/// Scale a validated brief up to concrete event-wide totals.
pub fn scale(brief: &MenuBrief) -> Menu {
    let mut dishes = Vec::with_capacity(brief.dishes.len());

    for dish in &brief.dishes {
        let servings = whole_servings(brief.guests, dish.servings_per_guest());
        let servings_f = servings as f64;

        let mut ingredients = Vec::with_capacity(dish.ingredients.len());
        let mut dish_nutrition = Nutrition::default();
        let mut has_nutrition = false;
        let mut dish_cost = 0.0;
        let mut costed_all = !dish.ingredients.is_empty();

        for ing in &dish.ingredients {
            let total_qty = ing.qty_per_serving * servings_f;
            let total_cost = ing.cost_per_unit.map(|c| c * total_qty);
            match total_cost {
                Some(c) => dish_cost += c,
                None => costed_all = false,
            }
            let nutrition = ing.nutrition.map(|n| n.scaled(total_qty));
            if let Some(n) = &nutrition {
                dish_nutrition.add(n);
                has_nutrition = true;
            }
            ingredients.push(ScaledIngredient {
                name: ing.name.clone(),
                unit: ing.unit.clone(),
                total_qty,
                cost_per_unit: ing.cost_per_unit,
                total_cost,
                nutrition,
            });
        }

        dishes.push(ScaledDish {
            name: dish.name.clone(),
            course: dish.normalized_course(),
            total_servings: servings,
            ingredients,
            tags: dish.tags.clone(),
            nutrition: has_nutrition.then_some(dish_nutrition),
            total_cost: costed_all.then_some(dish_cost),
        });
    }

    Menu {
        event: brief.event.clone(),
        guests: brief.guests,
        currency: brief.currency().to_string(),
        dishes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gastronome::brief::MenuBrief;

    const BRIEF: &str = r#"{
        "event":"Dinner","guests":10,"currency":"USD",
        "dishes":[
            {"name":"Soup","course":"starter",
             "ingredients":[
                {"name":"Squash","qty_per_serving":200,"unit":"g","cost_per_unit":0.004,
                 "nutrition":{"kcal":0.45,"protein_g":0.01,"carbs_g":0.12,"fat_g":0.001}}]},
            {"name":"Roast","course":"main","servings_per_guest":1.5,
             "ingredients":[
                {"name":"Beef","qty_per_serving":180,"unit":"g","cost_per_unit":0.02}]}
        ]}"#;

    fn menu() -> Menu {
        let brief = MenuBrief::from_json_bytes(BRIEF.as_bytes()).unwrap();
        scale(&brief)
    }

    #[test]
    fn scales_quantities_by_servings() {
        let m = menu();
        let soup = &m.dishes[0];
        assert_eq!(soup.total_servings, 10);
        assert!((soup.ingredients[0].total_qty - 2000.0).abs() < 1e-9);
        assert!((soup.ingredients[0].total_cost.unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn rounds_fractional_servings_up() {
        // 10 guests * 1.5 servings = 15 whole servings.
        let m = menu();
        let roast = &m.dishes[1];
        assert_eq!(roast.total_servings, 15);
        assert!((roast.ingredients[0].total_qty - 2700.0).abs() < 1e-9);
    }

    #[test]
    fn rolls_up_dish_nutrition_and_cost() {
        let m = menu();
        let soup = &m.dishes[0];
        let n = soup.nutrition.unwrap();
        // 2000 g * 0.45 kcal/g = 900 kcal.
        assert!((n.kcal - 900.0).abs() < 1e-6);
        assert!((soup.total_cost.unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn missing_cost_leaves_dish_cost_unknown() {
        let brief = MenuBrief::from_json_bytes(
            br#"{"event":"x","guests":4,"dishes":[{"name":"d","course":"main",
                "ingredients":[{"name":"i","qty_per_serving":1,"unit":"g"}]}]}"#,
        )
        .unwrap();
        let m = scale(&brief);
        assert!(m.dishes[0].total_cost.is_none());
        assert!(m.dishes[0].nutrition.is_none());
    }

    #[test]
    fn totals_and_course_count() {
        let m = menu();
        assert_eq!(m.total_servings(), 25);
        assert_eq!(m.course_count(), 2);
    }
}
