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
mod types;

// Re-export all public items so `crate::self_improve::X` still works.
pub use cycle::{apply_improvements, run_improvement_cycle, summarize_cycle};
pub use types::{
    ImprovementConfig, ImprovementCycle, ImprovementDecision, ImprovementPhase, ProposedChange,
};
