//! Self-improvement loop that evaluates, analyzes, and decides on changes.
//!
//! The improvement cycle follows a disciplined sequence:
//! `Eval -> Analyze -> Research -> Improve -> ReEval -> Decide`.
//!
//! Each cycle produces a typed [`ImprovementCycle`] record with full
//! provenance so decisions are reviewable (Pillar 6). Changes are only
//! committed when the net improvement meets the threshold and no single
//! dimension regresses beyond the allowed maximum (Pillar 11).

pub(crate) mod cycle;
pub mod history;
pub mod prioritization;
pub mod trend;
mod types;

#[cfg(test)]
mod tests_cycle;

#[cfg(test)]
mod tests_history;

#[cfg(test)]
mod tests_prioritization;

#[cfg(test)]
mod tests_trend;

// Re-export all public items so `crate::self_improve::X` still works.
pub use cycle::{
    apply_improvements, decide, find_weak_dimensions, run_improvement_cycle, summarize_cycle,
};
pub use history::ImprovementHistory;
pub use prioritization::{
    PrioritizedDimension, PriorityWeights, find_weak_dimensions_detailed, prioritize_dimensions,
    prioritize_dimensions_default,
};
pub use trend::{CycleTrend, DimensionTrend, analyze_trends, rank_dimensions_by_priority};
pub use types::{
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange,
    WeakDimension,
};
