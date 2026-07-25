mod commands;

#[cfg(test)]
mod tests;

// Re-export all public items so `crate::operator_commands_gym::X` still works.
pub use commands::{
    run_gym_compare, run_gym_enrichment_ablation, run_gym_list, run_gym_recall_precision,
    run_gym_scenario, run_gym_suite,
};

#[cfg(test)]
pub(crate) use commands::{run_gym_compare_with_root, run_gym_scenario_with_root};
