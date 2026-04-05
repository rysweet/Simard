use std::path::PathBuf;

use crate::goals::{FileBackedGoalStore, GoalStore};
use crate::operator_commands::{
    GoalRegisterView, print_display, print_text, prompt_root, resolved_goal_curation_state_root,
};
use crate::{BootstrapConfig, BootstrapInputs, run_local_session};

pub fn run_goal_curation_probe(
    base_type: &str,
    topology: &str,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = "simard-goal-curator";
    let config = BootstrapConfig::resolve(BootstrapInputs {
        prompt_root: Some(prompt_root()),
        objective: Some(objective.to_string()),
        state_root: Some(resolved_goal_curation_state_root(
            state_root_override,
            base_type,
            topology,
        )?),
        identity: Some(identity.to_string()),
        base_type: Some(base_type.to_string()),
        topology: Some(topology.to_string()),
        ..BootstrapInputs::default()
    })?;

    let execution = run_local_session(&config)?;
    println!("Probe mode: goal-curation-run");
    println!("Identity: {}", execution.snapshot.identity_name);
    println!(
        "Selected base type: {}",
        execution.snapshot.selected_base_type
    );
    println!("Topology: {}", execution.snapshot.topology);
    print_display("State root", config.state_root_path().display());
    println!("Session phase: {}", execution.outcome.session.phase);
    println!(
        "Active goals count: {}",
        execution.snapshot.active_goal_count
    );
    for (index, goal) in execution.snapshot.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal);
    }
    print_text("Execution summary", &execution.outcome.execution_summary);
    print_text("Reflection summary", &execution.outcome.reflection.summary);
    Ok(())
}

pub fn run_goal_curation_read_probe(
    base_type: &str,
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_goal_curation_state_root(state_root_override, base_type, topology)?;
    let goal_store = FileBackedGoalStore::try_new(state_root.join("goal_records.json"))?;
    let goal_records = goal_store.list()?;
    let register = GoalRegisterView::from_records(goal_records);

    println!("Goal register: durable");
    print_text("Selected base type", base_type);
    print_text("Topology", topology);
    print_display("State root", state_root.display());
    register.print();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn goal_curation_read_probe_succeeds_with_empty_state() {
        let dir = TempDir::new().unwrap();
        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "expected success with empty state: {:?}",
            result.err()
        );
    }

    #[test]
    fn goal_curation_read_probe_with_missing_directory() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent");
        let result = run_goal_curation_read_probe("local-harness", "single-process", Some(missing));
        // FileBackedGoalStore::try_new handles missing files gracefully,
        // but the missing parent directory for state root resolution may error.
        // Either way, it should not panic.
        let _ = result;
    }

    #[test]
    fn goal_curation_read_probe_with_valid_goal_file() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("goal_records.json"), "[]").unwrap();
        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(
            result.is_ok(),
            "should succeed with empty goal file: {:?}",
            result.err()
        );
    }

    #[test]
    fn goal_curation_read_probe_with_empty_goal_records() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("goal_records.json"), "[]").unwrap();
        let result = run_goal_curation_read_probe(
            "local-harness",
            "single-process",
            Some(dir.path().to_path_buf()),
        );
        assert!(result.is_ok());
    }
}
