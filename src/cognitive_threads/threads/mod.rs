//! Concrete [`super::CognitiveThread`] implementations.
//!
//! - [`OodaThread`] — the primary loop (kind = `Ooda`, priority = `Critical`).
//! - [`MaintenanceThread`] — safe housekeeping (exemplar 1).
//! - [`EngineerLogAnalysisThread`] — improvement finder (exemplar 2).

pub mod engineer_log_analysis;
pub mod maintenance;
pub mod ooda;

// Issue #2419 (design spike) / #2647 (wiring): the Creative Ideas generator
// thread — reuses `ThreadKind::BackgroundThought`, default-ON opt-out, and
// registered with the `Mind` by the OODA daemon at startup.
pub mod creative_ideas;

pub use creative_ideas::CreativeIdeasThread;
pub use engineer_log_analysis::{EngineerLogAnalysisConfig, EngineerLogAnalysisThread};
pub use maintenance::{MaintenanceConfig, MaintenanceThread};
pub use ooda::OodaThread;
