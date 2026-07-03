//! Cognitive-thread scheduling: a [`Mind`] runs many [`CognitiveThread`]s on
//! their own cadence/trigger. See
//! `docs/reference/cognitive-thread-scheduling.md`.
//!
//! **Status (issue #2419):** this module is under TDD construction. The public
//! type surface (Appendix A of the design doc) is defined here so the test
//! suite in [`tests`] can pin behaviour first; the behaviour-bearing functions
//! (`schedule::*`, `Mind::{due_threads,run_due,health}`, each thread's `tick`
//! and its safety/analysis helpers) are `todo!()` stubs that the implementation
//! step fills in. Data types, constructors, and the telemetry seam are real.

mod mind;
mod schedule;
mod telemetry;
mod thread;
pub mod threads;

#[cfg(test)]
mod tests;

pub use mind::Mind;
pub use thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
pub use threads::{EngineerLogAnalysisThread, MaintenanceThread, OodaThread};
