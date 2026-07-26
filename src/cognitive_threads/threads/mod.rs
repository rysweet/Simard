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

// Issue #5: the ten reflective threads. Each is a thin `CognitiveThread` rail
// over an agentic recipe (or, for interoception, deterministic sensing),
// scheduled by the shared `Mind` alongside OODA and OFF by default behind the
// double env gate. See `docs/reference/cognitive-threads-catalog.md`.
pub mod analogy;
pub mod consolidation;
pub mod interoception;
pub mod metacognition;
pub mod narrative;
pub mod operator_model;
pub mod prospection;
pub mod reflection;
pub mod salience;
pub mod values_deliberation;

pub use creative_ideas::CreativeIdeasThread;
pub use engineer_log_analysis::{EngineerLogAnalysisConfig, EngineerLogAnalysisThread};
pub use maintenance::{MaintenanceConfig, MaintenanceThread};
pub use ooda::OodaThread;

pub use analogy::{AnalogyConfig, AnalogyThread};
pub use consolidation::{ConsolidationConfig, ConsolidationThread};
pub use interoception::{InteroceptionConfig, InteroceptionThread};
pub use metacognition::{MetacognitionConfig, MetacognitionThread};
pub use narrative::{NarrativeConfig, NarrativeThread};
pub use operator_model::{OperatorModelConfig, OperatorModelThread};
pub use prospection::{ProspectionConfig, ProspectionThread};
pub use reflection::{ReflectionConfig, ReflectionThread};
pub use salience::{SalienceConfig, SalienceThread};
pub use values_deliberation::{ValuesDeliberationConfig, ValuesDeliberationThread};

/// Register the ten reflective threads (issue #5) with the shared [`Mind`].
///
/// Called from the daemon **only** when the master gate
/// `SIMARD_COGNITIVE_THREADS_ENABLED` is truthy, so with the gate unset nothing
/// registers. Each thread is additionally OFF by default behind its own
/// per-thread gate: a registered-but-disabled thread never ticks (the `Mind`
/// filters on `enabled()`), so registration here is safe and fully additive.
/// Nine are recipe rails built from the environment (repo + state roots);
/// interoception is recipe-free deterministic sensing.
pub fn register_reflective_threads(
    mind: &mut super::Mind,
    repo_root: &std::path::Path,
    state_root: &std::path::Path,
) {
    let repo = repo_root.to_path_buf();
    let state = state_root.to_path_buf();
    mind.register(Box::new(MetacognitionThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(ConsolidationThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(ReflectionThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(ProspectionThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(SalienceThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(OperatorModelThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(AnalogyThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(ValuesDeliberationThread::from_env(
        repo.clone(),
        state.clone(),
    )));
    mind.register(Box::new(NarrativeThread::from_env(repo, state)));
    mind.register(Box::new(InteroceptionThread::from_env()));
}
