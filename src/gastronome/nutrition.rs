//! Nutrition accounting for the Gastronome identity.
//!
//! A [`Nutrition`] value is a small, additive macronutrient vector. It carries
//! calories plus the three macronutrients a menu planner reasons about
//! (protein, carbohydrate, fat). The type is deliberately closed and numeric so
//! that scaling a recipe, summing a menu, and reporting per-guest values are all
//! plain arithmetic with no domain surprises.

use std::ops::Add;

use serde::{Deserialize, Serialize};

/// A macronutrient profile. All fields are non-negative and expressed in the
/// units a recipe author works in: `kcal` for energy and grams for the macros.
///
/// `Nutrition` is additive (via the [`Add`] operator or the point-free
/// [`Nutrition::sum2`]) and scalable
/// ([`Nutrition::scale`]) so a menu's total is the sum of its dishes and a
/// dish's total is one serving scaled by the number of servings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Nutrition {
    /// Energy in kilocalories.
    pub calories: f64,
    /// Protein in grams.
    pub protein_g: f64,
    /// Carbohydrate in grams.
    pub carbs_g: f64,
    /// Fat in grams.
    pub fat_g: f64,
}

impl Nutrition {
    /// Construct a nutrition vector from its four components.
    #[must_use]
    pub fn new(calories: f64, protein_g: f64, carbs_g: f64, fat_g: f64) -> Self {
        Self {
            calories,
            protein_g,
            carbs_g,
            fat_g,
        }
    }

    /// Component-wise sum of two profiles.
    ///
    /// Equivalent to the [`Add`] operator; kept as an inherent name so it can be
    /// used point-free (e.g. `iter.fold(Nutrition::default(), Nutrition::sum2)`).
    #[must_use]
    pub fn sum2(self, other: Self) -> Self {
        self + other
    }

    /// Scale every component by `factor` (e.g. servings or a quantity).
    #[must_use]
    pub fn scale(self, factor: f64) -> Self {
        Self {
            calories: self.calories * factor,
            protein_g: self.protein_g * factor,
            carbs_g: self.carbs_g * factor,
            fat_g: self.fat_g * factor,
        }
    }

    /// Round every component to two decimal places for stable reporting.
    #[must_use]
    pub fn rounded(self) -> Self {
        let r = |v: f64| (v * 100.0).round() / 100.0;
        Self {
            calories: r(self.calories),
            protein_g: r(self.protein_g),
            carbs_g: r(self.carbs_g),
            fat_g: r(self.fat_g),
        }
    }
}

impl Add for Nutrition {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            calories: self.calories + other.calories,
            protein_g: self.protein_g + other.protein_g,
            carbs_g: self.carbs_g + other.carbs_g,
            fat_g: self.fat_g + other.fat_g,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_zero() {
        assert_eq!(Nutrition::default(), Nutrition::new(0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn add_is_component_wise() {
        let a = Nutrition::new(100.0, 10.0, 20.0, 5.0);
        let b = Nutrition::new(50.0, 4.0, 6.0, 2.0);
        assert_eq!(a + b, Nutrition::new(150.0, 14.0, 26.0, 7.0));
        assert_eq!(a.sum2(b), Nutrition::new(150.0, 14.0, 26.0, 7.0));
    }

    #[test]
    fn add_is_commutative() {
        let a = Nutrition::new(1.0, 2.0, 3.0, 4.0);
        let b = Nutrition::new(5.0, 6.0, 7.0, 8.0);
        assert_eq!(a + b, b + a);
    }

    #[test]
    fn scale_multiplies_each_component() {
        let n = Nutrition::new(100.0, 10.0, 20.0, 5.0);
        assert_eq!(n.scale(2.5), Nutrition::new(250.0, 25.0, 50.0, 12.5));
    }

    #[test]
    fn scale_by_zero_zeroes_out() {
        let n = Nutrition::new(100.0, 10.0, 20.0, 5.0);
        assert_eq!(n.scale(0.0), Nutrition::default());
    }

    #[test]
    fn rounded_clamps_to_two_decimals() {
        let n = Nutrition::new(100.126, 10.0, 20.005, 5.994);
        assert_eq!(n.rounded(), Nutrition::new(100.13, 10.0, 20.01, 5.99));
    }
}
