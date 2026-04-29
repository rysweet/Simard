use crate::error::{SimardError, SimardResult};

use super::types::BenchmarkScenario;

// NEEDLE-XYZ-GYM-MARKER: long-context-needle-in-haystack benchmark searches for this exact comment.

mod data_1;
mod data_2;
mod data_3;
mod data_4;
mod data_5;
mod data_6;

use std::sync::OnceLock;

static ALL_BENCHMARK_SCENARIOS: OnceLock<Vec<BenchmarkScenario>> = OnceLock::new();

fn all_benchmark_scenarios() -> &'static [BenchmarkScenario] {
    ALL_BENCHMARK_SCENARIOS
        .get_or_init(|| {
            let mut v = Vec::with_capacity(158);
            v.extend_from_slice(&data_1::SCENARIOS);
            v.extend_from_slice(&data_2::SCENARIOS);
            v.extend_from_slice(&data_3::SCENARIOS);
            v.extend_from_slice(&data_4::SCENARIOS);
            v.extend_from_slice(&data_5::SCENARIOS);
            v.extend_from_slice(&data_6::SCENARIOS);
            v
        })
        .as_slice()
}

pub fn benchmark_scenarios() -> &'static [BenchmarkScenario] {
    all_benchmark_scenarios()
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
mod checks_1;
mod checks_2;
mod checks_3;
mod checks_4;
mod checks_5;
mod checks_6;
pub(crate) use checks::class_specific_checks;
