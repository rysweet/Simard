//! Rust domain-expertise experiment (roadmap #2491, first vertical slice).
//!
//! This module is the first evidenced data point for the "durable
//! domain-expertise acquisition & retention" roadmap (#2491), with Rust as the
//! first domain. It wires together the three roadmap pillars for one bounded
//! competency (ownership / the borrow checker + error handling):
//!
//! * **Acquisition** — a small in-process [`pack`] (the `rust-expert` knowledge
//!   pack: durable facts + reusable procedures, each with source provenance).
//! * **Retention** — the [`ingest`] that ingests the pack into Simard's
//!   cognitive memory so learned facts/procedures persist and can be recalled.
//! * **Measurement** — a Rust competency [`scenarios`] set graded by
//!   [`measurement`] into a per-domain scorecard with a novice → competent →
//!   expert placement and the issue-#1241 calibration guard.
//!
//! The end-to-end flow is deterministic and runs in-process (no external
//! network / LLM auth), so the baseline and pack-lift numbers are reproducible
//! in CI. See `docs/rust-expertise-gym.md`.

pub mod ingest;
pub mod measurement;
pub mod pack;
pub mod scenarios;

#[cfg(test)]
mod tests;

pub use ingest::{IngestReport, IngestScope, ingest_pack_into_memory, ingest_pack_scoped};
pub use measurement::{
    CompetencyLevel, RustScorecard, ScenarioResult, SubskillScore, calibration_gap, measure,
    run_baseline, run_degraded, run_with_pack,
};
pub use pack::{
    PackFact, PackProcedure, PackProvenance, RUST_EXPERT_PACK, RustExpertPack, SUBSKILLS,
};
pub use scenarios::{RustScenario, rust_scenarios};
