use crate::error::{SimardError, SimardResult};

use super::types::BenchmarkScenario;

mod data;

/// Returns the curated V1 benchmark scenarios.
///
/// The returned slice has `'static` lifetime and contains a small, high-signal
/// set of [`BenchmarkScenario`]s covering the four sanctioned benchmark classes
/// (`repo-exploration`, `documentation`, `safe-code-change`, `session-quality`).
/// Per `Specs/ProductArchitecture.md` (line 214) V1 prefers a small benchmark
/// set with high signal over a large, noisy suite (issue #2087).
pub fn benchmark_scenarios() -> &'static [BenchmarkScenario] {
    &data::SCENARIOS
}

pub(super) fn resolve_benchmark_scenario(scenario_id: &str) -> SimardResult<BenchmarkScenario> {
    benchmark_scenarios()
        .iter()
        .copied()
        .find(|candidate| candidate.id == scenario_id)
        .ok_or_else(|| SimardError::BenchmarkScenarioNotFound {
            scenario_id: scenario_id.to_string(),
        })
}

mod checks;
pub(crate) use checks::class_specific_checks;
