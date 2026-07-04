//! Concrete [`super::CognitiveThread`] implementations.
//!
//! - [`OodaThread`] — the primary loop (kind = `Ooda`, priority = `Critical`).
//! - [`MaintenanceThread`] — safe housekeeping (exemplar 1).
//! - [`EngineerLogAnalysisThread`] — improvement finder (exemplar 2).

pub mod engineer_log_analysis;
pub mod maintenance;
pub mod ooda;

pub use engineer_log_analysis::{EngineerLogAnalysisConfig, EngineerLogAnalysisThread};
pub use maintenance::{MaintenanceConfig, MaintenanceThread};
pub use ooda::OodaThread;
