//! Cognitive-thread scheduling: a [`Mind`] runs many [`CognitiveThread`]s on
//! their own cadence/trigger. See
//! `docs/reference/cognitive-thread-scheduling.md`.
//!
//! The scheduler (`schedule::*`, `Mind::{due_threads,run_due,health}`) and the
//! OODA/maintenance/engineer-log threads are implemented. Issue #5 adds the ten
//! reflective threads ([`threads`]) as thin rails over agentic recipes plus one
//! shared invoke seam ([`recipe_rail`]) and the salience→Decide durable signal
//! ([`salience_signal`]); each is ENABLED by default (opt-out) behind a
//! default-ON double env gate (issue #4845). See
//! `docs/reference/cognitive-thread-full-activation.md`.

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

// Issue #4970: TDD contract for the ThreadReasoningRecord typed handoff, its
// fail-closed reader, and the `run_reflective_thread` rail helper.
#[cfg(test)]
mod tests_thread_reasoning_record;

// Issue #4786: TDD contract for cognitive-thread observability instrumentation
// (per-thread OTel series + durable error propagation).
#[cfg(test)]
mod tests_thread_telemetry;

pub use mind::Mind;
pub use thread::{
    CognitiveThread, Priority, SchedulePolicy, ThreadContext, ThreadHealth, ThreadKind,
    ThreadOutcome,
};
pub use threads::{EngineerLogAnalysisThread, MaintenanceThread, OodaThread};
