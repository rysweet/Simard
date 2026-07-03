//! Cognitive-thread scheduling: a [`Brain`] runs many [`CognitiveThread`]s on
//! their own cadence/trigger. See
//! `docs/reference/cognitive-thread-scheduling.md`.
//!
//! **Status (issue #2419):** this module is under TDD construction. The public
//! type surface (Appendix A of the design doc) is defined here so the test
//! suite in [`tests`] can pin behaviour first; the behaviour-bearing functions
//! (`schedule::*`, `Brain::{due_threads,run_due,health}`, each thread's `tick`
//! and its safety/analysis helpers) are `todo!()` stubs that the implementation
//! step fills in. Data types, constructors, and the telemetry seam are real.

mod brain;
mod schedule;
mod telemetry;
mod thread;
pub mod threads;

#[cfg(test)]
mod tests;

pub use brain::Brain;
pub use thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
pub use threads::{EngineerLogAnalysisThread, MaintenanceThread, OodaThread};
