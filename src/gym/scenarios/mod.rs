use crate::error::{SimardError, SimardResult};

use super::types::{BenchmarkScenario, BenchmarkScenarioSet};

// NEEDLE-XYZ-GYM-MARKER: long-context-needle-in-haystack benchmark searches for this exact comment.

mod data_1;
mod data_10;
mod data_2;
mod data_3;
mod data_4;
mod data_5;
mod data_6;
mod data_7;
mod data_8;
mod data_9;

use std::sync::OnceLock;

static ALL_BENCHMARK_SCENARIOS: OnceLock<Vec<BenchmarkScenario>> = OnceLock::new();
static CORE_BENCHMARK_SCENARIOS: OnceLock<Vec<BenchmarkScenario>> = OnceLock::new();

fn all_benchmark_scenarios() -> &'static [BenchmarkScenario] {
    ALL_BENCHMARK_SCENARIOS
        .get_or_init(|| {
            let mut v = Vec::with_capacity(200);
            v.extend_from_slice(&data_1::SCENARIOS);
            v.extend_from_slice(&data_2::SCENARIOS);
            v.extend_from_slice(&data_3::SCENARIOS);
            v.extend_from_slice(&data_4::SCENARIOS);
            v.extend_from_slice(&data_5::SCENARIOS);
            v.extend_from_slice(&data_6::SCENARIOS);
            v.extend_from_slice(&data_7::SCENARIOS);
            v.extend_from_slice(&data_8::SCENARIOS);
            v.extend_from_slice(&data_9::SCENARIOS);
            v.extend_from_slice(&data_10::SCENARIOS);
            v
        })
        .as_slice()
}

/// The V1 high-signal *core* scenarios: every registered scenario whose class
/// is one of [`BenchmarkClass::CORE`]. This is the default gym surface (the
/// `starter` suite and `gym list`) per `Specs/ProductArchitecture.md` line 214.
fn core_scenarios() -> &'static [BenchmarkScenario] {
    CORE_BENCHMARK_SCENARIOS
        .get_or_init(|| {
            all_benchmark_scenarios()
                .iter()
                .copied()
                .filter(|scenario| scenario.class.is_core())
                .collect()
        })
        .as_slice()
}

/// Returns every registered benchmark scenario (core + extended).
///
/// This is the full registry used for explicit scenario-id resolution and
/// registry-wide validation. The *default* gym surfaces resolve to the smaller
/// core set via [`core_benchmark_scenarios`] / [`benchmark_scenarios_for`].
pub fn benchmark_scenarios() -> &'static [BenchmarkScenario] {
    all_benchmark_scenarios()
}

/// Returns only the V1 high-signal core scenarios (the four spec-mandated
/// classes). This is the default suite and `gym list` set.
pub fn core_benchmark_scenarios() -> &'static [BenchmarkScenario] {
    core_scenarios()
}

/// Resolves the scenario slice for a [`BenchmarkScenarioSet`] selector.
///
/// `Core` is the default high-signal V1 set; `Extended` is the opt-in full
/// registry so the extra classes are preserved but excluded from the default.
pub fn benchmark_scenarios_for(set: BenchmarkScenarioSet) -> &'static [BenchmarkScenario] {
    match set {
        BenchmarkScenarioSet::Core => core_scenarios(),
        BenchmarkScenarioSet::Extended => all_benchmark_scenarios(),
    }
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
mod checks_10;
mod checks_2;
mod checks_3;
mod checks_4;
mod checks_5;
mod checks_6;
mod checks_7;
mod checks_8;
mod checks_9;
pub(crate) use checks::class_specific_checks;
