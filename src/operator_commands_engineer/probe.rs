use std::path::{Path, PathBuf};

use crate::operator_commands::{
    parse_runtime_topology, print_display, print_terminal_bridge_section, print_text,
    resolved_engineer_read_state_root, resolved_state_root,
};
use crate::run_local_engineer_loop;
use crate::terminal_engineer_bridge::{
    ENGINEER_MODE_BOUNDARY, SHARED_DEFAULT_STATE_ROOT_SOURCE, SHARED_EXPLICIT_STATE_ROOT_SOURCE,
};

use super::read_view::EngineerReadView;

pub fn run_engineer_loop_probe(
    topology: &str,
    workspace_root: &Path,
    objective: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let runtime_topology = parse_runtime_topology(topology)?;
    let state_root_was_explicit = state_root_override.is_some();
    let state_root = resolved_state_root(
        state_root_override,
        "simard-engineer",
        "terminal-shell",
        topology,
        "engineer-loop-run",
    )?;
    let run = run_local_engineer_loop(workspace_root, objective, runtime_topology, &state_root)
        .map_err(|error| format!("{error}"))?;

    println!("Probe mode: engineer-loop-run");
    print_text("Mode boundary", ENGINEER_MODE_BOUNDARY);
    print_display("Repo root", run.inspection.repo_root.display());
    print_text("Repo branch", &run.inspection.branch);
    print_text("Repo head", &run.inspection.head);
    println!("Worktree dirty: {}", run.inspection.worktree_dirty);
    println!(
        "Changed files: {}",
        if run.inspection.changed_files.is_empty() {
            "<none>".to_string()
        } else {
            run.inspection.changed_files.join(", ")
        }
    );
    println!("Active goals count: {}", run.inspection.active_goals.len());
    for (index, goal) in run.inspection.active_goals.iter().enumerate() {
        print_text(&format!("Active goal {}", index + 1), goal.concise_label());
    }
    println!(
        "Carried meeting decisions: {}",
        run.inspection.carried_meeting_decisions.len()
    );
    for (index, decision) in run.inspection.carried_meeting_decisions.iter().enumerate() {
        print_text(&format!("Carried meeting decision {}", index + 1), decision);
    }
    print_terminal_bridge_section(
        run.terminal_bridge_context.as_ref(),
        if state_root_was_explicit {
            SHARED_EXPLICIT_STATE_ROOT_SOURCE
        } else {
            SHARED_DEFAULT_STATE_ROOT_SOURCE
        },
    );
    print_text("Gap summary", &run.inspection.architecture_gap_summary);
    print_text("Execution scope", &run.execution_scope);
    print_text("Selected action", &run.action.selected.label);
    print_text("Action plan", &run.action.selected.plan_summary);
    print_text(
        "Verification steps",
        run.action.selected.verification_steps.join(" || "),
    );
    print_text("Action rationale", &run.action.selected.rationale);
    print_text("Action command", run.action.selected.argv.join(" "));
    println!("Action status: success");
    println!(
        "Changed files after action: {}",
        if run.action.changed_files.is_empty() {
            "<none>".to_string()
        } else {
            run.action.changed_files.join(", ")
        }
    );
    println!("Verification status: {}", run.verification.status);
    print_text("Verification summary", &run.verification.summary);
    println!("Elapsed duration: {:?}", run.elapsed_duration);
    println!("Phase traces: {}", run.phase_traces.len());
    for trace in &run.phase_traces {
        println!(
            "  Phase: {} | duration={:?} | outcome={:?}",
            trace.name, trace.duration, trace.outcome
        );
    }
    print_display("State root", run.state_root.display());
    Ok(())
}

pub fn run_engineer_read_probe(
    topology: &str,
    state_root_override: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_root = resolved_engineer_read_state_root(state_root_override, topology)?;
    let view = EngineerReadView::load(state_root)?;
    view.print();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- run_engineer_read_probe: error paths ---

    #[test]
    fn engineer_read_probe_rejects_nonexistent_state_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = run_engineer_read_probe("single-process", Some(missing));
        assert!(result.is_err(), "should fail for nonexistent state root");
    }

    #[test]
    fn engineer_read_probe_rejects_empty_state_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = run_engineer_read_probe("single-process", Some(dir.path().to_path_buf()));
        assert!(
            result.is_err(),
            "should fail when handoff artifacts missing"
        );
    }

    #[test]
    fn engineer_read_probe_invalid_topology() {
        let result = run_engineer_read_probe("invalid-topology", None);
        assert!(result.is_err(), "should fail for invalid topology");
    }

    // --- run_engineer_loop_probe: error paths ---

    #[test]
    fn engineer_loop_probe_invalid_topology() {
        let result = run_engineer_loop_probe(
            "invalid-topology",
            std::path::Path::new("/nonexistent"),
            "test objective",
            None,
        );
        assert!(result.is_err(), "should fail for invalid topology");
    }

    #[test]
    fn engineer_loop_probe_nonexistent_workspace() {
        let result = run_engineer_loop_probe(
            "single-process",
            std::path::Path::new("/nonexistent/workspace/path"),
            "test objective",
            None,
        );
        assert!(result.is_err(), "should fail for nonexistent workspace");
    }
}
