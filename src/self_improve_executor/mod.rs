//! Auto-apply executor for self-improvement proposals.
//!
//! Closes the loop on [`crate::self_improve`] by generating plans from
//! improvement proposals, executing them, running LLM review, and committing
//! or rolling back based on review outcomes.

mod executor;
mod git_ops;
mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_extra;

// Re-export all public items so `crate::self_improve_executor::X` still works.
pub use executor::{apply_and_review, generate_patch, run_autonomous_improvement};
pub use types::{ApplyResult, ImprovementPatch};
