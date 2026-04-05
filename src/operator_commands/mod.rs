mod dispatch;
mod evidence;
mod format;
mod goals;
mod probe;
mod recipes;
mod state_root;
mod validation;

// Re-export all public functions from sibling operator_commands_* modules.
pub use crate::operator_commands_engineer::{run_engineer_loop_probe, run_engineer_read_probe};
pub use crate::operator_commands_gym::{
    run_gym_compare, run_gym_list, run_gym_scenario, run_gym_suite,
};
pub use crate::operator_commands_meeting::{
    run_goal_curation_probe, run_goal_curation_read_probe, run_improvement_curation_probe,
    run_improvement_curation_read_probe, run_meeting_probe, run_meeting_read_probe,
};
pub use crate::operator_commands_review::{run_review_probe, run_review_read_probe};
pub use crate::operator_commands_terminal::{
    run_terminal_probe, run_terminal_probe_from_file, run_terminal_read_probe,
    run_terminal_recipe_list_probe, run_terminal_recipe_probe, run_terminal_recipe_show_probe,
};

// Re-export pub items from sub-modules.
pub use dispatch::{dispatch_legacy_gym_cli, dispatch_operator_probe, gym_usage};
pub use probe::{run_bootstrap_probe, run_copilot_submit_probe, run_handoff_probe};

// Re-export pub(crate) items from sub-modules so callers using
// `crate::operator_commands::<item>` continue to compile.
pub(crate) use evidence::{
    load_terminal_objective_file, optional_terminal_evidence_value,
    render_redacted_objective_metadata, required_terminal_evidence_value, terminal_evidence_values,
};
pub(crate) use format::{
    print_display, print_goal_section, print_meeting_goal_section, print_string_section,
    print_terminal_bridge_section, print_text,
};
pub(crate) use goals::GoalRegisterView;
pub(crate) use recipes::{
    ensure_terminal_recipe_is_runnable, list_terminal_recipe_descriptors, load_terminal_recipe,
    print_terminal_recipe,
};
pub(crate) use state_root::{
    parse_runtime_topology, prompt_root, resolved_engineer_read_state_root,
    resolved_goal_curation_state_root, resolved_improvement_curation_read_state_root,
    resolved_meeting_read_state_root, resolved_review_state_root, resolved_state_root,
    resolved_terminal_read_state_root,
};
pub(crate) use validation::{validated_engineer_read_artifacts, validated_terminal_read_artifacts};
