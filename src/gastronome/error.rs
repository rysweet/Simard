//! Error type for the Gastronome planning surface.
//!
//! The module keeps its own small, self-describing error enum rather than
//! reaching for the crate-wide `SimardError`: Gastronome is a self-contained
//! culinary planner whose failure modes (a requested course with no matching
//! recipe, an impossible serve time, a zero-guest brief) are domain-specific
//! and worth naming precisely for both the library API and the CLI.

use std::fmt::{self, Display, Formatter};

/// Something went wrong while turning a brief into a plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GastronomeError {
    /// The brief requested zero (or a nonsensical negative) guest count.
    InvalidGuestCount {
        /// The offending value.
        guests: i64,
    },
    /// A course in the brief could not be satisfied by any recipe in the book
    /// under the brief's dietary constraints.
    NoRecipeForCourse {
        /// The course name that went unfilled.
        course: String,
        /// Human-readable reason (e.g. the constraints that excluded matches).
        reason: String,
    },
    /// A recipe declared a base serving count that cannot scale (<= 0).
    InvalidBaseServings {
        /// The recipe id at fault.
        recipe: String,
        /// The offending base-serving value.
        base_servings: i64,
    },
    /// The serve time string could not be parsed as `HH:MM` (24-hour clock).
    InvalidServeTime {
        /// The raw value that failed to parse.
        value: String,
    },
    /// A prep step declared a negative duration.
    InvalidStepDuration {
        /// The recipe id at fault.
        recipe: String,
        /// The step description at fault.
        step: String,
        /// The offending duration in minutes.
        minutes: i64,
    },
}

impl Display for GastronomeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGuestCount { guests } => {
                write!(
                    f,
                    "invalid guest count {guests}: expected a positive number"
                )
            }
            Self::NoRecipeForCourse { course, reason } => {
                write!(f, "no recipe for course '{course}': {reason}")
            }
            Self::InvalidBaseServings {
                recipe,
                base_servings,
            } => write!(
                f,
                "recipe '{recipe}' has non-positive base_servings {base_servings}"
            ),
            Self::InvalidServeTime { value } => {
                write!(f, "invalid serve time '{value}': expected HH:MM (24-hour)")
            }
            Self::InvalidStepDuration {
                recipe,
                step,
                minutes,
            } => write!(
                f,
                "recipe '{recipe}' step '{step}' has negative duration {minutes} min"
            ),
        }
    }
}

impl std::error::Error for GastronomeError {}

/// Convenience alias for fallible Gastronome operations.
pub type GastronomeResult<T> = Result<T, GastronomeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_mentions_the_offending_value() {
        let e = GastronomeError::InvalidGuestCount { guests: 0 };
        assert!(e.to_string().contains('0'));

        let e = GastronomeError::NoRecipeForCourse {
            course: "main".into(),
            reason: "all vegan-excluded".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("main"));
        assert!(msg.contains("vegan-excluded"));
    }

    #[test]
    fn is_std_error() {
        fn assert_error<E: std::error::Error>(_: &E) {}
        assert_error(&GastronomeError::InvalidServeTime {
            value: "25:99".into(),
        });
    }
}
