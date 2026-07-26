//! Cognitive-thread scheduling: a [`Mind`] runs many [`CognitiveThread`]s on
//! their own cadence/trigger. See
//! `docs/reference/cognitive-thread-scheduling.md`.
//!
//! The scheduler (`schedule::*`, `Mind::{due_threads,run_due,health}`) and the
//! OODA/maintenance/engineer-log threads are implemented. Issue #5 adds the ten
//! reflective threads ([`threads`]) as thin rails over agentic recipes plus one
//! shared invoke seam ([`recipe_rail`]) and the salience→Decide durable signal
//! ([`salience_signal`]); each is OFF by default behind a double env gate. See
//! `docs/reference/cognitive-threads-catalog.md`.

mod mind;
mod schedule;
mod telemetry;
mod thread;
pub mod threads;

// Issue #5: the one shared brick (RecipeInvoker seam + security helpers) and
// the salience → Decide durable signal, both consumed by the ten reflective
// threads. See docs/reference/recipe-invoker-seam.md and
// docs/concepts/salience-and-decide.md.
pub mod recipe_rail;
pub mod salience_signal;

#[cfg(test)]
mod tests;

// Issue #5: TDD contract for the ten reflective threads + the shared seam.
#[cfg(test)]
mod tests_catalog;

#[cfg(test)]
mod tests_rework_contract;

pub use mind::Mind;
pub use thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
pub use threads::{EngineerLogAnalysisThread, MaintenanceThread, OodaThread};
